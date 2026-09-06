#[cfg(all(any(feature = "ocr", feature = "ocr-wasm"), feature = "layout-detection"))]
use super::document::assemble_ocr_page_paragraphs;
#[cfg(all(
    any(feature = "ocr", feature = "ocr-pipeline"),
    any(
        feature = "ocr",
        feature = "ocr-wasm",
        feature = "pdf",
        not(feature = "layout-detection")
    )
))]
use super::document::resolved_ocr_layout_dimensions;
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
use super::document::{
    accepted_ocr_page_replacements, apply_ocr_page_replacements, apply_ocr_text_list_fallback,
    fill_unstructured_ocr_pages, heuristically_restructured_ocr_pages,
};
// Read only by the two OCR-paragraph assembly blocks below -- one gated on
// `layout-detection` *with* `ocr`/`ocr-wasm`, the other on `not(layout-detection)`. With
// `layout-detection` and neither OCR frontend (the `formula-recognition,pdf` CI leg) both
// blocks compile out, so importing these under the enclosing function's plain
// `any(ocr, ocr-pipeline)` gate is an unused import. ~keep
#[cfg(all(
    any(feature = "ocr", feature = "ocr-pipeline"),
    any(feature = "ocr", feature = "ocr-wasm", not(feature = "layout-detection"))
))]
use super::document::{apply_ocr_layout_content_filter, ocr_points_per_pixel};
#[cfg(all(any(feature = "ocr", feature = "ocr-pipeline"), feature = "pdf"))]
use super::document::{
    build_mixed_ocr_page_document, build_pipeline_ocr_page_document, formula_bbox_to_page_points,
    ocr_margin_filter_capability_warning, public_ocr_elements_for_pdf_page, rescale_ocr_bboxes_to_page_points,
    should_use_document_processing, split_document_global_ocr_structure_by_page, undo_auto_rotate_point,
};
#[cfg(all(
    any(feature = "ocr", feature = "ocr-pipeline"),
    feature = "pdf",
    feature = "layout-detection"
))]
use super::document::{detection_for_mixed_route_page, single_stage_pipeline_for_layout};
// Same two blocks as `apply_ocr_layout_content_filter` above, but their uses sit inside
// the blocks' nested `#[cfg(feature = "pdf")]` margin-filter scopes. ~keep
#[cfg(all(
    any(feature = "ocr", feature = "ocr-pipeline"),
    feature = "pdf",
    any(feature = "ocr", feature = "ocr-wasm", not(feature = "layout-detection"))
))]
use super::document::{filter_ocr_paragraphs_by_page_margins, ocr_paragraphs_plain_text};
#[cfg(all(feature = "layout-detection", any(feature = "ocr", feature = "ocr-wasm")))]
use super::document::{
    recognized_table_to_public_table, scale_detection_to_dimensions, scale_detection_to_ocr_coordinates,
    transform_ocr_elements_to_render_space,
};
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
use super::rendering::EncodedPage;
#[cfg(all(any(feature = "ocr", feature = "ocr-pipeline"), feature = "pdf"))]
use super::rendering::{
    XObjectRecoveryOutcome, clone_rgb_for_png_encode, fallback_render_document, open_pdf_for_full_ocr,
    open_pdf_for_page_ocr, page_dimensions_pt, page_needs_xobject_fallback, recover_page_text_from_image_xobjects,
    render_full_pdf_ocr_batch, render_selected_pages_from_document, share_rendered_page_images, valid_page_indices,
    validate_png_encode_batch_peak, xobject_fallback_warning,
};
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
use super::scoring::{
    NativeTextStats, OcrPageNoiseVerdict, accept_or_reject_ocr_page, compute_quality_score, mean_text_conf_of,
    page_ocr_confidence, pipeline_stage_score, repair_ocr_list_markers, word_count_of,
};
#[cfg(all(any(feature = "ocr", feature = "ocr-pipeline"), feature = "pdf"))]
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
#[cfg(all(any(feature = "ocr", feature = "ocr-pipeline"), feature = "pdf"))]
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
use std::borrow::Cow;

#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
use crate::core::config::ExtractionConfig;
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
use crate::core::config::OcrQualityThresholds;

