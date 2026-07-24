//! Layout-guided PDF reading-order reconstruction.
//!
//! When enabled, this module projects text spans onto layout-detected regions,
//! performs column detection, and reorders spans in natural reading order
//! (top-to-bottom within a column, left-to-right across columns).
//!
//! This is critical for multi-column academic PDFs where native PDF text
//! extraction reads in column order rather than visual reading order.

#[cfg(feature = "layout-detection")]
use crate::pdf::structure::types::{LayoutHint, LayoutHintClass, LayoutRegionPath, LayoutRegionTag};

/// Region x-centers closer than this (in PDF points) are merged into one column.
const COLUMN_MERGE_THRESHOLD_PTS: f32 = 20.0;

/// A text span with bounding box information.
#[derive(Debug, Clone)]
pub struct TextSpan {
    pub text: String,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// Detect columns by clustering region x-centers.
///
/// Analyzes the horizontal positions of regions (using their x-centers) to
/// identify distinct columns. Uses k-means-like clustering with a distance
/// threshold to group regions that belong to the same column.
///
/// Returns a Vec of column assignments, one per region, mapping region index
/// to column ID (0 = leftmost column).
fn detect_columns(regions: &[RegionProjection]) -> Vec<usize> {
    if regions.is_empty() {
        return Vec::new();
    }

    let mut x_centers: Vec<f32> = regions.iter().map(|r| (r.left + r.right) / 2.0).collect();

    x_centers.sort_by(|a, b| a.total_cmp(b));

    let mut unique_centers: Vec<f32> = Vec::new();
    let merge_threshold: f32 = COLUMN_MERGE_THRESHOLD_PTS;

    for &center in &x_centers {
        if let Some(&last) = unique_centers.last() {
            if (center - last).abs() > merge_threshold {
                unique_centers.push(center);
            }
        } else {
            unique_centers.push(center);
        }
    }

    let mut assignments = vec![0usize; regions.len()];
    for (i, region) in regions.iter().enumerate() {
        let center = (region.left + region.right) / 2.0;
        let mut best_col = 0;
        let mut best_dist = f32::INFINITY;

        for (col_id, &cluster_center) in unique_centers.iter().enumerate() {
            let dist = (center - cluster_center).abs();
            if dist < best_dist {
                best_dist = dist;
                best_col = col_id;
            }
        }

        assignments[i] = best_col;
    }

    assignments
}

/// A region projection: layout region with indices of spans it contains.
#[derive(Debug, Clone)]
struct RegionProjection {
    left: f32,
    bottom: f32,
    right: f32,
    top: f32,
    span_indices: Vec<usize>,
}

/// Project spans onto regions using bounding box intersection/containment.
///
/// For each span, determines which region(s) it overlaps with using a simple
/// containment heuristic: if the span's center is within the region, the span
/// belongs to that region.
fn project_spans_to_regions(spans: &[TextSpan], hints: &[LayoutHint]) -> Vec<RegionProjection> {
    let mut regions: Vec<RegionProjection> = hints
        .iter()
        .map(|hint| RegionProjection {
            left: hint.left,
            bottom: hint.bottom,
            right: hint.right,
            top: hint.top,
            span_indices: Vec::new(),
        })
        .collect();

    for (span_idx, span) in spans.iter().enumerate() {
        let span_center_x = span.x + span.width / 2.0;
        let span_center_y = span.y + span.height / 2.0;

        let mut best_region = None;
        let mut best_overlap = 0.0;

        for (region_idx, region) in regions.iter().enumerate() {
            if span_center_x >= region.left
                && span_center_x <= region.right
                && span_center_y >= region.bottom
                && span_center_y <= region.top
            {
                let area = (region.right - region.left) * (region.top - region.bottom);
                if best_region.is_none() || area < best_overlap {
                    best_region = Some(region_idx);
                    best_overlap = area;
                }
            }
        }

        if let Some(region_idx) = best_region {
            regions[region_idx].span_indices.push(span_idx);
        }
    }

    regions.retain(|r| !r.span_indices.is_empty());
    regions
}

/// Tolerance mirroring Docling's `eps` in its bounding-box predicates.
#[cfg(feature = "layout-detection")]
const READING_ORDER_EPS: f32 = 1e-3;

/// Maximum horizontal expansion on either side, relative to the PDF page width.
#[cfg(feature = "layout-detection")]
const HORIZONTAL_DILATION_THRESHOLD_NORM: f32 = 0.15;

/// A layout block (bbox in PDF points, bottom-left origin) used by the
/// predecessor-graph reading-order reconstruction.
#[cfg(feature = "layout-detection")]
#[derive(Debug, Clone, Copy, PartialEq)]
struct OrderBlock {
    left: f32,
    bottom: f32,
    right: f32,
    top: f32,
}

#[cfg(feature = "layout-detection")]
impl OrderBlock {
    /// `self` lies entirely above `other` (bottom-left origin: larger y is higher).
    ///
    /// Port of docling-core `BoundingBox::is_strictly_above` for BOTTOMLEFT origin.
    fn is_strictly_above(&self, other: &OrderBlock) -> bool {
        (self.bottom + READING_ORDER_EPS) > other.top
    }

    /// The two blocks' x-ranges overlap. Strict: touching edges do not count.
    ///
    /// Port of docling-core `BoundingBox::overlaps_horizontally`.
    fn overlaps_horizontally(&self, other: &OrderBlock) -> bool {
        !(self.right <= other.left || other.right <= self.left)
    }
}

/// Reading-order comparator (`Ordering::Less` == `a` precedes `b`).
///
/// Port of docling `PageElement.__lt__`: same-column (horizontally overlapping)
/// blocks order top-to-bottom (higher bottom edge first); otherwise
/// left-to-right (smaller left edge first).
#[cfg(feature = "layout-detection")]
fn reading_order_cmp(a: &OrderBlock, b: &OrderBlock) -> std::cmp::Ordering {
    if a.overlaps_horizontally(b) {
        b.bottom.total_cmp(&a.bottom)
    } else {
        a.left.total_cmp(&b.left)
    }
}

/// Is there a block strictly between `i` and `j` that horizontally overlaps
/// either, interrupting the `i → j` reading-order edge?
///
/// Port of docling `_has_sequence_interruption`. This is what stops a full-width
/// heading or figure sitting between two columns from chaining blocks across them.
#[cfg(feature = "layout-detection")]
fn has_sequence_interruption(blocks: &[OrderBlock], i: usize, j: usize) -> bool {
    let bi = &blocks[i];
    let bj = &blocks[j];
    blocks.iter().enumerate().any(|(w, bw)| {
        w != i
            && w != j
            && (bi.overlaps_horizontally(bw) || bj.overlaps_horizontally(bw))
            && bi.is_strictly_above(bw)
            && bw.is_strictly_above(bj)
    })
}

/// Build the up/down predecessor maps over `blocks`.
///
/// Port of docling `_init_ud_maps`: an edge `i → j` exists when `i` is strictly
/// above `j`, they horizontally overlap, and no third block interrupts the pair.
/// `up[j]` collects predecessors of `j`; `dn[i]` collects successors of `i`.
#[cfg(feature = "layout-detection")]
fn build_updown_maps(blocks: &[OrderBlock]) -> (Vec<Vec<usize>>, Vec<Vec<usize>>) {
    let n = blocks.len();
    let mut up = vec![Vec::new(); n];
    let mut dn = vec![Vec::new(); n];
    for i in 0..n {
        for j in 0..n {
            if i != j
                && blocks[i].is_strictly_above(&blocks[j])
                && blocks[i].overlaps_horizontally(&blocks[j])
                && !has_sequence_interruption(blocks, i, j)
            {
                dn[i].push(j);
                up[j].push(i);
            }
        }
    }
    (up, dn)
}

/// Expand each block horizontally toward its first predecessor and successor.
///
/// This mirrors Docling's effective `_do_horizontal_dilation` behavior: both
/// candidate expansions are derived from the original relation maps and boxes,
/// each side is capped at 15% of the actual PDF page width, and rejection of
/// either candidate leaves the block entirely unchanged.
#[cfg(feature = "layout-detection")]
fn dilate_horizontally(
    blocks: &[OrderBlock],
    up: &[Vec<usize>],
    down: &[Vec<usize>],
    page_width_pts: f32,
) -> Vec<OrderBlock> {
    let threshold = HORIZONTAL_DILATION_THRESHOLD_NORM * page_width_pts;
    blocks
        .iter()
        .enumerate()
        .map(|(index, block)| {
            let mut left = block.left;
            let mut right = block.right;

            if let Some(&predecessor_index) = up[index].first() {
                let predecessor = &blocks[predecessor_index];
                let dilated_left = left.min(predecessor.left);
                let dilated_right = right.max(predecessor.right);
                if left - dilated_left > threshold || dilated_right - right > threshold {
                    return *block;
                }
                left = dilated_left;
                right = dilated_right;
            }

            if let Some(&successor_index) = down[index].first() {
                let successor = &blocks[successor_index];
                let dilated_left = left.min(successor.left);
                let dilated_right = right.max(successor.right);
                if left - dilated_left > threshold || dilated_right - right > threshold {
                    return *block;
                }
                left = dilated_left;
                right = dilated_right;
            }

            OrderBlock { left, right, ..*block }
        })
        .collect()
}

/// Walk up the predecessor map from `start`, always taking the first not-yet-
/// visited predecessor, until reaching a block whose predecessors are all
/// visited. Guarantees every predecessor is emitted before its successor.
///
/// Port of docling `_depth_first_search_upwards` (iterative).
#[cfg(feature = "layout-detection")]
fn walk_to_unvisited_root(start: usize, up: &[Vec<usize>], visited: &[bool]) -> usize {
    let mut k = start;
    loop {
        match up[k].iter().copied().find(|&p| !visited[p]) {
            Some(p) => k = p,
            None => return k,
        }
    }
}

/// Emit `start`'s successor subtree in reading order.
///
/// Port of docling `_depth_first_search_downwards` (iterative, explicit stack).
#[cfg(feature = "layout-detection")]
fn emit_downwards(start: usize, order: &mut Vec<usize>, visited: &mut [bool], up: &[Vec<usize>], dn: &[Vec<usize>]) {
    let mut stack: Vec<(usize, usize)> = vec![(start, 0)];
    while let Some(&(node, offset)) = stack.last() {
        let mut next = offset;
        let mut advanced = false;
        while next < dn[node].len() {
            let child = dn[node][next];
            let root = walk_to_unvisited_root(child, up, visited);
            if !visited[root] {
                order.push(root);
                visited[root] = true;
                let top = stack.len() - 1;
                stack[top].1 = next + 1;
                stack.push((root, 0));
                advanced = true;
                break;
            }
            next += 1;
        }
        if !advanced {
            stack.pop();
        }
    }
}

/// Whether the page is genuinely multi-column: two content blocks sit side by
/// side (their y-ranges overlap while their x-ranges do not).
///
/// Reading-order reorder only helps multi-column pages — single-column stream
/// order already reads top-to-bottom, so reordering it by (often noisy) layout
/// regions is pure downside. This is the defining geometric signal of columns.
#[cfg(feature = "layout-detection")]
fn is_multi_column(blocks: &[OrderBlock]) -> bool {
    for (i, a) in blocks.iter().enumerate() {
        for b in &blocks[i + 1..] {
            let vertical_overlap = !(a.top <= b.bottom || b.top <= a.bottom);
            if vertical_overlap && !a.overlaps_horizontally(b) {
                return true;
            }
        }
    }
    false
}

/// Order `blocks` (layout regions with content) in reading order via the
/// predecessor graph. Returns block indices in reading order.
///
/// Port of docling `ReadingOrderPredictor._predict_page`, including its
/// horizontal dilation refinement when the actual PDF page width is available.
#[cfg(feature = "layout-detection")]
fn order_blocks_by_graph(blocks: &[OrderBlock], page_width_pts: Option<f32>) -> Vec<usize> {
    let n = blocks.len();
    let (raw_up, raw_dn) = build_updown_maps(blocks);
    let (up, mut dn) = match page_width_pts.filter(|width| width.is_finite() && *width > 0.0) {
        Some(page_width_pts) => {
            let dilated = dilate_horizontally(blocks, &raw_up, &raw_dn, page_width_pts);
            build_updown_maps(&dilated)
        }
        None => (raw_up, raw_dn),
    };

    for children in dn.iter_mut() {
        children.sort_by(|&a, &b| reading_order_cmp(&blocks[a], &blocks[b]));
    }

    let mut heads: Vec<usize> = (0..n).filter(|&k| up[k].is_empty()).collect();
    heads.sort_by(|&a, &b| reading_order_cmp(&blocks[a], &blocks[b]));

    let mut visited = vec![false; n];
    let mut order = Vec::with_capacity(n);
    for &head in &heads {
        if !visited[head] {
            order.push(head);
            visited[head] = true;
            emit_downwards(head, &mut order, &mut visited, &up, &dn);
        }
    }
    // Safety net: append any block the traversal missed (degenerate geometry /
    // cycles) so no content is dropped. ~keep
    for (k, &seen) in visited.iter().enumerate() {
        if !seen {
            order.push(k);
        }
    }
    order
}

const MIN_SEGMENT_REGION_COVERAGE: f32 = 0.2;
const MIN_CHILD_REGION_CONTAINMENT: f32 = 0.8;
const MIN_PARTIAL_TEXT_OWNERS: usize = 2;
/// Treat small left-edge variation as indentation/noise; a larger page-relative
/// separation is strong evidence of distinct column origins.
const MAX_SINGLE_COLUMN_LEFT_SPREAD_NORM: f32 = 0.05;
/// All lines must share at least half of the narrowest line's width so a weak
/// overlap between adjacent columns cannot masquerade as one text flow.
const MIN_SINGLE_COLUMN_COMMON_WIDTH_RATIO: f32 = 0.5;
/// Semantic children covering nearly all of a segment outrank their enclosing
/// wrapper. This preserves Title/ListItem/Text classification while a partial
/// child (for example, a narrow caption overlapping a form) cannot steal text.
const MIN_SEMANTIC_CHILD_SEGMENT_COVERAGE: f32 = 0.8;
/// Maximum same-baseline gap that still represents a kerning-run split.
const ATOMIC_FRAGMENT_GAP_RATIO: f32 = 0.15;
/// Require several page-wide body lines before treating a Picture owner as a
/// layout false positive rather than embedded text in a real figure.
const MIN_FALSE_PICTURE_PROSE_LINES: usize = 3;
const MIN_FALSE_PICTURE_ALPHA_CHARS: usize = 40;
const MIN_FALSE_PICTURE_WORDS_PER_LINE: usize = 6;
const MIN_FALSE_PICTURE_ALPHA_RATIO: f32 = 0.65;
const MIN_FALSE_PICTURE_LINE_WIDTH_NORM: f32 = 0.75;
const MIN_FALSE_PICTURE_OUTSIDE_SPAN_NORM: f32 = 0.1;
const MAX_FALSE_PICTURE_LINE_GAP_NORM: f32 = 0.05;
const FALSE_PICTURE_BASELINE_TOLERANCE_RATIO: f32 = 0.35;

/// One root in the page's region-preserving reading-order plan.
///
/// `segment_indices` are indices into the post-table-filter segment slice.
/// `hint_indices` contains only regular classification hints; Table/Picture
/// wrappers establish boundaries but never destructively classify residual text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LayoutSegmentGroup {
    pub(crate) segment_indices: Vec<usize>,
    pub(crate) hint_indices: Vec<usize>,
    pub(crate) region_path: Option<LayoutRegionPath>,
}

