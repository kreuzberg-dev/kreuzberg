//! Per-page pre-screen for adaptive layout detection ([`LayoutStrategy::Auto`]).
//!
//! Grades every page from cheap geometry the PDF already carries (text-span
//! boxes, straight lines, raster and vector graphics, widget annotations) and
//! decides whether the expensive render + ONNX layout pass can pay off there.
//!
//! The gate is recall-biased: any structure evidence, sparse or absent text,
//! or a failure to gather signals selects the page for the model. Only pages
//! that positively look like plain single-column text are skipped, and a
//! skipped page processes exactly like a page where the model ran and found
//! no regions.
//!
//! All signal gathering is advisory and pixel-free, in the same shape as
//! [`super::scan_detect`]: a page that cannot be inspected must never abort
//! extraction, it just runs the model.
//!
//! [`LayoutStrategy::Auto`]: crate::core::config::layout::LayoutStrategy::Auto

use pdf_oxide::PdfDocument;
use pdf_oxide::annotation_types::AnnotationSubtype;
use pdf_oxide::document::ReadingOrder;

/// Pages with fewer spans than this look like slides, posters, or covers:
/// too little text to trust geometry, so the model runs.
const SPARSE_PAGE_MAX_SPANS: usize = 8;

/// Pages whose text layer carries fewer non-whitespace characters than this
/// are treated as having no usable text layer, so the model runs.
const TEXT_LAYER_MIN_CHARS: usize = 40;

/// Minimum width of an empty vertical band read as a column gutter, in points.
const COLUMN_GUTTER_MIN_WIDTH_PTS: f32 = 12.0;

/// Minimum spans fully on each side of a candidate gutter.
///
/// Deliberately far below the production column splitter's floor: a missed
/// split corrupts reading order, so the gate over-selects rather than trusts
/// the precision-tuned extraction constants.
const COLUMN_SIDE_MIN_SPANS: usize = 4;

/// Each side of a gutter must span at least this fraction of the text height.
const COLUMN_SIDE_MIN_HEIGHT_FRACTION: f32 = 0.15;

/// Horizontal tolerance when clustering span edges into table column anchors.
const GRID_EDGE_ALIGN_TOLERANCE_PTS: f32 = 3.0;

/// A span edge cluster must recur on this many distinct rows to count as a
/// table column anchor.
const GRID_MIN_ROWS: usize = 3;

/// Distinct aligned column anchors needed to read a page as table-bearing.
///
/// Two suffices (key-value blocks, two-column financial tables); the
/// production detector requires three, which the gate deliberately relaxes.
const GRID_MIN_COLS: usize = 2;

/// Rows are clustered on span vertical centers within this tolerance, points.
const GRID_ROW_TOLERANCE_PTS: f32 = 5.0;

/// Straight lines shorter than this are decoration, not rules, in points.
const RULED_LINE_MIN_LENGTH_PTS: f32 = 20.0;

/// Ruled-line evidence needs at least this many rules per orientation, so a
/// single underline or divider does not select the page.
const RULED_LINES_MIN_PER_ORIENTATION: usize = 2;

/// Combined raster and vector coverage at or above this fraction reads the
/// page as figure- or chart-bearing.
const GRAPHICS_COVERAGE_MIN: f32 = 0.15;

/// Why the gate selected (or skipped) a page.
///
/// Recorded per page so `Auto` runs are auditable from extraction metadata.
#[cfg_attr(alef, alef(skip))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GateReason {
    /// Too little text to trust geometry signals.
    SparseText,
    /// No usable native text layer; the model is the only structure source.
    NoTextLayer,
    /// An empty vertical band splits the text into two populated columns.
    MultiColumn,
    /// Span edges align into recurring column anchors across rows.
    TableGrid,
    /// Horizontal and vertical rules suggest a grid or form.
    RuledLines,
    /// Raster plus vector graphics cover a meaningful page fraction.
    GraphicsHeavy,
    /// The page carries interactive form widgets.
    FormWidgets,
    /// Signal gathering failed; the gate must not guess.
    SignalsUnavailable,
    /// Plain single-column text: the model has nothing to find.
    PlainText,
}

/// One page's gate outcome.
#[cfg_attr(alef, alef(skip))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PageGateDecision {
    /// Whether the layout model runs on this page.
    pub run_layout: bool,
    /// The dominant reason for the decision.
    pub reason: GateReason,
}

