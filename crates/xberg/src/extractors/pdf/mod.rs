//! PDF document extractor.
//!
//! Provides extraction of text, metadata, tables, and images from PDF documents
//! using pdf_oxide (pure Rust). Supports both native text extraction and OCR fallback.

mod extraction;
#[cfg(feature = "layout-detection")]
mod layout_hints;
#[cfg(all(feature = "pdf", feature = "layout-detection"))]
mod layout_runner;
mod ocr;
mod pages;
#[cfg(feature = "layout-detection")]
pub(crate) mod reading_order;
#[cfg(all(feature = "liter-llm", feature = "layout-detection"))]
mod region_vlm;

use crate::Result;
use crate::core::config::ExtractionConfig;
use crate::plugins::{InternalDocumentExtractor, Plugin};
use crate::types::internal::{ElementKind, InternalDocument, InternalElement};
use crate::types::{ExtractionMethod, Metadata};
use async_trait::async_trait;
#[cfg(feature = "tokio-runtime")]
use std::path::Path;

use extraction::extract_all_from_oxide_document;
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
use ocr::extract_with_ocr;

#[cfg(feature = "pdf")]
const PDF_OUTLINES_MARKER: &[u8] = b"/Outlines";
#[cfg(feature = "pdf")]
const PDF_PREVIOUS_XREF_MARKER: &[u8] = b"/Prev";

#[cfg(feature = "pdf")]
fn contains_pdf_marker(content: &[u8], marker: &[u8]) -> bool {
    memchr::memmem::find(content, marker).is_some()
}

#[cfg(feature = "pdf")]
fn catalog_needs_lopdf_compatibility_pass(catalog: Option<&pdf_oxide::object::Object>) -> bool {
    let Some(catalog) = catalog else {
        return true;
    };
    let Some(dictionary) = catalog.as_dict() else {
        return true;
    };
    dictionary.contains_key("Outlines")
}

#[cfg(feature = "pdf")]
fn raw_pdf_needs_lopdf_compatibility_pass(content: &[u8]) -> bool {
    contains_pdf_marker(content, PDF_PREVIOUS_XREF_MARKER) || contains_pdf_marker(content, PDF_OUTLINES_MARKER)
}

#[cfg(feature = "pdf")]
fn parsed_pdf_needs_lopdf_compatibility_pass(document: &crate::pdf::oxide::OxideDocument) -> bool {
    match document.doc.catalog() {
        Ok(catalog) => catalog_needs_lopdf_compatibility_pass(Some(&catalog)),
        Err(error) => {
            tracing::debug!(
                error = %error,
                "pdf_oxide catalog inspection failed; retaining lopdf compatibility pass"
            );
            catalog_needs_lopdf_compatibility_pass(None)
        }
    }
}

#[cfg(feature = "pdf")]
fn extract_lopdf_compatibility_data(
    content: &[u8],
) -> (
    Vec<crate::pdf::bookmarks::PdfOutlineEntry>,
    Vec<crate::types::ExtractedUri>,
    Option<Vec<crate::types::DocumentRevision>>,
) {
    match lopdf::Document::load_mem(content) {
        Ok(lopdf_doc) => {
            let entries = crate::pdf::bookmarks::extract_outline_entries(&lopdf_doc);
            let uris = crate::pdf::bookmarks::extract_bookmarks_from_entries(&entries);
            let revisions = crate::pdf::xref_revisions::extract_pdf_xref_revisions(content, &lopdf_doc);
            (entries, uris, revisions)
        }
        Err(_) => (Vec::new(), Vec::new(), None),
    }
}

#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PdfDocumentOrigin {
    Native,
    Mixed,
    Ocr,
}

#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
fn flat_pdf_document(text: &str, mime_type: &str) -> InternalDocument {
    let mut doc = InternalDocument::new("pdf");
    doc.mime_type = mime_type.to_string();
    for paragraph in text.split("\n\n").map(str::trim).filter(|text| !text.is_empty()) {
        doc.push_element(InternalElement::text(ElementKind::Paragraph, paragraph, 0));
    }
    doc
}

#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
fn select_pdf_document(
    extraction_method: ExtractionMethod,
    text: &str,
    mime_type: &str,
    pre_rendered_doc: Option<InternalDocument>,
    ocr_internal_doc: Option<InternalDocument>,
    ocr_results: Option<&ahash::AHashMap<u32, String>>,
) -> (InternalDocument, PdfDocumentOrigin, bool) {
    let (mut doc, origin, structured) = match extraction_method {
        ExtractionMethod::Native => match pre_rendered_doc {
            Some(doc) => (doc, PdfDocumentOrigin::Native, true),
            None => (flat_pdf_document(text, mime_type), PdfDocumentOrigin::Native, false),
        },
        ExtractionMethod::Mixed => match pre_rendered_doc {
            Some(mut doc) => {
                if let Some(results) = ocr_results {
                    ocr::merge_ocr_pages_into_internal_document(&mut doc, results);
                }
                (doc, PdfDocumentOrigin::Mixed, true)
            }
            None => (flat_pdf_document(text, mime_type), PdfDocumentOrigin::Mixed, false),
        },
        ExtractionMethod::Ocr => match ocr_internal_doc {
            Some(doc) => (doc, PdfDocumentOrigin::Ocr, true),
            None => (flat_pdf_document(text, mime_type), PdfDocumentOrigin::Ocr, false),
        },
    };
    doc.mime_type = mime_type.to_string();
    (doc, origin, structured)
}

fn inject_unrepresented_tables(doc: &mut InternalDocument, tables: Vec<crate::types::Table>, allow_injection: bool) {
    if !allow_injection
        || doc
            .elements
            .iter()
            .any(|element| matches!(element.kind, ElementKind::Table { .. }))
    {
        return;
    }
    if doc.tables.is_empty() {
        for table in tables {
            let table_index = doc.push_table(table);
            doc.push_element(InternalElement::text(ElementKind::Table { table_index }, "", 0));
        }
    } else {
        for table_index in 0..doc.tables.len() as u32 {
            doc.push_element(InternalElement::text(ElementKind::Table { table_index }, "", 0));
        }
    }
}

/// Pages to OCR under `OcrStrategy::ScannedPages`, 1-indexed.
///
/// The union of detected scans and pages failing the text-quality gate, so never
/// a subset of what `Auto` would OCR.
///
/// `None` means fall through to the `Auto` gate: wrong strategy, no page
/// qualifies, or the gate wants the whole document rather than a page subset.
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
fn scanned_pages_to_ocr(
    config: &ExtractionConfig,
    pdf_metadata: &crate::pdf::metadata::PdfExtractionMetadata,
    native_text: &str,
    boundaries: Option<&[crate::types::PageBoundary]>,
) -> Option<Vec<u32>> {
    use crate::core::config::OcrStrategy;

    if !matches!(config.ocr_strategy, OcrStrategy::ScannedPages { .. }) {
        return None;
    }

    let mut pages = pdf_metadata.pdf_specific.scanned_pages.clone()?;

    if let Some(ocr_config) = config.ocr.as_ref() {
        let decision = ocr::evaluate_per_page_ocr(
            native_text,
            boundaries,
            pdf_metadata.pdf_specific.page_count,
            &ocr_config.effective_thresholds(),
        );
        if decision.whole_doc_failure {
            return None;
        }
        pages.extend(decision.failing_pages);
    }

    pages.sort_unstable();
    pages.dedup();

    if pages.is_empty() { None } else { Some(pages) }
}
use pages::{assign_hierarchy_to_pages, assign_tables_and_images_to_pages};

#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
fn replace_tables_with_ocr_output(tables: &mut Vec<crate::types::Table>, mut ocr_tables: Vec<crate::types::Table>) {
    if ocr_tables.is_empty() {
        return;
    }

    ocr_tables.sort_by_key(|table| table.page_number);
    *tables = ocr_tables;
}

#[cfg(all(feature = "layout-detection", any(feature = "ocr", feature = "ocr-pipeline")))]
fn prepare_ocr_layout_inputs(
    images: Vec<image::RgbImage>,
    mut detections: Vec<crate::layout::DetectionResult>,
) -> (Vec<image::DynamicImage>, Vec<crate::layout::DetectionResult>) {
    if detections.len() != images.len() {
        tracing::warn!(
            images = images.len(),
            detections = detections.len(),
            "OCR layout input cardinality mismatch; discarding detections while reusing page rasters"
        );
        detections = images
            .iter()
            .map(|image| crate::layout::DetectionResult {
                page_width: image.width(),
                page_height: image.height(),
                detections: Vec::new(),
            })
            .collect();
    } else {
        for (page_index, (image, detection)) in images.iter().zip(&mut detections).enumerate() {
            if detection.page_width != image.width() || detection.page_height != image.height() {
                tracing::warn!(
                    page = page_index + 1,
                    image_width = image.width(),
                    image_height = image.height(),
                    detection_width = detection.page_width,
                    detection_height = detection.page_height,
                    "OCR layout dimensions mismatch; discarding detections for this page"
                );
                *detection = crate::layout::DetectionResult {
                    page_width: image.width(),
                    page_height: image.height(),
                    detections: Vec::new(),
                };
            }
        }
    }

    let images = images.into_iter().map(image::DynamicImage::ImageRgb8).collect();
    (images, detections)
}

/// Run OCR with optional layout detection on PDF bytes.
///
/// Reuses detections from native extraction when available. Otherwise, when
/// layout detection is configured, it runs a soft-failing layout pass before
/// OCR. Layout failures are logged and OCR continues without layout hints.
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
async fn run_ocr_with_layout(
    content: &[u8],
    config: &ExtractionConfig,
    path: Option<&std::path::Path>,
    #[cfg(feature = "layout-detection")] precomputed_layout_images: Option<Vec<image::RgbImage>>,
    #[cfg(feature = "layout-detection")] precomputed_layout_detections: Option<Vec<crate::layout::DetectionResult>>,
) -> crate::Result<(
    String,
    Vec<crate::types::Table>,
    Vec<crate::types::OcrElement>,
    Option<crate::types::internal::InternalDocument>,
    Vec<crate::types::LlmUsage>,
    Vec<String>,
    Option<Vec<crate::types::ExtractedImage>>,
    Vec<crate::types::Formula>,
)> {
    let default_ocr_config = crate::core::config::OcrConfig::default();
    let ocr_config = config.ocr.as_ref().unwrap_or(&default_ocr_config);

    #[cfg(all(feature = "pdf", feature = "layout-detection"))]
    let owned_layout = if precomputed_layout_detections.is_none() || precomputed_layout_images.is_none() {
        if let Some(layout_config) = config.resolved_layout_config() {
            let thread_budget = crate::core::config::concurrency::resolve_thread_budget(config.concurrency.as_ref());
            match layout_runner::run_layout_for_ocr(content, layout_config.as_ref(), thread_budget).await {
                Ok(layout) => Some(layout),
                Err(error) => {
                    tracing::warn!(
                        error = %error,
                        "OCR layout detection failed; continuing without layout assembly"
                    );
                    None
                }
            }
        } else {
            None
        }
    } else {
        None
    };

    #[cfg(all(feature = "pdf", feature = "layout-detection"))]
    let layout_inputs = match (precomputed_layout_images, precomputed_layout_detections) {
        (Some(images), Some(detections)) => Some((images, detections)),
        _ => owned_layout.map(|(images, _, _, detections)| (images, detections)),
    };
    #[cfg(all(not(feature = "pdf"), feature = "layout-detection"))]
    let layout_inputs = precomputed_layout_images.zip(precomputed_layout_detections);

    #[cfg(feature = "layout-detection")]
    let prepared_layout_inputs =
        layout_inputs.map(|(images, detections)| prepare_ocr_layout_inputs(images, detections));
    #[cfg(feature = "layout-detection")]
    let ocr_images = prepared_layout_inputs.as_ref().map(|(images, _)| images.as_slice());
    #[cfg(feature = "layout-detection")]
    let layout_detections = prepared_layout_inputs
        .as_ref()
        .map(|(_, detections)| detections.as_slice());

    if let Some(pipeline) = ocr_config.effective_pipeline() {
        let (text, ocr_tables, ocr_elements, pipeline_doc, llm_usage, ocr_pts, pipeline_rasters, pipeline_formulas) =
            Box::pin(ocr::run_ocr_pipeline(
                Some(content),
                #[cfg(feature = "layout-detection")]
                ocr_images,
                #[cfg(not(feature = "layout-detection"))]
                None,
                #[cfg(feature = "layout-detection")]
                layout_detections,
                config,
                &pipeline,
                path,
            ))
            .await?;
        return Ok((
            text,
            ocr_tables,
            ocr_elements,
            pipeline_doc,
            llm_usage,
            ocr_pts,
            pipeline_rasters,
            pipeline_formulas,
        ));
    }

    let (text, _mean_conf, ocr_tables, ocr_elements, ocr_doc, llm_usage, ocr_pts, ocr_rasters, formulas) =
        Box::pin(extract_with_ocr(
            Some(content),
            #[cfg(feature = "layout-detection")]
            ocr_images,
            #[cfg(not(feature = "layout-detection"))]
            None,
            #[cfg(feature = "layout-detection")]
            layout_detections,
            config,
            path,
        ))
        .await?;
    Ok((
        text,
        ocr_tables,
        ocr_elements,
        ocr_doc,
        llm_usage,
        ocr_pts,
        ocr_rasters,
        formulas,
    ))
}

/// PDF document extractor using pdf_oxide.
#[cfg_attr(alef, alef(skip))]
pub struct PdfExtractor;

impl Default for PdfExtractor {
    fn default() -> Self {
        Self::new()
    }
}

impl PdfExtractor {
    pub(crate) fn new() -> Self {
        Self
    }
}

impl Plugin for PdfExtractor {
    fn name(&self) -> &str {
        "pdf-extractor"
    }

    fn version(&self) -> String {
        env!("CARGO_PKG_VERSION").to_string()
    }

    fn initialize(&self) -> Result<()> {
        Ok(())
    }