#[derive(Debug)]
struct PlannedGroup {
    output: LayoutSegmentGroup,
    root_id: usize,
    order_block: Option<OrderBlock>,
    first_segment_index: usize,
}

fn is_wrapper_hint(hint: &LayoutHint) -> bool {
    hint.class_name.is_wrapper()
}

fn hint_block(hint: &LayoutHint) -> Option<OrderBlock> {
    let coordinates = [hint.left, hint.bottom, hint.right, hint.top];
    if coordinates.iter().any(|coordinate| !coordinate.is_finite())
        || hint.right <= hint.left
        || hint.top <= hint.bottom
    {
        return None;
    }
    let block = OrderBlock {
        left: hint.left,
        bottom: hint.bottom,
        right: hint.right,
        top: hint.top,
    };
    (block_area(&block).is_finite() && block_area(&block) > 0.0).then_some(block)
}

fn block_area(block: &OrderBlock) -> f32 {
    (block.right - block.left) * (block.top - block.bottom)
}

fn block_intersection_area(left: &OrderBlock, right: &OrderBlock) -> f32 {
    let width = (left.right.min(right.right) - left.left.max(right.left)).max(0.0);
    let height = (left.top.min(right.top) - left.bottom.max(right.bottom)).max(0.0);
    let area = width * height;
    if area.is_finite() { area } else { 0.0 }
}

fn confidence_rank(hint: &LayoutHint) -> f32 {
    if hint.confidence.is_finite() {
        hint.confidence
    } else {
        f32::NEG_INFINITY
    }
}

fn segment_block(segment: &crate::pdf::hierarchy::SegmentData) -> Option<OrderBlock> {
    let coordinates = [segment.x, segment.y, segment.width, segment.height];
    if coordinates.iter().any(|coordinate| !coordinate.is_finite()) || segment.width <= 0.0 || segment.height <= 0.0 {
        return None;
    }
    let block = OrderBlock {
        left: segment.x,
        bottom: segment.y,
        right: segment.x + segment.width,
        top: segment.y + segment.height,
    };
    let edges = [block.left, block.bottom, block.right, block.top];
    (edges.iter().all(|edge| edge.is_finite()) && block_area(&block).is_finite() && block_area(&block) > 0.0)
        .then_some(block)
}

fn segments_union_block(indices: &[usize], segments: &[crate::pdf::hierarchy::SegmentData]) -> Option<OrderBlock> {
    let blocks = indices
        .iter()
        .filter_map(|index| segment_block(&segments[*index]))
        .collect::<Vec<_>>();
    (!blocks.is_empty()).then(|| OrderBlock {
        left: blocks.iter().map(|block| block.left).fold(f32::INFINITY, f32::min),
        bottom: blocks.iter().map(|block| block.bottom).fold(f32::INFINITY, f32::min),
        right: blocks.iter().map(|block| block.right).fold(f32::NEG_INFINITY, f32::max),
        top: blocks.iter().map(|block| block.top).fold(f32::NEG_INFINITY, f32::max),
    })
}

fn eligible_hints(hints: &[LayoutHint], wrapper_ownership: &[bool]) -> Vec<bool> {
    hints
        .iter()
        .enumerate()
        .map(|(index, hint)| {
            hint_block(hint).is_some()
                && (!is_wrapper_hint(hint) || wrapper_ownership.get(index).copied().unwrap_or(true))
        })
        .collect()
}

fn choose_wrapper_root(
    child_index: usize,
    hints: &[LayoutHint],
    eligible: &[bool],
    blocks: &[Option<OrderBlock>],
) -> Option<usize> {
    let child = blocks[child_index].as_ref()?;
    let child_area = block_area(child);
    let mut candidates = hints
        .iter()
        .enumerate()
        .filter(|(index, hint)| eligible[*index] && is_wrapper_hint(hint))
        .filter_map(|(index, hint)| {
            let wrapper = blocks[index].as_ref()?;
            let containment = block_intersection_area(child, wrapper) / child_area;
            (containment.is_finite() && containment > MIN_CHILD_REGION_CONTAINMENT).then_some((
                index,
                containment,
                confidence_rank(hint),
                block_area(wrapper),
            ))
        })
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| {
        right
            .1
            .total_cmp(&left.1)
            .then_with(|| right.2.total_cmp(&left.2))
            .then_with(|| left.3.total_cmp(&right.3))
            .then_with(|| left.0.cmp(&right.0))
    });
    candidates.first().map(|candidate| candidate.0)
}

fn root_hint_indices(hints: &[LayoutHint], eligible: &[bool], blocks: &[Option<OrderBlock>]) -> Vec<Option<usize>> {
    hints
        .iter()
        .enumerate()
        .map(|(index, hint)| {
            if !eligible[index] {
                None
            } else if is_wrapper_hint(hint) {
                Some(index)
            } else {
                Some(choose_wrapper_root(index, hints, eligible, blocks).unwrap_or(index))
            }
        })
        .collect()
}

fn choose_segment_owner(
    segment: &crate::pdf::hierarchy::SegmentData,
    hints: &[LayoutHint],
    eligible: &[bool],
    blocks: &[Option<OrderBlock>],
    roots: &[Option<usize>],
) -> Option<usize> {
    let segment_block = segment_block(segment)?;
    let segment_area = block_area(&segment_block);
    let mut candidates = hints
        .iter()
        .enumerate()
        .filter(|(index, _)| eligible[*index])
        .filter_map(|(index, hint)| {
            let region = blocks[index].as_ref()?;
            let coverage = block_intersection_area(&segment_block, region) / segment_area;
            (coverage.is_finite() && coverage > MIN_SEGMENT_REGION_COVERAGE).then_some((
                index,
                coverage,
                confidence_rank(hint),
                block_area(region),
            ))
        })
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| {
        right
            .1
            .total_cmp(&left.1)
            .then_with(|| right.2.total_cmp(&left.2))
            .then_with(|| left.3.total_cmp(&right.3))
            .then_with(|| left.0.cmp(&right.0))
    });
    let winner = candidates.first()?;
    if !is_wrapper_hint(&hints[winner.0]) {
        return Some(winner.0);
    }

    Some(
        candidates
            .iter()
            .find(|candidate| {
                !is_wrapper_hint(&hints[candidate.0])
                    && roots[candidate.0] == Some(winner.0)
                    && candidate.1 >= MIN_SEMANTIC_CHILD_SEGMENT_COVERAGE
            })
            .map_or(winner.0, |candidate| candidate.0),
    )
}

fn contains_cjk(text: &str) -> bool {
    text.chars().any(|character| {
        matches!(
            character as u32,
            0x4E00..=0x9FFF
                | 0x3040..=0x309F
                | 0x30A0..=0x30FF
                | 0xAC00..=0xD7AF
                | 0x3400..=0x4DBF
                | 0xF900..=0xFAFF
                | 0x20000..=0x2A6DF
                | 0x2A700..=0x2B73F
                | 0x2B740..=0x2B81F
                | 0x2B820..=0x2CEAF
                | 0x2CEB0..=0x2EBEF
                | 0x30000..=0x3134F
                | 0x31350..=0x323AF
                | 0x2F800..=0x2FA1F
        )
    })
}

fn segments_form_atomic_fragment(
    previous: &crate::pdf::hierarchy::SegmentData,
    next: &crate::pdf::hierarchy::SegmentData,
) -> bool {
    let (Some(previous_last), Some(next_first)) = (previous.text.chars().last(), next.text.chars().next()) else {
        return false;
    };
    if previous_last.is_whitespace()
        || next_first.is_whitespace()
        || contains_cjk(&previous.text)
        || contains_cjk(&next.text)
        || segment_block(previous).is_none()
        || segment_block(next).is_none()
        || !previous.font_size.is_finite()
        || previous.font_size <= 0.0
        || previous.font_size != next.font_size
        || previous.is_bold != next.is_bold
        || previous.is_italic != next.is_italic
        || previous.is_monospace != next.is_monospace
        || previous.assigned_role != next.assigned_role
        || !previous.baseline_y.is_finite()
        || !next.baseline_y.is_finite()
        || next.x < previous.x
    {
        return false;
    }

    let effective_height = next.height.max(previous.height).max(next.font_size * 0.5);
    let same_baseline = (previous.baseline_y - next.baseline_y).abs() < effective_height * 0.5;
    let horizontal_gap = next.x - (previous.x + previous.width);
    let kerning_limit = next.font_size * ATOMIC_FRAGMENT_GAP_RATIO;
    same_baseline && (-kerning_limit..=kerning_limit).contains(&horizontal_gap)
}

fn reconcile_atomic_fragment_owners(segments: &[crate::pdf::hierarchy::SegmentData], owners: &mut [Option<usize>]) {
    debug_assert_eq!(segments.len(), owners.len());
    let mut run_start = 0;
    while run_start < segments.len() {
        let mut run_end = run_start + 1;
        while run_end < segments.len() && segments_form_atomic_fragment(&segments[run_end - 1], &segments[run_end]) {
            run_end += 1;
        }

        let mut run_owner = None;
        let has_conflicting_owner = owners[run_start..run_end]
            .iter()
            .flatten()
            .any(|owner| match run_owner {
                Some(current) => current != *owner,
                None => {
                    run_owner = Some(*owner);
                    false
                }
            });
        if !has_conflicting_owner && let Some(run_owner) = run_owner {
            owners[run_start..run_end].fill(Some(run_owner));
        }
        run_start = run_end;
    }
}

#[derive(Default)]
struct PictureTextLine {
    baseline: f32,
    left: f32,
    right: f32,
    alpha_chars: usize,
    visible_chars: usize,
    word_count: usize,
    owned_alpha_chars: usize,
    owned_visible_chars: usize,
    owned_word_count: usize,
    intervals: Vec<(f32, f32)>,
    owned_indices: Vec<usize>,
}

