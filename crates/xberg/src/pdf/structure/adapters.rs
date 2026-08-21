//! OCR-to-structure adapters: convert xberg internal types into the PDF
//! structure pipeline's paragraph representation.
// `types` is used by the OCR conversion helpers (`feature = "ocr"`) and by the
// unused when only `ocr-pipeline` is on without `layout-detection`, as in the
// WASM `ocr-wasm` feature set.
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
use super::types;

/// Separate points-per-pixel scale factors for [`resolve_ocr_font_size_pt`]'s two
/// geometric fallback branches.
///
/// `element.bbox` and `element.ocr_geometry` can be in different units by the time
/// font-size resolution runs, depending on the caller's route:
///
/// - On the mixed OCR route (`extractors::pdf::ocr::assemble_mixed_ocr_page_document`),
///   `element.bbox` has already been rescaled into PDF points by that route's own
///   `rescale_ocr_bboxes_to_page_points` before this runs, so the `block_bbox`-height
///   fallback needs no further scaling. `element.ocr_geometry` is untouched by that
///   rescale -- it stays raster-pixel space, because
///   `extraction::derive::OcrElement::geometry` documents that field as public,
///   raster-pixel-space API and rescaling it in place would both corrupt that contract
///   and, since its point type is `(u32, u32)`, round away sub-pixel precision. The
///   quad-edge fallback (sceptre/paddle's `Quadrilateral` geometry) therefore still
///   needs the real points-per-pixel ratio.
/// - On the pure-OCR route (`extract_with_ocr_for_page`), both fields are still in raw
///   OCR raster pixels when font-size resolution runs, so both branches need the same
///   real points-per-pixel ratio.
///
/// A single scalar cannot express this: collapsing to one number either corrupts the
/// mixed route's bbox branch (multiplying an already-in-points value again) or leaves
/// its geometry branch unconverted -- inflating sceptre/paddle font sizes by the raster
/// DPI's points-per-pixel ratio (~2.08x at 150 DPI) and pushing ordinary intra-line
/// ascender/descender variance over the heading heuristic's `MIN_HEADING_FONT_GAP`,
/// breaking paragraphs mid physical line.
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
#[derive(Clone, Copy, Debug)]
pub(crate) struct OcrFontSizeScale {
    bbox_points_per_pixel: f32,
    geometry_points_per_pixel: f32,
}

/// Scale to apply to a value that is already in the target unit (PDF points): a
/// genuine no-op, not a magic number picked for effect.
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
const NO_OP_POINTS_PER_PIXEL: f32 = 1.0;

#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
impl OcrFontSizeScale {
    /// `element.bbox` and `element.ocr_geometry` are both still in the same unit
    /// space (raw OCR raster pixels on the pure-OCR route, or already-uniform test
    /// fixtures): apply one real points-per-pixel ratio to both branches.
    pub(crate) fn uniform(points_per_pixel: f32) -> Self {
        Self {
            bbox_points_per_pixel: points_per_pixel,
            geometry_points_per_pixel: points_per_pixel,
        }
    }

    /// The mixed OCR route's shape: `element.bbox` was already rescaled into PDF
    /// points by the caller, so the bbox-height fallback is a no-op scale, while
    /// `element.ocr_geometry` is still raw raster pixels and needs the real
    /// points-per-pixel ratio for this page.
    pub(crate) fn bbox_already_in_points(geometry_points_per_pixel: f32) -> Self {
        Self {
            bbox_points_per_pixel: NO_OP_POINTS_PER_PIXEL,
            geometry_points_per_pixel,
        }
    }
}

/// Convert an OCR-produced [`crate::types::internal::InternalDocument`] into a vec of [`types::PdfParagraph`]s
/// for the structure assembly pipeline.
///
/// Coordinates are in image-space (y=0 at top) and are flipped to PDF-space
/// (y=0 at bottom) using `page_height_px`.
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
#[allow(dead_code)]
pub(crate) fn ocr_doc_to_paragraphs(
    doc: &crate::types::internal::InternalDocument,
    page_height_px: u32,
    font_size_scale: OcrFontSizeScale,
) -> Vec<types::PdfParagraph> {
    use crate::types::internal::ElementKind;
    let page_h = page_height_px as f32;
    let mut result = Vec::new();
    let mut previous_block_id = None;
    let block_font_sizes = ocr_block_median_font_sizes(doc, page_h, font_size_scale);

    for element in &doc.elements {
        if !matches!(element.kind, ElementKind::OcrText { .. }) || element.text.trim().is_empty() {
            previous_block_id = None;
            continue;
        }
        let block_id = hocr_block_id(element);
        let block_median_pt = block_id.and_then(|block_id| block_font_sizes.get(block_id).copied());
        // A block that split into several list-item paragraphs (see
        // `make_ocr_block_paragraphs`'s doc comment, #713) only offers its *first*
        // segment for the same-block-id merge below: that segment carries the block's
        // leading edge and is what a continuing block would have merged into before
        // this split existed. Later segments are already-separated list items and must
        // not be re-merged into the block that precedes them.
        for (segment_index, paragraph) in make_ocr_block_paragraphs(element, page_h, font_size_scale, block_median_pt)
            .into_iter()
            .enumerate()
        {
            if segment_index == 0 && block_id.is_some() && block_id == previous_block_id {
                if let Some(current) = result.last_mut() {
                    merge_ocr_block_paragraph(current, paragraph);
                } else {
                    result.push(paragraph);
                }
            } else {
                result.push(paragraph);
            }
        }
        previous_block_id = block_id;
    }

    trace_conversion(doc, &result);
    result
}

/// hOCR `x_fsize` attribute key, mirroring `ocr::hocr_parser::HOCR_FONT_SIZE_ATTRIBUTE`
/// (not imported directly to keep this module's feature-gate surface self-contained).
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
const HOCR_FONT_SIZE_ATTRIBUTE: &str = "x_fsize";

/// hOCR bold/italic fraction attribute keys, mirroring
/// `ocr::hocr_parser::HOCR_BOLD_FRACTION_ATTRIBUTE` / `HOCR_ITALIC_FRACTION_ATTRIBUTE`
/// (not imported directly, for the same self-containment reason as
/// [`HOCR_FONT_SIZE_ATTRIBUTE`]). Only `ocr::hocr_parser::parse_hocr_to_internal_document`
/// (the Tesseract hOCR block-parsing path) ever writes these; sceptre and paddle elements
/// never carry them, so [`resolve_ocr_style_flags`] falls back to `(false, false)` for both.
///
/// Each value is the fraction (0.0-1.0) of words in the hOCR block that Tesseract
/// reported as bold/italic, not a boolean -- a block is a mix of words, not a single
/// style.
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
const HOCR_BOLD_FRACTION_ATTRIBUTE: &str = "x_bold_fraction";
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
const HOCR_ITALIC_FRACTION_ATTRIBUTE: &str = "x_italic_fraction";

/// A block counts as bold/italic once more than half its words carry that style,
/// mirroring the majority-vote convention already used for native-PDF segments
/// (`pdf::structure::pipeline`'s `is_bold = ... .count() > segments.len() / 2`).
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
const STYLE_MAJORITY_FRACTION: f32 = 0.5;

/// Resolve block-level bold/italic flags for an OCR-produced text element from its
/// hOCR fraction attributes, defaulting to `(false, false)` when the attribute is
/// absent (sceptre, paddle) or unparseable (defensive: never panics on a malformed
/// value).
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
fn resolve_ocr_style_flags(element: &crate::types::internal::InternalElement) -> (bool, bool) {
    let attrs = element.attributes.as_ref();
    let is_majority = |attribute: &str| {
        attrs
            .and_then(|attrs| attrs.get(attribute))
            .and_then(|value| value.parse::<f32>().ok())
            .filter(|value| value.is_finite() && (0.0..=1.0).contains(value))
            .is_some_and(|fraction| fraction > STYLE_MAJORITY_FRACTION)
    };
    (
        is_majority(HOCR_BOLD_FRACTION_ATTRIBUTE),
        is_majority(HOCR_ITALIC_FRACTION_ATTRIBUTE),
    )
}

/// Fallback font size (points) when neither an `x_fsize` attribute nor a usable
/// bbox height is available. Matches the constant every call site hardcoded before.
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
const DEFAULT_OCR_FONT_SIZE_PT: f32 = 12.0;

/// A/B switch for the block-median font-size resolution described on
/// [`ocr_block_median_font_sizes`] (#712).
///
/// **Off.** Measured a net regression over GT-scored full-text F1 across 3
/// backends x 7 scanned fixtures: better on 2 files, worse on 10, tied on 2.
/// The mechanism is over-fusion -- taking a block-wide median collapses
/// legitimately distinct fragments into one font size, and the max block size
/// grows in every affected file (ordinance/paddle 327 words vs 70;
/// nougat_007/sceptre 549 vs 111), which drags word-gluing artefacts up with
/// it. It does deliver one real win -- fabricated mid-sentence headings
/// (`### storage.`, `### groundwork for future developments. Over time,`) went
/// 14 -> 0 on multi_page/paddle and 9 -> 0 on nougat_007/sceptre -- but that
/// win is now covered directly at the heading gate by
/// `SUPPRESS_LOWERCASE_START_HEADINGS` (#712), without paying for it in
/// full-text fusion. For this to come back, the median would need to be scoped
/// tighter than "whole hOCR block" -- e.g. per physical line rather than per
/// block -- so it stops fusing fragments that legitimately belong to different
/// lines.
///
/// **This is the single edit that neutralises the whole feature**: flip it to
/// `true` and [`ocr_block_median_font_sizes`] resumes producing entries, so
/// every element goes back through the block-median path -- no other line of
/// this module needs to change, and no call site does either.
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
const RESOLVE_OCR_FONT_SIZE_PER_BLOCK: bool = false;

/// A block needs at least this many measurable fragments before a median is worth
/// taking. At one sample the median is the sample, so the entry would be an exact
/// identity; requiring two keeps "nothing to stabilise" structurally distinct from
/// "stabilised" instead of relying on that identity holding.
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
const MIN_BLOCK_FRAGMENTS_FOR_MEDIAN: usize = 2;

/// The hOCR `x_fsize` font size (points) an element carries, if any.
///
/// Only `ocr::hocr_parser` (the Tesseract path) ever writes this attribute, and it
/// writes one value per `ocr_par`, so every fragment of a Tesseract block already
/// reports the *same* number. That is exactly the within-block stability the median
/// below manufactures for the geometric backends.
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
fn hocr_font_size_pt(element: &crate::types::internal::InternalElement) -> Option<f32> {
    element
        .attributes
        .as_ref()
        .and_then(|attrs| attrs.get(HOCR_FONT_SIZE_ATTRIBUTE))
        .and_then(|value| value.parse::<f32>().ok())
        .filter(|value| value.is_finite() && *value > 0.0)
}

/// The geometric font-size proxy (PDF points) for a single OCR fragment, or `None`
/// when the element carries neither usable quadrilateral geometry nor a usable bbox.
///
/// Split out of [`resolve_ocr_font_size_pt`] unchanged so that
/// [`ocr_block_median_font_sizes`] can sample exactly the same quantity the
/// per-fragment path would have produced -- the median is taken over resolved point
/// sizes, not over raw pixel heights, so the two are directly comparable and the
/// `None` case is excluded from the sample set rather than entering it as
/// [`DEFAULT_OCR_FONT_SIZE_PT`].
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
fn geometric_ocr_font_size_pt(
    element: &crate::types::internal::InternalElement,
    block_bbox: Option<(f32, f32, f32, f32)>,
    line_count: usize,
    font_size_scale: OcrFontSizeScale,
) -> Option<f32> {
    element
        .ocr_geometry
        .as_ref()
        .and_then(quad_edge_height_px)
        .map(|height_px| height_px * font_size_scale.geometry_points_per_pixel)
        .or_else(|| block_bbox.map(|(_, bottom, _, top)| (top - bottom) * font_size_scale.bbox_points_per_pixel))
        .map(|height| height / line_count.max(1) as f32)
        .filter(|value| value.is_finite() && *value > 0.0)
}

/// One font size (PDF points) per hOCR block id, taken as the **median** over every
/// measurable fragment of that block (#712).
///
/// `SegmentData::font_size` carries two incompatible physical quantities depending
/// on backend. Tesseract's `x_fsize` is a typographic point size and is a per-
/// `ocr_par` constant. Sceptre and PaddleOCR write no `x_fsize`, so their font size
/// is the *height of a detection box* -- a geometric quantity that varies with the
/// glyph mix (ascenders, descenders, punctuation) *within one physical line*.
/// Measured on the sceptre regression fixture, six fragments of a single body line
/// resolved to 15.4-17.3pt: a 1.12 ratio inside one line, against
/// `MIN_HEADING_FONT_RATIO = 1.15`, and a 1.9pt spread against the 1.5pt absolute
/// `font_change` paragraph break -- so paragraphs split mid-line, between two words
/// of the same sentence, and the heading heuristic had ~80% of its headroom eaten
/// before any real size difference was considered.
///
/// **Median, not mean.** The samples are detection-box heights, and OCR detection
/// routinely emits a badly fragmented box: a single stray character, a piece of a
/// rule mis-detected as text, a box that swallowed part of the line above. Those are
/// outliers of arbitrary magnitude in *both* directions, and a mean moves with them
/// proportionally to how extreme they are -- one 4x-too-tall fragment in a five-
/// fragment block drags the mean 60% above the true line height. The median ignores
/// magnitude entirely and only asks how many fragments fall on each side, so it
/// still reports the type size the majority of the block actually has.
///
/// Blocks whose fragments carry `x_fsize` are skipped entirely (never sampled, never
/// keyed), so the Tesseract path cannot be perturbed even in principle -- see
/// [`resolve_ocr_font_size_pt`], which returns on `x_fsize` before it ever consults
/// this map. Elements with no `hocr_block_id` (and blocks under
/// [`MIN_BLOCK_FRAGMENTS_FOR_MEDIAN`] measurable fragments) produce no entry, and
/// their callers fall back to the unchanged per-fragment resolution.
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
fn ocr_block_median_font_sizes(
    doc: &crate::types::internal::InternalDocument,
    page_height: f32,
    font_size_scale: OcrFontSizeScale,
) -> ahash::AHashMap<String, f32> {
    use crate::types::internal::ElementKind;

    if !RESOLVE_OCR_FONT_SIZE_PER_BLOCK {
        return ahash::AHashMap::new();
    }

    let mut samples: ahash::AHashMap<String, Vec<f32>> = ahash::AHashMap::new();
    for element in &doc.elements {
        if !matches!(element.kind, ElementKind::OcrText { .. }) || element.text.trim().is_empty() {
            continue;
        }
        if hocr_font_size_pt(element).is_some() {
            continue;
        }
        let Some(block_id) = hocr_block_id(element) else {
            continue;
        };
        let line_count = element.text.split('\n').count().max(1);
        let block_bbox = pdf_block_bbox(element, page_height);
        let Some(font_size) = geometric_ocr_font_size_pt(element, block_bbox, line_count, font_size_scale) else {
            continue;
        };
        samples.entry(block_id.to_owned()).or_default().push(font_size);
    }

    samples
        .into_iter()
        .filter(|(_, values)| values.len() >= MIN_BLOCK_FRAGMENTS_FOR_MEDIAN)
        .filter_map(|(block_id, mut values)| median_font_size_pt(&mut values).map(|median| (block_id, median)))
        .collect()
}

/// Median of a non-empty slice of font sizes, averaging the two central values for
/// an even count. Sorted with `f32::total_cmp` rather than `partial_cmp`: the input
/// is already filtered to finite positives, and `total_cmp` is a total order, so
/// this cannot panic on a stray NaN reaching it through a future edit.
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
fn median_font_size_pt(values: &mut [f32]) -> Option<f32> {
    if values.is_empty() {
        return None;
    }
    values.sort_unstable_by(f32::total_cmp);
    let middle = values.len() / 2;
    let median = if values.len().is_multiple_of(2) {
        (values[middle - 1] + values[middle]) / 2.0
    } else {
        values[middle]
    };
    (median.is_finite() && median > 0.0).then_some(median)
}

/// Resolve a font size in PDF points for an OCR-produced text element.
///
/// Prefers the hOCR `x_fsize` attribute when present: it is already reported in
/// points by the backend that populated it (tesseract's per-block average, set by
/// `ocr::hocr_parser`), so it needs no unit conversion. That branch returns
/// *before* `block_median_pt` is consulted, so the Tesseract path is bit-identical
/// to what it was before #712 -- not "provably a no-op through the median", but
/// never routed through it at all, which is a stronger and cheaper guarantee.
///
/// Otherwise prefers `block_median_pt`, the median over every measurable fragment
/// of this element's hOCR block (see [`ocr_block_median_font_sizes`]), which is what
/// removes the intra-line detection-box variance that sceptre and PaddleOCR would
/// otherwise feed into the heading and paragraph-break thresholds.
///
/// Falls back last to a proxy derived from the element's own line-height
/// (`quad_edge_height_px` when the backend reports a quadrilateral, else the raw
/// bbox height), scaled from OCR raster pixels to PDF points via `font_size_scale`.
/// Each branch is scaled by the factor appropriate to *its own* unit space -- see
/// [`OcrFontSizeScale`] -- because `element.bbox` and `element.ocr_geometry` are not
/// always in the same unit space by the time this runs. The scaling matters either
/// way: the document-level heading heuristic
/// (`pdf::structure::pipeline::extract_document_structure_from_segments`) compares
/// font sizes against `MIN_HEADING_FONT_GAP`, an **absolute-points** constant: a
/// pixel-space value would either swamp that constant into irrelevance (high-DPI
/// rasters) or make it dominate spuriously (low-DPI rasters), silently mis-tuning
/// heading promotion either way.
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
fn resolve_ocr_font_size_pt(
    element: &crate::types::internal::InternalElement,
    block_bbox: Option<(f32, f32, f32, f32)>,
    line_count: usize,
    font_size_scale: OcrFontSizeScale,
    block_median_pt: Option<f32>,
) -> f32 {
    if let Some(font_size) = hocr_font_size_pt(element) {
        return font_size;
    }
    if let Some(median) = block_median_pt {
        return median;
    }
    geometric_ocr_font_size_pt(element, block_bbox, line_count, font_size_scale).unwrap_or(DEFAULT_OCR_FONT_SIZE_PT)
}

/// Rotation-robust line-height proxy for a 4-point OCR quadrilateral, in raster
/// pixels.
///
/// `resolve_ocr_font_size_pt`'s previous fallback took the axis-aligned bounding
/// box height (`top - bottom` of `block_bbox`) as a font-size proxy. For a
/// perfectly horizontal quad that is the line height; for a quad with *any* skew
/// -- unavoidable on real scanned pages -- the AABB height also picks up
/// `line_width * sin(skew_angle)`, a term that grows with how much *text* is on
/// the line rather than with its *point size*. Measured on
/// `test_documents/pdf_scanned/ordinance_2197_scanned.pdf` (sceptre backend,
/// non-layout-detection route): resolved font sizes for `blocks_to_paragraphs`
/// ranged from 8.6 to 472.3 and tracked word count (13-word title: 470.4;
/// 2-word "EXHIBIT B-3": 74.9; 1-word "H.": 13.4) rather than the two visibly
/// different type sizes actually on the page, so the heading/body clusters
/// were inseparable and every page logged `headings=0`.
///
/// This computes the two side-edge lengths of the quad (`top_left`-`bottom_left`
/// and `top_right`-`bottom_right`) and averages them instead. A side edge runs
/// *across* the line, not *along* it, so it measures the true glyph-height band
/// regardless of the line's length or skew.
///
/// Returns `None` for `OcrBoundingGeometry::Rectangle` (already axis-aligned by
/// construction; Tesseract's proxy path already used `block_bbox` directly and is
/// unaffected) so callers fall back to the existing `block_bbox`-height proxy.
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
fn quad_edge_height_px(geometry: &crate::types::OcrBoundingGeometry) -> Option<f32> {
    let crate::types::OcrBoundingGeometry::Quadrilateral { points } = geometry else {
        return None;
    };
    // Clockwise from top-left: [top_left, top_right, bottom_right, bottom_left].
    let [top_left, top_right, bottom_right, bottom_left] = *points;
    let left_edge = point_distance(top_left, bottom_left);
    let right_edge = point_distance(top_right, bottom_right);
    let height = (left_edge + right_edge) / 2.0;
    (height.is_finite() && height > 0.0).then_some(height)
}

#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
fn point_distance(a: (u32, u32), b: (u32, u32)) -> f32 {
    let dx = a.0 as f32 - b.0 as f32;
    let dy = a.1 as f32 - b.1 as f32;
    dx.hypot(dy)
}

