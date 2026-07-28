//! Layout detection runner for PDF pages.
//!
//! Renders pages of a PDF document in chunks, runs the layout engine on each
//! chunk in sequence, and converts pixel-space detections to PDF
//! coordinate–space [`PageLayoutResult`] values.
//!
//! Chunked rendering+detection keeps peak memory proportional to
//! `LAYOUT_BATCH_CHUNK_SIZE` images plus the accumulated output images,
//! rather than requiring the whole document's rasterised frames and the full
//! ONNX batch tensor to be live simultaneously.
//!
//! The resulting images, page metadata, layout hints, and raw detections feed
//! both native markdown structure recovery and OCR layout assembly.

/// Maximum number of pages sent to the layout model in a single ONNX call.
///
/// Each chunk requires:  chunk_size × 3 × 640 × 640 × 4 bytes ≈ 4.9 MB × chunk_size
/// for the batch tensor.  8 pages ≈ 39 MB vs 214 pages ≈ 1.05 GB without chunking.
#[cfg(all(feature = "pdf", feature = "layout-detection"))]
const LAYOUT_BATCH_CHUNK_SIZE: usize = 8;

/// Small white raster used when one page cannot be rendered.
///
/// Keeping a page-aligned placeholder lets downstream OCR reuse every other
/// rendered page without retrying the entire document. The matching detection
/// result is empty and uses these same dimensions.
#[cfg(all(feature = "pdf", feature = "layout-detection"))]
const FAILED_RENDER_PLACEHOLDER_SIDE: u32 = 64;

#[cfg(all(feature = "pdf", feature = "layout-detection"))]
use crate::{
    Result, XbergError,
    core::config::{
        ExtractionConfig,
        layout::{LayoutDetectionConfig, LayoutStrategy},
    },
    extractors::pdf::layout_hints::pixel_detection_to_layout_hints_pdf_space,
    pdf::layout_gate::PageGateDecision,
    pdf::structure::types::{LayoutHint, PageLayoutResult},
};

/// How [`run_layout_for_pdf_pages`] treats pages the `Auto` gate skips.
///
/// The markdown path only needs rasters for pages the model inspects, so it
/// skips rendering entirely. The OCR path reuses the layout pass's rasters as
/// OCR input images, so gated pages must still render; only inference is
/// skipped there.
#[cfg(all(feature = "pdf", feature = "layout-detection"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum GatedPageHandling {
    /// Skip both the raster render and model inference for gated pages.
    ///
    /// The page-aligned output still carries a small white placeholder image
    /// for gated pages (see [`render_failure_placeholder`]); downstream
    /// markdown consumers never read it because gated pages have no hints.
    SkipRender,
    /// Render the page raster but skip model inference for gated pages.
    ///
    /// Only the OCR entry point constructs this, so builds without an OCR
    /// feature see the variant as unused.
    #[cfg_attr(not(any(feature = "ocr", feature = "ocr-pipeline")), allow(dead_code))]
    RenderWithoutInference,
}

/// Page-aligned layout pass data: `(images, results, hints_per_page,
/// detections_per_page)`, one entry per page at every index.
///
/// - `images[i]` is the rendered RGB image for page `i`, or a small white
///   placeholder when the page failed to render or the `Auto` gate skipped it
///   on the markdown path.
/// - `results[i]` holds per-region detection metadata in PDF coordinate space.
/// - `hints_per_page[i]` holds the layout hints for page `i` (empty for
///   render-failed, gate-skipped, and no-detection pages).
/// - `detections_per_page[i]` preserves pixel-space detections for OCR layout
///   assembly (empty for render-failed and gate-skipped pages).
#[cfg(all(feature = "pdf", feature = "layout-detection"))]
type LayoutForMarkdownOutput = (
    Vec<image::RgbImage>,
    Vec<PageLayoutResult>,
    Vec<Vec<LayoutHint>>,
    Vec<crate::layout::DetectionResult>,
);