impl PageGateDecision {
    fn run(reason: GateReason) -> Self {
        Self {
            run_layout: true,
            reason,
        }
    }

    fn skip() -> Self {
        Self {
            run_layout: false,
            reason: GateReason::PlainText,
        }
    }
}

/// Axis-aligned span box in PDF points, the only geometry the gate reads.
#[cfg_attr(alef, alef(skip))]
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct SpanBox {
    pub left: f32,
    pub bottom: f32,
    pub right: f32,
    pub top: f32,
}

impl SpanBox {
    fn height(&self) -> f32 {
        (self.top - self.bottom).abs()
    }

    fn vertical_center(&self) -> f32 {
        (self.top + self.bottom) / 2.0
    }
}

/// Everything [`decide_page`] needs, gathered without decoding any pixels.
#[cfg_attr(alef, alef(skip))]
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PageGateSignals {
    /// Text-span boxes in PDF points.
    pub spans: Vec<SpanBox>,
    /// Non-whitespace characters in the native text layer.
    pub text_chars: usize,
    /// Horizontal rules at least [`RULED_LINE_MIN_LENGTH_PTS`] long.
    pub horizontal_rules: usize,
    /// Vertical rules at least [`RULED_LINE_MIN_LENGTH_PTS`] long.
    pub vertical_rules: usize,
    /// Fraction of the page under raster images plus vector paths, `[0, 1]`.
    ///
    /// Overlaps are summed, not unioned: an upper bound that can over-select
    /// a page for the model, never under-select one.
    pub graphics_coverage: f32,
    /// Whether the page carries widget (form field) annotations.
    pub has_form_widgets: bool,
}

/// Decide one page from its signals. Pure, so it is testable without a
/// [`PdfDocument`].
///
/// Checks are ordered cheapest-first; the page is skipped only when every
/// run-condition stays quiet.
pub(crate) fn decide_page(signals: &PageGateSignals) -> PageGateDecision {
    if signals.text_chars < TEXT_LAYER_MIN_CHARS {
        return PageGateDecision::run(GateReason::NoTextLayer);
    }
    if signals.spans.len() < SPARSE_PAGE_MAX_SPANS {
        return PageGateDecision::run(GateReason::SparseText);
    }
    if signals.has_form_widgets {
        return PageGateDecision::run(GateReason::FormWidgets);
    }
    if signals.horizontal_rules >= RULED_LINES_MIN_PER_ORIENTATION
        && signals.vertical_rules >= RULED_LINES_MIN_PER_ORIENTATION
    {
        return PageGateDecision::run(GateReason::RuledLines);
    }
    if signals.graphics_coverage >= GRAPHICS_COVERAGE_MIN {
        return PageGateDecision::run(GateReason::GraphicsHeavy);
    }
    if has_column_split(&signals.spans) {
        return PageGateDecision::run(GateReason::MultiColumn);
    }
    if has_aligned_grid(&signals.spans) {
        return PageGateDecision::run(GateReason::TableGrid);
    }
    PageGateDecision::skip()
}

/// Whether an empty vertical band splits the spans into two populated sides.
///
/// Sweeps the x-projection of all span intervals: a maximal gap wider than
/// [`COLUMN_GUTTER_MIN_WIDTH_PTS`] with [`COLUMN_SIDE_MIN_SPANS`] spans on
/// each side, both sides covering [`COLUMN_SIDE_MIN_HEIGHT_FRACTION`] of the
/// text height, reads as a column gutter.
fn has_column_split(spans: &[SpanBox]) -> bool {
    if spans.len() < COLUMN_SIDE_MIN_SPANS * 2 {
        return false;
    }
    let text_height = vertical_extent(spans.iter());
    if text_height <= f32::EPSILON {
        return false;
    }

    let mut by_left: Vec<&SpanBox> = spans.iter().collect();
    by_left.sort_by(|a, b| a.left.total_cmp(&b.left));

    let mut covered_right = by_left[0].right;
    for (index, span) in by_left.iter().enumerate().skip(1) {
        let gap = span.left - covered_right;
        if gap >= COLUMN_GUTTER_MIN_WIDTH_PTS && sides_populated(&by_left, index, text_height) {
            return true;
        }
        covered_right = covered_right.max(span.right);
    }
    false
}

