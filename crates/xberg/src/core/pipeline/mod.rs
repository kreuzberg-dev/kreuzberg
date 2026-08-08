//! Post-processing pipeline orchestration.
//!
//! This module orchestrates the post-processing pipeline, executing validators,
//! quality processing, chunking, and custom hooks in the correct order.

mod cache;
mod execution;
pub(crate) mod features;
mod format;
mod initialization;
mod page_markers;

#[cfg(test)]
mod tests;

pub use cache::clear_processor_cache;
pub use format::apply_output_format;

use crate::Result;
use crate::core::config::ExtractionConfig;
use crate::types::ExtractedDocument;
use crate::types::internal::InternalDocument;

use execution::{execute_processor_stages, execute_validators};
use features::{execute_chunking, execute_language_detection, execute_token_reduction};
use initialization::{
    builtin_registration_error, get_processors_from_cache, initialize_features, initialize_processor_cache,
};

const CAPTIONING_PROCESSOR_NAME: &str = "captioning";
const BUILTIN_REGISTRATION_SOURCE: &str = "builtin_registration";

type PostProcessorHandle = std::sync::Arc<dyn crate::plugins::PostProcessor>;

fn processors_without_captioning(
    processors: &std::sync::Arc<Vec<PostProcessorHandle>>,
) -> std::sync::Arc<Vec<PostProcessorHandle>> {
    std::sync::Arc::new(
        processors
            .iter()
            .filter(|processor| processor.name() != CAPTIONING_PROCESSOR_NAME)
            .cloned()
            .collect(),
    )
}

/// Values produced by the captioning prepass that have no `InternalDocument`
/// destination and therefore cannot be merged back onto `doc`.
///
/// The prepass runs the captioning post-processor against a full
/// `ExtractedDocument` derived from `doc`, but the pipeline then re-derives the
/// final result from `doc`. Fields the derivation does not read back are carried
/// here and re-applied to the derived result instead of being discarded.
#[derive(Debug, Default)]
struct CaptioningCarryOver {
    /// Content authored by the prepass processor, when it rewrote `content`.
    content: Option<String>,
    /// Named entities produced by the prepass processor.
    entities: Option<Vec<crate::types::Entity>>,
}

impl CaptioningCarryOver {
    fn apply(self, result: &mut ExtractedDocument) {
        if let Some(content) = self.content {
            result.content = content;
        }
        if self.entities.is_some() {
            result.entities = self.entities;
        }
    }
}

/// Whether the prepass changed any image description, including by adding or
/// removing images. A change invalidates the extractor's pre-rendered content.
fn image_descriptions_changed(
    before: &[crate::types::ExtractedImage],
    after: Option<&Vec<crate::types::ExtractedImage>>,
) -> bool {
    let after = match after {
        Some(images) => images.as_slice(),
        None => &[],
    };
    before.len() != after.len()
        || before
            .iter()
            .zip(after)
            .any(|(retained, captioned)| retained.description != captioned.description)
}

/// Push a `ProcessingWarning` onto `doc` when the one-time built-in post-processor
/// registration pass (#271) reported a failure. `registration_error` is
/// `initialization::builtin_registration_error()`; `None` means every enabled
/// built-in processor registered successfully (or registration has not run yet).
///
/// Without this, a processor that failed to register (e.g. `summarization`) simply
/// never appears in the processor cache, so a caller who configured it saw a clean
/// `Ok` with no output for that stage and no indication why — the same class of gap
/// the captioning-only "processor missing" warning below closes for captioning.
fn push_builtin_registration_warning(doc: &mut InternalDocument, registration_error: Option<String>) {
    let Some(error) = registration_error else {
        return;
    };
    doc.processing_warnings.push(crate::types::ProcessingWarning {
        source: std::borrow::Cow::Borrowed(BUILTIN_REGISTRATION_SOURCE),
        message: std::borrow::Cow::Owned(format!(
            "built-in post-processor registration was incomplete ({error}); a configured \
             processor may silently produce no output for its stage"
        )),
    });
}

async fn run_captioning_prepass(
    doc: &mut InternalDocument,
    config: &ExtractionConfig,
    include_structure: bool,
    pp_config: &Option<&crate::core::config::PostProcessorConfig>,
    middle_processors: &std::sync::Arc<Vec<PostProcessorHandle>>,
) -> Result<CaptioningCarryOver> {
    if config.captioning.is_none() {
        return Ok(CaptioningCarryOver::default());
    }

    let captioning_processors = std::sync::Arc::new(
        middle_processors
            .iter()
            .filter(|processor| processor.name() == CAPTIONING_PROCESSOR_NAME)
            .cloned()
            .collect::<Vec<_>>(),
    );
    if captioning_processors.is_empty() {
        // Captioning was requested but no captioning processor is registered — the
        // feature is compiled out (e.g. the Docker `--features all` gap in #1382) or
        // otherwise unavailable. Surface it instead of silently no-opping.
        doc.processing_warnings.push(crate::types::ProcessingWarning {
            source: std::borrow::Cow::Borrowed("captioning"),
            message: std::borrow::Cow::Borrowed("captioning feature not enabled — rebuild with --features captioning"),
        });
        return Ok(CaptioningCarryOver::default());
    }

    crate::extraction::derive::resolve_relationships(doc);
    let mut caption_result = crate::extraction::derive::derive_extraction_result(
        doc.clone(),
        include_structure,
        config.output_format.clone(),
    );

    let content_before = caption_result.content.clone();

    execute_processor_stages(
        &mut caption_result,
        config,
        pp_config,
        &[(crate::plugins::ProcessingStage::Middle, captioning_processors)],
    )
    .await?;

    // Merge the prepass result back. Everything the processor produced that the
    // derivation re-reads goes onto `doc`; the rest is carried to the derived
    // result. Previously only description/caption, warnings and usage survived.
    let description_changed = image_descriptions_changed(&doc.images, caption_result.images.as_ref());
    // Replace the image vec wholesale instead of zipping it against `doc.images`:
    // a processor that adds or removes an image made `zip` truncate to the shorter
    // side, dropping added images and mis-pairing the rest.
    doc.images = caption_result.images.take().unwrap_or_default();
    if description_changed {
        doc.pre_rendered_content = None;
    }
    doc.metadata = std::mem::take(&mut caption_result.metadata);
    // #355: the prepass derives a full `ExtractedDocument` from `doc` above, and that
    // derivation destructively `.remove()`s `CODE_INTELLIGENCE_SCRATCH_KEY` from
    // `metadata.additional` (see `derive::derive_extraction_result`) so the raw
    // scratch payload never leaks into user-visible metadata. The `doc.metadata`
    // assignment just above then overwrites `doc`'s metadata with that
    // already-stripped copy, so the *second* derivation the pipeline runs later
    // (after this prepass returns) finds no scratch key and falls back to a
    // degraded `CodeMetadata`-only `code_intelligence` payload. Put the full
    // payload the first derivation already computed back under the scratch key so
    // the second derivation round-trips the same `.remove()` and reconstructs the
    // identical, non-degraded result.
    #[cfg(feature = "tree-sitter")]
    if let Some(code_intelligence) = caption_result.code_intelligence.take() {
        doc.metadata.additional.insert(
            std::borrow::Cow::Borrowed(crate::extractors::code::CODE_INTELLIGENCE_SCRATCH_KEY),
            code_intelligence,
        );
    }
    doc.uris = caption_result.uris.take().unwrap_or_default();
    doc.processing_warnings = std::mem::take(&mut caption_result.processing_warnings);
    doc.llm_usage = caption_result.llm_usage.take();

    Ok(CaptioningCarryOver {
        content: (caption_result.content != content_before).then(|| std::mem::take(&mut caption_result.content)),
        entities: caption_result.entities.take(),
    })
}