/// Harvest [`crate::pdf::hierarchy::SegmentData`] out of already-built OCR paragraphs,
/// one page at a time, for the document-level heading/list heuristic
/// (`pdf::structure::pipeline::extract_document_structure_from_segments`).
///
/// Reuses the geometry `make_ocr_pdf_line` already computed instead of re-deriving it
/// from the raw `InternalDocument`: every [`types::PdfLine`] built by the functions in
/// this module carries exactly one `SegmentData` (see `make_ocr_pdf_line`), so this is
/// a plain extraction, not a second conversion path.
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
pub(crate) fn segments_from_ocr_pages(
    pages: &[Vec<types::PdfParagraph>],
) -> Vec<Vec<crate::pdf::hierarchy::SegmentData>> {
    pages
        .iter()
        .map(|paragraphs| {
            paragraphs
                .iter()
                .flat_map(|paragraph| paragraph.lines.iter())
                .flat_map(|line| line.segments.iter().cloned())
                .filter(|segment| !segment.text.trim().is_empty())
                .collect()
        })
        .collect()
}

#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
fn hocr_block_id(element: &crate::types::internal::InternalElement) -> Option<&str> {
    const HOCR_BLOCK_ID_ATTRIBUTE: &str = "hocr_block_id";
    const MAX_HOCR_BLOCK_FRAGMENT_LINES: usize = 6;

    if element.text.lines().count() > MAX_HOCR_BLOCK_FRAGMENT_LINES {
        return None;
    }

    element
        .attributes
        .as_ref()?
        .get(HOCR_BLOCK_ID_ATTRIBUTE)
        .map(String::as_str)
        .filter(|block_id| !block_id.is_empty())
}

#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
fn merge_ocr_block_paragraph(current: &mut types::PdfParagraph, next: types::PdfParagraph) {
    current.text.push('\n');
    current.text.push_str(&next.text);
    current.lines.extend(next.lines);
    current.block_bbox = match (current.block_bbox, next.block_bbox) {
        (Some(a), Some(b)) => Some((a.0.min(b.0), a.1.min(b.1), a.2.max(b.2), a.3.max(b.3))),
        (bbox @ Some(_), None) | (None, bbox @ Some(_)) => bbox,
        (None, None) => None,
    };
    current.word_count = types::PdfParagraph::compute_word_count(&current.text, &current.lines);
}

/// Convert unstructured OCR text into page paragraphs without inventing geometry.
///
/// Line endings are normalized first: OCR page text is not guaranteed to be LF-only.
/// The VLM backend returns the model's markdown verbatim out of an HTTP JSON body
/// (`crate::llm::vlm_ocr`), which routinely carries `\r\n`, and nothing on the way
/// here rewrites it. Splitting raw would fold a whole page into one paragraph (#316).
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
pub(crate) fn ocr_text_to_paragraphs(text: &str) -> Vec<types::PdfParagraph> {
    crate::extraction::transform::normalize_line_endings(text)
        .split("\n\n")
        .map(str::trim)
        .filter(|paragraph| !paragraph.is_empty())
        .map(|paragraph| make_ocr_paragraph(paragraph.to_string(), Vec::new(), None, DEFAULT_OCR_FONT_SIZE_PT))
        .collect()
}

#[cfg(all(feature = "ocr", feature = "layout-detection"))]
pub(crate) fn ocr_doc_to_layout_paragraphs(
    doc: &crate::types::internal::InternalDocument,
    page_height_px: u32,
    hints: &[types::LayoutHint],
    min_confidence: f32,
    min_containment: f32,
    font_size_scale: OcrFontSizeScale,
) -> Vec<types::PdfParagraph> {
    use crate::types::internal::ElementKind;
    let page_height = page_height_px as f32;
    let mut all_lines = Vec::new();
    let mut all_hint_indices = Vec::new();
    let mut element_indices = Vec::new();
    let mut block_ids = Vec::new();
    let block_font_sizes = ocr_block_median_font_sizes(doc, page_height, font_size_scale);
    let elements = doc
        .elements
        .iter()
        .filter(|element| matches!(element.kind, ElementKind::OcrText { .. }))
        .filter(|element| !element.text.trim().is_empty())
        .collect::<Vec<_>>();

    for (element_index, element) in elements.iter().enumerate() {
        let promote_logo_title = element_index == 0
            && should_promote_logo_followed_by_title(
                element,
                elements.get(1).copied(),
                page_height,
                hints,
                min_confidence,
            );
        let block_median_pt = hocr_block_id(element).and_then(|block_id| block_font_sizes.get(block_id).copied());
        let mut lines = make_ocr_line_paragraphs(element, page_height, font_size_scale, block_median_pt);
        let selected = super::layout_classify::apply_layout_overrides_with_matches(
            &mut lines,
            hints,
            min_confidence,
            min_containment,
            None,
        );
        let mut hint_indices = compatible_hint_indices(&lines, hints, selected, min_containment);
        if promote_logo_title {
            promote_second_line_to_title(&mut lines, &mut hint_indices);
        }
        element_indices.extend(std::iter::repeat_n(element_index, lines.len()));
        block_ids.extend(std::iter::repeat_n(
            hocr_block_id(element).map(str::to_owned),
            lines.len(),
        ));
        all_lines.extend(lines);
        all_hint_indices.extend(hint_indices);
    }

    let result = regroup_layout_lines_by_element(all_lines, all_hint_indices, element_indices, block_ids);
    trace_conversion(doc, &result);
    result
}

#[cfg(all(any(feature = "ocr", feature = "ocr-pipeline"), feature = "layout-detection"))]
pub(crate) fn promote_anchored_ordered_list_sequences(pages: &mut [Vec<types::PdfParagraph>]) {
    const MAX_INTERVENING_PARAGRAPHS: usize = 8;

    let positions = nonempty_paragraph_positions(pages);
    let mut promotions = Vec::new();

    for (position_index, &(page_index, paragraph_index)) in positions.iter().enumerate() {
        let anchor = &pages[page_index][paragraph_index];
        let Some(anchor_value) = anchored_numeric_list_value(anchor) else {
            continue;
        };
        let Some(second_value) = anchor_value.checked_add(1) else {
            continue;
        };
        let Some(third_value) = anchor_value.checked_add(2) else {
            continue;
        };
        let Some(second_index) = find_ordered_list_successor(
            pages,
            &positions,
            position_index,
            second_value,
            MAX_INTERVENING_PARAGRAPHS,
        ) else {
            continue;
        };
        let Some(third_index) =
            find_ordered_list_successor(pages, &positions, second_index, third_value, MAX_INTERVENING_PARAGRAPHS)
        else {
            continue;
        };

        promotions.extend([positions[second_index], positions[third_index]]);
    }

    promotions.sort_unstable();
    promotions.dedup();
    for (page_index, paragraph_index) in promotions {
        let paragraph = &mut pages[page_index][paragraph_index];
        paragraph.is_list_item = true;
        paragraph.layout_class = Some(types::LayoutHintClass::ListItem);
    }
}

#[cfg(all(any(feature = "ocr", feature = "ocr-pipeline"), feature = "layout-detection"))]
fn nonempty_paragraph_positions(pages: &[Vec<types::PdfParagraph>]) -> Vec<(usize, usize)> {
    pages
        .iter()
        .enumerate()
        .flat_map(|(page_index, paragraphs)| {
            paragraphs
                .iter()
                .enumerate()
                .filter(|(_, paragraph)| !paragraph.text.trim().is_empty())
                .map(move |(paragraph_index, _)| (page_index, paragraph_index))
        })
        .collect()
}

#[cfg(all(any(feature = "ocr", feature = "ocr-pipeline"), feature = "layout-detection"))]
fn anchored_numeric_list_value(paragraph: &types::PdfParagraph) -> Option<u16> {
    paragraph.is_list_item.then(|| numeric_list_value(paragraph)).flatten()
}

#[cfg(all(any(feature = "ocr", feature = "ocr-pipeline"), feature = "layout-detection"))]
fn numeric_list_value(paragraph: &types::PdfParagraph) -> Option<u16> {
    let marker = super::list_marker::parse_ordered_list_marker(&paragraph.text)?;
    (marker.has_content && marker.has_separator)
        .then_some(marker.numeric_value)
        .flatten()
}

#[cfg(all(any(feature = "ocr", feature = "ocr-pipeline"), feature = "layout-detection"))]
fn find_ordered_list_successor(
    pages: &[Vec<types::PdfParagraph>],
    positions: &[(usize, usize)],
    predecessor_index: usize,
    expected_value: u16,
    max_intervening_paragraphs: usize,
) -> Option<usize> {
    let first_candidate = predecessor_index + 1;
    if first_candidate >= positions.len() {
        return None;
    }
    let last_candidate = (first_candidate + max_intervening_paragraphs).min(positions.len().saturating_sub(1));
    for (offset, &(page_index, paragraph_index)) in positions[first_candidate..=last_candidate].iter().enumerate() {
        let candidate_index = first_candidate + offset;
        let paragraph = &pages[page_index][paragraph_index];
        if let Some(actual_value) = numeric_list_value(paragraph) {
            return (actual_value == expected_value && is_ordered_list_candidate(paragraph)).then_some(candidate_index);
        }
    }
    None
}

#[cfg(all(any(feature = "ocr", feature = "ocr-pipeline"), feature = "layout-detection"))]
fn is_ordered_list_candidate(paragraph: &types::PdfParagraph) -> bool {
    paragraph.heading_level.is_none()
        && !paragraph.is_list_item
        && !paragraph.is_code_block
        && !paragraph.is_formula
        && !paragraph.is_page_furniture
        && !super::classify::is_numbered_section_heading(&paragraph.text)
        && matches!(
            paragraph.layout_class,
            None | Some(types::LayoutHintClass::Text | types::LayoutHintClass::Other)
        )
}

#[cfg(all(feature = "ocr", feature = "layout-detection"))]
fn should_promote_logo_followed_by_title(
    element: &crate::types::internal::InternalElement,
    next_element: Option<&crate::types::internal::InternalElement>,
    page_height: f32,
    hints: &[types::LayoutHint],
    min_confidence: f32,
) -> bool {
    const MAX_LOGO_CHARACTERS: usize = 12;
    const MAX_LOGO_WORDS: usize = 2;
    const MIN_TITLE_WORDS: usize = 2;
    const MAX_TITLE_WORDS: usize = 12;
    const MAX_TITLE_CHARACTERS: usize = 120;

    let mut lines = element.text.lines();
    let Some(logo) = lines.next().map(str::trim) else {
        return false;
    };
    let Some(title) = lines.next().map(str::trim) else {
        return false;
    };
    if lines.next().is_some() || logo.is_empty() || title.is_empty() {
        return false;
    }

    !has_semantic_heading_hint(hints, min_confidence)
        && is_uppercase_logo(logo, MAX_LOGO_CHARACTERS, MAX_LOGO_WORDS)
        && is_conservative_title(title, MIN_TITLE_WORDS, MAX_TITLE_WORDS, MAX_TITLE_CHARACTERS)
        && next_element.is_some_and(|next| follows_with_prose(element, next, page_height))
}

#[cfg(all(feature = "ocr", feature = "layout-detection"))]
fn has_semantic_heading_hint(hints: &[types::LayoutHint], min_confidence: f32) -> bool {
    hints.iter().any(|hint| {
        hint.confidence >= min_confidence
            && matches!(
                hint.class_name,
                types::LayoutHintClass::Title | types::LayoutHintClass::SectionHeader
            )
    })
}

#[cfg(all(feature = "ocr", feature = "layout-detection"))]
fn is_uppercase_logo(text: &str, max_characters: usize, max_words: usize) -> bool {
    let alphabetic = text
        .chars()
        .filter(|character| character.is_alphabetic())
        .collect::<Vec<_>>();
    (2..=max_characters).contains(&text.chars().count())
        && text.split_whitespace().count() <= max_words
        && alphabetic.len() >= 2
        && alphabetic.iter().all(|character| character.is_uppercase())
        && text
            .chars()
            .all(|character| character.is_alphabetic() || character.is_whitespace())
}

#[cfg(all(feature = "ocr", feature = "layout-detection"))]
fn is_conservative_title(text: &str, min_words: usize, max_words: usize, max_characters: usize) -> bool {
    let word_count = text.split_whitespace().count();
    let starts_uppercase = text
        .chars()
        .find(|character| character.is_alphabetic())
        .is_some_and(|character| character.is_uppercase());
    (min_words..=max_words).contains(&word_count)
        && text.chars().count() <= max_characters
        && starts_uppercase
        && text.chars().any(|character| character.is_lowercase())
        && !text.ends_with(['.', '!', '?', ':', ';', ','])
}

#[cfg(all(feature = "ocr", feature = "layout-detection"))]
fn follows_with_prose(
    element: &crate::types::internal::InternalElement,
    next_element: &crate::types::internal::InternalElement,
    page_height: f32,
) -> bool {
    const MIN_PROSE_WORDS: usize = 8;
    const MIN_FIRST_BLOCK_TOP_FRACTION: f32 = 0.65;
    const MAX_FIRST_BLOCK_HEIGHT_FRACTION: f32 = 0.15;
    const MAX_PROSE_GAP_FRACTION: f32 = 0.15;

    let prose = next_element.text.trim();
    let Some((_, first_bottom, _, first_top)) = pdf_block_bbox(element, page_height) else {
        return false;
    };
    let Some((_, _, _, prose_top)) = pdf_block_bbox(next_element, page_height) else {
        return false;
    };
    let prose_gap = first_bottom - prose_top;
    first_top >= page_height * MIN_FIRST_BLOCK_TOP_FRACTION
        && first_top - first_bottom <= page_height * MAX_FIRST_BLOCK_HEIGHT_FRACTION
        && (0.0..=page_height * MAX_PROSE_GAP_FRACTION).contains(&prose_gap)
        && prose.split_whitespace().count() >= MIN_PROSE_WORDS
        && prose.chars().any(|character| character.is_lowercase())
        && prose.ends_with(['.', '!', '?'])
}

#[cfg(all(feature = "ocr", feature = "layout-detection"))]
fn promote_second_line_to_title(lines: &mut [types::PdfParagraph], hint_indices: &mut [Option<usize>]) {
    const TITLE_LINE_INDEX: usize = 1;
    let Some(title) = lines.get_mut(TITLE_LINE_INDEX) else {
        return;
    };
    if has_structural_override(title) {
        return;
    }
    title.heading_level = Some(1);
    title.layout_class = Some(types::LayoutHintClass::Title);
    if let Some(hint_index) = hint_indices.get_mut(TITLE_LINE_INDEX) {
        *hint_index = None;
    }
}

#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
fn trace_conversion(doc: &crate::types::internal::InternalDocument, result: &[types::PdfParagraph]) {
    tracing::debug!(
        input_elements = doc
            .elements
            .iter()
            .filter(|element| matches!(element.kind, crate::types::internal::ElementKind::OcrText { .. }))
            .count(),
        output_paragraphs = result.len(),
        total_text_chars = result.iter().map(|paragraph| paragraph.text.len()).sum::<usize>(),
        "ocr_doc_to_paragraphs"
    );
}

/// Build one paragraph per hOCR block -- or several, when the block's own OCR lines
/// contain at least [`MIN_LIST_MARKERS_TO_SPLIT`] independent list-marker openings.
///
/// Tesseract (and other backends) group physically adjacent lines into one hOCR
/// block by layout proximity alone, with no notion of "list item" -- a tightly
/// spaced list can end up as a single block spanning every item. Before this, the
/// non-layout OCR route (`ocr_doc_to_paragraphs`) turned that whole block into one
/// paragraph unconditionally, so only the block's first line was ever visible to a
/// downstream text-marker classifier; items 2..N were swallowed into item 1's body
/// (#713 -- the same problem `push_body_group`/`split_body_group_at_list_markers`
/// already solve for the ML-layout route, reused here rather than re-implemented).
///
/// Below the threshold this returns exactly the single paragraph the old
/// `make_ocr_block_paragraph` built, byte-for-byte: same `text` (the raw
/// `element.text`, not a rejoin), same block-level (not per-segment) font size. Only
/// once a real split happens do segments switch to the `split_body_group_at_list_markers`
/// / `build_body_paragraph` construction already used by the layout route (including its
/// marker-led space-join -- see `join_body_segment_text`'s doc comment for why that
/// matters for rendering).
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
fn make_ocr_block_paragraphs(
    element: &crate::types::internal::InternalElement,
    page_height: f32,
    font_size_scale: OcrFontSizeScale,
    block_median_pt: Option<f32>,
) -> Vec<types::PdfParagraph> {
    let block_bbox = pdf_block_bbox(element, page_height);
    let line_paragraphs = make_ocr_line_paragraphs(element, page_height, font_size_scale, block_median_pt);

    let marker_line_count = line_paragraphs
        .iter()
        .filter(|paragraph| super::pipeline::looks_like_list_item(&paragraph.text))
        .count();
    if marker_line_count < MIN_LIST_MARKERS_TO_SPLIT {
        let lines = line_paragraphs
            .into_iter()
            .flat_map(|paragraph| paragraph.lines)
            .collect();
        let text_line_count = element.text.split('\n').count().max(1);
        let font_size =
            resolve_ocr_font_size_pt(element, block_bbox, text_line_count, font_size_scale, block_median_pt);
        return vec![make_ocr_paragraph(element.text.clone(), lines, block_bbox, font_size)];
    }

    split_body_group_at_list_markers(line_paragraphs)
        .into_iter()
        .filter_map(build_body_paragraph)
        .collect()
}

#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
fn make_ocr_line_paragraphs(
    element: &crate::types::internal::InternalElement,
    page_height: f32,
    font_size_scale: OcrFontSizeScale,
    block_median_pt: Option<f32>,
) -> Vec<types::PdfParagraph> {
    let block_bbox = pdf_block_bbox(element, page_height);
    let text_lines = element.text.split('\n').collect::<Vec<_>>();
    let line_count = text_lines.len().max(1);
    // Resolved once per element (an hOCR block, or a single OCR line for
    // backends that emit line-level elements): `x_fsize` when the backend
    // provides one is itself a block/element-level average, so re-deriving a
    // per-line value would add false precision without a real per-line signal.
    // For the geometric backends, `block_median_pt` widens that "once" from the
    // element to the whole hOCR block -- an element there is a single detection
    // box, so per-element resolution is precisely the per-fragment variance #712
    // is about.
    let font_size = resolve_ocr_font_size_pt(element, block_bbox, line_count, font_size_scale, block_median_pt);
    // Same reasoning applies to bold/italic: `x_bold_fraction`/`x_italic_fraction`
    // are block-level averages, resolved once and applied uniformly to every line.
    let (is_bold, is_italic) = resolve_ocr_style_flags(element);

    text_lines
        .into_iter()
        .enumerate()
        .map(|(line_index, text)| {
            make_ocr_line_paragraph(text, line_index, line_count, block_bbox, font_size, is_bold, is_italic)
        })
        .collect()
}

#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
fn pdf_block_bbox(element: &crate::types::internal::InternalElement, page_height: f32) -> Option<(f32, f32, f32, f32)> {
    element.bbox.as_ref().map(|bbox| {
        (
            bbox.x0 as f32,
            page_height - bbox.y1 as f32,
            bbox.x1 as f32,
            page_height - bbox.y0 as f32,
        )
    })
}

#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
fn make_ocr_line_paragraph(
    text: &str,
    line_index: usize,
    line_count: usize,
    block_bbox: Option<(f32, f32, f32, f32)>,
    font_size: f32,
    is_bold: bool,
    is_italic: bool,
) -> types::PdfParagraph {
    const DEFAULT_LINE_WIDTH: f32 = 100.0;

    let line_height = block_bbox
        .map(|(_, bottom, _, top)| (top - bottom) / line_count as f32)
        .unwrap_or(DEFAULT_OCR_FONT_SIZE_PT);
    let line_bbox = block_bbox.map(|(left, _bottom, right, top)| {
        let line_top = top - line_index as f32 * line_height;
        (left, line_top - line_height, right, line_top)
    });
    let (x, baseline_y, width) = line_bbox
        .map(|(left, bottom, right, _)| (left, bottom, right - left))
        .unwrap_or((0.0, 0.0, DEFAULT_LINE_WIDTH));
    let lines = if text.trim().is_empty() {
        Vec::new()
    } else {
        vec![make_ocr_pdf_line(
            text,
            x,
            baseline_y,
            width,
            line_height,
            font_size,
            is_bold,
            is_italic,
        )]
    };
    make_ocr_paragraph(text.to_string(), lines, line_bbox, font_size)
}