/// Layout pass outcome plus the `Auto` gate's per-page decisions.
///
/// `data` is `None` when the gate skipped every page (the caller proceeds
/// exactly as if layout were off). `gate_decisions` is `None` under strategy
/// `Always` and always present under `Auto`, including the all-gated case, so
/// gate behavior stays auditable from extraction metadata.
#[cfg(all(feature = "pdf", feature = "layout-detection"))]
pub(super) struct LayoutRunOutput {
    pub data: Option<LayoutForMarkdownOutput>,
    pub gate_decisions: Option<Vec<PageGateDecision>>,
}

#[cfg(all(feature = "pdf", feature = "layout-detection"))]
struct RenderedLayoutPage {
    page_index: usize,
    page_width_pts: f32,
    page_height_pts: f32,
    rotation: u32,
    image: Option<image::RgbImage>,
    /// `false` when the `Auto` gate skipped this page: it stays out of the
    /// ONNX batch and keeps an empty detection, exactly like a page where the
    /// model ran and found nothing.
    run_inference: bool,
}

#[cfg(all(feature = "pdf", feature = "layout-detection"))]
struct AssembledLayoutPage {
    image: image::RgbImage,
    result: PageLayoutResult,
    hints: Vec<LayoutHint>,
    detection: crate::layout::DetectionResult,
}

#[cfg(all(feature = "pdf", feature = "layout-detection"))]
async fn run_layout_for_pdf_pages_async(
    content: &[u8],
    layout_config: &LayoutDetectionConfig,
    thread_budget: usize,
    gated_handling: GatedPageHandling,
) -> Result<LayoutRunOutput> {
    #[cfg(feature = "tokio-runtime")]
    {
        let owned_content = content.to_vec();
        let owned_config = layout_config.clone();
        tokio::task::spawn_blocking(move || {
            run_layout_for_pdf_pages(&owned_content, &owned_config, thread_budget, gated_handling)
        })
        .await
        .map_err(|error| XbergError::Other(format!("layout runner task failed: {error}")))?
    }

    #[cfg(not(feature = "tokio-runtime"))]
    run_layout_for_pdf_pages(content, layout_config, thread_budget, gated_handling)
}

#[cfg(all(feature = "pdf", feature = "layout-detection"))]
fn validate_batch_cardinality(expected: usize, actual: usize) -> Result<()> {
    if actual == expected {
        return Ok(());
    }

    Err(XbergError::Other(format!(
        "layout runner: batch detection returned {actual} results for {expected} rendered pages"
    )))
}

#[cfg(all(feature = "pdf", feature = "layout-detection"))]
fn render_failure_placeholder() -> image::RgbImage {
    image::RgbImage::from_pixel(
        FAILED_RENDER_PLACEHOLDER_SIDE,
        FAILED_RENDER_PLACEHOLDER_SIDE,
        image::Rgb([u8::MAX; 3]),
    )
}

#[cfg(all(feature = "pdf", feature = "layout-detection"))]
fn displayed_page_dimensions(width: f32, height: f32, rotation_degrees: u32) -> (f32, f32) {
    match rotation_degrees % 360 {
        90 | 270 => (height, width),
        _ => (width, height),
    }
}

/// Run the `Auto` gate over every page, or `None` under strategy `Always`.
#[cfg(all(feature = "pdf", feature = "layout-detection"))]
fn auto_gate_decisions(
    doc: &pdf_oxide::PdfDocument,
    layout_config: &LayoutDetectionConfig,
    page_count: usize,
) -> Option<Vec<PageGateDecision>> {
    match layout_config.strategy {
        LayoutStrategy::Always => None,
        LayoutStrategy::Auto => {
            let decisions = crate::pdf::layout_gate::decide_pages(doc, page_count);
            let selected = decisions.iter().filter(|decision| decision.run_layout).count();
            tracing::info!(
                pages = page_count,
                selected,
                skipped = page_count - selected,
                "layout gate: auto strategy page selection"
            );
            Some(decisions)
        }
    }
}

/// Whether the `Auto` gate selected `page_index` for the model.
///
/// `None` gate decisions (strategy `Always`) select every page; a page index
/// beyond the decision vector also runs, so a gate bug can only ever cost
/// speed, never coverage.
#[cfg(all(feature = "pdf", feature = "layout-detection"))]
fn gate_selects_page(gate_decisions: Option<&[PageGateDecision]>, page_index: usize) -> bool {
    gate_decisions.is_none_or(|decisions| decisions.get(page_index).is_none_or(|decision| decision.run_layout))
}