fn add_segment_to_picture_line(
    lines: &mut Vec<PictureTextLine>,
    index: usize,
    segment: &crate::pdf::hierarchy::SegmentData,
    is_owned: bool,
) {
    let tolerance = segment.font_size.max(segment.height) * FALSE_PICTURE_BASELINE_TOLERANCE_RATIO;
    let line_index = lines
        .iter()
        .position(|line| (line.baseline - segment.baseline_y).abs() <= tolerance)
        .unwrap_or_else(|| {
            lines.push(PictureTextLine {
                baseline: segment.baseline_y,
                left: f32::INFINITY,
                right: f32::NEG_INFINITY,
                ..PictureTextLine::default()
            });
            lines.len() - 1
        });
    let line = &mut lines[line_index];
    line.left = line.left.min(segment.x);
    line.right = line.right.max(segment.x + segment.width);
    line.alpha_chars += segment
        .text
        .chars()
        .filter(|character| character.is_alphabetic())
        .count();
    line.visible_chars += segment
        .text
        .chars()
        .filter(|character| !character.is_whitespace())
        .count();
    line.word_count += segment.text.split_whitespace().count();
    line.intervals.push((segment.x, segment.x + segment.width));
    if is_owned {
        line.owned_alpha_chars += segment
            .text
            .chars()
            .filter(|character| character.is_alphabetic())
            .count();
        line.owned_visible_chars += segment
            .text
            .chars()
            .filter(|character| !character.is_whitespace())
            .count();
        line.owned_word_count += segment.text.split_whitespace().count();
        line.owned_indices.push(index);
    }
}

fn picture_line_is_contiguous(line: &PictureTextLine, page_width_pts: f32) -> bool {
    let mut intervals = line.intervals.clone();
    intervals.sort_by(|left, right| left.0.total_cmp(&right.0));
    intervals
        .windows(2)
        .all(|pair| pair[1].0 - pair[0].1 <= page_width_pts * MAX_FALSE_PICTURE_LINE_GAP_NORM)
}

fn picture_line_is_body_prose(line: &PictureTextLine, picture: &OrderBlock, page_width_pts: f32) -> bool {
    let alpha_ratio = line.alpha_chars as f32 / line.visible_chars.max(1) as f32;
    let owned_alpha_ratio = line.owned_alpha_chars as f32 / line.owned_visible_chars.max(1) as f32;
    line.right - line.left >= page_width_pts * MIN_FALSE_PICTURE_LINE_WIDTH_NORM
        && line.left < picture.left - page_width_pts * MIN_FALSE_PICTURE_OUTSIDE_SPAN_NORM
        && line.word_count >= MIN_FALSE_PICTURE_WORDS_PER_LINE
        && line.owned_word_count >= MIN_FALSE_PICTURE_WORDS_PER_LINE
        && alpha_ratio >= MIN_FALSE_PICTURE_ALPHA_RATIO
        && owned_alpha_ratio >= MIN_FALSE_PICTURE_ALPHA_RATIO
        && picture_line_is_contiguous(line, page_width_pts)
}

fn picture_prose_owned_indices(
    owner: usize,
    segments: &[crate::pdf::hierarchy::SegmentData],
    owners: &[Option<usize>],
    picture: &OrderBlock,
    page_width_pts: f32,
) -> Vec<usize> {
    let owned_indices = owners
        .iter()
        .enumerate()
        .filter_map(|(index, candidate)| (*candidate == Some(owner)).then_some(index))
        .collect::<Vec<_>>();
    let (Some(first), Some(last)) = (owned_indices.first(), owned_indices.last()) else {
        return Vec::new();
    };
    let owner_span = &owners[*first..=*last];
    if !owner_span.iter().any(Option::is_none) || owner_span.iter().flatten().any(|candidate| *candidate != owner) {
        return Vec::new();
    }

    let mut lines = Vec::new();
    for index in *first..=*last {
        if owners[index].is_none() || owners[index] == Some(owner) {
            add_segment_to_picture_line(&mut lines, index, &segments[index], owners[index] == Some(owner));
        }
    }
    let qualifying_lines = lines
        .into_iter()
        .filter(|line| picture_line_is_body_prose(line, picture, page_width_pts))
        .collect::<Vec<_>>();
    let alpha_chars = qualifying_lines.iter().map(|line| line.alpha_chars).sum::<usize>();
    if qualifying_lines.len() < MIN_FALSE_PICTURE_PROSE_LINES || alpha_chars < MIN_FALSE_PICTURE_ALPHA_CHARS {
        return Vec::new();
    }
    qualifying_lines
        .into_iter()
        .flat_map(|line| line.owned_indices)
        .collect()
}

fn reconcile_false_picture_prose_owners(
    segments: &[crate::pdf::hierarchy::SegmentData],
    hints: &[LayoutHint],
    blocks: &[Option<OrderBlock>],
    owners: &mut [Option<usize>],
    page_width_pts: Option<f32>,
) {
    let Some(page_width_pts) = page_width_pts.filter(|width| width.is_finite() && *width > 0.0) else {
        return;
    };
    for (owner, hint) in hints.iter().enumerate() {
        if hint.class_name != LayoutHintClass::Picture {
            continue;
        }
        let Some(picture) = blocks[owner].as_ref() else {
            continue;
        };
        let prose_indices = picture_prose_owned_indices(owner, segments, owners, picture, page_width_pts);
        for index in prose_indices {
            owners[index] = None;
        }
    }
}

fn has_single_column_segment_geometry(
    segments: &[crate::pdf::hierarchy::SegmentData],
    page_width_pts: Option<f32>,
) -> bool {
    let Some(page_width_pts) = page_width_pts.filter(|width| width.is_finite() && *width > 0.0) else {
        return false;
    };
    let Some(segment_blocks) = segments.iter().map(segment_block).collect::<Option<Vec<_>>>() else {
        return false;
    };
    let left_min = segment_blocks
        .iter()
        .map(|block| block.left)
        .fold(f32::INFINITY, f32::min);
    let left_max = segment_blocks
        .iter()
        .map(|block| block.left)
        .fold(f32::NEG_INFINITY, f32::max);
    if left_max - left_min > MAX_SINGLE_COLUMN_LEFT_SPREAD_NORM * page_width_pts {
        return false;
    }

    let common_left = segment_blocks
        .iter()
        .map(|block| block.left)
        .fold(f32::NEG_INFINITY, f32::max);
    let common_right = segment_blocks
        .iter()
        .map(|block| block.right)
        .fold(f32::INFINITY, f32::min);
    let narrowest_width = segment_blocks
        .iter()
        .map(|block| block.right - block.left)
        .fold(f32::INFINITY, f32::min);
    let common_width_ratio = (common_right - common_left).max(0.0) / narrowest_width;

    common_width_ratio.is_finite() && common_width_ratio >= MIN_SINGLE_COLUMN_COMMON_WIDTH_RATIO
}

fn partial_ownership_is_generic_text_flow(
    owners: &[Option<usize>],
    hints: &[LayoutHint],
    eligible: &[bool],
    roots: &[Option<usize>],
) -> bool {
    if hints
        .iter()
        .enumerate()
        .any(|(index, hint)| eligible[index] && hint.class_name != LayoutHintClass::Text)
    {
        return false;
    }
    let owned_count = owners.iter().filter(|owner| owner.is_some()).count();
    let uncovered_count = owners.len() - owned_count;
    if owned_count == 0 || uncovered_count == 0 || owned_count > uncovered_count {
        return false;
    }

    let owner_indices = owners
        .iter()
        .flatten()
        .copied()
        .collect::<std::collections::BTreeSet<_>>();
    owner_indices.len() >= MIN_PARTIAL_TEXT_OWNERS
        && owner_indices
            .iter()
            .all(|owner| roots.get(*owner).is_some_and(Option::is_some))
}

fn should_preserve_native_partial_text_flow(
    segments: &[crate::pdf::hierarchy::SegmentData],
    owners: &[Option<usize>],
    hints: &[LayoutHint],
    eligible: &[bool],
    roots: &[Option<usize>],
    no_reorder: bool,
    page_width_pts: Option<f32>,
) -> bool {
    !no_reorder
        && partial_ownership_is_generic_text_flow(owners, hints, eligible, roots)
        && has_single_column_segment_geometry(segments, page_width_pts)
}

fn pathless_group(segment_count: usize) -> Vec<LayoutSegmentGroup> {
    vec![LayoutSegmentGroup {
        segment_indices: (0..segment_count).collect(),
        hint_indices: Vec::new(),
        region_path: None,
    }]
}

#[cfg(feature = "layout-detection")]
pub(crate) fn has_eligible_layout_hints(hints: &[LayoutHint], wrapper_ownership: &[bool]) -> bool {
    eligible_hints(hints, wrapper_ownership)
        .into_iter()
        .any(|eligible| eligible)
}

fn uncovered_group(
    indices: Vec<usize>,
    segments: &[crate::pdf::hierarchy::SegmentData],
    synthetic_id: usize,
) -> PlannedGroup {
    let first_segment_index = indices[0];
    let order_block = segments_union_block(&indices, segments);
    PlannedGroup {
        output: LayoutSegmentGroup {
            segment_indices: indices,
            hint_indices: Vec::new(),
            region_path: Some(LayoutRegionPath {
                root: LayoutRegionTag {
                    id: synthetic_id,
                    class_name: None,
                },
                child: None,
            }),
        },
        root_id: synthetic_id,
        order_block,
        first_segment_index,
    }
}

fn ordered_indices(
    blocks: &[Option<OrderBlock>],
    first_indices: &[usize],
    no_reorder: bool,
    page_width_pts: Option<f32>,
) -> Vec<usize> {
    if no_reorder {
        let mut order = (0..blocks.len()).collect::<Vec<_>>();
        order.sort_by_key(|index| first_indices[*index]);
        return order;
    }

    let valid = blocks
        .iter()
        .enumerate()
        .filter_map(|(index, block)| block.as_ref().map(|_| index))
        .collect::<Vec<_>>();
    let valid_blocks = valid
        .iter()
        .map(|index| blocks[*index].expect("validated block"))
        .collect::<Vec<_>>();
    let valid_order = if is_multi_column(&valid_blocks) {
        order_blocks_by_graph(&valid_blocks, page_width_pts)
    } else {
        let mut order = (0..valid_blocks.len()).collect::<Vec<_>>();
        order.sort_by(|left, right| {
            valid_blocks[*right]
                .top
                .total_cmp(&valid_blocks[*left].top)
                .then_with(|| valid_blocks[*left].left.total_cmp(&valid_blocks[*right].left))
        });
        order
    };
    let mut result = valid_order.into_iter().map(|index| valid[index]).collect::<Vec<_>>();
    let mut invalid = (0..blocks.len())
        .filter(|index| blocks[*index].is_none())
        .collect::<Vec<_>>();
    invalid.sort_by_key(|index| first_indices[*index]);
    result.extend(invalid);
    result
}

fn order_planned_groups(
    groups: Vec<PlannedGroup>,
    root_blocks: &[Option<OrderBlock>],
    no_reorder: bool,
    page_width_pts: Option<f32>,
) -> Vec<LayoutSegmentGroup> {
    let mut by_root = std::collections::BTreeMap::<usize, Vec<PlannedGroup>>::new();
    for group in groups {
        by_root.entry(group.root_id).or_default().push(group);
    }

    let root_ids = by_root.keys().copied().collect::<Vec<_>>();
    let root_order_blocks = root_ids
        .iter()
        .map(|root_id| root_blocks.get(*root_id).copied().flatten())
        .collect::<Vec<_>>();
    let root_first_indices = root_ids
        .iter()
        .map(|root_id| {
            by_root[root_id]
                .iter()
                .map(|group| group.first_segment_index)
                .min()
                .expect("non-empty root")
        })
        .collect::<Vec<_>>();

    let mut ordered = Vec::new();
    for root_position in ordered_indices(&root_order_blocks, &root_first_indices, no_reorder, page_width_pts) {
        let root_id = root_ids[root_position];
        let mut children = by_root.remove(&root_id).expect("known root");
        let child_blocks = children.iter().map(|group| group.order_block).collect::<Vec<_>>();
        let child_first = children
            .iter()
            .map(|group| group.first_segment_index)
            .collect::<Vec<_>>();
        let child_order = ordered_indices(&child_blocks, &child_first, no_reorder, page_width_pts);
        let mut slots = children.drain(..).map(Some).collect::<Vec<_>>();
        ordered.extend(
            child_order
                .into_iter()
                .filter_map(|index| slots[index].take())
                .map(|group| group.output),
        );
    }
    ordered
}