/// Build mixed text from native extraction and per-page OCR results.
///
/// For each page boundary, if the page is in `ocr_page_numbers` (1-indexed),
/// use the OCR result; otherwise use the native text slice.
///
/// Page numbers must be >= 1 (invalid values are filtered out with a warning).
/// An `ocr` config is recommended but not required; defaults are used if absent.
#[cfg(all(any(feature = "ocr", feature = "ocr-pipeline"), feature = "pdf"))]
pub(crate) async fn extract_mixed_ocr_native(
    native_text: &str,
    boundaries: &[crate::types::PageBoundary],
    ocr_page_numbers: &[u32],
    content: &[u8],
    config: &ExtractionConfig,
    _path: Option<&std::path::Path>,
) -> crate::Result<(
    String,
    ahash::AHashMap<u32, String>,
    ahash::AHashMap<u32, crate::types::internal::InternalDocument>,
    Vec<crate::types::LlmUsage>,
    Option<Vec<crate::types::ExtractedImage>>,
    Vec<crate::types::Formula>,
    ahash::AHashMap<u32, crate::types::ImagePreprocessingMetadata>,
    ahash::AHashMap<u32, crate::types::page::PageOcrConfidence>,
    Vec<crate::types::ProcessingWarning>,
)> {
    let ocr_set: std::collections::HashSet<u32> = ocr_page_numbers
        .iter()
        .copied()
        .filter(|&p| {
            if p == 0 {
                tracing::warn!("force_ocr_pages contains 0; page numbers are 1-indexed, ignoring");
                false
            } else {
                true
            }
        })
        .collect();

    if ocr_set.is_empty() {
        return Ok((
            native_text.to_string(),
            ahash::AHashMap::new(),
            ahash::AHashMap::new(),
            Vec::new(),
            None,
            Vec::new(),
            ahash::AHashMap::new(),
            ahash::AHashMap::new(),
            Vec::new(),
        ));
    }

    let mut page_indices: Vec<usize> = ocr_set.iter().map(|&p| (p - 1) as usize).collect();
    page_indices.sort_unstable();
    let (render_doc, page_count, page_rotations) = open_pdf_for_page_ocr(content)?;
    page_indices = valid_page_indices(&page_indices, page_count);
    if page_indices.is_empty() {
        return Ok((
            native_text.to_string(),
            ahash::AHashMap::new(),
            ahash::AHashMap::new(),
            Vec::new(),
            None,
            Vec::new(),
            ahash::AHashMap::new(),
            ahash::AHashMap::new(),
            Vec::new(),
        ));
    }

    // Layout detection for this mixed OCR route (#665). The full-document OCR routes
    // (`force_ocr`, the OCR-gate fallback) already run layout via `run_ocr_with_layout` ->
    // `layout_runner::run_layout_for_ocr`, keyed on `config.resolved_layout_config()` (i.e.
    // `config.layout` being set, which `--layout` alone does). This route never called that:
    // it built `structured_ocr_pages` straight from raw backend OCR output, so `--layout`
    // alone produced byte-identical text with zero layout log lines even though layout
    // detection is what should be classifying headings/lists/tables here. Runs the exact
    // same whole-document pass (`RenderWithoutInference`: every page renders, gated pages
    // skip inference, CPU-retry-on-accelerated-failure) the full-document routes use; only
    // the pages this call actually OCRs read from the result below. `page_idx` throughout
    // this function is the same document-wide 0-based index `run_layout_for_ocr`'s per-page
    // `Vec` is indexed by, so `detections.get(page_idx)` needs no extra alignment step.
    #[cfg(feature = "layout-detection")]
    let (layout_detections_for_mixed, layout_pass_warning, layout_pass_glyph_drop_warnings): (
        Option<Vec<crate::layout::DetectionResult>>,
        Option<crate::types::ProcessingWarning>,
        Vec<crate::types::ProcessingWarning>,
    ) = if let Some(layout_config) = config.resolved_layout_config() {
        let layout_thread_budget = crate::core::config::concurrency::resolve_thread_budget(config.concurrency.as_ref());
        let default_security_limits = crate::extractors::security::SecurityLimits::default();
        let security_limits = config.security_limits.as_ref().unwrap_or(&default_security_limits);
        match super::super::layout_runner::run_layout_for_ocr(
            content,
            layout_config.as_ref(),
            layout_thread_budget,
            security_limits,
        )
        .await
        {
            Ok((
                super::super::layout_runner::LayoutAttempt {
                    output:
                        super::super::layout_runner::LayoutRunOutput {
                            data: Some((_, _, _, detections)),
                            ..
                        },
                    warning,
                    ..
                },
                glyph_drop_warnings,
            )) => (Some(detections), warning, glyph_drop_warnings),
            Ok((
                super::super::layout_runner::LayoutAttempt {
                    output: super::super::layout_runner::LayoutRunOutput { data: None, .. },
                    warning,
                    ..
                },
                glyph_drop_warnings,
            )) => {
                tracing::info!(
                    "OCR layout (mixed route): auto gate skipped every page, continuing without layout assembly"
                );
                (None, warning, glyph_drop_warnings)
            }
            Err(error) => {
                tracing::warn!(
                    error = %error,
                    "OCR layout detection failed for mixed OCR route; continuing without layout assembly"
                );
                (
                    None,
                    Some(super::super::layout_runner::layout_failure_warning(&error)),
                    Vec::new(),
                )
            }
        }
    } else {
        (None, None, Vec::new())
    };
    #[cfg(feature = "layout-detection")]
    let mixed_route_layout_active = layout_detections_for_mixed.is_some();
    #[cfg(not(feature = "layout-detection"))]
    let mixed_route_layout_active = false;

    use image::ImageEncoder;
    use image::codecs::png::PngEncoder;
    // rayon's work-stealing pool needs OS threads; wasm32 has none, so the parallel encode
    // paths below fall back to sequential `.iter()` there. Gate the import to match. ~keep
    #[cfg(all(feature = "tokio-runtime", not(target_arch = "wasm32")))]
    use rayon::prelude::*;
    use std::io::Cursor;
    use std::sync::Arc;

    let default_ocr_config = crate::core::config::OcrConfig::default();
    let mut ocr_config_resolved = config.ocr.as_ref().unwrap_or(&default_ocr_config).clone();
    if ocr_config_resolved.acceleration.is_none() {
        ocr_config_resolved.acceleration = config.acceleration.clone();
    }
    // GH#1554: mirrors `acceleration` above so this mixed native/OCR route inherits the
    // caller's configured decode limits instead of always falling back to
    // `SecurityLimits::default()`. ~keep
    if ocr_config_resolved.security_limits.is_none() {
        ocr_config_resolved.security_limits = config.security_limits.clone();
    }

    let batch_size = crate::core::config::concurrency::resolve_thread_budget(config.concurrency.as_ref());

    let capture_rasters = config.images.as_ref().is_some_and(|c| c.include_page_rasters);
    let ocr_config_owned = ensure_elements_enabled(&ocr_config_resolved);
    // When a `vlm_fallback` policy or an explicit multi-stage `pipeline` is configured,
    // each page must run through the shared pipeline runner so fallback backends (e.g.
    // the VLM) apply on this mixed/per-page OCR route too. Previously only the single
    // configured backend ran here, silently ignoring `vlm_fallback` on the
    // `scanned_pages` / `force_ocr_pages` / per-page-fallback routes (#1341). The
    // default (no fallback, no explicit pipeline) keeps the fast single-backend path.
    //
    // Layout detections (#665, `mixed_route_layout_active`) are threaded the same way: the
    // pipeline route is the only one that hands `layout_detections` down to
    // `extract_with_ocr_for_page`'s classification. Wrapping the single configured backend
    // in a one-stage pipeline (`single_stage_pipeline_for_layout`) reuses that exact,
    // already-tested code path instead of duplicating pixel-space layout assembly here. This
    // only fires when a real detection is available for this call, so `--layout` producing
    // nothing (gate skipped every page, or no `config.layout`) leaves the fast path untouched.
    let effective_pipeline = if ocr_config_owned.vlm_fallback != crate::core::config::VlmFallbackPolicy::Disabled
        || ocr_config_owned.pipeline.is_some()
    {
        ocr_config_owned.effective_pipeline()
    } else if mixed_route_layout_active {
        #[cfg(feature = "layout-detection")]
        {
            Some(single_stage_pipeline_for_layout(&ocr_config_owned))
        }
        #[cfg(not(feature = "layout-detection"))]
        {
            None
        }
    } else {
        None
    };

    // The top-level `backend` registry lookup is only needed by the single-backend
    // route below; the pipeline route resolves each of its own stage backends
    // internally via `run_ocr_pipeline`. Resolving it eagerly meant a
    // `vlm_fallback = Always` config (or an explicit `pipeline`) that never touches
    // this top-level backend still failed if it happened to be unregistered
    // (review follow-up to #1341).
    let backend = if effective_pipeline.is_none() {
        let registry = crate::plugins::registry::get_ocr_backend_registry();
        let registry = registry.read();
        Some(registry.get(&ocr_config_owned.backend)?)
    } else {
        None
    };

    let total = page_indices.len();
    let mut ocr_results: ahash::AHashMap<u32, String> = ahash::AHashMap::with_capacity(total);
    // Tesseract's own mean confidence for each page it read, 0-100. Captured here because
    // the per-page metadata is gone by the time `ocr_results` is judged.
    let mut page_mean_confidence: ahash::AHashMap<u32, f64> = ahash::AHashMap::new();
    let mut page_dictionary_invalid_word_ratio: ahash::AHashMap<u32, f64> = ahash::AHashMap::new();
    // Collected next to `page_mean_confidence` for the same reason: the per-page metadata is
    // gone by the time the summary attached to `PageContent.ocr_confidence` is built (#1568). ~keep
    let mut page_word_count: ahash::AHashMap<u32, u32> = ahash::AHashMap::new();
    let mut ocr_confidence_by_page: ahash::AHashMap<u32, crate::types::page::PageOcrConfidence> =
        ahash::AHashMap::new();
    let mut structured_ocr_pages: ahash::AHashMap<u32, crate::types::internal::InternalDocument> =
        ahash::AHashMap::with_capacity(total);
    // Bare, unclassified per-page paragraphs for every OCR'd page, real 1-indexed page
    // number -> that page's paragraphs. Collected across both the pipeline and
    // single-backend routes below so the document-global heading/list heuristic
    // (`heuristically_restructured_ocr_pages`) can run exactly once, after this
    // function's per-page loop ends, over every OCR'd page at once -- the same
    // font-clustering pass `extract_with_ocr_for_page` runs for the OCR-only route,
    // previously unreachable here (#665-adjacent gap; see that function's own doc
    // comment on `skip_document_global_heuristic`).
    let mut ocr_page_paragraphs: ahash::AHashMap<u32, Vec<crate::pdf::structure::types::PdfParagraph>> =
        ahash::AHashMap::with_capacity(total);
    let mut accumulated_llm_usage: Vec<crate::types::LlmUsage> = Vec::new();
    let mut accumulated_formulas: Vec<crate::types::Formula> = Vec::new();
    let mut accumulated_warnings: Vec<crate::types::ProcessingWarning> = Vec::new();
    #[cfg(feature = "layout-detection")]
    {
        if let Some(warning) = layout_pass_warning {
            crate::core::diagnostics::push_warning_deduped(&mut accumulated_warnings, warning);
        }
        for warning in layout_pass_glyph_drop_warnings {
            crate::core::diagnostics::push_warning_deduped(&mut accumulated_warnings, warning);
        }
    }
    let mut captured_rasters: Vec<crate::types::ExtractedImage> = Vec::new();
    let mut preprocessing_by_page: ahash::AHashMap<u32, crate::types::ImagePreprocessingMetadata> =
        ahash::AHashMap::new();
    for batch_start in (0..total).step_by(batch_size) {
        let batch_end = (batch_start + batch_size).min(total);
        let default_security_limits = crate::extractors::security::SecurityLimits::default();
        let security_limits = config.security_limits.as_ref().unwrap_or(&default_security_limits);
        let page_images = render_selected_pages_from_document(
            &render_doc,
            &page_rotations,
            &page_indices[batch_start..batch_end],
            security_limits,
        )?;

        // Multi-stage pipeline route (#1341): drive each page through `run_ocr_pipeline`
        // so `vlm_fallback` / explicit-pipeline stages apply here, mirroring the image
        // extractor's per-image pipeline path. Bounded to this batch's page count (at
        // most `batch_size`, the resolved worker budget) via a `JoinSet`, mirroring the
        // concurrency shape of the single-backend path below.
        if let Some(ref pipeline) = effective_pipeline {
            let page_images = share_rendered_page_images(page_images);
            // on wasm32 (no OS threads, and extractor/backend futures are `!Send` there —
            // see the matching gate on the single-backend path below). Falls back to the
            // sequential loop there even though `tokio-runtime` may be active.
            #[cfg(all(feature = "tokio-runtime", not(target_arch = "wasm32")))]
            {
                let mut join_set = tokio::task::JoinSet::new();
                for (page_idx, image) in &page_images {
                    let image_arc = Arc::clone(image);
                    let pipeline_clone = pipeline.clone();
                    let config_clone = config.clone();
                    let idx = *page_idx;
                    // This page's own known `/Rotate` value, already resolved by
                    // `open_pdf_for_page_ocr` above -- the sibling single-backend route
                    // below reads the same `page_rotations` slice the same way. Passed
                    // explicitly rather than via `content` because the pipeline call below
                    // hands `run_ocr_pipeline` a single detached image (not the full page
                    // list `extract_with_ocr`'s own content-based auto-detection expects
                    // index-aligned to page 0), so an index-based content lookup there would
                    // resolve the wrong page's rotation (or none at all) for anything but the
                    // document's first OCR'd page (#651).
                    let page_rotation_degrees = page_rotations.get(*page_idx).copied().unwrap_or(0);
                    // See `extract_with_ocr_for_page`'s doc comment on
                    // `points_per_pixel_override`: this call hands the stage a single
                    // detached image with `content: None`, so its own pixel -> point lookup
                    // cannot resolve this page's real MediaBox height and would silently fall
                    // back to `1.0`. Computed from the same `render_doc` / raster height the
                    // sibling single-backend route below rescales bboxes with.
                    let points_per_pixel_override = {
                        let (_, page_height_pt) = page_dimensions_pt(&render_doc, *page_idx);
                        let image_height_px = image_arc.height();
                        (image_height_px > 0).then(|| page_height_pt / image_height_px as f32)
                    };
                    // This page's own pixel-space detection from the whole-document layout
                    // pass above (#665), cloned out here because the spawned task below must
                    // own everything it captures. `extract_with_ocr_for_page` (reached through
                    // `run_ocr_pipeline_for_page`) rescales it to match this exact raster via
                    // `scale_detection_to_dimensions`/`scale_detection_to_ocr_coordinates`, so
                    // a DPI mismatch between the layout pass's own render and this page's OCR
                    // render is handled there, not here.
                    #[cfg(feature = "layout-detection")]
                    let page_detection: Option<crate::layout::DetectionResult> =
                        detection_for_mixed_route_page(layout_detections_for_mixed.as_deref(), *page_idx).cloned();
                    join_set.spawn(async move {
                        #[cfg(feature = "layout-detection")]
                        let page_detection_slice = page_detection.as_ref().map(std::slice::from_ref);
                        let result = Box::pin(run_ocr_pipeline_for_page(
                            None,
                            Some(std::slice::from_ref(image_arc.as_ref())),
                            #[cfg(feature = "layout-detection")]
                            page_detection_slice,
                            &config_clone,
                            &pipeline_clone,
                            None,
                            page_rotation_degrees,
                            true,
                            points_per_pixel_override,
                            idx,
                        ))
                        .await;
                        (idx, result)
                    });
                }
                while let Some(join_result) = join_set.join_next().await {
                    let (page_idx, result) = join_result.map_err(|e| crate::XbergError::Plugin {
                        message: format!("OCR pipeline task panicked: {}", e),
                        plugin_name: "ocr".to_string(),
                    })?;
                    let (
                        text,
                        tables,
                        elements,
                        doc,
                        usage,
                        page_texts,
                        _rasters,
                        formulas,
                        mut page_raw_paragraphs,
                        preprocessing,
                        page_ocr_confidences,
                    ) = result?;
                    accumulated_llm_usage.extend(usage);
                    ocr_confidence_by_page.extend(page_ocr_confidences);
                    let page_number = (page_idx + 1) as u32;
                    if let Some(metadata) = preprocessing.into_values().next() {
                        preprocessing_by_page.insert(page_number, metadata);
                    }
                    // `run_ocr_pipeline_for_page` drove exactly one detached image
                    // (`std::slice::from_ref` above), so its own per-page paragraph vec has
                    // at most one entry -- this page's own, still bare/unclassified because
                    // `skip_document_global_heuristic = true` was passed above.
                    if let Some(paragraphs) = page_raw_paragraphs.pop().filter(|p| !p.is_empty()) {
                        ocr_page_paragraphs.insert(page_number, paragraphs);
                    }
                    let page_dims = page_images
                        .iter()
                        .find(|(i, _)| *i == page_idx)
                        .map(|(_, img)| (img.width(), img.height()));
                    for mut formula in formulas {
                        formula.page = Some(page_number);
                        if let Some((w, h)) = page_dims {
                            formula_bbox_to_page_points(&mut formula, &render_doc, page_idx, None, w, h);
                        }
                        accumulated_formulas.push(formula);
                    }
                    // `run_ocr_pipeline`/`extract_with_ocr` assemble `text` as if this
                    // lone image were page 0 of the document, so a configured page marker
                    // is stamped "page 1" regardless of the real page number. The raw
                    // `page_texts` entry has no marker injected at that layer, so prefer
                    // fall back to `text` only if the backend returned no page_texts.
                    let page_text = page_texts.into_iter().next().unwrap_or(text);
                    // The pipeline's tables and OCR elements used to be dropped here (#60);
                    // they now ride along on the page's structured document. ~keep
                    // This route also skipped the pixel -> point bbox conversion entirely,
                    // so its bboxes stayed in raster pixels after #1423 fixed the
                    // single-backend route; `build_pipeline_ocr_page_document` applies it.
                    let raster_size_px = page_images
                        .iter()
                        .find(|(rendered_page, _)| *rendered_page == page_idx)
                        .map_or((0, 0), |(_, image)| (image.width(), image.height()));
                    if let Some(mut d) = build_pipeline_ocr_page_document(
                        doc,
                        tables,
                        elements,
                        &page_text,
                        page_number,
                        raster_size_px,
                        page_dimensions_pt(&render_doc, page_idx),
                    ) {
                        crate::core::diagnostics::dedup_extend_warnings(
                            &mut accumulated_warnings,
                            std::mem::take(&mut d.processing_warnings),
                        );
                        structured_ocr_pages.insert(page_number, d);
                    }
                    ocr_results.insert(page_number, page_text);
                }
            }
            #[cfg(any(not(feature = "tokio-runtime"), target_arch = "wasm32"))]
            {
                for (page_idx, image) in &page_images {
                    // See the matching comments on the sibling `JoinSet` branch above (#651,
                    // and `points_per_pixel_override` re. `extract_with_ocr_for_page`'s doc
                    // comment).
                    let page_rotation_degrees = page_rotations.get(*page_idx).copied().unwrap_or(0);
                    let points_per_pixel_override = {
                        let (_, page_height_pt) = page_dimensions_pt(&render_doc, *page_idx);
                        let image_height_px = image.height();
                        (image_height_px > 0).then(|| page_height_pt / image_height_px as f32)
                    };
                    // See the matching comment on the sibling `JoinSet` branch above (#665).
                    #[cfg(feature = "layout-detection")]
                    let page_detection: Option<&crate::layout::DetectionResult> =
                        detection_for_mixed_route_page(layout_detections_for_mixed.as_deref(), *page_idx);
                    let (
                        text,
                        tables,
                        elements,
                        doc,
                        usage,
                        page_texts,
                        _rasters,
                        formulas,
                        mut page_raw_paragraphs,
                        preprocessing,
                        page_ocr_confidences,
                    ) = Box::pin(run_ocr_pipeline_for_page(
                        None,
                        Some(std::slice::from_ref(image.as_ref())),
                        #[cfg(feature = "layout-detection")]
                        page_detection.map(std::slice::from_ref),
                        config,
                        pipeline,
                        None,
                        page_rotation_degrees,
                        true,
                        points_per_pixel_override,
                        *page_idx,
                    ))
                    .await?;
                    accumulated_llm_usage.extend(usage);
                    ocr_confidence_by_page.extend(page_ocr_confidences);
                    let page_number = (*page_idx + 1) as u32;
                    if let Some(metadata) = preprocessing.into_values().next() {
                        preprocessing_by_page.insert(page_number, metadata);
                    }
                    if let Some(paragraphs) = page_raw_paragraphs.pop().filter(|p| !p.is_empty()) {
                        ocr_page_paragraphs.insert(page_number, paragraphs);
                    }
                    for mut formula in formulas {
                        formula.page = Some(page_number);
                        formula_bbox_to_page_points(
                            &mut formula,
                            &render_doc,
                            *page_idx,
                            None,
                            image.width(),
                            image.height(),
                        );
                        accumulated_formulas.push(formula);
                    }
                    let page_text = page_texts.into_iter().next().unwrap_or(text);
                    if let Some(mut d) = build_pipeline_ocr_page_document(
                        doc,
                        tables,
                        elements,
                        &page_text,
                        page_number,
                        (image.width(), image.height()),
                        page_dimensions_pt(&render_doc, *page_idx),
                    ) {
                        crate::core::diagnostics::dedup_extend_warnings(
                            &mut accumulated_warnings,
                            std::mem::take(&mut d.processing_warnings),
                        );
                        structured_ocr_pages.insert(page_number, d);
                    }
                    ocr_results.insert(page_number, page_text);
                }
            }
            if capture_rasters {
                let default_security_limits = crate::extractors::security::SecurityLimits::default();
                let security_limits = config.security_limits.as_ref().unwrap_or(&default_security_limits);
                validate_png_encode_batch_peak(
                    page_images.iter().map(|(_, image)| image.as_ref()),
                    false,
                    security_limits,
                )?;
                for (page_idx, image) in &page_images {
                    let rgb = clone_rgb_for_png_encode(image, security_limits)?;
                    let (w, h) = rgb.dimensions();
                    let mut buf = Cursor::new(Vec::new());
                    PngEncoder::new(&mut buf)
                        .write_image(&rgb, w, h, image::ColorType::Rgb8.into())
                        .map_err(|e| crate::XbergError::Parsing {
                            message: format!("Failed to encode page {} raster: {}", page_idx + 1, e),
                            source: None,
                        })?;
                    captured_rasters.push(build_page_raster_image(
                        *page_idx,
                        bytes::Bytes::from(buf.into_inner()),
                        w,
                        h,
                    ));
                }
            }
            continue;
        }

        // Reached only when `effective_pipeline` is `None`, so `backend` was resolved above.
        let backend = backend
            .as_ref()
            .expect("backend is resolved above whenever effective_pipeline is None");
        let orientation_handling = backend.page_orientation_handling();
        let batch_slice = &page_images;
        let default_security_limits = crate::extractors::security::SecurityLimits::default();
        let security_limits = config.security_limits.as_ref().unwrap_or(&default_security_limits);
        #[cfg(all(feature = "tokio-runtime", not(target_arch = "wasm32")))]
        validate_png_encode_batch_peak(batch_slice.iter().map(|(_, image)| image), true, security_limits)?;
        #[cfg(any(not(feature = "tokio-runtime"), target_arch = "wasm32"))]
        validate_png_encode_batch_peak(batch_slice.iter().map(|(_, image)| image), false, security_limits)?;

        #[cfg(all(feature = "tokio-runtime", not(target_arch = "wasm32")))]
        let encoded: crate::Result<Vec<EncodedPage>> = batch_slice
            .par_iter()
            .map(|(page_idx, image)| {
                let default_security_limits = crate::extractors::security::SecurityLimits::default();
                let security_limits = config.security_limits.as_ref().unwrap_or(&default_security_limits);
                let rgb = clone_rgb_for_png_encode(image, security_limits)?;
                let (w, h) = rgb.dimensions();
                let mut buf = Cursor::new(Vec::new());
                PngEncoder::new(&mut buf)
                    .write_image(&rgb, w, h, image::ColorType::Rgb8.into())
                    .map_err(|e| crate::XbergError::Parsing {
                        message: format!("Failed to encode page {} for OCR: {}", page_idx + 1, e),
                        source: None,
                    })?;
                Ok((*page_idx, Arc::new(buf.into_inner()), w, h))
            })
            .collect();
        #[cfg(any(not(feature = "tokio-runtime"), target_arch = "wasm32"))]
        let encoded: crate::Result<Vec<EncodedPage>> = batch_slice
            .iter()
            .map(|(page_idx, image)| {
                let default_security_limits = crate::extractors::security::SecurityLimits::default();
                let security_limits = config.security_limits.as_ref().unwrap_or(&default_security_limits);
                let rgb = clone_rgb_for_png_encode(image, security_limits)?;
                let (w, h) = rgb.dimensions();
                let mut buf = Cursor::new(Vec::new());
                PngEncoder::new(&mut buf)
                    .write_image(&rgb, w, h, image::ColorType::Rgb8.into())
                    .map_err(|e| crate::XbergError::Parsing {
                        message: format!("Failed to encode page {} for OCR: {}", page_idx + 1, e),
                        source: None,
                    })?;
                Ok((*page_idx, Arc::new(buf.into_inner()), w, h))
            })
            .collect();
        let encoded = encoded?;
        drop(page_images);

        // `tokio::task::JoinSet::spawn` requires `Send` futures, but extractor/backend futures
        // are `!Send` on wasm32 (async_trait(?Send), see plugins/extractor/trait.rs) — and
        // wasm32 has no OS threads to run them on regardless. Fall back to the sequential path
        // there even though `tokio-runtime` is active (it's pulled in by
        // `chunking-tokenizers`/`static-embeddings`, not concurrency support). ~keep
        #[cfg(all(feature = "tokio-runtime", not(target_arch = "wasm32")))]
        {
            let mut join_set = tokio::task::JoinSet::new();
            for (page_idx, data, width, height) in &encoded {
                let backend_clone = Arc::clone(backend);
                let page_rotation_degrees = page_rotations.get(*page_idx).copied().unwrap_or(0);
                // Derived from the MediaBox-oriented raster, before `upright_raster_for_backend`
                // may swap its axes.
                let source_dpi = rendered_page_source_dpi(&render_doc, *page_idx, *width);
                let config_clone =
                    ocr_config_with_page_rotation_hint(&ocr_config_owned, page_rotation_degrees, source_dpi)
                        .into_owned();
                let (upright_data, upright_width, upright_height, correction_degrees) = upright_raster_for_backend(
                    data,
                    *width,
                    *height,
                    page_rotation_degrees,
                    orientation_handling,
                    config.security_limits.as_ref(),
                )?;
                let idx = *page_idx;
                join_set.spawn(async move {
                    let result = backend_clone.process_image_owned(upright_data, &config_clone).await;
                    (idx, correction_degrees, upright_width, upright_height, result)
                });
            }
            while let Some(join_result) = join_set.join_next().await {
                let (page_idx, correction_degrees, upright_width, upright_height, result) =
                    join_result.map_err(|e| crate::XbergError::Plugin {
                        message: format!("OCR task panicked: {}", e),
                        plugin_name: "ocr".to_string(),
                    })?;
                let mut extraction_result = result?;
                if let Some(metadata) = extraction_result.metadata.image_preprocessing.clone() {
                    preprocessing_by_page.insert((page_idx + 1) as u32, metadata);
                }
                undo_upright_raster_correction(
                    &mut extraction_result,
                    correction_degrees,
                    upright_width,
                    upright_height,
                );
                if let Some(usage) = extraction_result.llm_usage.take() {
                    accumulated_llm_usage.extend(usage);
                }
                let page_dims = encoded
                    .iter()
                    .find(|(encoded_page, ..)| *encoded_page == page_idx)
                    .map(|(_, _, w, h)| (*w, *h));
                for mut formula in std::mem::take(&mut extraction_result.formulas) {
                    formula.page = Some((page_idx + 1) as u32);
                    if let Some((w, h)) = page_dims {
                        formula_bbox_to_page_points(
                            &mut formula,
                            &render_doc,
                            page_idx,
                            Some(&extraction_result.metadata),
                            w,
                            h,
                        );
                    }
                    accumulated_formulas.push(formula);
                }
                // The backend's own warnings used to be dropped on this route (#60).
                crate::core::diagnostics::dedup_extend_warnings(
                    &mut accumulated_warnings,
                    std::mem::take(&mut extraction_result.processing_warnings),
                );
                let (width, height) = encoded
                    .iter()
                    .find(|(encoded_page, ..)| *encoded_page == page_idx)
                    .map_or((0, 0), |(_, _, w, h)| (*w, *h));
                let (page_width_pt, page_height_pt) = page_dimensions_pt(&render_doc, page_idx);
                if let Some((mut page_doc, paragraphs)) = build_mixed_ocr_page_document(
                    &mut extraction_result,
                    &ocr_config_resolved,
                    (page_idx + 1) as u32,
                    width,
                    height,
                    page_width_pt,
                    page_height_pt,
                    crate::pdf::native::text::PageMarginFractions::from_extraction_config(Some(config)),
                ) {
                    crate::core::diagnostics::dedup_extend_warnings(
                        &mut accumulated_warnings,
                        std::mem::take(&mut page_doc.processing_warnings),
                    );
                    if !paragraphs.is_empty() {
                        ocr_page_paragraphs.insert((page_idx + 1) as u32, paragraphs);
                    }
                    structured_ocr_pages.insert((page_idx + 1) as u32, page_doc);
                }
                if let Some(conf) = mean_text_conf_of(&extraction_result.metadata.additional) {
                    page_mean_confidence.insert((page_idx + 1) as u32, conf);
                }
                if let Some(words) = word_count_of(&extraction_result.metadata.additional) {
                    page_word_count.insert((page_idx + 1) as u32, words);
                }
                if let Some(ratio) = extraction_result
                    .metadata
                    .additional
                    .get(crate::ocr_metadata_keys::OCR_TESSERACT_DICT_INVALID_WORD_RATIO_METADATA_KEY)
                    .and_then(serde_json::Value::as_f64)
                {
                    page_dictionary_invalid_word_ratio.insert((page_idx + 1) as u32, ratio);
                }
                ocr_results.insert((page_idx + 1) as u32, extraction_result.content);
            }
        }
        #[cfg(any(not(feature = "tokio-runtime"), target_arch = "wasm32"))]
        {
            for (page_idx, data, width, height) in &encoded {
                let page_rotation_degrees = page_rotations.get(*page_idx).copied().unwrap_or(0);
                let source_dpi = rendered_page_source_dpi(&render_doc, *page_idx, *width);
                let config_for_page =
                    ocr_config_with_page_rotation_hint(&ocr_config_owned, page_rotation_degrees, source_dpi);
                let (upright_data, upright_width, upright_height, correction_degrees) = upright_raster_for_backend(
                    data,
                    *width,
                    *height,
                    page_rotation_degrees,
                    orientation_handling,
                    config.security_limits.as_ref(),
                )?;
                let mut extraction_result = backend
                    .process_image(upright_data.as_slice(), config_for_page.as_ref())
                    .await?;
                if let Some(metadata) = extraction_result.metadata.image_preprocessing.clone() {
                    preprocessing_by_page.insert((*page_idx + 1) as u32, metadata);
                }
                undo_upright_raster_correction(
                    &mut extraction_result,
                    correction_degrees,
                    upright_width,
                    upright_height,
                );
                if let Some(usage) = extraction_result.llm_usage.take() {
                    accumulated_llm_usage.extend(usage);
                }
                for mut formula in std::mem::take(&mut extraction_result.formulas) {
                    formula.page = Some((*page_idx + 1) as u32);
                    formula_bbox_to_page_points(
                        &mut formula,
                        &render_doc,
                        *page_idx,
                        Some(&extraction_result.metadata),
                        *width,
                        *height,
                    );
                    accumulated_formulas.push(formula);
                }
                crate::core::diagnostics::dedup_extend_warnings(
                    &mut accumulated_warnings,
                    std::mem::take(&mut extraction_result.processing_warnings),
                );
                let (page_width_pt, page_height_pt) = page_dimensions_pt(&render_doc, *page_idx);
                if let Some((mut page_doc, paragraphs)) = build_mixed_ocr_page_document(
                    &mut extraction_result,
                    &ocr_config_resolved,
                    (*page_idx + 1) as u32,
                    *width,
                    *height,
                    page_width_pt,
                    page_height_pt,
                    crate::pdf::native::text::PageMarginFractions::from_extraction_config(Some(config)),
                ) {
                    crate::core::diagnostics::dedup_extend_warnings(
                        &mut accumulated_warnings,
                        std::mem::take(&mut page_doc.processing_warnings),
                    );
                    if !paragraphs.is_empty() {
                        ocr_page_paragraphs.insert((*page_idx + 1) as u32, paragraphs);
                    }
                    structured_ocr_pages.insert((*page_idx + 1) as u32, page_doc);
                }
                if let Some(conf) = mean_text_conf_of(&extraction_result.metadata.additional) {
                    page_mean_confidence.insert((*page_idx + 1) as u32, conf);
                }
                if let Some(words) = word_count_of(&extraction_result.metadata.additional) {
                    page_word_count.insert((*page_idx + 1) as u32, words);
                }
                if let Some(ratio) = extraction_result
                    .metadata
                    .additional
                    .get(crate::ocr_metadata_keys::OCR_TESSERACT_DICT_INVALID_WORD_RATIO_METADATA_KEY)
                    .and_then(serde_json::Value::as_f64)
                {
                    page_dictionary_invalid_word_ratio.insert((*page_idx + 1) as u32, ratio);
                }
                ocr_results.insert((*page_idx + 1) as u32, extraction_result.content);
            }
        }

        if capture_rasters {
            for (page_idx, png_arc, w, h) in &encoded {
                let png_bytes = bytes::Bytes::copy_from_slice(png_arc.as_ref());
                captured_rasters.push(build_page_raster_image(*page_idx, png_bytes, *w, *h));
            }
        }
    }

    // Pipeline stages already assess their output in `extract_with_ocr_for_page` using the
    // actual producing backend. Assess only the direct single-backend route here to avoid
    // duplicate warnings and guessed pipeline confidence semantics. ~keep
    let ocr_output_thresholds = config
        .ocr
        .as_ref()
        .and_then(|ocr| ocr.quality_thresholds.clone())
        .unwrap_or_default();
    if let Some(producing_backend) = backend.as_ref() {
        let confidence_semantics = producing_backend.confidence_semantics();
        let producing_backend_name = producing_backend.name().to_string();
        for (page_number, text) in &mut ocr_results {
            let confidence = page_mean_confidence.get(page_number).copied();
            let dictionary_ratio = page_dictionary_invalid_word_ratio.get(page_number).copied();
            tracing::debug!(page = *page_number, ?confidence, "OCR page mean confidence");
            // Recorded before the accept/reject call below, and never retracted afterwards: a
            // page the noise gate discards was still OCR'd, and its confidence is exactly the
            // evidence a caller needs to understand why it was discarded (#1568). ~keep
            if let Some(summary) = page_ocr_confidence(
                confidence_semantics,
                confidence,
                page_word_count.get(page_number).copied().unwrap_or(0),
                &producing_backend_name,
            ) {
                ocr_confidence_by_page.insert(*page_number, summary);
            }
            let acceptance = accept_or_reject_ocr_page(
                (*page_number as usize).saturating_sub(1),
                std::mem::take(text),
                &ocr_output_thresholds,
                &mut accumulated_warnings,
                dictionary_ratio,
                confidence_semantics,
                confidence,
            );
            *text = acceptance.content;
        }
    }

    for text in ocr_results.values_mut() {
        if let std::borrow::Cow::Owned(repaired) = repair_ocr_list_markers(text) {
            *text = repaired;
        }
    }

    let accepted_replacements =
        accepted_ocr_page_replacements(native_text, boundaries, &ocr_results, &ocr_output_thresholds);
    structured_ocr_pages.retain(|page, _| accepted_replacements.contains_key(page));
    retain_ocr_formulas_for_accepted_pages(&mut accumulated_formulas, &accepted_replacements);

    // Document-global heading/list heuristic (same font-clustering pass the OCR-only
    // route runs via `extract_with_ocr_for_page`, previously unreachable from this mixed
    // route -- see `ocr_page_paragraphs`'s own comment above). Runs once, over every
    // accepted OCR'd page's paragraphs at once: the heuristic clusters font sizes
    // document-wide and needs several pages in hand, which is exactly what the per-page
    // callers above could not give it on their own.
    if !structured_ocr_pages.is_empty() {
        let mut pages_for_heuristic: Vec<Vec<crate::pdf::structure::types::PdfParagraph>> =
            vec![Vec::new(); page_count];
        for (&page_number, paragraphs) in &ocr_page_paragraphs {
            // `structured_ocr_pages` (not `ocr_page_paragraphs`) is the source of truth for
            // which pages are still in play: the destructive noise-filter opt-in above may
            // have dropped a page after its paragraphs were already collected.
            if structured_ocr_pages.contains_key(&page_number)
                && let Some(slot) = pages_for_heuristic.get_mut((page_number - 1) as usize)
            {
                *slot = paragraphs.clone();
            }
        }
        let tables_for_heuristic: Vec<crate::types::Table> = structured_ocr_pages
            .values()
            .flat_map(|doc| doc.tables.iter().cloned())
            .collect();
        let mut ocr_page_numbers: Vec<u32> = structured_ocr_pages.keys().copied().collect();
        ocr_page_numbers.sort_unstable();
        let page_heights_for_heuristic = (0..page_count)
            .map(|page_index| page_dimensions_pt(&render_doc, page_index).1)
            .collect::<Vec<_>>();
        let mut split_pages = heuristically_restructured_ocr_pages(
            &pages_for_heuristic,
            &page_heights_for_heuristic,
            &tables_for_heuristic,
            config,
        )
        .map(|combined_doc| split_document_global_ocr_structure_by_page(combined_doc, &ocr_page_numbers))
        .unwrap_or_default();
        for page_number in &ocr_page_numbers {
            let Some(existing) = structured_ocr_pages.get(page_number) else {
                continue;
            };
            let new_page_doc = match split_pages.remove(page_number) {
                // The heuristic's combined document has no notion of the backend's raw
                // per-word OCR elements or this page's earlier warnings -- both come
                // from the fallback per-page document already built above; only the
                // *structural* elements (headings/paragraphs/list items/tables) come
                // from the document-global pass.
                Some(mut new_page_doc) => {
                    new_page_doc.prebuilt_ocr_elements = existing.prebuilt_ocr_elements.clone();
                    new_page_doc.processing_warnings = existing.processing_warnings.clone();
                    new_page_doc
                }
                // The heuristic either didn't run at all for this document (Plain output, or
                // some other page was already ML-layout-classified -- `already_structured` is
                // document-wide) or dropped this specific page. Either way this page's
                // paragraphs never got ANY list-item classification (#713): apply a
                // text-marker-only fallback pass, which never touches `heading_level`, so it
                // cannot regress heading detection the way pre-classifying before the
                // heuristic call would.
                None => {
                    let mut paragraphs = pages_for_heuristic
                        .get((*page_number - 1) as usize)
                        .cloned()
                        .unwrap_or_default();
                    apply_ocr_text_list_fallback(&mut paragraphs);
                    // #729: recover a bare marker no ML hint ever classified, e.g. a
                    // marker paragraph whose body landed several paragraphs away --
                    // see `adapters::reattach_detached_ocr_list_markers`'s doc comment.
                    // #760: threaded with this page's known PDF `/Rotate` so the
                    // reattachment's baseline/indent comparison runs in the rotation-
                    // corrected frame instead of the raw raster one.
                    let page_rotation_degrees = page_rotations.get((*page_number - 1) as usize).copied().unwrap_or(0);
                    crate::pdf::structure::adapters::reattach_detached_ocr_list_markers(
                        &mut paragraphs,
                        page_rotation_degrees,
                    );
                    let mut new_page_doc = crate::pdf::structure::assemble_internal_document(
                        vec![paragraphs],
                        &existing.tables,
                        Some(&existing.images),
                        &[],
                    );
                    new_page_doc.prebuilt_ocr_elements = existing.prebuilt_ocr_elements.clone();
                    new_page_doc.processing_warnings = existing.processing_warnings.clone();
                    new_page_doc
                }
            };
            structured_ocr_pages.insert(*page_number, new_page_doc);
        }
    }

    let result = apply_ocr_page_replacements(native_text, boundaries, &accepted_replacements);

    Ok((
        result,
        accepted_replacements,
        structured_ocr_pages,
        accumulated_llm_usage,
        if capture_rasters { Some(captured_rasters) } else { None },
        accumulated_formulas,
        preprocessing_by_page,
        ocr_confidence_by_page,
        accumulated_warnings,
    ))
}
/// Extract text from PDF using OCR on pre-rendered page images.
///
/// When `layout_detections` are provided (pixel-space, from the same images), uses
/// layout-aware markdown assembly for structured output. Otherwise, when
/// `config.output_format` is not [`OutputFormat::Plain`], structure (headings, list
/// items) is instead recovered document-wide by the same font-clustering heuristic
/// the native xberg_native_pdf path uses
/// (`pdf::structure::pipeline::extract_document_structure_from_segments`), fed from
/// segments harvested out of the per-page OCR paragraphs
/// (`pdf::structure::adapters::segments_from_ocr_pages`). Under `Plain`, or when that
/// heuristic yields nothing, falls back to plain OCR text concatenation.
///
/// # Arguments
///
/// * `images` - Pre-rendered page images (shared with layout detection)
/// * `layout_detections` - Optional pixel-space layout detections per page
/// * `config` - Extraction configuration including OCR settings
///
/// # Returns
///
/// Concatenated text from all pages, with markdown structure when layout is available
///
/// Thin, signature-preserving wrapper over [`extract_with_ocr_for_page`] with no page
/// rotation override (`0`), so every pre-existing caller keeps today's behavior: content-based
/// per-page auto-detection when `content` is available and index-aligned to `images`, or no
/// rotation correction at all otherwise.
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
pub(crate) async fn extract_with_ocr(
    content: Option<&[u8]>,
    images: Option<&[image::DynamicImage]>,
    #[cfg(feature = "layout-detection")] layout_detections: Option<&[crate::layout::DetectionResult]>,
    config: &ExtractionConfig,
    path: Option<&std::path::Path>,
) -> crate::Result<(
    String,
    Option<f64>,
    Vec<crate::types::Table>,
    Vec<crate::types::OcrElement>,
    Option<crate::types::internal::InternalDocument>,
    Vec<crate::types::LlmUsage>,
    Vec<String>,
    Option<Vec<crate::types::ExtractedImage>>,
    Vec<crate::types::Formula>,
    ahash::AHashMap<u32, crate::types::ImagePreprocessingMetadata>,
    ahash::AHashMap<u32, crate::types::page::PageOcrConfidence>,
)> {
    let (
        text,
        mean_conf,
        tables,
        elements,
        doc,
        usage,
        page_texts,
        rasters,
        formulas,
        _raw_page_paragraphs,
        preprocessing,
        ocr_confidence,
        _recognition_noise_verdicts,
    ) = Box::pin(extract_with_ocr_for_page(
        content,
        images,
        #[cfg(feature = "layout-detection")]
        layout_detections,
        config,
        path,
        0,
        false,
        None,
        0,
    ))
    .await?;
    Ok((
        text,
        mean_conf,
        tables,
        elements,
        doc,
        usage,
        page_texts,
        rasters,
        formulas,
        preprocessing,
        ocr_confidence,
    ))
}
/// Same as [`extract_with_ocr`], but `page_rotation_override` -- when non-zero -- is used as
/// the known `/Rotate` value for every image in `images` instead of this function's own
/// content-based per-page lookup (`lazy_pdf_render_state` / `external_image_page_rotations`).
///
/// Needed by a caller that hands in a single image detached from its original page index
/// (currently only [`run_ocr_pipeline_for_page`]'s per-stage runs), where an index-based
/// content lookup would silently resolve the wrong page's rotation (or none at all) for any
/// page but the document's first. `0` defers entirely to the existing per-page
/// auto-detection -- see [`extract_with_ocr`], the public entry point every other caller uses.
///
/// `skip_document_global_heuristic` -- when `true`, skips the internal
/// [`heuristically_restructured_ocr_pages`] call and always uses the plain,
/// unstructured `assemble_internal_document` fallback for the returned document instead.
/// Needed by [`extract_mixed_ocr_native`]'s per-page pipeline route (#665/#1423-followup):
/// that caller drives this function with exactly one image per call (one PDF page detached
/// from the rest of the document), so the heuristic -- which clusters font sizes and needs
/// several pages in hand to be useful -- would only ever see a single page here and either
/// do nothing or, worse, mark that page's paragraphs "already structured" and block the
/// caller's own *document-wide* pass over every OCR'd page from running at all. The extra
/// tuple element this function returns (bare, unclassified per-page paragraphs) is what lets
/// that caller run the heuristic itself, once, over the whole document.
///
/// `points_per_pixel_override` -- when `Some`, used in place of this function's own
/// [`ocr_points_per_pixel`] lookup for every image in `images`. That lookup resolves a page's
/// pixel -> point scale from `lazy_pdf_render_state`, derived from `content` and indexed by
/// this function's own 0-based loop position over `images` -- exactly the "single detached
/// image is not really page 0" problem `page_rotation_override` already exists to solve, so a
/// caller driving one detached image per call (currently only
/// `extract_mixed_ocr_native`'s per-page pipeline route) needs the same override here: without
/// it, `content: None` makes the lookup fall back to `1.0` (pixels treated as points), silently
/// defeating the document-global heading heuristic's absolute-point font-gap comparisons.
///
/// `page_index_offset` maps this function's local image indices back to document page
/// indices when a caller supplies a detached page image. It affects externally visible page
/// identity only; internal vectors remain indexed from zero.
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
// Each parameter is independently documented above and forwarded verbatim by every caller
// (see `run_ocr_pipeline_for_page`); bundling them into a params struct would only move the
// arity, not remove it, since callers still need to name each field individually. Matches the
// same suppression already used for other multi-parameter internal helpers in this crate (see
// `onnx::download_model_files`).
#[allow(clippy::too_many_arguments)]
pub(super) async fn extract_with_ocr_for_page(
    content: Option<&[u8]>,
    images: Option<&[image::DynamicImage]>,
    #[cfg(feature = "layout-detection")] layout_detections: Option<&[crate::layout::DetectionResult]>,
    config: &ExtractionConfig,
    path: Option<&std::path::Path>,
    page_rotation_override: u32,
    skip_document_global_heuristic: bool,
    points_per_pixel_override: Option<f32>,
    page_index_offset: usize,
) -> crate::Result<(
    String,
    Option<f64>,
    Vec<crate::types::Table>,
    Vec<crate::types::OcrElement>,
    Option<crate::types::internal::InternalDocument>,
    Vec<crate::types::LlmUsage>,
    Vec<String>,
    Option<Vec<crate::types::ExtractedImage>>,
    Vec<crate::types::Formula>,
    Vec<Vec<crate::pdf::structure::types::PdfParagraph>>,
    ahash::AHashMap<u32, crate::types::ImagePreprocessingMetadata>,
    ahash::AHashMap<u32, crate::types::page::PageOcrConfidence>,
    Vec<OcrPageNoiseVerdict>,
)> {
    use crate::plugins::registry::get_ocr_backend_registry;
    use image::ImageEncoder;
    use image::codecs::png::PngEncoder;
    use std::io::Cursor;

    // Re-seed the built-in backends if a prior `clear_ocr_backends()` call (a real user API,
    // also exercised by binding e2e suites sharing this process) left the registry without a
    // usable default. Cheap when the default is already present -- see
    // `ensure_ocr_backends_initialized`'s own doc comment. ~keep
    crate::plugins::ensure_ocr_backends_initialized();

    // Same slice as `ocr_points_per_pixel`'s suppression above: the only two readers of
    // `points_per_pixel_override` are compiled out, but every caller still passes it. ~keep
    #[cfg(all(feature = "layout-detection", not(feature = "ocr"), not(feature = "ocr-wasm")))]
    let _ = points_per_pixel_override;

    let default_ocr_config = crate::core::config::OcrConfig::default();
    let base_ocr_config = config.ocr.as_ref().unwrap_or(&default_ocr_config);

    let accel_ocr_config;
    let base_ocr_config = if (base_ocr_config.acceleration.is_none() && config.acceleration.is_some())
        || (base_ocr_config.security_limits.is_none() && config.security_limits.is_some())
    {
        accel_ocr_config = {
            let mut c = base_ocr_config.clone();
            if c.acceleration.is_none() {
                c.acceleration = config.acceleration.clone();
            }
            // GH#1554: mirrors `acceleration` above so the scanned-page OCR route inherits the
            // caller's configured decode limits instead of always falling back to
            // `SecurityLimits::default()`. ~keep
            if c.security_limits.is_none() {
                c.security_limits = config.security_limits.clone();
            }
            c
        };
        &accel_ocr_config
    } else {
        base_ocr_config
    };

    let backend = {
        let registry = get_ocr_backend_registry();
        let registry = registry.read();
        registry.get(&base_ocr_config.backend)?
    };
    // Only `ConfidenceSemantics::Legibility` backends report a `mean_text_conf` whose scale
    // means legibility; anything else (`Uncalibrated`, `None`) must never be turned into a
    // normalized confidence, or a page-rejection gate downstream would compare it against an
    // absolute floor it was never calibrated against (see `resolve_confidence_semantics`).
    let backend_confidence_semantics = backend.confidence_semantics();
    // Owned up front: the per-page summaries below outlive the borrow-heavy OCR loop, and
    // the backend that actually produced a page is part of what `ocr_confidence` reports. ~keep
    let backend_name = backend.name().to_string();
    let backend_confidence_scale = match backend_confidence_semantics {
        crate::plugins::ConfidenceSemantics::Legibility { scale_max } if scale_max > 0.0 => Some(scale_max),
        _ => None,
    };
    // Only meaningful with the `pdf` feature: without it there is no `/Rotate` to correct for
    // (`page_rotation_degrees` is always `0` below), so nothing reads this in that build.
    #[cfg(feature = "pdf")]
    let orientation_handling = backend.page_orientation_handling();

    let structured_ocr_config;
    let ocr_config = {
        let cfg = ensure_elements_enabled(base_ocr_config);
        #[cfg(all(feature = "ocr", feature = "layout-detection"))]
        let cfg = if layout_detections.is_some() || backend.emits_structured_markdown() {
            inject_layout_config_to_backend(&cfg, config)
        } else {
            cfg
        };
        structured_ocr_config = cfg;
        &structured_ocr_config
    };

    #[cfg(not(feature = "layout-detection"))]
    let supports_doc = backend.supports_document_processing();
    #[cfg(feature = "layout-detection")]
    let supports_doc = backend.supports_document_processing() && layout_detections.is_none();

    #[cfg(feature = "pdf")]
    let page_margins = crate::pdf::native::text::PageMarginFractions::from_extraction_config(Some(config));
    #[cfg(not(feature = "pdf"))]
    let use_document_processing = supports_doc && path.is_some();
    #[cfg(feature = "pdf")]
    let use_document_processing = should_use_document_processing(supports_doc, path.is_some(), page_margins);

    if let Some(doc_path) = path
        && use_document_processing
    {
        tracing::debug!(backend = %ocr_config.backend, "Using document-level OCR processing");
        let result = backend.process_document(doc_path, ocr_config).await?;
        let preprocessing = result
            .metadata
            .image_preprocessing
            .clone()
            .map(|metadata| ahash::AHashMap::from([(1, metadata)]))
            .unwrap_or_default();
        let mean_conf = backend_confidence_scale.and_then(|scale_max| {
            result
                .metadata
                .additional
                .get("mean_text_conf")
                .and_then(|v| v.as_f64())
                .map(|v| v / scale_max)
        });
        let backend_elements = result.ocr_elements.unwrap_or_default();
        let ocr_elements = filter_public_ocr_elements(&backend_elements, base_ocr_config);
        let llm_usage = result.llm_usage.unwrap_or_default();
        let formulas = result.formulas;
        let page_texts = if let Some(pages) = result.pages {
            pages.into_iter().map(|p| p.content).collect()
        } else {
            vec![result.content.clone()]
        };
        return Ok((
            result.content,
            mean_conf,
            Vec::new(),
            ocr_elements,
            None,
            llm_usage,
            page_texts,
            None,
            formulas,
            // The document-level backend route returns whole-document content, not per-page
            // paragraph geometry, so there is nothing for the document-global heuristic to
            // cluster over here.
            Vec::new(),
            preprocessing,
            // The backend judged the document as a whole, so there is no per-page confidence
            // to report. An empty map keeps every page's `ocr_confidence` absent rather than
            // pinning a document-wide number onto page 1 and leaving the rest bare (#1568). ~keep
            ahash::AHashMap::new(),
            // Same reasoning: this route never calls `accept_or_reject_ocr_page` per page.
            Vec::new(),
        ));
    }
    let capture_rasters = config.images.as_ref().is_some_and(|c| c.include_page_rasters);
    let mut captured_rasters: Vec<crate::types::ExtractedImage> = Vec::new();
    let mut preprocessing_by_page: ahash::AHashMap<u32, crate::types::ImagePreprocessingMetadata> =
        ahash::AHashMap::new();

    #[cfg(feature = "pdf")]
    let lazy_pdf_render_state = if !use_document_processing && images.is_none() {
        content.map(open_pdf_for_full_ocr).transpose()?
    } else {
        None
    };
    #[cfg(feature = "pdf")]
    let lazy_pdf_page_count = lazy_pdf_render_state
        .as_ref()
        .map_or(0, |(_, page_count, _)| *page_count);
    #[cfg(not(feature = "pdf"))]
    let lazy_pdf_page_count = 0;

    // rayon's work-stealing pool needs OS threads; wasm32 has none, so the parallel encode
    // paths below fall back to sequential `.iter()` there. Gate the import to match. ~keep
    #[cfg(all(feature = "tokio-runtime", not(target_arch = "wasm32")))]
    use rayon::prelude::*;
    use std::sync::Arc;
    #[cfg(all(feature = "tokio-runtime", not(target_arch = "wasm32")))]
    use tokio::task::JoinSet;

    let configured_batch_size = crate::core::config::concurrency::resolve_thread_budget(config.concurrency.as_ref());

    let batch_size = if images.is_none() {
        adapt_batch_size_to_memory(configured_batch_size, content.map(|b| b.len()).unwrap_or(0))
    } else {
        configured_batch_size
    };

    if batch_size < configured_batch_size {
        tracing::info!(
            configured = configured_batch_size,
            adapted = batch_size,
            "Reduced OCR batch size to fit available memory"
        );
    }

    let mut ocr_config_owned = ocr_config.clone();
    ocr_config_owned.acceleration = config.acceleration.clone();
    // GH#1554: mirrors `acceleration` above so the full-document scanned-page OCR route
    // inherits the caller's configured decode limits instead of always falling back to
    // `SecurityLimits::default()`. ~keep
    ocr_config_owned.security_limits = config.security_limits.clone();
    let total_pages = if let Some(imgs) = images {
        imgs.len()
    } else {
        lazy_pdf_page_count
    };

    // The layout-detection route hands in pre-rendered `images`, so `lazy_pdf_render_state`
    // above is never populated (it's gated on `images.is_none()`) and the page-rotation
    // lookups below would always fall back to their `unwrap_or(0)` default. When the
    // original PDF bytes are still available via `content`, read the same lightweight
    // `/Rotate` lookup the full-page and per-page routes use so this route's calls to
    // `ocr_config_with_page_rotation_hint` get a real hint too (see #530 / 972d2269f7).
    // `content` is `None` on some image-only callers (e.g. the image extractor's own OCR
    // path with no source PDF); there is genuinely no rotation to read in that case.
    #[cfg(feature = "pdf")]
    let external_image_page_rotations: Option<Vec<u32>> = if images.is_some() {
        content.map(|c| crate::pdf::render::get_page_rotations_from_bytes(c, total_pages))
    } else {
        None
    };

    let mut page_texts = vec![String::new(); total_pages];
    // Which pages the quality gate rejected. Kept separately from `page_texts` because an
    // empty string is ambiguous -- a blank page produces one too -- and the structured
    // paragraphs of a rejected page must be dropped along with its text.
    let mut rejected_pages = vec![false; total_pages];
    let mut all_page_paragraphs: Vec<Option<Vec<crate::pdf::structure::types::PdfParagraph>>> = vec![None; total_pages];
    // Written only by the two OCR-paragraph assembly blocks below (`layout-detection`
    // *with* `ocr`/`ocr-wasm`, or `not(layout-detection)`); still read unconditionally by
    // the document-global heuristic. `layout-detection` without either OCR frontend (the
    // `formula-recognition,pdf` CI leg) leaves it write-free. ~keep
    #[cfg_attr(
        all(feature = "layout-detection", not(feature = "ocr"), not(feature = "ocr-wasm")),
        allow(unused_mut)
    )]
    let mut ocr_page_heights = vec![0.0_f32; total_pages];
    #[allow(unused_mut)]
    let mut collected_tables: Vec<crate::types::Table> = Vec::new();
    let mut all_ocr_elements: Vec<crate::types::OcrElement> = Vec::new();
    let mut accumulated_llm_usage: Vec<crate::types::LlmUsage> = Vec::new();
    let mut accumulated_formulas: Vec<crate::types::Formula> = Vec::new();
    let mut conf_sum: f64 = 0.0;
    let mut conf_count: usize = 0;
    // Per-page OCR confidence summaries, keyed by 1-based document page number (#1568).
    // Only pages this route actually OCR'd get an entry, so an absent key downstream means
    // "not OCR'd" rather than "OCR'd with nothing to report". ~keep
    let mut ocr_confidence_by_page: ahash::AHashMap<u32, crate::types::page::PageOcrConfidence> =
        ahash::AHashMap::new();
    // Warnings from the force_ocr image-XObject fallback (#1355): a page rendered
    // blank by xberg_native_pdf but carrying image XObjects the renderer couldn't paint.
    #[cfg(feature = "pdf")]
    let mut image_fallback_warnings: Vec<crate::types::ProcessingWarning> = Vec::new();
    #[cfg(feature = "pdf")]
    let mut xobject_recovery_budget = crate::extractors::security::SecurityBudget::from_config(config);

    // #1444: a backend failure on one page used to propagate with `?`, aborting the whole
    // extraction and — crucially — never reaching the image-XObject fallback below, which is
    // exactly the recovery such a page needs. Failures are captured per page instead: the
    // page degrades to empty text (so the blank-page fallback fires for it), the failure is
    // surfaced as a warning, and only a document where *every* page failed *and* nothing was
    // recovered still returns an error.
    let mut page_backend_errors: Vec<(usize, String)> = Vec::new();
    let mut page_failure_warnings: Vec<crate::types::ProcessingWarning> = Vec::new();

    #[cfg(feature = "pdf")]
    let mut margin_filter_warnings: Vec<crate::types::ProcessingWarning> = Vec::new();

    // Opened on first blank page only; see `fallback_render_document`.
    #[cfg(feature = "pdf")]
    let mut fallback_pdf_state: Option<Option<xberg_native_pdf::PdfDocument>> = None;

    // Judged per page just before the OCR text is accepted, so a drawing page contributes
    // nothing instead of contributing invented words. See `is_ocr_recognition_noise`.
    let ocr_output_thresholds = base_ocr_config.quality_thresholds.clone().unwrap_or_default();
    let mut recognition_noise_warnings: Vec<crate::types::ProcessingWarning> = Vec::new();
    // Numeric evidence behind each fired `recognition_noise_warnings` entry, so a caller
    // above the accept/reject decision can observe the signal without recomputing stats
    // (see `OcrPageNoiseVerdict`). Populated exactly when a warning above is pushed. ~keep
    let mut recognition_noise_verdicts: Vec<OcrPageNoiseVerdict> = Vec::new();

    #[cfg(feature = "layout-detection")]
    let mut tatr_model = if layout_detections.is_some() {
        crate::layout::take_or_create_tatr(
            config.resolved_layout_acceleration(),
            crate::core::config::concurrency::resolve_thread_budget(config.concurrency.as_ref()),
        )
    } else {
        None
    };

    for batch_start in (0..total_pages).step_by(batch_size) {
        let batch_end = (batch_start + batch_size).min(total_pages);

        #[allow(unused_variables)]
        let (batch_slice, encoded_batch) = if let Some(imgs) = images {
            let slice: Cow<'_, [image::DynamicImage]> = Cow::Borrowed(&imgs[batch_start..batch_end]);
            let default_security_limits = crate::extractors::security::SecurityLimits::default();
            let security_limits = config.security_limits.as_ref().unwrap_or(&default_security_limits);
            #[cfg(all(feature = "tokio-runtime", not(target_arch = "wasm32")))]
            validate_png_encode_batch_peak(slice.iter(), true, security_limits)?;
            #[cfg(any(not(feature = "tokio-runtime"), target_arch = "wasm32"))]
            validate_png_encode_batch_peak(slice.iter(), false, security_limits)?;
            #[allow(clippy::type_complexity)]
            #[cfg(all(feature = "tokio-runtime", not(target_arch = "wasm32")))]
            let encoded: crate::Result<Vec<(usize, Arc<Vec<u8>>, u32, u32)>> = slice
                .par_iter()
                .enumerate()
                .map(|(offset, image)| {
                    let page_idx = batch_start + offset;
                    let default_security_limits = crate::extractors::security::SecurityLimits::default();
                    let security_limits = config.security_limits.as_ref().unwrap_or(&default_security_limits);
                    let rgb_image = clone_rgb_for_png_encode(image, security_limits)?;
                    let (width, height) = rgb_image.dimensions();
                    let mut image_bytes = Cursor::new(Vec::new());
                    let encoder = PngEncoder::new(&mut image_bytes);
                    encoder
                        .write_image(&rgb_image, width, height, image::ColorType::Rgb8.into())
                        .map_err(|e| crate::XbergError::Parsing {
                            message: format!("Failed to encode image: {}", e),
                            source: None,
                        })?;
                    Ok((page_idx, Arc::new(image_bytes.into_inner()), width, height))
                })
                .collect();
            #[allow(clippy::type_complexity)]
            #[cfg(any(not(feature = "tokio-runtime"), target_arch = "wasm32"))]
            let encoded: crate::Result<Vec<(usize, Arc<Vec<u8>>, u32, u32)>> = slice
                .iter()
                .enumerate()
                .map(|(offset, image)| {
                    let page_idx = batch_start + offset;
                    let default_security_limits = crate::extractors::security::SecurityLimits::default();
                    let security_limits = config.security_limits.as_ref().unwrap_or(&default_security_limits);
                    let rgb_image = clone_rgb_for_png_encode(image, security_limits)?;
                    let (width, height) = rgb_image.dimensions();
                    let mut image_bytes = Cursor::new(Vec::new());
                    let encoder = PngEncoder::new(&mut image_bytes);
                    encoder
                        .write_image(&rgb_image, width, height, image::ColorType::Rgb8.into())
                        .map_err(|e| crate::XbergError::Parsing {
                            message: format!("Failed to encode image: {}", e),
                            source: None,
                        })?;
                    Ok((page_idx, Arc::new(image_bytes.into_inner()), width, height))
                })
                .collect();
            (Some(slice), encoded?)
        } else {
            #[cfg(feature = "pdf")]
            let encoded = {
                let (doc, _, page_rotations) =
                    lazy_pdf_render_state
                        .as_ref()
                        .ok_or_else(|| crate::XbergError::Parsing {
                            message: "PDF content is required for OCR rendering but was not provided".to_string(),
                            source: None,
                        })?;
                let default_security_limits = crate::extractors::security::SecurityLimits::default();
                let security_limits = config.security_limits.as_ref().unwrap_or(&default_security_limits);
                render_full_pdf_ocr_batch(doc, page_rotations, batch_start..batch_end, security_limits)?
            };
            #[cfg(not(feature = "pdf"))]
            let encoded: Vec<(usize, Arc<Vec<u8>>, u32, u32)> = Vec::new();
            (None::<Cow<'_, [image::DynamicImage]>>, encoded)
        };

        let batch_count = encoded_batch.len();
        let mut batch_ocr_results: Vec<Option<crate::types::ExtractedDocument>> = vec![None; batch_count];
        // Backend error text for a page whose OCR call failed, if any (#1444). The page's
        // entry in `batch_ocr_results` is an empty document in that case, so the blank-page
        // image-XObject fallback below gets its chance before the failure is reported.
        let mut batch_page_errors: Vec<Option<String>> = vec![None; batch_count];
        // (correction_degrees, upright_width, upright_height) applied by
        // `upright_raster_for_backend` before this page's backend call, so the matching
        // `undo_upright_raster_correction` below maps the result back correctly (#643).
        let mut batch_upright_correction: Vec<(u32, u32, u32)> = vec![(0, 0, 0); batch_count];

        // See the sibling JoinSet block above: `Send` futures aren't available on wasm32. ~keep
        #[cfg(all(feature = "tokio-runtime", not(target_arch = "wasm32")))]
        {
            let mut join_set: JoinSet<(usize, u32, u32, u32, crate::Result<crate::types::ExtractedDocument>)> =
                JoinSet::new();
            for (page_idx, image_data, width, height) in &encoded_batch {
                let backend_clone = std::sync::Arc::clone(&backend);
                #[cfg(feature = "pdf")]
                let page_rotation_degrees = if page_rotation_override != 0 {
                    page_rotation_override
                } else {
                    lazy_pdf_render_state
                        .as_ref()
                        .and_then(|(_, _, rotations)| rotations.get(*page_idx))
                        .or_else(|| external_image_page_rotations.as_ref().and_then(|r| r.get(*page_idx)))
                        .copied()
                        .unwrap_or(0)
                };
                #[cfg(not(feature = "pdf"))]
                let page_rotation_degrees: u32 = 0;
                // Only the branch that rendered the pages itself knows their resolution.
                // `lazy_pdf_render_state` is exactly that branch's marker — it is only opened
                // when `images.is_none()` (see its `let` above) — so when the caller supplied
                // arbitrary pre-rendered images the hint stays absent and they keep the 72-DPI
                // assumption, which for them is the honest answer.
                #[cfg(feature = "pdf")]
                let source_dpi = lazy_pdf_render_state
                    .as_ref()
                    .and_then(|(doc, _, _)| rendered_page_source_dpi(doc, *page_idx, *width));
                #[cfg(not(feature = "pdf"))]
                let source_dpi: Option<f64> = None;
                let config_clone =
                    ocr_config_with_page_rotation_hint(&ocr_config_owned, page_rotation_degrees, source_dpi)
                        .into_owned();
                // No PDF `/Rotate` is ever known without the `pdf` feature (`page_rotation_degrees`
                // is always `0` above in that build), so there is nothing to correct upright.
                #[cfg(feature = "pdf")]
                let (upright_data, upright_width, upright_height, correction_degrees) = upright_raster_for_backend(
                    image_data,
                    *width,
                    *height,
                    page_rotation_degrees,
                    orientation_handling,
                    config.security_limits.as_ref(),
                )?;
                #[cfg(not(feature = "pdf"))]
                let (upright_data, upright_width, upright_height, correction_degrees) =
                    (Arc::clone(image_data), *width, *height, 0u32);
                let idx = *page_idx;
                join_set.spawn(async move {
                    let result = backend_clone.process_image_owned(upright_data, &config_clone).await;
                    (idx, correction_degrees, upright_width, upright_height, result)
                });
            }
            while let Some(join_result) = join_set.join_next().await {
                let (page_idx, correction_degrees, upright_width, upright_height, ocr_result) =
                    join_result.map_err(|e| crate::XbergError::Plugin {
                        message: format!("OCR task panicked: {}", e),
                        plugin_name: "ocr".to_string(),
                    })?;
                batch_upright_correction[page_idx - batch_start] = (correction_degrees, upright_width, upright_height);
                match ocr_result {
                    Ok(document) => batch_ocr_results[page_idx - batch_start] = Some(document),
                    Err(error) => {
                        tracing::warn!(
                            page = page_index_offset + page_idx + 1,
                            error = %error,
                            "OCR backend failed for page"
                        );
                        batch_page_errors[page_idx - batch_start] = Some(error.to_string());
                        batch_ocr_results[page_idx - batch_start] = Some(crate::types::ExtractedDocument::default());
                    }
                }
            }
        }
        #[cfg(any(not(feature = "tokio-runtime"), target_arch = "wasm32"))]
        {
            for (page_idx, image_data, width, height) in &encoded_batch {
                #[cfg(feature = "pdf")]
                let page_rotation_degrees = if page_rotation_override != 0 {
                    page_rotation_override
                } else {
                    lazy_pdf_render_state
                        .as_ref()
                        .and_then(|(_, _, rotations)| rotations.get(*page_idx))
                        .or_else(|| external_image_page_rotations.as_ref().and_then(|r| r.get(*page_idx)))
                        .copied()
                        .unwrap_or(0)
                };
                #[cfg(not(feature = "pdf"))]
                let page_rotation_degrees: u32 = 0;
                // See the JoinSet branch above: only the PDF-rendered branch knows the DPI.
                #[cfg(feature = "pdf")]
                let source_dpi = lazy_pdf_render_state
                    .as_ref()
                    .and_then(|(doc, _, _)| rendered_page_source_dpi(doc, *page_idx, *width));
                #[cfg(not(feature = "pdf"))]
                let source_dpi: Option<f64> = None;
                let config_for_page =
                    ocr_config_with_page_rotation_hint(&ocr_config_owned, page_rotation_degrees, source_dpi);
                #[cfg(feature = "pdf")]
                let (upright_data, upright_width, upright_height, correction_degrees) = upright_raster_for_backend(
                    image_data,
                    *width,
                    *height,
                    page_rotation_degrees,
                    orientation_handling,
                    config.security_limits.as_ref(),
                )?;
                #[cfg(not(feature = "pdf"))]
                let (upright_data, upright_width, upright_height, correction_degrees) =
                    (Arc::clone(image_data), *width, *height, 0u32);
                let ocr_result = backend
                    .process_image(upright_data.as_slice(), config_for_page.as_ref())
                    .await;
                batch_upright_correction[page_idx - batch_start] = (correction_degrees, upright_width, upright_height);
                match ocr_result {
                    Ok(document) => batch_ocr_results[page_idx - batch_start] = Some(document),
                    Err(error) => {
                        tracing::warn!(
                            page = page_index_offset + page_idx + 1,
                            error = %error,
                            "OCR backend failed for page"
                        );
                        batch_page_errors[page_idx - batch_start] = Some(error.to_string());
                        batch_ocr_results[page_idx - batch_start] = Some(crate::types::ExtractedDocument::default());
                    }
                }
            }
        }

        for offset in 0..batch_count {
            let page_idx = batch_start + offset;
            let document_page_idx = page_index_offset + page_idx;
            let document_page_number = (document_page_idx + 1) as u32;
            let mut ocr_result = batch_ocr_results[offset].take().expect("OCR result missing for page");
            if let Some(metadata) = ocr_result.metadata.image_preprocessing.clone() {
                preprocessing_by_page.insert(document_page_number, metadata);
            }
            #[cfg(feature = "pdf")]
            {
                let (correction_degrees, upright_width, upright_height) = batch_upright_correction[offset];
                undo_upright_raster_correction(&mut ocr_result, correction_degrees, upright_width, upright_height);
            }
            #[cfg(feature = "layout-detection")]
            let _height = encoded_batch[offset].3;

            if let Some(conf_val) = ocr_result
                .metadata
                .additional
                .get("mean_text_conf")
                .and_then(|v| v.as_i64())
            {
                conf_sum += conf_val as f64;
                conf_count += 1;
            }

            if let Some(usage) = ocr_result.llm_usage.take() {
                accumulated_llm_usage.extend(usage);
            }

            let mut backend_tables = std::mem::take(&mut ocr_result.tables);
            for table in &mut backend_tables {
                table.page_number = document_page_number;
            }
            #[cfg(feature = "pdf")]
            if let Some((doc, _, _)) = lazy_pdf_render_state.as_ref() {
                let (page_width_pt, page_height_pt) = page_dimensions_pt(doc, page_idx);
                rescale_ocr_bboxes_to_page_points(
                    None,
                    &mut backend_tables,
                    encoded_batch[offset].2,
                    encoded_batch[offset].3,
                    page_width_pt,
                    page_height_pt,
                );
            }
            collected_tables.append(&mut backend_tables);

            if let Some(ref mut elems) = ocr_result.ocr_elements {
                #[cfg(feature = "pdf")]
                let public_elements = {
                    let (_, layout_height) = resolved_ocr_layout_dimensions(
                        &ocr_result.metadata,
                        encoded_batch[offset].2,
                        encoded_batch[offset].3,
                    );
                    let (public_elements, outcome) = public_ocr_elements_for_pdf_page(
                        elems,
                        base_ocr_config,
                        document_page_number,
                        layout_height,
                        page_margins,
                    );
                    if outcome.missing_geometry && !public_elements.is_empty() {
                        crate::core::diagnostics::push_warning_deduped(
                            &mut margin_filter_warnings,
                            ocr_margin_filter_capability_warning(),
                        );
                    }
                    public_elements
                };
                #[cfg(not(feature = "pdf"))]
                let public_elements = {
                    for elem in elems.iter_mut() {
                        elem.page_number = document_page_number;
                    }
                    filter_public_ocr_elements(elems, base_ocr_config)
                };
                all_ocr_elements.extend(public_elements);
            }

            for mut formula in ocr_result.formulas {
                formula.page = Some(document_page_number);
                #[cfg(feature = "pdf")]
                if let Some((doc, _, _)) = lazy_pdf_render_state.as_ref() {
                    let (w, h) = (encoded_batch[offset].2, encoded_batch[offset].3);
                    formula_bbox_to_page_points(&mut formula, doc, page_idx, Some(&ocr_result.metadata), w, h);
                }
                accumulated_formulas.push(formula);
            }

            // force_ocr image-XObject fallback (#1355): xberg_native_pdf can catch an
            // image-decode error internally and substitute a blank white bitmap for
            // the whole-page render, so the page comes back from OCR as blank with no
            // indication anything was wrong. When that happens and the page actually
            // carries image XObjects, retry OCR directly on the embedded image bytes
            // (decoded pixels re-encoded to PNG, or the raw JPEG/JP2 stream) and always
            // surface a warning so the silent drop becomes visible.
            //
            // #1444 widened when this runs: a page whose backend call *failed* arrives here
            // as an empty document (see `batch_page_errors`), and a page the backend merely
            // *described* as blank is caught by the ink probe in
            // `page_needs_xobject_fallback` -- both used to sail past this block.
            #[cfg(feature = "pdf")]
            let default_security_limits = crate::extractors::security::SecurityLimits::default();
            let security_limits = config.security_limits.as_ref().unwrap_or(&default_security_limits);
            if page_needs_xobject_fallback(&ocr_result.content, encoded_batch[offset].1.as_slice(), security_limits) {
                // The layout-detection route hands in pre-rendered `images`, which leaves
                // `lazy_pdf_render_state` unopened; that used to disable this fallback
                // entirely for exactly the scanned documents most likely to need it. The
                // fallback needs nothing from the render state but the page's XObject
                // table, so it opens its own handle rather than perturbing the rotation and
                // points-per-pixel lookups that state is indexed for (#1444).
                let render_doc = match lazy_pdf_render_state.as_ref() {
                    Some((doc, _, _)) => Some(doc),
                    None => fallback_render_document(&mut fallback_pdf_state, content),
                };
                if let Some(render_doc) = render_doc
                    && let Some(recovery) = recover_page_text_from_image_xobjects(
                        &backend,
                        render_doc,
                        document_page_idx,
                        &ocr_config_owned,
                        &mut xobject_recovery_budget,
                    )
                    .await?
                {
                    let XObjectRecoveryOutcome {
                        text,
                        attempted,
                        images,
                        mut llm_usage,
                        mut tables,
                        mut formulas,
                        image_preprocessing,
                    } = recovery;
                    if !text.is_empty() {
                        ocr_result.content = text;
                    }
                    accumulated_llm_usage.append(&mut llm_usage);
                    collected_tables.append(&mut tables);
                    accumulated_formulas.append(&mut formulas);
                    if let Some(metadata) = image_preprocessing {
                        preprocessing_by_page.insert(document_page_number, metadata);
                    }
                    if capture_rasters {
                        captured_rasters.extend(images);
                    }
                    image_fallback_warnings.push(xobject_fallback_warning(document_page_idx, attempted));
                }
            }

            // Report a page whose backend call failed, now that the fallback above has had
            // its chance to recover it (#1444). Either way the failure is visible: a page
            // that vanishes silently is the defect this replaces.
            if let Some(error) = batch_page_errors[offset].take() {
                let recovered = !ocr_result.content.trim().is_empty();
                page_failure_warnings.push(crate::types::ProcessingWarning {
                    source: std::borrow::Cow::Borrowed("ocr"),
                    message: std::borrow::Cow::Owned(if recovered {
                        format!(
                            "OCR of page {} failed ({error}); its text was recovered from the page's \
                             embedded image XObjects instead.",
                            document_page_number
                        )
                    } else {
                        format!(
                            "OCR of page {} failed and could not be recovered: {error}",
                            document_page_number
                        )
                    }),
                });
                page_backend_errors.push((page_idx, error));
            }

            // Both are read by the `pdf` margin-warning/page-content code below in every
            // feature set, but only written inside the two assembly blocks -- so with
            // `layout-detection` and neither OCR frontend they stay at their initializers.
            // ~keep
            #[cfg(feature = "pdf")]
            #[cfg_attr(
                all(feature = "layout-detection", not(feature = "ocr"), not(feature = "ocr-wasm")),
                allow(unused_mut)
            )]
            let mut margin_filtered_content: Option<String> = None;
            #[cfg(feature = "pdf")]
            #[cfg_attr(
                all(feature = "layout-detection", not(feature = "ocr"), not(feature = "ocr-wasm")),
                allow(unused_mut)
            )]
            let mut margin_filter_complete = false;

            #[cfg(all(feature = "layout-detection", any(feature = "ocr", feature = "ocr-wasm")))]
            if ocr_result.ocr_internal_document.is_some()
                || ocr_result
                    .ocr_elements
                    .as_ref()
                    .is_some_and(|elements| !elements.is_empty())
            {
                let elements = ocr_result.ocr_elements.as_deref().unwrap_or_default();
                let detection = layout_detections.and_then(|detections| detections.get(page_idx));

                let ocr_render_width = encoded_batch[offset].2;
                let ocr_render_height = encoded_batch[offset].3;
                let render_scaled_detection =
                    detection.map(|det| scale_detection_to_dimensions(det, ocr_render_width, ocr_render_height));
                let (_, ocr_layout_height) =
                    resolved_ocr_layout_dimensions(&ocr_result.metadata, ocr_render_width, ocr_render_height);
                ocr_page_heights[page_idx] = ocr_layout_height as f32;
                let points_per_pixel = points_per_pixel_override.unwrap_or_else(|| {
                    ocr_points_per_pixel(
                        #[cfg(feature = "pdf")]
                        lazy_pdf_render_state.as_ref(),
                        page_idx,
                        ocr_layout_height,
                    )
                });
                let ocr_scaled_detection = detection.map(|det| {
                    scale_detection_to_ocr_coordinates(det, &ocr_result.metadata, ocr_render_width, ocr_render_height)
                });
                let render_ocr_elements = transform_ocr_elements_to_render_space(
                    elements,
                    &ocr_result.metadata,
                    ocr_render_width,
                    ocr_render_height,
                );

                let recognized_tables = match (render_scaled_detection.as_ref(), tatr_model.as_mut()) {
                    (Some(scaled_det), Some(model)) => {
                        let rgb = if let Some(ref slice) = batch_slice {
                            let default_security_limits = crate::extractors::security::SecurityLimits::default();
                            let security_limits = config.security_limits.as_ref().unwrap_or(&default_security_limits);
                            crate::extraction::image_decode::clone_dynamic_image_to_rgb8_with_security_limits(
                                &slice[offset],
                                security_limits,
                            )?
                        } else {
                            let png_data = &encoded_batch[offset].1;
                            let default_security_limits = crate::extractors::security::SecurityLimits::default();
                            let security_limits = config.security_limits.as_ref().unwrap_or(&default_security_limits);
                            crate::extraction::image_decode::decode_standard_rgb8_with_security_limits(
                                png_data,
                                security_limits,
                            )
                            .map_err(|e| crate::XbergError::Parsing {
                                message: format!("Failed to decode PNG for TATR: {}", e),
                                source: None,
                            })?
                        };
                        crate::ocr::layout_assembly::recognize_page_tables(
                            &rgb,
                            scaled_det,
                            &render_ocr_elements,
                            model,
                        )
                    }
                    _ => Vec::new(),
                };

                for rt in &recognized_tables {
                    if !rt.markdown.is_empty() {
                        // The id is this table's 1-based position in `collected_tables`;
                        // pages are processed strictly in increasing `page_idx` order
                        // above, so push order is deterministic document order. ~keep
                        let table_index = collected_tables.len();
                        collected_tables.push(recognized_table_to_public_table(rt, document_page_number, table_index));
                    }
                }

                if let Some(ref ocr_doc) = ocr_result.ocr_internal_document {
                    // Same lookup as the per-page `page_rotation_degrees` computed above for
                    // the backend call (#760) -- that one is scoped to the join_set/backend
                    // loop and does not survive to here, so it is recomputed identically:
                    // an explicit override wins, otherwise the PDF's own known `/Rotate` for
                    // this page (falling back to the image-derived rotation when the render
                    // state was never opened), defaulting to `0` when nothing is known.
                    #[cfg(feature = "pdf")]
                    let page_rotation_degrees = if page_rotation_override != 0 {
                        page_rotation_override
                    } else {
                        lazy_pdf_render_state
                            .as_ref()
                            .and_then(|(_, _, rotations)| rotations.get(page_idx))
                            .or_else(|| external_image_page_rotations.as_ref().and_then(|r| r.get(page_idx)))
                            .copied()
                            .unwrap_or(0)
                    };
                    #[cfg(not(feature = "pdf"))]
                    let page_rotation_degrees: u32 = 0;
                    let mut paragraphs = assemble_ocr_page_paragraphs(
                        ocr_doc,
                        ocr_layout_height,
                        ocr_scaled_detection.as_ref(),
                        points_per_pixel,
                        page_rotation_degrees,
                    );
                    apply_ocr_layout_content_filter(&mut paragraphs, config);
                    #[cfg(feature = "pdf")]
                    {
                        let outcome = filter_ocr_paragraphs_by_page_margins(
                            &mut paragraphs,
                            ocr_layout_height as f32,
                            page_margins,
                        );
                        margin_filter_complete = !outcome.missing_geometry;
                        if outcome.removed {
                            margin_filtered_content = Some(ocr_paragraphs_plain_text(&paragraphs));
                        }
                    }

                    tracing::debug!(
                        page = document_page_number,
                        paragraphs = paragraphs.len(),
                        raw_content_len = ocr_result.content.len(),
                        "OCR page layout classification complete"
                    );

                    all_page_paragraphs[page_idx] = Some(paragraphs);
                }

                if capture_rasters {
                    let (_, png_arc, w, h) = &encoded_batch[offset];
                    let png_bytes = bytes::Bytes::copy_from_slice(png_arc.as_ref());
                    captured_rasters.push(build_page_raster_image(document_page_idx, png_bytes, *w, *h));
                }
                let dict_invalid_word_ratio = ocr_result
                    .metadata
                    .additional
                    .get(crate::ocr_metadata_keys::OCR_TESSERACT_DICT_INVALID_WORD_RATIO_METADATA_KEY)
                    .and_then(|v| v.as_f64());
                let confidence = mean_text_conf_of(&ocr_result.metadata.additional);
                if let Some(summary) = page_ocr_confidence(
                    backend_confidence_semantics,
                    confidence,
                    word_count_of(&ocr_result.metadata.additional).unwrap_or(0),
                    &backend_name,
                ) {
                    ocr_confidence_by_page.insert(document_page_number, summary);
                }
                #[cfg(feature = "pdf")]
                if (page_margins.top != 0.0 || page_margins.bottom != 0.0)
                    && !margin_filter_complete
                    && !ocr_result.content.trim().is_empty()
                {
                    crate::core::diagnostics::push_warning_deduped(
                        &mut margin_filter_warnings,
                        ocr_margin_filter_capability_warning(),
                    );
                }
                #[cfg(feature = "pdf")]
                let page_content = margin_filtered_content.unwrap_or(ocr_result.content);
                #[cfg(not(feature = "pdf"))]
                let page_content = ocr_result.content;
                let acceptance = accept_or_reject_ocr_page(
                    document_page_idx,
                    page_content,
                    &ocr_output_thresholds,
                    &mut recognition_noise_warnings,
                    dict_invalid_word_ratio,
                    backend_confidence_semantics,
                    confidence,
                );
                page_texts[page_idx] = acceptance.content;
                rejected_pages[page_idx] = acceptance.discarded;
                if let Some(verdict) = acceptance.verdict {
                    recognition_noise_verdicts.push(verdict);
                }
                continue;
            }

            #[cfg(not(feature = "layout-detection"))]
            if let Some(ref ocr_doc) = ocr_result.ocr_internal_document {
                let ocr_render_width = encoded_batch[offset].2;
                let ocr_render_height = encoded_batch[offset].3;
                let (_, ocr_layout_height) =
                    resolved_ocr_layout_dimensions(&ocr_result.metadata, ocr_render_width, ocr_render_height);
                ocr_page_heights[page_idx] = ocr_layout_height as f32;
                let points_per_pixel = points_per_pixel_override.unwrap_or_else(|| {
                    ocr_points_per_pixel(
                        #[cfg(feature = "pdf")]
                        lazy_pdf_render_state.as_ref(),
                        page_idx,
                        ocr_layout_height,
                    )
                });
                // `ocr_doc`'s bbox AND ocr_geometry are both still raw OCR raster pixels here
                // (same pure-OCR-route reasoning as `assemble_ocr_page_paragraphs` above).
                let font_size_scale = crate::pdf::structure::adapters::OcrFontSizeScale::uniform(points_per_pixel);
                let mut paragraphs =
                    crate::pdf::structure::adapters::ocr_doc_to_paragraphs(ocr_doc, ocr_layout_height, font_size_scale);
                apply_ocr_layout_content_filter(&mut paragraphs, config);
                #[cfg(feature = "pdf")]
                {
                    let outcome =
                        filter_ocr_paragraphs_by_page_margins(&mut paragraphs, ocr_layout_height as f32, page_margins);
                    margin_filter_complete = !outcome.missing_geometry;
                    if outcome.removed {
                        margin_filtered_content = Some(ocr_paragraphs_plain_text(&paragraphs));
                    }
                }
                all_page_paragraphs[page_idx] = Some(paragraphs);
            }

            let _ = page_idx;
            if capture_rasters {
                let (_, png_arc, w, h) = &encoded_batch[offset];
                let png_bytes = bytes::Bytes::copy_from_slice(png_arc.as_ref());
                captured_rasters.push(build_page_raster_image(document_page_idx, png_bytes, *w, *h));
            }
            let dict_invalid_word_ratio = ocr_result
                .metadata
                .additional
                .get(crate::ocr_metadata_keys::OCR_TESSERACT_DICT_INVALID_WORD_RATIO_METADATA_KEY)
                .and_then(|v| v.as_f64());
            let confidence = mean_text_conf_of(&ocr_result.metadata.additional);
            if let Some(summary) = page_ocr_confidence(
                backend_confidence_semantics,
                confidence,
                word_count_of(&ocr_result.metadata.additional).unwrap_or(0),
                &backend_name,
            ) {
                ocr_confidence_by_page.insert(document_page_number, summary);
            }
            #[cfg(feature = "pdf")]
            if (page_margins.top != 0.0 || page_margins.bottom != 0.0)
                && !margin_filter_complete
                && !ocr_result.content.trim().is_empty()
            {
                crate::core::diagnostics::push_warning_deduped(
                    &mut margin_filter_warnings,
                    ocr_margin_filter_capability_warning(),
                );
            }
            #[cfg(feature = "pdf")]
            let page_content = margin_filtered_content.unwrap_or(ocr_result.content);
            #[cfg(not(feature = "pdf"))]
            let page_content = ocr_result.content;
            let acceptance = accept_or_reject_ocr_page(
                document_page_idx,
                page_content,
                &ocr_output_thresholds,
                &mut recognition_noise_warnings,
                dict_invalid_word_ratio,
                backend_confidence_semantics,
                confidence,
            );
            page_texts[page_idx] = acceptance.content;
            rejected_pages[page_idx] = acceptance.discarded;
            if let Some(verdict) = acceptance.verdict {
                recognition_noise_verdicts.push(verdict);
            }
        }
    }

    #[cfg(feature = "layout-detection")]
    if let Some(model) = tatr_model.take() {
        crate::layout::return_tatr(model);
    }

    // Degrading a per-page failure to a warning must not turn a wholesale OCR failure into a
    // silently empty document: when every page failed *and* nothing was recovered from any
    // page's embedded images, there is no partial result to preserve, so report it (#1444).
    if !page_backend_errors.is_empty()
        && page_backend_errors.len() == total_pages
        && page_texts.iter().all(|text| text.trim().is_empty())
    {
        let (_, first_error) = &page_backend_errors[0];
        return Err(crate::XbergError::Plugin {
            message: format!(
                "OCR failed on all {total_pages} page(s) and no text could be recovered from the pages' \
                 embedded images; first failure: {first_error}"
            ),
            plugin_name: "ocr".to_string(),
        });
    }

    let mean_text_conf = match (conf_count > 0, backend_confidence_scale) {
        (true, Some(scale_max)) => Some((conf_sum / conf_count as f64) / scale_max),
        _ => None,
    };

    let page_marker_cfg = config.pages.as_ref().filter(|p| p.insert_page_markers);
    let mut result = String::new();
    for (i, text) in page_texts.iter().enumerate() {
        if let Some(cfg) = page_marker_cfg {
            let marker = cfg
                .marker_format
                .replace("{page_num}", &(page_index_offset + i + 1).to_string());
            result.push_str(&marker);
        } else if i > 0 {
            result.push_str("\n\n");
        }
        result.push_str(text);
    }

    // A page the quality gate rejected must not keep its structured paragraphs either.
    //
    // The paragraphs and the page text are two renderings of the same OCR output, built by
    // different assemblies: `all_page_paragraphs` above, `page_texts` from
    // `accept_or_reject_ocr_page`. Only the text was ever judged, and the rendered document
    // is built exclusively from the paragraphs -- so a scanned survey plat lost its text,
    // logged its warning, and rendered its garbage anyway. The mixed route already couples
    // the two (`ocr_results.retain` feeding `merge_structured_ocr_pages_into_internal_document`);
    // this is that same coupling for the force-ocr route, which never had it.
    //
    // Clearing before `fill_unstructured_ocr_pages` is deliberate and safe: that function
    // only rebuilds paragraphs from `page_texts[page_index]`, which is empty for exactly
    // these pages, so it cannot resurrect them.
    for (page_idx, page_rejected) in rejected_pages.iter().enumerate() {
        if *page_rejected {
            all_page_paragraphs[page_idx] = None;
        }
    }
    discard_ocr_elements_from_rejected_pages(&mut all_ocr_elements, &rejected_pages, page_index_offset);
    discard_rejected_ocr_page_payloads(
        &mut collected_tables,
        &mut accumulated_formulas,
        &rejected_pages,
        page_index_offset,
    );

    fill_unstructured_ocr_pages(&mut all_page_paragraphs, &page_texts);

    let (ocr_doc, raw_page_paragraphs) = {
        let has_structured = all_page_paragraphs
            .iter()
            .any(|paragraphs| paragraphs.as_ref().is_some_and(|paragraphs| !paragraphs.is_empty()));
        if has_structured {
            let pages: Vec<Vec<crate::pdf::structure::types::PdfParagraph>> = all_page_paragraphs
                .into_iter()
                .map(|opt| opt.unwrap_or_default())
                .collect();
            #[cfg(feature = "layout-detection")]
            let pages = {
                let mut pages = pages;
                crate::pdf::structure::adapters::promote_anchored_ordered_list_sequences(&mut pages);
                pages
            };
            // Document-global heading/list heuristic: `ocr_doc_to_paragraphs` /
            // `ocr_doc_to_layout_paragraphs` (above) build `pages` one OCR page at a
            // time and never call it, which is why every non-layout OCR route has
            // emitted zero headings and zero list items regardless of output format.
            // `None` (Plain output, or pages ML layout already classified) falls
            // through to the pre-existing assembly unchanged. Skipped entirely when
            // `skip_document_global_heuristic` is set -- see this function's own doc
            // comment: the caller wants the *bare* `pages` back to run this pass
            // itself, document-wide, and a heuristic run here first would either be
            // wasted (too few pages to cluster) or would mark `pages` "already
            // structured" and block that wider pass from doing anything.
            let doc = if skip_document_global_heuristic {
                Some(crate::pdf::structure::assemble_internal_document(
                    pages.clone(),
                    &collected_tables,
                    None,
                    &[],
                ))
            } else {
                match heuristically_restructured_ocr_pages(&pages, &ocr_page_heights, &collected_tables, config) {
                    Some(doc) => Some(doc),
                    None => {
                        // The document-global heading/list heuristic declined to run (Plain
                        // output, or a paragraph was already ML-layout-classified) or found
                        // nothing. Apply a text-marker-only list pass to the bare pages before
                        // falling back to plain assembly, so a non-layout OCR document that
                        // never reaches the heuristic still gets *some* list-item recovery
                        // (#713) -- this never touches `heading_level`, so it cannot regress
                        // heading detection the way pre-classifying before the heuristic call
                        // would (see `apply_ocr_text_list_fallback`'s own doc comment on the
                        // "already structured" gate this must not trip).
                        let mut fallback_pages = pages.clone();
                        for (page_idx, page) in fallback_pages.iter_mut().enumerate() {
                            apply_ocr_text_list_fallback(page);
                            // #729: recover a bare marker no ML hint ever classified --
                            // see `adapters::reattach_detached_ocr_list_markers`'s doc
                            // comment.
                            // #760: same rotation lookup as `assemble_ocr_page_paragraphs`'s
                            // call site above -- an explicit override wins, otherwise this
                            // page's own known PDF `/Rotate`, defaulting to `0`.
                            #[cfg(feature = "pdf")]
                            let page_rotation_degrees = if page_rotation_override != 0 {
                                page_rotation_override
                            } else {
                                lazy_pdf_render_state
                                    .as_ref()
                                    .and_then(|(_, _, rotations)| rotations.get(page_idx))
                                    .or_else(|| external_image_page_rotations.as_ref().and_then(|r| r.get(page_idx)))
                                    .copied()
                                    .unwrap_or(0)
                            };
                            #[cfg(not(feature = "pdf"))]
                            let page_rotation_degrees: u32 = 0;
                            crate::pdf::structure::adapters::reattach_detached_ocr_list_markers(
                                page,
                                page_rotation_degrees,
                            );
                        }
                        Some(crate::pdf::structure::assemble_internal_document(
                            fallback_pages,
                            &collected_tables,
                            None,
                            &[],
                        ))
                    }
                }
            };
            (doc, pages)
        } else {
            (None, Vec::new())
        }
    };

    #[cfg(feature = "pdf")]
    let ocr_doc = {
        let mut warnings = image_fallback_warnings;
        warnings.extend(recognition_noise_warnings);
        warnings.extend(page_failure_warnings);
        warnings.extend(margin_filter_warnings);
        attach_ocr_fallback_warnings(ocr_doc, &result, warnings)
    };
    // Without `pdf` there is no page renderer, so no page-level OCR runs and the vector is
    // always empty; bind it so the non-pdf build does not warn about an unused value.
    #[cfg(not(feature = "pdf"))]
    let _ = (recognition_noise_warnings, page_failure_warnings);

    Ok((
        result,
        mean_text_conf,
        collected_tables,
        all_ocr_elements,
        ocr_doc,
        accumulated_llm_usage,
        page_texts,
        if capture_rasters { Some(captured_rasters) } else { None },
        accumulated_formulas,
        raw_page_paragraphs,
        preprocessing_by_page,
        ocr_confidence_by_page,
        recognition_noise_verdicts,
    ))
}
/// Build an [`crate::types::ExtractedImage`] for a full-page OCR raster.
///
/// `image_index` is set to 0; the caller must reindex after merging into
/// the document's image collection.
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
pub(crate) fn build_page_raster_image(
    page_idx: usize,
    png_bytes: bytes::Bytes,
    width: u32,
    height: u32,
) -> crate::types::ExtractedImage {
    crate::types::ExtractedImage {
        data: png_bytes,
        format: std::borrow::Cow::Borrowed("png"),
        image_index: 0,
        page_number: Some((page_idx + 1) as u32),
        width: Some(width),
        height: Some(height),
        colorspace: Some("RGB".to_string()),
        bits_per_component: Some(8),
        is_mask: false,
        description: None,
        ocr_result: None,
        bounding_box: None,
        source_path: None,
        image_kind: Some(crate::types::ImageKind::PageRaster),
        kind_confidence: None,
        cluster_id: None,
        caption: None,
        qr_codes: None,
        data_base64: None,
    }
}
/// Adapt batch size to available system memory.
///
/// Estimates per-page memory cost based on typical page dimensions at 300 DPI
/// and compares against available system memory. Returns a batch size that
/// should keep peak memory within safe bounds.
///
/// Conservative estimate: each page in a batch needs approximately:
/// - ~50MB for render + encode working set (RGB buffer briefly, then PNG)
/// - ~100MB for OCR working set per concurrent page
/// - Plus the document itself and base allocations
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
pub(super) fn adapt_batch_size_to_memory(configured: usize, document_size: usize) -> usize {
    let available_bytes = get_available_memory();

    if available_bytes == 0 {
        return configured;
    }

    let reserved = document_size + 512 * 1024 * 1024;
    let usable = available_bytes.saturating_sub(reserved);

    const PER_PAGE_ESTIMATE: usize = 150 * 1024 * 1024;

    let memory_limited_batch = (usable / PER_PAGE_ESTIMATE).max(1);

    let result = configured.min(memory_limited_batch);

    tracing::debug!(
        available_mb = available_bytes / (1024 * 1024),
        usable_mb = usable / (1024 * 1024),
        document_mb = document_size / (1024 * 1024),
        memory_limited_batch,
        configured,
        result,
        "OCR batch size adaptation"
    );

    result
}
/// Query available system memory without external dependencies.
///
/// On Linux (including Docker), reads `/proc/meminfo` for `MemAvailable`.
/// On macOS, uses `sysctl hw.memsize` for total memory (conservative fallback).
/// Returns 0 if the query fails, signaling the caller to use the default batch size.
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
pub(super) fn get_available_memory() -> usize {
    #[cfg(target_os = "linux")]
    {
        let host = read_meminfo_available();
        host.min(cgroup_headroom().unwrap_or(usize::MAX))
    }
    #[cfg(target_os = "macos")]
    {
        use std::process::Command;
        if let Ok(output) = Command::new("sysctl").args(["-n", "hw.memsize"]).output()
            && let Ok(s) = std::str::from_utf8(&output.stdout)
            && let Ok(total) = s.trim().parse::<usize>()
        {
            return total / 2;
        }
        0
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        0
    }
}
#[cfg(all(any(feature = "ocr", feature = "ocr-pipeline"), target_os = "linux"))]
pub(super) fn parse_meminfo_available(contents: &str) -> usize {
    contents
        .lines()
        .find_map(|l| {
            l.strip_prefix("MemAvailable:")?
                .trim()
                .trim_end_matches("kB")
                .trim()
                .parse::<usize>()
                .ok()
        })
        .map(|kb| kb * 1024)
        .unwrap_or(0)
}
#[cfg(all(any(feature = "ocr", feature = "ocr-pipeline"), target_os = "linux"))]
pub(super) fn read_meminfo_available() -> usize {
    parse_meminfo_available(&std::fs::read_to_string("/proc/meminfo").unwrap_or_default())
}
#[cfg(all(any(feature = "ocr", feature = "ocr-pipeline"), target_os = "linux"))]
pub(super) fn parse_cgroup_v2(max: &str, current: &str) -> Option<usize> {
    let max = max.trim();
    if max == "max" {
        return None;
    }
    let limit = max.parse::<usize>().ok()?;
    let usage = current.trim().parse::<usize>().ok()?;
    Some(limit.saturating_sub(usage))
}
#[cfg(all(any(feature = "ocr", feature = "ocr-pipeline"), target_os = "linux"))]
pub(super) fn parse_cgroup_v1(limit: &str, usage: &str) -> Option<usize> {
    let limit = limit.trim().parse::<usize>().ok()?;
    let usage = usage.trim().parse::<usize>().ok()?;
    (limit < (isize::MAX as usize)).then(|| limit.saturating_sub(usage))
}
#[cfg(all(any(feature = "ocr", feature = "ocr-pipeline"), target_os = "linux"))]
pub(super) fn cgroup_headroom() -> Option<usize> {
    if let (Ok(max), Ok(cur)) = (
        std::fs::read_to_string("/sys/fs/cgroup/memory.max"),
        std::fs::read_to_string("/sys/fs/cgroup/memory.current"),
    ) {
        return parse_cgroup_v2(&max, &cur);
    }
    let limit = std::fs::read_to_string("/sys/fs/cgroup/memory/memory.limit_in_bytes").ok()?;
    let usage = std::fs::read_to_string("/sys/fs/cgroup/memory/memory.usage_in_bytes").ok()?;
    parse_cgroup_v1(&limit, &usage)
}
/// Minimum meaningful-word density (per 1,000 non-whitespace characters) a
/// [`OcrPipelineSelection::PreferLastNonEmpty`] candidate must clear -- when the incumbent it
/// would replace already clears it -- for the override to be accepted (F46).
///
/// Provenance: measured on a 25-document sample where the VLM stage of a `vlm_fallback`
/// pipeline was taken unconditionally whenever non-empty. Scored by absolute dictionary-valid
/// word count, unconditional acceptance gained +877 valid words over tesseract-only, but a
/// "keep whichever result is better" oracle would have gained +1,071 -- 3 of 25 documents were
/// actively damaged by the VLM stage. A *dictionary-based* valid-word-density floor of ~60
/// words per 1,000 characters recovered +1,027 of that +1,071.
///
/// This constant applies that floor to [`NativeTextStats::meaningful_words`] instead: a
/// dictionary lookup is unavailable to this pure, backend-free decision point, and
/// `meaningful_words` (alphanumeric tokens of at least
/// [`OcrQualityThresholds::min_meaningful_word_len`] characters) is the existing dictionary-free
/// stand-in the codebase already computes for quality scoring. It is a looser signal than a real
/// dictionary check -- a 4+ character garbled token still counts -- so this reuses F46's measured
/// value as a starting operating point rather than a reproduction of that measurement; it has not
/// been separately calibrated against the `meaningful_words` metric.
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
pub(super) const MIN_VLM_OVERRIDE_WORD_DENSITY_PER_1000_CHARS: f64 = 60.0;
/// Minimum whitespace-delimited token count before meaningful-word density is treated as a
/// judgement rather than noise.
///
/// Mirrors the sibling convention in [`NativeTextStats::compute`], where `fragmented_word_ratio`
/// and `consecutive_repeat_ratio` both abstain below a token floor: the question is "is there
/// enough on this page to judge at all". Density is a ratio, so it is blind to amount -- a bare
/// `"Page 12"` header scores ~167 per 1,000 and would otherwise read as a dense page worth
/// protecting, and a single extra or missing token swings a short page's density by tens of
/// points. ~keep
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
pub(super) const MIN_TOKENS_FOR_DENSITY_JUDGEMENT: usize = 10;
/// Meaningful-word density of `text`, per 1,000 non-whitespace characters, or `None` when the
/// text is too short to judge (see [`MIN_TOKENS_FOR_DENSITY_JUDGEMENT`]).
///
/// Shared scale for comparing a `PreferLastNonEmpty` candidate against the incumbent it would
/// replace (see [`MIN_VLM_OVERRIDE_WORD_DENSITY_PER_1000_CHARS`]). Scores the same normalized
/// input as [`compute_quality_score`](super::scoring::compute_quality_score) via
/// [`scoring_input`](super::scoring::scoring_input), so Markdown scaffolding cannot inflate the
/// denominator. Empty (post-whitespace-strip) text yields `None` rather than a
/// division-by-zero `NaN`.
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
pub(super) fn meaningful_word_density_per_1000_chars(text: &str, thresholds: &OcrQualityThresholds) -> Option<f64> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    let input = super::scoring::scoring_input(trimmed);
    let stats = NativeTextStats::compute(&input, thresholds);
    if stats.non_whitespace == 0 || stats.word_count < MIN_TOKENS_FOR_DENSITY_JUDGEMENT {
        return None;
    }
    Some(stats.meaningful_words as f64 / stats.non_whitespace as f64 * 1000.0)
}
/// Minimum share of a text's non-whitespace characters that must fall in a CJK ideographic
/// or kana block before `split_whitespace` token counts are treated as unreliable for it (see
/// [`is_non_space_delimited_script`]).
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
const MIN_CJK_CHAR_RATIO_FOR_SCRIPT_ABSTAIN: f64 = 0.3;
/// True for Chinese Han ideographs and Japanese kana -- scripts that do not delimit words
/// with whitespace. Deliberately excludes Hangul: modern Korean orthography is
/// space-delimited, so a Hangul page does not share the failure mode this guards against. ~keep
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
fn is_cjk_ideographic_or_kana(ch: char) -> bool {
    matches!(ch as u32,
        0x3400..=0x4DBF   // CJK Unified Ideographs Extension A
        | 0x4E00..=0x9FFF // CJK Unified Ideographs
        | 0xF900..=0xFAFF // CJK Compatibility Ideographs
        | 0x3040..=0x309F // Hiragana
        | 0x30A0..=0x30FF // Katakana
    )
}
/// True when `text` is dense enough in CJK ideographs/kana that `meaningful_word_density_per_1000_chars`'s
/// `split_whitespace` tokenization is not a meaningful word-count proxy for it: a whole
/// correct, dense paragraph in these scripts carries no internal spaces, so an OCR line break
/// is the only token boundary -- one line of genuinely dense, correct text collapses to a
/// single token while `non_whitespace` still counts every character, cratering density
/// independent of quality. This is a false-negative direction distinct from the ratio's
/// intended use (rejecting genuinely sparse/garbled Latin-script OCR output) that a
/// space-delimited-word assumption cannot see (F46 CJK false-negative). ~keep
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
fn is_non_space_delimited_script(text: &str) -> bool {
    let mut non_whitespace = 0usize;
    let mut cjk = 0usize;
    for ch in text.chars() {
        if ch.is_whitespace() {
            continue;
        }
        non_whitespace += 1;
        if is_cjk_ideographic_or_kana(ch) {
            cjk += 1;
        }
    }
    non_whitespace > 0 && (cjk as f64 / non_whitespace as f64) >= MIN_CJK_CHAR_RATIO_FOR_SCRIPT_ABSTAIN
}
/// Whether a non-empty `PreferLastNonEmpty` candidate is a materially worse replacement for a
/// non-empty incumbent, per [`MIN_VLM_OVERRIDE_WORD_DENSITY_PER_1000_CHARS`] (F46).
///
/// One-sided by construction: this only ever *blocks* a replacement, and only when the
/// incumbent was itself dense enough to be worth protecting. An incumbent that was already
/// below the floor has nothing worth protecting, so the later stage's non-empty override still
/// wins by default -- preserving #1341's intent that a later stage's result is a deliberate
/// override, not noise, and must not be pinned to a correctness-blind score comparison.
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
pub(super) fn candidate_is_materially_degraded(
    candidate_text: &str,
    incumbent_text: &str,
    thresholds: &OcrQualityThresholds,
) -> bool {
    // Word-count density cannot judge either side of this comparison when either text is
    // predominantly CJK: abstain entirely rather than let the space-delimited-word
    // assumption misjudge a script it was never built for (see
    // `is_non_space_delimited_script`). ~keep
    if is_non_space_delimited_script(incumbent_text) || is_non_space_delimited_script(candidate_text) {
        return false;
    }
    // An incumbent too short to judge has nothing established to protect, so the later stage's
    // override still wins -- same outcome as an incumbent measurably below the floor. ~keep
    let Some(incumbent_density) = meaningful_word_density_per_1000_chars(incumbent_text, thresholds) else {
        return false;
    };
    if incumbent_density < MIN_VLM_OVERRIDE_WORD_DENSITY_PER_1000_CHARS {
        return false;
    }
    // The incumbent is established dense text. A candidate too short to judge is not evidence
    // of a better result, and density alone cannot see that it is a fragment: `"Page 12"` scores
    // far above the floor. Keep the incumbent rather than trade it for an unjudgeable stub. ~keep
    let Some(candidate_density) = meaningful_word_density_per_1000_chars(candidate_text, thresholds) else {
        return true;
    };
    candidate_density < MIN_VLM_OVERRIDE_WORD_DENSITY_PER_1000_CHARS
}
/// Decide whether a pipeline stage's result should replace the current best-effort
/// candidate, given the pipeline's [`OcrPipelineSelection`](crate::core::config::OcrPipelineSelection) policy.
///
/// Only called once no stage has cleared `quality_thresholds.pipeline_min_quality` (the
/// accept-threshold early return in [`run_ocr_pipeline`] handles that case directly).
/// Pure and backend-free so the policy can be unit-tested without a registered OCR
/// backend.
///
/// - [`OcrPipelineSelection::HighestScore`]: replace only if `candidate_score` strictly
///   exceeds the current best score (or there is no current best). This is the original,
///   correctness-blind quality-max behavior.
/// - [`OcrPipelineSelection::PreferLastNonEmpty`]: replace whenever `candidate_text` is
///   non-empty, regardless of score, since a later stage in a fallback pipeline only ran
///   because the earlier stage(s) were judged inadequate. An empty candidate never
///   replaces an existing best, so a destroyed page still keeps the earlier text. The one
///   exception (F46) is a one-sided quality guard: a non-empty candidate that is materially
///   worse than a non-empty, already-dense incumbent (see
///   [`candidate_is_materially_degraded`]) does not replace it either. The later stage still
///   wins whenever it is not a clear regression -- this does not resurrect `HighestScore`'s
///   symmetric best-score comparison.
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
pub(super) fn should_replace_best_effort_result(
    selection: crate::core::config::OcrPipelineSelection,
    best_score: Option<f64>,
    best_text: Option<&str>,
    candidate_text: &str,
    candidate_score: f64,
    thresholds: &OcrQualityThresholds,
) -> bool {
    use crate::core::config::OcrPipelineSelection;

    match selection {
        OcrPipelineSelection::HighestScore => match best_score {
            Some(best) => candidate_score > best,
            None => true,
        },
        OcrPipelineSelection::PreferLastNonEmpty => {
            if candidate_text.trim().is_empty() {
                return best_score.is_none();
            }
            match best_text.map(str::trim).filter(|text| !text.is_empty()) {
                Some(incumbent_text) => !candidate_is_materially_degraded(candidate_text, incumbent_text, thresholds),
                None => true,
            }
        }
    }
}
/// Attach skipped and failed stage diagnostics to the result that survives the pipeline.
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
pub(super) fn attach_ocr_pipeline_stage_warnings(
    mut doc: Option<crate::types::internal::InternalDocument>,
    text: &str,
    unavailable_backends: &[String],
    stage_failures: &[(String, String)],
) -> Option<crate::types::internal::InternalDocument> {
    if unavailable_backends.is_empty() && stage_failures.is_empty() {
        return doc;
    }

    let retained_doc = doc.get_or_insert_with(|| {
        let mut doc = crate::types::internal::InternalDocument::new("pdf");
        // Backend text verbatim (see `flat_ocr_page_document`): normalize before splitting.
        let text = crate::extraction::transform::normalize_line_endings(text);
        for paragraph in text.split("\n\n").map(str::trim).filter(|text| !text.is_empty()) {
            doc.push_element(crate::types::internal::InternalElement::text(
                crate::types::internal::ElementKind::Paragraph,
                paragraph,
                0,
            ));
        }
        doc
    });

    for backend in unavailable_backends {
        retained_doc.processing_warnings.push(crate::types::ProcessingWarning {
            source: std::borrow::Cow::Borrowed("ocr_pipeline"),
            message: std::borrow::Cow::Owned(format!(
                "Requested OCR pipeline backend '{backend}' is unavailable and was skipped."
            )),
        });
    }
    for (backend, error) in stage_failures {
        retained_doc.processing_warnings.push(crate::types::ProcessingWarning {
            source: std::borrow::Cow::Borrowed("ocr_pipeline"),
            message: std::borrow::Cow::Owned(format!(
                "OCR fallback backend '{backend}' failed and was skipped: {error}"
            )),
        });
    }

    doc
}
/// Attach force_ocr image-XObject fallback warnings (#1355) to the OCR-produced
/// document, mirroring [`attach_ocr_pipeline_stage_warnings`]'s `get_or_insert_with`
/// shape so the warning always survives even when no structured document was built.
//
// `ocr-pipeline` (not just `ocr`): the caller is inside `extract_with_ocr`
// (`any(ocr, ocr-pipeline)`), and the `binstall` CLI profile enables `ocr-pipeline`
// via `liter-llm` without `ocr`. ~keep
#[cfg(all(feature = "pdf", any(feature = "ocr", feature = "ocr-pipeline")))]
pub(super) fn attach_ocr_fallback_warnings(
    mut doc: Option<crate::types::internal::InternalDocument>,
    text: &str,
    warnings: Vec<crate::types::ProcessingWarning>,
) -> Option<crate::types::internal::InternalDocument> {
    if warnings.is_empty() {
        return doc;
    }

    let retained_doc = doc.get_or_insert_with(|| {
        let mut doc = crate::types::internal::InternalDocument::new("pdf");
        // Backend text verbatim (see `flat_ocr_page_document`): normalize before splitting.
        let text = crate::extraction::transform::normalize_line_endings(text);
        for paragraph in text.split("\n\n").map(str::trim).filter(|text| !text.is_empty()) {
            doc.push_element(crate::types::internal::InternalElement::text(
                crate::types::internal::ElementKind::Paragraph,
                paragraph,
                0,
            ));
        }
        doc
    });

    retained_doc.processing_warnings.extend(warnings);

    doc
}
/// Last-resort image-XObject recovery for the OCR *pipeline* route, used when every stage
/// failed outright (#1444).
///
/// The per-page fallback inside [`extract_with_ocr_for_page`] covers a stage that ran and
/// came back blank. It cannot cover a stage that never produced a result at all — a VLM that
/// errored on every page, an unreachable service — because [`run_ocr_pipeline_for_page`]
/// catches that `Err` and moves on, and once the last stage is exhausted the pipeline used to
/// error out having never tried the pages' embedded images.
///
/// Uses the highest-priority *available* stage's backend, which is the one the pipeline
/// itself would have preferred. Returns `None` when there is no PDF content, no usable
/// backend, or no page yielded any recoverable payload — the caller must then still report the failure.
#[cfg(all(feature = "pdf", any(feature = "ocr", feature = "ocr-pipeline")))]
pub(super) struct PipelineXObjectRecoveryOutcome {
    pub(super) text: String,
    pub(super) page_texts: Vec<String>,
    pub(super) images: Vec<crate::types::ExtractedImage>,
    pub(super) warnings: Vec<crate::types::ProcessingWarning>,
    pub(super) llm_usage: Vec<crate::types::LlmUsage>,
    pub(super) tables: Vec<crate::types::Table>,
    pub(super) formulas: Vec<crate::types::Formula>,
    pub(super) preprocessing: ahash::AHashMap<u32, crate::types::ImagePreprocessingMetadata>,
}
#[cfg(all(feature = "pdf", any(feature = "ocr", feature = "ocr-pipeline")))]
impl PipelineXObjectRecoveryOutcome {
    fn new(page_count: usize) -> Self {
        Self {
            text: String::new(),
            page_texts: vec![String::new(); page_count],
            images: Vec::new(),
            warnings: Vec::new(),
            llm_usage: Vec::new(),
            tables: Vec::new(),
            formulas: Vec::new(),
            preprocessing: ahash::AHashMap::new(),
        }
    }

