//! Drawn ruling-line detection from a PDF page's vector paths.
//!
//! Shared by the adaptive layout pre-screen ([`super::layout_gate`], which is
//! only compiled with the `layout-detection` feature) and the borderless
//! heuristic table gate in [`super::oxide::table`], which is compiled whenever
//! `pdf` is. It lives here rather than in `layout_gate` so the heuristic gate's
//! behaviour does not silently change with the `layout-detection` feature — a
//! table admitted in one build and rejected in another would be worse than
//! either answer on its own (xberg-io/xberg#1399).

/// Horizontal tolerance when clustering span edges into table column anchors,
/// and the maximum thickness of a line still considered a rule.
pub(crate) const GRID_EDGE_ALIGN_TOLERANCE_PTS: f32 = 3.0;

/// Straight lines shorter than this are decoration, not rules, in points.
pub(crate) const RULED_LINE_MIN_LENGTH_PTS: f32 = 20.0;

/// Count horizontal and vertical rules at least [`RULED_LINE_MIN_LENGTH_PTS`]
/// long. A rule must also be thin (minor bbox dimension within
/// [`GRID_EDGE_ALIGN_TOLERANCE_PTS`]) so long diagonals on chart-heavy pages
/// do not count.
pub(crate) fn count_rules(paths: &[pdf_oxide::elements::PathContent]) -> (usize, usize) {
    let mut horizontal = 0usize;
    let mut vertical = 0usize;
    for path in paths.iter().filter(|path| path.is_straight_line()) {
        let width = path.bbox.width.abs();
        let height = path.bbox.height.abs();
        if width >= RULED_LINE_MIN_LENGTH_PTS && height <= GRID_EDGE_ALIGN_TOLERANCE_PTS {
            horizontal += 1;
        } else if height >= RULED_LINE_MIN_LENGTH_PTS && width <= GRID_EDGE_ALIGN_TOLERANCE_PTS {
            vertical += 1;
        }
    }
    (horizontal, vertical)
}