/// Build a deterministic reading-order plan without flattening layout regions.
///
/// Every post-table-filter segment appears exactly once. Table/Picture regions
/// stay as top-level wrappers, regular regions contained by a wrapper are folded
/// into that root, and segments outside regions remain in contiguous source runs.
#[cfg(feature = "layout-detection")]
pub(crate) fn plan_segment_groups_by_layout(
    segments: &[crate::pdf::hierarchy::SegmentData],
    hints: &[LayoutHint],
    wrapper_ownership: &[bool],
    no_reorder: bool,
    page_width_pts: Option<f32>,
) -> Vec<LayoutSegmentGroup> {
    if segments.is_empty() {
        return Vec::new();
    }
    if hints.is_empty() {
        return pathless_group(segments.len());
    }

    let blocks = hints.iter().map(hint_block).collect::<Vec<_>>();
    let eligible = eligible_hints(hints, wrapper_ownership);
    if !eligible.iter().any(|value| *value) {
        return pathless_group(segments.len());
    }
    let roots = root_hint_indices(hints, &eligible, &blocks);
    let mut owners = segments
        .iter()
        .map(|segment| choose_segment_owner(segment, hints, &eligible, &blocks, &roots))
        .collect::<Vec<_>>();
    reconcile_atomic_fragment_owners(segments, &mut owners);
    reconcile_false_picture_prose_owners(segments, hints, &blocks, &mut owners, page_width_pts);
    if owners.iter().all(Option::is_none) {
        return pathless_group(segments.len());
    }
    if should_preserve_native_partial_text_flow(segments, &owners, hints, &eligible, &roots, no_reorder, page_width_pts)
    {
        return pathless_group(segments.len());
    }

    let mut region_segments = std::collections::BTreeMap::<usize, Vec<usize>>::new();
    for (segment_index, owner) in owners.iter().enumerate() {
        if let Some(owner) = owner
            && roots[*owner].is_some()
        {
            region_segments.entry(*owner).or_default().push(segment_index);
        }
    }

    let mut groups = region_segments
        .into_iter()
        .map(|(owner, mut segment_indices)| {
            if !no_reorder {
                segment_indices.sort_by(|left, right| {
                    let left_segment = &segments[*left];
                    let right_segment = &segments[*right];
                    let left_top = left_segment.y + left_segment.height;
                    let right_top = right_segment.y + right_segment.height;
                    right_top
                        .total_cmp(&left_top)
                        .then_with(|| left_segment.x.total_cmp(&right_segment.x))
                        .then_with(|| left.cmp(right))
                });
            }
            let first_segment_index = *segment_indices.iter().min().expect("non-empty region group");
            let order_block = if is_wrapper_hint(&hints[owner]) {
                segments_union_block(&segment_indices, segments)
            } else {
                blocks[owner]
            };
            PlannedGroup {
                first_segment_index,
                output: LayoutSegmentGroup {
                    segment_indices,
                    hint_indices: (!is_wrapper_hint(&hints[owner])).then_some(owner).into_iter().collect(),
                    region_path: roots[owner].map(|root| LayoutRegionPath {
                        root: LayoutRegionTag {
                            id: root,
                            class_name: Some(hints[root].class_name),
                        },
                        child: (root != owner).then_some(LayoutRegionTag {
                            id: owner,
                            class_name: Some(hints[owner].class_name),
                        }),
                    }),
                },
                root_id: roots[owner].expect("eligible owner has a root"),
                order_block,
            }
        })
        .collect::<Vec<_>>();

    let mut uncovered = Vec::new();
    let mut next_synthetic_id = hints.len();
    for (segment_index, owner) in owners.iter().enumerate() {
        if owner.is_none() {
            uncovered.push(segment_index);
        } else if !uncovered.is_empty() {
            groups.push(uncovered_group(
                std::mem::take(&mut uncovered),
                segments,
                next_synthetic_id,
            ));
            next_synthetic_id += 1;
        }
    }
    if !uncovered.is_empty() {
        groups.push(uncovered_group(uncovered, segments, next_synthetic_id));
    }

    let mut root_blocks = blocks;
    root_blocks.resize(next_synthetic_id + 1, None);
    for group in &groups {
        if group.root_id >= hints.len() {
            root_blocks[group.root_id] = group.order_block;
        }
    }
    order_planned_groups(groups, &root_blocks, no_reorder, page_width_pts)
}

/// Compatibility helper used by the legacy reading-order unit tests.
#[cfg(all(feature = "layout-detection", test))]
pub(crate) fn reorder_segments_by_layout(
    segments: Vec<crate::pdf::hierarchy::SegmentData>,
    hints: &[LayoutHint],
    page_width_pts: Option<f32>,
) -> Vec<crate::pdf::hierarchy::SegmentData> {
    let no_reorder = crate::pdf::structure::layout_debug::layout_debug_flags().no_reorder;
    plan_segment_groups_by_layout(&segments, hints, &[], no_reorder, page_width_pts)
        .into_iter()
        .flat_map(|group| group.segment_indices)
        .map(|index| segments[index].clone())
        .collect()
}

/// Reorder spans using purely geometric column detection (no layout hints needed).
///
/// Detects columns by clustering span x-centers, then orders spans
/// left-to-right across columns, and top-to-bottom within each column.
///
/// Returns a Vec of span indices in reading order.
fn reorder_spans_geometric(spans: &[TextSpan]) -> Vec<usize> {
    if spans.is_empty() {
        return Vec::new();
    }

    let mut x_centers: Vec<f32> = spans.iter().map(|s| s.x + s.width / 2.0).collect();
    x_centers.sort_by(|a, b| a.total_cmp(b));

    let mut unique_centers: Vec<f32> = Vec::new();
    for &center in &x_centers {
        if let Some(&last) = unique_centers.last() {
            if (center - last).abs() > COLUMN_MERGE_THRESHOLD_PTS {
                unique_centers.push(center);
            }
        } else {
            unique_centers.push(center);
        }
    }

    let mut span_columns: Vec<(usize, f32, usize)> = Vec::new();
    for (span_idx, span) in spans.iter().enumerate() {
        let span_center = span.x + span.width / 2.0;
        let mut best_col = 0;
        let mut best_dist = f32::INFINITY;

        for (col_id, &cluster_center) in unique_centers.iter().enumerate() {
            let dist = (span_center - cluster_center).abs();
            if dist < best_dist {
                best_dist = dist;
                best_col = col_id;
            }
        }

        let top_y = span.y + span.height;
        span_columns.push((best_col, top_y, span_idx));
    }

    span_columns.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| b.1.total_cmp(&a.1)));

    span_columns.into_iter().map(|(_, _, idx)| idx).collect()
}