/// Run the post-processing pipeline on an `InternalDocument`.
///
/// Derives `ExtractedDocument` from `InternalDocument` via the derivation pipeline,
/// then executes post-processing in the following order:
/// 1. Post-Processors - Execute by stage (Early immediately; Middle/Late after a
///    provisional chunking pass, so chunk-aware post-processors see `result.chunks`)
/// 2. Language detection
/// 3. Quality Processing - Token reduction, NFC normalization, output-format application
/// 4. Validators - Run validation hooks on the processed result (can fail fast)
/// 5. Chunking (final) - Text splitting if enabled
///
/// Chunking runs **twice** when Middle/Late post-processors are active: once right
/// after Early post-processors (so a chunk-aware post-processor sees non-empty
/// `result.chunks`, per the tested contract in
/// `test_middle_postprocessors_run_after_explicit_chunking`), and again as the
/// **last** step, after every step that rewrites `content` (token reduction, NFC
/// normalization, output-format swapping). Only the final pass's `result.chunks`
/// is returned. Chunk `byte_start`/`byte_end` (and page-derived
/// `first_page`/`last_page`) are byte offsets into whatever `content` looked like at
/// chunking time, and are the join key consumers use for highlighting and
/// rehydration — running chunking only once, early, left those offsets silently
/// indexing stale content once later steps mutated it (#213). The first pass's
/// offsets are provisional; a Middle/Late post-processor must not treat them as
/// final. This trades an extra (usually cheap) chunking pass — and, if embeddings
/// are configured, a discarded first embedding batch — for offset correctness in
/// the returned result.
///
/// # Arguments
///
/// * `doc` - The internal document produced by the extractor
/// * `config` - Extraction configuration
///
/// # Returns
///
/// The processed extraction result.
///
/// # Errors
///
/// - Validator errors bubble up immediately
/// - Post-processor errors are caught and recorded in metadata
/// - System errors (IO, RuntimeError equivalents) always bubble up
#[cfg_attr(feature = "otel", tracing::instrument(
    skip(doc, config),
    fields(
        { crate::telemetry::conventions::OPERATION } = crate::telemetry::conventions::operations::PIPELINE,
        content.element_count = doc.elements.len(),
    )
))]
#[cfg_attr(alef, alef(skip))]
pub async fn run_pipeline(mut doc: InternalDocument, config: &ExtractionConfig) -> Result<ExtractedDocument> {
    doc.ocr_text_only = config.images.as_ref().map(|i| i.ocr_text_only).unwrap_or(false);
    doc.append_ocr_text = config.images.as_ref().map(|i| i.append_ocr_text).unwrap_or(false);
    doc.escape_markdown = config.escape_markdown;
    doc.table_anchors = config.table_anchors;
    doc.page_marker_format = config
        .pages
        .as_ref()
        .filter(|p| p.insert_page_markers)
        .map(|p| p.marker_format.clone());
    if let Some(format) = doc.page_marker_format.clone() {
        page_markers::inject_page_marker_elements(&mut doc, &format);
    }

    #[cfg(all(feature = "ocr", feature = "tokio-runtime"))]
    let image_ocr_enabled = config.images.as_ref().map(|i| i.run_ocr_on_images).unwrap_or(true);
    #[cfg(all(feature = "ocr", feature = "tokio-runtime"))]
    if image_ocr_enabled && config.ocr.is_some() && !doc.images.is_empty() {
        let images_to_process = std::mem::take(&mut doc.images);
        match crate::extraction::image_ocr::process_images_with_ocr(
            images_to_process,
            config,
            &mut doc.processing_warnings,
        )
        .await
        {
            Ok(processed) => {
                doc.images = processed;
            }
            Err(e) => {
                doc.processing_warnings.push(crate::types::ProcessingWarning {
                    source: std::borrow::Cow::Borrowed("image_ocr"),
                    message: std::borrow::Cow::Owned(format!("Image OCR failed: {e}")),
                });
            }
        }
    }

    replace_embedded_image_markdown_with_ocr(&mut doc);
    append_embedded_image_ocr_text(&mut doc);

    let pp_config = config.postprocessor.as_ref();
    let postprocessing_enabled = pp_config.is_none_or(|processor_config| processor_config.enabled);
    let processor_stages = if postprocessing_enabled {
        initialize_features();
        push_builtin_registration_warning(&mut doc, builtin_registration_error());
        initialize_processor_cache()?;

        let (early_processors, middle_processors, late_processors) = get_processors_from_cache()?;
        Some((early_processors, middle_processors, late_processors))
    } else {
        None
    };

    let include_structure = config.include_document_structure;
    let mut captioning_carry_over = CaptioningCarryOver::default();
    if let Some((_, middle_processors, _)) = &processor_stages {
        captioning_carry_over =
            run_captioning_prepass(&mut doc, config, include_structure, &pp_config, middle_processors).await?;
    }

    // Computed once, up front, from `doc` (independent of the later derivation and
    // content-mutating steps) and carried to the relocated `execute_chunking` call
    // near the end of this function — see the ordering note on `run_pipeline` (#213).
    #[cfg(feature = "chunking")]
    let chunker_heading_source: Option<String> = {
        let needs_markdown = config.chunking.as_ref().is_some_and(|c| {
            c.chunker_type == crate::core::config::ChunkerType::Markdown
                || c.resolve_preset().chunker_type == crate::core::config::ChunkerType::Markdown
        }) && config.output_format == crate::core::config::OutputFormat::Plain;
        if needs_markdown {
            Some(crate::rendering::render_markdown(&doc))
        } else {
            None
        }
    };
    #[cfg(not(feature = "chunking"))]
    let chunker_heading_source: Option<String> = None;

    #[cfg(feature = "html")]
    let styled_html_prerender: Option<String> = {
        use crate::plugins::InternalRenderer as _;
        if config.output_format == crate::core::config::OutputFormat::Html {
            config.html_output.as_ref().and_then(|html_cfg| {
                match crate::rendering::StyledHtmlRenderer::new(html_cfg.clone()) {
                    Ok(renderer) => match renderer.render(&doc) {
                        Ok(html) => Some(html),
                        Err(e) => {
                            tracing::warn!("StyledHtmlRenderer render failed, falling back to default HTML: {e}");
                            None
                        }
                    },
                    Err(e) => {
                        tracing::warn!("StyledHtmlRenderer construction failed, falling back to default HTML: {e}");
                        None
                    }
                }
            })
        } else {
            None
        }
    };

    let doc_for_elements = if config.result_format == crate::types::ResultFormat::ElementBased {
        Some(doc.clone())
    } else {
        None
    };

    let mut result =
        crate::extraction::derive::derive_extraction_result(doc, include_structure, config.output_format.clone());
    result.internal_document = doc_for_elements;
    captioning_carry_over.apply(&mut result);

    // #286: record the text the preserved element tree stands for, so the divergence check
    // below can tell whether post-processing has since made the tree a stale second copy of
    // the document text. See `discard_diverged_internal_document`.
    let internal_document_source_content = result.internal_document.is_some().then(|| result.content.clone());

    #[cfg(feature = "html")]
    if let Some(html) = styled_html_prerender {
        result.formatted_content = Some(html);
    }

    // #331: same idea for the rendered output format, which `apply_output_format` swaps into
    // `content` at the very end. See `discard_diverged_formatted_content`.
    let formatted_content_source = result
        .formatted_content
        .as_ref()
        .map(|formatted| (result.content.clone(), formatted.clone()));

    #[cfg(feature = "image-encode")]
    if let Some(ref image_cfg) = config.images {
        apply_output_format_pass(&mut result, image_cfg);
    }

    if let Some(ref image_cfg) = config.images {
        apply_data_base64_pass(&mut result, image_cfg);
    }

    if let Some((early_processors, _, _)) = &processor_stages {
        execute_processor_stages(
            &mut result,
            config,
            &pp_config,
            &[(
                crate::plugins::ProcessingStage::Early,
                std::sync::Arc::clone(early_processors),
            )],
        )
        .await?;
    }

    execute_language_detection(&mut result, config)?;

    // Chunk here too (in addition to the corrective re-chunk near the end, after all
    // content-mutating steps — see the doc comment above and #213) so that Middle/Late
    // post-processors can see `result.chunks` populated, per the documented pipeline
    // contract ("1. Post-Processors ... 3. Chunking" is honored for processor
    // visibility) and the tested chunk-aware-post-processor contract
    // (`test_middle_postprocessors_run_after_explicit_chunking`). This first pass's
    // chunks (and their byte offsets) are provisional and get fully recomputed — and
    // `result.chunks` overwritten — by the final `execute_chunking` call below once
    // `content` stops changing, so a middle/late processor must not treat these
    // offsets as final.
    execute_chunking(&mut result, config, chunker_heading_source.as_deref())?;

    if let Some((_, middle_processors, late_processors)) = &processor_stages {
        let middle_processors = if config.captioning.is_some() {
            processors_without_captioning(middle_processors)
        } else {
            std::sync::Arc::clone(middle_processors)
        };
        execute_processor_stages(
            &mut result,
            config,
            &pp_config,
            &[
                (crate::plugins::ProcessingStage::Middle, middle_processors),
                (
                    crate::plugins::ProcessingStage::Late,
                    std::sync::Arc::clone(late_processors),
                ),
            ],
        )
        .await?;
    }

    execute_token_reduction(&mut result, config)?;
    execute_validators(&result, config).await?;

    // NFC normalization moved ahead of the element transform so the divergence check below
    // sees the final plain-text `content`, and so the fallback element build reads the same
    // normalized text that `content` carries (#286).
    normalize_nfc(&mut result);
    discard_diverged_internal_document(&mut result, internal_document_source_content.as_deref());
    discard_diverged_formatted_content(&mut result, formatted_content_source.as_ref());
    apply_element_transform(&mut result, config);

    // ~keep Run LLM-based structured extraction BEFORE output formatting
    // ~keep so extraction sees plain text, not markdown/HTML
    #[cfg(all(feature = "liter-llm", not(target_arch = "wasm32")))]
    if let Some(ref structured_config) = config.structured_extraction {
        match crate::llm::structured::extract_structured(&result.content, structured_config).await {
            Ok((output, usage)) => {
                result.structured_output = Some(output);
                crate::llm::usage::push_llm_usage(&mut result, usage);
            }
            Err(e) => {
                tracing::warn!("Structured extraction failed: {e}");
                result.processing_warnings.push(crate::types::ProcessingWarning {
                    source: std::borrow::Cow::Borrowed("structured_extraction"),
                    message: std::borrow::Cow::Owned(format!("Structured extraction failed: {e}")),
                });
            }
        }
    }

    #[cfg(not(feature = "liter-llm"))]
    if config.structured_extraction.is_some() {
        result.processing_warnings.push(crate::types::ProcessingWarning {
            source: std::borrow::Cow::Borrowed("structured_extraction"),
            message: std::borrow::Cow::Borrowed("Structured extraction requires the 'liter-llm' feature"),
        });
    }

    #[cfg(all(feature = "liter-llm", target_arch = "wasm32"))]
    if config.structured_extraction.is_some() {
        result.processing_warnings.push(crate::types::ProcessingWarning {
            source: std::borrow::Cow::Borrowed("structured_extraction"),
            message: std::borrow::Cow::Borrowed("Structured extraction is not available on wasm builds"),
        });
    }

    result = apply_output_format(result, config.output_format.clone());

    // Chunking runs last, after every step above that rewrites `content` — see the
    // ordering note on this function's doc comment (#213).
    execute_chunking(&mut result, config, chunker_heading_source.as_deref())?;

    populate_document_counts(&mut result);

    #[cfg(feature = "heuristics")]
    {
        use crate::heuristics::confidence::{ConfidenceSignals, ConfidenceWeights, SchemaCompliance, score_confidence};
        let text_coverage = measure_text_coverage(&result);
        let signals = ConfidenceSignals::from_extraction_result(&result, SchemaCompliance::AllValid, text_coverage);
        result.extraction_confidence = Some(score_confidence(signals, ConfidenceWeights::default()));
    }

    Ok(result)
}