#[cfg(all(feature = "pdf", feature = "layout-detection"))]
fn render_layout_chunk(
    doc: &pdf_oxide::PdfDocument,
    page_rotations: &[u32],
    chunk_start: usize,
    chunk_end: usize,
    gate_decisions: Option<&[PageGateDecision]>,
    gated_handling: GatedPageHandling,
) -> Vec<RenderedLayoutPage> {
    (chunk_start..chunk_end)
        .map(|page_index| {
            let (media_width, media_height) = doc
                .get_page_media_box(page_index)
                .map(|(llx, lly, urx, ury)| ((urx - llx).abs(), (ury - lly).abs()))
                .unwrap_or((612.0, 792.0));
            let rotation = page_rotations.get(page_index).copied().unwrap_or(0);
            let (page_width_pts, page_height_pts) = displayed_page_dimensions(media_width, media_height, rotation);
            let run_inference = gate_selects_page(gate_decisions, page_index);
            let skip_render = !run_inference && gated_handling == GatedPageHandling::SkipRender;
            let image = if skip_render {
                None
            } else {
                render_layout_page(doc, page_index, page_width_pts, page_height_pts)
            };

            RenderedLayoutPage {
                page_index,
                page_width_pts,
                page_height_pts,
                rotation,
                image,
                run_inference,
            }
        })
        .collect()
}

#[cfg(all(feature = "pdf", feature = "layout-detection"))]
fn render_layout_page(
    doc: &pdf_oxide::PdfDocument,
    page_index: usize,
    page_width_pts: f32,
    page_height_pts: f32,
) -> Option<image::RgbImage> {
    let rendered = crate::pdf::render::render_page_with_safeguards(doc, page_index, 150).map_err(|error| {
        tracing::warn!(
            page = page_index + 1,
            page_width_pts,
            page_height_pts,
            error = %error,
            "layout runner: skipping page with render failure, returning empty detections"
        );
    });
    let rendered = rendered.ok()?;

    image::load_from_memory(&rendered.data)
        .map(image::DynamicImage::into_rgb8)
        .map_err(|error| {
            tracing::warn!(
                page = page_index + 1,
                page_width_pts,
                page_height_pts,
                error = %error,
                "layout runner: skipping page (PNG decode failed), returning empty detections"
            );
        })
        .ok()
}

/// Whether a page joins the ONNX batch: it must both be gate-selected and
/// have a real raster. Render-failed and gate-skipped pages stay out and end
/// up with empty detections.
#[cfg(all(feature = "pdf", feature = "layout-detection"))]
fn enters_inference_batch(page: &RenderedLayoutPage) -> bool {
    page.run_inference && page.image.is_some()
}

#[cfg(all(feature = "pdf", feature = "layout-detection"))]
fn detect_layout_chunk(
    engine: &mut crate::layout::LayoutEngine,
    pages: &[RenderedLayoutPage],
) -> Result<Vec<Option<crate::layout::DetectionResult>>> {
    let rendered_positions: Vec<usize> = pages
        .iter()
        .enumerate()
        .filter_map(|(position, page)| enters_inference_batch(page).then_some(position))
        .collect();
    if rendered_positions.is_empty() {
        return Ok((0..pages.len()).map(|_| None).collect());
    }

    let images: Vec<&image::RgbImage> = rendered_positions
        .iter()
        .map(|&position| pages[position].image.as_ref().expect("filtered to rendered pages"))
        .collect();
    let results = engine
        .detect_batch(&images)
        .map_err(|error| XbergError::Other(format!("layout runner: batch detection failed: {error}")))?;
    validate_batch_cardinality(images.len(), results.len())?;

    let mut detections: Vec<Option<crate::layout::DetectionResult>> = (0..pages.len()).map(|_| None).collect();
    for (&position, (detection, _timings)) in rendered_positions.iter().zip(results) {
        detections[position] = Some(detection);
    }
    Ok(detections)
}