/// Whether both sides of the gap before `split_index` hold enough spans and
/// vertical coverage to be real columns.
fn sides_populated(by_left: &[&SpanBox], split_index: usize, text_height: f32) -> bool {
    let (left, right) = by_left.split_at(split_index);
    if left.len() < COLUMN_SIDE_MIN_SPANS || right.len() < COLUMN_SIDE_MIN_SPANS {
        return false;
    }
    let min_height = text_height * COLUMN_SIDE_MIN_HEIGHT_FRACTION;
    vertical_extent(left.iter().copied()) >= min_height && vertical_extent(right.iter().copied()) >= min_height
}

fn vertical_extent<'a>(spans: impl Iterator<Item = &'a SpanBox>) -> f32 {
    let mut bottom = f32::INFINITY;
    let mut top = f32::NEG_INFINITY;
    for span in spans {
        bottom = bottom.min(span.bottom);
        top = top.max(span.top);
    }
    if top > bottom { top - bottom } else { 0.0 }
}

/// Whether span left or right edges recur in aligned column anchors across
/// enough rows to look table-bearing.
fn has_aligned_grid(spans: &[SpanBox]) -> bool {
    let rows = cluster_rows(spans);
    let multi_span_rows: Vec<&Vec<&SpanBox>> = rows.iter().filter(|row| row.len() >= GRID_MIN_COLS).collect();
    if multi_span_rows.len() < GRID_MIN_ROWS {
        return false;
    }

    let left_anchors = recurring_edge_clusters(&multi_span_rows, |span| span.left);
    if left_anchors >= GRID_MIN_COLS {
        return true;
    }
    let right_anchors = recurring_edge_clusters(&multi_span_rows, |span| span.right);
    right_anchors >= GRID_MIN_COLS
}

/// Group spans into rows on vertical centers within [`GRID_ROW_TOLERANCE_PTS`].
fn cluster_rows(spans: &[SpanBox]) -> Vec<Vec<&SpanBox>> {
    let mut by_center: Vec<&SpanBox> = spans.iter().collect();
    by_center.sort_by(|a, b| a.vertical_center().total_cmp(&b.vertical_center()));

    let mut rows: Vec<Vec<&SpanBox>> = Vec::new();
    for span in by_center {
        match rows.last_mut() {
            Some(row)
                if (span.vertical_center() - row[0].vertical_center()).abs()
                    <= GRID_ROW_TOLERANCE_PTS.max(span.height() * 0.6) =>
            {
                row.push(span);
            }
            _ => rows.push(vec![span]),
        }
    }
    rows
}

/// Count edge positions that recur on at least [`GRID_MIN_ROWS`] rows.
///
/// Edges are clustered within [`GRID_EDGE_ALIGN_TOLERANCE_PTS`]; each cluster
/// counts the number of distinct rows contributing to it.
fn recurring_edge_clusters(rows: &[&Vec<&SpanBox>], edge: fn(&SpanBox) -> f32) -> usize {
    let mut edges: Vec<(f32, usize)> = rows
        .iter()
        .enumerate()
        .flat_map(|(row_index, row)| row.iter().map(move |span| (edge(span), row_index)))
        .collect();
    edges.sort_by(|a, b| a.0.total_cmp(&b.0));

    let mut anchors = 0usize;
    let mut cluster_start = 0usize;
    for index in 1..=edges.len() {
        let cluster_ended = index == edges.len() || edges[index].0 - edges[index - 1].0 > GRID_EDGE_ALIGN_TOLERANCE_PTS;
        if cluster_ended {
            let mut row_indices: Vec<usize> = edges[cluster_start..index].iter().map(|(_, row)| *row).collect();
            row_indices.sort_unstable();
            row_indices.dedup();
            if row_indices.len() >= GRID_MIN_ROWS {
                anchors += 1;
            }
            cluster_start = index;
        }
    }
    anchors
}

/// Gather one page's signals from `doc`, or `None` when inspection fails.
///
/// Failures are advisory (matching [`super::scan_detect`]): the caller maps
/// `None` to [`GateReason::SignalsUnavailable`] and runs the model.
fn page_signals(doc: &PdfDocument, page_index: usize) -> Option<PageGateSignals> {
    let page_text = super::oxide::guard_oxide_panic(
        || {
            doc.extract_page_text_with_options(page_index, ReadingOrder::ColumnAware)
                .map_err(|error| error.to_string())
        },
        |message| message,
    )
    .ok()?;

    let spans: Vec<SpanBox> = page_text
        .spans
        .iter()
        .filter(|span| !span.text.trim().is_empty())
        .map(|span| SpanBox {
            left: span.bbox.x,
            bottom: span.bbox.y,
            right: span.bbox.x + span.bbox.width,
            top: span.bbox.y + span.bbox.height,
        })
        .collect();
    let text_chars = page_text
        .spans
        .iter()
        .map(|span| span.text.chars().filter(|c| !c.is_whitespace()).count())
        .sum();

    let (horizontal_rules, vertical_rules) = count_rules(doc, page_index)?;

    Some(PageGateSignals {
        spans,
        text_chars,
        horizontal_rules,
        vertical_rules,
        graphics_coverage: graphics_coverage(doc, page_index)?,
        has_form_widgets: has_form_widgets(doc, page_index)?,
    })
}