/// Run the post-processing pipeline synchronously (WASM-compatible version).
///
/// This is a synchronous implementation for WASM and non-async contexts.
/// It performs a subset of the full async pipeline, excluding async post-processors
/// and validators.
///
/// # Arguments
///
/// * `doc` - The internal document produced by the extractor
/// * `config` - Extraction configuration
///
/// # Returns
///
/// The processed extraction result.
///
/// # Notes
///
/// This function is only available when the `tokio-runtime` feature is disabled.
/// It handles:
/// - Language detection (if enabled)
/// - Quality processing (token reduction, NFC normalization, output-format application)
/// - Chunking (if enabled) — runs last, after the content-mutating steps above (#213)
///
/// It does NOT handle:
/// - Async post-processors
/// - Async validators
#[cfg(not(feature = "tokio-runtime"))]
#[cfg_attr(feature = "otel", tracing::instrument(
    skip(doc, config),
    fields(
        { crate::telemetry::conventions::OPERATION } = crate::telemetry::conventions::operations::PIPELINE,
        content.element_count = doc.elements.len(),
    )
))]
#[cfg_attr(alef, alef(skip))]
pub fn run_pipeline_sync(mut doc: InternalDocument, config: &ExtractionConfig) -> Result<ExtractedDocument> {
    doc.ocr_text_only = config.images.as_ref().map(|i| i.ocr_text_only).unwrap_or(false);
    doc.append_ocr_text = config.images.as_ref().map(|i| i.append_ocr_text).unwrap_or(false);
    doc.escape_markdown = config.escape_markdown;
    doc.table_anchors = config.table_anchors;
    doc.page_marker_format = config
        .pages
        .as_ref()
        .filter(|p| p.insert_page_markers)
        .map(|p| p.marker_format.clone());
    if let Some(format) = doc.page_marker_format.clone() {
        page_markers::inject_page_marker_elements(&mut doc, &format);
    }

    // Mirror `run_pipeline`'s embedded-image OCR text handling (#219): without these,
    // `images.ocr_text_only` / `images.append_ocr_text` are silently ignored on the
    // sync (non-tokio, WASM) path even though the fields above are now set.
    replace_embedded_image_markdown_with_ocr(&mut doc);
    append_embedded_image_ocr_text(&mut doc);

    // Computed once, up front, from `doc` (independent of the later derivation and
    // content-mutating steps) and carried to the relocated `execute_chunking` call
    // near the end of this function — see the ordering note on `run_pipeline` (#213).
    #[cfg(feature = "chunking")]
    let chunker_heading_source: Option<String> = {
        let needs_markdown = config.chunking.as_ref().is_some_and(|c| {
            c.chunker_type == crate::core::config::ChunkerType::Markdown
                || c.resolve_preset().chunker_type == crate::core::config::ChunkerType::Markdown
        }) && config.output_format == crate::core::config::OutputFormat::Plain;
        if needs_markdown {
            Some(crate::rendering::render_markdown(&doc))
        } else {
            None
        }
    };
    #[cfg(not(feature = "chunking"))]
    let chunker_heading_source: Option<String> = None;

    #[cfg(feature = "html")]
    let styled_html_prerender: Option<String> = {
        use crate::plugins::InternalRenderer as _;
        if config.output_format == crate::core::config::OutputFormat::Html {
            config.html_output.as_ref().and_then(|html_cfg| {
                match crate::rendering::StyledHtmlRenderer::new(html_cfg.clone()) {
                    Ok(renderer) => match renderer.render(&doc) {
                        Ok(html) => Some(html),
                        Err(e) => {
                            tracing::warn!("StyledHtmlRenderer render failed, falling back to default HTML: {e}");
                            None
                        }
                    },
                    Err(e) => {
                        tracing::warn!("StyledHtmlRenderer construction failed, falling back to default HTML: {e}");
                        None
                    }
                }
            })
        } else {
            None
        }
    };

    let doc_for_elements = if config.result_format == crate::types::ResultFormat::ElementBased {
        Some(doc.clone())
    } else {
        None
    };
    let include_structure = config.include_document_structure;
    let mut result =
        crate::extraction::derive::derive_extraction_result(doc, include_structure, config.output_format.clone());
    result.internal_document = doc_for_elements;

    // #286: mirrors `run_pipeline` — see `discard_diverged_internal_document`.
    let internal_document_source_content = result.internal_document.is_some().then(|| result.content.clone());

    #[cfg(feature = "html")]
    if let Some(html) = styled_html_prerender {
        result.formatted_content = Some(html);
    }

    // #331: same idea for the rendered output format, which `apply_output_format` swaps into
    // `content` at the very end. See `discard_diverged_formatted_content`.
    let formatted_content_source = result
        .formatted_content
        .as_ref()
        .map(|formatted| (result.content.clone(), formatted.clone()));

    #[cfg(feature = "image-encode")]
    if let Some(ref image_cfg) = config.images {
        apply_output_format_pass(&mut result, image_cfg);
    }

    if let Some(ref image_cfg) = config.images {
        apply_data_base64_pass(&mut result, image_cfg);
    }

    execute_language_detection(&mut result, config)?;
    execute_token_reduction(&mut result, config)?;

    normalize_nfc(&mut result);
    discard_diverged_internal_document(&mut result, internal_document_source_content.as_deref());
    discard_diverged_formatted_content(&mut result, formatted_content_source.as_ref());
    apply_element_transform(&mut result, config);

    result = apply_output_format(result, config.output_format.clone());

    // Chunking runs last, after every step above that rewrites `content` — see the
    // ordering note on `run_pipeline`'s doc comment (#213).
    execute_chunking(&mut result, config, chunker_heading_source.as_deref())?;

    populate_document_counts(&mut result);

    #[cfg(feature = "heuristics")]
    {
        use crate::heuristics::confidence::{ConfidenceSignals, ConfidenceWeights, SchemaCompliance, score_confidence};
        let text_coverage = measure_text_coverage(&result);
        let signals = ConfidenceSignals::from_extraction_result(&result, SchemaCompliance::AllValid, text_coverage);
        result.extraction_confidence = Some(score_confidence(signals, ConfidenceWeights::default()));
    }

    Ok(result)
}