    fn has_content(&self) -> bool {
        self.page_texts.iter().any(|text| !text.trim().is_empty())
            || !self.tables.is_empty()
            || !self.formulas.is_empty()
    }
}
#[cfg(all(feature = "pdf", any(feature = "ocr", feature = "ocr-pipeline")))]
pub(super) async fn collect_pipeline_xobject_pages(
    backend: &std::sync::Arc<dyn crate::plugins::OcrBackend>,
    doc: &xberg_native_pdf::PdfDocument,
    page_count: usize,
    ocr_config: &crate::core::config::OcrConfig,
    budget: &mut crate::extractors::security::SecurityBudget,
) -> crate::Result<PipelineXObjectRecoveryOutcome> {
    let mut outcome = PipelineXObjectRecoveryOutcome::new(page_count);
    for page_idx in 0..page_count {
        let Some(mut recovery) =
            recover_page_text_from_image_xobjects(backend, doc, page_idx, ocr_config, budget).await?
        else {
            continue;
        };
        let recovered_payload =
            !recovery.text.is_empty() || !recovery.tables.is_empty() || !recovery.formulas.is_empty();
        outcome.page_texts[page_idx] = std::mem::take(&mut recovery.text);
        if recovered_payload {
            outcome
                .warnings
                .push(xobject_fallback_warning(page_idx, recovery.attempted));
        }
        outcome.llm_usage.append(&mut recovery.llm_usage);
        outcome.tables.append(&mut recovery.tables);
        outcome.formulas.append(&mut recovery.formulas);
        if let Some(metadata) = recovery.image_preprocessing {
            outcome.preprocessing.insert((page_idx + 1) as u32, metadata);
        }
        outcome.images.append(&mut recovery.images);
    }
    Ok(outcome)
}
#[cfg(all(feature = "pdf", any(feature = "ocr", feature = "ocr-pipeline")))]
pub(super) fn join_pipeline_xobject_text(
    page_texts: &[String],
    config: &ExtractionConfig,
    budget: &mut crate::extractors::security::SecurityBudget,
) -> crate::Result<String> {
    let page_marker_cfg = config.pages.as_ref().filter(|pages| pages.insert_page_markers);
    let mut text = String::new();
    for (page_idx, page_text) in page_texts.iter().enumerate() {
        if let Some(cfg) = page_marker_cfg {
            let marker = cfg.marker_format.replace("{page_num}", &(page_idx + 1).to_string());
            budget.account_text(marker.len())?;
            text.push_str(&marker);
        } else if page_idx > 0 {
            budget.account_text(2)?;
            text.push_str("\n\n");
        }
        text.push_str(page_text);
    }
    Ok(text)
}
#[cfg(all(feature = "pdf", any(feature = "ocr", feature = "ocr-pipeline")))]
pub(super) async fn recover_pipeline_document_from_image_xobjects(
    content: Option<&[u8]>,
    config: &ExtractionConfig,
    ocr_config: &crate::core::config::OcrConfig,
    backend_name: &str,
) -> crate::Result<Option<PipelineXObjectRecoveryOutcome>> {
    let Some(content) = content else {
        return Ok(None);
    };
    let backend = {
        let registry = crate::plugins::registry::get_ocr_backend_registry();
        let registry = registry.read();
        let Ok(backend) = registry.get(backend_name) else {
            return Ok(None);
        };
        backend
    };
    let Ok((doc, page_count, _)) = open_pdf_for_full_ocr(content) else {
        return Ok(None);
    };

    let mut budget = crate::extractors::security::SecurityBudget::from_config(config);
    let mut outcome = collect_pipeline_xobject_pages(&backend, &doc, page_count, ocr_config, &mut budget).await?;
    if !outcome.has_content() {
        return Ok(None);
    }
    outcome.text = join_pipeline_xobject_text(&outcome.page_texts, config, &mut budget)?;
    Ok(Some(outcome))
}
/// Run a multi-backend OCR pipeline with quality-based fallback.
///
/// Images and layout detections are computed once and shared across all stages.
/// Each stage produces OCR output that is scored; if the score meets the
/// pipeline's quality threshold, the result is accepted. Otherwise, the next
/// backend is tried. If no stage clears the threshold, `pipeline.selection`
/// decides which stage's result is returned as the best effort: the
/// highest-scoring one ([`OcrPipelineSelection::HighestScore`], the default, used
/// for explicit and classical auto-fallback pipelines), or the last stage that
/// produced non-empty text ([`OcrPipelineSelection::PreferLastNonEmpty`], used by
/// `vlm_fallback`-synthesised pipelines -- see
/// [`should_replace_best_effort_result`]).
///
/// Thin, signature-preserving wrapper over [`run_ocr_pipeline_for_page`] with no page
/// rotation (`0`), so every pre-existing caller keeps today's behavior -- each stage's own
/// [`extract_with_ocr`] call falls back to its content-based per-page auto-detection.
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
pub(crate) async fn run_ocr_pipeline(
    content: Option<&[u8]>,
    images: Option<&[image::DynamicImage]>,
    #[cfg(feature = "layout-detection")] layout_detections: Option<&[crate::layout::DetectionResult]>,
    config: &ExtractionConfig,
    pipeline: &crate::core::config::OcrPipelineConfig,
    path: Option<&std::path::Path>,
) -> crate::Result<(
    String,
    Vec<crate::types::Table>,
    Vec<crate::types::OcrElement>,
    Option<crate::types::internal::InternalDocument>,
    Vec<crate::types::LlmUsage>,
    Vec<String>,
    Option<Vec<crate::types::ExtractedImage>>,
    Vec<crate::types::Formula>,
    ahash::AHashMap<u32, crate::types::ImagePreprocessingMetadata>,
    ahash::AHashMap<u32, crate::types::page::PageOcrConfidence>,
)> {
    let (
        text,
        tables,
        elements,
        doc,
        usage,
        page_texts,
        rasters,
        formulas,
        _raw_page_paragraphs,
        preprocessing,
        ocr_confidence,
    ) = Box::pin(run_ocr_pipeline_for_page(
        content,
        images,
        #[cfg(feature = "layout-detection")]
        layout_detections,
        config,
        pipeline,
        path,
        0,
        false,
        None,
        0,
    ))
    .await?;
    Ok((
        text,
        tables,
        elements,
        doc,
        usage,
        page_texts,
        rasters,
        formulas,
        preprocessing,
        ocr_confidence,
    ))
}
/// Same as [`run_ocr_pipeline`], but `page_rotation_degrees` -- the page's known PDF
/// `/Rotate` value, `0` if unrotated or genuinely unknown to the caller -- is forwarded to
/// every stage's [`extract_with_ocr_for_page`] call as its `page_rotation_override`. Each
/// stage backend still resolves its own [`crate::plugins::PageOrientationHandling`], so a
/// `RequiresUpright` stage gets an upright raster via `upright_raster_for_backend` while a
/// `SelfCorrecting` or `RecognisesRotatedText` stage's raster is left untouched (or only gets
/// the `ocr_config_with_page_rotation_hint` block-order hint) -- see `extract_with_ocr_for_page`.
///
/// Needed by a caller that drives a single page's image through the pipeline detached from
/// the rest of its document (currently only `extract_mixed_ocr_native`'s per-page pipeline
/// route, see #651): that caller already knows the page's own `/Rotate` value, but the
/// stage's own content-based auto-detection cannot recover it from a lone, index-0 image.
///
/// `skip_document_global_heuristic` is forwarded verbatim to every stage's
/// [`extract_with_ocr_for_page`] call, and this function's own extra return element -- the
/// winning stage's bare, unclassified per-page paragraphs -- is that stage's own extra
/// element, so callers get back exactly the material a stage never got to structure itself.
/// `points_per_pixel_override` is likewise forwarded verbatim to every stage's
/// [`extract_with_ocr_for_page`] call. See that function's doc comments for why both exist.
/// `page_index_offset` carries the detached image's original zero-based document page index
/// through each stage so warnings, traces, and structured payloads retain their real page.
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
// Same rationale as `extract_with_ocr_for_page`, whose full parameter set this function
// forwards to every pipeline stage verbatim (see doc comment above).
#[allow(clippy::too_many_arguments)]
pub(super) async fn run_ocr_pipeline_for_page(
    content: Option<&[u8]>,
    images: Option<&[image::DynamicImage]>,
    #[cfg(feature = "layout-detection")] layout_detections: Option<&[crate::layout::DetectionResult]>,
    config: &ExtractionConfig,
    pipeline: &crate::core::config::OcrPipelineConfig,
    path: Option<&std::path::Path>,
    page_rotation_degrees: u32,
    skip_document_global_heuristic: bool,
    points_per_pixel_override: Option<f32>,
    page_index_offset: usize,
) -> crate::Result<(
    String,
    Vec<crate::types::Table>,
    Vec<crate::types::OcrElement>,
    Option<crate::types::internal::InternalDocument>,
    Vec<crate::types::LlmUsage>,
    Vec<String>,
    Option<Vec<crate::types::ExtractedImage>>,
    Vec<crate::types::Formula>,
    Vec<Vec<crate::pdf::structure::types::PdfParagraph>>,
    ahash::AHashMap<u32, crate::types::ImagePreprocessingMetadata>,
    ahash::AHashMap<u32, crate::types::page::PageOcrConfidence>,
)> {
    use crate::plugins::registry::get_ocr_backend_registry;

    // Re-seed the built-in backends before deciding which pipeline stages are available. A
    // prior `clear_ocr_backends()` call in the same process (e.g. a binding e2e suite's
    // backend-management test running before this one) otherwise leaves the registry
    // empty, so every stage below reads as unavailable and the pipeline fails outright
    // instead of falling back to (or re-using) the built-in defaults. ~keep
    crate::plugins::ensure_ocr_backends_initialized();

    let default_ocr_config = crate::core::config::OcrConfig::default();
    let ocr_config = config.ocr.as_ref().unwrap_or(&default_ocr_config);

    // Best-effort selection policy is derived from the config that produced this pipeline
    // (a `vlm_fallback`-synthesised pipeline prefers its last non-empty stage; explicit and
    // classical pipelines stay score-based) rather than carried on `OcrPipelineConfig`, so
    // the binding-facing config stays unchanged (#1341).
    let selection = ocr_config.pipeline_selection();

    let mut stages = pipeline.stages.clone();
    stages.sort_by_key(|b| std::cmp::Reverse(b.priority));

    let requested_backends: Vec<String> = stages.iter().map(|s| s.backend.clone()).collect();
    let (available_stages, unavailable_backends): (Vec<_>, Vec<_>) = {
        let registry = get_ocr_backend_registry();
        let registry = registry.read();
        stages
            .into_iter()
            .partition(|stage| registry.get(&stage.backend).is_ok())
    };
    let unavailable_backends = unavailable_backends
        .into_iter()
        .map(|stage| stage.backend)
        .collect::<Vec<_>>();

    if available_stages.is_empty() {
        return Err(crate::XbergError::Parsing {
            message: format!(
                "No available OCR backends for pipeline (requested: {})",
                requested_backends.join(", ")
            ),
            source: None,
        });
    }

    #[allow(clippy::type_complexity)]
    let mut best_result: Option<(
        String,
        f64,
        Vec<crate::types::Table>,
        Vec<crate::types::OcrElement>,
        Option<crate::types::internal::InternalDocument>,
        Vec<String>,
        Option<Vec<crate::types::ExtractedImage>>,
        Vec<crate::types::Formula>,
        Vec<Vec<crate::pdf::structure::types::PdfParagraph>>,
        ahash::AHashMap<u32, crate::types::ImagePreprocessingMetadata>,
        ahash::AHashMap<u32, crate::types::page::PageOcrConfidence>,
    )> = None;

    let mut accumulated_usage: Vec<crate::types::LlmUsage> = Vec::new();
    // Track stages that errored outright (e.g. a VLM fallback that failed
    // authentication) so the failure is surfaced to the caller instead of being
    // silently replaced by a lower-quality earlier result (issue #1339).
    let mut stage_failures: Vec<(String, String)> = Vec::new();

    for stage in &available_stages {
        let mut stage_ocr = ocr_config.clone();
        stage_ocr.backend = stage.backend.clone();
        if let Some(ref lang) = stage.language {
            stage_ocr.language = lang.clone();
        }
        if let Some(ref tc) = stage.tesseract_config {
            stage_ocr.tesseract_config = Some(tc.clone());
        }
        if let Some(ref pc) = stage.paddle_ocr_config {
            stage_ocr.paddle_ocr_config = Some(pc.clone());
        }
        stage_ocr.quality_thresholds = Some(pipeline.quality_thresholds.clone());
        stage_ocr.vlm_config = stage.vlm_config.clone();
        stage_ocr.backend_options = stage.backend_options.clone();

        let stage_config = ExtractionConfig {
            ocr: Some(stage_ocr),
            ..config.clone()
        };

        tracing::debug!(
            backend = %stage.backend,
            priority = stage.priority,
            "Pipeline: trying OCR backend"
        );

        let result = Box::pin(extract_with_ocr_for_page(
            content,
            images,
            #[cfg(feature = "layout-detection")]
            layout_detections,
            &stage_config,
            path,
            page_rotation_degrees,
            skip_document_global_heuristic,
            points_per_pixel_override,
            page_index_offset,
        ))
        .await;

        match result {
            Ok((
                text,
                mean_conf,
                stage_tables,
                stage_ocr_elements,
                stage_doc,
                stage_llm_usage,
                stage_page_texts,
                stage_rasters,
                stage_formulas,
                stage_raw_paragraphs,
                stage_preprocessing,
                stage_ocr_confidence,
                stage_recognition_noise_verdicts,
            )) => {
                let text_score = compute_quality_score(&text, &pipeline.quality_thresholds);
                let score = pipeline_stage_score(text_score, mean_conf);

                tracing::debug!(
                    backend = %stage.backend,
                    score,
                    text_score,
                    mean_text_conf = ?mean_conf,
                    threshold = pipeline.quality_thresholds.pipeline_min_quality,
                    "Pipeline: backend produced result"
                );

                // Plumbed to the accept decision for observability only (see
                // `OcrPageNoiseVerdict`) -- the accept/reject outcome below must not change
                // until a threshold is calibrated against real corpus data. ~keep
                for verdict in &stage_recognition_noise_verdicts {
                    tracing::debug!(
                        backend = %stage.backend,
                        page = verdict.page_index + 1,
                        fragmented_word_ratio = verdict.fragmented_word_ratio,
                        word_count = verdict.word_count,
                        mean_confidence = verdict.mean_confidence,
                        low_confidence = verdict.low_confidence,
                        fragmented_noise = verdict.fragmented_noise,
                        dictionary_noise = verdict.dictionary_noise,
                        dict_invalid_word_ratio = verdict.dict_invalid_word_ratio,
                        discarded = verdict.discarded,
                        "Pipeline: OCR recognition-noise verdict in scope at accept decision"
                    );
                }

                accumulated_usage.extend(stage_llm_usage);

                if score >= pipeline.quality_thresholds.pipeline_min_quality {
                    // ~keep Attach prior-stage diagnostics before this accepted-stage early
                    // return; otherwise successful fallback silently erases why it ran.
                    let stage_doc =
                        attach_ocr_pipeline_stage_warnings(stage_doc, &text, &unavailable_backends, &stage_failures);
                    return Ok((
                        text,
                        stage_tables,
                        stage_ocr_elements,
                        stage_doc,
                        accumulated_usage,
                        stage_page_texts,
                        stage_rasters,
                        stage_formulas,
                        stage_raw_paragraphs,
                        stage_preprocessing,
                        stage_ocr_confidence,
                    ));
                }

                // Selection policy decides which stage's result to keep once no stage has
                // cleared the accept threshold (see `should_replace_best_effort_result`).
                // `HighestScore` (explicit / classical auto-fallback pipelines) keeps the
                // original strict quality-max behavior. `PreferLastNonEmpty`
                // (`vlm_fallback`-synthesised pipelines) prefers the deepest non-empty
                // fallback instead: stages run in priority order (primary first), so a
                // later non-empty result was invoked precisely because the higher-priority
                // stages were inadequate, and a correctness-blind score-max heuristic can
                // otherwise pin selection to an inadequate primary (e.g. merged-word
                // tesseract text scoring above a correct VLM transcription), discarding the
                // very fallback the pipeline ran (#1341). An empty fallback never
                // overwrites, so the earlier text is still kept in that case -- nor does a
                // non-empty one that is a materially worse replacement for an already-dense
                // incumbent (F46).
                let best_score = best_result.as_ref().map(|(_, best_score, ..)| *best_score);
                let best_text = best_result.as_ref().map(|(text, ..)| text.as_str());
                if should_replace_best_effort_result(
                    selection,
                    best_score,
                    best_text,
                    &text,
                    score,
                    &pipeline.quality_thresholds,
                ) {
                    best_result = Some((
                        text,
                        score,
                        stage_tables,
                        stage_ocr_elements,
                        stage_doc,
                        stage_page_texts,
                        stage_rasters,
                        stage_formulas,
                        stage_raw_paragraphs,
                        stage_preprocessing,
                        stage_ocr_confidence,
                    ));
                }
            }
            Err(e) => {
                tracing::warn!(
                    backend = %stage.backend,
                    error = %e,
                    "Pipeline: backend failed, trying next"
                );
                stage_failures.push((stage.backend.clone(), e.to_string()));
            }
        }
    }

    match best_result {
        Some((
            text,
            score,
            tables,
            elements,
            doc,
            page_texts,
            rasters,
            formulas,
            raw_page_paragraphs,
            preprocessing,
            ocr_confidence,
        )) => {
            let threshold = pipeline.quality_thresholds.pipeline_min_quality;
            tracing::warn!(
                score,
                threshold,
                selection = ?selection,
                "All OCR pipeline backends produced suboptimal quality, using best-effort result \
                 selected per the pipeline's selection policy"
            );
            let mut doc = doc.unwrap_or_else(|| {
                let mut d = crate::types::internal::InternalDocument::new("pdf");
                // Backend text verbatim (see `flat_ocr_page_document`). This best-effort arm
                // is where `PreferLastNonEmpty` lands VLM output, the most likely CR source.
                let text = crate::extraction::transform::normalize_line_endings(&text);
                for paragraph in text.split("\n\n") {
                    let trimmed = paragraph.trim();
                    if !trimmed.is_empty() {
                        d.push_element(crate::types::internal::InternalElement::text(
                            crate::types::internal::ElementKind::Paragraph,
                            trimmed,
                            0,
                        ));
                    }
                }
                d
            });
            doc.processing_warnings.push(crate::types::ProcessingWarning {
                source: std::borrow::Cow::Borrowed("ocr_pipeline"),
                message: std::borrow::Cow::Owned(format!(
                    "All OCR pipeline backends scored below the configured quality threshold \
                     (best score {score:.3} < {threshold:.3}); returning the best-effort result \
                     chosen by the pipeline's {:?} selection policy, which may be inaccurate or \
                     incomplete.",
                    selection
                )),
            });
            let doc = attach_ocr_pipeline_stage_warnings(Some(doc), &text, &unavailable_backends, &stage_failures);
            Ok((
                text,
                tables,
                elements,
                doc,
                accumulated_usage,
                page_texts,
                rasters,
                formulas,
                raw_page_paragraphs,
                preprocessing,
                ocr_confidence,
            ))
        }
        None => {
            // #1444: every stage errored, so no stage ever reached its own per-page
            // image-XObject fallback. Try the pages' embedded images once here before
            // giving up -- for a scanned PDF whose rasterizer output is blank, those
            // images are the only place the text ever was.
            #[cfg(feature = "pdf")]
            if let Some(first_stage) = available_stages.first()
                && let Some(mut recovery) = Box::pin(recover_pipeline_document_from_image_xobjects(
                    content,
                    config,
                    ocr_config,
                    &first_stage.backend,
                ))
                .await?
            {
                accumulated_usage.append(&mut recovery.llm_usage);
                let doc =
                    attach_ocr_pipeline_stage_warnings(None, &recovery.text, &unavailable_backends, &stage_failures);
                let doc = attach_ocr_fallback_warnings(doc, &recovery.text, recovery.warnings);
                let capture_rasters = config.images.as_ref().is_some_and(|c| c.include_page_rasters);
                return Ok((
                    recovery.text,
                    recovery.tables,
                    Vec::new(),
                    doc,
                    accumulated_usage,
                    recovery.page_texts,
                    if capture_rasters { Some(recovery.images) } else { None },
                    recovery.formulas,
                    Vec::new(),
                    recovery.preprocessing,
                    ahash::AHashMap::new(),
                ));
            }

            let detail = if stage_failures.is_empty() {
                String::new()
            } else {
                let causes = stage_failures
                    .iter()
                    .map(|(backend, error)| format!("{backend}: {error}"))
                    .collect::<Vec<_>>()
                    .join("; ");
                format!(" ({causes})")
            };
            Err(crate::XbergError::Parsing {
                message: format!("All OCR pipeline backends failed{detail}"),
                source: None,
            })
        }
    }
}
/// Clone an OCR config with word-level elements forced on for structure consumers.
///
/// Table recognition requires word geometry while semantic paragraph assembly
/// consumes the backend's line-only internal document even without ML layout.
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
pub(super) fn ensure_elements_enabled(
    config: &crate::core::config::ocr::OcrConfig,
) -> crate::core::config::ocr::OcrConfig {
    let mut config = config.clone();
    match config.element_config.as_mut() {
        Some(ec) => {
            ec.include_elements = true;
            ec.min_level = crate::types::OcrElementLevel::Word;
        }
        None => {
            config.element_config = Some(crate::types::OcrElementConfig {
                include_elements: true,
                min_level: crate::types::OcrElementLevel::Word,
                ..Default::default()
            });
        }
    }
    config
}
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
pub(super) fn filter_public_ocr_elements(
    elements: &[crate::types::OcrElement],
    config: &crate::core::config::ocr::OcrConfig,
) -> Vec<crate::types::OcrElement> {
    let Some(element_config) = config.element_config.as_ref() else {
        return Vec::new();
    };
    element_config.select_elements(elements)
}
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
pub(super) fn discard_ocr_elements_from_rejected_pages(
    elements: &mut Vec<crate::types::OcrElement>,
    rejected_pages: &[bool],
    page_index_offset: usize,
) {
    elements.retain(|element| !ocr_page_is_rejected(element.page_number, rejected_pages, page_index_offset));
}
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
pub(super) fn ocr_page_is_rejected(page_number: u32, rejected_pages: &[bool], page_index_offset: usize) -> bool {
    page_number
        .checked_sub(1)
        .and_then(|index| usize::try_from(index).ok())
        .and_then(|index| index.checked_sub(page_index_offset))
        .and_then(|local_index| rejected_pages.get(local_index))
        .copied()
        .unwrap_or(false)
}
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
pub(super) fn discard_rejected_ocr_page_payloads(
    tables: &mut Vec<crate::types::Table>,
    formulas: &mut Vec<crate::types::Formula>,
    rejected_pages: &[bool],
    page_index_offset: usize,
) {
    tables.retain(|table| !ocr_page_is_rejected(table.page_number, rejected_pages, page_index_offset));
    formulas.retain(|formula| {
        formula
            .page
            .is_none_or(|page_number| !ocr_page_is_rejected(page_number, rejected_pages, page_index_offset))
    });
}
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
pub(super) fn retain_ocr_formulas_for_accepted_pages(
    formulas: &mut Vec<crate::types::Formula>,
    accepted_pages: &ahash::AHashMap<u32, String>,
) {
    formulas.retain(|formula| {
        formula
            .page
            .is_none_or(|page_number| accepted_pages.contains_key(&page_number))
    });
}
/// Inject a page's PDF `/Rotate` value into `OcrConfig.backend_options` so a
/// backend that can use it (currently PaddleOCR, see
/// `paddle_ocr::backend::PaddleOcrBackend::reorder_blocks_for_page_rotation`) can
/// correct its detector's raster-space block order into true reading order
/// (#640). `normalize_rendered_page_for_ocr` deliberately hands every backend a
/// raster in the page's raw MediaBox orientation rather than its display
/// orientation (see the #530 regression test in `crate::pdf::render`), which
/// Tesseract's own layout analysis already reads correctly; PaddleOCR's detector
/// does not, so this hint is the minimal, backend-local fix rather than changing
/// the shared raster every backend (including Tesseract) receives.
///
/// Also injects `source_dpi`: the true resolution of the raster accompanying this call, which
/// only this route can know because it rendered the page. Without it the OCR preprocessor
/// assumes 72 DPI (`image::preprocessing::normalize_image_dpi_owned`), which is wrong for every
/// page here — they are rendered at 150, or lower still when `choose_safe_dpi` reduced an
/// oversized page — and that mistake both inflates the resize past the `max_image_dimension`
/// clamp and hands Tesseract a `scan_res` unrelated to the raster it is looking at.
///
/// Both hints are per page, not per document: the config is cloned for each page, so mixed page
/// sizes and per-page DPI reductions each report their own value.
///
/// A no-op when there is nothing to say (`page_rotation_degrees == 0` and no known DPI) so such
/// pages never pay a config clone. Backends that don't recognise either key ignore it, per
/// `OcrConfig.backend_options`'s documented contract.
#[cfg(all(any(feature = "ocr", feature = "ocr-pipeline"), feature = "pdf"))]
pub(super) fn ocr_config_with_page_rotation_hint(
    config: &crate::core::config::ocr::OcrConfig,
    page_rotation_degrees: u32,
    source_dpi: Option<f64>,
) -> Cow<'_, crate::core::config::ocr::OcrConfig> {
    let source_dpi = source_dpi.and_then(serde_json::Number::from_f64);
    if page_rotation_degrees == 0 && source_dpi.is_none() {
        return Cow::Borrowed(config);
    }
    let mut config = config.clone();
    let mut opts = config.backend_options.take().unwrap_or_else(|| serde_json::json!({}));
    if !opts.is_object() {
        opts = serde_json::json!({});
    }
    if let Some(obj) = opts.as_object_mut() {
        if page_rotation_degrees != 0 {
            obj.insert(
                "page_rotation_degrees".to_string(),
                serde_json::Value::Number(page_rotation_degrees.into()),
            );
        }
        if let Some(source_dpi) = source_dpi {
            obj.insert(
                crate::core::config::ocr::SOURCE_DPI_BACKEND_OPTION.to_string(),
                serde_json::Value::Number(source_dpi),
            );
        }
    }
    config.backend_options = Some(opts);
    Cow::Owned(config)
}
/// Derive the [`ocr_config_with_page_rotation_hint`] `source_dpi` value for one rendered page.
///
/// `rendered_width_px` must be the width of the MediaBox-oriented raster as
/// `normalize_rendered_page_for_ocr` produced it, before any `upright_raster_for_backend`
/// rotation — see `crate::pdf::render::rendered_page_dpi`.
#[cfg(all(any(feature = "ocr", feature = "ocr-pipeline"), feature = "pdf"))]
pub(super) fn rendered_page_source_dpi(
    doc: &xberg_native_pdf::PdfDocument,
    page_index: usize,
    rendered_width_px: u32,
) -> Option<f64> {
    let (page_width_pt, _) = page_dimensions_pt(doc, page_index);
    crate::pdf::render::rendered_page_dpi(rendered_width_px, page_width_pt)
}
/// Rotate a page raster upright before handing it to a backend that cannot cope with a
/// sideways page (`PageOrientationHandling::RequiresUpright`), returning the bytes/dimensions
/// actually sent to the backend and the rotation (in `image`-crate terms: one of `0`, `90`,
/// `180`, `270`) that must later be undone on any pixel-space geometry the backend returns.
///
/// `normalize_rendered_page_for_ocr` deliberately hands every backend a raster in the page's
/// raw MediaBox orientation rather than its display orientation (#530), which Tesseract
/// (`SelfCorrecting`) reads fine and PaddleOCR (`RecognisesRotatedText`) recognises correctly
/// (only its block order needs fixing, see `ocr_config_with_page_rotation_hint`). A backend
/// that declares `RequiresUpright` (currently sceptre) produces character garbage on that same
/// sideways raster instead — measured this session on `/Rotate 270` scanned pages, and
/// confirmed by feeding the same page through an upright render. This is the backend-local
/// fix: re-apply the page's original `/Rotate` value on top of the already-corrected raster,
/// undoing exactly what `normalize_rendered_page_for_ocr` undid, so only that backend's input
/// changes (#643).
///
/// A no-op — returns `data` unchanged and a correction of `0` — when `page_rotation_degrees ==
/// 0` or `orientation_handling` is not `RequiresUpright`. In particular, `SelfCorrecting` and
/// `RecognisesRotatedText` backends never pay a re-encode and never see a different raster than
/// they do today: Tesseract's input is untouched by this function under every input.
#[cfg(all(any(feature = "ocr", feature = "ocr-pipeline"), feature = "pdf"))]
pub(super) fn upright_raster_for_backend(
    data: &std::sync::Arc<Vec<u8>>,
    width: u32,
    height: u32,
    page_rotation_degrees: u32,
    orientation_handling: crate::plugins::PageOrientationHandling,
    security_limits: Option<&crate::extractors::security::SecurityLimits>,
) -> crate::Result<(std::sync::Arc<Vec<u8>>, u32, u32, u32)> {
    if page_rotation_degrees == 0 || orientation_handling != crate::plugins::PageOrientationHandling::RequiresUpright {
        return Ok((std::sync::Arc::clone(data), width, height, 0));
    }
    let correction_degrees = page_rotation_degrees % 360;
    let default_security_limits = crate::extractors::security::SecurityLimits::default();
    let security_limits = security_limits.unwrap_or(&default_security_limits);
    let (rotated, new_width, new_height) = crate::pdf::render::rotate_png_page_if_needed_with_security_limits(
        (**data).clone(),
        width,
        height,
        correction_degrees,
        security_limits,
    )?;
    Ok((std::sync::Arc::new(rotated), new_width, new_height, correction_degrees))
}
/// Undo [`upright_raster_for_backend`]'s rotation on a backend result's pixel-space geometry,
/// mapping bboxes from the upright raster the backend actually ran OCR on back to the raw
/// MediaBox raster every downstream consumer (`build_mixed_ocr_page_document`,
/// `rescale_ocr_bboxes_to_page_points`, the #530 regression test) expects.
///
/// A no-op when `correction_degrees == 0` (the overwhelmingly common case: no correction was
/// applied at all). Reuses [`undo_auto_rotate_point`] — the same per-point inverse
/// [`undo_auto_rotate_document_bboxes`] uses for a backend's own internal auto-rotation — since
/// the geometry problem is identical: a point in a rotated raster mapped back to the unrotated
/// one it will be rescaled against.
///
/// `ocr_internal_document` element bboxes, `tables` bboxes, `formulas` bboxes, and
/// `ocr_elements` word/line/block geometry are corrected: those are the pixel-space
/// geometry sources read from a backend result before rescaling into page points.
/// `formulas` matters for GLM paired mode, which pushes the SAME `region_bbox` into
/// both `formulas[].bbox` and `table_bboxes` (`candle_ocr/glm_ocr_backend.rs`); correcting only
/// the table half left `formula_bbox_to_page_points` rescaling an upright-raster box against
/// MediaBox-raster page dimensions on any `/Rotate != 0` page. `ocr_elements` matters because
/// `attach_page_ocr_payload` copies it straight onto the assembled document's
/// `prebuilt_ocr_elements` with no further pixel-space transform of its own (unlike
/// `ocr_internal_document`, which `rescale_ocr_bboxes_to_page_points` still rescales downstream)
/// — leaving it uncorrected here means every word/line box a `RequiresUpright` backend reports
/// stays in the upright raster's frame forever on any `/Rotate != 0` page.
#[cfg(all(any(feature = "ocr", feature = "ocr-pipeline"), feature = "pdf"))]
pub(super) fn undo_upright_raster_correction(
    result: &mut crate::types::ExtractedDocument,
    correction_degrees: u32,
    upright_width: u32,
    upright_height: u32,
) {
    if correction_degrees == 0 {
        return;
    }
    let correction_degrees = correction_degrees as u16;
    let (processed_width, processed_height) = (f64::from(upright_width), f64::from(upright_height));
    let undo_point =
        |x: f64, y: f64| undo_auto_rotate_point(x, y, correction_degrees, processed_width, processed_height);
    let undo_bbox = |bbox: &mut crate::types::extraction::BoundingBox| {
        let (x0, y0) = undo_point(bbox.x0, bbox.y0);
        let (x1, y1) = undo_point(bbox.x1, bbox.y1);
        bbox.x0 = x0.min(x1);
        bbox.x1 = x0.max(x1);
        bbox.y0 = y0.min(y1);
        bbox.y1 = y0.max(y1);
    };
    if let Some(doc) = result.ocr_internal_document.as_mut() {
        for element in &mut doc.elements {
            if let Some(bbox) = element.bbox.as_mut() {
                undo_bbox(bbox);
            }
        }
    }
    for table in &mut result.tables {
        if let Some(bbox) = table.bounding_box.as_mut() {
            undo_bbox(bbox);
        }
    }
    for formula in &mut result.formulas {
        if let Some(bbox) = formula.bbox.as_mut() {
            undo_bbox(bbox);
        }
    }
    if let Some(elements) = result.ocr_elements.as_mut() {
        for element in elements {
            undo_ocr_element_geometry(&mut element.geometry, undo_point);
        }
    }
}
/// Undo an upright-raster (or auto-rotate) correction on one `OcrElement`'s geometry, the
/// word/line/block-level boxes reported by [`crate::types::OcrElement`]. Missed entirely by
/// [`undo_upright_raster_correction`] before this fix (#657 follow-up): that function only
/// walked `ocr_internal_document`, `tables`, and `formulas`, leaving every `ocr_elements` entry
/// in the upright raster's pixel frame.
///
/// `Rectangle` is corrected corner-to-corner like a [`crate::types::extraction::BoundingBox`]
/// (top-left and bottom-right corners, then re-derived into `left`/`top`/`width`/`height` from
/// the transformed corners' min/max, since a quarter turn can swap which corner is which).
/// `Quadrilateral` is corrected point-by-point — a quarter turn does not preserve the
/// "clockwise from top-left" point order the type promises, but no downstream reader currently
/// depends on point order, only on the region the four points enclose.
#[cfg(all(any(feature = "ocr", feature = "ocr-pipeline"), feature = "pdf"))]
pub(super) fn undo_ocr_element_geometry(
    geometry: &mut crate::types::ocr_elements::OcrBoundingGeometry,
    undo_point: impl Fn(f64, f64) -> (f64, f64),
) {
    use crate::types::ocr_elements::OcrBoundingGeometry;
    match geometry {
        OcrBoundingGeometry::Rectangle {
            left,
            top,
            width,
            height,
        } => {
            let (x0, y0) = (f64::from(*left), f64::from(*top));
            let (x1, y1) = (x0 + f64::from(*width), y0 + f64::from(*height));
            let (nx0, ny0) = undo_point(x0, y0);
            let (nx1, ny1) = undo_point(x1, y1);
            let min_x = nx0.min(nx1);
            let min_y = ny0.min(ny1);
            let max_x = nx0.max(nx1);
            let max_y = ny0.max(ny1);
            *left = min_x.round().max(0.0) as u32;
            *top = min_y.round().max(0.0) as u32;
            *width = (max_x - min_x).round().max(0.0) as u32;
            *height = (max_y - min_y).round().max(0.0) as u32;
        }
        OcrBoundingGeometry::Quadrilateral { points } => {
            for point in points.iter_mut() {
                let (x, y) = undo_point(f64::from(point.x), f64::from(point.y));
                point.x = x.round().max(0.0) as u32;
                point.y = y.round().max(0.0) as u32;
            }
        }
    }
}
/// Inject layout-detection settings into OcrConfig backend options for paired-mode backends.
///
/// When layout detection is active and provides detections, certain backends (e.g., GLM-OCR)
/// may need configuration injected from the layout-detection config. This function ensures
/// that the `enable_chart_understanding` flag from `ExtractionConfig.layout` is propagated
/// to the OCR backend via `backend_options` so per-region task dispatch can honor it.
#[cfg(all(feature = "ocr", feature = "layout-detection"))]
pub(super) fn inject_layout_config_to_backend(
    config: &crate::core::config::ocr::OcrConfig,
    extraction_config: &ExtractionConfig,
) -> crate::core::config::ocr::OcrConfig {
    let mut config = config.clone();
    if let Some(layout_cfg) = &extraction_config.layout {
        let mut opts = config.backend_options.take().unwrap_or_else(|| serde_json::json!({}));

        if !opts.is_object() {
            if !opts.is_null() {
                tracing::warn!(
                    backend_options = %opts,
                    "backend_options was not a JSON object; replacing with new object to inject enable_chart_understanding"
                );
            }
            opts = serde_json::json!({});
        }

        if let Some(obj) = opts.as_object_mut() {
            obj.insert(
                "enable_chart_understanding".to_string(),
                serde_json::Value::Bool(layout_cfg.enable_chart_understanding),
            );
        }

        config.backend_options = Some(opts);
    }
    config
}