/// Count horizontal and vertical rules at least [`RULED_LINE_MIN_LENGTH_PTS`] long.
fn count_rules(doc: &PdfDocument, page_index: usize) -> Option<(usize, usize)> {
    let lines = super::oxide::guard_oxide_panic(
        || doc.extract_lines(page_index).map_err(|error| error.to_string()),
        |message| message,
    )
    .ok()?;

    let mut horizontal = 0usize;
    let mut vertical = 0usize;
    for line in &lines {
        let width = line.bbox.width.abs();
        let height = line.bbox.height.abs();
        if width >= RULED_LINE_MIN_LENGTH_PTS && width > height {
            horizontal += 1;
        } else if height >= RULED_LINE_MIN_LENGTH_PTS && height > width {
            vertical += 1;
        }
    }
    Some((horizontal, vertical))
}

/// Fraction of the page under raster images plus stroked or filled vector
/// paths, without decoding pixels. Summed, not unioned: an upper bound.
fn graphics_coverage(doc: &PdfDocument, page_index: usize) -> Option<f32> {
    let (x0, y0, x1, y1) = doc.get_page_media_box(page_index).ok()?;
    let page_area = ((x1 - x0) * (y1 - y0)).abs();
    if page_area <= f32::EPSILON {
        return None;
    }

    let raster: f32 = doc
        .page_image_handles(page_index)
        .ok()?
        .iter()
        .map(|handle| (handle.bbox.width * handle.bbox.height).abs())
        .sum();
    let vector: f32 = super::oxide::guard_oxide_panic(
        || doc.extract_paths(page_index).map_err(|error| error.to_string()),
        |message| message,
    )
    .ok()?
    .iter()
    .filter(|path| !path.is_straight_line())
    .map(|path| (path.bbox.width * path.bbox.height).abs())
    .sum();

    Some(((raster + vector) / page_area).clamp(0.0, 1.0))
}

fn has_form_widgets(doc: &PdfDocument, page_index: usize) -> Option<bool> {
    let annotations = super::oxide::guard_oxide_panic(
        || doc.get_annotations(page_index).map_err(|error| error.to_string()),
        |message| message,
    )
    .ok()?;
    Some(
        annotations
            .iter()
            .any(|annotation| annotation.subtype_enum == AnnotationSubtype::Widget),
    )
}