/// Populate [`ExtractedDocument::counts`] with cheap structural counts.
///
/// The page count is read from the parse-time page inventory
/// (`metadata.pages.total_count`) so it is available even when per-page content
/// extraction is disabled; it falls back to the materialized `pages` length and
/// finally `0` for inputs that are not page-addressable (plain text, etc.).
/// Table and image counts are the lengths of the already-populated collections.
fn populate_document_counts(result: &mut ExtractedDocument) {
    let pages = result
        .metadata
        .pages
        .as_ref()
        .map(|p| p.total_count as usize)
        .filter(|&n| n > 0)
        .or_else(|| result.pages.as_ref().map(Vec::len))
        .unwrap_or(0);
    result.counts = crate::types::DocumentCounts {
        pages,
        tables: result.tables.len(),
        images: result.images.as_ref().map_or(0, Vec::len),
    };
}

/// Measure the fraction of pages with usable (non-blank) text, for
/// [`crate::heuristics::confidence::ConfidenceSignals::text_coverage`] (#214).
///
/// For page-addressable documents (`result.pages` populated — PDFs, and any format
/// with a per-page breakdown), this is the fraction of pages whose trimmed content is
/// non-empty: a document with 3 of 10 pages OCR-blank or unreadable measures `0.7`,
/// not a hardcoded `1.0` that hides the gap. For formats without a page breakdown
/// (plain text, Markdown, HTML, …), coverage collapses to a binary full/empty signal:
/// `1.0` when `result.content` has any non-whitespace text, `0.0` when it does not —
/// a document that produced no text at all should not score as if it were fully
/// covered.
#[cfg(feature = "heuristics")]
fn measure_text_coverage(result: &ExtractedDocument) -> f32 {
    match result.pages.as_deref() {
        Some(pages) if !pages.is_empty() => {
            let usable = pages.iter().filter(|page| !page.content.trim().is_empty()).count();
            usable as f32 / pages.len() as f32
        }
        _ => {
            if result.content.trim().is_empty() {
                0.0
            } else {
                1.0
            }
        }
    }
}

