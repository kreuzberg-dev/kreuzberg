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
use initialization::{get_processors_from_cache, initialize_features, initialize_processor_cache};

const CAPTIONING_PROCESSOR_NAME: &str = "captioning";

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

async fn run_captioning_prepass(
    doc: &mut InternalDocument,
    config: &ExtractionConfig,
    include_structure: bool,
    pp_config: &Option<&crate::core::config::PostProcessorConfig>,
    middle_processors: &std::sync::Arc<Vec<PostProcessorHandle>>,
) -> Result<()> {
    if config.captioning.is_none() {
        return Ok(());
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
        return Ok(());
    }

    crate::extraction::derive::resolve_relationships(doc);
    let mut caption_result = crate::extraction::derive::derive_extraction_result(
        doc.clone(),
        include_structure,
        config.output_format.clone(),
    );

    execute_processor_stages(
        &mut caption_result,
        config,
        pp_config,
        &[(crate::plugins::ProcessingStage::Middle, captioning_processors)],
    )
    .await?;

    if let Some(captioned_images) = caption_result.images.as_ref() {
        let mut description_changed = false;
        for (retained, captioned) in doc.images.iter_mut().zip(captioned_images) {
            description_changed |= retained.description != captioned.description;
            retained.description = captioned.description.clone();
            retained.caption = captioned.caption.clone();
        }
        if description_changed {
            doc.pre_rendered_content = None;
        }
    }
    doc.processing_warnings = caption_result.processing_warnings;
    doc.llm_usage = caption_result.llm_usage;

    Ok(())
}

/// Run the post-processing pipeline on an `InternalDocument`.
///
/// Derives `ExtractedDocument` from `InternalDocument` via the derivation pipeline,
/// then executes post-processing in the following order:
/// 1. Post-Processors - Execute by stage (Early, Middle, Late) to modify/enhance the result
/// 2. Quality Processing - Text cleaning and quality scoring
/// 3. Chunking - Text splitting if enabled
/// 4. Validators - Run validation hooks on the processed result (can fail fast)
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
        pipeline.stage = "post_processing",
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
        initialize_processor_cache()?;

        let (early_processors, middle_processors, late_processors) = get_processors_from_cache()?;
        Some((early_processors, middle_processors, late_processors))
    } else {
        None
    };

    let include_structure = config.include_document_structure;
    if let Some((_, middle_processors, _)) = &processor_stages {
        run_captioning_prepass(&mut doc, config, include_structure, &pp_config, middle_processors).await?;
    }

    #[cfg(feature = "chunking")]
    let chunker_heading_source = {
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

    #[cfg(feature = "html")]
    if let Some(html) = styled_html_prerender {
        result.formatted_content = Some(html);
    }

    #[cfg(feature = "chunking")]
    let chunker_only_markdown = result.formatted_content.is_none();
    #[cfg(feature = "chunking")]
    if chunker_only_markdown && let Some(md) = chunker_heading_source {
        result.formatted_content = Some(md);
    }

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
    execute_chunking(&mut result, config)?;

    #[cfg(feature = "chunking")]
    if chunker_only_markdown {
        result.formatted_content = None;
    }

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

    apply_element_transform(&mut result, config);
    normalize_nfc(&mut result);

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

    populate_document_counts(&mut result);

    #[cfg(feature = "heuristics")]
    {
        use crate::heuristics::confidence::{ConfidenceSignals, ConfidenceWeights, SchemaCompliance, score_confidence};
        const DEFAULT_TEXT_COVERAGE: f32 = 1.0;
        let signals =
            ConfidenceSignals::from_extraction_result(&result, SchemaCompliance::AllValid, DEFAULT_TEXT_COVERAGE);
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
/// - Quality processing (if enabled)
/// - Chunking (if enabled)
/// - Language detection (if enabled)
///
/// It does NOT handle:
/// - Async post-processors
/// - Async validators
#[cfg(not(feature = "tokio-runtime"))]
#[cfg_attr(alef, alef(skip))]
pub fn run_pipeline_sync(mut doc: InternalDocument, config: &ExtractionConfig) -> Result<ExtractedDocument> {
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

    #[cfg(feature = "chunking")]
    let chunker_heading_source = {
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

    #[cfg(feature = "html")]
    if let Some(html) = styled_html_prerender {
        result.formatted_content = Some(html);
    }

    #[cfg(feature = "chunking")]
    let chunker_only_markdown = result.formatted_content.is_none();
    #[cfg(feature = "chunking")]
    if chunker_only_markdown && let Some(md) = chunker_heading_source {
        result.formatted_content = Some(md);
    }

    #[cfg(feature = "image-encode")]
    if let Some(ref image_cfg) = config.images {
        apply_output_format_pass(&mut result, image_cfg);
    }

    if let Some(ref image_cfg) = config.images {
        apply_data_base64_pass(&mut result, image_cfg);
    }

    execute_chunking(&mut result, config)?;

    #[cfg(feature = "chunking")]
    if chunker_only_markdown {
        result.formatted_content = None;
    }

    execute_language_detection(&mut result, config)?;
    execute_token_reduction(&mut result, config)?;

    apply_element_transform(&mut result, config);
    normalize_nfc(&mut result);

    result = apply_output_format(result, config.output_format.clone());

    populate_document_counts(&mut result);

    #[cfg(feature = "heuristics")]
    {
        use crate::heuristics::confidence::{ConfidenceSignals, ConfidenceWeights, SchemaCompliance, score_confidence};
        const DEFAULT_TEXT_COVERAGE: f32 = 1.0;
        let signals =
            ConfidenceSignals::from_extraction_result(&result, SchemaCompliance::AllValid, DEFAULT_TEXT_COVERAGE);
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