/// Decide every page of `doc`.
///
/// Infallible: a page whose signals cannot be gathered runs the model with
/// [`GateReason::SignalsUnavailable`] rather than failing extraction.
pub(crate) fn decide_pages(doc: &PdfDocument, page_count: usize) -> Vec<PageGateDecision> {
    (0..page_count)
        .map(|page_index| match page_signals(doc, page_index) {
            Some(signals) => decide_page(&signals),
            None => PageGateDecision::run(GateReason::SignalsUnavailable),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span(left: f32, bottom: f32, right: f32, top: f32) -> SpanBox {
        SpanBox {
            left,
            bottom,
            right,
            top,
        }
    }

    /// A believable single-column prose page: one span per line, common margins.
    fn prose_spans(lines: usize) -> Vec<SpanBox> {
        (0..lines)
            .map(|line| {
                let top = 720.0 - line as f32 * 14.0;
                span(72.0, top - 10.0, 540.0, top)
            })
            .collect()
    }

    fn prose_signals() -> PageGateSignals {
        PageGateSignals {
            spans: prose_spans(40),
            text_chars: 2_000,
            horizontal_rules: 0,
            vertical_rules: 0,
            graphics_coverage: 0.0,
            has_form_widgets: false,
        }
    }

    #[test]
    fn plain_prose_page_is_skipped() {
        let decision = decide_page(&prose_signals());
        assert!(!decision.run_layout);
        assert_eq!(decision.reason, GateReason::PlainText);
    }

    #[test]
    fn missing_text_layer_runs_the_model() {
        let signals = PageGateSignals {
            text_chars: 0,
            spans: Vec::new(),
            ..prose_signals()
        };
        let decision = decide_page(&signals);
        assert!(decision.run_layout);
        assert_eq!(decision.reason, GateReason::NoTextLayer);
    }

    #[test]
    fn sparse_slide_like_page_runs_the_model() {
        let signals = PageGateSignals {
            spans: prose_spans(3),
            text_chars: 120,
            ..prose_signals()
        };
        let decision = decide_page(&signals);
        assert!(decision.run_layout);
        assert_eq!(decision.reason, GateReason::SparseText);
    }

    #[test]
    fn two_column_page_runs_the_model() {
        let mut spans = Vec::new();
        for line in 0..20 {
            let top = 720.0 - line as f32 * 14.0;
            spans.push(span(72.0, top - 10.0, 290.0, top));
            spans.push(span(322.0, top - 10.0, 540.0, top));
        }
        let signals = PageGateSignals {
            spans,
            ..prose_signals()
        };
        let decision = decide_page(&signals);
        assert!(decision.run_layout);
        assert_eq!(decision.reason, GateReason::MultiColumn);
    }

    #[test]
    fn margin_note_alone_is_not_a_column_split() {
        let mut spans = prose_spans(30);
        // Three short notes in the right margin: populated side too thin. ~keep
        for note in 0..3 {
            let top = 700.0 - note as f32 * 200.0;
            spans.push(span(560.0, top - 8.0, 600.0, top));
        }
        // The gap between body (right edge 540) and notes (left 560) is 20pts
        // wide, but the note side has fewer than COLUMN_SIDE_MIN_SPANS spans. ~keep
        let signals = PageGateSignals {
            spans,
            ..prose_signals()
        };
        assert!(!decide_page(&signals).run_layout);
    }

    #[test]
    fn aligned_word_grid_runs_the_model() {
        // A 4-row, 3-column borderless table embedded under prose. ~keep
        let mut spans = prose_spans(20);
        for row in 0..4 {
            let top = 400.0 - row as f32 * 18.0;
            for column in 0..3 {
                let left = 90.0 + column as f32 * 150.0;
                spans.push(span(left, top - 10.0, left + 60.0, top));
            }
        }
        let signals = PageGateSignals {
            spans,
            ..prose_signals()
        };
        let decision = decide_page(&signals);
        assert!(decision.run_layout);
        assert_eq!(decision.reason, GateReason::TableGrid);
    }

    #[test]
    fn prose_left_margin_alignment_is_not_a_grid() {
        // Every prose line shares one left anchor; one anchor is below
        // GRID_MIN_COLS so justified prose must not read as a table. ~keep
        assert!(!has_aligned_grid(&prose_spans(40)));
    }

    #[test]
    fn ruled_grid_runs_the_model() {
        let signals = PageGateSignals {
            horizontal_rules: 5,
            vertical_rules: 4,
            ..prose_signals()
        };
        let decision = decide_page(&signals);
        assert!(decision.run_layout);
        assert_eq!(decision.reason, GateReason::RuledLines);
    }

    #[test]
    fn horizontal_dividers_alone_do_not_run_the_model() {
        let signals = PageGateSignals {
            horizontal_rules: 3,
            vertical_rules: 0,
            ..prose_signals()
        };
        assert!(!decide_page(&signals).run_layout);
    }

    #[test]
    fn graphics_heavy_page_runs_the_model() {
        let signals = PageGateSignals {
            graphics_coverage: 0.4,
            ..prose_signals()
        };
        let decision = decide_page(&signals);
        assert!(decision.run_layout);
        assert_eq!(decision.reason, GateReason::GraphicsHeavy);
    }

    #[test]
    fn small_inline_figure_does_not_run_the_model() {
        let signals = PageGateSignals {
            graphics_coverage: 0.05,
            ..prose_signals()
        };
        assert!(!decide_page(&signals).run_layout);
    }

    #[test]
    fn form_widgets_run_the_model() {
        let signals = PageGateSignals {
            has_form_widgets: true,
            ..prose_signals()
        };
        let decision = decide_page(&signals);
        assert!(decision.run_layout);
        assert_eq!(decision.reason, GateReason::FormWidgets);
    }
}