/// Re-encode all images in `result` to the format requested by `config.output_format`.
///
/// Runs after OCR has completed and before post-processors so that downstream
/// consumers (captioning, QR) always see coherent `data` + `format` pairs.
/// Images whose source format cannot be decoded (e.g. EMF, WMF) are left untouched;
/// a `ProcessingWarning` is pushed for each failure.
///
/// When the `svg` feature is active and `config.output_format` is `Native`, a
/// sanitization pass is still applied to SVG images if `config.svg.sanitize` is set.
#[cfg(feature = "image-encode")]
fn apply_output_format_pass(
    result: &mut ExtractedDocument,
    config: &crate::core::config::extraction::ImageExtractionConfig,
) {
    use crate::core::config::extraction::ImageOutputFormat;
    use crate::core::image_encode::re_encode;

    #[cfg(not(feature = "svg"))]
    if matches!(config.output_format, ImageOutputFormat::Native) {
        return;
    }
    #[cfg(feature = "svg")]
    if matches!(config.output_format, ImageOutputFormat::Native) && !config.svg.sanitize {
        return;
    }

    let target = config.output_format;
    for image in result.images.iter_mut().flatten() {
        match re_encode(
            image,
            target,
            #[cfg(feature = "svg")]
            &config.svg,
        ) {
            Ok(_) => {}
            Err(warning) => {
                result.processing_warnings.push(crate::types::ProcessingWarning {
                    source: std::borrow::Cow::Borrowed("image_encoder"),
                    message: std::borrow::Cow::Owned(warning.to_string()),
                });
            }
        }
    }
}

/// Populate `ExtractedImage::data_base64` when the caller opts in via
/// `ImageExtractionConfig::include_data_base64`.
fn apply_data_base64_pass(
    result: &mut ExtractedDocument,
    config: &crate::core::config::extraction::ImageExtractionConfig,
) {
    if !config.include_data_base64 {
        return;
    }
    use base64::Engine as _;
    for image in result.images.iter_mut().flatten() {
        image.data_base64 = Some(base64::engine::general_purpose::STANDARD.encode(&image.data));
    }
}

/// Drop the preserved extractor `InternalDocument` once post-processing has rewritten
/// `content`, so nothing downstream can hand out pre-post-processing text (#286).
///
/// `source_content` is `result.content` as it stood when the tree was stored, or `None`
/// when no tree was stored at all.
///
/// The tree is a verbatim clone of the extractor's element list, taken before the
/// post-processor stages run. No stage writes back into it: redaction, summarisation,
/// translation and token reduction all rewrite `result.content` and leave the tree holding
/// the original text. Two consumers then read that stale tree:
///
/// - `plugins::renderer`'s blanket `impl<T: InternalRenderer> Renderer for T`, so the
///   public `Renderer::render_result` renders text that never went through
///   post-processing and disagrees with `ExtractedDocument::content`;
/// - `extraction::transform::transform_extraction_result_to_elements`, which prefers the
///   tree over `content`/`pages`, so the public `elements` — including the copy the
///   renderer registry hands to foreign renderers — carries the same stale text.
///
/// Dropping the tree makes both fall back to the post-processed `content`/`pages`: the
/// blanket impl returns `content` verbatim, per its documented `Renderer` default. The
/// tree is kept whenever `content` is untouched, which is the overwhelmingly common case
/// and preserves the extractor reading order the field exists to carry.
fn discard_diverged_internal_document(result: &mut ExtractedDocument, source_content: Option<&str>) {
    let Some(source_content) = source_content else {
        return;
    };
    if result.content != source_content {
        result.internal_document = None;
    }
}