#[cfg(all(feature = "pdf", feature = "layout-detection"))]
fn assemble_layout_chunk(
    pages: Vec<RenderedLayoutPage>,
    detections: Vec<Option<crate::layout::DetectionResult>>,
) -> Result<Vec<AssembledLayoutPage>> {
    validate_batch_cardinality(pages.len(), detections.len())?;
    Ok(pages
        .into_iter()
        .zip(detections)
        .map(|(page, detection)| assemble_layout_page(page, detection))
        .collect())
}

#[cfg(all(feature = "pdf", feature = "layout-detection"))]
fn assemble_layout_page(
    page: RenderedLayoutPage,
    detection: Option<crate::layout::DetectionResult>,
) -> AssembledLayoutPage {
    let image = page.image.unwrap_or_else(render_failure_placeholder);
    let detection = detection.unwrap_or_else(|| crate::layout::DetectionResult {
        page_width: image.width(),
        page_height: image.height(),
        detections: Vec::new(),
    });
    let hints = pixel_detection_to_layout_hints_pdf_space(
        &detection,
        image.width(),
        image.height(),
        page.page_width_pts,
        page.page_height_pts,
    );

    tracing::debug!(
        page = page.page_index + 1,
        detections = detection.detections.len(),
        hints = hints.len(),
        page_width_pts = page.page_width_pts,
        page_height_pts = page.page_height_pts,
        rotation = page.rotation,
        image_width_px = image.width(),
        image_height_px = image.height(),
        "layout runner: page detections"
    );

    AssembledLayoutPage {
        image,
        result: PageLayoutResult {
            page_width_pts: page.page_width_pts,
            page_height_pts: page.page_height_pts,
        },
        hints,
        detection,
    }
}

#[cfg(all(feature = "pdf", feature = "layout-detection"))]
pub(super) fn run_layout_for_pdf_pages(
    content: &[u8],
    layout_config: &LayoutDetectionConfig,
    thread_budget: usize,
    gated_handling: GatedPageHandling,
) -> Result<LayoutRunOutput> {
    let doc = pdf_oxide::PdfDocument::from_bytes(content.to_vec()).map_err(|e| XbergError::Parsing {
        message: format!("layout runner: failed to open PDF: {e}"),
        source: None,
    })?;

    let page_count = doc.page_count().map_err(|e| XbergError::Parsing {
        message: format!("layout runner: failed to get page count: {e}"),
        source: None,
    })?;

    if page_count == 0 {
        return Ok(LayoutRunOutput {
            data: Some((Vec::new(), Vec::new(), Vec::new(), Vec::new())),
            gate_decisions: None,
        });
    }

    let gate_decisions = auto_gate_decisions(&doc, layout_config, page_count);
    if let Some(decisions) = &gate_decisions
        && decisions.iter().all(|decision| !decision.run_layout)
    {
        // No page can benefit: behave exactly as if layout were off, with no
        // engine init and no renders. The OCR path then produces its own
        // rasters once, as it does without layout. ~keep
        return Ok(LayoutRunOutput {
            data: None,
            gate_decisions,
        });
    }

    let mut engine = crate::layout::take_or_create_engine(layout_config, thread_budget)
        .map_err(|e| XbergError::Other(format!("layout runner: engine init failed: {e}")))?;

    let page_rotations = crate::pdf::render::get_page_rotations(content, page_count);

    let mut all_images: Vec<image::RgbImage> = Vec::with_capacity(page_count);
    let mut all_layout_results: Vec<PageLayoutResult> = Vec::with_capacity(page_count);
    let mut all_hints: Vec<Vec<LayoutHint>> = Vec::with_capacity(page_count);
    let mut all_detections: Vec<crate::layout::DetectionResult> = Vec::with_capacity(page_count);

    let total_chunks = page_count.div_ceil(LAYOUT_BATCH_CHUNK_SIZE);

    for (chunk_idx, chunk_start) in (0..page_count).step_by(LAYOUT_BATCH_CHUNK_SIZE).enumerate() {
        let chunk_end = (chunk_start + LAYOUT_BATCH_CHUNK_SIZE).min(page_count);
        let pages = render_layout_chunk(
            &doc,
            &page_rotations,
            chunk_start,
            chunk_end,
            gate_decisions.as_deref(),
            gated_handling,
        );
        let rendered = pages.iter().filter(|page| page.image.is_some()).count();
        tracing::debug!(
            chunk_idx,
            total_chunks,
            chunk_start,
            chunk_end,
            rendered,
            "layout runner: detecting chunk"
        );

        let detections = match detect_layout_chunk(&mut engine, &pages) {
            Ok(detections) => detections,
            Err(error) => {
                crate::layout::return_engine(engine);
                return Err(error);
            }
        };
        for page in assemble_layout_chunk(pages, detections)? {
            all_images.push(page.image);
            all_layout_results.push(page.result);
            all_hints.push(page.hints);
            all_detections.push(page.detection);
        }
    }

    crate::layout::return_engine(engine);

    Ok(LayoutRunOutput {
        data: Some((all_images, all_layout_results, all_hints, all_detections)),
        gate_decisions,
    })
}