#[cfg(all(feature = "ocr", feature = "layout-detection"))]
fn compatible_hint_indices(
    lines: &[types::PdfParagraph],
    hints: &[types::LayoutHint],
    selected: Vec<Option<usize>>,
    min_containment: f32,
) -> Vec<Option<usize>> {
    let mut compatible = vec![None; lines.len()];
    let mut previous_list_hint = None;
    for (index, line) in lines.iter().enumerate() {
        let actual = selected[index].filter(|&hint_index| {
            hints
                .get(hint_index)
                .is_some_and(|hint| classification_matches_hint(line, hint.class_name))
        });
        compatible[index] = actual
            .or_else(|| inherit_list_continuation(line, selected[index], previous_list_hint, hints, min_containment));
        previous_list_hint = compatible[index].filter(|&hint_index| {
            hints[hint_index].class_name == types::LayoutHintClass::ListItem && !line.text.trim().is_empty()
        });
    }
    compatible
}

#[cfg(all(feature = "ocr", feature = "layout-detection"))]
fn classification_matches_hint(paragraph: &types::PdfParagraph, class_name: types::LayoutHintClass) -> bool {
    use types::LayoutHintClass as L;
    if paragraph.layout_class != Some(class_name) {
        return false;
    }
    match class_name {
        L::Title | L::SectionHeader => paragraph.heading_level.is_some(),
        L::Code => paragraph.is_code_block,
        L::Formula => paragraph.is_formula,
        L::ListItem => paragraph.is_list_item,
        L::Text => true,
        L::Caption | L::Footnote => true,
        L::PageHeader | L::PageFooter | L::Picture => paragraph.is_page_furniture,
        _ => false,
    }
}

#[cfg(all(feature = "ocr", feature = "layout-detection"))]
fn inherit_list_continuation(
    paragraph: &types::PdfParagraph,
    selected_hint: Option<usize>,
    previous_list_hint: Option<usize>,
    hints: &[types::LayoutHint],
    min_containment: f32,
) -> Option<usize> {
    let hint_index = previous_list_hint?;
    let hint = hints.get(hint_index)?;
    (selected_hint.is_none()
        && !paragraph.text.trim().is_empty()
        && hint_containment(paragraph.block_bbox?, hint) >= min_containment)
        .then_some(hint_index)
}

#[cfg(all(feature = "ocr", feature = "layout-detection"))]
fn hint_containment(bbox: (f32, f32, f32, f32), hint: &types::LayoutHint) -> f32 {
    let intersection_width = (bbox.2.min(hint.right) - bbox.0.max(hint.left)).max(0.0);
    let intersection_height = (bbox.3.min(hint.top) - bbox.1.max(hint.bottom)).max(0.0);
    let paragraph_area = (bbox.2 - bbox.0).max(0.0) * (bbox.3 - bbox.1).max(0.0);
    if paragraph_area > 0.0 {
        intersection_width * intersection_height / paragraph_area
    } else {
        0.0
    }
}

#[cfg(all(test, feature = "ocr", feature = "layout-detection"))]
fn regroup_layout_lines(lines: Vec<types::PdfParagraph>, hint_indices: Vec<Option<usize>>) -> Vec<types::PdfParagraph> {
    let element_indices = vec![0; lines.len()];
    let block_ids = vec![None; lines.len()];
    regroup_layout_lines_by_element(lines, hint_indices, element_indices, block_ids)
}

#[cfg(all(feature = "ocr", feature = "layout-detection"))]
fn regroup_layout_lines_by_element(
    lines: Vec<types::PdfParagraph>,
    hint_indices: Vec<Option<usize>>,
    element_indices: Vec<usize>,
    block_ids: Vec<Option<String>>,
) -> Vec<types::PdfParagraph> {
    let mut result = Vec::new();
    let mut body_lines = Vec::new();
    let mut body_region = None;
    let mut groups = group_by_hint(lines, hint_indices, element_indices, block_ids);

    for group in groups.drain(..) {
        if group.lines.iter().any(has_structural_override) {
            push_body_group(&mut result, std::mem::take(&mut body_lines));
            body_region = None;
            if let Some(paragraph) = merge_structural_group(group.lines) {
                result.push(paragraph);
            }
        } else {
            let lines_are_near = body_lines
                .last()
                .zip(group.lines.first())
                .is_some_and(|(previous, current)| layout_lines_are_near(previous, current));
            let same_region = lines_are_near
                && body_region
                    .as_ref()
                    .is_some_and(|(hint_index, element_index, block_id)| {
                        (group.hint_index.is_some() && *hint_index == group.hint_index)
                            || *element_index == group.element_index
                            || (group.block_id.is_some() && *block_id == group.block_id)
                    });
            if !body_lines.is_empty() && !same_region {
                push_body_group(&mut result, std::mem::take(&mut body_lines));
            }
            if body_lines.is_empty() {
                body_region = Some((group.hint_index, group.element_index, group.block_id));
            }
            body_lines.extend(group.lines);
        }
    }
    push_body_group(&mut result, body_lines);
    result
}

#[cfg(all(feature = "ocr", feature = "layout-detection"))]
struct LayoutLineGroup {
    hint_index: Option<usize>,
    element_index: usize,
    block_id: Option<String>,
    lines: Vec<types::PdfParagraph>,
}

#[cfg(all(feature = "ocr", feature = "layout-detection"))]
fn group_by_hint(
    lines: Vec<types::PdfParagraph>,
    hint_indices: Vec<Option<usize>>,
    element_indices: Vec<usize>,
    block_ids: Vec<Option<String>>,
) -> Vec<LayoutLineGroup> {
    let mut groups: Vec<LayoutLineGroup> = Vec::new();
    for (((line, hint_index), element_index), block_id) in
        lines.into_iter().zip(hint_indices).zip(element_indices).zip(block_ids)
    {
        if let Some(group) = groups.last_mut()
            && hint_index.is_some()
            && group.hint_index == hint_index
            && group
                .lines
                .last()
                .is_some_and(|previous| layout_lines_are_near(previous, &line))
        {
            group.lines.push(line);
        } else {
            groups.push(LayoutLineGroup {
                hint_index,
                element_index,
                block_id,
                lines: vec![line],
            });
        }
    }
    groups
}

#[cfg(all(feature = "ocr", feature = "layout-detection"))]
fn layout_lines_are_near(previous: &types::PdfParagraph, current: &types::PdfParagraph) -> bool {
    const MAX_GAP_IN_LINE_HEIGHTS: f32 = 1.5;

    let (Some(previous_bbox), Some(current_bbox)) = (previous.block_bbox, current.block_bbox) else {
        return false;
    };
    let previous_height = (previous_bbox.3 - previous_bbox.1).abs();
    let current_height = (current_bbox.3 - current_bbox.1).abs();
    let line_height = previous_height.max(current_height).max(1.0);
    let vertical_gap = if current_bbox.3 < previous_bbox.1 {
        previous_bbox.1 - current_bbox.3
    } else if previous_bbox.3 < current_bbox.1 {
        current_bbox.1 - previous_bbox.3
    } else {
        0.0
    };
    vertical_gap <= line_height * MAX_GAP_IN_LINE_HEIGHTS
}

#[cfg(all(feature = "ocr", feature = "layout-detection"))]
fn has_structural_override(paragraph: &types::PdfParagraph) -> bool {
    !paragraph.text.trim().is_empty()
        && (paragraph.heading_level.is_some()
            || paragraph.is_list_item
            || paragraph.is_code_block
            || paragraph.is_formula
            || paragraph.is_page_furniture
            || matches!(
                paragraph.layout_class,
                Some(types::LayoutHintClass::Caption | types::LayoutHintClass::Footnote)
            ))
}

/// A merged body group is only split into separate paragraphs once it contains at
/// least this many marker-opening lines. A single marker-opening line is not
/// enough signal: ordinary prose regularly *starts* with something that parses as
/// a list marker (e.g. a lead-in sentence "1. Overview covers..."), and splitting
/// on that alone would carve one such paragraph into a spurious empty lead segment
/// plus itself. Two or more independent marker openings in the same merged group
/// is the point where "this is a list" stops being a guess.
///
/// Shared by both the layout route (`push_body_group`, below) and the non-layout
/// route (`make_ocr_block_paragraphs`): the layout-detection feature requirement was
/// incidental, not deliberate -- this predicate is pure text, needs no layout input,
/// and #713 found the non-layout OCR routes had no marker-splitting at all.
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
const MIN_LIST_MARKERS_TO_SPLIT: usize = 2;

/// Flush an accumulated run of OCR-line paragraphs from the same layout region.
///
/// A layout element spanning a whole list (see `regroup_layout_lines_by_element`'s
/// `same_region` test, which never flushes mid-element because its middle OR-arm
/// is true for every line of the same hOCR element) previously became ONE
/// paragraph regardless of how many list markers it contained, so only the first
/// marker in the run was ever visible to the downstream `looks_like_list_item`
/// classifier -- items 2..N were swallowed into item 1's body. This splits the run
/// at each marker-opening line once there are at least [`MIN_LIST_MARKERS_TO_SPLIT`]
/// of them, so every item gets its own paragraph; a lone marker-opening line (the
/// common false-positive case) still produces exactly one paragraph, unchanged
/// from before.
#[cfg(all(feature = "ocr", feature = "layout-detection"))]
fn push_body_group(result: &mut Vec<types::PdfParagraph>, lines: Vec<types::PdfParagraph>) {
    let lines = trim_blank_boundaries(lines);
    if lines.is_empty() {
        return;
    }
    for segment in split_body_group_at_list_markers(lines) {
        if let Some(paragraph) = build_body_paragraph(segment) {
            result.push(paragraph);
        }
    }
}

/// Split `lines` into segments at each line for which `looks_like_list_item` is
/// true, provided there are at least [`MIN_LIST_MARKERS_TO_SPLIT`] such lines.
/// Lines before the first marker (e.g. a lead-in sentence) form their own leading
/// segment rather than being absorbed into the first item; a wrapped continuation
/// line carries no marker of its own and so stays in the segment it already
/// belongs to. Below the threshold, `lines` is returned unsplit as the sole
/// segment. Segments are built forward into a `Vec<Vec<_>>` in original order --
/// no reversal, no `split_off`.
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
fn split_body_group_at_list_markers(lines: Vec<types::PdfParagraph>) -> Vec<Vec<types::PdfParagraph>> {
    let marker_indices = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| super::pipeline::looks_like_list_item(&line.text))
        .map(|(index, _)| index)
        .collect::<Vec<_>>();

    if marker_indices.len() < MIN_LIST_MARKERS_TO_SPLIT {
        return vec![lines];
    }

    let mut boundaries = marker_indices;
    if boundaries.first() != Some(&0) {
        boundaries.insert(0, 0);
    }

    let mut slots = lines.into_iter().map(Some).collect::<Vec<_>>();
    boundaries
        .iter()
        .enumerate()
        .map(|(window_index, &start)| {
            let end = boundaries.get(window_index + 1).copied().unwrap_or(slots.len());
            slots[start..end]
                .iter_mut()
                .filter_map(Option::take)
                .collect::<Vec<_>>()
        })
        .filter(|segment| !segment.is_empty())
        .collect()
}