/// `ProcessingWarning::source` for the output-format downgrade below.
const OUTPUT_FORMAT_WARNING_SOURCE: &str = "output_format";

/// Drop `formatted_content` when post-processing has rewritten `content` since it was
/// rendered.
///
/// `formatted_content` is produced by `derive_extraction_result` from the extractor's
/// element tree, before any post-processor runs, and [`apply_output_format`] then
/// overwrites `content` with it at the very end of the pipeline. So a processor that
/// rewrote `content` — redaction, summarisation, translation — had its work silently
/// thrown away for every output format that renders one (Markdown, Djot, HTML, JSON,
/// Custom); with redaction configured, the returned document was the *unredacted*
/// rendering (#331).
///
/// A processor that maintains both surfaces is honoured, not punished: the built-in
/// redaction processor rewrites `formatted_content` alongside `content`
/// (`text/redaction/engine.rs`), and its rendering is correct, so it is kept. Only a
/// rendering that did *not* move while `content` did is stale, and only that one is
/// dropped — correctness over presentation, with the caller told the requested format was
/// downgraded rather than being handed text no processor ever saw.
fn discard_diverged_formatted_content(result: &mut ExtractedDocument, source: Option<&(String, String)>) {
    let Some((source_content, source_formatted)) = source else {
        return;
    };

    // `content` never moved — the rendering still stands for it.
    if result.content == *source_content {
        return;
    }
    // Both moved: a processor rewrote the rendering alongside the text, so it is current.
    if result.formatted_content.as_deref() != Some(source_formatted.as_str()) {
        return;
    }

    result.formatted_content = None;
    crate::core::diagnostics::push_warning(
        &mut result.processing_warnings,
        OUTPUT_FORMAT_WARNING_SOURCE,
        "Post-processing rewrote the document text after the requested output format had \
         already been rendered, so the rendering was discarded and the post-processed plain \
         text is returned instead. Rendering it in the requested format would have undone \
         the post-processors' changes",
    );
}

/// Transform to element-based output if requested by the config.
fn apply_element_transform(result: &mut ExtractedDocument, config: &ExtractionConfig) {
    if config.result_format == crate::types::ResultFormat::ElementBased {
        result.elements = Some(crate::extraction::transform::transform_extraction_result_to_elements(
            result,
        ));
    }
}

/// Replace inline markdown image references with OCR text for formats (e.g. PPTX)
/// that bake placeholders into paragraph text rather than using `ElementKind::Image`.
fn replace_embedded_image_markdown_with_ocr(doc: &mut InternalDocument) {
    if !doc.ocr_text_only || doc.images.is_empty() {
        return;
    }

    let mut image_idx = 0usize;

    for elem in &mut doc.elements {
        if !matches!(elem.kind, crate::types::internal::ElementKind::Paragraph) {
            continue;
        }
        if !is_markdown_image_reference(&elem.text) {
            continue;
        }
        if let Some(img) = doc.images.get(image_idx)
            && let Some(ocr) = &img.ocr_result
            && !ocr.content.is_empty()
        {
            elem.text = ocr.content.clone();
            image_idx += 1;
            continue;
        }
        image_idx += 1;
    }

    for table in &mut doc.tables {
        for row in &mut table.cells {
            for cell in row {
                if !is_markdown_image_reference(cell) {
                    continue;
                }
                if let Some(img) = doc.images.get(image_idx)
                    && let Some(ocr) = &img.ocr_result
                    && !ocr.content.is_empty()
                {
                    *cell = ocr.content.clone();
                    image_idx += 1;
                    continue;
                }
                image_idx += 1;
            }
        }
    }
}

/// Append OCR text after inline markdown image references for formats (e.g. PPTX)
/// that bake placeholders into paragraph text. Only runs when `append_ocr_text` is
/// `true` and `ocr_text_only` is `false`.
fn append_embedded_image_ocr_text(doc: &mut InternalDocument) {
    if doc.ocr_text_only || !doc.append_ocr_text || doc.images.is_empty() {
        return;
    }

    let mut image_idx = 0usize;
    let mut new_elements = Vec::with_capacity(doc.elements.len() * 2);

    for elem in &doc.elements {
        new_elements.push(elem.clone());

        if matches!(elem.kind, crate::types::internal::ElementKind::Paragraph)
            && is_markdown_image_reference(&elem.text)
        {
            if let Some(img) = doc.images.get(image_idx)
                && let Some(ocr) = &img.ocr_result
                && !ocr.content.is_empty()
            {
                let ocr_elem = crate::types::internal::InternalElement::text(
                    crate::types::internal::ElementKind::Paragraph,
                    ocr.content.clone(),
                    0,
                );
                new_elements.push(ocr_elem);
            }
            image_idx += 1;
        }
    }

    doc.elements = new_elements;

    for table in &mut doc.tables {
        for row in &mut table.cells {
            for cell in row {
                if !is_markdown_image_reference(cell) {
                    continue;
                }
                if let Some(img) = doc.images.get(image_idx)
                    && let Some(ocr) = &img.ocr_result
                    && !ocr.content.is_empty()
                {
                    *cell = format!("{}\n\n{}", cell.trim(), ocr.content);
                }
                image_idx += 1;
            }
        }
    }
}

/// Returns `true` if `text` is exactly a markdown image reference (`![alt](url)`).
fn is_markdown_image_reference(text: &str) -> bool {
    let t = text.trim();
    if !t.starts_with("![") {
        return false;
    }
    let Some(bracket_end) = t.find("](") else {
        return false;
    };
    if bracket_end < 2 {
        return false;
    }
    let after = &t[bracket_end + 2..];
    after.ends_with(')')
}