/// Convenience wrapper that reads `use_layout_for_markdown` and other gate
/// conditions from `config` and, when they are all satisfied, runs
/// [`run_layout_for_pdf_pages`].
///
/// The first four values are `None` when the feature is not requested, when
/// the `Auto` gate skipped every page, or on soft failure (logged so the
/// markdown path continues without layout hints). The fifth value carries the
/// `Auto` gate's per-page decisions whenever the gate ran, including the
/// all-gated case. Rendering and inference run off the async executor when a
/// Tokio runtime is enabled.
#[cfg(all(feature = "pdf", feature = "layout-detection"))]
type LayoutForMarkdownOptional = (
    Option<Vec<image::RgbImage>>,
    Option<Vec<PageLayoutResult>>,
    Option<Vec<Vec<LayoutHint>>>,
    Option<Vec<crate::layout::DetectionResult>>,
    Option<Vec<PageGateDecision>>,
);

#[cfg(all(feature = "pdf", feature = "layout-detection"))]
pub(super) async fn maybe_run_layout_for_markdown(
    content: &[u8],
    config: &ExtractionConfig,
) -> LayoutForMarkdownOptional {
    if !config.use_layout_for_markdown {
        return (None, None, None, None, None);
    }
    let Some(layout_config) = config.resolved_layout_config() else {
        return (None, None, None, None, None);
    };
    if config.force_ocr {
        return (None, None, None, None, None);
    }
    let thread_budget = crate::core::config::concurrency::resolve_thread_budget(config.concurrency.as_ref());
    match run_layout_for_pdf_pages_async(
        content,
        layout_config.as_ref(),
        thread_budget,
        GatedPageHandling::SkipRender,
    )
    .await
    {
        Ok(LayoutRunOutput {
            data: Some((images, results, hints, detections)),
            gate_decisions,
        }) => {
            let total_hints: usize = hints.iter().map(|h| h.len()).sum();
            tracing::info!(
                pages = images.len(),
                total_hints,
                "layout-for-markdown: detection succeeded"
            );
            (
                Some(images),
                Some(results),
                Some(hints),
                Some(detections),
                gate_decisions,
            )
        }
        Ok(LayoutRunOutput {
            data: None,
            gate_decisions,
        }) => {
            tracing::info!("layout-for-markdown: auto gate skipped every page, continuing without layout hints");
            (None, None, None, None, gate_decisions)
        }
        Err(e) => {
            tracing::warn!(
                error = %e,
                "layout-for-markdown: detection failed, continuing without layout hints"
            );
            (None, None, None, None, None)
        }
    }
}

/// Run the layout pass used by OCR without blocking a Tokio worker thread.
///
/// `data: None` means the `Auto` gate skipped every page: the caller proceeds
/// exactly as if layout were off, rendering its own OCR rasters once.
#[cfg(all(
    feature = "pdf",
    feature = "layout-detection",
    any(feature = "ocr", feature = "ocr-pipeline")
))]
pub(super) async fn run_layout_for_ocr(
    content: &[u8],
    layout_config: &LayoutDetectionConfig,
    thread_budget: usize,
) -> Result<LayoutRunOutput> {
    // OCR consumes the layout pass's rasters as its input images, so gated
    // pages still render; only model inference is skipped for them.
    run_layout_for_pdf_pages_async(
        content,
        layout_config,
        thread_budget,
        GatedPageHandling::RenderWithoutInference,
    )
    .await
}