/// Join a body segment's lines into paragraph text.
///
/// A segment whose first line satisfies `looks_like_list_item` is marker-led: its
/// remaining lines are wrapped continuations of one logical item, so they join
/// with a single space, not a newline. Newline-joining a marker-led segment looks
/// harmless in isolation, but `apply_ocr_text_list_fallback`
/// (`extractors::pdf::ocr`) flags any paragraph whose text starts with a list
/// marker as `is_list_item = true` regardless of how many lines it has, and the
/// markdown renderer then treats an embedded `"\n"` inside a list-item paragraph
/// as a block break -- so a wrapped continuation line came out as its own
/// standalone paragraph in production markdown. Non-marker-led segments (plain
/// prose) keep the long-standing `"\n"` join, unchanged.
///
/// Each line is trimmed before joining, and empty lines are dropped, so a line
/// with trailing/leading whitespace (or a rare interior blank line) can't turn
/// into a double space.
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
fn join_body_segment_text(lines: &[types::PdfParagraph]) -> String {
    let marker_led = lines
        .first()
        .is_some_and(|line| super::pipeline::looks_like_list_item(&line.text));
    if marker_led {
        lines
            .iter()
            .map(|line| line.text.trim())
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join(" ")
    } else {
        lines
            .iter()
            .map(|line| line.text.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// Build one merged body paragraph from a (possibly split) run of OCR-line
/// paragraphs -- the construction `push_body_group` always used, extracted so it
/// applies identically to every segment `split_body_group_at_list_markers`
/// produces: `pdf_lines` is collected only from this segment's lines, `block_bbox`
/// unions only this segment's boxes, and `layout_class` takes the first non-`None`
/// value within this segment (not the whole original group).
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
fn build_body_paragraph(lines: Vec<types::PdfParagraph>) -> Option<types::PdfParagraph> {
    let text = join_body_segment_text(&lines);
    if text.trim().is_empty() {
        return None;
    }
    let bbox = union_bboxes(&lines);
    let layout_class = lines.iter().find_map(|line| line.layout_class);
    let font_size = lines.iter().map(|line| line.dominant_font_size).fold(0.0_f32, f32::max);
    let font_size = if font_size > 0.0 {
        font_size
    } else {
        DEFAULT_OCR_FONT_SIZE_PT
    };
    let pdf_lines = lines.into_iter().flat_map(|line| line.lines).collect();
    let mut paragraph = make_ocr_paragraph(text, pdf_lines, bbox, font_size);
    paragraph.layout_class = layout_class;
    Some(paragraph)
}

#[cfg(all(feature = "ocr", feature = "layout-detection"))]
fn merge_structural_group(lines: Vec<types::PdfParagraph>) -> Option<types::PdfParagraph> {
    let lines = trim_blank_boundaries(lines);
    let template = lines.iter().find(|line| has_structural_override(line))?.clone();
    let text = lines
        .iter()
        .map(|line| line.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let bbox = union_bboxes(&lines);
    let pdf_lines = lines.into_iter().flat_map(|line| line.lines).collect::<Vec<_>>();
    let mut merged = template;
    merged.word_count = types::PdfParagraph::compute_word_count(&text, &pdf_lines);
    merged.text = text;
    merged.lines = pdf_lines;
    merged.block_bbox = bbox;
    Some(merged)
}

// Both call sites -- `push_body_group` and `merge_structural_group` -- are gated on
// `layout-detection` as well, so the looser `any(ocr, ocr-pipeline)` gate this used to carry
// left the function compiled with zero callers on the `ocr`-without-`layout-detection` leg,
// which is one of the narrow feature legs CI now checks independently (a167989b2e). ~keep
#[cfg(all(feature = "ocr", feature = "layout-detection"))]
fn trim_blank_boundaries(mut lines: Vec<types::PdfParagraph>) -> Vec<types::PdfParagraph> {
    let first_content = lines
        .iter()
        .position(|line| !line.text.trim().is_empty())
        .unwrap_or(lines.len());
    let retained = lines[first_content..]
        .iter()
        .rposition(|line| !line.text.trim().is_empty())
        .map_or(0, |index| index + 1);
    lines.drain(..first_content);
    lines.truncate(retained);
    lines
}

#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
fn union_bboxes(lines: &[types::PdfParagraph]) -> Option<(f32, f32, f32, f32)> {
    lines
        .iter()
        .filter_map(|line| line.block_bbox)
        .reduce(|a, b| (a.0.min(b.0), a.1.min(b.1), a.2.max(b.2), a.3.max(b.3)))
}

/// Master switch for [`reattach_ocr_layout_list_markers`].
///
/// Independent of `pipeline::REATTACH_DETACHED_LIST_MARKERS`, which gates the
/// native structure-tree pass and is under separate evaluation -- flip this one
/// alone to build a control binary that differs from the shipped OCR layout route
/// only in this behaviour, without touching the native pass at all.
#[cfg(all(feature = "ocr", feature = "layout-detection"))]
const REATTACH_OCR_LAYOUT_LIST_MARKERS: bool = true;

/// A/B switch for the marker-run/body-run pairing phase of
/// [`reattach_ocr_layout_list_markers`] (#729).
///
/// Independent of [`REATTACH_OCR_LAYOUT_LIST_MARKERS`], which gates the
/// single-marker/adjacent-body pass above -- flip either alone to evaluate the
/// two passes separately. The single-marker pass pairs by *baseline*, which
/// can never match a stacked marker column against a stacked body column (see
/// [`DETACHED_MARKER_RUN_MIN_LENGTH`]'s doc comment); this second phase pairs
/// by *position* instead, to recover that shape.
#[cfg(all(feature = "ocr", feature = "layout-detection"))]
const REATTACH_OCR_LAYOUT_MARKER_RUN_PAIRS: bool = true;

/// Minimum number of consecutive bare markers required before the marker-run
/// pairing phase (#729) will pair a run at all.
///
/// A lone bare marker is exactly as likely to be an exhibit label (`"B."`
/// immediately before a `"## EXHIBIT B"` heading) as a genuine list-item
/// marker -- `accepts_detached_list_marker`'s heading-level rejection is what
/// correctly keeps that case unpaired, and this run-pairing phase must not
/// re-open it by treating a single marker as sufficient signal on its own.
/// Requiring at least two consecutive markers is the only evidence that
/// distinguishes a real marker column ("(a)", "(b)", "(c)", ...) from an
/// isolated, ambiguous bare marker. Precision is the scarce resource on this
/// corpus: a prior rewrite of a related detached-marker pass went from 50.39
/// to 47.64 GT F1 with zero files improved and was rejected on exactly that
/// basis.
#[cfg(all(feature = "ocr", feature = "layout-detection"))]
const DETACHED_MARKER_RUN_MIN_LENGTH: usize = 2;

/// Reattach an OCR layout-route list marker that `regroup_layout_lines_by_element`
/// isolated into its own paragraph back onto the body paragraph it belongs to.
/// (#729)
///
/// `merge_structural_group` flushes and isolates any group containing a line an ML
/// `ListItem` hint fired on (`has_structural_override`), so a marker-only OCR line
/// -- "1." alone on its own hOCR line/element -- comes out as a standalone
/// paragraph with `is_list_item = true` and no body text, while its body
/// accumulates separately as an unflagged plain paragraph (`push_body_group`).
/// Nothing rejoins them: the native fixup
/// (`pipeline::reattach_detached_list_markers`) runs only when
/// `heuristically_restructured_ocr_pages` (`extractors::pdf::ocr`) reaches the
/// structure-tree pipeline's heuristic branch, and on the layout route that
/// function's own "already structured" gate is essentially always tripped before
/// this point (the ML hints that caused the split already set `is_list_item`
/// elsewhere), so the native pass is skipped for this route entirely.
///
/// Deliberately NOT the native pass with a loosened precondition: a native
/// detached marker is unclassified prose that merely happens to be marker-shaped
/// (`is_list_item == false`), so `pipeline::detached_list_marker` rejects any
/// paragraph already carrying `is_list_item`. Here the opposite is true -- the
/// marker paragraph is detached *because* an ML hint already classified it, so it
/// is `is_list_item == true` by construction (`layout_classify::apply_hint_to_paragraph`,
/// `LayoutHintClass::ListItem if hint.confidence >= 0.8`). Accepting that flag on
/// the native predicate would also accept a genuine, complete, single-marker list
/// item in the structure-tree route (e.g. a bare "-" that IS the whole item) --
/// a real regression risk in code under separate evaluation that this task does
/// not touch. The body-side test (`pipeline::accepts_detached_list_marker`) has no
/// such conflict and is reused, given `page_rotation_degrees` (the page's PDF
/// `/Rotate`) so it compares geometry in the OCR-corrected frame (#760) instead
/// of the native one -- see `pipeline::DetachedMarkerFrame`.
///
/// Runs directly on the per-page paragraphs `ocr_doc_to_layout_paragraphs`
/// produces, inside `extractors::pdf::ocr::assemble_ocr_page_paragraphs`, right
/// after `apply_ocr_text_list_fallback` and before that function returns --
/// before `segments_from_ocr_pages` flattens paragraph boundaries away, after
/// which a marker and its body are just two entries in one flat per-page stream
/// with no paragraph grouping left to exploit.
///
/// Unlike the native pass, this rebuilds `body.text` directly instead of clearing
/// it: `assemble_ocr_page_paragraphs` has no `synchronize_paragraph_text_metadata`
/// call after it to repopulate an emptied `.text`, and the eventual renderer
/// (`assembly::push_paragraph_element`) reads `.text` directly whenever it is
/// non-empty. Reconstructing from `.lines` on an empty `.text` is untested for
/// "a marker was just prepended onto a multi-line list item" -- an embedded `"\n"`
/// inside a list item's text becomes a spurious block break in the markdown
/// renderer, the exact defect `join_body_segment_text`'s own doc comment
/// documents as a real production bug for a sibling code path. Space-joining every
/// line's segment text mirrors that existing marker-led convention instead.
#[cfg(all(feature = "ocr", feature = "layout-detection"))]
pub(crate) fn reattach_ocr_layout_list_markers(paragraphs: &mut Vec<types::PdfParagraph>, page_rotation_degrees: u32) {
    if !REATTACH_OCR_LAYOUT_LIST_MARKERS || paragraphs.len() < 2 {
        return;
    }

    let frame = super::pipeline::DetachedMarkerFrame::OcrOnPage(page_rotation_degrees);
    let mut consumed = vec![false; paragraphs.len()];
    let mut pairs: Vec<(usize, usize)> = Vec::new();

    for marker_index in 0..paragraphs.len() {
        if consumed[marker_index] {
            continue;
        }
        let Some(marker) = ocr_detached_list_marker(&paragraphs[marker_index]) else {
            continue;
        };
        let limit = (marker_index + 1 + super::pipeline::DETACHED_MARKER_MAX_LOOKAHEAD).min(paragraphs.len());
        let body_index = (marker_index + 1..limit).find(|&candidate| {
            !consumed[candidate]
                && super::pipeline::accepts_detached_list_marker(&paragraphs[candidate], &marker, frame)
        });
        let Some(body_index) = body_index else {
            continue;
        };
        consumed[marker_index] = true;
        consumed[body_index] = true;
        pairs.push((marker_index, body_index));
    }

    // Second phase (#729): pair a whole marker RUN with the whole body RUN that
    // immediately follows it, positionally. The pass above pairs by baseline, which
    // can never match this shape -- six vertically stacked paragraphs, three bare
    // markers ("(a)", "(b)", "(c)") followed by three body paragraphs in the same
    // order -- because ordinary single-spaced leading (~1.15-1.5x font size) is more
    // than double `DETACHED_MARKER_BASELINE_TOLERANCE_FONT_FACTOR` (0.6), so no
    // marker and no body in this shape ever shares a baseline with anything. What
    // proves the pairing here is POSITION, not geometry: the nth marker in a maximal
    // marker run pairs with the nth paragraph of the maximal body run that follows
    // it, provided the body run is at least as long as the marker run (never a
    // partial pairing) and every resulting pair agrees on rotation frame.
    if REATTACH_OCR_LAYOUT_MARKER_RUN_PAIRS {
        let mut index = 0usize;
        while index < paragraphs.len() {
            if consumed[index] || ocr_detached_list_marker(&paragraphs[index]).is_none() {
                index += 1;
                continue;
            }

            let mut marker_indices = vec![index];
            let mut cursor = index + 1;
            while cursor < paragraphs.len()
                && !consumed[cursor]
                && ocr_detached_list_marker(&paragraphs[cursor]).is_some()
            {
                marker_indices.push(cursor);
                cursor += 1;
            }

            if marker_indices.len() < DETACHED_MARKER_RUN_MIN_LENGTH {
                index = cursor;
                continue;
            }

            let mut body_indices = Vec::new();
            let mut body_cursor = cursor;
            while body_cursor < paragraphs.len()
                && !consumed[body_cursor]
                && accepts_marker_run_body(&paragraphs[body_cursor])
            {
                body_indices.push(body_cursor);
                body_cursor += 1;
            }

            if body_indices.len() < marker_indices.len() {
                index = body_cursor.max(cursor);
                continue;
            }
            body_indices.truncate(marker_indices.len());

            let rotation_agrees = marker_indices
                .iter()
                .zip(body_indices.iter())
                .all(|(&marker_idx, &body_idx)| {
                    let marker_segment = paragraphs[marker_idx]
                        .lines
                        .first()
                        .and_then(|line| line.segments.first());
                    let body_segment = paragraphs[body_idx]
                        .lines
                        .first()
                        .and_then(|line| line.segments.first());
                    match (marker_segment, body_segment) {
                        (Some(marker_segment), Some(body_segment)) => body_segment.has_same_rotation(marker_segment),
                        _ => false,
                    }
                });

            if rotation_agrees {
                for (&marker_idx, &body_idx) in marker_indices.iter().zip(body_indices.iter()) {
                    consumed[marker_idx] = true;
                    consumed[body_idx] = true;
                    pairs.push((marker_idx, body_idx));
                }
            }

            index = body_cursor;
        }
    }

    if pairs.is_empty() {
        return;
    }

    for (marker_index, body_index) in &pairs {
        let Some(marker_segment) = paragraphs[*marker_index]
            .lines
            .first()
            .and_then(|line| line.segments.first())
            .cloned()
        else {
            continue;
        };
        let marker_bbox = paragraphs[*marker_index].block_bbox;
        let body = &mut paragraphs[*body_index];
        if let Some(line) = body.lines.first_mut() {
            line.segments.insert(0, marker_segment);
        }
        body.is_list_item = true;
        body.layout_class = Some(types::LayoutHintClass::ListItem);
        body.block_bbox = match (body.block_bbox, marker_bbox) {
            (Some(body_bbox), Some(marker_bbox)) => Some((
                body_bbox.0.min(marker_bbox.0),
                body_bbox.1.min(marker_bbox.1),
                body_bbox.2.max(marker_bbox.2),
                body_bbox.3.max(marker_bbox.3),
            )),
            (bbox @ Some(_), None) | (None, bbox @ Some(_)) => bbox,
            (None, None) => None,
        };
        body.text = body
            .lines
            .iter()
            .map(|line| {
                line.segments
                    .iter()
                    .map(|segment| segment.text.trim())
                    .filter(|text| !text.is_empty())
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        body.word_count = types::PdfParagraph::compute_word_count(&body.text, &body.lines);
    }

    let mut index = 0usize;
    paragraphs.retain(|_| {
        let keep = !pairs.iter().any(|(marker_index, _)| *marker_index == index);
        index += 1;
        keep
    });
}

/// Reattach a genuinely UNCLASSIFIED bare OCR list marker -- `is_list_item ==
/// false`, because no ML layout hint ever fired on it -- to the body paragraph it
/// belongs to. (#729)
///
/// Delegates to the native pipeline's own
/// [`super::pipeline::reattach_detached_list_markers`], passing
/// [`super::pipeline::DetachedMarkerFrame::OcrOnPage`] with this page's PDF
/// `/Rotate` value (#760): that function's own geometry test is expressed
/// relative to font size specifically so it "behaves identically" on OCR's
/// raster-derived paragraph geometry and native PDF points, but on a *rotated*
/// OCR page the raw baseline/advance geometry sits in the raster frame, not the
/// upright one, and OCR segments cannot carry the correction on
/// `rotation_degrees` the way native rotated text does -- see
/// `DetachedMarkerFrame`'s doc comment for why. Nothing on any OCR call path
/// actually invoked this function before #729 -- its only real-world
/// reachability from OCR was through `heuristically_restructured_ocr_pages`,
/// whose document-wide "already structured" gate is tripped by the very ML
/// classifications that make list-marker recovery necessary in the first place,
/// so in practice it ran natively only.
///
/// This is the mirror image of [`reattach_ocr_layout_list_markers`], not a
/// duplicate of it. That function's marker-side test (`ocr_detached_list_marker`)
/// requires `is_list_item == true` -- proof an ML hint already fired on the marker
/// -- and pairs a whole marker run against a whole body run by position. This
/// function's marker-side test (`pipeline::detached_list_marker`) requires the
/// OPPOSITE, `is_list_item == false`, and pairs a single marker to a single body by
/// baseline instead: it covers the shape `reattach_ocr_layout_list_markers` cannot
/// reach at all -- no hint ever fired on the marker (missing detection, confidence
/// below 0.8, or `layout-detection` disabled entirely) -- so the marker paragraph
/// is not `is_list_item == true` by construction the way that function's
/// precondition requires. Without this pass such a marker stays a bare, unmerged
/// paragraph and renders as literal marker text (e.g. `"(a)"`) in the output.
///
/// Never touches `heading_level` (`pipeline::reattach_detached_list_markers` only
/// ever sets `is_list_item`), so -- like `apply_ocr_text_list_fallback` in
/// `extractors::pdf::ocr` -- it cannot trip
/// `heuristically_restructured_ocr_pages`'s "already structured" gate or regress
/// heading detection: callers run it entirely outside that gate, as a fallback
/// pass alongside `apply_ocr_text_list_fallback`, never before it.
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
pub(crate) fn reattach_detached_ocr_list_markers(
    paragraphs: &mut Vec<types::PdfParagraph>,
    page_rotation_degrees: u32,
) {
    super::pipeline::reattach_detached_list_markers(
        paragraphs,
        super::pipeline::DetachedMarkerFrame::OcrOnPage(page_rotation_degrees),
    );
}

/// The lone segment of an OCR layout-route paragraph that is nothing but a list
/// marker AND that an ML `ListItem` hint already classified as such.
///
/// See [`reattach_ocr_layout_list_markers`]'s doc comment for why this
/// deliberately requires `is_list_item == true` -- the mirror image of
/// `pipeline::detached_list_marker`'s precondition, not a copy of it.
#[cfg(all(feature = "ocr", feature = "layout-detection"))]
fn ocr_detached_list_marker(paragraph: &types::PdfParagraph) -> Option<crate::pdf::hierarchy::SegmentData> {
    if !paragraph.is_list_item || paragraph.is_code_block || paragraph.is_formula || paragraph.is_page_furniture {
        return None;
    }
    let [line] = paragraph.lines.as_slice() else {
        return None;
    };
    let [segment] = line.segments.as_slice() else {
        return None;
    };
    if !super::pipeline::is_bare_detached_list_marker(&segment.text) {
        return None;
    }
    let geometry_is_usable = segment.x.is_finite()
        && segment.width.is_finite()
        && segment.width >= 0.0
        && segment.font_size.is_finite()
        && segment.font_size > 0.0
        && segment.upright_baseline().is_finite();
    geometry_is_usable.then(|| segment.clone())
}

/// Body-side test for the marker-run/body-run pairing phase of
/// [`reattach_ocr_layout_list_markers`] (#729).
///
/// Applies every SEMANTIC guard of `pipeline::accepts_detached_list_marker` --
/// not already a heading, list item, code block, formula, or page furniture;
/// first line not itself marker-shaped; at least
/// [`super::pipeline::DETACHED_MARKER_MIN_BODY_WORDS`] words -- but
/// deliberately NOT that function's baseline/indent geometry. A stacked marker
/// run and the stacked body run that follows it live on different baselines by
/// construction: "(a)", "(b)", "(c)" each own their own baseline, and
/// body1/body2/body3 sit on baselines below all three, so the single-marker
/// geometric test can never pass for this shape -- that mismatch is exactly
/// why this second phase exists, and re-applying that geometry here would
/// reject every candidate it is meant to accept. What proves the pairing
/// instead is POSITION (nth marker in the run pairs with nth body in the
/// immediately following run) -- established by the caller, not by this
/// predicate. Rotation agreement is checked separately by the caller, per
/// resulting pair, once the run lengths are known.
#[cfg(all(feature = "ocr", feature = "layout-detection"))]
fn accepts_marker_run_body(paragraph: &types::PdfParagraph) -> bool {
    if paragraph.heading_level.is_some()
        || paragraph.is_list_item
        || paragraph.is_code_block
        || paragraph.is_formula
        || paragraph.is_page_furniture
    {
        return false;
    }
    let Some(first_line) = paragraph.lines.first() else {
        return false;
    };
    if first_line.segments.is_empty() {
        return false;
    }
    let first_line_text = first_line
        .segments
        .iter()
        .map(|segment| segment.text.trim())
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    if super::pipeline::looks_like_list_item(&first_line_text) || super::pipeline::is_bare_list_marker(&first_line_text)
    {
        return false;
    }

    let body_words = paragraph
        .lines
        .iter()
        .flat_map(|line| line.segments.iter())
        .flat_map(|segment| segment.text.split_whitespace())
        .count();
    body_words >= super::pipeline::DETACHED_MARKER_MIN_BODY_WORDS
}

#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
fn make_ocr_paragraph(
    text: String,
    lines: Vec<types::PdfLine>,
    block_bbox: Option<(f32, f32, f32, f32)>,
    font_size: f32,
) -> types::PdfParagraph {
    types::PdfParagraph {
        word_count: types::PdfParagraph::compute_word_count(&text, &lines),
        text,
        lines,
        dominant_font_size: font_size,
        heading_level: None,
        is_bold: false,
        is_list_item: false,
        is_code_block: false,
        is_formula: false,
        is_page_furniture: false,
        layout_class: None,
        layout_region_path: None,
        caption_for: None,
        block_bbox,
    }
}

#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
#[allow(clippy::too_many_arguments)]
fn make_ocr_pdf_line(
    text: &str,
    x: f32,
    baseline_y: f32,
    width: f32,
    line_height: f32,
    font_size: f32,
    is_bold: bool,
    is_italic: bool,
) -> types::PdfLine {
    let segment = crate::pdf::hierarchy::SegmentData {
        text: text.to_string(),
        x,
        y: baseline_y,
        width,
        height: line_height,
        font_size,
        is_bold,
        is_italic,
        is_monospace: false,
        baseline_y,
        rotation_degrees: 0.0,
        assigned_role: None,
    };
    types::PdfLine {
        segments: vec![segment],
        baseline_y,
        dominant_font_size: font_size,
        is_bold,
        is_monospace: false,
    }
}

#[cfg(all(feature = "ocr", test))]
mod tests {
    use super::*;
    use crate::types::extraction::BoundingBox;
    use crate::types::internal::{ElementKind, InternalDocument, InternalElement};
    use crate::types::ocr_elements::OcrElementLevel;

    /// The three hOCR attribute keys this module reads are declared here as literals
    /// rather than imported, because `ocr::hocr_parser` is behind `feature = "ocr"`
    /// while this module is not. That duplication can silently drift: if the parser
    /// renamed a key, every lookup here would simply miss and fall back to its
    /// default, and no fallback test would fail. This pins the two copies together.
    #[test]
    fn test_hocr_attribute_keys_match_the_parser_that_writes_them() {
        use crate::ocr::hocr_parser::{
            HOCR_BOLD_FRACTION_ATTRIBUTE as PARSER_BOLD, HOCR_FONT_SIZE_ATTRIBUTE as PARSER_FONT_SIZE,
            HOCR_ITALIC_FRACTION_ATTRIBUTE as PARSER_ITALIC,
        };

        assert_eq!(
            HOCR_FONT_SIZE_ATTRIBUTE, PARSER_FONT_SIZE,
            "font-size attribute key drifted from the parser"
        );
        assert_eq!(
            HOCR_BOLD_FRACTION_ATTRIBUTE, PARSER_BOLD,
            "bold-fraction attribute key drifted from the parser"
        );
        assert_eq!(
            HOCR_ITALIC_FRACTION_ATTRIBUTE, PARSER_ITALIC,
            "italic-fraction attribute key drifted from the parser"
        );
    }

    #[test]
    fn test_ocr_doc_soft_wrapped_body_stays_one_paragraph() {
        let mut doc = InternalDocument::new("test");
        let mut elem = InternalElement::text(
            ElementKind::OcrText {
                level: OcrElementLevel::Block,
            },
            "First soft-wrapped body line\ncontinues on the next visual line",
            0,
        );
        elem.bbox = Some(BoundingBox {
            x0: 10.0,
            y0: 10.0,
            x1: 200.0,
            y1: 50.0,
        });
        doc.push_element(elem);

        let paragraphs = ocr_doc_to_paragraphs(&doc, 1000, OcrFontSizeScale::uniform(1.0));

        assert_eq!(paragraphs.len(), 1);
        assert_eq!(
            paragraphs[0].text,
            "First soft-wrapped body line\ncontinues on the next visual line"
        );
        assert_eq!(paragraphs[0].lines.len(), 2);
    }

    /// #713: the non-layout OCR route (`ocr_doc_to_paragraphs`, used whenever
    /// `--layout` is off) had no marker-splitting logic at all, unlike the ML-layout
    /// route's `push_body_group`/`split_body_group_at_list_markers`. A single hOCR
    /// block spanning three numbered items -- exactly what Tesseract emits for a
    /// tightly spaced list -- became ONE paragraph, so only the first marker was ever
    /// visible to any downstream classifier and items 2-3 were swallowed into item 1's
    /// body.
    ///
    /// Against unfixed code this asserts `paragraphs.len() == 3` and gets `1`: the
    /// whole block comes back as a single paragraph whose `text` is
    /// `"1. First item\n2. Second item\n3. Third item"`.
    #[test]
    fn test_ocr_doc_to_paragraphs_splits_multi_marker_block_into_separate_list_items() {
        let mut doc = InternalDocument::new("test");
        let mut elem = InternalElement::text(
            ElementKind::OcrText {
                level: OcrElementLevel::Block,
            },
            "1. First item\n2. Second item\n3. Third item",
            0,
        );
        elem.bbox = Some(BoundingBox {
            x0: 10.0,
            y0: 10.0,
            x1: 200.0,
            y1: 100.0,
        });
        doc.push_element(elem);

        let paragraphs = ocr_doc_to_paragraphs(&doc, 1000, OcrFontSizeScale::uniform(1.0));

        assert_eq!(
            paragraphs.len(),
            3,
            "each marker-opening line must become its own paragraph"
        );
        assert_eq!(paragraphs[0].text, "1. First item");
        assert_eq!(paragraphs[1].text, "2. Second item");
        assert_eq!(paragraphs[2].text, "3. Third item");
    }

    /// A single marker-opening line is not enough signal to split (an ordinary
    /// lead-in sentence like "1. Overview covers..." also matches the marker shape) --
    /// mirrors `push_body_group`'s own "unchanged from before" guarantee for the
    /// ML-layout route. This passes both before and after the #713 fix; it exists to
    /// pin the below-threshold behavior so a future change to
    /// `MIN_LIST_MARKERS_TO_SPLIT` (or the splitting wiring) cannot silently start
    /// splitting on a lone marker.
    #[test]
    fn test_ocr_doc_to_paragraphs_does_not_split_a_lone_marker_opening_line() {
        let mut doc = InternalDocument::new("test");
        let mut elem = InternalElement::text(
            ElementKind::OcrText {
                level: OcrElementLevel::Block,
            },
            "1. Overview covers the whole exhibit\nand continues describing it here",
            0,
        );
        elem.bbox = Some(BoundingBox {
            x0: 10.0,
            y0: 10.0,
            x1: 200.0,
            y1: 100.0,
        });
        doc.push_element(elem);

        let paragraphs = ocr_doc_to_paragraphs(&doc, 1000, OcrFontSizeScale::uniform(1.0));

        assert_eq!(paragraphs.len(), 1);
        assert_eq!(
            paragraphs[0].text,
            "1. Overview covers the whole exhibit\nand continues describing it here"
        );
    }

    /// Regression for the sceptre/paddle heading-detection defect: the bbox-height
    /// font-size proxy used the axis-aligned bounding box's `top - bottom`, which
    /// for a *skewed* quadrilateral also picks up `line_width * sin(skew_angle)` --
    /// a term proportional to how much text is on the line, not to its point size.
    /// On `test_documents/pdf_scanned/ordinance_2197_scanned.pdf` this made resolved
    /// font sizes track word count (13-word title: 470.4pt; 2-word "EXHIBIT B-3":
    /// 74.9pt) instead of the page's two actual type sizes, so every page logged
    /// `headings=0` even though the same document OCR'd with tesseract found 61.
    ///
    /// This quad has a deliberate skew (top edge rises 70px over an 800px run,
    /// left/right edges both 35px tall): the naive AABB height is `205 - 100 =
    /// 105`, three times the quad's true edge-based height of `35`. Against
    /// unfixed code (`resolve_ocr_font_size_pt` reading only `block_bbox`), this
    /// asserts `105.0` and fails.
    #[test]
    fn test_ocr_doc_uses_quad_edge_height_not_skewed_aabb_height_for_font_size() {
        let mut doc = InternalDocument::new("test");
        let mut elem = InternalElement::text(
            ElementKind::OcrText {
                level: OcrElementLevel::Line,
            },
            "AN ORDINANCE OF THE CITY COUNCIL OF THE CITY OF SUGAR LAND",
            0,
        );
        elem.bbox = Some(BoundingBox {
            x0: 100.0,
            y0: 100.0,
            x1: 900.0,
            y1: 205.0,
        });
        elem.ocr_geometry = Some(crate::types::OcrBoundingGeometry::Quadrilateral {
            points: [(100, 100), (900, 170), (900, 205), (100, 135)],
        });
        doc.push_element(elem);

        let paragraphs = ocr_doc_to_paragraphs(&doc, 1000, OcrFontSizeScale::uniform(1.0));

        assert_eq!(paragraphs.len(), 1);
        assert!(
            (paragraphs[0].dominant_font_size - 35.0).abs() < 0.01,
            "expected the skew-robust quad edge height (35.0), got {}",
            paragraphs[0].dominant_font_size
        );
    }

    /// Unit-mismatch regression: `element.bbox` and `element.ocr_geometry` are not
    /// always in the same coordinate unit by the time `resolve_ocr_font_size_pt` runs
    /// (see [`OcrFontSizeScale`]'s doc comment) -- on the mixed OCR route,
    /// `element.bbox` has already been rescaled into PDF points by the caller
    /// (`extractors::pdf::ocr::rescale_ocr_bboxes_to_page_points`) while
    /// `element.ocr_geometry` is still raw OCR raster pixels. A `Quadrilateral`
    /// element (the only geometry shape `quad_edge_height_px` reads) must therefore
    /// have its font size scaled by `OcrFontSizeScale::bbox_already_in_points`'s
    /// `geometry_points_per_pixel`, not left in raw pixels or treated as a no-op.
    ///
    /// The quad here is a straight (unskewed) 200px-tall band -- `quad_edge_height_px`
    /// averages the two side edges, both exactly 200px -- so the correctly-scaled font
    /// size at a 0.36 points-per-pixel ratio (matching the 1700x2200px-over-612x792pt
    /// fixture used elsewhere in this crate) is `200.0 * 0.36 = 72.0`pt.
    ///
    /// Against a caller that collapses `OcrFontSizeScale` back to one flat scale for
    /// both branches (the pre-fix shape of `assemble_mixed_ocr_page_document`, which
    /// called `ocr_doc_to_paragraphs(&doc, page_height, 1.0)` unconditionally), this
    /// asserts `72.0` and fails: the actual value is `200.0`, the raw unscaled
    /// raster-pixel quad-edge height.
    #[test]
    fn test_ocr_doc_scales_quad_geometry_by_its_own_ratio_when_bbox_is_already_in_points() {
        let mut doc = InternalDocument::new("test");
        let mut elem = InternalElement::text(
            ElementKind::OcrText {
                level: OcrElementLevel::Line,
            },
            "SCEPTRE HEADING",
            0,
        );
        // `block_bbox` is deliberately a different height (300pt) from the quad's raw
        // pixel height (200px) below, so a result of `300.0` (or `300.0 * 0.36 =
        // 108.0`) would indicate the bbox-height fallback fired instead of the
        // quad-edge one -- it must not, since `resolve_ocr_font_size_pt` always
        // prefers `ocr_geometry` when present.
        elem.bbox = Some(BoundingBox {
            x0: 100.0,
            y0: 100.0,
            x1: 900.0,
            y1: 400.0,
        });
        elem.ocr_geometry = Some(crate::types::OcrBoundingGeometry::Quadrilateral {
            points: [(100, 100), (900, 100), (900, 300), (100, 300)],
        });
        doc.push_element(elem);

        let font_size_scale = OcrFontSizeScale::bbox_already_in_points(0.36);
        let paragraphs = ocr_doc_to_paragraphs(&doc, 1000, font_size_scale);

        assert_eq!(paragraphs.len(), 1);
        assert!(
            (paragraphs[0].dominant_font_size - 72.0).abs() < 0.01,
            "expected the quad-edge height rescaled by its own points-per-pixel ratio \
             (200px * 0.36 = 72.0pt), got {}",
            paragraphs[0].dominant_font_size
        );
    }

    /// Build a document of single-line OCR elements that all belong to one hOCR
    /// block, each carrying a `Quadrilateral` geometry of the given raster-pixel
    /// height and no `x_fsize` -- the exact shape sceptre and PaddleOCR emit (one
    /// element per *detection box*, several boxes per physical line).
    ///
    /// The quads are unskewed, so `quad_edge_height_px` returns the height verbatim
    /// and, at `OcrFontSizeScale::uniform(1.0)`, each fragment's per-element font
    /// size is exactly its pixel height. That keeps the expected values in the tests
    /// below arithmetic rather than approximate.
    fn quad_fragment_document(block_id: Option<&str>, heights_px: &[u32], x_fsize: Option<&str>) -> InternalDocument {
        let mut doc = InternalDocument::new("test");
        let mut top = 100_u32;
        for (index, &height) in heights_px.iter().enumerate() {
            let mut element = InternalElement::text(
                ElementKind::OcrText {
                    level: OcrElementLevel::Line,
                },
                format!("word{index}"),
                0,
            );
            element.bbox = Some(BoundingBox {
                x0: 100.0,
                y0: f64::from(top),
                x1: 500.0,
                y1: f64::from(top + height),
            });
            element.ocr_geometry = Some(crate::types::OcrBoundingGeometry::Quadrilateral {
                points: [(100, top), (500, top), (500, top + height), (100, top + height)],
            });
            let attributes = block_id
                .map(|block_id| ("hocr_block_id".to_string(), block_id.to_string()))
                .into_iter()
                .chain(x_fsize.map(|value| (HOCR_FONT_SIZE_ATTRIBUTE.to_string(), value.to_string())))
                .collect::<ahash::AHashMap<_, _>>();
            element.attributes = (!attributes.is_empty()).then_some(attributes);
            doc.push_element(element);
            top += height;
        }
        doc
    }

    fn ocr_segment_font_sizes(paragraphs: Vec<types::PdfParagraph>) -> Vec<f32> {
        segments_from_ocr_pages(&[paragraphs])
            .into_iter()
            .flatten()
            .map(|segment| segment.font_size)
            .collect()
    }

    /// #712 proposed block-median font sizing to remove intra-line detection-box
    /// variance, but `f4a9c19b13` reverted it: measured on GT-scored full-text F1
    /// across 3 backends x 7 scanned fixtures, it was better on 2 files, worse on
    /// 10, tied on 2 (over-fusion -- taking a whole-block median collapsed
    /// legitimately distinct fragments and grew max block size in every affected
    /// file). `RESOLVE_OCR_FONT_SIZE_PER_BLOCK` now defaults to `false`, so
    /// `ocr_block_median_font_sizes` returns an empty map and every fragment falls
    /// back to `geometric_ocr_font_size_pt`, i.e. its own quad edge height (here,
    /// `font_size_scale` is `OcrFontSizeScale::uniform(1.0)` and each element is a
    /// single line, so the resolved font size is exactly the raw fragment height
    /// in pixels, unchanged fragment to fragment).
    ///
    /// This pins that shipped per-fragment behaviour as a regression guard: the
    /// real recorded sceptre heights for one body line of the regression fixture,
    /// 32/36/32/36/32/32px, must resolve to six *different* font sizes -- the
    /// mid-sentence heading fabrication that motivated the median is now caught
    /// downstream instead, by `SUPPRESS_LOWERCASE_START_HEADINGS` in
    /// `pdf::structure::pipeline`. If this test starts failing with all-32.0
    /// values, `RESOLVE_OCR_FONT_SIZE_PER_BLOCK` was flipped back to `true`
    /// without re-running the A/B that rejected it.
    #[test]
    fn test_ocr_block_median_font_size_removes_intra_line_detection_box_variance() {
        let doc = quad_fragment_document(Some("block_1_1"), &[32, 36, 32, 36, 32, 32], None);

        let paragraphs = ocr_doc_to_paragraphs(&doc, 1000, OcrFontSizeScale::uniform(1.0));
        let font_sizes = ocr_segment_font_sizes(paragraphs);

        assert_eq!(
            font_sizes,
            vec![32.0_f32, 36.0, 32.0, 36.0, 32.0, 32.0],
            "per-fragment resolution (RESOLVE_OCR_FONT_SIZE_PER_BLOCK = false) must \
             reproduce each fragment's own detection-box height verbatim"
        );
        let spread =
            font_sizes.iter().copied().fold(f32::MIN, f32::max) - font_sizes.iter().copied().fold(f32::MAX, f32::min);
        assert_eq!(
            spread, 4.0,
            "intra-block font-size spread is the un-smoothed 36.0 - 32.0 gap; the block \
             median (spread 0.0) was measured and rejected, see f4a9c19b13"
        );
    }

    /// The block-median aggregate (median, not mean, to survive a fragmented
    /// outlier) is intact but *inert*: `f4a9c19b13` set
    /// `RESOLVE_OCR_FONT_SIZE_PER_BLOCK = false` after an A/B showed it was a net
    /// full-text F1 regression (2 files better, 10 worse, 2 tied across 3 backends
    /// x 7 fixtures). With it off, `ocr_block_median_font_sizes` returns no
    /// entries at all, so `resolve_ocr_font_size_pt` never sees a
    /// `block_median_pt` and falls through to `geometric_ocr_font_size_pt` for
    /// every fragment individually -- the 120px outlier resolves to its own raw
    /// `120.0`, not the block's `30.0` median.
    ///
    /// This pins that shipped per-fragment behaviour as a regression guard for
    /// the const staying `false`.
    #[test]
    fn test_ocr_block_font_size_uses_median_so_a_fragmented_outlier_does_not_move_it() {
        let doc = quad_fragment_document(Some("block_1_1"), &[30, 30, 30, 30, 120], None);

        let paragraphs = ocr_doc_to_paragraphs(&doc, 1000, OcrFontSizeScale::uniform(1.0));
        let font_sizes = ocr_segment_font_sizes(paragraphs);

        assert_eq!(
            font_sizes,
            vec![30.0_f32, 30.0, 30.0, 30.0, 120.0],
            "with the block median off (f4a9c19b13), the outlier fragment resolves to \
             its own raw detection-box height, 120.0, not the block's 30.0 median"
        );
    }

    /// The ML-layout route (`ocr_doc_to_layout_paragraphs`, which builds lines
    /// through `make_ocr_line_paragraphs` rather than `make_ocr_block_paragraphs`)
    /// shares the same `ocr_block_median_font_sizes` wiring as the non-layout
    /// route, so it must also stay on per-fragment resolution while
    /// `RESOLVE_OCR_FONT_SIZE_PER_BLOCK = false` (`f4a9c19b13`): each fragment
    /// keeps its own raw detection-box height instead of being fused to a block
    /// median.
    #[cfg(feature = "layout-detection")]
    #[test]
    fn test_layout_route_also_resolves_font_size_as_a_block_median() {
        let doc = quad_fragment_document(Some("block_1_1"), &[32, 36, 32, 36, 32, 32], None);

        let paragraphs = ocr_doc_to_layout_paragraphs(&doc, 1000, &[], 0.5, 0.2, OcrFontSizeScale::uniform(1.0));
        let font_sizes = ocr_segment_font_sizes(paragraphs);

        assert_eq!(
            font_sizes,
            vec![32.0_f32, 36.0, 32.0, 36.0, 32.0, 32.0],
            "the layout route must reproduce the same per-fragment (non-median) \
             resolution as the non-layout route while the const is false"
        );
    }

    /// Tesseract's `x_fsize` is a per-`ocr_par` constant and a genuine typographic
    /// point size, so it must not be routed through the median at all: the fragments
    /// here report `x_fsize = 14` while their detection boxes are 32px and 300px tall,
    /// and both must still resolve to `14.0`. A median over the geometry would give
    /// `166.0`; a median over the `x_fsize` values themselves would give `14.0` too,
    /// so only the geometry mismatch can distinguish "skipped" from "no-op", which is
    /// why the two heights are so far apart.
    ///
    /// This passes on both fixed and unfixed code -- it is a behaviour-preservation
    /// pin, not a proof of the fix, and cannot be otherwise: the guarantee it states
    /// is that nothing changed.
    #[test]
    fn test_tesseract_x_fsize_font_size_is_never_routed_through_the_block_median() {
        let doc = quad_fragment_document(Some("block_1_1"), &[32, 300], Some("14"));

        let paragraphs = ocr_doc_to_paragraphs(&doc, 1000, OcrFontSizeScale::uniform(1.0));

        assert_eq!(
            ocr_segment_font_sizes(paragraphs),
            vec![14.0_f32; 2],
            "x_fsize must win outright; a geometry median would give 166.0"
        );
    }

    /// Fragments with no `hocr_block_id` cannot be grouped, so they keep the exact
    /// per-fragment geometric resolution that shipped before #712 -- `32.0` and
    /// `36.0`, not a median of `34.0`. Blocks with fewer than
    /// `MIN_BLOCK_FRAGMENTS_FOR_MEDIAN` measurable fragments take the same path.
    ///
    /// Passes on both fixed and unfixed code: it documents the fallback.
    #[test]
    fn test_ocr_font_size_falls_back_to_per_fragment_geometry_without_a_block_id() {
        let doc = quad_fragment_document(None, &[32, 36], None);

        let paragraphs = ocr_doc_to_paragraphs(&doc, 1000, OcrFontSizeScale::uniform(1.0));

        assert_eq!(
            ocr_segment_font_sizes(paragraphs),
            vec![32.0_f32, 36.0_f32],
            "without a block id each fragment must keep its own geometric font size"
        );
    }

    #[test]
    fn test_median_font_size_pt_handles_even_odd_and_degenerate_inputs() {
        assert_eq!(median_font_size_pt(&mut [36.0, 32.0, 32.0]), Some(32.0));
        assert_eq!(median_font_size_pt(&mut [36.0, 32.0, 34.0, 30.0]), Some(33.0));
        assert_eq!(median_font_size_pt(&mut [17.0]), Some(17.0));
        assert_eq!(median_font_size_pt(&mut []), None);
    }

    #[test]
    fn test_ocr_doc_merges_only_consecutive_paragraphs_in_same_hocr_block() {
        let mut same_block = layout_line_document(&[
            ("First", 100.0, 100.0, 500.0, 120.0),
            ("Second", 100.0, 120.0, 500.0, 140.0),
        ]);
        set_hocr_block_ids(&mut same_block, &[Some("block_1_1"), Some("block_1_1")]);
        let merged = ocr_doc_to_paragraphs(&same_block, 1000, OcrFontSizeScale::uniform(1.0));
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].text, "First\nSecond");

        let mut different_blocks = same_block.clone();
        set_hocr_block_ids(&mut different_blocks, &[Some("block_1_1"), Some("block_1_2")]);
        assert_eq!(
            ocr_doc_to_paragraphs(&different_blocks, 1000, OcrFontSizeScale::uniform(1.0)).len(),
            2
        );

        let no_blocks = layout_line_document(&[
            ("First", 100.0, 100.0, 500.0, 120.0),
            ("Second", 100.0, 120.0, 500.0, 140.0),
        ]);
        assert_eq!(
            ocr_doc_to_paragraphs(&no_blocks, 1000, OcrFontSizeScale::uniform(1.0)).len(),
            2
        );

        let mut long_paragraphs = layout_line_document(&[
            ("One\nTwo\nThree\nFour\nFive\nSix\nSeven", 100.0, 100.0, 500.0, 240.0),
            (
                "Eight\nNine\nTen\nEleven\nTwelve\nThirteen\nFourteen",
                100.0,
                240.0,
                500.0,
                380.0,
            ),
        ]);
        set_hocr_block_ids(&mut long_paragraphs, &[Some("block_1_1"), Some("block_1_1")]);
        assert_eq!(
            ocr_doc_to_paragraphs(&long_paragraphs, 1000, OcrFontSizeScale::uniform(1.0)).len(),
            2
        );
    }

    /// Test that OCR elements with mixed content and blank lines preserve all text.
    #[test]
    fn test_ocr_doc_preserves_mixed_content_with_blanks() {
        let mut doc = InternalDocument::new("test");
        let mut elem = InternalElement::text(
            ElementKind::OcrText {
                level: OcrElementLevel::Line,
            },
            "line1\n\nline3",
            0,
        );
        elem.bbox = Some(BoundingBox {
            x0: 10.0,
            y0: 10.0,
            x1: 100.0,
            y1: 70.0,
        });
        doc.push_element(elem);

        let paragraphs = ocr_doc_to_paragraphs(&doc, 1000, OcrFontSizeScale::uniform(1.0));

        assert_eq!(paragraphs.len(), 1);
        assert_eq!(paragraphs[0].text, "line1\n\nline3");
        assert_eq!(paragraphs[0].word_count, 2);
        assert_eq!(paragraphs[0].lines.len(), 2);
    }

    /// Test that whitespace-only OCR elements are filtered out (correct behavior).
    #[test]
    fn test_ocr_doc_filters_whitespace_only_elements() {
        let mut doc = InternalDocument::new("test");
        let mut elem1 = InternalElement::text(
            ElementKind::OcrText {
                level: OcrElementLevel::Line,
            },
            "   \n  \n  ",
            0,
        );
        elem1.bbox = Some(BoundingBox {
            x0: 10.0,
            y0: 10.0,
            x1: 100.0,
            y1: 70.0,
        });
        doc.push_element(elem1);

        let mut elem2 = InternalElement::text(
            ElementKind::OcrText {
                level: OcrElementLevel::Line,
            },
            "real content",
            0,
        );
        elem2.bbox = Some(BoundingBox {
            x0: 10.0,
            y0: 80.0,
            x1: 100.0,
            y1: 140.0,
        });
        doc.push_element(elem2);

        let paragraphs = ocr_doc_to_paragraphs(&doc, 1000, OcrFontSizeScale::uniform(1.0));

        assert_eq!(paragraphs.len(), 1, "Should filter out whitespace-only element");
        assert_eq!(paragraphs[0].text, "real content");
    }

    /// Test that whitespace-only lines and their exact text remain in the OCR block.
    #[test]
    fn test_ocr_doc_whitespace_lines_text_preserved() {
        let mut doc = InternalDocument::new("test");
        let mut elem = InternalElement::text(
            ElementKind::OcrText {
                level: OcrElementLevel::Line,
            },
            "Para1\n   \nPara2",
            0,
        );
        elem.bbox = Some(BoundingBox {
            x0: 10.0,
            y0: 10.0,
            x1: 100.0,
            y1: 70.0,
        });
        doc.push_element(elem);

        let paragraphs = ocr_doc_to_paragraphs(&doc, 1000, OcrFontSizeScale::uniform(1.0));

        assert_eq!(paragraphs.len(), 1);
        assert_eq!(paragraphs[0].text, "Para1\n   \nPara2");
        assert_eq!(paragraphs[0].lines.len(), 2);
        let line_height = (70.0 - 10.0) / 3.0;
        let para_1_y = 1000.0 - 10.0 - line_height;
        let para_2_y = 1000.0 - 10.0 - 3.0 * line_height;
        assert!(
            (paragraphs[0].lines[0].baseline_y - para_1_y).abs() < 0.1,
            "Line 1 Y position incorrect"
        );
        assert!(
            (paragraphs[0].lines[1].baseline_y - para_2_y).abs() < 0.1,
            "Line 2 Y position incorrect"
        );
    }

    /// Test that blank lines in OCR elements don't affect vertical positioning.
    /// When text contains blank lines (e.g., "A\n\nC"), the lines array should still
    /// have correct y-positions (0 for A, 2*line_height for C, not 1*line_height).
    /// This ensures correct sorting order when multiple paragraphs are interleaved.
    #[test]
    fn test_ocr_doc_blank_lines_preserve_vertical_spacing() {
        let mut doc = InternalDocument::new("test");
        let mut elem = InternalElement::text(
            ElementKind::OcrText {
                level: OcrElementLevel::Line,
            },
            "Line1\n\nLine3",
            0,
        );
        elem.bbox = Some(BoundingBox {
            x0: 10.0,
            y0: 10.0,
            x1: 100.0,
            y1: 90.0,
        });
        doc.push_element(elem);

        let paragraphs = ocr_doc_to_paragraphs(&doc, 1000, OcrFontSizeScale::uniform(1.0));
        assert_eq!(paragraphs.len(), 1);
        assert_eq!(paragraphs[0].text, "Line1\n\nLine3");
        assert_eq!(paragraphs[0].lines.len(), 2);

        let expected_line_height = 80.0 / 3.0;

        assert!(
            (paragraphs[0].lines[0].baseline_y - (990.0 - expected_line_height)).abs() < 0.1,
            "Line1 baseline is incorrect: {}",
            paragraphs[0].lines[0].baseline_y
        );
        let first_segment = &paragraphs[0].lines[0].segments[0];
        assert!((first_segment.y + first_segment.height - 990.0).abs() < 0.1);
        assert!(
            (paragraphs[0].lines[1].baseline_y - (990.0 - 3.0 * expected_line_height)).abs() < 0.1,
            "Line3 baseline is incorrect: {}",
            paragraphs[0].lines[1].baseline_y
        );
    }

    /// Test that OCR elements with content followed by blanks preserve content.
    #[test]
    fn test_ocr_doc_preserves_content_before_blanks() {
        let mut doc = InternalDocument::new("test");
        let mut elem = InternalElement::text(
            ElementKind::OcrText {
                level: OcrElementLevel::Line,
            },
            "important\n\n",
            0,
        );
        elem.bbox = Some(BoundingBox {
            x0: 10.0,
            y0: 10.0,
            x1: 100.0,
            y1: 70.0,
        });
        doc.push_element(elem);

        let paragraphs = ocr_doc_to_paragraphs(&doc, 1000, OcrFontSizeScale::uniform(1.0));

        assert_eq!(paragraphs.len(), 1);
        assert_eq!(paragraphs[0].text, "important\n\n");
        assert_eq!(paragraphs[0].word_count, 1);
        assert_eq!(
            paragraphs[0].lines.len(),
            1,
            "Only the non-blank line should be in lines array"
        );
    }

    /// An element whose hOCR `x_bold_fraction` attribute reports a clear majority
    /// of bold words must produce a bold `SegmentData`. Against unfixed code
    /// (`make_ocr_pdf_line` hardcoding `is_bold: false`), this assertion fails:
    /// the segment comes back `is_bold == false` regardless of the attribute.
    #[test]
    fn test_ocr_doc_segment_is_bold_when_element_reports_majority_bold_fraction() {
        use ahash::AHashMap;

        let mut doc = InternalDocument::new("test");
        let mut elem = InternalElement::text(
            ElementKind::OcrText {
                level: OcrElementLevel::Line,
            },
            "Bold Heading",
            0,
        );
        elem.bbox = Some(BoundingBox {
            x0: 10.0,
            y0: 10.0,
            x1: 200.0,
            y1: 50.0,
        });
        let mut attrs = AHashMap::new();
        attrs.insert("x_bold_fraction".to_string(), "1".to_string());
        // `InternalElement::with_attributes` is gated on the xml/hwpx features, which the
        // OCR test set does not enable; assign the public field directly instead.
        elem.attributes = Some(attrs);
        doc.push_element(elem);

        let paragraphs = ocr_doc_to_paragraphs(&doc, 1000, OcrFontSizeScale::uniform(1.0));

        assert_eq!(paragraphs[0].lines.len(), 1);
        assert!(
            paragraphs[0].lines[0].segments[0].is_bold,
            "expected the majority-bold fraction to mark the segment bold"
        );
    }

    /// An element carrying no style attributes at all (the sceptre/paddle case,
    /// since only Tesseract's hOCR block parser ever writes `x_bold_fraction` /
    /// `x_italic_fraction`) must keep the previous default of `is_bold == false`
    /// and `is_italic == false`. This is the behavior-preservation guarantee for
    /// non-Tesseract backends and passes on both fixed and unfixed code -- it
    /// documents the fallback rather than proving the fix.
    #[test]
    fn test_ocr_doc_segment_style_defaults_to_false_when_attribute_absent() {
        let mut doc = InternalDocument::new("test");
        let mut elem = InternalElement::text(
            ElementKind::OcrText {
                level: OcrElementLevel::Line,
            },
            "Plain text",
            0,
        );
        elem.bbox = Some(BoundingBox {
            x0: 10.0,
            y0: 10.0,
            x1: 200.0,
            y1: 50.0,
        });
        doc.push_element(elem);

        let paragraphs = ocr_doc_to_paragraphs(&doc, 1000, OcrFontSizeScale::uniform(1.0));

        assert_eq!(paragraphs[0].lines.len(), 1);
        assert!(!paragraphs[0].lines[0].segments[0].is_bold);
        assert!(!paragraphs[0].lines[0].segments[0].is_italic);
    }

    /// A malformed (non-numeric) or out-of-range `x_bold_fraction` /
    /// `x_italic_fraction` value must not panic and must fall back to `false`.
    /// Against unfixed code this assertion trivially passes too (both are
    /// hardcoded `false`), so this test's job is to prove `.parse::<f32>()`
    /// never panics on garbage input and that out-of-range fractions
    /// (negative, or > 1.0, which cannot be legitimate word-fractions) are
    /// rejected rather than misread as "bold".
    #[test]
    fn test_ocr_doc_segment_style_falls_back_on_malformed_fraction_without_panicking() {
        use ahash::AHashMap;

        let mut doc = InternalDocument::new("test");
        let mut elem = InternalElement::text(
            ElementKind::OcrText {
                level: OcrElementLevel::Line,
            },
            "Garbled attrs",
            0,
        );
        elem.bbox = Some(BoundingBox {
            x0: 10.0,
            y0: 10.0,
            x1: 200.0,
            y1: 50.0,
        });
        let mut attrs = AHashMap::new();
        attrs.insert("x_bold_fraction".to_string(), "not-a-number".to_string());
        attrs.insert("x_italic_fraction".to_string(), "5.0".to_string());
        // `InternalElement::with_attributes` is gated on the xml/hwpx features, which the
        // OCR test set does not enable; assign the public field directly instead.
        elem.attributes = Some(attrs);
        doc.push_element(elem);

        let paragraphs = ocr_doc_to_paragraphs(&doc, 1000, OcrFontSizeScale::uniform(1.0));

        assert_eq!(paragraphs[0].lines.len(), 1);
        assert!(!paragraphs[0].lines[0].segments[0].is_bold);
        assert!(!paragraphs[0].lines[0].segments[0].is_italic);
    }

    #[cfg(feature = "layout-detection")]
    #[test]
    fn test_multiline_ocr_block_applies_line_sized_layout_hint_only_to_matching_line() {
        use crate::pdf::structure::types::{LayoutHint, LayoutHintClass};

        let mut doc = InternalDocument::new("test");
        let mut elem = InternalElement::text(
            ElementKind::OcrText {
                level: OcrElementLevel::Block,
            },
            "Document title\nFirst body line\nSecond body line",
            0,
        );
        elem.bbox = Some(BoundingBox {
            x0: 100.0,
            y0: 100.0,
            x1: 500.0,
            y1: 160.0,
        });
        doc.push_element(elem);

        let hints = [LayoutHint {
            class_name: LayoutHintClass::Title,
            confidence: 0.95,
            left: 100.0,
            bottom: 880.0,
            right: 500.0,
            top: 900.0,
        }];
        let paragraphs = ocr_doc_to_layout_paragraphs(&doc, 1000, &hints, 0.5, 0.2, OcrFontSizeScale::uniform(1.0));

        assert_eq!(paragraphs.len(), 2);
        assert_eq!(paragraphs[0].text, "Document title");
        assert_eq!(paragraphs[1].text, "First body line\nSecond body line");
        assert_eq!(paragraphs[0].block_bbox, Some((100.0, 880.0, 500.0, 900.0)));
        assert_eq!(paragraphs[1].block_bbox, Some((100.0, 840.0, 500.0, 880.0)));
        assert_eq!(paragraphs[0].heading_level, Some(1));
        assert_eq!(paragraphs[1].heading_level, None);

        let assembled = crate::pdf::structure::assemble_internal_document(vec![paragraphs], &[], None, &[]);
        assert_eq!(assembled.elements.len(), 2);
        assert_eq!(assembled.elements[0].kind, ElementKind::Heading { level: 1 });
        assert_eq!(assembled.elements[1].kind, ElementKind::Paragraph);
        assert_eq!(assembled.elements[1].text, "First body line\nSecond body line");
    }

    #[cfg(feature = "layout-detection")]
    fn layout_test_document(text: &str, line_count: u32) -> InternalDocument {
        let mut doc = InternalDocument::new("test");
        let mut element = InternalElement::text(
            ElementKind::OcrText {
                level: OcrElementLevel::Block,
            },
            text,
            0,
        );
        element.bbox = Some(BoundingBox {
            x0: 100.0,
            y0: 100.0,
            x1: 500.0,
            y1: 100.0 + f64::from(line_count * 20),
        });
        doc.push_element(element);
        doc
    }

    #[cfg(feature = "layout-detection")]
    fn layout_test_hint(class_name: types::LayoutHintClass, bottom: f32, top: f32) -> types::LayoutHint {
        types::LayoutHint {
            class_name,
            confidence: 0.95,
            left: 100.0,
            bottom,
            right: 500.0,
            top,
        }
    }

    fn layout_line_document(lines: &[(&str, f64, f64, f64, f64)]) -> InternalDocument {
        let mut doc = InternalDocument::new("test");
        for &(text, x0, y0, x1, y1) in lines {
            let mut element = InternalElement::text(
                ElementKind::OcrText {
                    level: OcrElementLevel::Line,
                },
                text,
                0,
            );
            element.bbox = Some(BoundingBox { x0, y0, x1, y1 });
            doc.push_element(element);
        }
        doc
    }

    fn set_hocr_block_ids(doc: &mut InternalDocument, block_ids: &[Option<&str>]) {
        for (element, block_id) in doc.elements.iter_mut().zip(block_ids) {
            element.attributes = block_id.map(|block_id| {
                [("hocr_block_id".to_string(), block_id.to_string())]
                    .into_iter()
                    .collect()
            });
        }
    }

    /// Regression test for #631: PaddleOCR emits one `InternalElement` per
    /// detected line with `attributes == None`, so before block ids were
    /// derived from line geometry (`ocr::conversion::assign_line_block_ids`),
    /// `hocr_block_id` was always `None` and every line became its own
    /// `PdfParagraph` here — fragmenting paragraphs and, on a real scanned
    /// document, losing 33 of 39 list markers relative to Tesseract. With
    /// geometry-derived block ids wired through (as `paddle_ocr::backend` now
    /// does), consecutive close, x-overlapping lines merge into one paragraph
    /// exactly like Tesseract's `ocr_par`-derived elements do.
    #[cfg(paddle_ocr)]
    #[test]
    fn test_ocr_doc_to_paragraphs_merges_paddle_derived_lines_regression_631() {
        let raw_elements = vec![
            crate::types::OcrElement::new(
                "First wrapped line",
                crate::types::OcrBoundingGeometry::Rectangle {
                    left: 100,
                    top: 100,
                    width: 400,
                    height: 20,
                },
                crate::types::OcrConfidence::from_paddle(0.9, 0.9),
            ),
            crate::types::OcrElement::new(
                "continues the same paragraph",
                crate::types::OcrBoundingGeometry::Rectangle {
                    left: 100,
                    top: 124,
                    width: 400,
                    height: 20,
                },
                crate::types::OcrConfidence::from_paddle(0.9, 0.9),
            ),
        ];
        let block_ids = crate::ocr::conversion::assign_line_block_ids(&raw_elements);
        let block_id_refs = block_ids.iter().map(|id| Some(id.as_str())).collect::<Vec<_>>();

        let mut doc = layout_line_document(&[
            ("First wrapped line", 100.0, 100.0, 500.0, 120.0),
            ("continues the same paragraph", 100.0, 124.0, 500.0, 144.0),
        ]);
        set_hocr_block_ids(&mut doc, &block_id_refs);

        let paragraphs = ocr_doc_to_paragraphs(&doc, 1000, OcrFontSizeScale::uniform(1.0));

        assert_eq!(
            paragraphs.len(),
            1,
            "geometry-adjacent PaddleOCR lines must merge into one paragraph, not fragment"
        );
        assert_eq!(paragraphs[0].text, "First wrapped line\ncontinues the same paragraph");
    }

    #[cfg(feature = "layout-detection")]
    #[test]
    fn test_layout_merges_only_adjacent_lines_in_same_hocr_block() {
        let mut same_block = layout_line_document(&[
            ("First", 100.0, 100.0, 500.0, 120.0),
            ("Second", 100.0, 120.0, 500.0, 140.0),
        ]);
        set_hocr_block_ids(&mut same_block, &[Some("block_1_1"), Some("block_1_1")]);
        let merged = ocr_doc_to_layout_paragraphs(&same_block, 1000, &[], 0.5, 0.2, OcrFontSizeScale::uniform(1.0));
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].text, "First\nSecond");

        let mut different_blocks = same_block.clone();
        set_hocr_block_ids(&mut different_blocks, &[Some("block_1_1"), Some("block_1_2")]);
        assert_eq!(
            ocr_doc_to_layout_paragraphs(&different_blocks, 1000, &[], 0.5, 0.2, OcrFontSizeScale::uniform(1.0)).len(),
            2
        );

        let no_blocks = layout_line_document(&[
            ("First", 100.0, 100.0, 500.0, 120.0),
            ("Second", 100.0, 120.0, 500.0, 140.0),
        ]);
        assert_eq!(
            ocr_doc_to_layout_paragraphs(&no_blocks, 1000, &[], 0.5, 0.2, OcrFontSizeScale::uniform(1.0)).len(),
            2
        );

        let mut long_paragraphs = layout_line_document(&[
            ("One\nTwo\nThree\nFour\nFive\nSix\nSeven", 100.0, 100.0, 500.0, 240.0),
            (
                "Eight\nNine\nTen\nEleven\nTwelve\nThirteen\nFourteen",
                100.0,
                240.0,
                500.0,
                380.0,
            ),
        ]);
        set_hocr_block_ids(&mut long_paragraphs, &[Some("block_1_1"), Some("block_1_1")]);
        assert_eq!(
            ocr_doc_to_layout_paragraphs(&long_paragraphs, 1000, &[], 0.5, 0.2, OcrFontSizeScale::uniform(1.0)).len(),
            2
        );
    }

    #[cfg(feature = "layout-detection")]
    #[test]
    fn should_coalesce_adjacent_ocr_elements_in_same_text_region() {
        let doc = layout_line_document(&[
            ("First wrapped line", 100.0, 100.0, 500.0, 120.0),
            ("continues in the same paragraph", 100.0, 120.0, 500.0, 140.0),
        ]);
        let hints = [layout_test_hint(types::LayoutHintClass::Text, 860.0, 900.0)];

        let paragraphs = ocr_doc_to_layout_paragraphs(&doc, 1000, &hints, 0.5, 0.2, OcrFontSizeScale::uniform(1.0));

        assert_eq!(paragraphs.len(), 1);
        assert_eq!(
            paragraphs[0].text,
            "First wrapped line\ncontinues in the same paragraph"
        );
        assert_eq!(paragraphs[0].layout_class, Some(types::LayoutHintClass::Text));
    }

    #[cfg(feature = "layout-detection")]
    #[test]
    fn should_flush_body_regions_when_heading_separates_ocr_elements() {
        let doc = layout_line_document(&[
            ("Body before", 100.0, 100.0, 500.0, 120.0),
            ("Section title", 100.0, 120.0, 500.0, 140.0),
            ("Body after", 100.0, 140.0, 500.0, 160.0),
        ]);
        let hints = [
            layout_test_hint(types::LayoutHintClass::Text, 840.0, 900.0),
            layout_test_hint(types::LayoutHintClass::SectionHeader, 860.0, 880.0),
        ];

        let paragraphs = ocr_doc_to_layout_paragraphs(&doc, 1000, &hints, 0.5, 0.2, OcrFontSizeScale::uniform(1.0));

        assert_eq!(paragraphs.len(), 3);
        assert_eq!(paragraphs[0].text, "Body before");
        assert_eq!(paragraphs[1].text, "Section title");
        assert_eq!(paragraphs[1].heading_level, Some(2));
        assert_eq!(paragraphs[2].text, "Body after");
    }

    #[cfg(feature = "layout-detection")]
    #[test]
    fn should_keep_adjacent_ocr_elements_in_distinct_text_regions_separate() {
        let doc = layout_line_document(&[
            ("Left column", 100.0, 100.0, 280.0, 120.0),
            ("Right column", 320.0, 100.0, 500.0, 120.0),
        ]);
        let hints = [
            types::LayoutHint {
                class_name: types::LayoutHintClass::Text,
                confidence: 0.95,
                left: 100.0,
                bottom: 880.0,
                right: 280.0,
                top: 900.0,
            },
            types::LayoutHint {
                class_name: types::LayoutHintClass::Text,
                confidence: 0.95,
                left: 320.0,
                bottom: 880.0,
                right: 500.0,
                top: 900.0,
            },
        ];

        let paragraphs = ocr_doc_to_layout_paragraphs(&doc, 1000, &hints, 0.5, 0.2, OcrFontSizeScale::uniform(1.0));

        assert_eq!(paragraphs.len(), 2);
        assert_eq!(paragraphs[0].text, "Left column");
        assert_eq!(paragraphs[1].text, "Right column");
    }

    #[cfg(feature = "layout-detection")]
    #[test]
    fn should_keep_distant_ocr_elements_in_same_text_region_separate() {
        let doc = layout_line_document(&[
            ("First paragraph", 100.0, 100.0, 500.0, 120.0),
            ("Second paragraph", 100.0, 300.0, 500.0, 320.0),
        ]);
        let hints = [layout_test_hint(types::LayoutHintClass::Text, 680.0, 900.0)];

        let paragraphs = ocr_doc_to_layout_paragraphs(&doc, 1000, &hints, 0.5, 0.2, OcrFontSizeScale::uniform(1.0));

        assert_eq!(paragraphs.len(), 2);
        assert_eq!(paragraphs[0].text, "First paragraph");
        assert_eq!(paragraphs[1].text, "Second paragraph");
    }

    #[cfg(feature = "layout-detection")]
    fn logo_title_test_document(first_block: &str, prose: Option<&str>) -> InternalDocument {
        let mut doc = InternalDocument::new("test");
        let mut first = InternalElement::text(
            ElementKind::OcrText {
                level: OcrElementLevel::Block,
            },
            first_block,
            0,
        );
        first.bbox = Some(BoundingBox {
            x0: 100.0,
            y0: 100.0,
            x1: 500.0,
            y1: 180.0,
        });
        doc.push_element(first);

        if let Some(prose) = prose {
            let mut body = InternalElement::text(
                ElementKind::OcrText {
                    level: OcrElementLevel::Block,
                },
                prose,
                0,
            );
            body.bbox = Some(BoundingBox {
                x0: 50.0,
                y0: 220.0,
                x1: 550.0,
                y1: 400.0,
            });
            doc.push_element(body);
        }
        doc
    }

    #[cfg(feature = "layout-detection")]
    #[test]
    fn test_first_logo_block_promotes_following_title_without_reordering() {
        let prose = "This is a complete body sentence with enough words to identify ordinary prose.";
        let doc = logo_title_test_document("IDRH\nNon-text-searchable PDF", Some(prose));
        let hints = [types::LayoutHint {
            class_name: types::LayoutHintClass::Text,
            confidence: 0.97,
            left: 50.0,
            bottom: 750.0,
            right: 550.0,
            top: 920.0,
        }];

        let paragraphs = ocr_doc_to_layout_paragraphs(&doc, 1000, &hints, 0.5, 0.2, OcrFontSizeScale::uniform(1.0));

        assert_eq!(paragraphs.len(), 3);
        assert_eq!(paragraphs[0].text, "IDRH");
        assert_eq!(paragraphs[0].heading_level, None);
        assert_eq!(paragraphs[1].text, "Non-text-searchable PDF");
        assert_eq!(paragraphs[1].heading_level, Some(1));
        assert_eq!(paragraphs[1].layout_class, Some(types::LayoutHintClass::Title));
        assert_eq!(paragraphs[2].text, prose);
        assert_eq!(paragraphs[2].heading_level, None);
    }

    #[cfg(feature = "layout-detection")]
    #[test]
    fn test_logo_title_fallback_requires_following_prose() {
        let doc = logo_title_test_document("IDRH\nNon-text-searchable PDF", None);

        let paragraphs = ocr_doc_to_layout_paragraphs(&doc, 1000, &[], 0.5, 0.2, OcrFontSizeScale::uniform(1.0));

        assert_eq!(paragraphs.len(), 1);
        assert_eq!(paragraphs[0].text, "IDRH\nNon-text-searchable PDF");
        assert_eq!(paragraphs[0].heading_level, None);
    }

    #[cfg(feature = "layout-detection")]
    #[test]
    fn test_logo_title_fallback_preserves_non_logo_first_blocks() {
        let prose = "This is a complete body sentence with enough words to identify ordinary prose.";
        for first_block in [
            "Quarterly results\nRevenue increased",
            "Geschäftsstelle Ludwigstraße 23\n80539 München",
            "NIVE <¥ Rs), ,\nYr A %",
            "IDRH\nNon-text-searchable PDF.",
        ] {
            let doc = logo_title_test_document(first_block, Some(prose));

            let paragraphs = ocr_doc_to_layout_paragraphs(&doc, 1000, &[], 0.5, 0.2, OcrFontSizeScale::uniform(1.0));

            assert_eq!(paragraphs[0].text, first_block);
            assert_eq!(paragraphs[0].heading_level, None);
        }
    }

    #[cfg(feature = "layout-detection")]
    #[test]
    fn test_logo_title_fallback_defers_to_semantic_heading_hint() {
        let prose = "This is a complete body sentence with enough words to identify ordinary prose.";
        let doc = logo_title_test_document("IDRH\nNon-text-searchable PDF", Some(prose));
        let hints = [types::LayoutHint {
            class_name: types::LayoutHintClass::SectionHeader,
            confidence: 0.95,
            left: 700.0,
            bottom: 700.0,
            right: 900.0,
            top: 750.0,
        }];

        let paragraphs = ocr_doc_to_layout_paragraphs(&doc, 1000, &hints, 0.5, 0.2, OcrFontSizeScale::uniform(1.0));

        assert_eq!(paragraphs[0].text, "IDRH\nNon-text-searchable PDF");
        assert_eq!(paragraphs[0].heading_level, None);
    }

    #[cfg(feature = "layout-detection")]
    #[test]
    fn test_logo_title_fallback_preserves_existing_structural_override() {
        let prose = "This is a complete body sentence with enough words to identify ordinary prose.";
        let doc = logo_title_test_document("IDRH\nfunction title() {", Some(prose));
        let hints = [types::LayoutHint {
            class_name: types::LayoutHintClass::Code,
            confidence: 0.95,
            left: 100.0,
            bottom: 820.0,
            right: 500.0,
            top: 860.0,
        }];

        let paragraphs = ocr_doc_to_layout_paragraphs(&doc, 1000, &hints, 0.5, 0.2, OcrFontSizeScale::uniform(1.0));

        assert_eq!(paragraphs[0].text, "IDRH");
        assert_eq!(paragraphs[0].heading_level, None);
        assert!(paragraphs[1].is_code_block);
        assert_eq!(paragraphs[1].heading_level, None);
        assert_eq!(paragraphs[1].layout_class, Some(types::LayoutHintClass::Code));
    }

    #[cfg(feature = "layout-detection")]
    #[test]
    fn test_multiline_title_lines_under_same_hint_merge() {
        let doc = layout_test_document("A long document\nsubtitle line\nBody text", 3);
        let hints = [layout_test_hint(types::LayoutHintClass::Title, 860.0, 900.0)];

        let paragraphs = ocr_doc_to_layout_paragraphs(&doc, 1000, &hints, 0.5, 0.2, OcrFontSizeScale::uniform(1.0));

        assert_eq!(paragraphs.len(), 2);
        assert_eq!(paragraphs[0].text, "A long document\nsubtitle line");
        assert_eq!(paragraphs[0].heading_level, Some(1));
        assert_eq!(paragraphs[0].block_bbox, Some((100.0, 860.0, 500.0, 900.0)));
        assert_eq!(paragraphs[1].text, "Body text");
    }

    #[cfg(feature = "layout-detection")]
    #[test]
    fn test_multiline_code_lines_under_same_hint_merge() {
        let doc = layout_test_document("fn main() {\nprintln!(\"hello\");\nBody text", 3);
        let hints = [layout_test_hint(types::LayoutHintClass::Code, 860.0, 900.0)];

        let paragraphs = ocr_doc_to_layout_paragraphs(&doc, 1000, &hints, 0.5, 0.2, OcrFontSizeScale::uniform(1.0));

        assert_eq!(paragraphs.len(), 2);
        assert_eq!(paragraphs[0].text, "fn main() {\nprintln!(\"hello\");");
        assert!(paragraphs[0].is_code_block);
        assert_eq!(paragraphs[1].text, "Body text");
    }

    #[cfg(feature = "layout-detection")]
    #[test]
    fn test_rejected_title_prose_does_not_merge_with_title() {
        let prose = "one two three four five six seven eight nine ten eleven twelve thirteen fourteen fifteen \
                     sixteen seventeen eighteen nineteen twenty twentyone twentytwo twentythree twentyfour twentyfive";
        let doc = layout_test_document(&format!("Document title\n{prose}"), 2);
        let hints = [layout_test_hint(types::LayoutHintClass::Title, 860.0, 900.0)];

        let paragraphs = ocr_doc_to_layout_paragraphs(&doc, 1000, &hints, 0.5, 0.2, OcrFontSizeScale::uniform(1.0));

        assert_eq!(paragraphs.len(), 2);
        assert_eq!(paragraphs[0].text, "Document title");
        assert_eq!(paragraphs[0].heading_level, Some(1));
        assert_eq!(paragraphs[1].text, prose);
        assert_eq!(paragraphs[1].heading_level, None);
    }

    #[cfg(feature = "layout-detection")]
    #[test]
    fn test_rejected_code_prose_does_not_merge_with_code() {
        let prose = "This ordinary prose sentence contains many words, and it clearly should remain body text rather than code.";
        let doc = layout_test_document(&format!("fn main() {{\n{prose}"), 2);
        let hints = [layout_test_hint(types::LayoutHintClass::Code, 860.0, 900.0)];

        let paragraphs = ocr_doc_to_layout_paragraphs(&doc, 1000, &hints, 0.5, 0.2, OcrFontSizeScale::uniform(1.0));

        assert_eq!(paragraphs.len(), 2);
        assert_eq!(paragraphs[0].text, "fn main() {");
        assert!(paragraphs[0].is_code_block);
        assert_eq!(paragraphs[1].text, prose);
        assert!(!paragraphs[1].is_code_block);
    }

    #[cfg(feature = "layout-detection")]
    #[test]
    fn test_wrapped_list_item_lines_under_same_hint_merge() {
        let doc = layout_test_document(
            "1. Wrapped item starts\nand continues here\nacross another line\nBody text",
            4,
        );
        let hints = [layout_test_hint(types::LayoutHintClass::ListItem, 840.0, 900.0)];

        let paragraphs = ocr_doc_to_layout_paragraphs(&doc, 1000, &hints, 0.5, 0.2, OcrFontSizeScale::uniform(1.0));

        assert_eq!(paragraphs.len(), 2);
        assert_eq!(
            paragraphs[0].text,
            "1. Wrapped item starts\nand continues here\nacross another line"
        );
        assert!(paragraphs[0].is_list_item);
        assert_eq!(paragraphs[1].text, "Body text");
    }

    #[cfg(feature = "layout-detection")]
    #[test]
    fn test_title_split_trims_boundary_blank_before_assembled_body() {
        let doc = layout_test_document("Document title\n\nBody text", 3);
        let hints = [layout_test_hint(types::LayoutHintClass::Title, 880.0, 900.0)];
        let paragraphs = ocr_doc_to_layout_paragraphs(&doc, 1000, &hints, 0.5, 0.2, OcrFontSizeScale::uniform(1.0));

        assert_eq!(paragraphs.len(), 2);
        assert_eq!(paragraphs[0].text, "Document title");
        assert_eq!(paragraphs[1].text, "Body text");

        let assembled = crate::pdf::structure::assemble_internal_document(vec![paragraphs], &[], None, &[]);
        assert_eq!(assembled.elements.len(), 2);
        assert_eq!(assembled.elements[0].kind, ElementKind::Heading { level: 1 });
        assert_eq!(assembled.elements[1].kind, ElementKind::Paragraph);
        assert_eq!(assembled.elements[1].text, "Body text");
    }

    #[cfg(feature = "layout-detection")]
    #[test]
    fn test_unassociated_structural_line_does_not_capture_adjacent_body_lines() {
        let doc = layout_test_document("Promoted line\nBody one\nBody two", 3);
        let mut lines = make_ocr_line_paragraphs(&doc.elements[0], 1000.0, OcrFontSizeScale::uniform(1.0), None);
        lines[0].heading_level = Some(1);

        let paragraphs = regroup_layout_lines(lines, vec![None, None, None]);

        assert_eq!(paragraphs.len(), 2);
        assert_eq!(paragraphs[0].text, "Promoted line");
        assert_eq!(paragraphs[0].heading_level, Some(1));
        assert_eq!(paragraphs[1].text, "Body one\nBody two");
        assert_eq!(paragraphs[1].heading_level, None);
    }

    #[cfg(feature = "layout-detection")]
    #[test]
    fn test_picture_uses_canonical_match_identity() {
        let doc = layout_test_document("Detected picture region", 1);
        let hints = [layout_test_hint(types::LayoutHintClass::Picture, 880.0, 900.0)];

        let paragraphs = ocr_doc_to_layout_paragraphs(&doc, 1000, &hints, 0.5, 0.2, OcrFontSizeScale::uniform(1.0));

        assert_eq!(paragraphs.len(), 1);
        assert_eq!(paragraphs[0].layout_class, Some(types::LayoutHintClass::Picture));
        assert!(paragraphs[0].is_page_furniture);
    }

    #[cfg(feature = "layout-detection")]
    fn ordered_list_test_paragraph(text: &str) -> types::PdfParagraph {
        make_ocr_paragraph(text.to_string(), Vec::new(), None, DEFAULT_OCR_FONT_SIZE_PT)
    }

    #[cfg(feature = "layout-detection")]
    fn anchored_ordered_list_test_pages() -> Vec<Vec<types::PdfParagraph>> {
        let mut anchor = ordered_list_test_paragraph("1. First item");
        anchor.is_list_item = true;
        anchor.layout_class = Some(types::LayoutHintClass::ListItem);
        vec![
            vec![anchor, ordered_list_test_paragraph("First continuation")],
            vec![
                ordered_list_test_paragraph("2. Second item"),
                ordered_list_test_paragraph("Second continuation one"),
                ordered_list_test_paragraph("Second continuation two"),
                ordered_list_test_paragraph("Second continuation three"),
                ordered_list_test_paragraph("Second continuation four"),
                ordered_list_test_paragraph("Second continuation five"),
                ordered_list_test_paragraph("Second continuation six"),
                ordered_list_test_paragraph("Second continuation seven"),
                ordered_list_test_paragraph("3. Third item"),
            ],
        ]
    }

    #[cfg(feature = "layout-detection")]
    #[test]
    fn test_promotes_complete_anchored_ordered_list_sequence_across_pages() {
        let mut pages = anchored_ordered_list_test_pages();
        pages[1].insert(8, ordered_list_test_paragraph("Second continuation eight"));
        let original_text = pages
            .iter()
            .flatten()
            .map(|paragraph| paragraph.text.clone())
            .collect::<Vec<_>>();

        promote_anchored_ordered_list_sequences(&mut pages);

        assert!(pages[1][0].is_list_item);
        assert_eq!(pages[1][0].layout_class, Some(types::LayoutHintClass::ListItem));
        assert!(pages[1][9].is_list_item);
        assert_eq!(pages[1][9].layout_class, Some(types::LayoutHintClass::ListItem));
        assert_eq!(
            pages
                .iter()
                .flatten()
                .map(|paragraph| paragraph.text.clone())
                .collect::<Vec<_>>(),
            original_text,
            "promotion must preserve intervening paragraphs and reading order"
        );
    }

    #[cfg(feature = "layout-detection")]
    #[test]
    fn test_does_not_promote_unanchored_or_incomplete_ordered_sequences() {
        let mut unanchored = anchored_ordered_list_test_pages();
        unanchored[0][0].is_list_item = false;
        unanchored[0][0].layout_class = None;
        promote_anchored_ordered_list_sequences(&mut unanchored);
        assert!(!unanchored[1][0].is_list_item);
        assert!(!unanchored[1][8].is_list_item);

        let mut incomplete = anchored_ordered_list_test_pages();
        incomplete[1].pop();
        promote_anchored_ordered_list_sequences(&mut incomplete);
        assert!(!incomplete[1][0].is_list_item, "a two-item prefix must not mutate");
    }

    #[cfg(feature = "layout-detection")]
    #[test]
    fn test_does_not_promote_broken_or_over_gap_ordered_sequences() {
        let mut broken = anchored_ordered_list_test_pages();
        broken[1][0].text = "3. Out of sequence".to_string();
        broken[1][8].text = "4. Still out of sequence".to_string();
        promote_anchored_ordered_list_sequences(&mut broken);
        assert!(!broken[1][0].is_list_item);
        assert!(!broken[1][8].is_list_item);

        let mut over_gap = anchored_ordered_list_test_pages();
        const EXTRA_CONTINUATIONS: usize = 8;
        for _ in 0..EXTRA_CONTINUATIONS {
            over_gap[0].insert(1, ordered_list_test_paragraph("Extra continuation"));
        }
        promote_anchored_ordered_list_sequences(&mut over_gap);
        assert!(!over_gap[1][0].is_list_item);
        assert!(!over_gap[1][8].is_list_item);
    }

    #[cfg(feature = "layout-detection")]
    #[test]
    fn test_empty_paragraphs_do_not_consume_ordered_list_gap() {
        const EMPTY_PARAGRAPHS: usize = 12;
        const ORIGINAL_THIRD_ITEM_INDEX: usize = 8;
        let mut pages = anchored_ordered_list_test_pages();
        for _ in 0..EMPTY_PARAGRAPHS {
            pages[1].insert(1, ordered_list_test_paragraph(" \n "));
        }

        promote_anchored_ordered_list_sequences(&mut pages);

        assert!(pages[1][0].is_list_item);
        assert!(pages[1][ORIGINAL_THIRD_ITEM_INDEX + EMPTY_PARAGRAPHS].is_list_item);
    }

    #[cfg(feature = "layout-detection")]
    #[test]
    fn test_rejects_structural_ordered_list_candidates() {
        for structural_kind in ["heading", "list", "code", "formula", "furniture", "caption"] {
            let mut pages = anchored_ordered_list_test_pages();
            let candidate = &mut pages[1][0];
            match structural_kind {
                "heading" => candidate.heading_level = Some(2),
                "list" => candidate.is_list_item = true,
                "code" => candidate.is_code_block = true,
                "formula" => candidate.is_formula = true,
                "furniture" => candidate.is_page_furniture = true,
                "caption" => candidate.layout_class = Some(types::LayoutHintClass::Caption),
                _ => unreachable!("all test cases are handled"),
            }

            promote_anchored_ordered_list_sequences(&mut pages);

            assert!(!pages[1][8].is_list_item, "structural kind: {structural_kind}");
        }
    }

    #[cfg(feature = "layout-detection")]
    #[test]
    fn test_rejects_numbered_section_heading_candidates() {
        let mut pages = anchored_ordered_list_test_pages();
        pages[1][0].text = "2. DEFINITIONS".to_string();
        pages[1][8].text = "3. TERM".to_string();

        promote_anchored_ordered_list_sequences(&mut pages);

        assert!(!pages[1][0].is_list_item);
        assert!(!pages[1][8].is_list_item);
    }

    /// Builds a test paragraph with a real `PdfLine`/segment whose x/width match
    /// `bbox` exactly (via `make_ocr_pdf_line`, the same constructor production
    /// code uses) rather than an empty `.lines`, so geometry-sensitive grouping
    /// logic sees the same shape it would in production.
    #[cfg(feature = "layout-detection")]
    fn list_test_paragraph(text: &str, bbox: (f32, f32, f32, f32)) -> types::PdfParagraph {
        let (left, bottom, right, top) = bbox;
        let line = make_ocr_pdf_line(
            text,
            left,
            bottom,
            right - left,
            top - bottom,
            DEFAULT_OCR_FONT_SIZE_PT,
            false,
            false,
        );
        make_ocr_paragraph(text.to_string(), vec![line], Some(bbox), DEFAULT_OCR_FONT_SIZE_PT)
    }

    /// Regression for the layout-route list collapse: `regroup_layout_lines_by_element`'s
    /// `same_region` test never flushes mid hOCR-element, so a whole run of list items
    /// inside one element used to reach `push_body_group` as a single accumulated run and
    /// come out as ONE paragraph. Against unfixed code (`push_body_group` always joining
    /// every line with no split) this asserts `result.len() == 3` and fails with
    /// `result.len() == 1`, text "1. First item\n2. Second item\n3. Third item".
    #[cfg(feature = "layout-detection")]
    #[test]
    fn test_push_body_group_splits_pure_list_run_into_one_paragraph_per_item() {
        let lines = vec![
            list_test_paragraph("1. First item", (10.0, 780.0, 500.0, 800.0)),
            list_test_paragraph("2. Second item", (10.0, 760.0, 500.0, 780.0)),
            list_test_paragraph("3. Third item", (10.0, 740.0, 500.0, 760.0)),
        ];

        let mut result = Vec::new();
        push_body_group(&mut result, lines);

        assert_eq!(result.len(), 3, "each list item must become its own paragraph");
        assert_eq!(result[0].text, "1. First item");
        assert_eq!(result[0].block_bbox, Some((10.0, 780.0, 500.0, 800.0)));
        assert_eq!(result[1].text, "2. Second item");
        assert_eq!(result[2].text, "3. Third item");
    }

    /// A lead-in sentence before a list run must stay its own paragraph, not be
    /// absorbed into the first list item. Against unfixed code this asserts
    /// `result.len() == 3` and fails with `result.len() == 1`, text
    /// "Please review the following items:\n1. First item\n2. Second item".
    #[cfg(feature = "layout-detection")]
    #[test]
    fn test_push_body_group_keeps_lead_in_prose_separate_from_following_list_run() {
        let lines = vec![
            list_test_paragraph("Please review the following items:", (10.0, 820.0, 500.0, 840.0)),
            list_test_paragraph("1. First item", (10.0, 780.0, 500.0, 800.0)),
            list_test_paragraph("2. Second item", (10.0, 760.0, 500.0, 780.0)),
        ];

        let mut result = Vec::new();
        push_body_group(&mut result, lines);

        assert_eq!(
            result.len(),
            3,
            "lead-in prose and each list item must be separate paragraphs"
        );
        assert_eq!(result[0].text, "Please review the following items:");
        assert_eq!(result[1].text, "1. First item");
        assert_eq!(result[2].text, "2. Second item");
    }

    /// #729: a marker isolated into its own paragraph by `merge_structural_group`
    /// (simulated directly here rather than via the full ML-hint pipeline) must be
    /// rejoined onto the very next, otherwise-unclassified paragraph when their
    /// geometry lines up. Against unfixed code (no `reattach_ocr_layout_list_markers`
    /// call at all) this would assert `result.len() == 1` and fail with
    /// `result.len() == 2`, and `result[0].text == "1. Overview of the setback requirements"`
    /// would fail against `"1."`. With `REATTACH_OCR_LAYOUT_LIST_MARKERS` flipped to
    /// `false` the same failures reproduce, since the function becomes a no-op.
    #[cfg(feature = "layout-detection")]
    #[test]
    fn reattach_ocr_layout_list_markers_rejoins_marker_and_immediate_body() {
        let mut marker = list_test_paragraph("1.", (10.0, 780.0, 30.0, 800.0));
        marker.is_list_item = true;
        let body = list_test_paragraph("Overview of the setback requirements", (35.0, 780.0, 500.0, 800.0));

        let mut paragraphs = vec![marker, body];
        reattach_ocr_layout_list_markers(&mut paragraphs, 0);

        assert_eq!(paragraphs.len(), 1, "marker and body must merge into one paragraph");
        assert!(paragraphs[0].is_list_item);
        assert_eq!(paragraphs[0].text, "1. Overview of the setback requirements");
    }

    /// #729 (redefined): a bare marker paragraph that NO ML layout hint ever
    /// classified -- `is_list_item` left at its default `false`, unlike the
    /// `reattach_ocr_layout_list_markers` fixtures above which set it to `true` to
    /// simulate a fired hint -- is outside that function's precondition and stays
    /// unmerged. `reattach_detached_ocr_list_markers` covers exactly this shape by
    /// delegating to the native pipeline's own baseline-paired
    /// `reattach_detached_list_markers`, whose marker-side test requires the
    /// opposite precondition (`is_list_item == false`).
    ///
    /// Against unfixed code (no `reattach_detached_ocr_list_markers` wired into any
    /// OCR call path) this asserts `paragraphs.len() == 1` and
    /// `paragraphs[0].is_list_item == true`; today `reattach_detached_ocr_list_markers`
    /// does not exist at all, so calling it is itself the change under test, and
    /// without it the two paragraphs stay unmerged (`len() == 2`) with the marker
    /// segment never spliced into the body's first line -- the exact #729 symptom
    /// (`(a)`, `(b)`, `(c)` rendered as plain text).
    ///
    /// `reattach_detached_ocr_list_markers` wraps
    /// `pipeline::reattach_detached_list_markers`, which -- by design -- does
    /// `body.text.clear()` after splicing the marker segment into `body.lines`
    /// (see that function's comment: "Text is rebuilt from `lines` downstream").
    /// The rebuild is `synchronize_paragraph_text_metadata` /
    /// `join_line_texts_plain` in `assembly.rs`, which this unit test never calls,
    /// so `paragraphs[0].text` is `""` here BY CONSTRUCTION and can never equal
    /// the joined string. Assert on the thing this function actually mutates
    /// instead: the spliced segment list.
    #[cfg(feature = "layout-detection")]
    #[test]
    fn reattach_detached_ocr_list_markers_rejoins_a_marker_no_ml_hint_ever_classified() {
        let marker = list_test_paragraph("1.", (10.0, 780.0, 30.0, 800.0));
        let body = list_test_paragraph("Overview of the setback requirements", (35.0, 780.0, 500.0, 800.0));
        assert!(
            !marker.is_list_item,
            "fixture must start unclassified, unlike the ML-hint fixtures above"
        );

        let mut paragraphs = vec![marker, body];
        reattach_detached_ocr_list_markers(&mut paragraphs, 0);

        assert_eq!(paragraphs.len(), 1, "marker and body must merge into one paragraph");
        assert!(
            paragraphs[0].is_list_item,
            "the merged paragraph must be classified as a list item"
        );
        let segment_texts = paragraphs[0].lines[0]
            .segments
            .iter()
            .map(|segment| segment.text.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            segment_texts,
            vec!["1.", "Overview of the setback requirements"],
            "the marker segment must be spliced in ahead of the body segment; \
             .text itself is deliberately left empty for downstream rebuild"
        );
    }

    /// Helper: an OCR paragraph carrying the given raw raster geometry (`x`,
    /// `y` == `baseline_y`, `width`, `height`, `font_size`), matching the shape
    /// [`make_ocr_pdf_line`] always produces for OCR segments
    /// (`rotation_degrees == 0.0`).
    fn rotated_list_test_paragraph(
        text: &str,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        font_size: f32,
    ) -> types::PdfParagraph {
        let line = make_ocr_pdf_line(text, x, y, width, height, font_size, false, false);
        make_ocr_paragraph(
            text.to_string(),
            vec![line],
            Some((x, y, x + width, y + height)),
            font_size,
        )
    }

    /// #760: on a page with a PDF `/Rotate 270`, the marker/body pair measured on
    /// fixture `ordinance_2197` (marker `y=2378.0 h=65.0`, body `y=1324.0
    /// h=933.0`, `font_size=30.0` -- see `pipeline::tests::ocr_frame_270_...`
    /// for the derivation of the exact numbers) must be rejoined once
    /// `page_rotation_degrees` is threaded through to
    /// `pipeline::accepts_detached_list_marker` as `DetachedMarkerFrame::OcrOnPage(270)`.
    ///
    /// Against unfixed code -- which had no `page_rotation_degrees` parameter at
    /// all, equivalent to always calling with `0` (the `DetachedMarkerFrame::OcrOnPage(_)`
    /// default arm, byte-identical to the pre-#760 `Native` frame for an
    /// unrotated-tagged OCR segment) -- this asserts `paragraphs.len() == 1` and
    /// FAILS with `paragraphs.len() == 2`: the baseline gate rejects the pair
    /// (delta 1054.0 against tolerance 18.0) before the indent check ever runs,
    /// exactly the #760 symptom (18 detached markers, 0 of their bodies
    /// reattached).
    #[cfg(feature = "layout-detection")]
    #[test]
    fn reattach_ocr_layout_list_markers_pairs_a_270_rotated_marker_using_the_corrected_frame() {
        let mut marker = rotated_list_test_paragraph("(a)", 3317.0, 2378.0, 100.0, 65.0, 30.0);
        marker.is_list_item = true;
        let body = rotated_list_test_paragraph("Buffer requirement", 3367.0, 1324.0, 50.0, 933.0, 30.0);

        let mut unrotated = vec![marker.clone(), body.clone()];
        reattach_ocr_layout_list_markers(&mut unrotated, 0);
        assert_eq!(
            unrotated.len(),
            2,
            "page_rotation_degrees == 0 must NOT correct this rotated-page pair (pre-#760 behaviour)"
        );

        let mut rotated = vec![marker, body];
        reattach_ocr_layout_list_markers(&mut rotated, 270);
        assert_eq!(
            rotated.len(),
            1,
            "page_rotation_degrees == 270 must reattach the marker to its body"
        );
        assert!(rotated[0].is_list_item);
    }

    /// #760: the same fix, exercised through [`reattach_detached_ocr_list_markers`]
    /// (the unclassified-marker mirror -- `is_list_item == false`, unlike the ML-hint
    /// fixture above). See that test's doc comment for the measured numbers and the
    /// unfixed-code prediction: `page_rotation_degrees == 0` must leave the pair
    /// unmerged (`len() == 2`); `270` must merge it (`len() == 1`).
    #[cfg(feature = "layout-detection")]
    #[test]
    fn reattach_detached_ocr_list_markers_pairs_a_270_rotated_marker_using_the_corrected_frame() {
        let marker = rotated_list_test_paragraph("(a)", 3317.0, 2378.0, 100.0, 65.0, 30.0);
        let body = rotated_list_test_paragraph("Buffer requirement", 3367.0, 1324.0, 50.0, 933.0, 30.0);
        assert!(!marker.is_list_item, "fixture must start unclassified");

        let mut unrotated = vec![marker.clone(), body.clone()];
        reattach_detached_ocr_list_markers(&mut unrotated, 0);
        assert_eq!(
            unrotated.len(),
            2,
            "page_rotation_degrees == 0 must NOT correct this rotated-page pair"
        );

        let mut rotated = vec![marker, body];
        reattach_detached_ocr_list_markers(&mut rotated, 270);
        assert_eq!(
            rotated.len(),
            1,
            "page_rotation_degrees == 270 must reattach the marker to its body"
        );
        assert!(rotated[0].is_list_item);
    }

    /// #729: the body is not necessarily the very next paragraph -- an unrelated,
    /// already-classified paragraph (here a heading) may sit between the marker and
    /// its body, mirroring the native `DETACHED_MARKER_MAX_LOOKAHEAD` case. Against
    /// unfixed code this asserts `result.len() == 2` and fails with `result.len() == 3`
    /// (nothing merged).
    #[cfg(feature = "layout-detection")]
    #[test]
    fn reattach_ocr_layout_list_markers_looks_ahead_past_an_unrelated_paragraph() {
        let mut marker = list_test_paragraph("1.", (10.0, 900.0, 30.0, 920.0));
        marker.is_list_item = true;
        let mut heading = list_test_paragraph("Unrelated Section", (10.0, 840.0, 500.0, 860.0));
        heading.heading_level = Some(2);
        // Same baseline as the marker: a marker column stacks markers vertically and
        // puts each body to the RIGHT of its own marker, so the body a lookahead has
        // to reach still shares that marker's baseline. A body on a different
        // baseline is correctly rejected by `accepts_detached_list_marker`. ~keep
        let body = list_test_paragraph("Overview of the setback requirements", (35.0, 900.0, 500.0, 920.0));

        let mut paragraphs = vec![marker, heading, body];
        reattach_ocr_layout_list_markers(&mut paragraphs, 0);

        assert_eq!(
            paragraphs.len(),
            2,
            "the unrelated heading must not be consumed as a body"
        );
        assert_eq!(paragraphs[0].heading_level, Some(2));
        assert!(paragraphs[1].is_list_item);
        assert_eq!(paragraphs[1].text, "1. Overview of the setback requirements");
    }

    /// #729 false-positive guard: a single-word neighbour must not be treated as a
    /// body (the reused `DETACHED_MARKER_MIN_BODY_WORDS` guard). This does NOT
    /// discriminate under `REATTACH_OCR_LAYOUT_LIST_MARKERS = false` -- with the
    /// pass disabled the paragraphs are also left unmerged, so both states produce
    /// `result.len() == 2`. It verifies the reused guard is actually wired in, not
    /// the on/off switch.
    #[cfg(feature = "layout-detection")]
    #[test]
    fn reattach_ocr_layout_list_markers_does_not_merge_a_one_word_neighbour() {
        let mut marker = list_test_paragraph("1.", (10.0, 780.0, 30.0, 800.0));
        marker.is_list_item = true;
        let body = list_test_paragraph("Overview", (35.0, 780.0, 500.0, 800.0));

        let mut paragraphs = vec![marker, body];
        reattach_ocr_layout_list_markers(&mut paragraphs, 0);

        assert_eq!(
            paragraphs.len(),
            2,
            "a one-word neighbour must not be absorbed as a body"
        );
    }

    /// #729 false-positive guard: a neighbour that is ALREADY its own list item
    /// (e.g. a genuinely independent "2. Second item") must not be swallowed as
    /// this marker's body. Same honesty caveat as the one-word-neighbour test: does
    /// not discriminate under the const flip, verifies the reused
    /// `!paragraph.is_list_item` guard.
    #[cfg(feature = "layout-detection")]
    #[test]
    fn reattach_ocr_layout_list_markers_does_not_swallow_an_already_classified_neighbour() {
        let mut marker = list_test_paragraph("1.", (10.0, 780.0, 30.0, 800.0));
        marker.is_list_item = true;
        let mut second_item = list_test_paragraph("2. Second item", (10.0, 760.0, 500.0, 780.0));
        second_item.is_list_item = true;

        let mut paragraphs = vec![marker, second_item];
        reattach_ocr_layout_list_markers(&mut paragraphs, 0);

        assert_eq!(
            paragraphs.len(),
            2,
            "an already-classified neighbour must not be absorbed"
        );
        assert_eq!(paragraphs[1].text, "2. Second item");
    }

    /// #729 false-positive guard: a candidate far outside the hanging-indent bound
    /// (110pt against a 72pt bound at the 12pt default test font size) must not be
    /// treated as this marker's body, even though it is otherwise word-count- and
    /// classification-eligible. Same honesty caveat: does not discriminate under
    /// the const flip, verifies the reused geometry bound.
    #[cfg(feature = "layout-detection")]
    #[test]
    fn reattach_ocr_layout_list_markers_respects_the_hanging_indent_bound() {
        let mut marker = list_test_paragraph("1.", (10.0, 780.0, 30.0, 800.0));
        marker.is_list_item = true;
        let body = list_test_paragraph("Overview of the setback requirements", (140.0, 780.0, 600.0, 800.0));

        let mut paragraphs = vec![marker, body];
        reattach_ocr_layout_list_markers(&mut paragraphs, 0);

        assert_eq!(
            paragraphs.len(),
            2,
            "a candidate outside the hanging-indent bound must not merge"
        );
    }

    /// #729 marker-run/body-run pairing: a marker COLUMN ("(a)", "(b)", "(c)")
    /// stacked directly above a body COLUMN of three paragraphs, in the same order,
    /// must pair positionally even though no marker and no body share a baseline --
    /// ordinary single-spaced leading is far outside
    /// `DETACHED_MARKER_BASELINE_TOLERANCE_FONT_FACTOR`, so the single-marker,
    /// baseline-paired pass above cannot pair any of these six paragraphs. Against
    /// unfixed code (no marker-run pairing phase, or
    /// `REATTACH_OCR_LAYOUT_MARKER_RUN_PAIRS` flipped to `false`) this asserts
    /// `paragraphs.len() == 3` and fails with `paragraphs.len() == 6` (nothing
    /// merges).
    #[cfg(feature = "layout-detection")]
    #[test]
    fn reattach_ocr_layout_list_markers_pairs_a_stacked_marker_run_with_a_stacked_body_run() {
        let mut marker_a = list_test_paragraph("(a)", (10.0, 900.0, 30.0, 920.0));
        marker_a.is_list_item = true;
        let mut marker_b = list_test_paragraph("(b)", (10.0, 880.0, 30.0, 900.0));
        marker_b.is_list_item = true;
        let mut marker_c = list_test_paragraph("(c)", (10.0, 860.0, 30.0, 880.0));
        marker_c.is_list_item = true;

        let body1 = list_test_paragraph(
            "A ten foot minimum buffer applies to this parcel.",
            (10.0, 840.0, 500.0, 860.0),
        );
        let body2 = list_test_paragraph(
            "Ten foot buffers apply similarly to adjoining parcels.",
            (10.0, 820.0, 500.0, 840.0),
        );
        let body3 = list_test_paragraph(
            "Buffers must be maintained along every property line.",
            (10.0, 800.0, 500.0, 820.0),
        );

        let mut paragraphs = vec![marker_a, marker_b, marker_c, body1, body2, body3];
        reattach_ocr_layout_list_markers(&mut paragraphs, 0);

        assert_eq!(
            paragraphs.len(),
            3,
            "the marker run and the following body run must pair positionally"
        );
        assert!(paragraphs.iter().all(|paragraph| paragraph.is_list_item));
        assert!(paragraphs[0].text.starts_with("(a) "), "got {:?}", paragraphs[0].text);
        assert!(paragraphs[1].text.starts_with("(b) "), "got {:?}", paragraphs[1].text);
        assert!(paragraphs[2].text.starts_with("(c) "), "got {:?}", paragraphs[2].text);
    }

    /// #729 negative: a marker RUN of length one must never pair through the
    /// run-pairing phase, even with an eligible body run immediately following it --
    /// see `DETACHED_MARKER_RUN_MIN_LENGTH`'s doc comment (a lone bare marker is
    /// exactly as likely to be an exhibit label as a genuine list marker). The
    /// fixture is geometrically stacked, not side-by-side, so the single-marker,
    /// baseline-paired pass above cannot pair it either. This test does NOT
    /// discriminate on `REATTACH_OCR_LAYOUT_MARKER_RUN_PAIRS` (both `true` and
    /// `false` leave `paragraphs.len() == 4`, since the min-length guard rejects the
    /// run before pairing regardless) -- it verifies `DETACHED_MARKER_RUN_MIN_LENGTH`
    /// specifically. Against code with `DETACHED_MARKER_RUN_MIN_LENGTH` lowered to
    /// `1` this asserts `paragraphs.len() == 4` and fails with
    /// `paragraphs.len() == 3`.
    #[cfg(feature = "layout-detection")]
    #[test]
    fn reattach_ocr_layout_list_markers_does_not_pair_a_run_of_length_one() {
        let mut marker_a = list_test_paragraph("(a)", (10.0, 900.0, 30.0, 920.0));
        marker_a.is_list_item = true;

        let body1 = list_test_paragraph(
            "A ten foot minimum buffer applies to this parcel.",
            (10.0, 840.0, 500.0, 860.0),
        );
        let body2 = list_test_paragraph(
            "Ten foot buffers apply similarly to adjoining parcels.",
            (10.0, 820.0, 500.0, 840.0),
        );
        let body3 = list_test_paragraph(
            "Buffers must be maintained along every property line.",
            (10.0, 800.0, 500.0, 820.0),
        );

        let mut paragraphs = vec![marker_a, body1, body2, body3];
        reattach_ocr_layout_list_markers(&mut paragraphs, 0);

        assert_eq!(
            paragraphs.len(),
            4,
            "a run of length one must not be paired by the run-pairing phase"
        );
    }

    /// A single marker-opening line is not enough signal to SPLIT -- e.g. a plain
    /// paragraph that happens to start "1. Overview covers...". This is the guard for
    /// `MIN_LIST_MARKERS_TO_SPLIT`: the segment count assertion (`result.len() == 1`)
    /// passes on both unfixed and fixed code, since unfixed code always merges into one
    /// paragraph regardless of marker count and the guard keeps fixed code doing the
    /// same below the two-marker threshold.
    ///
    /// The TEXT assertion is not guard-neutral, though: this segment's first line still
    /// satisfies `looks_like_list_item` (it is a real, if lone, marker), so
    /// `join_body_segment_text` treats it as marker-led and space-joins it -- because
    /// `apply_ocr_text_list_fallback` downstream flags any paragraph starting with a
    /// marker as `is_list_item = true` regardless of segment count, so a lone-marker
    /// paragraph is just as exposed to the embedded-newline-becomes-block-break bug as a
    /// split one. Against the code as it stood before `join_body_segment_text` (plain
    /// `"\n"`-join in `build_body_paragraph`, still the current in-tree state before this
    /// fix), this assert fails: the actual value is
    /// "1. Overview covers the whole document.\nIt spans several themes." (embedded
    /// newline), not the space-joined text asserted below.
    #[cfg(feature = "layout-detection")]
    #[test]
    fn test_push_body_group_does_not_split_on_a_single_marker_opening_line() {
        let lines = vec![
            list_test_paragraph("1. Overview covers the whole document.", (10.0, 780.0, 500.0, 800.0)),
            list_test_paragraph("It spans several themes.", (10.0, 760.0, 500.0, 780.0)),
        ];

        let mut result = Vec::new();
        push_body_group(&mut result, lines);

        assert_eq!(result.len(), 1, "a lone marker-opening line must not trigger a split");
        assert_eq!(
            result[0].text, "1. Overview covers the whole document. It spans several themes.",
            "a marker-led paragraph must space-join, even when push_body_group did not split it, \
             because the downstream is_list_item fallback keys off the leading marker alone"
        );
    }

    /// A wrapped continuation line carries no marker of its own and must stay glued to
    /// the item above it, AND the join between it and its marker line must be a space,
    /// not a newline -- an embedded newline inside a paragraph the downstream
    /// `is_list_item` fallback flags as a list item gets rendered as a block break,
    /// which turned the continuation into its own standalone markdown paragraph in
    /// production (`multi_page_scanned.pdf`, `--ocr-scanned-pages --layout`, tesseract).
    ///
    /// Segment count: against code with no split logic at all, `result.len() == 2`
    /// fails with `result.len() == 1`. Join style: against the code as it stood right
    /// after splitting was added (correct segments, but `build_body_paragraph` still
    /// joined every segment with `"\n"`, the current in-tree state before this fix),
    /// `result[0].text` fails -- the actual value is
    /// "1. First item\ncontinues on wrapped line" (embedded newline), not the
    /// space-joined text asserted below.
    #[cfg(feature = "layout-detection")]
    #[test]
    fn test_push_body_group_keeps_wrapped_continuation_lines_with_their_item() {
        let lines = vec![
            list_test_paragraph("1. First item", (10.0, 780.0, 500.0, 800.0)),
            list_test_paragraph("continues on wrapped line", (10.0, 760.0, 500.0, 780.0)),
            list_test_paragraph("2. Second item", (10.0, 740.0, 500.0, 760.0)),
        ];

        let mut result = Vec::new();
        push_body_group(&mut result, lines);

        assert_eq!(
            result.len(),
            2,
            "the wrapped line must stay with item 1, not start a new paragraph"
        );
        assert_eq!(
            result[0].text, "1. First item continues on wrapped line",
            "a marker-led segment's wrapped continuation must space-join, not newline-join, \
             or the renderer emits it as a separate block"
        );
        assert_eq!(
            result[0].block_bbox,
            Some((10.0, 760.0, 500.0, 800.0)),
            "item 1's bbox must union only its own two lines, not the whole original group"
        );
        assert_eq!(result[1].text, "2. Second item");
        assert_eq!(result[1].block_bbox, Some((10.0, 740.0, 500.0, 760.0)));
    }

    /// Production shape: a marker-led segment with TWO wrapped continuation lines (not
    /// just one), each of which must glue onto the item with a single space, in order,
    /// while the second list item stays a separate paragraph. Against the code as it
    /// stood right after splitting was added (segments correct, `"\n"`-join still in
    /// `build_body_paragraph`, the current in-tree state before this fix), `result[0].text`
    /// fails: the actual value is
    /// "1. Undo/Redo: introduced in the 1980s\nmade experimentation easier\nand remains \
    /// popular today" (two embedded newlines), not the single-line space-joined text
    /// asserted below.
    #[cfg(feature = "layout-detection")]
    #[test]
    fn test_push_body_group_space_joins_marker_led_segment_with_two_continuation_lines() {
        let lines = vec![
            list_test_paragraph("1. Undo/Redo: introduced in the 1980s", (10.0, 800.0, 500.0, 820.0)),
            list_test_paragraph("made experimentation easier", (10.0, 780.0, 500.0, 800.0)),
            list_test_paragraph("and remains popular today", (10.0, 760.0, 500.0, 780.0)),
            list_test_paragraph(
                "2. Spell Check: became standard in the 1990s",
                (10.0, 740.0, 500.0, 760.0),
            ),
        ];

        let mut result = Vec::new();
        push_body_group(&mut result, lines);

        assert_eq!(
            result.len(),
            2,
            "each list item, including its continuations, is one paragraph"
        );
        assert_eq!(
            result[0].text,
            "1. Undo/Redo: introduced in the 1980s made experimentation easier and remains popular today",
            "both continuation lines must space-join onto item 1, in order"
        );
        assert_eq!(result[1].text, "2. Spell Check: became standard in the 1990s");
    }

    /// A continuation line with stray leading/trailing whitespace must not produce a
    /// double space when space-joined onto its marker line. Against the code as it stood
    /// right after splitting was added (segments correct, `"\n"`-join still in
    /// `build_body_paragraph`, the current in-tree state before this fix),
    /// `result[0].text` fails: the actual value is
    /// "1. First item \n  continues with extra spacing  " (original whitespace and a
    /// newline preserved verbatim), not the single-space-normalized text asserted below.
    #[cfg(feature = "layout-detection")]
    #[test]
    fn test_push_body_group_marker_led_join_avoids_double_spaces_from_stray_whitespace() {
        let lines = vec![
            list_test_paragraph("1. First item ", (10.0, 780.0, 500.0, 800.0)),
            list_test_paragraph("  continues with extra spacing  ", (10.0, 760.0, 500.0, 780.0)),
            list_test_paragraph("2. Second item", (10.0, 740.0, 500.0, 760.0)),
        ];

        let mut result = Vec::new();
        push_body_group(&mut result, lines);

        assert_eq!(result.len(), 2);
        assert_eq!(
            result[0].text, "1. First item continues with extra spacing",
            "trailing/leading whitespace on either line must not survive as a double space"
        );
        assert_eq!(result[1].text, "2. Second item");
    }
}