/// Apply NFC unicode normalization to all text content.
///
/// Ensures consistent representation of composed characters (e.g., é vs e+combining accent)
/// across all extraction backends (PDF, OCR, DOCX, HTML, etc.).
fn normalize_nfc(result: &mut ExtractedDocument) {
    #[cfg(feature = "quality")]
    {
        use unicode_normalization::UnicodeNormalization;
        result.content = result.content.nfc().collect();
        if let Some(pages) = result.pages.as_mut() {
            for page in pages.iter_mut() {
                page.content = page.content.nfc().collect();
            }
        }
    }
    let _ = result;
}

/// Regression tests for #214: `measure_text_coverage` replaces the hardcoded
/// `DEFAULT_TEXT_COVERAGE = 1.0` passed into `ConfidenceSignals::from_extraction_result`.
#[cfg(all(test, feature = "heuristics"))]
mod issue_214_text_coverage_tests {
    use super::*;
    use crate::types::PageContent;

    fn page(page_number: u32, content: &str) -> PageContent {
        PageContent {
            page_number,
            content: content.to_string(),
            tables: Vec::new(),
            image_indices: Vec::new(),
            hierarchy: None,
            is_blank: None,
            layout_regions: None,
            speaker_notes: None,
            section_name: None,
            sheet_name: None,
        }
    }

    #[test]
    fn measures_fraction_of_non_blank_pages() {
        let result = ExtractedDocument {
            pages: Some(vec![
                page(1, "Real text here"),
                page(2, "   "),
                page(3, "More real text"),
            ]),
            ..Default::default()
        };
        assert!(
            (measure_text_coverage(&result) - (2.0 / 3.0)).abs() < f32::EPSILON,
            "expected 2/3 non-blank pages, got {}",
            measure_text_coverage(&result)
        );
    }

    #[test]
    fn measures_full_coverage_when_all_pages_have_text() {
        let result = ExtractedDocument {
            pages: Some(vec![page(1, "Text one"), page(2, "Text two")]),
            ..Default::default()
        };
        assert_eq!(measure_text_coverage(&result), 1.0);
    }

    #[test]
    fn measures_zero_coverage_when_all_pages_are_blank() {
        let result = ExtractedDocument {
            pages: Some(vec![page(1, ""), page(2, "   \n\t")]),
            ..Default::default()
        };
        assert_eq!(measure_text_coverage(&result), 0.0);
    }

    #[test]
    fn falls_back_to_binary_signal_when_pages_are_absent() {
        let with_text = ExtractedDocument {
            pages: None,
            content: "Some extracted text".to_string(),
            ..Default::default()
        };
        assert_eq!(measure_text_coverage(&with_text), 1.0);

        let empty = ExtractedDocument {
            pages: None,
            content: String::new(),
            ..Default::default()
        };
        assert_eq!(measure_text_coverage(&empty), 0.0);
    }

    #[test]
    fn falls_back_to_binary_signal_when_pages_is_empty_vec() {
        let result = ExtractedDocument {
            pages: Some(vec![]),
            content: "Some text".to_string(),
            ..Default::default()
        };
        assert_eq!(measure_text_coverage(&result), 1.0);
    }
}

/// Regression test for #219: `run_pipeline_sync` (the non-tokio/WASM path) omitted
/// `doc.ocr_text_only` / `doc.append_ocr_text` and the corresponding embedded-image OCR
/// text substitution the async `run_pipeline` performs, so `images.ocr_text_only` /
/// `images.append_ocr_text` were silently ignored on that path.
///
/// Compiled only under `not(feature = "tokio-runtime")`, exactly like `run_pipeline_sync`
/// itself — a `--features full` build never compiles this test (or the function).
#[cfg(all(test, not(feature = "tokio-runtime")))]
mod issue_219_sync_pipeline_ocr_text_options_tests {
    use super::*;
    use crate::core::config::extraction::ImageExtractionConfig;
    use crate::types::ExtractedImage;
    use crate::types::internal::{ElementKind, InternalDocument, InternalElement};
    use std::borrow::Cow;

    fn image_with_ocr_text(ocr_text: &str) -> ExtractedImage {
        ExtractedImage {
            data: bytes::Bytes::new(),
            format: Cow::Borrowed("png"),
            ocr_result: Some(Box::new(ExtractedDocument {
                content: ocr_text.to_string(),
                ..Default::default()
            })),
            ..Default::default()
        }
    }

    #[test]
    fn ocr_text_only_replaces_embedded_image_markdown_on_sync_pipeline() {
        let mut doc = InternalDocument::new("pptx");
        doc.push_element(InternalElement::text(ElementKind::Paragraph, "![img](embedded)", 0));
        doc.images = vec![image_with_ocr_text("Recognized OCR text")];

        let config = ExtractionConfig {
            images: Some(ImageExtractionConfig {
                ocr_text_only: true,
                ..Default::default()
            }),
            ..Default::default()
        };

        let result = run_pipeline_sync(doc, &config).unwrap();

        assert!(
            result.content.contains("Recognized OCR text"),
            "sync pipeline must replace embedded image markdown with OCR text when \
             ocr_text_only=true, got: {:?}",
            result.content
        );
        assert!(
            !result.content.contains("![img]"),
            "sync pipeline must not leave the markdown image placeholder when ocr_text_only=true, got: {:?}",
            result.content
        );
    }

    #[test]
    fn append_ocr_text_appends_after_embedded_image_markdown_on_sync_pipeline() {
        let mut doc = InternalDocument::new("pptx");
        doc.push_element(InternalElement::text(ElementKind::Paragraph, "![img](embedded)", 0));
        doc.images = vec![image_with_ocr_text("Appended OCR text")];

        let config = ExtractionConfig {
            images: Some(ImageExtractionConfig {
                append_ocr_text: true,
                ..Default::default()
            }),
            ..Default::default()
        };

        let result = run_pipeline_sync(doc, &config).unwrap();

        assert!(
            result.content.contains("![img]"),
            "append_ocr_text must keep the original markdown placeholder, got: {:?}",
            result.content
        );
        assert!(
            result.content.contains("Appended OCR text"),
            "sync pipeline must append the OCR text when append_ocr_text=true, got: {:?}",
            result.content
        );
    }