#[cfg(all(test, feature = "pdf", feature = "layout-detection"))]
mod tests {
    use super::{
        FAILED_RENDER_PLACEHOLDER_SIDE, GatedPageHandling, RenderedLayoutPage, assemble_layout_chunk,
        displayed_page_dimensions, gate_selects_page, render_failure_placeholder, render_layout_chunk,
        validate_batch_cardinality,
    };
    use crate::pdf::layout_gate::PageGateDecision;

    fn rendered_page(page_index: usize, width: u32, page_width_pts: f32) -> RenderedLayoutPage {
        RenderedLayoutPage {
            page_index,
            page_width_pts,
            page_height_pts: 200.0,
            rotation: 0,
            image: Some(image::RgbImage::new(width, 20)),
            run_inference: true,
        }
    }

    fn rotated_pdf(inherited: bool) -> Vec<u8> {
        use lopdf::{Document, Object, Stream, dictionary};

        let mut document = Document::with_version("1.5");
        let pages_id = document.new_object_id();
        let page_id = document.new_object_id();
        let content_id = document.add_object(Stream::new(dictionary! {}, Vec::new()));

        let mut page = dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "MediaBox" => vec![0.into(), 0.into(), 200.into(), 100.into()],
            "Resources" => dictionary! {},
            "Contents" => content_id,
        };
        if !inherited {
            page.set("Rotate", 90);
        }
        document.objects.insert(page_id, Object::Dictionary(page));

        let mut pages = dictionary! {
            "Type" => "Pages",
            "Kids" => vec![page_id.into()],
            "Count" => 1,
        };
        if inherited {
            pages.set("Rotate", 90);
        }
        document.objects.insert(pages_id, Object::Dictionary(pages));

        let catalog_id = document.add_object(dictionary! {
            "Type" => "Catalog",
            "Pages" => pages_id,
        });
        document.trailer.set("Root", catalog_id);