    fn shutdown(&self) -> Result<()> {
        Ok(())
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
impl InternalDocumentExtractor for PdfExtractor {
    async fn extract_content(
        &self,
        content: &[u8],
        mime_type: &str,
        config: &ExtractionConfig,
    ) -> Result<InternalDocument> {
        self.extract_core(content, mime_type, config, None).await
    }

    #[cfg(feature = "tokio-runtime")]
    async fn extract_path(&self, path: &Path, mime_type: &str, config: &ExtractionConfig) -> Result<InternalDocument> {
        #[cfg(feature = "pdf")]
        crate::pdf::oxide_text::set_current_pdf_path(Some(path.to_path_buf()));
        // Async on native (non-blocking tokio::fs); sync fallback on wasm32 where tokio's `fs`
        // feature is unavailable. See `core::io::read_file_async`. ~keep
        let bytes = crate::core::io::read_file_async(path).await?;
        let result = self.extract_core(&bytes, mime_type, config, Some(path)).await;
        #[cfg(feature = "pdf")]
        crate::pdf::oxide_text::set_current_pdf_path(None);
        result
    }

    fn supported_mime_types(&self) -> &[&str] {
        &["application/pdf"]
    }
}

impl PdfExtractor {
    /// Core extraction logic shared between extract_bytes and extract_file.
    ///
    /// Accepts an optional `path` which is passed to OCR backends to allow
    /// direct document-level processing (bypassing page rendering).
    async fn extract_core(
        &self,
        content: &[u8],
        mime_type: &str,
        config: &ExtractionConfig,
        path: Option<&std::path::Path>,
    ) -> Result<InternalDocument> {
        tracing::debug!(format = "pdf", size_bytes = content.len(), "extraction starting");
        self.extract_core_oxide(content, mime_type, config, path).await
    }

    /// Core extraction via the pdf_oxide backend.
    ///
    /// Runs text + metadata, tables, and annotation extraction through the oxide
    /// modules, then builds an `InternalDocument` using the same post-processing
    /// pipeline (OCR evaluation, page assembly, image extraction, bookmarks, etc.).
    #[cfg(feature = "pdf")]
    async fn extract_core_oxide(
        &self,
        content: &[u8],
        mime_type: &str,
        config: &ExtractionConfig,
        path: Option<&std::path::Path>,
    ) -> Result<InternalDocument> {
        let _ = &path;

        #[cfg(all(feature = "pdf", feature = "layout-detection"))]
        #[allow(unused_mut, unused_variables)]
        let (
            mut markdown_layout_images,
            markdown_layout_results,
            markdown_layout_hints,
            mut markdown_layout_detections,
        ) = layout_runner::maybe_run_layout_for_markdown(content, config).await;

        #[cfg(all(feature = "pdf", feature = "layout-detection"))]
        let layout_hints: Option<&[Vec<crate::pdf::structure::types::LayoutHint>]> = markdown_layout_hints.as_deref();
        #[cfg(not(feature = "layout-detection"))]
        let layout_hints: Option<&[Vec<crate::pdf::structure::types::LayoutHint>]> = None;

        let passwords = config
            .pdf_options
            .as_ref()
            .and_then(|options| options.passwords.as_deref())
            .unwrap_or(&[]);
        let raw_compatibility_signal = raw_pdf_needs_lopdf_compatibility_pass(content);
        let compatibility_data = raw_compatibility_signal.then(|| extract_lopdf_compatibility_data(content));
        let mut oxide_document = crate::pdf::oxide::OxideDocument::open_bytes_with_passwords(content, passwords)?;

        let compatibility_data = match compatibility_data {
            Some(data) => Some(data),
            None if parsed_pdf_needs_lopdf_compatibility_pass(&oxide_document) => {
                // Avoid holding both parser object graphs at once. Compressed
                // catalog outlines are rare, so reopen pdf_oxide for this path. ~keep
                drop(oxide_document);
                let data = extract_lopdf_compatibility_data(content);
                oxide_document = crate::pdf::oxide::OxideDocument::open_bytes_with_passwords(content, passwords)?;
                Some(data)
            }
            None => None,
        };
        let (outline_entries, bookmark_uris, pdf_revisions) = compatibility_data.unwrap_or_default();

        #[allow(unused_variables, unused_mut)]
        let (
            mut pdf_metadata,
            native_text,
            mut tables,
            page_contents,
            boundaries,
            pre_rendered_doc,
            _has_font_encoding_issues,
            pdf_annotations,
            mut extracted_images,
            pdf_form_fields,
        ) = extract_all_from_oxide_document(
            oxide_document,
            config,
            &outline_entries,
            layout_hints,
            #[cfg(feature = "layout-detection")]
            markdown_layout_images.as_deref(),
            #[cfg(not(feature = "layout-detection"))]
            None,
            #[cfg(feature = "layout-detection")]
            markdown_layout_results.as_deref(),
            #[cfg(not(feature = "layout-detection"))]
            None,
        )?;

        #[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
        if config.pdf_options.as_ref().is_some_and(|p| p.ocr_inline_images)
            && let Some(ref mut imgs) = extracted_images
            && !imgs.is_empty()
        {
            let default_ocr_config;
            let ocr_config = match config.ocr.as_ref() {
                Some(c) => c,
                None => {
                    default_ocr_config = crate::core::config::OcrConfig::default();
                    &default_ocr_config
                }
            };
            crate::plugins::ensure_ocr_backends_initialized();
            let backend = {
                let registry = crate::plugins::registry::get_ocr_backend_registry();
                registry.read().get(&ocr_config.backend)?
            };
            let mut ocr_config_with_format = ocr_config.clone();
            ocr_config_with_format.output_format = Some(config.output_format.clone());
            for img in imgs.iter_mut() {
                if config.cancel_token.as_ref().is_some_and(|t| t.is_cancelled()) {
                    break;
                }
                match backend.process_image(&img.data, &ocr_config_with_format).await {
                    Ok(ocr_result) => {
                        img.ocr_result = Some(Box::new(ocr_result));
                    }
                    Err(e) => {
                        tracing::warn!(
                            page = img.page_number,
                            image_index = img.image_index,
                            error = %e,
                            "inline image OCR failed; image returned without OCR result"
                        );
                    }
                }
            }
        }

        #[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
        let mut ocr_tables: Vec<crate::types::Table> = Vec::new();
        #[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
        let mut ocr_elements: Vec<crate::types::OcrElement> = Vec::new();
        #[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
        let mut ocr_internal_doc: Option<InternalDocument> = None;
        #[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
        let mut ocr_llm_usage: Vec<crate::types::LlmUsage> = Vec::new();
        #[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
        let mut ocr_page_texts: Option<Vec<String>> = None;
        #[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
        let mut ocr_results_map: Option<ahash::AHashMap<u32, String>> = None;
        #[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
        let mut ocr_page_rasters: Option<Vec<crate::types::ExtractedImage>> = None;
        #[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
        let mut ocr_formulas: Vec<crate::types::Formula> = Vec::new();
        #[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
        let mut ocr_fallback_warnings: Vec<crate::types::ProcessingWarning> = Vec::new();

        #[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
        let (text, extraction_method) = if config.effective_disable_ocr() {
            (native_text, ExtractionMethod::Native)
        } else if config.force_ocr {
            let (ocr_text, ocr_tbls, ocr_elems, ocr_doc, llm_usage, ocr_pts, ocr_rstrs, formulas) =
                run_ocr_with_layout(
                    content,
                    config,
                    path,
                    #[cfg(feature = "layout-detection")]
                    markdown_layout_images.take(),
                    #[cfg(feature = "layout-detection")]
                    markdown_layout_detections.take(),
                )
                .await?;
            ocr_tables = ocr_tbls;
            ocr_elements = ocr_elems;
            ocr_internal_doc = ocr_doc;
            ocr_llm_usage = llm_usage;
            ocr_page_texts = Some(ocr_pts);
            ocr_page_rasters = ocr_rstrs;
            ocr_formulas = formulas;
            (ocr_text, ExtractionMethod::Ocr)
        } else if let Some(ref ocr_pages) = config.force_ocr_pages {
            if !ocr_pages.is_empty() {
                if let Some(ref bounds) = boundaries {
                    if !bounds.is_empty() {
                        let (mixed, results_map, mixed_llm_usage, mixed_rstrs, mixed_formulas) =
                            ocr::extract_mixed_ocr_native(&native_text, bounds, ocr_pages, content, config, path)
                                .await?;
                        ocr_llm_usage = mixed_llm_usage;
                        ocr_results_map = Some(results_map);
                        ocr_page_rasters = mixed_rstrs;
                        if !mixed_formulas.is_empty() {
                            ocr_formulas = mixed_formulas;
                        }
                        (mixed, ExtractionMethod::Mixed)
                    } else {
                        tracing::warn!("force_ocr_pages set but no page boundaries available; using native text");
                        (native_text, ExtractionMethod::Native)
                    }
                } else {
                    tracing::warn!("force_ocr_pages set but no page boundaries available; using native text");
                    (native_text, ExtractionMethod::Native)
                }
            } else {
                (native_text, ExtractionMethod::Native)
            }
        } else if let Some(scanned_pages) =
            scanned_pages_to_ocr(config, &pdf_metadata, &native_text, boundaries.as_deref())
        {
            // A scanner's invisible sidecar passes the gate below, so detected
            // pages are selected before it runs. ~keep
            if let Some(ref bounds) = boundaries
                && !bounds.is_empty()
            {
                let (mixed, results_map, mixed_llm_usage, mixed_rstrs, mixed_formulas) =
                    ocr::extract_mixed_ocr_native(&native_text, bounds, &scanned_pages, content, config, path).await?;
                ocr_llm_usage = mixed_llm_usage;
                ocr_results_map = Some(results_map);
                ocr_page_rasters = mixed_rstrs;
                if !mixed_formulas.is_empty() {
                    ocr_formulas = mixed_formulas;
                }
                (mixed, ExtractionMethod::Mixed)
            } else {
                tracing::warn!("scanned pages detected but no page boundaries available; using native text");
                (native_text, ExtractionMethod::Native)
            }
        } else if let Some(ocr_config) = config.ocr.as_ref() {
            let thresholds = ocr_config.effective_thresholds();
            let decision = ocr::evaluate_per_page_ocr(
                &native_text,
                boundaries.as_deref(),
                pdf_metadata.pdf_specific.page_count,
                &thresholds,
            );

            if std::env::var("XBERG_DEBUG_OCR").is_ok() {
                eprintln!(
                    "[xberg::pdf::ocr] fallback={} non_whitespace={} alnum={} meaningful_words={} \
                     avg_non_whitespace={:.2} avg_alnum={:.2} alnum_ratio={:.3} fragmented_word_ratio={:.3} \
                     avg_word_length={:.2} word_count={} consecutive_repeat_ratio={:.3}",
                    decision.fallback,
                    decision.stats.non_whitespace,
                    decision.stats.alnum,
                    decision.stats.meaningful_words,
                    decision.avg_non_whitespace,
                    decision.avg_alnum,
                    decision.stats.alnum_ratio,
                    decision.stats.fragmented_word_ratio,
                    decision.stats.avg_word_length,
                    decision.stats.word_count,
                    decision.stats.consecutive_repeat_ratio
                );
            }

            let total_chars = native_text.chars().count();
            let alnum_ws_chars = native_text
                .chars()
                .filter(|c| c.is_alphanumeric() || c.is_whitespace())
                .count();
            let alnum_ws_ratio = if total_chars > 0 {
                alnum_ws_chars as f64 / total_chars as f64
            } else {
                1.0
            };

            match ocr::evaluate_ocr_skip_gate(
                pre_rendered_doc.is_some(),
                total_chars,
                alnum_ws_ratio,
                &decision,
                &thresholds,
            ) {
                ocr::OcrGateOutcome::SkipNonText => {
                    tracing::debug!(
                        alnum_ws_ratio,
                        total_chars,
                        alnum_ws_chars,
                        "Skipping OCR: content is non-textual and pre-rendered structured doc available"
                    );
                    (native_text, ExtractionMethod::Native)
                }
                ocr::OcrGateOutcome::SkipSubstantive => {
                    tracing::debug!(
                        total_chars,
                        alnum_ws_ratio,
                        ocr_fallback = decision.fallback,
                        "Skipping OCR: pre-rendered structured doc available with substantive native text"
                    );
                    (native_text, ExtractionMethod::Native)
                }
                ocr::OcrGateOutcome::RunFallback => {
                    let skip_fallback = config.images.as_ref().map(|i| i.run_ocr_on_images).unwrap_or(false);
                    if skip_fallback {
                        tracing::debug!("Skipping document-level OCR fallback: run_ocr_on_images=true");
                        (native_text, ExtractionMethod::Native)
                    } else {
                        match run_ocr_with_layout(
                            content,
                            config,
                            path,
                            #[cfg(feature = "layout-detection")]
                            markdown_layout_images.take(),
                            #[cfg(feature = "layout-detection")]
                            markdown_layout_detections.take(),
                        )
                        .await
                        {
                            Ok((ocr_text, ocr_tbls, ocr_elems, ocr_doc, llm_usage, ocr_pts, ocr_rstrs, formulas)) => {
                                ocr_tables = ocr_tbls;
                                ocr_elements = ocr_elems;
                                ocr_internal_doc = ocr_doc;
                                ocr_llm_usage = llm_usage;
                                ocr_page_texts = Some(ocr_pts);
                                ocr_page_rasters = ocr_rstrs;
                                ocr_formulas = formulas;
                                (ocr_text, ExtractionMethod::Ocr)
                            }
                            Err(e) => {
                                tracing::warn!(
                                    error = %e,
                                    "OCR fallback failed; using native text extraction result"
                                );
                                ocr_fallback_warnings.push(crate::types::ProcessingWarning {
                                    source: std::borrow::Cow::Borrowed("ocr"),
                                    message: std::borrow::Cow::Owned(format!(
                                        "OCR fallback failed ({e}); returning native text that was below the \
                                         quality threshold which triggered OCR. Extracted content may be empty \
                                         or incomplete."
                                    )),
                                });
                                (native_text, ExtractionMethod::Native)
                            }
                        }
                    }
                }
                ocr::OcrGateOutcome::RunFallbackOnPages(pages) => match boundaries.as_deref() {
                    Some(bounds) if !bounds.is_empty() => {
                        match ocr::extract_mixed_ocr_native(&native_text, bounds, &pages, content, config, path).await {
                            Ok((mixed, results_map, mixed_llm_usage, mixed_rstrs, mixed_formulas)) => {
                                ocr_llm_usage = mixed_llm_usage;
                                ocr_results_map = Some(results_map);
                                ocr_page_rasters = mixed_rstrs;
                                if !mixed_formulas.is_empty() {
                                    ocr_formulas = mixed_formulas;
                                }
                                (mixed, ExtractionMethod::Mixed)
                            }
                            Err(e) => {
                                tracing::warn!(
                                    error = %e,
                                    failing_pages = ?pages,
                                    "Targeted OCR fallback failed; using native text extraction result"
                                );
                                ocr_fallback_warnings.push(crate::types::ProcessingWarning {
                                    source: std::borrow::Cow::Borrowed("ocr"),
                                    message: std::borrow::Cow::Owned(format!(
                                        "Targeted OCR fallback failed ({e}) for pages {pages:?}; those pages \
                                         retain native text that was below the OCR-trigger quality threshold \
                                         and may be empty or incomplete."
                                    )),
                                });
                                (native_text, ExtractionMethod::Native)
                            }
                        }
                    }
                    _ => {
                        tracing::warn!(
                            failing_pages = ?pages,
                            "Targeted OCR requested but no page boundaries available; using native text"
                        );
                        ocr_fallback_warnings.push(crate::types::ProcessingWarning {
                            source: std::borrow::Cow::Borrowed("ocr"),
                            message: std::borrow::Cow::Owned(format!(
                                "Targeted OCR was required for pages {pages:?} but no page boundaries were \
                                 available; those pages retain native text that was below the OCR-trigger \
                                 quality threshold and may be empty or incomplete."
                            )),
                        });
                        (native_text, ExtractionMethod::Native)
                    }
                },
                ocr::OcrGateOutcome::UseNative => (native_text, ExtractionMethod::Native),
            }
        } else {
            (native_text, ExtractionMethod::Native)
        };

        #[cfg(not(any(feature = "ocr", feature = "ocr-pipeline")))]
        let (text, extraction_method) = (native_text, ExtractionMethod::Native);

        #[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
        // Full-document OCR is authoritative for tables when it produced
        // them. The structured OCR document already contains the same table
        // values, so later document assembly must not inject them a second
        // time. ~keep
        replace_tables_with_ocr_output(&mut tables, ocr_tables);

        let (images, image_fallback_warning): (
            Option<Vec<crate::types::ExtractedImage>>,
            Option<crate::types::ProcessingWarning>,
        ) = (extracted_images, None);

        #[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
        let mut page_contents = page_contents;

        #[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
        {
            if let Some(pts) = ocr_page_texts {
                if let Some(ref mut pages) = page_contents {
                    let pts_len = pts.len();
                    let pages_len = pages.len();

                    for (page, text) in pages.iter_mut().zip(pts) {
                        page.content = crate::pdf::text::fix_pdf_control_chars(&text).into_owned();
                        page.is_blank = Some(crate::extraction::blank_detection::is_page_text_blank(&page.content));
                    }

                    if pts_len == 1 && pages_len > 1 {
                        for p in pages.iter_mut().skip(1) {
                            p.content.clear();
                            p.is_blank = Some(true);
                        }
                    }
                } else {
                    page_contents = Some(
                        pts.into_iter()
                            .enumerate()
                            .map(|(i, text)| {
                                let content = crate::pdf::text::fix_pdf_control_chars(&text).into_owned();
                                let is_blank = Some(crate::extraction::blank_detection::is_page_text_blank(&content));
                                crate::types::PageContent {
                                    page_number: (i + 1) as u32,
                                    content,
                                    tables: Vec::new(),
                                    image_indices: vec![],
                                    hierarchy: None,
                                    is_blank,
                                    layout_regions: None,
                                    speaker_notes: None,
                                    section_name: None,
                                    sheet_name: None,
                                }
                            })
                            .collect(),
                    );
                }
            }

            if let Some(results_map) = ocr_results_map.as_ref()
                && let Some(ref mut pages) = page_contents
            {
                for page in pages.iter_mut() {
                    if let Some(ocr_text) = results_map.get(&page.page_number) {
                        page.content = crate::pdf::text::fix_pdf_control_chars(ocr_text).into_owned();
                        page.is_blank = Some(crate::extraction::blank_detection::is_page_text_blank(&page.content));
                    }
                }
            }

            if let Some(ref content_pages) = page_contents
                && let Some(ref mut page_structure) = pdf_metadata.page_structure
                && let Some(ref mut info_pages) = page_structure.pages
            {
                for info in info_pages.iter_mut() {
                    if let Some(content_page) = content_pages.iter().find(|p| p.page_number == info.number) {
                        info.is_blank = content_page.is_blank;
                    }
                }
            }
        }

        #[cfg(all(any(feature = "ocr", feature = "ocr-pipeline"), feature = "chunking"))]
        if extraction_method.used_ocr()
            && let Some(ref pages) = page_contents
            && !pages.is_empty()
        {
            let combined: String = pages
                .iter()
                .filter(|p| !p.content.trim().is_empty())
                .map(|p| p.content.trim())
                .collect::<Vec<_>>()
                .join("\n\n");
            if let Some(ref mut page_structure) = pdf_metadata.page_structure {
                page_structure.boundaries = Some(crate::core::pipeline::features::recompute_boundaries_from_pages(
                    &combined, pages,
                ));
            }
        }

        let mut final_pages =
            assign_tables_and_images_to_pages(page_contents, &tables, images.as_deref().unwrap_or(&[]));

        let pre_formatted_output: Option<String> = None;

        let used_ocr = extraction_method.used_ocr();

        #[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
        let (mut doc, document_origin, document_is_structured) = select_pdf_document(
            extraction_method,
            &text,
            mime_type,
            pre_rendered_doc,
            ocr_internal_doc.take(),
            ocr_results_map.as_ref(),
        );
        #[cfg(not(any(feature = "ocr", feature = "ocr-pipeline")))]
        let mut doc = if let Some(mut pre_doc) = pre_rendered_doc {
            pre_doc.mime_type = mime_type.to_string();
            pre_doc
        } else {
            let mut d = InternalDocument::new("pdf");
            d.mime_type = mime_type.to_string();
            for paragraph in text.split("\n\n") {
                let trimmed = paragraph.trim();
                if !trimmed.is_empty() {
                    d.push_element(InternalElement::text(ElementKind::Paragraph, trimmed, 0));
                }
            }
            d
        };
        #[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
        tracing::debug!(?document_origin, document_is_structured, "selected PDF document origin");

        #[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
        doc.processing_warnings.append(&mut ocr_fallback_warnings);

        doc.metadata = Metadata {
            output_format: pre_formatted_output,
            title: pdf_metadata.title.clone(),
            subject: pdf_metadata.subject.clone(),
            authors: pdf_metadata.authors.clone(),
            keywords: pdf_metadata.keywords.clone(),
            created_at: pdf_metadata.created_at.clone(),
            modified_at: pdf_metadata.modified_at.clone(),
            created_by: pdf_metadata.created_by.clone(),
            pages: pdf_metadata.page_structure.clone(),
            format: Some(crate::types::FormatMetadata::Pdf(pdf_metadata.pdf_specific)),
            ocr_used: used_ocr,
            ..Default::default()
        };
        doc.metadata.additional.insert(
            std::borrow::Cow::Borrowed("extraction_method"),
            serde_json::Value::String(extraction_method.as_str().to_string()),
        );

        doc.form_fields = pdf_form_fields;

        #[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
        let allow_table_injection =
            !document_is_structured || (document_origin == PdfDocumentOrigin::Ocr && doc.tables.is_empty());
        #[cfg(not(any(feature = "ocr", feature = "ocr-pipeline")))]
        let allow_table_injection = true;
        inject_unrepresented_tables(&mut doc, tables, allow_table_injection);

        if let Some(imgs) = images {
            // The OCR path has its own guarded injection block below (see the `#[cfg(feature = "ocr")]`
            let inject_placeholders = config.images.as_ref().is_some_and(|c| c.inject_placeholders);
            let document_has_image_elements = doc
                .elements
                .iter()
                .any(|element| matches!(element.kind, ElementKind::Image { .. }));
            if !document_has_image_elements && inject_placeholders {
                for (idx, img) in imgs.iter().enumerate() {
                    let mut elem = InternalElement::text(
                        ElementKind::Image {
                            image_index: idx as u32,
                        },
                        "",
                        0,
                    );
                    elem.page = img.page_number;
                    doc.push_element(elem);
                }
            }
            doc.images = imgs;
        }

        #[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
        let ocr_rasters_bypass = ocr_page_rasters.is_none();
        #[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
        if let Some(rasters) = ocr_page_rasters {
            let base_idx = doc.images.len() as u32;
            for (offset, mut raster) in rasters.into_iter().enumerate() {
                raster.image_index = base_idx + offset as u32;
                doc.images.push(raster);
            }
        }

        #[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
        if used_ocr
            && ocr_rasters_bypass
            && extraction_method == ExtractionMethod::Ocr
            && config.images.as_ref().is_some_and(|c| c.include_page_rasters)
        {
            doc.processing_warnings.push(crate::types::ProcessingWarning {
                source: std::borrow::Cow::Borrowed("page_rasters"),
                message: std::borrow::Cow::Borrowed(
                    "include_page_rasters is set but no page rasters were produced; \
                     the active OCR backend used document-level processing without per-page rendering",
                ),
            });
        }

        #[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
        if used_ocr && !doc.images.is_empty() {
            let images_enabled = config.images.as_ref().map(|c| c.extract_images).unwrap_or(false)
                || config.pdf_options.as_ref().map(|p| p.extract_images).unwrap_or(false);
            if images_enabled && config.images.as_ref().map(|c| c.inject_placeholders).unwrap_or(false) {
                let referenced_images: std::collections::HashSet<u32> = doc
                    .elements
                    .iter()
                    .filter_map(|element| match element.kind {
                        ElementKind::Image { image_index } => Some(image_index),
                        _ => None,
                    })
                    .collect();
                let elems: Vec<InternalElement> = doc
                    .images
                    .iter()
                    .filter(|image| !referenced_images.contains(&image.image_index))
                    .map(|img| {
                        let elem = InternalElement::text(
                            ElementKind::Image {
                                image_index: img.image_index,
                            },
                            "",
                            0,
                        );
                        if let Some(page) = img.page_number {
                            elem.with_page(page)
                        } else {
                            elem
                        }
                    })
                    .collect();
                for elem in elems {
                    doc.push_element(elem);
                }
            }
        }

        if let Some(warning) = image_fallback_warning {
            doc.processing_warnings.push(warning);
        }
        doc.annotations = pdf_annotations;

        {
            use crate::types::annotations::PdfAnnotationType;
            use crate::types::uri::{ExtractedUri, UriKind};

            let uris: Vec<ExtractedUri> = doc
                .annotations
                .as_ref()
                .map(|annotations| {
                    annotations
                        .iter()
                        .filter(|a| a.annotation_type == PdfAnnotationType::Link)
                        .filter_map(|a| {
                            a.content.as_ref().map(|url| {
                                let kind = if url.starts_with('#') {
                                    UriKind::Anchor
                                } else if url.starts_with("mailto:") {
                                    UriKind::Email
                                } else {
                                    UriKind::Hyperlink
                                };
                                ExtractedUri {
                                    url: url.clone(),
                                    label: Some(url.clone()),
                                    page: Some(a.page_number),
                                    kind,
                                }
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();
            for uri in uris {
                doc.push_uri(uri);
            }
        }

        #[cfg(feature = "pdf")]
        {
            for uri in bookmark_uris {
                doc.push_uri(uri);
            }
            doc.revisions = pdf_revisions;
        }

        #[cfg(all(feature = "pdf", feature = "tokio-runtime"))]
        {
            let (embedded_children, embedded_warnings) =
                crate::pdf::embedded_files::extract_and_process_embedded_files(content, config).await;
            if !embedded_children.is_empty() {
                match doc.children {
                    Some(ref mut existing) => existing.extend(embedded_children),
                    None => doc.children = Some(embedded_children),
                }
            }
            for warning in embedded_warnings {
                doc.processing_warnings.push(warning);
            }
        }

        if let Some(ref mut pages) = final_pages {
            assign_hierarchy_to_pages(pages, &doc);
        }

        doc.prebuilt_pages = final_pages;

        #[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
        if !ocr_elements.is_empty() {
            doc.prebuilt_ocr_elements = Some(ocr_elements);
        }

        #[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
        if !ocr_formulas.is_empty() {
            doc.formulas = ocr_formulas;
        }

        #[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
        if !ocr_llm_usage.is_empty() {
            doc.llm_usage = Some(ocr_llm_usage);
        }

        tracing::debug!(
            elements = doc.elements.len(),
            tables = doc.tables.len(),
            has_pages = doc.prebuilt_pages.is_some(),
            "InternalDocument finalized (oxide path)"
        );

        #[cfg(all(feature = "liter-llm", feature = "layout-detection"))]
        {
            let vlm_enabled = config
                .ocr
                .as_ref()
                .map(|o| o.vlm_fallback != crate::core::config::VlmFallbackPolicy::Disabled && o.vlm_config.is_some())
                .unwrap_or(false);

            if vlm_enabled
                && let (Some(layout_images), Some(hints)) =
                    (markdown_layout_images.as_deref(), markdown_layout_hints.as_deref())
            {
                let vlm_cfg = config
                    .ocr
                    .as_ref()
                    .and_then(|o| o.vlm_config.as_ref())
                    .expect("vlm_config checked above");

                let region_results = region_vlm::extract_vlm_regions(layout_images, hints, vlm_cfg).await;
                if !region_results.is_empty() {
                    tracing::debug!(
                        count = region_results.len(),
                        "injecting VLM region results into document"
                    );
                    region_vlm::inject_region_results(&mut doc, region_results);
                }
            }
        }

        {
            let mut budget = crate::extractors::security::SecurityBudget::from_config(config);
            for elem in &doc.elements {
                budget.account_text(elem.text.len())?;
            }
        }

        Ok(doc)
    }

    /// Fallback extraction path when pdf feature is not enabled.
    #[cfg(not(feature = "pdf"))]
    async fn extract_core_oxide(
        &self,
        _content: &[u8],
        mime_type: &str,
        _config: &ExtractionConfig,
        _path: Option<&std::path::Path>,
    ) -> Result<InternalDocument> {
        let doc = InternalDocument::new(mime_type);
        Ok(doc)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(feature = "ocr")]
    use crate::core::config::OcrQualityThresholds;
    #[cfg(all(feature = "pdf", feature = "ocr"))]
    use serial_test::serial;

    #[cfg(feature = "pdf")]
    fn catalog(
        entries: impl IntoIterator<Item = (&'static str, pdf_oxide::object::Object)>,
    ) -> pdf_oxide::object::Object {
        pdf_oxide::object::Object::Dictionary(
            entries
                .into_iter()
                .map(|(key, value)| (key.to_string(), value))
                .collect(),
        )
    }

    #[cfg(feature = "pdf")]
    #[test]
    fn lopdf_compatibility_pass_skips_pdf_without_relevant_signals() {
        let catalog = catalog([]);

        assert!(!raw_pdf_needs_lopdf_compatibility_pass(
            b"%PDF-1.7\nordinary content\n%%EOF",
        ));
        assert!(!catalog_needs_lopdf_compatibility_pass(Some(&catalog)));
    }

    #[cfg(feature = "pdf")]
    #[test]
    fn lopdf_compatibility_pass_keeps_raw_pdf_signals() {
        let catalog = catalog([]);

        assert!(raw_pdf_needs_lopdf_compatibility_pass(
            b"%PDF-1.7\ntrailer << /Prev 42 >>",
        ));
        assert!(raw_pdf_needs_lopdf_compatibility_pass(
            b"%PDF-1.7\n<< /Outlines 7 0 R >>",
        ));
        assert!(!catalog_needs_lopdf_compatibility_pass(Some(&catalog)));
    }

    #[cfg(feature = "pdf")]
    #[test]
    fn lopdf_compatibility_pass_keeps_parsed_catalog_outline() {
        let catalog = catalog([("Outlines", pdf_oxide::object::Object::Null)]);

        assert!(catalog_needs_lopdf_compatibility_pass(Some(&catalog)));
    }

    #[cfg(feature = "pdf")]
    #[test]
    fn lopdf_compatibility_pass_fails_open_without_catalog() {
        assert!(catalog_needs_lopdf_compatibility_pass(None));
    }

    #[cfg(feature = "pdf")]
    fn pdf_test_document(name: &str) -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../../test_documents/pdf/{name}"))
    }

    #[cfg(feature = "pdf")]
    fn extraction_method(result: &crate::types::ExtractedDocument) -> Option<ExtractionMethod> {
        result.extraction_method
    }

    #[cfg(all(feature = "pdf", feature = "ocr", feature = "chunking"))]
    fn assert_occurs_once(haystack: &str, needle: &str, projection: &str) {
        assert_eq!(
            haystack.matches(needle).count(),
            1,
            "mixed OCR text must occur exactly once in {projection}: {haystack}"
        );
    }

    #[cfg(all(feature = "pdf", feature = "ocr", feature = "chunking"))]
    fn mixed_native_and_scanned_pdf() -> Vec<u8> {
        use lopdf::content::{Content, Operation};
        use lopdf::{Document, Object, Stream, dictionary};

        let mut document = Document::with_version("1.5");
        let pages_id = document.new_object_id();
        let font_id = document.add_object(dictionary! {
            "Type" => "Font",
            "Subtype" => "Type1",
            "BaseFont" => "Helvetica",
        });

        let native_content = Content {
            operations: vec![
                Operation::new("BT", vec![]),
                Operation::new("Tf", vec!["F1".into(), 18.into()]),
                Operation::new("Td", vec![72.into(), 720.into()]),
                Operation::new(
                    "Tj",
                    vec![Object::string_literal(
                        "Issue 1281 native text remains on page one with enough meaningful words to pass the automatic per-page quality gate and preserve mixed routing.",
                    )],
                ),
                Operation::new("ET", vec![]),
            ],
        };
        let native_content_id = document.add_object(Stream::new(
            dictionary! {},
            native_content.encode().expect("native PDF content must encode"),
        ));
        let native_page_id = document.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "Contents" => native_content_id,
            "Resources" => dictionary! { "Font" => dictionary! { "F1" => font_id } },
            "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
        });

        let image_id = document.add_object(Stream::new(
            dictionary! {
                "Type" => "XObject",
                "Subtype" => "Image",
                "Width" => 1,
                "Height" => 1,
                "ColorSpace" => "DeviceGray",
                "BitsPerComponent" => 8,
            },
            vec![0],
        ));
        let scanned_content = Content {
            operations: vec![
                Operation::new("q", vec![]),
                Operation::new(
                    "cm",
                    vec![612.into(), 0.into(), 0.into(), 792.into(), 0.into(), 0.into()],
                ),
                Operation::new("Do", vec![Object::Name(b"Scan".to_vec())]),
                Operation::new("Q", vec![]),
            ],
        };
        let scanned_content_id = document.add_object(Stream::new(
            dictionary! {},
            scanned_content.encode().expect("scanned PDF content must encode"),
        ));
        let scanned_page_id = document.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "Contents" => scanned_content_id,
            "Resources" => dictionary! { "XObject" => dictionary! { "Scan" => image_id } },
            "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
        });

        document.objects.insert(
            pages_id,
            Object::Dictionary(dictionary! {
                "Type" => "Pages",
                "Kids" => vec![native_page_id.into(), scanned_page_id.into()],
                "Count" => 2,
            }),
        );
        let catalog_id = document.add_object(dictionary! {
            "Type" => "Catalog",
            "Pages" => pages_id,
        });
        document.trailer.set("Root", catalog_id);

        let mut bytes = Vec::new();
        document.save_to(&mut bytes).expect("mixed PDF fixture must serialize");
        bytes
    }

    #[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
    #[test]
    fn ocr_tables_replace_native_tables_and_are_sorted() {
        let table = |markdown: &str, page_number| crate::types::Table {
            cells: Vec::new(),
            markdown: markdown.to_string(),
            page_number,
            bounding_box: None,
            ..Default::default()
        };
        let mut tables = vec![table("native", 1)];

        replace_tables_with_ocr_output(&mut tables, vec![table("ocr-page-2", 2), table("ocr-page-1", 1)]);

        assert_eq!(tables.len(), 2);
        assert_eq!(tables[0].markdown, "ocr-page-1");
        assert_eq!(tables[1].markdown, "ocr-page-2");
    }

    #[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
    #[test]
    fn empty_ocr_tables_preserve_native_tables() {
        let mut tables = vec![crate::types::Table {
            cells: Vec::new(),
            markdown: "native".to_string(),
            page_number: 1,
            bounding_box: None,
            ..Default::default()
        }];

        replace_tables_with_ocr_output(&mut tables, Vec::new());

        assert_eq!(tables.len(), 1);
        assert_eq!(tables[0].markdown, "native");
    }

    #[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
    #[test]
    fn full_ocr_flat_fallback_never_selects_stale_native_document() {
        let mut native = InternalDocument::new("pdf");
        native.push_element(InternalElement::text(ElementKind::Paragraph, "stale native content", 0));

        let (doc, origin, structured) = select_pdf_document(
            ExtractionMethod::Ocr,
            "authoritative OCR content",
            "application/pdf",
            Some(native),
            None,
            None,
        );

        assert_eq!(origin, PdfDocumentOrigin::Ocr);
        assert!(!structured);
        assert!(
            doc.elements
                .iter()
                .any(|element| element.text == "authoritative OCR content")
        );
        assert!(
            !doc.elements
                .iter()
                .any(|element| element.text == "stale native content")
        );
    }

    #[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
    #[test]
    fn mixed_origin_replaces_only_targeted_structured_page() {
        let mut native = InternalDocument::new("pdf");
        let table = crate::types::Table {
            cells: vec![vec!["native table".to_string()]],
            markdown: "| native table |".to_string(),
            page_number: 2,
            bounding_box: None,
            ..Default::default()
        };
        native.tables.push(table.clone());
        native.push_element(InternalElement::text(ElementKind::Paragraph, "native page one", 0).with_page(1));
        native.push_element(InternalElement::text(ElementKind::PageBreak, "", 0));
        native.push_element(InternalElement::text(ElementKind::Paragraph, "stale page two", 0).with_page(2));
        native.push_element(InternalElement::text(ElementKind::Table { table_index: 0 }, "", 0).with_page(2));
        let mut results = ahash::AHashMap::new();
        results.insert(2, "OCR page two".to_string());

        let (mut doc, origin, structured) = select_pdf_document(
            ExtractionMethod::Mixed,
            "flat text must not be selected",
            "application/pdf",
            Some(native),
            None,
            Some(&results),
        );

        assert_eq!(origin, PdfDocumentOrigin::Mixed);
        assert!(structured);
        assert!(doc.elements.iter().any(|element| element.text == "native page one"));
        assert!(doc.elements.iter().any(|element| element.text == "OCR page two"));
        assert!(!doc.elements.iter().any(|element| element.text == "stale page two"));
        inject_unrepresented_tables(&mut doc, vec![table], false);
        assert_eq!(doc.tables.len(), 1, "structured table data must remain available");
        assert!(
            !doc.elements
                .iter()
                .any(|element| matches!(element.kind, ElementKind::Table { .. })),
            "target-page table markup must not be re-injected after OCR replacement"
        );
    }

    #[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
    #[test]
    fn full_ocr_structured_without_ocr_tables_injects_native_fallback_once() {
        let ocr_doc = InternalDocument::new("pdf");
        let table = crate::types::Table {
            cells: vec![vec!["native fallback".to_string()]],
            markdown: "| native fallback |".to_string(),
            page_number: 1,
            bounding_box: None,
            ..Default::default()
        };
        let (mut doc, origin, structured) = select_pdf_document(
            ExtractionMethod::Ocr,
            "OCR content",
            "application/pdf",
            None,
            Some(ocr_doc),
            None,
        );
        let allow_injection = !structured || (origin == PdfDocumentOrigin::Ocr && doc.tables.is_empty());

        inject_unrepresented_tables(&mut doc, vec![table], allow_injection);

        assert_eq!(doc.tables.len(), 1);
        assert_eq!(
            doc.elements
                .iter()
                .filter(|element| matches!(element.kind, ElementKind::Table { .. }))
                .count(),
            1
        );
    }

    #[cfg(all(feature = "layout-detection", any(feature = "ocr", feature = "ocr-pipeline")))]
    #[test]
    fn preparing_ocr_layout_inputs_transfers_raster_ownership() {
        let image = image::RgbImage::from_pixel(4, 3, image::Rgb([1, 2, 3]));
        let original_pixels = image.as_ptr();
        let detections = vec![crate::layout::DetectionResult {
            page_width: 4,
            page_height: 3,
            detections: Vec::new(),
        }];

        let (images, detections) = prepare_ocr_layout_inputs(vec![image], detections);

        let image::DynamicImage::ImageRgb8(transferred) = &images[0] else {
            panic!("RGB raster must retain its storage type");
        };
        assert_eq!(transferred.as_ptr(), original_pixels);
        assert_eq!((detections[0].page_width, detections[0].page_height), (4, 3));
    }

    #[cfg(all(feature = "layout-detection", any(feature = "ocr", feature = "ocr-pipeline")))]
    #[test]
    fn preparing_ocr_layout_inputs_discards_only_mismatched_page_detections() {
        let images = vec![image::RgbImage::new(4, 3), image::RgbImage::new(8, 6)];
        let detections = vec![
            crate::layout::DetectionResult {
                page_width: 4,
                page_height: 3,
                detections: Vec::new(),
            },
            crate::layout::DetectionResult {
                page_width: 16,
                page_height: 12,
                detections: Vec::new(),
            },
        ];

        let (_, detections) = prepare_ocr_layout_inputs(images, detections);

        assert_eq!((detections[0].page_width, detections[0].page_height), (4, 3));
        assert_eq!((detections[1].page_width, detections[1].page_height), (8, 6));
        assert!(detections[1].detections.is_empty());
    }

    #[cfg(all(feature = "layout-detection", any(feature = "ocr", feature = "ocr-pipeline")))]
    #[test]
    fn preparing_ocr_layout_inputs_repairs_detection_cardinality() {
        let images = vec![image::RgbImage::new(4, 3), image::RgbImage::new(8, 6)];

        let (_, detections) = prepare_ocr_layout_inputs(images, Vec::new());

        assert_eq!(detections.len(), 2);
        assert_eq!((detections[0].page_width, detections[0].page_height), (4, 3));
        assert_eq!((detections[1].page_width, detections[1].page_height), (8, 6));
    }

    #[cfg(feature = "ocr")]
    fn mk_decision(fallback: bool, whole_doc_failure: bool, failing_pages: Vec<u32>) -> ocr::OcrFallbackDecision {
        ocr::OcrFallbackDecision {
            stats: ocr::NativeTextStats::default(),
            avg_non_whitespace: 0.0,
            avg_alnum: 0.0,
            fallback,
            failing_pages,
            whole_doc_failure,
        }
    }

    #[cfg(all(feature = "pdf", feature = "ocr"))]
    struct MockPdfOcrBackend {
        name: &'static str,
        content: &'static str,
    }

    #[cfg(all(feature = "pdf", feature = "ocr"))]
    impl crate::plugins::Plugin for MockPdfOcrBackend {
        fn name(&self) -> &str {
            self.name
        }

        fn version(&self) -> String {
            "1.0.0".to_string()
        }

        fn initialize(&self) -> crate::Result<()> {
            Ok(())
        }

        fn shutdown(&self) -> crate::Result<()> {
            Ok(())
        }
    }

    #[cfg(all(feature = "pdf", feature = "ocr"))]
    #[async_trait::async_trait]
    impl crate::plugins::OcrBackend for MockPdfOcrBackend {
        fn backend_type(&self) -> crate::plugins::OcrBackendType {
            crate::plugins::OcrBackendType::Custom
        }

        fn supports_language(&self, _lang: &str) -> bool {
            true
        }

        async fn process_image(
            &self,
            _image_bytes: &[u8],
            _config: &crate::core::config::OcrConfig,
        ) -> crate::Result<crate::types::ExtractedDocument> {
            Ok(crate::types::ExtractedDocument {
                content: self.content.to_string(),
                mime_type: std::borrow::Cow::Borrowed("text/plain"),
                ..Default::default()
            })
        }
    }

    #[cfg(all(feature = "pdf", feature = "ocr"))]
    struct RegisteredOcrBackendGuard {
        name: &'static str,
    }

    #[cfg(all(feature = "pdf", feature = "ocr"))]
    impl Drop for RegisteredOcrBackendGuard {
        fn drop(&mut self) {
            let _ = crate::plugins::unregister_ocr_backend(self.name);
        }
    }

    #[cfg(all(feature = "pdf", feature = "ocr"))]
    fn register_mock_ocr_backend(name: &'static str, content: &'static str) -> RegisteredOcrBackendGuard {
        crate::plugins::register_ocr_backend(std::sync::Arc::new(MockPdfOcrBackend { name, content })).unwrap();
        RegisteredOcrBackendGuard { name }
    }

    #[test]
    fn test_pdf_extractor_plugin_interface() {
        let extractor = PdfExtractor::new();
        assert_eq!(extractor.name(), "pdf-extractor");
        assert!(extractor.initialize().is_ok());
        assert!(extractor.shutdown().is_ok());
    }

    #[test]
    fn test_pdf_extractor_supported_mime_types() {
        let extractor = PdfExtractor::new();
        let mime_types = extractor.supported_mime_types();
        assert_eq!(mime_types.len(), 1);
        assert!(mime_types.contains(&"application/pdf"));
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_should_fallback_to_ocr_for_empty_text() {
        assert!(ocr::evaluate_native_text_for_ocr("", Some(1), &OcrQualityThresholds::default()).fallback);
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_should_not_fallback_for_meaningful_text() {
        let sample = "This page has searchable vector text and should avoid OCR.";
        assert!(!ocr::evaluate_native_text_for_ocr(sample, Some(1), &OcrQualityThresholds::default()).fallback);
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_should_fallback_for_punctuation_only_text() {
        let sample = " . , ; : -- -- ";
        assert!(ocr::evaluate_native_text_for_ocr(sample, Some(2), &OcrQualityThresholds::default()).fallback);
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_per_page_ocr_no_boundaries_falls_back_to_whole_doc() {
        let text = "This document has enough meaningful words for evaluation purposes here.";
        let decision = ocr::evaluate_per_page_ocr(text, None, Some(1), &OcrQualityThresholds::default());
        assert!(!decision.fallback);
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_per_page_ocr_empty_boundaries_falls_back_to_whole_doc() {
        let text = "This document has enough meaningful words for evaluation purposes here.";
        let decision = ocr::evaluate_per_page_ocr(text, Some(&[]), Some(1), &OcrQualityThresholds::default());
        assert!(!decision.fallback);
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_per_page_ocr_all_pages_good() {
        use crate::types::PageBoundary;

        let page1 = "This first page has plenty of meaningful searchable text content here.";
        let page2 = "This second page also has plenty of meaningful searchable text content.";
        let text = format!("{}{}", page1, page2);
        let boundaries = vec![
            PageBoundary {
                byte_start: 0,
                byte_end: page1.len(),
                page_number: 1,
            },
            PageBoundary {
                byte_start: page1.len(),
                byte_end: text.len(),
                page_number: 2,
            },
        ];

        let decision = ocr::evaluate_per_page_ocr(&text, Some(&boundaries), Some(2), &OcrQualityThresholds::default());
        assert!(!decision.fallback);
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_per_page_ocr_one_bad_page_triggers_fallback() {
        use crate::types::PageBoundary;

        let good_page = "This page has plenty of meaningful searchable text content for extraction.";
        let bad_page = " . ; ";
        let text = format!("{}{}", good_page, bad_page);
        let boundaries = vec![
            PageBoundary {
                byte_start: 0,
                byte_end: good_page.len(),
                page_number: 1,
            },
            PageBoundary {
                byte_start: good_page.len(),
                byte_end: text.len(),
                page_number: 2,
            },
        ];

        let decision = ocr::evaluate_per_page_ocr(&text, Some(&boundaries), Some(2), &OcrQualityThresholds::default());
        assert!(decision.fallback);
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_per_page_ocr_empty_page_triggers_fallback() {
        use crate::types::PageBoundary;

        let good_page = "This page has plenty of meaningful searchable text content for extraction.";
        let empty_page = "";
        let text = format!("{}{}", good_page, empty_page);
        let boundaries = vec![
            PageBoundary {
                byte_start: 0,
                byte_end: good_page.len(),
                page_number: 1,
            },
            PageBoundary {
                byte_start: good_page.len(),
                byte_end: text.len(),
                page_number: 2,
            },
        ];

        let decision = ocr::evaluate_per_page_ocr(&text, Some(&boundaries), Some(2), &OcrQualityThresholds::default());
        assert!(decision.fallback);
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_per_page_ocr_preserves_document_stats_on_fallback() {
        use crate::types::PageBoundary;

        let good_page = "This page has plenty of meaningful searchable text content for extraction.";
        let bad_page = " . ; ";
        let text = format!("{}{}", good_page, bad_page);
        let boundaries = vec![
            PageBoundary {
                byte_start: 0,
                byte_end: good_page.len(),
                page_number: 1,
            },
            PageBoundary {
                byte_start: good_page.len(),
                byte_end: text.len(),
                page_number: 2,
            },
        ];

        let decision = ocr::evaluate_per_page_ocr(&text, Some(&boundaries), Some(2), &OcrQualityThresholds::default());
        assert!(decision.fallback);
        assert!(decision.stats.non_whitespace > 0);
        assert!(decision.stats.meaningful_words > 0);
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_per_page_ocr_invalid_boundaries_skipped() {
        use crate::types::PageBoundary;

        let text = "This page has plenty of meaningful searchable text content for extraction.";
        let boundaries = vec![
            PageBoundary {
                byte_start: 0,
                byte_end: text.len(),
                page_number: 1,
            },
            PageBoundary {
                byte_start: 999,
                byte_end: 9999,
                page_number: 2,
            },
        ];

        let decision = ocr::evaluate_per_page_ocr(text, Some(&boundaries), Some(1), &OcrQualityThresholds::default());
        assert!(!decision.fallback);
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_per_page_ocr_multi_page_correct_page_count() {
        let text = "ab cd ef";
        let decision_wrong = ocr::evaluate_native_text_for_ocr(text, None, &OcrQualityThresholds::default());
        let decision_correct = ocr::evaluate_native_text_for_ocr(text, Some(20), &OcrQualityThresholds::default());
        assert!(
            decision_correct.avg_non_whitespace < decision_wrong.avg_non_whitespace,
            "Correct page count should produce lower per-page averages"
        );
    }

    #[tokio::test]
    #[cfg(feature = "pdf")]
    async fn test_pdf_batch_mode_validates_page_config_enabled() {
        use crate::core::config::PageConfig;

        let extractor = PdfExtractor::new();

        let config = ExtractionConfig {
            pages: Some(PageConfig {
                extract_pages: true,
                insert_page_markers: false,
                marker_format: "<!-- PAGE {page_num} -->".to_string(),
            }),
            ..Default::default()
        };

        let pdf_path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test_documents/pdf/google_doc_document.pdf");
        if let Ok(content) = std::fs::read(pdf_path) {
            let result = extractor.extract_content(&content, "application/pdf", &config).await;
            assert!(
                result.is_ok(),
                "Failed to extract PDF with page config: {:?}",
                result.err()
            );

            let extraction_result = result.unwrap();
            let extraction_result = crate::extraction::derive::derive_extraction_result(
                extraction_result,
                true,
                crate::core::config::OutputFormat::Plain,
            );
            assert!(
                !extraction_result.content.is_empty(),
                "Content should be extracted from PDF"
            );
        }
    }

    #[tokio::test]
    #[cfg(feature = "pdf")]
    async fn test_pdf_batch_mode_validates_page_config_disabled() {
        let extractor = PdfExtractor::new();
        let config = ExtractionConfig::default();

        let pdf_path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test_documents/pdf/google_doc_document.pdf");
        if let Ok(content) = std::fs::read(pdf_path) {
            let result = extractor.extract_content(&content, "application/pdf", &config).await;
            assert!(
                result.is_ok(),
                "Failed to extract PDF without page config: {:?}",
                result.err()
            );

            let extraction_result = result.unwrap();
            let extraction_result = crate::extraction::derive::derive_extraction_result(
                extraction_result,
                true,
                crate::core::config::OutputFormat::Plain,
            );
            assert!(
                extraction_result.pages.is_none(),
                "Pages should not be extracted when pages config is None"
            );
        }
    }

    #[tokio::test]
    #[cfg(feature = "pdf")]
    async fn test_pdf_page_marker_validation() {
        use crate::core::config::PageConfig;

        let extractor = PdfExtractor::new();

        let config = ExtractionConfig {
            pages: Some(PageConfig {
                extract_pages: true,
                insert_page_markers: true,
                marker_format: "\n\n<!-- PAGE {page_num} -->\n\n".to_string(),
            }),
            ..Default::default()
        };

        let pdf_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test_documents/pdf/multi_page.pdf");
        if let Ok(content) = std::fs::read(pdf_path) {
            let result = extractor.extract_content(&content, "application/pdf", &config).await;
            assert!(
                result.is_ok(),
                "Failed to extract PDF with page markers: {:?}",
                result.err()
            );

            let extraction_result = result.unwrap();
            let extraction_result = crate::extraction::derive::derive_extraction_result(
                extraction_result,
                true,
                crate::core::config::OutputFormat::Plain,
            );
            let marker_placeholder = "<!-- PAGE ";
            if extraction_result.content.len() > 100 {
                assert!(
                    extraction_result.content.contains(marker_placeholder),
                    "Page markers should be inserted when configured and document has multiple pages"
                );
            }
        }
    }

    #[tokio::test]
    #[cfg(feature = "pdf")]
    async fn test_pdf_exposes_native_extraction_method() {
        let extractor = PdfExtractor::new();
        let config = ExtractionConfig::default();
        let pdf_path = pdf_test_document("google_doc_document.pdf");

        if let Ok(content) = std::fs::read(pdf_path) {
            let result = extractor
                .extract_content(&content, "application/pdf", &config)
                .await
                .expect("native PDF extraction should succeed");
            let result = crate::extraction::derive::derive_extraction_result(
                result,
                true,
                crate::core::config::OutputFormat::Plain,
            );

            assert_eq!(extraction_method(&result), Some(ExtractionMethod::Native));
        }
    }

    #[tokio::test]
    #[cfg(all(feature = "pdf", feature = "ocr"))]
    #[serial]
    async fn test_pdf_exposes_ocr_extraction_method() {
        use crate::core::config::OcrConfig;

        let _backend = register_mock_ocr_backend("pdf-extraction-method-ocr", "mock OCR text");
        let extractor = PdfExtractor::new();
        let config = ExtractionConfig {
            force_ocr: true,
            ocr: Some(OcrConfig {
                backend: "pdf-extraction-method-ocr".to_string(),
                language: vec!["eng".to_string()],
                ..Default::default()
            }),
            ..Default::default()
        };
        let pdf_path = pdf_test_document("multi_page.pdf");

        if let Ok(content) = std::fs::read(pdf_path) {
            let result = extractor
                .extract_content(&content, "application/pdf", &config)
                .await
                .expect("forced OCR extraction should succeed");
            let result = crate::extraction::derive::derive_extraction_result(
                result,
                true,
                crate::core::config::OutputFormat::Plain,
            );

            assert_eq!(extraction_method(&result), Some(ExtractionMethod::Ocr));
        }
    }

    #[tokio::test]
    #[cfg(all(feature = "pdf", feature = "ocr", feature = "chunking"))]
    #[serial]
    async fn test_mixed_pdf_ocr_survives_full_postprocessing() {
        use crate::core::config::{ChunkingConfig, OcrConfig, PageConfig};

        const OCR_TEXT: &str = "Issue 1281 authoritative OCR replacement on page one.";
        const RETAINED_PAGE_TWO_MARKER: &str = "Other notable software from this era included WordPerfect";
        let _backend = register_mock_ocr_backend("pdf-extraction-method-mixed", OCR_TEXT);
        let extractor = PdfExtractor::new();
        let pdf_path = pdf_test_document("multi_page.pdf");
        let content = std::fs::read(pdf_path).expect("mixed OCR fixture must be available");
        let native_config = ExtractionConfig {
            pages: Some(PageConfig {
                extract_pages: true,
                ..Default::default()
            }),
            ..Default::default()
        };
        let native = extractor
            .extract_content(&content, "application/pdf", &native_config)
            .await
            .expect("native PDF extraction should succeed");
        let native =
            crate::extraction::derive::derive_extraction_result(native, true, crate::core::config::OutputFormat::Plain);
        let native_pages = native.pages.expect("native extraction must expose pages");
        let stale_page_one = native_pages[0].content.clone();

        let config = ExtractionConfig {
            force_ocr_pages: Some(vec![1]),
            output_format: crate::core::config::OutputFormat::Markdown,
            include_document_structure: true,
            use_cache: false,
            ocr: Some(OcrConfig {
                backend: "pdf-extraction-method-mixed".to_string(),
                language: vec!["eng".to_string()],
                ..Default::default()
            }),
            pages: Some(PageConfig {
                extract_pages: true,
                ..Default::default()
            }),
            chunking: Some(ChunkingConfig {
                max_characters: OCR_TEXT.chars().count() + 1,
                overlap: 0,
                ..Default::default()
            }),
            ..Default::default()
        };
        let internal = extractor
            .extract_content(&content, "application/pdf", &config)
            .await
            .expect("mixed OCR/native extraction should succeed");
        let derived = crate::extraction::derive::derive_extraction_result(
            internal.clone(),
            true,
            crate::core::config::OutputFormat::Markdown,
        );
        assert_occurs_once(&derived.content, OCR_TEXT, "derived plain content");
        assert_occurs_once(
            derived
                .formatted_content
                .as_deref()
                .expect("derived Markdown output must exist"),
            OCR_TEXT,
            "derived Markdown output",
        );
        let result = crate::core::pipeline::run_pipeline(internal, &config)
            .await
            .expect("mixed OCR post-processing should succeed");

        assert_eq!(extraction_method(&result), Some(ExtractionMethod::Mixed));
        assert_occurs_once(&result.content, OCR_TEXT, "post-processed Markdown content");
        assert!(!result.content.contains(&stale_page_one));
        assert!(result.content.contains(RETAINED_PAGE_TWO_MARKER));

        let pages = result.pages.as_ref().expect("mixed extraction must expose pages");
        assert_occurs_once(&pages[0].content, OCR_TEXT, "page one");
        assert!(!pages[0].content.contains(&stale_page_one));
        assert_occurs_once(&pages[1].content, RETAINED_PAGE_TWO_MARKER, "retained page two");
        assert!(!pages[1].content.contains(OCR_TEXT));

        let document = result.document.as_ref().expect("document structure must be derived");
        let ocr_nodes: Vec<_> = document
            .nodes
            .iter()
            .filter(|node| node.content.text().is_some_and(|text| text.contains(OCR_TEXT)))
            .collect();
        assert_eq!(ocr_nodes.len(), 1, "document structure must not duplicate OCR text");
        assert_eq!(ocr_nodes[0].page, Some(1));
        assert!(
            !document
                .nodes
                .iter()
                .any(|node| node.content.text().is_some_and(|text| text.contains(&stale_page_one)))
        );

        let chunks = result.chunks.as_ref().expect("chunking must run");
        let ocr_chunks: Vec<_> = chunks.iter().filter(|chunk| chunk.content.contains(OCR_TEXT)).collect();
        assert_eq!(ocr_chunks.len(), 1, "chunks must not duplicate OCR text");
        assert_eq!(ocr_chunks[0].metadata.first_page, Some(1));
        assert_eq!(ocr_chunks[0].metadata.last_page, Some(1));
    }

    #[tokio::test]
    #[cfg(all(feature = "pdf", feature = "ocr", feature = "chunking"))]
    #[serial]
    async fn test_scanned_page_strategy_automatically_routes_only_the_scan_to_ocr() {
        use crate::core::config::{ChunkingConfig, OcrConfig, OcrStrategy, PageConfig};

        const NATIVE_TEXT: &str = "Issue 1281 native text remains on page one";
        const OCR_TEXT: &str = "Issue 1281 automatic OCR replacement on page two.";
        let _backend = register_mock_ocr_backend("pdf-automatic-mixed-routing", OCR_TEXT);
        let config = ExtractionConfig {
            ocr_strategy: OcrStrategy::ScannedPages { min_confidence: 0.7 },
            output_format: crate::core::config::OutputFormat::Markdown,
            include_document_structure: true,
            use_cache: false,
            ocr: Some(OcrConfig {
                backend: "pdf-automatic-mixed-routing".to_string(),
                language: vec!["eng".to_string()],
                ..Default::default()
            }),
            pages: Some(PageConfig {
                extract_pages: true,
                ..Default::default()
            }),
            chunking: Some(ChunkingConfig {
                max_characters: OCR_TEXT.chars().count() + 1,
                overlap: 0,
                ..Default::default()
            }),
            ..Default::default()
        };

        let internal = PdfExtractor::new()
            .extract_content(&mixed_native_and_scanned_pdf(), "application/pdf", &config)
            .await
            .expect("automatic mixed PDF extraction should succeed");
        let derived = crate::extraction::derive::derive_extraction_result(
            internal.clone(),
            true,
            crate::core::config::OutputFormat::Markdown,
        );
        assert_occurs_once(&derived.content, OCR_TEXT, "automatic derived content");
        assert_occurs_once(
            derived
                .formatted_content
                .as_deref()
                .expect("automatic derived Markdown output must exist"),
            OCR_TEXT,
            "automatic derived Markdown output",
        );
        let result = crate::core::pipeline::run_pipeline(internal, &config)
            .await
            .expect("automatic mixed PDF post-processing should succeed");

        assert_eq!(extraction_method(&result), Some(ExtractionMethod::Mixed));
        assert_occurs_once(&result.content, NATIVE_TEXT, "automatic mixed Markdown content");
        assert_occurs_once(&result.content, OCR_TEXT, "automatic mixed Markdown content");
        let pages = result
            .pages
            .as_ref()
            .expect("automatic mixed extraction must expose pages");
        assert_eq!(pages.len(), 2);
        assert!(pages[0].content.contains(NATIVE_TEXT));
        assert!(!pages[0].content.contains(OCR_TEXT));
        assert_occurs_once(&pages[1].content, OCR_TEXT, "automatically OCR'd page two");
        assert!(!pages[1].content.contains(NATIVE_TEXT));

        let document = result.document.as_ref().expect("document structure must be derived");
        let ocr_nodes: Vec<_> = document
            .nodes
            .iter()
            .filter(|node| node.content.text().is_some_and(|text| text.contains(OCR_TEXT)))
            .collect();
        assert_eq!(ocr_nodes.len(), 1);
        assert_eq!(ocr_nodes[0].page, Some(2));

        let chunks = result.chunks.as_ref().expect("automatic mixed chunking must run");
        let ocr_chunks: Vec<_> = chunks.iter().filter(|chunk| chunk.content.contains(OCR_TEXT)).collect();
        assert_eq!(ocr_chunks.len(), 1, "automatic chunks must not duplicate OCR text");
        assert_eq!(ocr_chunks[0].metadata.first_page, Some(2));
        assert_eq!(ocr_chunks[0].metadata.last_page, Some(2));
    }

    #[tokio::test]
    #[cfg(all(feature = "pdf", feature = "ocr"))]
    async fn test_pdf_force_ocr_without_ocr_config() {
        use crate::core::config::ExtractionConfig;

        let extractor = PdfExtractor::new();

        let config = ExtractionConfig {
            force_ocr: true,
            ocr: None,
            ..Default::default()
        };

        let pdf_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test_documents/pdf/multi_page.pdf");
        if let Ok(content) = std::fs::read(pdf_path) {
            let result = extractor.extract_content(&content, "application/pdf", &config).await;

            if let Err(e) = result {
                assert!(
                    !e.to_string().contains("OCR config required for force_ocr"),
                    "Should not require manual OCR config when force_ocr is true"
                );
            }
        }
    }

    /// Verifies that per-page OCR text segments correctly override native page
    /// content in each `PageContent` entry (#928).
    #[cfg(all(feature = "pdf", feature = "ocr"))]
    #[tokio::test]
    async fn test_ocr_page_texts_override_native_page_content() {
        use crate::core::config::OcrConfig;
        use crate::plugins::{OcrBackend, OcrBackendType, Plugin};
        use crate::types::ExtractedDocument;
        use std::sync::Arc;

        struct PerPageMockBackend;

        #[async_trait::async_trait]
        impl OcrBackend for PerPageMockBackend {
            fn backend_type(&self) -> OcrBackendType {
                OcrBackendType::Custom
            }
            fn supports_language(&self, _: &str) -> bool {
                true
            }
            async fn process_image(&self, _: &[u8], _: &OcrConfig) -> crate::Result<ExtractedDocument> {
                static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
                let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(ExtractedDocument {
                    content: format!("ocr-page-{n}"),
                    ..Default::default()
                })
            }
            fn supports_document_processing(&self) -> bool {
                false
            }
        }

        impl Plugin for PerPageMockBackend {
            fn name(&self) -> &str {
                "per-page-ocr-mock-928"
            }
            fn version(&self) -> String {
                "1.0.0".to_string()
            }
            fn initialize(&self) -> crate::Result<()> {
                Ok(())
            }
            fn shutdown(&self) -> crate::Result<()> {
                Ok(())
            }
        }

        crate::plugins::register_ocr_backend(Arc::new(PerPageMockBackend)).unwrap();

        use image::ImageEncoder as _;
        let make_png = || {
            let img = image::DynamicImage::new_rgb8(1, 1);
            let rgb = img.to_rgb8();
            let (w, h) = rgb.dimensions();
            let mut buf = std::io::Cursor::new(Vec::new());
            image::codecs::png::PngEncoder::new(&mut buf)
                .write_image(&rgb, w, h, image::ColorType::Rgb8.into())
                .unwrap();
            image::load_from_memory(&buf.into_inner()).unwrap()
        };
        let images = vec![make_png(), make_png()];

        let config = crate::core::config::ExtractionConfig {
            ocr: Some(OcrConfig {
                backend: "per-page-ocr-mock-928".to_string(),
                ..Default::default()
            }),
            ..Default::default()
        };

        let result = super::ocr::extract_with_ocr(
            None,
            Some(&images),
            #[cfg(feature = "layout-detection")]
            None,
            &config,
            None,
        )
        .await;

        crate::plugins::unregister_ocr_backend("per-page-ocr-mock-928").unwrap();

        let (_text, _conf, _tables, _elems, _doc, _llm, page_texts, _rasters, _formulas) =
            result.expect("extract_with_ocr should succeed");

        assert_eq!(page_texts.len(), 2, "expected one entry per page");
        assert!(page_texts[0].starts_with("ocr-page-"), "page 0 should have OCR text");
        assert!(page_texts[1].starts_with("ocr-page-"), "page 1 should have OCR text");
        assert_ne!(page_texts[0], page_texts[1], "each page should get unique OCR text");
    }

    /// Verifies that when a VLM returns a single string for a multi-page PDF,
    /// the guard clears stale native text on secondary pages (#928).
    #[cfg(feature = "ocr")]
    #[test]
    fn test_vlm_single_string_guard_clears_secondary_pages() {
        use crate::types::PageContent;

        let vlm_text = "whole-doc VLM summary".to_string();
        let pts = vec![vlm_text.clone()];
        let pts_len = pts.len();

        let mut pages: Vec<PageContent> = (1u32..=3u32)
            .map(|n| PageContent {
                page_number: n,
                content: format!("native page {n}"),
                tables: Vec::new(),
                image_indices: Vec::new(),
                hierarchy: None,
                is_blank: None,
                layout_regions: None,
                speaker_notes: None,
                section_name: None,
                sheet_name: None,
            })
            .collect();
        let pages_len = pages.len();

        for (page, text) in pages.iter_mut().zip(pts) {
            page.content = crate::pdf::text::fix_pdf_control_chars(&text).into_owned();
            page.is_blank = Some(crate::extraction::blank_detection::is_page_text_blank(&page.content));
        }
        if pts_len == 1 && pages_len > 1 {
            for p in pages.iter_mut().skip(1) {
                p.content.clear();
                p.is_blank = Some(true);
            }
        }

        assert_eq!(pages[0].content, vlm_text, "page 1 should carry the VLM text");
        assert!(pages[1].content.is_empty(), "page 2 must be cleared by VLM guard");
        assert!(pages[2].content.is_empty(), "page 3 must be cleared by VLM guard");
        assert_eq!(
            pages[0].is_blank,
            Some(false),
            "page 1 has VLM content so must not be blank"
        );
        assert_eq!(pages[1].is_blank, Some(true), "page 2 was cleared so must be blank");
        assert_eq!(pages[2].is_blank, Some(true), "page 3 was cleared so must be blank");
    }

    /// Regression for #1095: when OCR texts are written into existing PageContent entries,
    /// is_blank must be recalculated from the new content, not left stale from native extraction.
    ///
    /// Simulates scanned PDF pages (all blank natively) receiving OCR content.
    #[cfg(feature = "ocr")]
    #[test]
    fn test_ocr_page_texts_update_is_blank_on_existing_pages() {
        use crate::extraction::blank_detection::is_page_text_blank;
        use crate::types::PageContent;

        let pts = vec!["page one content".to_string(), "page two content".to_string()];
        let pts_len = pts.len();

        let mut pages: Vec<PageContent> = (1u32..=2u32)
            .map(|n| PageContent {
                page_number: n,
                content: String::new(),
                tables: Vec::new(),
                image_indices: Vec::new(),
                hierarchy: None,
                is_blank: Some(true),
                layout_regions: None,
                speaker_notes: None,
                section_name: None,
                sheet_name: None,
            })
            .collect();
        let pages_len = pages.len();

        for (page, text) in pages.iter_mut().zip(pts) {
            page.content = crate::pdf::text::fix_pdf_control_chars(&text).into_owned();
            page.is_blank = Some(is_page_text_blank(&page.content));
        }
        if pts_len == 1 && pages_len > 1 {
            for p in pages.iter_mut().skip(1) {
                p.content.clear();
                p.is_blank = Some(true);
            }
        }

        assert_eq!(
            pages[0].is_blank,
            Some(false),
            "page with OCR content must not be blank"
        );
        assert_eq!(
            pages[1].is_blank,
            Some(false),
            "page with OCR content must not be blank"
        );
    }

    /// Regression for #1095: pages built from OCR texts when no native pages exist
    /// must have is_blank derived from the OCR content, not left as None.
    #[cfg(feature = "ocr")]
    #[test]
    fn test_ocr_scratch_pages_is_blank_set_from_content() {
        use crate::extraction::blank_detection::is_page_text_blank;
        use crate::types::PageContent;

        let pts = vec!["substantial ocr content".to_string(), String::new()];

        let page_contents: Vec<PageContent> = pts
            .into_iter()
            .enumerate()
            .map(|(i, text)| {
                let content = crate::pdf::text::fix_pdf_control_chars(&text).into_owned();
                let is_blank = Some(is_page_text_blank(&content));
                PageContent {
                    page_number: (i + 1) as u32,
                    content,
                    tables: Vec::new(),
                    image_indices: vec![],
                    hierarchy: None,
                    is_blank,
                    layout_regions: None,
                    speaker_notes: None,
                    section_name: None,
                    sheet_name: None,
                }
            })
            .collect();

        assert_eq!(
            page_contents[0].is_blank,
            Some(false),
            "page with content must not be blank"
        );
        assert_eq!(
            page_contents[1].is_blank,
            Some(true),
            "page with empty content must be blank"
        );
    }

    /// Integration regression for #1095: force_ocr on a scanned (non-searchable) PDF with
    /// extract_pages=true must produce pages where is_blank reflects OCR content, not stale
    /// native-extraction state.
    #[tokio::test]
    #[cfg(all(feature = "pdf", feature = "ocr"))]
    #[serial]
    async fn test_force_ocr_sets_is_blank_from_ocr_content() {
        use crate::core::config::{OcrConfig, PageConfig};

        let _backend = register_mock_ocr_backend("is-blank-fix-1095", "extracted ocr text content here");
        let extractor = PdfExtractor::new();
        let config = ExtractionConfig {
            force_ocr: true,
            ocr: Some(OcrConfig {
                backend: "is-blank-fix-1095".to_string(),
                language: vec!["eng".to_string()],
                ..Default::default()
            }),
            pages: Some(PageConfig {
                extract_pages: true,
                ..Default::default()
            }),
            ..Default::default()
        };

        let pdf_path = pdf_test_document("non_searchable.pdf");
        let content = std::fs::read(&pdf_path).unwrap_or_else(|e| panic!("non_searchable.pdf must be readable: {e}"));
        let result = extractor
            .extract_content(&content, "application/pdf", &config)
            .await
            .expect("extraction should succeed");
        let result = crate::extraction::derive::derive_extraction_result(
            result,
            false,
            crate::core::config::OutputFormat::Plain,
        );

        let pages = result.pages.expect("pages must be present when extract_pages=true");
        assert!(!pages.is_empty(), "must have at least one page");
        for page in &pages {
            assert_eq!(
                page.is_blank,
                Some(false),
                "page {} has OCR content, is_blank must be Some(false) (issue #1095)",
                page.page_number
            );
        }
    }

    /// Regression for #1293: `metadata.pages.pages[].is_blank` must agree with the
    /// top-level `pages[].is_blank` for OCR'd pages. Previously `PageInfo.is_blank`
    /// was computed once from native content in `build_page_structure` and never
    /// refreshed after OCR text overwrote `PageContent`, so a scanned page with no
    /// native text but real OCR text reported `is_blank=true` in metadata while the
    /// top-level page reported `is_blank=false`.
    #[tokio::test]
    #[cfg(all(feature = "pdf", feature = "ocr"))]
    #[serial]
    async fn test_metadata_page_info_is_blank_matches_page_content_after_ocr() {
        use crate::core::config::{OcrConfig, PageConfig};

        let _backend = register_mock_ocr_backend("is-blank-metadata-fix-1293", "extracted ocr text content here");
        let extractor = PdfExtractor::new();
        let config = ExtractionConfig {
            force_ocr: true,
            ocr: Some(OcrConfig {
                backend: "is-blank-metadata-fix-1293".to_string(),
                language: vec!["eng".to_string()],
                ..Default::default()
            }),
            pages: Some(PageConfig {
                extract_pages: true,
                ..Default::default()
            }),
            ..Default::default()
        };

        let pdf_path = pdf_test_document("non_searchable.pdf");
        let content = std::fs::read(&pdf_path).unwrap_or_else(|e| panic!("non_searchable.pdf must be readable: {e}"));
        let result = extractor
            .extract_content(&content, "application/pdf", &config)
            .await
            .expect("extraction should succeed");
        let result = crate::extraction::derive::derive_extraction_result(
            result,
            false,
            crate::core::config::OutputFormat::Plain,
        );

        let pages = result
            .pages
            .clone()
            .expect("pages must be present when extract_pages=true");
        let metadata_pages = result
            .metadata
            .pages
            .as_ref()
            .and_then(|structure| structure.pages.as_ref())
            .expect("metadata.pages.pages must be present when extract_pages=true");

        assert!(!pages.is_empty(), "must have at least one page");
        assert_eq!(
            pages.len(),
            metadata_pages.len(),
            "page counts must match between the two sources"
        );

        for page in &pages {
            let info = metadata_pages
                .iter()
                .find(|p| p.number == page.page_number)
                .unwrap_or_else(|| panic!("metadata.pages.pages missing entry for page {}", page.page_number));
            assert_eq!(
                info.is_blank, page.is_blank,
                "page {}: metadata.pages.pages[].is_blank ({:?}) must match top-level pages[].is_blank ({:?}) (issue #1293)",
                page.page_number, info.is_blank, page.is_blank
            );
            assert_eq!(
                info.is_blank,
                Some(false),
                "page {} has OCR content, metadata is_blank must be Some(false) (issue #1293)",
                page.page_number
            );
        }
    }

    /// ocr_inline_images=true on a text-only PDF (no embedded images) must succeed
    /// and return an empty images list, not panic or error.
    #[tokio::test]
    #[cfg(feature = "pdf")]
    async fn test_pdf_ocr_inline_images_no_images_in_document() {
        use crate::core::config::ExtractionConfig;
        use crate::core::config::pdf::PdfConfig;

        let extractor = PdfExtractor::new();

        let pdf_path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test_documents/pdf/code_and_formula.pdf");

        if !pdf_path.exists() {
            panic!("missing test fixture: {pdf_path:?}");
        }

        let content = std::fs::read(pdf_path).expect("Failed to read PDF");

        let config = ExtractionConfig {
            pdf_options: Some(PdfConfig {
                ocr_inline_images: true,
                ..Default::default()
            }),
            ..Default::default()
        };

        let result = extractor
            .extract_content(&content, "application/pdf", &config)
            .await
            .expect("Extraction should succeed even when there are no images to OCR");

        for img in &result.images {
            assert!(img.ocr_result.is_none(), "text-only PDF should produce no OCR results");
        }
    }

    /// ocr_inline_images=true with config.ocr=None must use TesseractConfig::default()
    /// and not panic.
    #[tokio::test]
    #[cfg(all(feature = "pdf", feature = "ocr"))]
    async fn test_pdf_ocr_inline_images_no_ocr_config() {
        use crate::core::config::ExtractionConfig;
        use crate::core::config::pdf::PdfConfig;

        let extractor = PdfExtractor::new();

        let pdf_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../test_documents/pdf/embedded_images_tables.pdf");

        assert!(
            pdf_path.exists(),
            "missing test fixture: {pdf_path:?} — add embedded_images_tables.pdf to test_documents/pdf/"
        );

        let content = std::fs::read(pdf_path).expect("Failed to read PDF");

        let config = ExtractionConfig {
            ocr: None,
            pdf_options: Some(PdfConfig {
                ocr_inline_images: true,
                ..Default::default()
            }),
            ..Default::default()
        };

        let _result = extractor
            .extract_content(&content, "application/pdf", &config)
            .await
            .expect("Extraction with ocr=None and ocr_inline_images=true must not panic");
    }

    /// Regression for issue #917: a mixed document with good aggregate text but a
    /// scanned page must still reach OCR. Before the fix, `has_substantive_doc=true`
    /// alone suppressed OCR even when `decision.fallback=true`.
    ///
    /// Tests `evaluate_ocr_skip_gate` directly — the function that the production
    /// path delegates to — so that reverting `&& !decision_fallback` breaks this test.
    #[cfg(feature = "ocr")]
    #[test]
    fn test_ocr_gate_runs_ocr_when_substantive_doc_but_fallback_needed() {
        let thresholds = OcrQualityThresholds::default();

        let outcome = ocr::evaluate_ocr_skip_gate(true, 500, 0.9, &mk_decision(true, true, vec![]), &thresholds);
        assert_eq!(
            outcome,
            ocr::OcrGateOutcome::RunFallback,
            "substantive doc must not suppress OCR when per-page fallback is needed (issue #917)"
        );
    }

    /// Counterpart: when no per-page fallback is needed, a substantive doc correctly
    /// skips OCR. Ensures the fix doesn't over-correct and always run OCR.
    #[cfg(feature = "ocr")]
    #[test]
    fn test_ocr_gate_skips_when_substantive_doc_and_no_fallback() {
        let thresholds = OcrQualityThresholds::default();

        let outcome = ocr::evaluate_ocr_skip_gate(true, 500, 0.9, &mk_decision(false, false, vec![]), &thresholds);
        assert_eq!(
            outcome,
            ocr::OcrGateOutcome::SkipSubstantive,
            "OCR should be skipped when doc is substantive and no per-page fallback is needed"
        );
    }

    /// Regression for #987: image placeholders must appear in Markdown output when
    /// `force_ocr` is used and `config.images.inject_placeholders = true`.
    ///
    /// Uses a mock OCR backend so the test is independent of tessdata availability.
    /// Image elements are appended after text on the OCR path; positional interleaving
    /// is a known follow-up (tracked separately).
    #[tokio::test]
    #[cfg(all(feature = "pdf", feature = "ocr"))]
    #[serial]
    async fn test_inject_placeholders_present_on_force_ocr_path() {
        use crate::core::config::{ImageExtractionConfig, OcrConfig, OutputFormat};

        let _backend = register_mock_ocr_backend("inject-placeholder-ocr", "mock page text");
        let extractor = PdfExtractor::new();

        let pdf_path = pdf_test_document("embedded_images_tables.pdf");
        assert!(
            pdf_path.exists(),
            "missing test fixture: {pdf_path:?} — add embedded_images_tables.pdf to test_documents/pdf/"
        );
        let content = std::fs::read(&pdf_path).expect("failed to read embedded_images_tables.pdf");

        let config = crate::core::config::ExtractionConfig {
            output_format: OutputFormat::Markdown,
            force_ocr: true,
            ocr: Some(OcrConfig {
                backend: "inject-placeholder-ocr".to_string(),
                language: vec!["eng".to_string()],
                ..Default::default()
            }),
            images: Some(ImageExtractionConfig {
                extract_images: true,
                inject_placeholders: true,
                ..Default::default()
            }),
            ..Default::default()
        };

        let result = extractor
            .extract_content(&content, "application/pdf", &config)
            .await
            .expect("force_ocr extraction with images should succeed");

        let result = crate::extraction::derive::derive_extraction_result(result, true, OutputFormat::Markdown);

        if result.images.as_ref().is_some_and(|imgs| !imgs.is_empty()) {
            assert!(
                result.formatted_content.as_deref().unwrap_or("").contains("![") || result.content.contains("!["),
                "Markdown must contain image placeholders on the force_ocr path when inject_placeholders=true"
            );
        }
    }

    /// Verifies that `inject_placeholders` defaults to false when only
    /// `pdf_options.extract_images` is set and `config.images` is absent,
    /// so callers who never touched `config.images` do not get unexpected placeholders.
    #[tokio::test]
    #[cfg(all(feature = "pdf", feature = "ocr"))]
    #[serial]
    async fn test_inject_placeholders_absent_when_only_pdf_options_set() {
        use crate::core::config::{OcrConfig, OutputFormat, pdf::PdfConfig};

        let _backend = register_mock_ocr_backend("inject-placeholder-absent-ocr", "mock page text");
        let extractor = PdfExtractor::new();

        let pdf_path = pdf_test_document("embedded_images_tables.pdf");
        assert!(
            pdf_path.exists(),
            "missing test fixture: {pdf_path:?} — add embedded_images_tables.pdf to test_documents/pdf/"
        );
        let content = std::fs::read(&pdf_path).expect("failed to read embedded_images_tables.pdf");

        let config = crate::core::config::ExtractionConfig {
            output_format: OutputFormat::Markdown,
            force_ocr: true,
            ocr: Some(OcrConfig {
                backend: "inject-placeholder-absent-ocr".to_string(),
                language: vec!["eng".to_string()],
                ..Default::default()
            }),
            pdf_options: Some(PdfConfig {
                extract_images: true,
                ..Default::default()
            }),
            ..Default::default()
        };

        let result = extractor
            .extract_content(&content, "application/pdf", &config)
            .await
            .expect("force_ocr extraction with pdf_options should succeed");

        let result = crate::extraction::derive::derive_extraction_result(result, true, OutputFormat::Markdown);

        let markdown = result.formatted_content.as_deref().unwrap_or(&result.content);
        assert!(
            !markdown.contains("!["),
            "Markdown must NOT contain image placeholders when config.images is absent (inject_placeholders defaults to false)"
        );
    }

    /// Non-textual content (charts, diagrams) with a pre-rendered structured doc
    /// present should skip OCR regardless of the per-page fallback flag — OCR
    /// won't improve extraction quality for non-textual pages.
    #[cfg(feature = "ocr")]
    #[test]
    fn test_ocr_gate_skips_non_textual_content_even_when_fallback_requested() {
        let thresholds = OcrQualityThresholds::default();

        let outcome = ocr::evaluate_ocr_skip_gate(true, 500, 0.1, &mk_decision(true, true, vec![]), &thresholds);
        assert_eq!(
            outcome,
            ocr::OcrGateOutcome::SkipNonText,
            "non-textual content with a structured doc must skip OCR even if fallback was requested"
        );
    }

    /// Hybrid PDF: per-page check fires on specific pages while the whole-document
    /// quality check passes. Gate must route to RunFallbackOnPages, not RunFallback.
    #[cfg(feature = "ocr")]
    #[test]
    fn test_ocr_gate_targets_specific_pages_on_hybrid_pdf() {
        let thresholds = OcrQualityThresholds::default();

        let outcome = ocr::evaluate_ocr_skip_gate(false, 500, 0.9, &mk_decision(true, false, vec![3, 7]), &thresholds);
        assert_eq!(
            outcome,
            ocr::OcrGateOutcome::RunFallbackOnPages(vec![3, 7]),
            "hybrid PDF with specific failing pages must route to targeted OCR"
        );
    }

    /// Whole-document failure with no per-page list → full document OCR (existing behaviour).
    #[cfg(feature = "ocr")]
    #[test]
    fn test_ocr_gate_full_document_when_whole_doc_failure() {
        let thresholds = OcrQualityThresholds::default();
        let outcome = ocr::evaluate_ocr_skip_gate(false, 500, 0.9, &mk_decision(true, true, vec![]), &thresholds);
        assert_eq!(outcome, ocr::OcrGateOutcome::RunFallback);
    }

    /// Edge case: whole-doc failure is true AND per-page list is populated.
    /// Whole-doc failure dominates (the document is fundamentally bad).
    #[cfg(feature = "ocr")]
    #[test]
    fn test_ocr_gate_whole_doc_failure_dominates_per_page_list() {
        let thresholds = OcrQualityThresholds::default();
        let outcome =
            ocr::evaluate_ocr_skip_gate(false, 500, 0.9, &mk_decision(true, true, vec![1, 2, 3]), &thresholds);
        assert_eq!(
            outcome,
            ocr::OcrGateOutcome::RunFallback,
            "whole-doc failure must trigger full OCR even when per-page list is populated"
        );
    }

    /// evaluate_per_page_ocr must collect ALL failing pages, not short-circuit on the first.
    /// This is the core regression the original implementation had.
    #[cfg(feature = "ocr")]
    #[test]
    fn test_evaluate_per_page_ocr_collects_all_failing_pages() {
        use crate::types::PageBoundary;

        let good = "This page has plenty of meaningful searchable text content for extraction.";
        let bad = " . ; ";
        let text = format!("{}{}{}{}", good, bad, good, bad);
        let boundaries = vec![
            PageBoundary {
                byte_start: 0,
                byte_end: good.len(),
                page_number: 1,
            },
            PageBoundary {
                byte_start: good.len(),
                byte_end: good.len() + bad.len(),
                page_number: 2,
            },
            PageBoundary {
                byte_start: good.len() + bad.len(),
                byte_end: 2 * good.len() + bad.len(),
                page_number: 3,
            },
            PageBoundary {
                byte_start: 2 * good.len() + bad.len(),
                byte_end: text.len(),
                page_number: 4,
            },
        ];

        let decision = ocr::evaluate_per_page_ocr(&text, Some(&boundaries), Some(4), &OcrQualityThresholds::default());
        assert!(decision.fallback);
        assert_eq!(
            decision.failing_pages,
            vec![2, 4],
            "all failing pages must be collected, not just the first"
        );
        assert!(
            !decision.whole_doc_failure,
            "whole-doc check should pass when half the document has good text"
        );
    }

    /// When every page fails the per-page quality check, the gate must route to
    /// RunFallback (ExtractionMethod::Ocr), not RunFallbackOnPages (ExtractionMethod::Mixed).
    /// A document where every page needs OCR is not a mixed document.
    #[cfg(feature = "ocr")]
    #[test]
    fn test_all_pages_failing_routes_to_run_fallback_not_mixed() {
        use crate::types::PageBoundary;

        let bad = " . ; ";
        let text = format!("{}{}", bad, bad);
        let boundaries = vec![
            PageBoundary {
                byte_start: 0,
                byte_end: bad.len(),
                page_number: 1,
            },
            PageBoundary {
                byte_start: bad.len(),
                byte_end: text.len(),
                page_number: 2,
            },
        ];

        let decision = ocr::evaluate_per_page_ocr(&text, Some(&boundaries), Some(2), &OcrQualityThresholds::default());
        assert!(decision.fallback);
        assert!(
            decision.failing_pages.is_empty(),
            "doc-level failure fires before per-page scan when all pages fail"
        );
        assert!(
            decision.whole_doc_failure,
            "all pages failing must set whole_doc_failure so gate routes to RunFallback"
        );

        let outcome = ocr::evaluate_ocr_skip_gate(false, text.len(), 0.1, &decision, &OcrQualityThresholds::default());
        assert_eq!(
            outcome,
            ocr::OcrGateOutcome::RunFallback,
            "all-pages-failing must produce RunFallback (Ocr), not RunFallbackOnPages (Mixed)"
        );
    }

    /// Mock backend that records the OcrConfig it receives so tests can assert
    /// that fields like output_format are forwarded correctly.
    #[cfg(all(feature = "pdf", feature = "ocr"))]
    struct ConfigCapturingBackend {
        name: &'static str,
        sentinel: &'static str,
        received_config: std::sync::Arc<std::sync::Mutex<Option<crate::core::config::OcrConfig>>>,
    }

    #[cfg(all(feature = "pdf", feature = "ocr"))]
    impl crate::plugins::Plugin for ConfigCapturingBackend {
        fn name(&self) -> &str {
            self.name
        }
        fn version(&self) -> String {
            "0.0.0".to_string()
        }
        fn initialize(&self) -> crate::Result<()> {
            Ok(())
        }
        fn shutdown(&self) -> crate::Result<()> {
            Ok(())
        }
    }

    #[cfg(all(feature = "pdf", feature = "ocr"))]
    #[async_trait::async_trait]
    impl crate::plugins::OcrBackend for ConfigCapturingBackend {
        fn backend_type(&self) -> crate::plugins::OcrBackendType {
            crate::plugins::OcrBackendType::Custom
        }
        fn supports_language(&self, _: &str) -> bool {
            true
        }
        async fn process_image(
            &self,
            _image_bytes: &[u8],
            config: &crate::core::config::OcrConfig,
        ) -> crate::Result<crate::types::ExtractedDocument> {
            *self.received_config.lock().unwrap() = Some(config.clone());
            Ok(crate::types::ExtractedDocument {
                content: self.sentinel.to_string(),
                mime_type: std::borrow::Cow::Borrowed("text/plain"),
                ..Default::default()
            })
        }
    }

    /// Regression for #1088: ocr_inline_images must call the backend named in
    /// OcrConfig.backend, not always Tesseract via OcrProcessor.
    ///
    /// Uses the existing register_mock_ocr_backend helper. The sentinel string
    /// appearing in img.ocr_result.content proves which backend ran — no separate
    /// AtomicBool needed.
    ///
    /// Fixture note: with_images.pdf is used here (not embedded_images_tables.pdf)
    /// because pdf_oxide reliably extracts its single raster XObject.
    #[tokio::test]
    #[cfg(all(feature = "pdf", feature = "ocr"))]
    #[serial]
    async fn test_ocr_inline_images_uses_configured_backend() {
        const BACKEND_NAME: &str = "mock-inline-ocr-routing-1088";
        const SENTINEL: &str = "__inline_ocr_sentinel_1088__";
        let _guard = register_mock_ocr_backend(BACKEND_NAME, SENTINEL);

        let pdf_path = pdf_test_document("with_images.pdf");
        assert!(pdf_path.exists(), "missing test fixture: {pdf_path:?}");
        let content = std::fs::read(&pdf_path).expect("read fixture");

        let config = crate::core::config::ExtractionConfig {
            ocr: Some(crate::core::config::OcrConfig {
                backend: BACKEND_NAME.to_string(),
                ..Default::default()
            }),
            pdf_options: Some(crate::core::config::pdf::PdfConfig {
                ocr_inline_images: true,
                ..Default::default()
            }),
            ..Default::default()
        };

        let result = PdfExtractor::new()
            .extract_content(&content, "application/pdf", &config)
            .await
            .expect("extraction must not fail");

        assert!(
            !result.images.is_empty(),
            "with_images.pdf must yield at least one embedded image; \
             fixture may need replacing if pdf_oxide no longer extracts from it"
        );

        let images_with_ocr: Vec<_> = result.images.iter().filter(|img| img.ocr_result.is_some()).collect();

        assert!(
            !images_with_ocr.is_empty(),
            "at least one image must have an ocr_result when ocr_inline_images=true"
        );

        for img in &images_with_ocr {
            let content = img.ocr_result.as_ref().unwrap().content.as_str();
            assert!(
                content.contains(SENTINEL),
                "ocr_result content '{content}' does not contain sentinel — \
                 backend routing is still going through hardcoded Tesseract"
            );
        }
    }

    /// Verifies that the extraction-level output_format is forwarded to the backend
    /// via OcrConfig.output_format. This mirrors the standalone image extractor
    /// (image.rs) and allows backends that produce format-aware output (e.g. Markdown
    /// table rendering) to behave correctly for inline PDF images.
    #[tokio::test]
    #[cfg(all(feature = "pdf", feature = "ocr"))]
    #[serial]
    async fn test_ocr_inline_images_forwards_output_format() {
        use std::sync::{Arc, Mutex};
        const BACKEND_NAME: &str = "mock-inline-ocr-format-1088";
        const SENTINEL: &str = "__format_sentinel_1088__";

        let received_config = Arc::new(Mutex::new(None));
        let backend = Arc::new(ConfigCapturingBackend {
            name: BACKEND_NAME,
            sentinel: SENTINEL,
            received_config: Arc::clone(&received_config),
        });
        crate::plugins::register_ocr_backend(backend).unwrap();
        let _guard = RegisteredOcrBackendGuard { name: BACKEND_NAME };

        let pdf_path = pdf_test_document("with_images.pdf");
        assert!(pdf_path.exists(), "missing test fixture: {pdf_path:?}");
        let content = std::fs::read(&pdf_path).expect("read fixture");

        let config = crate::core::config::ExtractionConfig {
            output_format: crate::core::config::OutputFormat::Markdown,
            ocr: Some(crate::core::config::OcrConfig {
                backend: BACKEND_NAME.to_string(),
                ..Default::default()
            }),
            pdf_options: Some(crate::core::config::pdf::PdfConfig {
                ocr_inline_images: true,
                ..Default::default()
            }),
            ..Default::default()
        };

        let result = PdfExtractor::new()
            .extract_content(&content, "application/pdf", &config)
            .await
            .expect("extraction must not fail");

        if result.images.is_empty() {
            panic!("with_images.pdf must yield images; fixture may need replacing");
        }

        let captured = received_config.lock().unwrap();
        let captured_config = captured
            .as_ref()
            .expect("backend was never called — no images were processed");
        assert_eq!(
            captured_config.output_format,
            Some(crate::core::config::OutputFormat::Markdown),
            "output_format was not forwarded to the inline-image OCR backend"
        );
    }

    /// When ocr_inline_images is false the mock backend must NOT be called even
    /// though it is registered as the configured backend.
    #[tokio::test]
    #[cfg(all(feature = "pdf", feature = "ocr"))]
    #[serial]
    async fn test_ocr_inline_images_disabled_does_not_call_backend() {
        const BACKEND_NAME: &str = "mock-inline-ocr-disabled-1088";
        let _guard = register_mock_ocr_backend(BACKEND_NAME, "should-never-appear");

        let pdf_path = pdf_test_document("with_images.pdf");
        assert!(pdf_path.exists(), "missing test fixture: {pdf_path:?}");
        let content = std::fs::read(&pdf_path).expect("read fixture");

        let config = crate::core::config::ExtractionConfig {
            ocr: Some(crate::core::config::OcrConfig {
                backend: BACKEND_NAME.to_string(),
                ..Default::default()
            }),
            pdf_options: Some(crate::core::config::pdf::PdfConfig {
                ocr_inline_images: false,
                extract_images: true,
                ..Default::default()
            }),
            ..Default::default()
        };

        let result = PdfExtractor::new()
            .extract_content(&content, "application/pdf", &config)
            .await
            .expect("extraction must not fail");

        assert!(
            !result.images.is_empty(),
            "with_images.pdf must yield images; fixture may need replacing"
        );
        for img in &result.images {
            assert!(
                img.ocr_result.is_none(),
                "img {} on page {:?} has ocr_result even though ocr_inline_images=false",
                img.image_index,
                img.page_number,
            );
        }
    }

    /// Tests form field extraction from a PDF.
    /// Uses an existing test PDF rather than creating one programmatically.
    /// This is a simple smoke test to verify that form field extraction works
    /// and doesn't panic or crash the extraction pipeline.
    /// Path to the vendored fillable-form fixture (AcroForm with text, button,
    /// and choice fields).
    #[cfg(feature = "pdf")]
    fn form_test_document() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../test_documents/vendored/pdfium-render/form-test.pdf")
    }

    /// End-to-end: a real AcroForm PDF yields populated, correctly-typed
    /// `form_fields` on the extractor's `InternalDocument` carrier.
    #[tokio::test]
    #[cfg(feature = "pdf")]
    async fn test_form_field_extraction_enabled() {
        let content = std::fs::read(form_test_document()).expect("read form-test.pdf fixture");

        let config = crate::core::config::ExtractionConfig {
            pdf_options: Some(crate::core::config::pdf::PdfConfig {
                extract_form_fields: true,
                ..Default::default()
            }),
            ..Default::default()
        };

        let internal_doc = PdfExtractor::new()
            .extract_content(&content, "application/pdf", &config)
            .await
            .expect("extraction must not fail");

        assert!(
            !internal_doc.form_fields.is_empty(),
            "AcroForm PDF must yield form fields, got none"
        );
        assert!(
            internal_doc.form_fields.iter().all(|f| !f.full_name.is_empty()),
            "every extracted field must have a full_name"
        );
        assert!(
            internal_doc
                .form_fields
                .iter()
                .any(|f| f.field_type == crate::types::FormFieldType::Text),
            "fixture has text fields; at least one must map to FormFieldType::Text"
        );
    }

    /// With `extract_form_fields = false`, the same AcroForm PDF yields no fields.
    #[tokio::test]
    #[cfg(feature = "pdf")]
    async fn test_form_field_extraction_disabled() {
        let content = std::fs::read(form_test_document()).expect("read form-test.pdf fixture");

        let config = crate::core::config::ExtractionConfig {
            pdf_options: Some(crate::core::config::pdf::PdfConfig {
                extract_form_fields: false,
                ..Default::default()
            }),
            ..Default::default()
        };

        let internal_doc = PdfExtractor::new()
            .extract_content(&content, "application/pdf", &config)
            .await
            .expect("extraction must not fail");

        assert!(
            internal_doc.form_fields.is_empty(),
            "form fields must be empty when extract_form_fields is disabled"
        );
    }

    /// Tests that form fields are properly extracted and carried through the pipeline
    /// using the InternalDocument.form_fields carrier (not metadata.additional).
    /// This test manually constructs an InternalDocument with form_fields to verify
    /// the carrier pattern works end-to-end, without relying on a complex PDF fixture.
    #[test]
    #[cfg(feature = "pdf")]
    fn test_form_fields_carrier_via_internal_document() {
        let mut doc = InternalDocument::new("pdf");
        doc.mime_type = "application/pdf".to_string();

        doc.form_fields = vec![crate::types::PdfFormField {
            name: "full_name".to_string(),
            full_name: "form.full_name".to_string(),
            field_type: crate::types::FormFieldType::Text,
            value: Some("Ada Lovelace".to_string()),
            default_value: Some("Default Name".to_string()),
            flags: 0,
            page: None,
            bbox: None,
            max_length: None,
            tooltip: None,
        }];

        assert_eq!(doc.form_fields.len(), 1);
        let field = &doc.form_fields[0];
        assert_eq!(field.name, "full_name");
        assert_eq!(field.value, Some("Ada Lovelace".to_string()));
        assert_eq!(field.field_type, crate::types::FormFieldType::Text);

        assert!(
            doc.metadata.additional.get("_pdf_form_fields").is_none(),
            "metadata.additional should not contain _pdf_form_fields (leak check)"
        );

        let result =
            crate::extraction::derive::derive_extraction_result(doc, false, crate::core::config::OutputFormat::Plain);

        assert_eq!(result.form_fields.len(), 1);
        assert_eq!(result.form_fields[0].name, "full_name");
        assert_eq!(result.form_fields[0].value, Some("Ada Lovelace".to_string()));
    }

    /// Tests that with form_fields extraction disabled, the result contains no form fields.
    #[test]
    #[cfg(feature = "pdf")]
    fn test_form_fields_disabled_no_leak() {
        let mut doc = InternalDocument::new("pdf");
        doc.mime_type = "application/pdf".to_string();

        assert!(doc.form_fields.is_empty());

        assert!(doc.metadata.additional.get("_pdf_form_fields").is_none());

        let result =
            crate::extraction::derive::derive_extraction_result(doc, false, crate::core::config::OutputFormat::Plain);

        assert!(result.form_fields.is_empty());
    }
}