    #[test]
    fn default_config_does_not_touch_embedded_image_markdown_on_sync_pipeline() {
        let mut doc = InternalDocument::new("pptx");
        doc.push_element(InternalElement::text(ElementKind::Paragraph, "![img](embedded)", 0));
        doc.images = vec![image_with_ocr_text("Should not appear")];

        let config = ExtractionConfig::default();

        let result = run_pipeline_sync(doc, &config).unwrap();

        assert!(
            result.content.contains("![img]"),
            "default config must leave the markdown placeholder untouched, got: {:?}",
            result.content
        );
        assert!(
            !result.content.contains("Should not appear"),
            "default config must not inject OCR text, got: {:?}",
            result.content
        );
    }
}

/// Regression test for #213: chunking must run after every pipeline step that rewrites
/// `content`, so `Chunk::metadata::byte_start`/`byte_end` always index the *returned*
/// `content` — not a pre-mutation snapshot of it.
#[cfg(all(test, feature = "chunking", feature = "quality", feature = "tokio-runtime"))]
mod issue_213_chunk_offset_ordering_tests {
    use super::*;
    use crate::core::config::{ChunkerType, ChunkingConfig, OutputFormat};
    use crate::types::internal::{ElementKind, InternalDocument, InternalElement};

    /// "Cafe" + combining acute accent (U+0301) — 3 bytes for "e\u{0301}", decomposed.
    /// `normalize_nfc` composes this into "é" (2 bytes), shortening `content`. Before
    /// the fix, chunk offsets were computed against the pre-normalization (longer)
    /// text but returned alongside the post-normalization (shorter) `content`.
    const DECOMPOSED: &str = "Cafe\u{0301} is served here with extra words to keep the chunk large enough to matter.";

    #[tokio::test]
    async fn chunk_offsets_index_the_final_normalized_content() {
        let mut doc = InternalDocument::new("plain");
        doc.push_element(InternalElement::text(ElementKind::Paragraph, DECOMPOSED, 0));

        let config = ExtractionConfig {
            output_format: OutputFormat::Plain,
            chunking: Some(ChunkingConfig {
                max_characters: 2000,
                overlap: 0,
                trim: true,
                chunker_type: ChunkerType::Text,
                ..Default::default()
            }),
            ..Default::default()
        };

        let result = run_pipeline(doc, &config).await.unwrap();

        assert!(
            !result.content.contains('\u{0301}'),
            "expected NFC normalization to compose the combining accent away, got: {:?}",
            result.content
        );
        assert!(
            result.content.contains('é'),
            "expected composed 'é' in: {:?}",
            result.content
        );
        assert!(
            result.content.len() < DECOMPOSED.len(),
            "normalization must have shortened the content by at least one byte"
        );

        let chunks = result.chunks.expect("chunks must be populated");
        assert_eq!(chunks.len(), 1);
        for chunk in &chunks {
            let slice = &result.content[chunk.metadata.byte_start..chunk.metadata.byte_end];
            assert_eq!(
                slice, chunk.content,
                "chunk byte_start/byte_end must index the FINAL (post-normalization) content, \
                 not a pre-mutation snapshot"
            );
        }
    }
}

/// Regression tests for #271: `builtin_registration_error()` used to be dead code —
/// nothing surfaced it, so a user whose e.g. `summarization` processor failed to
/// register got a clean `Ok` with no output and no explanation. These test the pure
/// warning-construction helper directly (no global registry involved) rather than
/// the real one-time `OnceLock` registration pass, which cannot be re-triggered or
/// forced to fail from a test without mutating process-global state (#310).
#[cfg(test)]
mod issue_271_builtin_registration_warning_tests {
    use super::*;

    #[test]
    fn pushes_a_warning_naming_the_registration_error() {
        let mut doc = InternalDocument::new("plain");

        push_builtin_registration_warning(&mut doc, Some("summarization: boom".to_string()));

        assert_eq!(doc.processing_warnings.len(), 1);
        assert_eq!(doc.processing_warnings[0].source, "builtin_registration");
        assert_eq!(
            doc.processing_warnings[0].message,
            "built-in post-processor registration was incomplete (summarization: boom); a configured \
             processor may silently produce no output for its stage"
        );
    }

    #[test]
    fn pushes_nothing_when_registration_succeeded() {
        let mut doc = InternalDocument::new("plain");

        push_builtin_registration_warning(&mut doc, None);

        assert!(doc.processing_warnings.is_empty());
    }

    /// Exercises the actual call site in `run_pipeline` (not just the pure helper
    /// above), by forcing `initialization::builtin_registration_error()` to report
    /// a failure via the test-only setter and checking the warning that comes back
    /// out of a real `run_pipeline` call. This is the test that catches a regression
    /// where `push_builtin_registration_warning` exists but nothing calls it — the
    /// exact shape of the bug #271 originally described.
    ///
    /// `#[serial]`: `BUILTIN_REGISTRATION_ERROR` is a process-global static with no
    /// injectable variant, so a concurrent pipeline run expecting `None` would race
    /// this test — same reasoning as the other process-global-static tests in this
    /// crate (see `initialization::tests::processor_cache_rebuilds_when_registry_changes_after_first_use`).
    #[tokio::test]
    #[serial_test::serial]
    async fn run_pipeline_surfaces_a_forced_builtin_registration_failure() {
        use crate::core::pipeline::initialization::test_support::set_registration_error;
        use crate::types::internal::InternalDocument;

        set_registration_error(Some("summarization: boom".to_string()));

        let mut doc = InternalDocument::new("plain");
        doc.mime_type = "text/plain".to_string();
        let config = ExtractionConfig::default();

        let result = run_pipeline(doc, &config).await;

        set_registration_error(None);

        let processed = result.expect("run_pipeline must still succeed despite the registration failure");
        assert_eq!(processed.processing_warnings.len(), 1);
        assert_eq!(processed.processing_warnings[0].source, "builtin_registration");
        assert_eq!(
            processed.processing_warnings[0].message,
            "built-in post-processor registration was incomplete (summarization: boom); a configured \
             processor may silently produce no output for its stage"
        );
    }
}