        let mut bytes = Vec::new();
        document.save_to(&mut bytes).expect("fixture PDF must serialize");
        bytes
    }

    fn assert_pdf_oxide_applies_rotation(bytes: Vec<u8>) {
        let document = pdf_oxide::PdfDocument::from_bytes(bytes.clone()).expect("fixture PDF must open");
        let rendered =
            crate::pdf::render::render_page_with_safeguards(&document, 0, 72).expect("rotated fixture must render");
        let rotations = crate::pdf::render::get_page_rotations(&bytes, 1);
        let (media_width, media_height) = document
            .get_page_media_box(0)
            .map(|(llx, lly, urx, ury)| ((urx - llx).abs(), (ury - lly).abs()))
            .expect("fixture must have a MediaBox");

        assert_eq!(rotations, vec![90]);
        assert!(
            rendered.height > rendered.width,
            "rotated landscape page must render as portrait"
        );
        assert_eq!(
            displayed_page_dimensions(media_width, media_height, rotations[0]),
            (100.0, 200.0)
        );
    }

    #[test]
    fn batch_cardinality_accepts_one_result_per_rendered_page() {
        assert!(validate_batch_cardinality(3, 3).is_ok());
    }

    #[test]
    fn batch_cardinality_rejects_truncated_results() {
        let error = validate_batch_cardinality(3, 2).expect_err("truncated results must fail");
        assert!(error.to_string().contains("2 results for 3 rendered pages"));
    }

    #[test]
    fn render_failure_placeholder_is_nonempty_and_white() {
        let image = render_failure_placeholder();

        assert_eq!(
            image.dimensions(),
            (FAILED_RENDER_PLACEHOLDER_SIDE, FAILED_RENDER_PLACEHOLDER_SIDE)
        );
        assert!(image.pixels().all(|pixel| pixel.0 == [u8::MAX; 3]));
    }

    #[test]
    fn assembly_preserves_page_order_and_detection_mapping() {
        let pages = vec![rendered_page(0, 10, 100.0), rendered_page(1, 20, 110.0)];
        let detections = vec![
            Some(crate::layout::DetectionResult {
                page_width: 10,
                page_height: 20,
                detections: Vec::new(),
            }),
            Some(crate::layout::DetectionResult {
                page_width: 20,
                page_height: 20,
                detections: Vec::new(),
            }),
        ];

        let assembled = assemble_layout_chunk(pages, detections).expect("aligned chunk must assemble");

        assert_eq!(assembled[0].image.width(), 10);
        assert_eq!(assembled[0].result.page_width_pts, 100.0);
        assert_eq!(assembled[0].detection.page_width, 10);
        assert_eq!(assembled[1].image.width(), 20);
        assert_eq!(assembled[1].result.page_width_pts, 110.0);
        assert_eq!(assembled[1].detection.page_width, 20);
    }

    #[test]
    fn assembly_keeps_failed_render_slot_with_empty_detection() {
        let mut failed_page = rendered_page(1, 20, 110.0);
        failed_page.image = None;

        let assembled = assemble_layout_chunk(vec![failed_page], vec![None]).expect("failed render must stay aligned");

        assert_eq!(
            assembled[0].image.dimensions(),
            (FAILED_RENDER_PLACEHOLDER_SIDE, FAILED_RENDER_PLACEHOLDER_SIDE)
        );
        assert!(assembled[0].detection.detections.is_empty());
        assert_eq!(assembled[0].result.page_width_pts, 110.0);
    }

    #[test]
    fn assembly_rejects_detection_cardinality_mismatch() {
        let error = assemble_layout_chunk(vec![rendered_page(0, 10, 100.0)], Vec::new())
            .err()
            .expect("missing detection slot must fail");

        assert!(error.to_string().contains("0 results for 1 rendered pages"));
    }

    fn gate_decision(run_layout: bool) -> PageGateDecision {
        PageGateDecision {
            run_layout,
            reason: if run_layout {
                crate::pdf::layout_gate::GateReason::MultiColumn
            } else {
                crate::pdf::layout_gate::GateReason::PlainText
            },
        }
    }

    /// A one-page single-column prose PDF with a real text content stream:
    /// the gate must classify it as plain text and skip the model.
    fn prose_pdf(lines: usize) -> Vec<u8> {
        use lopdf::{Document, Object, Stream, dictionary};

        let mut document = Document::with_version("1.5");
        let pages_id = document.new_object_id();
        let font_id = document.add_object(dictionary! {
            "Type" => "Font",
            "Subtype" => "Type1",
            "BaseFont" => "Helvetica",
        });
        let mut content = String::from("BT /F1 10 Tf 72 720 Td 14 TL\n");
        for line in 0..lines {
            content.push_str(&format!(
                "(Line {line} of steady single column prose with enough characters to pass the floor.) Tj T*\n"
            ));
        }
        content.push_str("ET");
        let content_id = document.add_object(Stream::new(dictionary! {}, content.into_bytes()));
        let page_id = document.new_object_id();
        document.objects.insert(
            page_id,
            Object::Dictionary(dictionary! {
                "Type" => "Page",
                "Parent" => pages_id,
                "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
                "Resources" => dictionary! { "Font" => dictionary! { "F1" => font_id } },
                "Contents" => content_id,
            }),
        );
        document.objects.insert(
            pages_id,
            Object::Dictionary(dictionary! {
                "Type" => "Pages",
                "Kids" => vec![page_id.into()],
                "Count" => 1,
            }),
        );
        let catalog_id = document.add_object(dictionary! {
            "Type" => "Catalog",
            "Pages" => pages_id,
        });
        document.trailer.set("Root", catalog_id);

        let mut bytes = Vec::new();
        document.save_to(&mut bytes).expect("fixture PDF must serialize");
        bytes
    }

    /// End-to-end `Auto` on real PDF bytes: the gate skips plain prose, and
    /// the runner returns no layout data before any engine init (this test
    /// must never need ONNX models). The gate is asserted first so a gate
    /// regression fails here rather than in an engine-init attempt.
    #[test]
    fn auto_strategy_gates_prose_page_and_skips_the_layout_pass_entirely() {
        use crate::core::config::layout::LayoutStrategy;

        let bytes = prose_pdf(20);
        let doc = pdf_oxide::PdfDocument::from_bytes(bytes.clone()).expect("fixture PDF must open");
        let decisions = crate::pdf::layout_gate::decide_pages(&doc, 1);
        assert_eq!(decisions.len(), 1);
        assert!(
            !decisions[0].run_layout,
            "prose fixture must be gated, got {:?}",
            decisions[0].reason
        );
        assert_eq!(decisions[0].reason, crate::pdf::layout_gate::GateReason::PlainText);

        let config = crate::core::config::layout::LayoutDetectionConfig {
            strategy: LayoutStrategy::Auto,
            ..Default::default()
        };
        let output = super::run_layout_for_pdf_pages(&bytes, &config, 1, GatedPageHandling::SkipRender)
            .expect("all-gated run must succeed without a layout engine");
        assert!(output.data.is_none(), "all-gated prose must skip the layout pass");
        let recorded = output.gate_decisions.expect("auto strategy must record decisions");
        assert_eq!(recorded, decisions);
    }

    #[test]
    fn only_gate_selected_pages_with_real_rasters_enter_the_inference_batch() {
        let selected_with_image = rendered_page(0, 10, 100.0);
        let mut gated_with_image = rendered_page(1, 10, 100.0);
        gated_with_image.run_inference = false;
        let mut selected_render_failed = rendered_page(2, 10, 100.0);
        selected_render_failed.image = None;

        assert!(super::enters_inference_batch(&selected_with_image));
        assert!(!super::enters_inference_batch(&gated_with_image));
        assert!(!super::enters_inference_batch(&selected_render_failed));
    }

    #[test]
    fn gated_page_assembles_placeholder_image_with_empty_detection_and_hints() {
        let mut gated = rendered_page(0, 10, 100.0);
        gated.image = None;
        gated.run_inference = false;

        let assembled = assemble_layout_chunk(vec![gated], vec![None]).expect("gated page must stay aligned");

        assert_eq!(
            assembled[0].image.dimensions(),
            (FAILED_RENDER_PLACEHOLDER_SIDE, FAILED_RENDER_PLACEHOLDER_SIDE)
        );
        assert!(assembled[0].detection.detections.is_empty());
        assert!(assembled[0].hints.is_empty());
    }

    #[test]
    fn absent_gate_decisions_select_every_page() {
        assert!(gate_selects_page(None, 0));
        assert!(gate_selects_page(None, 7));
    }

    #[test]
    fn gate_decisions_beyond_the_vector_fail_open_to_running_the_model() {
        let decisions = vec![gate_decision(false)];
        assert!(gate_selects_page(Some(&decisions), 1));
    }

    #[test]
    fn gated_page_is_excluded_and_selected_page_is_kept() {
        let decisions = vec![gate_decision(false), gate_decision(true)];
        assert!(!gate_selects_page(Some(&decisions), 0));
        assert!(gate_selects_page(Some(&decisions), 1));
    }

    /// SkipRender must not rasterise gated pages; RenderWithoutInference must.
    #[test]
    fn gated_handling_controls_whether_gated_pages_render() {
        let bytes = rotated_pdf(false);
        let doc = pdf_oxide::PdfDocument::from_bytes(bytes).expect("fixture PDF must open");
        let decisions = vec![gate_decision(false)];

        let skipped = render_layout_chunk(&doc, &[0], 0, 1, Some(&decisions), GatedPageHandling::SkipRender);
        assert!(skipped[0].image.is_none(), "gated page must not render");
        assert!(!skipped[0].run_inference);

        let rendered = render_layout_chunk(
            &doc,
            &[0],
            0,
            1,
            Some(&decisions),
            GatedPageHandling::RenderWithoutInference,
        );
        assert!(rendered[0].image.is_some(), "OCR-path gated page must still render");
        assert!(!rendered[0].run_inference, "gated page must stay out of the ONNX batch");
    }

    #[test]
    fn pdf_oxide_applies_direct_page_rotation() {
        assert_pdf_oxide_applies_rotation(rotated_pdf(false));
    }

    #[test]
    fn pdf_oxide_applies_inherited_page_rotation() {
        assert_pdf_oxide_applies_rotation(rotated_pdf(true));
    }
}