/// Reorder spans based on layout regions and column detection.
///
/// Given a set of spans with bounding boxes and layout-detected regions:
/// 1. Project spans onto regions
/// 2. Detect columns from region x-centers
/// 3. Sort regions by (column_id, top-to-bottom within column)
/// 4. Emit spans in the order of their sorted regions
///
/// When layout hints are unavailable, falls back to geometric column detection.
///
/// Returns a Vec of span indices in reading order.
pub(crate) fn reorder_spans_by_layout(spans: &[TextSpan], hints: &[LayoutHint]) -> Vec<usize> {
    if spans.is_empty() {
        return Vec::new();
    }

    if hints.is_empty() {
        return reorder_spans_geometric(spans);
    }

    let regions = project_spans_to_regions(spans, hints);
    if regions.is_empty() {
        return (0..spans.len()).collect();
    }

    let column_assignments = detect_columns(&regions);

    let mut sorted_regions: Vec<(usize, f32, usize)> = regions
        .iter()
        .enumerate()
        .map(|(region_idx, region)| {
            let col_id = column_assignments[region_idx];
            let top_y = region.top;
            (col_id, top_y, region_idx)
        })
        .collect();

    sorted_regions.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| b.1.total_cmp(&a.1)));

    let mut result = Vec::new();
    let mut projected_spans = std::collections::HashSet::new();

    for (_, _, region_idx) in sorted_regions {
        let mut sorted_span_indices: Vec<usize> = regions[region_idx].span_indices.clone();
        sorted_span_indices.sort_by(|&a, &b| {
            let span_a = &spans[a];
            let span_b = &spans[b];
            let top_a = span_a.y + span_a.height;
            let top_b = span_b.y + span_b.height;
            top_b.total_cmp(&top_a).then_with(|| span_a.x.total_cmp(&span_b.x))
        });

        for &span_idx in &sorted_span_indices {
            result.push(span_idx);
            projected_spans.insert(span_idx);
        }
    }

    for span_idx in 0..spans.len() {
        if !projected_spans.contains(&span_idx) {
            result.push(span_idx);
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn planned_segment(text: &str, x: f32, y: f32, width: f32, height: f32) -> crate::pdf::hierarchy::SegmentData {
        crate::pdf::hierarchy::SegmentData {
            text: text.to_string(),
            x,
            y,
            width,
            height,
            font_size: 10.0,
            is_bold: false,
            is_italic: false,
            is_monospace: false,
            baseline_y: y,
            assigned_role: None,
        }
    }

    fn planned_hint(
        class_name: crate::pdf::structure::types::LayoutHintClass,
        left: f32,
        bottom: f32,
        right: f32,
        top: f32,
    ) -> LayoutHint {
        LayoutHint {
            class_name,
            confidence: 0.9,
            left,
            bottom,
            right,
            top,
        }
    }

    fn assert_fragments_remain_separate(
        segments: Vec<crate::pdf::hierarchy::SegmentData>,
        classes: [LayoutHintClass; 2],
    ) {
        let hints = [
            planned_hint(
                classes[0],
                segments[0].x,
                segments[0].y,
                segments[0].x + segments[0].width,
                segments[0].y + segments[0].height,
            ),
            planned_hint(
                classes[1],
                segments[1].x,
                segments[1].y,
                segments[1].x + segments[1].width,
                segments[1].y + segments[1].height,
            ),
        ];
        let groups = plan_segment_groups_by_layout(&segments, &hints, &[], true, None);
        let flattened = groups
            .iter()
            .flat_map(|group| group.segment_indices.iter().copied())
            .collect::<Vec<_>>();
        assert_eq!(groups.len(), 2);
        assert_eq!(flattened, [0, 1]);
    }

    #[cfg(feature = "layout-detection")]
    fn order_block(left: f32, bottom: f32, right: f32, top: f32) -> OrderBlock {
        OrderBlock {
            left,
            bottom,
            right,
            top,
        }
    }

    #[cfg(feature = "layout-detection")]
    #[test]
    fn horizontal_dilation_uses_page_width_threshold() {
        let target = order_block(100.0, 100.0, 200.0, 120.0);
        let accepted_predecessor = order_block(-50.0, 200.0, 200.0, 220.0);
        let accepted_blocks = vec![target, accepted_predecessor];
        let accepted = dilate_horizontally(
            &accepted_blocks,
            &[vec![1], Vec::new()],
            &[Vec::new(), Vec::new()],
            1_000.0,
        );
        assert_eq!(accepted[0].left, -50.0, "widening exactly 15% must be accepted");

        let rejected_predecessor = order_block(-50.1, 200.0, 200.0, 220.0);
        let rejected_blocks = vec![target, rejected_predecessor];
        let rejected = dilate_horizontally(
            &rejected_blocks,
            &[vec![1], Vec::new()],
            &[Vec::new(), Vec::new()],
            1_000.0,
        );
        assert_eq!(
            rejected[0], target,
            "widening greater than 15% must leave the block unchanged"
        );
    }

    #[cfg(feature = "layout-detection")]
    #[test]
    fn horizontal_dilation_rolls_back_predecessor_when_successor_exceeds_threshold() {
        let target = order_block(100.0, 100.0, 200.0, 120.0);
        let predecessor = order_block(0.0, 200.0, 200.0, 220.0);
        let successor = order_block(100.0, 0.0, 400.1, 20.0);
        let blocks = vec![target, predecessor, successor];

        let dilated = dilate_horizontally(
            &blocks,
            &[vec![1], Vec::new(), Vec::new()],
            &[vec![2], Vec::new(), Vec::new()],
            1_000.0,
        );

        assert_eq!(
            dilated[0], target,
            "a rejected successor expansion must discard the accepted predecessor expansion"
        );
    }

    #[cfg(feature = "layout-detection")]
    #[test]
    fn horizontal_dilation_preserves_raw_blocks() {
        let blocks = vec![
            order_block(100.0, 100.0, 200.0, 120.0),
            order_block(0.0, 200.0, 200.0, 220.0),
        ];
        let original = blocks.clone();

        let dilated = dilate_horizontally(&blocks, &[vec![1], Vec::new()], &[Vec::new(), Vec::new()], 1_000.0);

        assert_eq!(blocks, original, "dilation must not mutate the raw geometry");
        assert_ne!(dilated[0], blocks[0], "the copied geometry should be widened");
    }

    #[cfg(feature = "layout-detection")]
    #[test]
    fn horizontal_dilation_uses_only_first_neighbors() {
        let blocks = vec![
            order_block(40.0, 40.0, 50.0, 50.0),
            order_block(35.0, 60.0, 50.0, 70.0),
            order_block(20.0, 80.0, 50.0, 90.0),
            order_block(40.0, 20.0, 55.0, 30.0),
            order_block(40.0, 0.0, 70.0, 10.0),
        ];
        let mut up = vec![Vec::new(); blocks.len()];
        let mut down = vec![Vec::new(); blocks.len()];
        up[0] = vec![1, 2];
        down[0] = vec![3, 4];

        let dilated = dilate_horizontally(&blocks, &up, &down, 100.0);

        assert_eq!(dilated[0].left, 35.0);
        assert_eq!(dilated[0].right, 55.0);
    }

    #[cfg(feature = "layout-detection")]
    #[test]
    fn invalid_page_width_preserves_legacy_graph_order() {
        let blocks = vec![
            order_block(0.0, 200.0, 100.0, 220.0),
            order_block(0.0, 100.0, 100.0, 120.0),
            order_block(200.0, 200.0, 300.0, 220.0),
            order_block(200.0, 100.0, 300.0, 120.0),
        ];
        let legacy = order_blocks_by_graph(&blocks, None);

        for invalid_width in [f32::NAN, f32::INFINITY, 0.0, -1.0] {
            assert_eq!(
                order_blocks_by_graph(&blocks, Some(invalid_width)),
                legacy,
                "invalid page width {invalid_width:?} must preserve the legacy graph"
            );
        }
    }

    #[cfg(feature = "layout-detection")]
    #[test]
    fn graph_relations_are_rebuilt_from_dilated_blocks() {
        let blocks = vec![
            order_block(0.0, 200.0, 120.0, 220.0),
            order_block(80.0, 300.0, 160.0, 320.0),
            order_block(120.0, 300.0, 240.0, 320.0),
            order_block(160.0, 200.0, 280.0, 220.0),
        ];

        assert_eq!(order_blocks_by_graph(&blocks, None), [1, 0, 2, 3]);
        assert_eq!(
            order_blocks_by_graph(&blocks, Some(400.0)),
            [1, 2, 0, 3],
            "dilated geometry must replace the raw predecessor maps"
        );
    }

    #[cfg(feature = "layout-detection")]
    #[test]
    fn segment_plan_uses_pdf_page_width_for_dilation() {
        use crate::pdf::structure::types::LayoutHintClass;

        let segments = vec![
            planned_segment("bottom-left", 10.0, 205.0, 10.0, 10.0),
            planned_segment("top-left", 90.0, 305.0, 10.0, 10.0),
            planned_segment("top-right", 200.0, 305.0, 10.0, 10.0),
            planned_segment("bottom-right", 250.0, 205.0, 10.0, 10.0),
        ];
        let hints = vec![
            planned_hint(LayoutHintClass::Text, 0.0, 200.0, 120.0, 220.0),
            planned_hint(LayoutHintClass::Text, 80.0, 300.0, 160.0, 320.0),
            planned_hint(LayoutHintClass::Text, 120.0, 300.0, 240.0, 320.0),
            planned_hint(LayoutHintClass::Text, 160.0, 200.0, 280.0, 220.0),
        ];
        let flattened = |page_width_pts| {
            plan_segment_groups_by_layout(&segments, &hints, &[], false, page_width_pts)
                .into_iter()
                .flat_map(|group| group.segment_indices)
                .collect::<Vec<_>>()
        };

        assert_eq!(flattened(None), [1, 0, 2, 3]);
        assert_eq!(
            flattened(Some(400.0)),
            [1, 2, 0, 3],
            "the page width must reach the graph refinement"
        );
    }

    #[test]
    fn plan_preserves_wrapper_and_child_paths() {
        use crate::pdf::structure::types::LayoutHintClass;

        let segments = vec![
            planned_segment("child", 20.0, 70.0, 20.0, 10.0),
            planned_segment("residual", 70.0, 20.0, 20.0, 10.0),
        ];
        let hints = vec![
            planned_hint(LayoutHintClass::Form, 0.0, 0.0, 100.0, 100.0),
            planned_hint(LayoutHintClass::Text, 10.0, 60.0, 50.0, 90.0),
        ];

        let groups = plan_segment_groups_by_layout(&segments, &hints, &[], false, None);
        assert_eq!(groups.len(), 2);
        let child = groups.iter().find(|group| group.hint_indices == [1]).unwrap();
        assert_eq!(child.segment_indices, [0]);
        assert_eq!(child.region_path.unwrap().root.id, 0);
        assert_eq!(child.region_path.unwrap().child.unwrap().id, 1);
        let residual = groups.iter().find(|group| group.segment_indices == [1]).unwrap();
        assert_eq!(residual.region_path.unwrap().root.id, 0);
        assert!(residual.region_path.unwrap().child.is_none());
    }

    #[test]
    fn segment_owner_keeps_stronger_wrapper_coverage() {
        use crate::pdf::structure::types::LayoutHintClass;

        let segments = vec![planned_segment("mostly wrapper", 0.0, 0.0, 100.0, 100.0)];
        let hints = vec![
            planned_hint(LayoutHintClass::Form, 0.0, 0.0, 100.0, 100.0),
            planned_hint(LayoutHintClass::Caption, 0.0, 0.0, 21.0, 100.0),
        ];

        let groups = plan_segment_groups_by_layout(&segments, &hints, &[], true, None);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].segment_indices, [0]);
        assert!(groups[0].hint_indices.is_empty());
        let path = groups[0].region_path.unwrap();
        assert_eq!(path.root.id, 0);
        assert!(path.child.is_none());
    }

    #[test]
    fn partial_minority_single_column_text_ownership_preserves_native_flow() {
        use crate::pdf::structure::types::LayoutHintClass;

        let segments = vec![
            planned_segment("May 5, 2023", 10.0, 100.0, 40.0, 10.0),
            planned_segment("To Whom it May Concern:", 10.0, 80.0, 90.0, 10.0),
            planned_segment("There were deliveries.", 10.0, 60.0, 100.0, 10.0),
            planned_segment("A total of 3 trucks were used.", 10.0, 50.0, 120.0, 10.0),
            planned_segment("Best Regards,", 10.0, 30.0, 50.0, 10.0),
            planned_segment("Mallori", 10.0, 10.0, 30.0, 10.0),
        ];
        let hints = vec![
            planned_hint(LayoutHintClass::Text, 0.0, 95.0, 120.0, 115.0),
            planned_hint(LayoutHintClass::Text, 0.0, 75.0, 120.0, 95.0),
        ];

        let groups = plan_segment_groups_by_layout(&segments, &hints, &[], false, Some(300.0));

        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].segment_indices, [0, 1, 2, 3, 4, 5]);
        assert!(groups[0].hint_indices.is_empty());
        assert!(groups[0].region_path.is_none());
    }

    #[test]
    fn single_column_geometry_accepts_left_spread_threshold() {
        let segments = vec![
            planned_segment("wide", 0.0, 20.0, 20.0, 10.0),
            planned_segment("indented", 10.0, 0.0, 10.0, 10.0),
        ];

        assert!(has_single_column_segment_geometry(&segments, Some(200.0)));
        assert!(!has_single_column_segment_geometry(
            &[segments[0].clone(), planned_segment("too far", 10.1, 0.0, 10.0, 10.0),],
            Some(200.0),
        ));
    }

    #[test]
    fn single_column_geometry_accepts_common_width_threshold() {
        let at_threshold = vec![
            planned_segment("left", 0.0, 20.0, 20.0, 10.0),
            planned_segment("right", 10.0, 0.0, 20.0, 10.0),
        ];
        let below_threshold = vec![
            at_threshold[0].clone(),
            planned_segment("weak overlap", 10.1, 0.0, 20.0, 10.0),
        ];

        assert!(has_single_column_segment_geometry(&at_threshold, Some(400.0)));
        assert!(!has_single_column_segment_geometry(&below_threshold, Some(400.0),));
    }

    #[test]
    fn partial_text_ownership_preserves_multi_column_groups() {
        use crate::pdf::structure::types::LayoutHintClass;

        let segments = vec![
            planned_segment("left", 10.0, 100.0, 40.0, 10.0),
            planned_segment("right", 210.0, 100.0, 40.0, 10.0),
            planned_segment("uncovered left", 10.0, 80.0, 80.0, 10.0),
            planned_segment("uncovered right", 210.0, 80.0, 80.0, 10.0),
        ];
        let hints = vec![
            planned_hint(LayoutHintClass::Text, 0.0, 95.0, 100.0, 115.0),
            planned_hint(LayoutHintClass::Text, 200.0, 95.0, 300.0, 115.0),
        ];

        let groups = plan_segment_groups_by_layout(&segments, &hints, &[], false, Some(300.0));

        assert!(groups.len() > 1);
        assert!(groups.iter().any(|group| group.region_path.is_some()));
    }

    #[test]
    fn owned_left_and_uncovered_right_preserve_layout_groups() {
        use crate::pdf::structure::types::LayoutHintClass;

        let segments = vec![
            planned_segment("owned left one", 10.0, 100.0, 80.0, 10.0),
            planned_segment("owned left two", 10.0, 80.0, 80.0, 10.0),
            planned_segment("uncovered right one", 180.0, 100.0, 100.0, 10.0),
            planned_segment("uncovered right two", 180.0, 80.0, 100.0, 10.0),
        ];
        let hints = vec![
            planned_hint(LayoutHintClass::Text, 0.0, 95.0, 100.0, 115.0),
            planned_hint(LayoutHintClass::Text, 0.0, 75.0, 100.0, 95.0),
        ];

        let groups = plan_segment_groups_by_layout(&segments, &hints, &[], false, Some(300.0));

        assert!(groups.len() > 1);
        assert!(groups.iter().any(|group| !group.hint_indices.is_empty()));
    }

    #[test]
    fn staggered_columns_preserve_layout_groups() {
        use crate::pdf::structure::types::LayoutHintClass;

        let segments = vec![
            planned_segment("owned left one", 10.0, 120.0, 80.0, 10.0),
            planned_segment("owned left two", 10.0, 100.0, 80.0, 10.0),
            planned_segment("uncovered right one", 180.0, 60.0, 100.0, 10.0),
            planned_segment("uncovered right two", 180.0, 40.0, 100.0, 10.0),
        ];
        let hints = vec![
            planned_hint(LayoutHintClass::Text, 0.0, 115.0, 100.0, 135.0),
            planned_hint(LayoutHintClass::Text, 0.0, 95.0, 100.0, 115.0),
        ];

        let groups = plan_segment_groups_by_layout(&segments, &hints, &[], false, Some(300.0));

        assert!(groups.len() > 1);
        assert!(groups.iter().any(|group| !group.hint_indices.is_empty()));
    }

    #[test]
    fn weak_horizontal_overlap_preserves_layout_groups() {
        use crate::pdf::structure::types::LayoutHintClass;

        let segments = vec![
            planned_segment("owned left one", 10.0, 100.0, 120.0, 10.0),
            planned_segment("owned left two", 10.0, 80.0, 120.0, 10.0),
            planned_segment("uncovered right one", 120.0, 100.0, 120.0, 10.0),
            planned_segment("uncovered right two", 120.0, 80.0, 120.0, 10.0),
        ];
        let hints = vec![
            planned_hint(LayoutHintClass::Text, 0.0, 95.0, 140.0, 115.0),
            planned_hint(LayoutHintClass::Text, 0.0, 75.0, 140.0, 95.0),
        ];

        let groups = plan_segment_groups_by_layout(&segments, &hints, &[], false, Some(300.0));

        assert!(groups.len() > 1);
        assert!(groups.iter().any(|group| !group.hint_indices.is_empty()));
    }

    #[test]
    fn majority_text_ownership_preserves_layout_groups() {
        use crate::pdf::structure::types::LayoutHintClass;

        let segments = vec![
            planned_segment("one", 10.0, 100.0, 40.0, 10.0),
            planned_segment("two", 10.0, 80.0, 40.0, 10.0),
            planned_segment("three", 10.0, 60.0, 40.0, 10.0),
            planned_segment("uncovered", 10.0, 40.0, 40.0, 10.0),
        ];
        let hints = vec![
            planned_hint(LayoutHintClass::Text, 0.0, 95.0, 100.0, 115.0),
            planned_hint(LayoutHintClass::Text, 0.0, 75.0, 100.0, 95.0),
            planned_hint(LayoutHintClass::Text, 0.0, 55.0, 100.0, 75.0),
        ];

        let groups = plan_segment_groups_by_layout(&segments, &hints, &[], false, Some(300.0));

        assert!(groups.len() > 1);
        assert!(groups.iter().any(|group| !group.hint_indices.is_empty()));
    }

    #[test]
    fn partial_text_ownership_has_no_segment_count_cliff() {
        use crate::pdf::structure::types::LayoutHintClass;

        let segments = (0..12)
            .map(|index| planned_segment("line", 10.0, 200.0 - index as f32 * 20.0, 40.0, 10.0))
            .collect::<Vec<_>>();
        let hints = vec![
            planned_hint(LayoutHintClass::Text, 0.0, 195.0, 100.0, 215.0),
            planned_hint(LayoutHintClass::Text, 0.0, 175.0, 100.0, 195.0),
        ];

        let groups = plan_segment_groups_by_layout(&segments, &hints, &[], false, Some(300.0));

        assert_eq!(groups.len(), 1);
        assert!(groups[0].region_path.is_none());
    }

    #[test]
    fn semantic_owner_preserves_partial_layout_groups() {
        use crate::pdf::structure::types::LayoutHintClass;

        let segments = vec![
            planned_segment("body", 10.0, 100.0, 40.0, 10.0),
            planned_segment("Section", 10.0, 80.0, 40.0, 10.0),
            planned_segment("uncovered one", 10.0, 60.0, 70.0, 10.0),
            planned_segment("uncovered two", 10.0, 40.0, 70.0, 10.0),
        ];
        let hints = vec![
            planned_hint(LayoutHintClass::Text, 0.0, 95.0, 100.0, 115.0),
            planned_hint(LayoutHintClass::SectionHeader, 0.0, 75.0, 100.0, 95.0),
        ];

        let groups = plan_segment_groups_by_layout(&segments, &hints, &[], false, Some(300.0));

        assert!(groups.len() > 1);
        assert!(groups.iter().any(|group| group.hint_indices == [1]));
    }

    #[test]
    fn unowned_semantic_hint_preserves_partial_layout_groups() {
        use crate::pdf::structure::types::LayoutHintClass;

        let segments = vec![
            planned_segment("owned one", 10.0, 100.0, 40.0, 10.0),
            planned_segment("owned two", 10.0, 80.0, 40.0, 10.0),
            planned_segment("uncovered one", 10.0, 60.0, 70.0, 10.0),
            planned_segment("uncovered two", 10.0, 40.0, 70.0, 10.0),
        ];
        let hints = vec![
            planned_hint(LayoutHintClass::Text, 0.0, 95.0, 100.0, 115.0),
            planned_hint(LayoutHintClass::Text, 0.0, 75.0, 100.0, 95.0),
            planned_hint(LayoutHintClass::SectionHeader, 200.0, 200.0, 280.0, 220.0),
        ];

        let groups = plan_segment_groups_by_layout(&segments, &hints, &[], false, Some(300.0));

        assert!(groups.len() > 1);
        assert!(groups.iter().any(|group| !group.hint_indices.is_empty()));
    }

    #[test]
    fn wrapper_root_preserves_partial_layout_groups() {
        use crate::pdf::structure::types::LayoutHintClass;

        let segments = vec![
            planned_segment("child one", 10.0, 100.0, 40.0, 10.0),
            planned_segment("child two", 10.0, 80.0, 40.0, 10.0),
            planned_segment("uncovered one", 200.0, 60.0, 70.0, 10.0),
            planned_segment("uncovered two", 200.0, 40.0, 70.0, 10.0),
        ];
        let hints = vec![
            planned_hint(LayoutHintClass::Form, 0.0, 70.0, 100.0, 120.0),
            planned_hint(LayoutHintClass::Text, 0.0, 95.0, 100.0, 115.0),
            planned_hint(LayoutHintClass::Text, 0.0, 75.0, 100.0, 95.0),
        ];

        let groups = plan_segment_groups_by_layout(&segments, &hints, &[], false, Some(300.0));

        assert!(groups.len() > 1);
        assert!(groups.iter().any(|group| {
            group
                .region_path
                .is_some_and(|path| path.root.class_name == Some(LayoutHintClass::Form))
        }));
    }

    #[test]
    fn prose_like_picture_owner_is_demoted_into_native_flow() {
        use crate::pdf::structure::types::LayoutHintClass;

        let segments = vec![
            planned_segment(
                "The first page-wide sentence is ordinary body prose.",
                10.0,
                100.0,
                280.0,
                10.0,
            ),
            planned_segment(
                "The second page-wide sentence continues the discussion.",
                10.0,
                88.0,
                280.0,
                10.0,
            ),
            planned_segment("A left fragment of the third sentence ", 10.0, 76.0, 80.0, 10.0),
            planned_segment("continues across the false picture boundary.", 95.0, 76.0, 195.0, 10.0),
            planned_segment(
                "The fourth page-wide sentence completes the paragraph.",
                10.0,
                64.0,
                280.0,
                10.0,
            ),
        ];
        let hints = vec![planned_hint(LayoutHintClass::Picture, 100.0, 60.0, 300.0, 115.0)];

        let groups = plan_segment_groups_by_layout(&segments, &hints, &[], false, Some(320.0));

        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].segment_indices, vec![0, 1, 2, 3, 4]);
        assert_eq!(groups[0].region_path, None);
    }

    #[test]
    fn mixed_picture_demotes_only_prose_and_retains_chart_labels() {
        use crate::pdf::structure::types::LayoutHintClass;

        let segments = vec![
            planned_segment(
                "The first page-wide sentence is ordinary body prose.",
                10.0,
                100.0,
                280.0,
                10.0,
            ),
            planned_segment(
                "The second page-wide sentence continues the discussion.",
                10.0,
                88.0,
                280.0,
                10.0,
            ),
            planned_segment("A left fragment of the third sentence ", 10.0, 76.0, 80.0, 10.0),
            planned_segment("continues across the false picture boundary.", 95.0, 76.0, 195.0, 10.0),
            planned_segment(
                "The fourth page-wide sentence completes the paragraph.",
                10.0,
                64.0,
                280.0,
                10.0,
            ),
            planned_segment("12", 120.0, 52.0, 20.0, 10.0),
            planned_segment("Concentration (g)", 160.0, 40.0, 80.0, 10.0),
        ];
        let hints = vec![planned_hint(LayoutHintClass::Picture, 100.0, 35.0, 300.0, 115.0)];

        let groups = plan_segment_groups_by_layout(&segments, &hints, &[], false, Some(320.0));

        let picture = groups
            .iter()
            .find(|group| {
                group
                    .region_path
                    .is_some_and(|path| path.root.class_name == Some(LayoutHintClass::Picture))
            })
            .expect("chart labels retain Picture ownership");
        assert_eq!(picture.segment_indices, vec![5, 6]);
        assert!(groups.iter().any(|group| group.segment_indices == vec![0, 1, 2, 3, 4]));
    }

    #[test]
    fn broad_alphabetic_diagram_labels_remain_owned_by_picture() {
        use crate::pdf::structure::types::LayoutHintClass;

        let segments = vec![
            planned_segment("Input validation and parsing", 10.0, 100.0, 280.0, 10.0),
            planned_segment("Feature extraction and routing", 10.0, 88.0, 280.0, 10.0),
            planned_segment("Model ", 10.0, 76.0, 80.0, 10.0),
            planned_segment("inference and scoring stage", 110.0, 76.0, 180.0, 10.0),
            planned_segment("Output formatting and storage", 10.0, 64.0, 280.0, 10.0),
        ];
        let hints = vec![planned_hint(LayoutHintClass::Picture, 100.0, 60.0, 300.0, 115.0)];

        let groups = plan_segment_groups_by_layout(&segments, &hints, &[], false, Some(320.0));

        assert!(groups.iter().any(|group| {
            group
                .region_path
                .is_some_and(|path| path.root.class_name == Some(LayoutHintClass::Picture))
        }));
    }

    #[test]
    fn side_by_side_body_text_does_not_demote_picture_labels() {
        use crate::pdf::structure::types::LayoutHintClass;

        let segments = vec![
            planned_segment("Series A", 220.0, 100.0, 30.0, 10.0),
            planned_segment(
                "The left body column contains ordinary paragraph text.",
                10.0,
                100.0,
                150.0,
                10.0,
            ),
            planned_segment("Series B", 220.0, 88.0, 30.0, 10.0),
            planned_segment(
                "Another body line supplies many alphabetic words here.",
                10.0,
                88.0,
                150.0,
                10.0,
            ),
            planned_segment("Series C", 220.0, 76.0, 30.0, 10.0),
            planned_segment(
                "The final body line remains outside the figure region.",
                10.0,
                76.0,
                150.0,
                10.0,
            ),
            planned_segment("Axis label", 220.0, 64.0, 30.0, 10.0),
        ];
        let hints = vec![planned_hint(LayoutHintClass::Picture, 200.0, 60.0, 300.0, 115.0)];

        let groups = plan_segment_groups_by_layout(&segments, &hints, &[], false, Some(320.0));

        let picture = groups
            .iter()
            .find(|group| {
                group
                    .region_path
                    .is_some_and(|path| path.root.class_name == Some(LayoutHintClass::Picture))
            })
            .expect("figure labels retain Picture ownership");
        assert_eq!(picture.segment_indices, vec![0, 2, 4, 6]);
    }

    #[test]
    fn false_picture_reconciliation_preserves_other_semantic_owners() {
        use crate::pdf::structure::types::LayoutHintClass;

        let segments = vec![
            planned_segment(
                "A full body sentence with enough words for prose.",
                10.0,
                100.0,
                280.0,
                10.0,
            ),
            planned_segment(
                "Another full body sentence continues the paragraph.",
                10.0,
                88.0,
                280.0,
                10.0,
            ),
            planned_segment("A left fragment of the third sentence ", 10.0, 76.0, 80.0, 10.0),
            planned_segment("continues across the false picture boundary.", 95.0, 76.0, 195.0, 10.0),
            planned_segment(
                "A final full body sentence closes the paragraph.",
                10.0,
                64.0,
                280.0,
                10.0,
            ),
            planned_segment("table cell", 10.0, 40.0, 70.0, 8.0),
            planned_segment("Figure 1.", 10.0, 25.0, 70.0, 8.0),
            planned_segment("ordinary text", 10.0, 10.0, 70.0, 8.0),
        ];
        let hints = vec![
            planned_hint(LayoutHintClass::Picture, 100.0, 60.0, 300.0, 115.0),
            planned_hint(LayoutHintClass::Table, 0.0, 35.0, 90.0, 50.0),
            planned_hint(LayoutHintClass::Caption, 0.0, 20.0, 90.0, 35.0),
            planned_hint(LayoutHintClass::Text, 0.0, 5.0, 90.0, 20.0),
        ];

        let groups = plan_segment_groups_by_layout(&segments, &hints, &[], false, Some(320.0));

        for (index, class_name) in [
            (5, LayoutHintClass::Table),
            (6, LayoutHintClass::Caption),
            (7, LayoutHintClass::Text),
        ] {
            assert!(groups.iter().any(|group| {
                group.segment_indices == vec![index]
                    && group
                        .region_path
                        .is_some_and(|path| path.root.class_name == Some(class_name))
            }));
        }
    }

    #[test]
    fn near_full_semantic_child_outranks_wrapper() {
        use crate::pdf::structure::types::LayoutHintClass;

        for class_name in [LayoutHintClass::Title, LayoutHintClass::ListItem, LayoutHintClass::Text] {
            let segments = vec![planned_segment("semantic", 0.0, 0.0, 100.0, 100.0)];
            let hints = vec![
                planned_hint(LayoutHintClass::Form, 0.0, 0.0, 100.0, 100.0),
                planned_hint(class_name, 5.0, 0.0, 95.0, 100.0),
            ];

            let groups = plan_segment_groups_by_layout(&segments, &hints, &[], true, None);
            assert_eq!(groups.len(), 1, "{class_name:?}");
            assert_eq!(groups[0].segment_indices, [0], "{class_name:?}");
            assert_eq!(groups[0].hint_indices, [1], "{class_name:?}");
            let path = groups[0].region_path.unwrap();
            assert_eq!(path.root.id, 0, "{class_name:?}");
            assert_eq!(path.child.unwrap().id, 1, "{class_name:?}");
        }
    }

    #[test]
    fn split_eli_t_fragments_share_the_owned_layout_owner() {
        use crate::pdf::structure::types::LayoutHintClass;

        let mut eli = planned_segment("eli", 100.0, 700.0, 15.0017, 11.0);
        eli.font_size = 11.0;
        let mut orphan_t = planned_segment("t", 115.0, 700.0, 5.0, 11.0);
        orphan_t.font_size = 11.0;
        let segments = vec![eli, orphan_t];
        let hints = vec![planned_hint(LayoutHintClass::Text, 100.0, 700.0, 115.0, 711.0)];

        let groups = plan_segment_groups_by_layout(&segments, &hints, &[], true, None);

        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].segment_indices, [0, 1]);
        assert_eq!(groups[0].hint_indices, [0]);
        assert_eq!(groups[0].region_path.unwrap().root.id, 0);
    }

    #[test]
    fn split_t_able_fragments_share_the_owned_layout_owner() {
        use crate::pdf::structure::types::LayoutHintClass;

        let mut orphan_t = planned_segment("T", 100.0, 700.0, 7.224, 11.0);
        orphan_t.font_size = 11.0;
        let mut able = planned_segment("able", 106.0, 700.0, 22.0, 11.0);
        able.font_size = 11.0;
        let segments = vec![orphan_t, able];
        let hints = vec![planned_hint(LayoutHintClass::Text, 106.0, 700.0, 128.0, 711.0)];

        let groups = plan_segment_groups_by_layout(&segments, &hints, &[], true, None);

        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].segment_indices, [0, 1]);
        assert_eq!(groups[0].hint_indices, [0]);
        assert_eq!(groups[0].region_path.unwrap().root.id, 0);
    }

    #[test]
    fn atomic_fragments_with_distinct_text_owners_preserve_both_paths() {
        use crate::pdf::structure::types::LayoutHintClass;

        let segments = vec![
            planned_segment("eli", 100.0, 700.0, 15.0, 10.0),
            planned_segment("t", 115.0, 700.0, 5.0, 10.0),
        ];
        let hints = vec![
            planned_hint(LayoutHintClass::Text, 100.0, 700.0, 115.0, 710.0),
            planned_hint(LayoutHintClass::Text, 115.0, 700.0, 120.0, 710.0),
        ];

        let groups = plan_segment_groups_by_layout(&segments, &hints, &[], true, None);

        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].segment_indices, [0]);
        assert_eq!(groups[0].region_path.unwrap().root.id, 0);
        assert_eq!(groups[1].segment_indices, [1]);
        assert_eq!(groups[1].region_path.unwrap().root.id, 1);
    }

    #[test]
    fn atomic_fragment_ownership_rejects_spacing_and_geometry_boundaries() {
        use crate::pdf::structure::types::LayoutHintClass;

        let text_classes = [LayoutHintClass::Text; 2];
        assert_fragments_remain_separate(
            vec![
                planned_segment("office", 100.0, 700.0, 30.0, 10.0),
                planned_segment("is", 140.0, 700.0, 8.0, 10.0),
            ],
            text_classes,
        );
        assert_fragments_remain_separate(
            vec![
                planned_segment("right", 200.0, 700.0, 20.0, 10.0),
                planned_segment("left", 10.0, 700.0, 20.0, 10.0),
            ],
            text_classes,
        );
        assert_fragments_remain_separate(
            vec![
                planned_segment("end", 100.0, 700.0, 15.0, 10.0),
                planned_segment("start", 115.0, 680.0, 20.0, 10.0),
            ],
            text_classes,
        );
    }

    #[test]
    fn atomic_fragment_ownership_rejects_excessive_overlap() {
        use crate::pdf::structure::types::LayoutHintClass;

        let segments = vec![
            planned_segment("owned", 100.0, 700.0, 30.0, 10.0),
            planned_segment("orphan", 110.0, 700.0, 200.0, 10.0),
        ];
        let hints = vec![planned_hint(LayoutHintClass::Text, 100.0, 700.0, 130.0, 710.0)];

        let groups = plan_segment_groups_by_layout(&segments, &hints, &[], true, None);
        let flattened = groups
            .iter()
            .flat_map(|group| group.segment_indices.iter().copied())
            .collect::<Vec<_>>();

        assert_eq!(groups.len(), 2);
        assert_eq!(flattened, [0, 1]);
    }

    #[test]
    fn atomic_fragment_ownership_rejects_style_and_text_boundaries() {
        use crate::pdf::structure::types::LayoutHintClass;

        let text_classes = [LayoutHintClass::Text; 2];
        let mut bold = planned_segment("t", 115.0, 700.0, 5.0, 10.0);
        bold.is_bold = true;
        assert_fragments_remain_separate(
            vec![planned_segment("eli", 100.0, 700.0, 15.0, 10.0), bold],
            text_classes,
        );
        let mut different_size = planned_segment("t", 115.0, 700.0, 5.0, 10.0);
        different_size.font_size = 11.0;
        assert_fragments_remain_separate(
            vec![planned_segment("eli", 100.0, 700.0, 15.0, 10.0), different_size],
            text_classes,
        );
        assert_fragments_remain_separate(
            vec![
                planned_segment("eli ", 100.0, 700.0, 15.0, 10.0),
                planned_segment("t", 115.0, 700.0, 5.0, 10.0),
            ],
            text_classes,
        );
        assert_fragments_remain_separate(
            vec![
                planned_segment("\u{4e00}", 100.0, 700.0, 12.0, 10.0),
                planned_segment("\u{4e01}", 112.0, 700.0, 12.0, 10.0),
            ],
            text_classes,
        );
    }

    #[test]
    fn atomic_fragment_ownership_rejects_semantic_boundaries() {
        use crate::pdf::structure::types::LayoutHintClass;

        assert_fragments_remain_separate(
            vec![
                planned_segment("semantic", 100.0, 700.0, 40.0, 10.0),
                planned_segment("boundary", 140.0, 700.0, 40.0, 10.0),
            ],
            [LayoutHintClass::Text, LayoutHintClass::Caption],
        );
    }

    #[test]
    fn atomic_fragment_ownership_rejects_invalid_geometry_without_dropping_segments() {
        use crate::pdf::structure::types::LayoutHintClass;

        let segments = vec![
            planned_segment("valid", 100.0, 700.0, 20.0, 10.0),
            planned_segment("invalid", f32::NAN, 700.0, 10.0, 10.0),
        ];
        let hints = vec![planned_hint(LayoutHintClass::Text, 100.0, 700.0, 120.0, 710.0)];

        let groups = plan_segment_groups_by_layout(&segments, &hints, &[], true, None);
        let flattened = groups
            .iter()
            .flat_map(|group| group.segment_indices.iter().copied())
            .collect::<Vec<_>>();

        assert_eq!(groups.len(), 2);
        assert_eq!(flattened, [0, 1]);
    }

    #[test]
    fn valid_non_overlapping_hint_returns_pathless_fallback() {
        use crate::pdf::structure::types::LayoutHintClass;

        let segments = vec![planned_segment("outside", 200.0, 200.0, 20.0, 10.0)];
        let hints = vec![planned_hint(LayoutHintClass::Text, 0.0, 0.0, 100.0, 100.0)];

        let groups = plan_segment_groups_by_layout(&segments, &hints, &[], true, None);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].segment_indices, [0]);
        assert!(groups[0].hint_indices.is_empty());
        assert!(groups[0].region_path.is_none());
    }

    #[test]
    fn plan_keeps_uncovered_runs_distinct_and_complete() {
        use crate::pdf::structure::types::LayoutHintClass;

        let segments = vec![
            planned_segment("outside-before", 200.0, 80.0, 10.0, 10.0),
            planned_segment("inside", 10.0, 50.0, 10.0, 10.0),
            planned_segment("outside-after", 200.0, 20.0, 10.0, 10.0),
        ];
        let hints = vec![planned_hint(LayoutHintClass::Text, 0.0, 40.0, 100.0, 70.0)];

        let groups = plan_segment_groups_by_layout(&segments, &hints, &[], true, None);
        let flattened = groups
            .iter()
            .flat_map(|group| group.segment_indices.iter().copied())
            .collect::<Vec<_>>();
        assert_eq!(flattened, [0, 1, 2]);
        assert_eq!(groups.len(), 3);
        assert_ne!(
            groups[0].region_path.unwrap().root.id,
            groups[2].region_path.unwrap().root.id
        );
    }

    #[test]
    fn plan_rejects_non_finite_derived_geometry() {
        use crate::pdf::structure::types::LayoutHintClass;

        let segments = vec![planned_segment("overflow", f32::MAX, 10.0, f32::MAX, 10.0)];
        let hints = vec![planned_hint(LayoutHintClass::Text, 0.0, 0.0, 100.0, 100.0)];
        let groups = plan_segment_groups_by_layout(&segments, &hints, &[], true, None);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].segment_indices, [0]);
        assert!(groups[0].hint_indices.is_empty());

        let invalid_hint = vec![planned_hint(LayoutHintClass::Text, 0.0, 0.0, f32::INFINITY, 100.0)];
        let groups = plan_segment_groups_by_layout(&segments, &invalid_hint, &[], true, None);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].segment_indices, [0]);
        assert!(groups[0].region_path.is_none());
    }

    #[test]
    fn empty_wrapper_validation_promotes_child_to_root() {
        use crate::pdf::structure::types::LayoutHintClass;

        let segments = vec![planned_segment("child", 20.0, 70.0, 20.0, 10.0)];
        let hints = vec![
            planned_hint(LayoutHintClass::Picture, 0.0, 0.0, 100.0, 100.0),
            planned_hint(LayoutHintClass::Caption, 10.0, 60.0, 50.0, 90.0),
        ];
        let groups = plan_segment_groups_by_layout(&segments, &hints, &[false], true, None);
        let path = groups[0].region_path.unwrap();
        assert_eq!(path.root.id, 1);
        assert!(path.child.is_none());
    }

    #[test]
    fn test_detect_columns_two_column_layout() {
        let regions = vec![
            RegionProjection {
                left: 100.0,
                bottom: 100.0,
                right: 200.0,
                top: 500.0,
                span_indices: vec![],
            },
            RegionProjection {
                left: 400.0,
                bottom: 100.0,
                right: 500.0,
                top: 500.0,
                span_indices: vec![],
            },
        ];

        let assignments = detect_columns(&regions);
        assert_eq!(assignments.len(), 2);
        assert_ne!(assignments[0], assignments[1]);
        assert_eq!(assignments[0], 0);
        assert_eq!(assignments[1], 1);
    }

    #[test]
    fn test_project_spans_to_regions() {
        let spans = vec![
            TextSpan {
                text: "Left column".to_string(),
                x: 110.0,
                y: 450.0,
                width: 70.0,
                height: 12.0,
            },
            TextSpan {
                text: "Right column".to_string(),
                x: 410.0,
                y: 450.0,
                width: 75.0,
                height: 12.0,
            },
        ];

        let hints = vec![
            LayoutHint {
                class_name: crate::pdf::structure::types::LayoutHintClass::Text,
                confidence: 0.95,
                left: 100.0,
                bottom: 100.0,
                right: 200.0,
                top: 500.0,
            },
            LayoutHint {
                class_name: crate::pdf::structure::types::LayoutHintClass::Text,
                confidence: 0.95,
                left: 400.0,
                bottom: 100.0,
                right: 500.0,
                top: 500.0,
            },
        ];

        let regions = project_spans_to_regions(&spans, &hints);
        assert_eq!(regions.len(), 2);
        assert_eq!(regions[0].span_indices.len(), 1);
        assert_eq!(regions[0].span_indices[0], 0);
        assert_eq!(regions[1].span_indices.len(), 1);
        assert_eq!(regions[1].span_indices[0], 1);
    }

    #[test]
    fn test_reorder_spans_two_column_layout() {
        let spans = vec![
            TextSpan {
                text: "A".to_string(),
                x: 110.0,
                y: 450.0,
                width: 10.0,
                height: 12.0,
            },
            TextSpan {
                text: "B".to_string(),
                x: 110.0,
                y: 200.0,
                width: 10.0,
                height: 12.0,
            },
            TextSpan {
                text: "C".to_string(),
                x: 410.0,
                y: 450.0,
                width: 10.0,
                height: 12.0,
            },
            TextSpan {
                text: "D".to_string(),
                x: 410.0,
                y: 200.0,
                width: 10.0,
                height: 12.0,
            },
        ];

        let hints = vec![
            LayoutHint {
                class_name: crate::pdf::structure::types::LayoutHintClass::Text,
                confidence: 0.95,
                left: 100.0,
                bottom: 100.0,
                right: 200.0,
                top: 500.0,
            },
            LayoutHint {
                class_name: crate::pdf::structure::types::LayoutHintClass::Text,
                confidence: 0.95,
                left: 400.0,
                bottom: 100.0,
                right: 500.0,
                top: 500.0,
            },
        ];

        let order = reorder_spans_by_layout(&spans, &hints);
        assert_eq!(order.len(), 4);
        assert_eq!(order[0], 0);
        assert_eq!(order[1], 1);
        assert_eq!(order[2], 2);
        assert_eq!(order[3], 3);
    }

    /// Segment-level reorder must produce true column-major reading order from
    /// interleaved input, independent of the layout-hint ordering. The hints
    /// here are supplied right-column-first; a correct reorder still yields
    /// A, B, C, D (left column top-to-bottom, then right column). A previous
    /// implementation emitted segments in raw hint order and would yield
    /// C, D, A, B here — this is the regression guard.
    #[test]
    fn test_reorder_segments_two_column_independent_of_hint_order() {
        fn seg(text: &str, x: f32, y: f32) -> crate::pdf::hierarchy::SegmentData {
            crate::pdf::hierarchy::SegmentData {
                text: text.to_string(),
                x,
                y,
                width: 10.0,
                height: 12.0,
                font_size: 10.0,
                is_bold: false,
                is_italic: false,
                is_monospace: false,
                baseline_y: y,
                assigned_role: None,
            }
        }

        let segments = vec![
            seg("A", 110.0, 450.0),
            seg("C", 410.0, 450.0),
            seg("B", 110.0, 200.0),
            seg("D", 410.0, 200.0),
        ];

        let hints = vec![
            LayoutHint {
                class_name: crate::pdf::structure::types::LayoutHintClass::Text,
                confidence: 0.95,
                left: 400.0,
                bottom: 100.0,
                right: 500.0,
                top: 500.0,
            },
            LayoutHint {
                class_name: crate::pdf::structure::types::LayoutHintClass::Text,
                confidence: 0.95,
                left: 100.0,
                bottom: 100.0,
                right: 200.0,
                top: 500.0,
            },
        ];

        let reordered = reorder_segments_by_layout(segments, &hints, Some(500.0));
        let order: Vec<&str> = reordered.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(
            order,
            vec!["A", "B", "C", "D"],
            "segments must be reordered column-major top-to-bottom regardless of hint order"
        );
    }

    #[test]
    fn test_reorder_segments_full_width_heading_breaks_columns() {
        fn seg(text: &str, x: f32, y: f32) -> crate::pdf::hierarchy::SegmentData {
            crate::pdf::hierarchy::SegmentData {
                text: text.to_string(),
                x,
                y,
                width: 10.0,
                height: 12.0,
                font_size: 10.0,
                is_bold: false,
                is_italic: false,
                is_monospace: false,
                baseline_y: y,
                assigned_role: None,
            }
        }

        // A full-width title above two columns. The predecessor graph must emit
        // the title first, then the whole left column, then the whole right
        // column — the title interrupts any left→right chaining across columns. ~keep
        let segments = vec![
            seg("Title", 50.0, 470.0),
            seg("L1", 50.0, 440.0),
            seg("R1", 270.0, 440.0),
            seg("L2", 50.0, 300.0),
            seg("R2", 270.0, 300.0),
        ];

        fn hint(left: f32, bottom: f32, right: f32, top: f32) -> LayoutHint {
            LayoutHint {
                class_name: crate::pdf::structure::types::LayoutHintClass::Text,
                confidence: 0.95,
                left,
                bottom,
                right,
                top,
            }
        }

        let hints = vec![
            hint(40.0, 460.0, 460.0, 490.0),
            hint(40.0, 100.0, 240.0, 450.0),
            hint(260.0, 100.0, 460.0, 450.0),
        ];

        let reordered = reorder_segments_by_layout(segments, &hints, Some(500.0));
        let order: Vec<&str> = reordered.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(
            order,
            vec!["Title", "L1", "L2", "R1", "R2"],
            "full-width heading must precede both columns, then each column reads top-to-bottom \
             without interleaving across the column boundary"
        );
    }

    #[test]
    fn test_reorder_spans_mixed_columns() {
        let spans = vec![
            TextSpan {
                text: "A".to_string(),
                x: 110.0,
                y: 480.0,
                width: 10.0,
                height: 12.0,
            },
            TextSpan {
                text: "B".to_string(),
                x: 110.0,
                y: 300.0,
                width: 10.0,
                height: 12.0,
            },
            TextSpan {
                text: "C".to_string(),
                x: 410.0,
                y: 470.0,
                width: 10.0,
                height: 12.0,
            },
            TextSpan {
                text: "D".to_string(),
                x: 410.0,
                y: 300.0,
                width: 10.0,
                height: 12.0,
            },
            TextSpan {
                text: "E".to_string(),
                x: 410.0,
                y: 150.0,
                width: 10.0,
                height: 12.0,
            },
            TextSpan {
                text: "X".to_string(),
                x: 550.0,
                y: 300.0,
                width: 10.0,
                height: 12.0,
            },
        ];

        let hints = vec![
            LayoutHint {
                class_name: crate::pdf::structure::types::LayoutHintClass::Text,
                confidence: 0.95,
                left: 100.0,
                bottom: 100.0,
                right: 200.0,
                top: 500.0,
            },
            LayoutHint {
                class_name: crate::pdf::structure::types::LayoutHintClass::Text,
                confidence: 0.95,
                left: 400.0,
                bottom: 100.0,
                right: 500.0,
                top: 500.0,
            },
        ];

        let order = reorder_spans_by_layout(&spans, &hints);
        assert_eq!(order.len(), 6);
        assert_eq!(order[0], 0);
        assert_eq!(order[1], 1);
        assert_eq!(order[2], 2);
        assert_eq!(order[3], 3);
        assert_eq!(order[4], 4);
        assert_eq!(order[5], 5);
    }

    #[test]
    fn test_reorder_spans_empty_input() {
        let spans = vec![];
        let hints = vec![];
        let order = reorder_spans_by_layout(&spans, &hints);
        assert!(order.is_empty());
    }

    #[test]
    fn test_reorder_spans_no_hints() {
        let spans = vec![
            TextSpan {
                text: "A".to_string(),
                x: 100.0,
                y: 100.0,
                width: 10.0,
                height: 12.0,
            },
            TextSpan {
                text: "B".to_string(),
                x: 120.0,
                y: 100.0,
                width: 10.0,
                height: 12.0,
            },
        ];
        let hints = vec![];
        let order = reorder_spans_by_layout(&spans, &hints);
        assert_eq!(order, vec![0, 1]);
    }

    #[test]
    fn test_config_default_reading_order_is_false() {
        let pdf_config = crate::core::config::PdfConfig::default();
        assert!(
            !pdf_config.reading_order,
            "Default reading_order must be false for backward compatibility"
        );
    }

    /// Test that within a region, a heading with a higher native index than its
    /// subsections is now emitted FIRST (top-to-bottom, not native order).
    /// This guards against issue #1170: chapter heading emitted after subsections.
    #[test]
    fn test_intra_region_segment_ordering_heading_before_subsections() {
        fn seg(text: &str, x: f32, y: f32) -> crate::pdf::hierarchy::SegmentData {
            crate::pdf::hierarchy::SegmentData {
                text: text.to_string(),
                x,
                y,
                width: 80.0,
                height: 12.0,
                font_size: 10.0,
                is_bold: false,
                is_italic: false,
                is_monospace: false,
                baseline_y: y,
                assigned_role: None,
            }
        }

        let segments = vec![
            seg("2.1 Algemeen", 50.0, 200.0),
            seg("2.1.1 ErP label", 50.0, 180.0),
            seg("2.1.2 Gascategorie", 50.0, 160.0),
            seg("Table row 1", 50.0, 140.0),
            seg("2 TOESTELGEGEVENS", 50.0, 450.0),
        ];

        let hints = vec![LayoutHint {
            class_name: crate::pdf::structure::types::LayoutHintClass::Text,
            confidence: 0.95,
            left: 40.0,
            bottom: 100.0,
            right: 400.0,
            top: 500.0,
        }];

        let reordered = reorder_segments_by_layout(segments, &hints, Some(500.0));
        let order: Vec<&str> = reordered.iter().map(|s| s.text.as_str()).collect();

        assert_eq!(
            order,
            vec![
                "2 TOESTELGEGEVENS",
                "2.1 Algemeen",
                "2.1.1 ErP label",
                "2.1.2 Gascategorie",
                "Table row 1"
            ],
            "Within a region, segments must be ordered by top coordinate (y + height) descending, \
             so the heading (y=450) comes before its subsections (y=200, 180, 160, 140)"
        );
    }

    /// Test that sub-subsections are ordered correctly (2.1.1 before 2.1.2)
    /// when they have inverted native indices.
    #[test]
    fn test_intra_region_subsection_ordering() {
        fn seg(text: &str, x: f32, y: f32) -> crate::pdf::hierarchy::SegmentData {
            crate::pdf::hierarchy::SegmentData {
                text: text.to_string(),
                x,
                y,
                width: 80.0,
                height: 12.0,
                font_size: 10.0,
                is_bold: false,
                is_italic: false,
                is_monospace: false,
                baseline_y: y,
                assigned_role: None,
            }
        }

        let segments = vec![
            seg("2.1.2 Gascategorie", 50.0, 180.0),
            seg("2.1.1 ErP label", 50.0, 200.0),
        ];

        let hints = vec![LayoutHint {
            class_name: crate::pdf::structure::types::LayoutHintClass::Text,
            confidence: 0.95,
            left: 40.0,
            bottom: 100.0,
            right: 400.0,
            top: 500.0,
        }];

        let reordered = reorder_segments_by_layout(segments, &hints, Some(500.0));
        let order: Vec<&str> = reordered.iter().map(|s| s.text.as_str()).collect();

        assert_eq!(
            order,
            vec!["2.1.1 ErP label", "2.1.2 Gascategorie"],
            "Segments within a region must be ordered by y coordinate, \
             so 2.1.1 (y=200) comes before 2.1.2 (y=180)"
        );
    }

    /// Test that span ordering works correctly within regions, matching segment behavior
    #[test]
    fn test_intra_region_span_ordering_heading_before_subsections() {
        let spans = vec![
            TextSpan {
                text: "2.1 Algemeen".to_string(),
                x: 50.0,
                y: 200.0,
                width: 80.0,
                height: 12.0,
            },
            TextSpan {
                text: "2.1.1 ErP".to_string(),
                x: 50.0,
                y: 180.0,
                width: 60.0,
                height: 12.0,
            },
            TextSpan {
                text: "2.1.2 Gas".to_string(),
                x: 50.0,
                y: 160.0,
                width: 60.0,
                height: 12.0,
            },
            TextSpan {
                text: "2 TOESTEL".to_string(),
                x: 50.0,
                y: 450.0,
                width: 80.0,
                height: 12.0,
            },
        ];

        let hints = vec![LayoutHint {
            class_name: crate::pdf::structure::types::LayoutHintClass::Text,
            confidence: 0.95,
            left: 40.0,
            bottom: 100.0,
            right: 400.0,
            top: 500.0,
        }];

        let order = reorder_spans_by_layout(&spans, &hints);
        assert_eq!(
            order,
            vec![3, 0, 1, 2],
            "Spans within a region must be ordered by top coordinate descending: \
             index 3 (y=450) first, then 0, 1, 2 (y=200, 180, 160)"
        );
    }

    /// Regression for issue #1198: NaN f32 coordinates in PDF spans create a cyclic
    /// comparison with `partial_cmp + unwrap_or(Equal)`, causing Rust's driftsort to
    /// panic with "comparison function does not correctly implement a total order".
    ///
    /// Concrete cycle produced by the old comparator (all 3 spans land in column 0
    /// because their x-centers are within COLUMN_MERGE_THRESHOLD_PTS):
    ///
    ///   A: top=NaN  x=1.0    B: top=17.0  x=0.0    C: top=22.0  x=2.0
    ///
    ///   compare(A, B): primary NaN→Equal, secondary x_A(1.0)>x_B(0.0) → Greater  (B before A)
    ///   compare(B, C): primary 17.0<22.0 → Greater                               (C before B)
    ///   compare(A, C): primary NaN→Equal, secondary x_A(1.0)<x_C(2.0) → Less    (A before C)
    ///
    /// → cycle  B < A,  C < B,  A < C  →  B < A < C < B  — driftsort panics.
    ///
    /// Fixed by using f32::total_cmp which places NaN after +inf, eliminating all
    /// non-finite ambiguity.  With total_cmp: NaN > 22.0 > 17.0, so A sorts first.
    #[test]
    fn test_geometric_sort_with_nan_top_does_not_panic() {
        let spans = vec![
            TextSpan {
                text: "A".to_string(),
                x: 1.0,
                y: f32::NAN,
                width: 10.0,
                height: 12.0,
            },
            TextSpan {
                text: "B".to_string(),
                x: 0.0,
                y: 5.0,
                width: 10.0,
                height: 12.0,
            },
            TextSpan {
                text: "C".to_string(),
                x: 2.0,
                y: 10.0,
                width: 10.0,
                height: 12.0,
            },
        ];
        let order = reorder_spans_geometric(&spans);
        assert_eq!(order.len(), 3, "all spans must be returned");
        assert_eq!(
            order[0], 0,
            "span with NaN top must sort first (NaN > finite in total_cmp)"
        );
        assert_eq!(order[1], 2, "C (top=22) must precede B (top=17)");
        assert_eq!(order[2], 1, "B (top=17) must be last");
    }

    /// Test geometric column detection when layout hints are absent
    #[test]
    fn test_geometric_column_fallback_two_columns() {
        let spans = vec![
            TextSpan {
                text: "Left top".to_string(),
                x: 50.0,
                y: 450.0,
                width: 80.0,
                height: 12.0,
            },
            TextSpan {
                text: "Left bottom".to_string(),
                x: 50.0,
                y: 200.0,
                width: 80.0,
                height: 12.0,
            },
            TextSpan {
                text: "Right top".to_string(),
                x: 300.0,
                y: 450.0,
                width: 80.0,
                height: 12.0,
            },
            TextSpan {
                text: "Right bottom".to_string(),
                x: 300.0,
                y: 200.0,
                width: 80.0,
                height: 12.0,
            },
        ];

        let order = reorder_spans_by_layout(&spans, &[]);
        assert_eq!(
            order,
            vec![0, 1, 2, 3],
            "Without hints, geometric fallback should detect columns by x-center \
             and order left column (0,1) before right column (2,3), top-to-bottom"
        );
    }
}
