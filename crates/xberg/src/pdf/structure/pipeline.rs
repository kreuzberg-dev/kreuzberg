//! Main PDF-to-Markdown pipeline orchestrator (oxide backend).

use std::borrow::Cow;

use crate::pdf::bookmarks::PdfOutlineEntry;
use crate::pdf::error::Result;
use crate::pdf::hierarchy::{BoundingBox, SegmentData, TextBlock, assign_heading_levels_smart, cluster_font_sizes};
#[cfg(not(target_arch = "wasm32"))]
use rayon::prelude::*;

use super::assembly::assemble_internal_document;
use super::classify::{
    classify_paragraphs, demote_heading_runs, demote_structure_annotation_headings, demote_unnumbered_subsections,
    mark_arxiv_noise, mark_cross_page_repeating_short_text, mark_cross_page_repeating_text, refine_heading_hierarchy,
};
use super::constants::{FULL_LINE_FRACTION, MIN_BLOCKS_FOR_FONT_HEADING, MIN_HEADING_FONT_GAP, MIN_HEADING_FONT_RATIO};
use super::lines::is_cjk_char;
use super::paragraphs::{merge_continuation_paragraphs, split_embedded_list_items};
use super::text_repair::{
    apply_to_all_segments, clean_duplicate_punctuation, collapse_spaced_hyphens,
    expand_ligatures_with_space_absorption, normalize_text_encoding, normalize_unicode_text,
    repair_contextual_ligatures, repair_ligature_spaces,
};
use super::types::{LayoutHint, PdfParagraph};

/// Stage 2: Cluster font sizes globally and assign heading levels.
///
/// Returns (heading_map, set of struct-tree page indices needing font-size classification).
#[allow(clippy::type_complexity)]
fn build_heading_map(
    all_page_segments: &[Vec<SegmentData>],
    struct_tree_results: &[Option<Vec<PdfParagraph>>],
    heuristic_pages: &[usize],
    k_clusters: usize,
) -> Result<(Vec<(f32, Option<u8>)>, ahash::AHashSet<usize>)> {
    let struct_tree_needs_classify: ahash::AHashSet<usize> = struct_tree_results
        .iter()
        .enumerate()
        .filter_map(|(i, result)| {
            result.as_ref().and_then(|paragraphs| {
                let has_headings = paragraphs.iter().any(|p| p.heading_level.is_some());
                if !has_headings && has_font_size_variation(paragraphs) {
                    Some(i)
                } else {
                    None
                }
            })
        })
        .collect();

    let mut all_blocks: Vec<TextBlock> = Vec::new();
    let empty_bbox = BoundingBox {
        left: 0.0,
        top: 0.0,
        right: 0.0,
        bottom: 0.0,
    };
    // The text is carried so `assign_heading_levels_smart` can pick the body
    // cluster by character mass (char-weighted body size). Leaving it empty makes
    // every cluster tie at length 0, so `max_by_key` falls back to the smallest
    // font as "body" and over-promotes every larger run to a heading. ~keep
    for &i in heuristic_pages {
        for seg in &all_page_segments[i] {
            if seg.text.trim().is_empty() {
                continue;
            }
            all_blocks.push(TextBlock {
                text: seg.text.clone(),
                bbox: empty_bbox,
                font_size: seg.font_size,
            });
        }
    }
    for &i in &struct_tree_needs_classify {
        if let Some(paragraphs) = &struct_tree_results[i] {
            for para in paragraphs {
                let text = if !para.text.is_empty() {
                    para.text.clone()
                } else {
                    para.lines
                        .iter()
                        .flat_map(|l| l.segments.iter())
                        .map(|s| s.text.as_str())
                        .collect::<Vec<_>>()
                        .join(" ")
                };
                all_blocks.push(TextBlock {
                    text,
                    bbox: empty_bbox,
                    font_size: para.dominant_font_size,
                });
            }
        }
    }

    let paragraph_count = all_blocks.len();
    let heading_map = if all_blocks.is_empty() {
        Vec::new()
    } else if paragraph_count < MIN_BLOCKS_FOR_FONT_HEADING {
        // Sparsity gate: too few text blocks to establish a reliable body-font
        // baseline. Return a body-only map (every cluster centroid mapped to
        // `None`) and skip both k-means heading promotion and the fallback
        // title promotion, so a lone larger line on a cover/title/one-line
        // document is not over-promoted to a heading. ~keep
        tracing::debug!(
            paragraph_count,
            min_blocks = MIN_BLOCKS_FOR_FONT_HEADING,
            "heading map: document too sparse for font-size heading inference; suppressing promotion"
        );
        let clusters = cluster_font_sizes(&all_blocks, 1)?;
        clusters.iter().map(|c| (c.centroid, None)).collect()
    } else {
        let effective_k = if paragraph_count < 20 {
            k_clusters.min(2usize.max(paragraph_count / 4))
        } else {
            k_clusters
        };

        let clusters = cluster_font_sizes(&all_blocks, effective_k)?;
        let mut map = assign_heading_levels_smart(&clusters, MIN_HEADING_FONT_RATIO, MIN_HEADING_FONT_GAP);

        let has_any_heading = map.iter().any(|(_, level)| level.is_some());
        if !has_any_heading && !heuristic_pages.is_empty() {
            let first_page = heuristic_pages[0];
            let first_seg_font = all_page_segments[first_page]
                .iter()
                .find(|s| !s.text.trim().is_empty())
                .map(|s| s.font_size);

            if let Some(first_font) = first_seg_font {
                let mut sizes: Vec<f32> = all_blocks.iter().map(|b| b.font_size).collect();
                sizes.sort_by(|a, b| a.total_cmp(b));
                let median = if sizes.is_empty() { 0.0 } else { sizes[sizes.len() / 2] };

                if median > 0.0
                    && first_font >= median * 1.2
                    && let Some(entry) = map.iter_mut().find(|(fs, _)| (*fs - first_font).abs() < 0.5)
                {
                    entry.1 = Some(1);
                }
            }
        }

        map
    };

    Ok((heading_map, struct_tree_needs_classify))
}

/// Build a heading map from structure-tree-assigned roles on segments.
///
/// Instead of clustering font sizes heuristically, this examines the
/// `assigned_role` field on each segment (populated from the PDF structure tree).
/// Each unique font size is mapped to the heading level most commonly assigned
/// to segments at that size. Font sizes with no assigned role are treated as body text.
fn build_heading_map_from_assigned_roles(all_page_segments: &[Vec<SegmentData>]) -> Vec<(f32, Option<u8>)> {
    use std::collections::HashMap;

    let mut size_roles: HashMap<u32, Vec<Option<u8>>> = HashMap::new();
    for page_segs in all_page_segments {
        for seg in page_segs {
            if seg.text.trim().is_empty() {
                continue;
            }
            let key = (seg.font_size * 10.0).round() as u32;
            size_roles.entry(key).or_default().push(seg.assigned_role);
        }
    }

    let mut heading_map: Vec<(f32, Option<u8>)> = size_roles
        .into_iter()
        .map(|(quantized_size, roles)| {
            let font_size = quantized_size as f32 / 10.0;
            let total = roles.len();
            let mut level_counts: HashMap<u8, usize> = HashMap::new();
            let mut none_count = 0usize;
            for role in &roles {
                match role {
                    Some(level) => *level_counts.entry(*level).or_default() += 1,
                    None => none_count += 1,
                }
            }
            let dominant_level = level_counts
                .into_iter()
                .max_by_key(|(_, count)| *count)
                .and_then(|(level, count)| if count * 2 >= total { Some(level) } else { None });

            if none_count > total / 2 && dominant_level.is_none() {
                (font_size, None)
            } else {
                (font_size, dominant_level)
            }
        })
        .collect();

    heading_map.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

    heading_map
}

/// Font-size tolerance (points) for merging consecutive raw segments into one
/// logical block in [`count_logical_blocks`]. Matches the font-change
/// threshold `blocks_to_paragraphs` uses to decide paragraph breaks, so a
/// single logical line that a font extractor split into several same-size
/// runs (ligature repair, kerning artifacts, mid-word splits) is not
/// double-counted as multiple blocks.
const LOGICAL_BLOCK_FONT_TOLERANCE: f32 = 1.5;

/// Count logical text blocks by merging consecutive same-role, same-size
/// segments, rather than counting raw segments.
///
/// Raw segment extraction can split one visual line into several runs (a
/// mid-word split from a font-encoding quirk, a bold/italic switch inside a
/// single sentence) that all carry the same `assigned_role`. Counting those
/// raw segments as separate "blocks" over-counts a document's real size and
/// can push a genuinely tiny document (see `hello_structure.pdf`,
/// `issue-987-test.pdf`) above the sparsity floor it should fall under.
/// Consecutive segments on the same page with the same `assigned_role` and a
/// font size within [`LOGICAL_BLOCK_FONT_TOLERANCE`] points collapse into a
/// single block, matching the granularity `total_paragraphs` eventually
/// reports after paragraph assembly.
fn count_logical_blocks(all_page_segments: &[Vec<SegmentData>]) -> usize {
    let mut total = 0usize;
    for page_segs in all_page_segments {
        let mut prev: Option<&SegmentData> = None;
        for seg in page_segs {
            if seg.text.trim().is_empty() {
                continue;
            }
            let continues_prev = prev.is_some_and(|p| {
                p.assigned_role == seg.assigned_role
                    && (p.font_size - seg.font_size).abs() <= LOGICAL_BLOCK_FONT_TOLERANCE
            });
            if !continues_prev {
                total += 1;
            }
            prev = Some(seg);
        }
    }
    total
}

/// Suppress structure-tree heading roles on documents too sparse to trust them.
///
/// A tagged PDF's structure tree is normally a reliable ground truth for
/// heading levels, but on a document with only a handful of text blocks (a
/// one-line note, a cover slide, a two-paragraph test fixture) the same
/// per-block noise that makes font-size clustering unreliable on small
/// samples (see [`MIN_BLOCKS_FOR_FONT_HEADING`] on the heuristic path) also
/// undermines the structure tree: a single mis-tagged or inconsistently
/// authored run is enough to make an entire tiny document look like it is
/// "mostly headings" even when nothing in it is a genuine section heading.
/// Below the same block floor, heading roles are suppressed regardless of
/// whether a body tier is present, matching the heuristic path's rule that
/// a reliable body-font baseline (or, here, a reliable heading/body
/// contrast) needs more than a couple of paragraphs to establish.
///
/// When the condition holds, every segment's `assigned_role` is cleared so
/// paragraph classification (which reads `assigned_role` directly,
/// bypassing the heading map) also treats the document as plain text, and
/// `heading_map` is rewritten to a single body-only entry.
///
/// Returns `true` when suppression fired.
fn suppress_all_heading_roles_when_sparse_and_untrusted(
    heading_map: &mut Vec<(f32, Option<u8>)>,
    all_page_segments: &mut [Vec<SegmentData>],
) -> bool {
    let total_blocks = count_logical_blocks(all_page_segments);
    let has_any_heading = heading_map.iter().any(|(_, level)| level.is_some());

    if total_blocks == 0 || total_blocks >= MIN_BLOCKS_FOR_FONT_HEADING || !has_any_heading {
        return false;
    }

    tracing::debug!(
        total_blocks,
        min_blocks = MIN_BLOCKS_FOR_FONT_HEADING,
        "structure tree: document too sparse to trust tagged heading roles; suppressing all heading roles"
    );

    for page_segs in all_page_segments.iter_mut() {
        for seg in page_segs.iter_mut() {
            seg.assigned_role = None;
        }
    }
    heading_map.clear();
    true
}

/// Promote an untagged document-title font tier above structure-tree headings.
///
/// Word processors tag the document title with a non-heading structure type
/// (e.g. LibreOffice's "Title" style resolves to a non-`H*` element), so
/// `build_heading_map_from_assigned_roles` classifies it as body text even
/// though it is visually the top-level heading. When such a tier exists —
/// strictly larger than every tagged heading font, bold, few segments, and
/// present on the first page — assign it level 1 and demote all tagged
/// heading levels by one (Title = h1, tagged H1 = h2, ...), matching the
/// pandoc/HTML convention that the document title outranks section headings.
///
/// Returns `true` when a title tier was promoted. The caller must then also
/// demote the per-segment `assigned_role` values (see
/// `demote_assigned_roles`), because paragraph classification honours
/// `assigned_role` directly, bypassing the heading map.
fn promote_untagged_document_title(
    heading_map: &mut [(f32, Option<u8>)],
    all_page_segments: &[Vec<SegmentData>],
) -> bool {
    /// A title is a handful of segments at most; more means a body/pull-quote tier.
    const MAX_TITLE_SEGMENTS: usize = 3;

    let Some(max_heading_font) = heading_map
        .iter()
        .filter(|(_, level)| level.is_some())
        .map(|(font, _)| *font)
        .fold(None, |acc: Option<f32>, f| Some(acc.map_or(f, |a| a.max(f))))
    else {
        return false;
    };

    let candidate = heading_map
        .iter()
        .position(|(font, level)| level.is_none() && *font > max_heading_font);
    let Some(candidate_idx) = candidate else {
        return false;
    };
    let candidate_font = heading_map[candidate_idx].0;

    let mut tier_segments = 0usize;
    let mut all_bold = true;
    let mut on_first_page = false;
    for (page_idx, page_segs) in all_page_segments.iter().enumerate() {
        for seg in page_segs {
            if seg.text.trim().is_empty() || (seg.font_size - candidate_font).abs() >= 0.05 {
                continue;
            }
            tier_segments += 1;
            all_bold &= seg.is_bold;
            on_first_page |= page_idx == 0;
        }
    }
    if tier_segments == 0 || tier_segments > MAX_TITLE_SEGMENTS || !all_bold || !on_first_page {
        return false;
    }

    tracing::debug!(
        title_font = candidate_font,
        max_heading_font,
        tier_segments,
        "structure tree: promoting untagged document-title tier to h1, demoting tagged levels"
    );
    for (font, level) in heading_map.iter_mut() {
        if let Some(l) = level {
            *level = Some((*l + 1).min(6));
        } else if (*font - candidate_font).abs() < 0.05 {
            *level = Some(1);
        }
    }
    true
}

/// Demote every structure-tree-assigned heading role by one level (capped at 6).
///
/// Companion to `promote_untagged_document_title`: paragraph classification
/// (`bridge.rs`) uses `assigned_role` directly as "the author's stated intent",
/// so the map-level demotion must be mirrored on the segments themselves.
fn demote_assigned_roles(all_page_segments: &mut [Vec<SegmentData>]) {
    for page_segs in all_page_segments.iter_mut() {
        for seg in page_segs.iter_mut() {
            if let Some(role) = seg.assigned_role {
                seg.assigned_role = Some((role + 1).min(6));
            }
        }
    }
}

/// Per-page input bundle for Stage 3 parallel processing.
///
/// Each page's data is pre-extracted before `into_par_iter` so all threads
/// receive owned, non-overlapping slices of the document's data.
struct PageInput {
    /// Index of this page in the document (0-based).
    page_index: usize,
    /// Paragraphs from the PDF structure tree, if extraction succeeded.
    struct_paragraphs: Option<Vec<PdfParagraph>>,
    /// Segments from heuristic extraction (non-empty only when `struct_paragraphs` is `None`).
    heuristic_segments: Vec<SegmentData>,
    /// Layout hints for this page, if layout detection was run.
    page_hints: Option<Vec<LayoutHint>>,
    /// Bounding boxes of tables that were successfully extracted for this page.
    table_bboxes: Vec<crate::types::BoundingBox>,
    /// Per-hint validation results from CC analysis (parallel to page_hints).
    /// Empty when layout-detection is not active.
    #[cfg(feature = "layout-detection")]
    hint_validations: Vec<super::regions::layout_validation::RegionValidation>,
    /// Actual PDF page width in points, used by layout reading-order refinement.
    #[cfg(feature = "layout-detection")]
    page_width_pts: Option<f32>,
    /// Whether this page's structure-tree paragraphs need font-size classification.
    needs_classify: bool,
    /// Y-coordinates of paragraph gaps detected from segment boundaries.
    paragraph_gap_ys: Vec<f32>,
    /// When true, paragraphs classified as `PageHeader` by the layout model are
    /// preserved rather than marked as furniture. Mirrors `ContentFilterConfig::include_headers`.
    include_headers: bool,
    /// When true, paragraphs classified as `PageFooter` by the layout model are
    /// preserved rather than marked as furniture. Mirrors `ContentFilterConfig::include_footers`.
    include_footers: bool,
}

/// Process a single page's data through Stage 3: classification, text repair,
/// layout overrides, dehyphenation, and list splitting.
///
/// This function is intentionally free of any shared mutable state so it can be
/// called from multiple threads via `rayon::par_iter`.
fn process_single_page(
    input: PageInput,
    heading_map: &[(f32, Option<u8>)],
    doc_body_font_size: Option<f32>,
) -> Vec<PdfParagraph> {
    let PageInput {
        page_index: i,
        struct_paragraphs,
        heuristic_segments,
        page_hints,
        table_bboxes,
        #[cfg(feature = "layout-detection")]
        hint_validations,
        #[cfg(feature = "layout-detection")]
        page_width_pts,
        needs_classify,
        paragraph_gap_ys,
        include_headers,
        include_footers,
    } = input;
    if let Some(mut paragraphs) = struct_paragraphs {
        apply_text_repair_to_structure_tree_paragraphs(&mut paragraphs, true);
        if needs_classify {
            tracing::debug!(
                page = i,
                "PDF structure pipeline: classifying struct tree page via font-size clustering"
            );
            classify_paragraphs(&mut paragraphs, heading_map);
        }
        merge_continuation_paragraphs(&mut paragraphs);
        synchronize_paragraph_text_metadata(&mut paragraphs);
        merge_spatial_footnote_markers(&mut paragraphs);
        if let Some(ref hints) = page_hints {
            let classification_hints = regular_layout_hints(hints);
            super::layout_classify::apply_layout_overrides(
                &mut paragraphs,
                &classification_hints,
                0.5,
                0.2,
                doc_body_font_size,
            );
            un_mark_layout_furniture_per_config(&mut paragraphs, include_headers, include_footers);
            tracing::debug!(
                page = i,
                headings = paragraphs.iter().filter(|p| p.heading_level.is_some()).count(),
                lists = paragraphs.iter().filter(|p| p.is_list_item).count(),
                furniture = paragraphs.iter().filter(|p| p.is_page_furniture).count(),
                "layout overrides applied"
            );
            retain_page_furniture_safely(&mut paragraphs);
        }
        demote_structure_annotation_headings(&mut paragraphs);
        paragraphs
    } else {
        let page_segments = heuristic_segments;
        tracing::debug!(
            page = i,
            segments = page_segments.len(),
            has_layout_hints = page_hints.is_some(),
            "process_single_page: heuristic path"
        );
        let page_segments = filter_segments_by_table_bboxes(page_segments, &table_bboxes);
        #[cfg(feature = "layout-detection")]
        let mut paragraphs = if let Some(ref hints) = page_hints {
            let wrapper_ownership = wrapper_ownership_by_hint(hints, &hint_validations);
            if crate::extractors::pdf::reading_order::has_eligible_layout_hints(hints, &wrapper_ownership) {
                process_layout_segment_groups(
                    page_segments,
                    hints,
                    &wrapper_ownership,
                    LayoutParagraphContext {
                        heading_map,
                        paragraph_gap_ys: &paragraph_gap_ys,
                        doc_body_font_size,
                        include_headers,
                        include_footers,
                        page_width_pts,
                    },
                )
            } else {
                segments_to_paragraphs(page_segments, heading_map, &paragraph_gap_ys)
            }
        } else {
            segments_to_paragraphs(page_segments, heading_map, &paragraph_gap_ys)
        };
        #[cfg(not(feature = "layout-detection"))]
        let mut paragraphs = segments_to_paragraphs(page_segments, heading_map, &paragraph_gap_ys);
        tracing::debug!(
            page = i,
            paragraphs = paragraphs.len(),
            "heuristic paragraphs classified"
        );
        #[cfg(not(feature = "layout-detection"))]
        if let Some(ref hints) = page_hints {
            let classification_hints = regular_layout_hints(hints);
            super::layout_classify::apply_layout_overrides(
                &mut paragraphs,
                &classification_hints,
                0.5,
                0.2,
                doc_body_font_size,
            );
            un_mark_layout_furniture_per_config(&mut paragraphs, include_headers, include_footers);
        }
        if page_hints.is_some() {
            tracing::debug!(
                page = i,
                headings = paragraphs.iter().filter(|p| p.heading_level.is_some()).count(),
                lists = paragraphs.iter().filter(|p| p.is_list_item).count(),
                furniture = paragraphs.iter().filter(|p| p.is_page_furniture).count(),
                "layout overrides applied"
            );
        }
        demote_structure_annotation_headings(&mut paragraphs);
        merge_spatial_footnote_markers(&mut paragraphs);
        retain_page_furniture_safely(&mut paragraphs);
        paragraphs
    }
}

fn is_wrapper_layout_hint(hint: &LayoutHint) -> bool {
    hint.class_name.is_wrapper()
}

fn regular_layout_hints(hints: &[LayoutHint]) -> Vec<LayoutHint> {
    hints
        .iter()
        .filter(|hint| !is_wrapper_layout_hint(hint))
        .cloned()
        .collect()
}

fn segments_to_paragraphs(
    segments: Vec<SegmentData>,
    heading_map: &[(f32, Option<u8>)],
    paragraph_gap_ys: &[f32],
) -> Vec<PdfParagraph> {
    let mut paragraphs = blocks_to_paragraphs(segments, heading_map, paragraph_gap_ys);
    apply_text_repair_to_structure_tree_paragraphs(&mut paragraphs, true);
    merge_continuation_paragraphs(&mut paragraphs);
    synchronize_paragraph_text_metadata(&mut paragraphs);
    paragraphs
}

#[cfg(feature = "layout-detection")]
fn wrapper_ownership_by_hint(
    hints: &[LayoutHint],
    validations: &[super::regions::layout_validation::RegionValidation],
) -> Vec<bool> {
    hints
        .iter()
        .enumerate()
        .map(|(index, hint)| {
            !is_wrapper_layout_hint(hint)
                || !matches!(
                    validations.get(index),
                    Some(super::regions::layout_validation::RegionValidation::Empty)
                )
        })
        .collect()
}

#[cfg(feature = "layout-detection")]
struct LayoutParagraphContext<'a> {
    heading_map: &'a [(f32, Option<u8>)],
    paragraph_gap_ys: &'a [f32],
    doc_body_font_size: Option<f32>,
    include_headers: bool,
    include_footers: bool,
    page_width_pts: Option<f32>,
}

#[cfg(feature = "layout-detection")]
fn process_layout_segment_groups(
    segments: Vec<SegmentData>,
    hints: &[LayoutHint],
    wrapper_ownership: &[bool],
    context: LayoutParagraphContext<'_>,
) -> Vec<PdfParagraph> {
    let no_reorder = super::layout_debug::layout_debug_flags().no_reorder;
    let groups = crate::extractors::pdf::reading_order::plan_segment_groups_by_layout(
        &segments,
        hints,
        wrapper_ownership,
        no_reorder,
        context.page_width_pts,
    );
    if matches!(groups.as_slice(), [group] if group.hint_indices.is_empty() && group.region_path.is_none()) {
        return segments_to_paragraphs(segments, context.heading_map, context.paragraph_gap_ys);
    }
    let mut slots = segments.into_iter().map(Some).collect::<Vec<_>>();
    let mut paragraphs = Vec::new();

    for group in groups {
        let region_path = group.region_path;
        let group_segments = group
            .segment_indices
            .into_iter()
            .filter_map(|index| slots.get_mut(index).and_then(Option::take))
            .collect::<Vec<_>>();
        if group_segments.is_empty() {
            continue;
        }
        let gap_ys = compute_paragraph_gap_ys(&group_segments);
        let mut group_paragraphs = segments_to_paragraphs(group_segments, context.heading_map, &gap_ys);
        let group_hints = group
            .hint_indices
            .into_iter()
            .filter_map(|index| hints.get(index).cloned())
            .collect::<Vec<_>>();
        super::layout_classify::apply_layout_overrides(
            &mut group_paragraphs,
            &group_hints,
            0.5,
            0.2,
            context.doc_body_font_size,
        );
        un_mark_layout_furniture_per_config(&mut group_paragraphs, context.include_headers, context.include_footers);
        for paragraph in &mut group_paragraphs {
            paragraph.layout_region_path = region_path;
        }
        paragraphs.extend(group_paragraphs);
    }

    let leftovers = slots.into_iter().flatten().collect::<Vec<_>>();
    if !leftovers.is_empty() {
        tracing::warn!(
            segments = leftovers.len(),
            "layout region plan omitted segments; appending an unsorted fallback group"
        );
        let gap_ys = compute_paragraph_gap_ys(&leftovers);
        paragraphs.extend(segments_to_paragraphs(leftovers, context.heading_map, &gap_ys));
    }
    paragraphs
}

/// Multiple of the median line height a whitespace band must exceed to count
/// as a paragraph break. Normal line pitch leaves well under one line height of
/// whitespace; a blank line leaves more than one.
const PARAGRAPH_GAP_HEIGHT_FACTOR: f32 = 1.5;
const INLINE_STYLE_BASELINE_TOLERANCE: f32 = 0.5;
const INLINE_STYLE_MAX_FORWARD_GAP_FONT_FACTOR: f32 = 1.0;
const INLINE_STYLE_MAX_OVERLAP_FONT_FACTOR: f32 = 0.15;

/// Detect paragraph-break y-positions from horizontal whitespace bands.
///
/// Segments are clustered into visual lines after sorting by y — stream order
/// is not positional (multi-column PDFs interleave columns, which a pairwise
/// scan misreads as phantom gaps). A break is recorded only where the band
/// between two consecutive lines is taller than
/// [`PARAGRAPH_GAP_HEIGHT_FACTOR`] × the median line height: pure blank-line
/// spacing. Bands between two monospace lines are skipped, because code
/// listings legitimately contain blank lines inside one logical block.
///
/// Without this, the heuristic path only breaks paragraphs on font/bold/list
/// changes, fusing visually separated blocks (standalone headings, display
/// formulas) into surrounding prose.
fn compute_paragraph_gap_ys(segments: &[SegmentData]) -> Vec<f32> {
    if segments.len() < 2 {
        return Vec::new();
    }

    let mut order: Vec<usize> = (0..segments.len()).collect();
    order.sort_by(|&a, &b| {
        segments[b]
            .y
            .partial_cmp(&segments[a].y)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    struct LineBand {
        top: f32,
        bottom: f32,
        height: f32,
        monospace: bool,
        anchor_y: f32,
    }
    let mut lines: Vec<LineBand> = Vec::new();
    for &i in &order {
        let seg = &segments[i];
        let tolerance = (seg.height * 0.5).max(1.0);
        match lines.last_mut() {
            Some(line) if (seg.y - line.anchor_y).abs() <= tolerance => {
                line.top = line.top.max(seg.y + seg.height);
                line.bottom = line.bottom.min(seg.y);
                line.height = line.height.max(seg.height);
                line.monospace &= seg.is_monospace;
            }
            _ => lines.push(LineBand {
                top: seg.y + seg.height,
                bottom: seg.y,
                height: seg.height,
                monospace: seg.is_monospace,
                anchor_y: seg.y,
            }),
        }
    }
    if lines.len() < 2 {
        return Vec::new();
    }

    let mut heights: Vec<f32> = lines.iter().map(|l| l.height).collect();
    heights.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let gap_threshold = heights[heights.len() / 2] * PARAGRAPH_GAP_HEIGHT_FACTOR;

    let mut gap_ys = Vec::new();
    for pair in lines.windows(2) {
        let gap = pair[0].bottom - pair[1].top;
        if gap > gap_threshold && !(pair[0].monospace && pair[1].monospace) {
            gap_ys.push((pair[0].bottom + pair[1].top) / 2.0);
        }
    }
    gap_ys
}

/// Convert a flat list of text segments into grouped paragraphs.
///
/// Groups consecutive segments by font changes, bold changes, list markers, and
/// paragraph gap positions. Each group is then classified via `finalize_paragraph`.
fn blocks_to_paragraphs(
    lines: Vec<SegmentData>,
    heading_map: &[(f32, Option<u8>)],
    paragraph_gap_ys: &[f32],
) -> Vec<PdfParagraph> {
    if lines.is_empty() {
        return Vec::new();
    }

    let gap_info = super::classify::precompute_gap_info(heading_map);

    let mut paragraphs: Vec<PdfParagraph> = Vec::new();
    let mut current_lines: Vec<&SegmentData> = Vec::new();
    let mut current_is_single_visual_line = true;

    for (line_idx, line) in lines.iter().enumerate() {
        let should_break = if current_lines.is_empty() {
            false
        } else {
            let prev = current_lines.last().unwrap();
            let font_change = (line.font_size - prev.font_size).abs() > 1.5;
            let role_change = line.assigned_role != prev.assigned_role;
            let bold_change =
                line.is_bold != prev.is_bold && !is_inline_style_transition(current_is_single_visual_line, prev, line);
            let starts_new_line = (line.baseline_y - prev.baseline_y).abs() > INLINE_STYLE_BASELINE_TOLERANCE;
            let has_same_line_follower = lines
                .get(line_idx + 1)
                .is_some_and(|next| (next.baseline_y - line.baseline_y).abs() <= INLINE_STYLE_BASELINE_TOLERANCE);
            let is_list = starts_new_line
                && (looks_like_list_item(&line.text) || (has_same_line_follower && is_bare_list_marker(&line.text)));
            let crossed_gap = paragraph_gap_ys.iter().any(|&gap_y| {
                let (upper, lower) = if prev.baseline_y > line.baseline_y {
                    (prev.baseline_y, line.baseline_y)
                } else {
                    (line.baseline_y, prev.baseline_y)
                };
                gap_y < upper && gap_y > lower
            });
            font_change || role_change || bold_change || is_list || crossed_gap
        };

        if should_break && !current_lines.is_empty() {
            if let Some(para) = finalize_paragraph(&current_lines, heading_map, &gap_info) {
                paragraphs.push(para);
            }
            current_lines.clear();
            current_is_single_visual_line = true;
        }
        if let Some(first) = current_lines.first() {
            current_is_single_visual_line &=
                (line.baseline_y - first.baseline_y).abs() <= INLINE_STYLE_BASELINE_TOLERANCE;
        }
        current_lines.push(line);
    }

    if !current_lines.is_empty()
        && let Some(para) = finalize_paragraph(&current_lines, heading_map, &gap_info)
    {
        paragraphs.push(para);
    }

    tracing::debug!(
        input_lines = lines.len(),
        output_paragraphs = paragraphs.len(),
        headings = paragraphs.iter().filter(|p| p.heading_level.is_some()).count(),
        lists = paragraphs.iter().filter(|p| p.is_list_item).count(),
        "blocks_to_paragraphs complete"
    );

    paragraphs
}

/// Whether a style transition is an inline run on the same visual line.
///
/// PDF glyph runs can overlap slightly because of font metrics. Larger
/// overlaps, reverse ordering, and wide gaps remain structural boundaries.
fn is_inline_style_transition(current_is_single_visual_line: bool, previous: &SegmentData, next: &SegmentData) -> bool {
    if !current_is_single_visual_line
        || previous.is_monospace
        || next.is_monospace
        || previous.assigned_role != next.assigned_role
    {
        return false;
    }
    if !previous.font_size.is_finite()
        || !next.font_size.is_finite()
        || previous.font_size <= 0.0
        || next.font_size <= 0.0
        || !previous.baseline_y.is_finite()
        || !next.baseline_y.is_finite()
        || !previous.x.is_finite()
        || !next.x.is_finite()
        || !previous.width.is_finite()
        || !next.width.is_finite()
        || previous.width < 0.0
        || next.width < 0.0
        || next.x < previous.x
    {
        return false;
    }
    if (next.baseline_y - previous.baseline_y).abs() > INLINE_STYLE_BASELINE_TOLERANCE {
        return false;
    }

    let font_size = previous.font_size.max(next.font_size);
    let horizontal_gap = next.x - (previous.x + previous.width);
    horizontal_gap >= -(font_size * INLINE_STYLE_MAX_OVERLAP_FONT_FACTOR)
        && horizontal_gap <= font_size * INLINE_STYLE_MAX_FORWARD_GAP_FONT_FACTOR
}

/// Reconstruct PdfLine objects from a flat list of SegmentData, grouping by baseline_y.
///
/// This preserves inline formatting information (is_bold, is_italic, is_monospace)
/// at the segment level so that the assembly layer can emit properly annotated markdown
/// with bold/italic emphasis.
fn reconstruct_pdf_lines(segments: &[&SegmentData]) -> Vec<super::types::PdfLine> {
    if segments.is_empty() {
        return Vec::new();
    }

    let mut lines: Vec<super::types::PdfLine> = Vec::new();
    let mut current_baseline = segments[0].baseline_y;
    let mut current_segments: Vec<SegmentData> = Vec::new();

    for seg in segments {
        if (seg.baseline_y - current_baseline).abs() > 0.5 {
            if !current_segments.is_empty() {
                let dominant_font_size = current_segments.iter().map(|s| s.font_size).fold(0.0, |a, b| {
                    if a > 0.0 && b > a / 2.0 && b < a * 2.0 {
                        (a + b) / 2.0
                    } else {
                        a.max(b)
                    }
                });
                let is_bold = current_segments.iter().filter(|s| s.is_bold).count() > current_segments.len() / 2;
                let is_monospace = current_segments.iter().all(|s| s.is_monospace);
                lines.push(super::types::PdfLine {
                    segments: current_segments.clone(),
                    baseline_y: current_baseline,
                    dominant_font_size,
                    is_bold,
                    is_monospace,
                });
            }
            current_baseline = seg.baseline_y;
            current_segments.clear();
        }
        current_segments.push((*seg).clone());
    }

    if !current_segments.is_empty() {
        let dominant_font_size = current_segments.iter().map(|s| s.font_size).fold(0.0, |a, b| {
            if a > 0.0 && b > a / 2.0 && b < a * 2.0 {
                (a + b) / 2.0
            } else {
                a.max(b)
            }
        });
        let is_bold = current_segments.iter().filter(|s| s.is_bold).count() > current_segments.len() / 2;
        let is_monospace = current_segments.iter().all(|s| s.is_monospace);
        lines.push(super::types::PdfLine {
            segments: current_segments,
            baseline_y: current_baseline,
            dominant_font_size,
            is_bold,
            is_monospace,
        });
    }

    lines
}

/// Build a PdfParagraph from a group of consecutive lines with compatible font properties.
fn finalize_paragraph(
    lines: &[&SegmentData],
    heading_map: &[(f32, Option<u8>)],
    gap_info: &super::classify::GapInfo,
) -> Option<PdfParagraph> {
    if lines.is_empty() {
        return None;
    }

    let text: String = lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>().join("\n");

    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }

    let first = lines[0];
    let word_count = trimmed.split_whitespace().count();
    let is_bold = lines.iter().filter(|l| l.is_bold).count() > lines.len() / 2;
    let has_mixed_inline_styles = lines
        .iter()
        .skip(1)
        .any(|line| line.is_bold != first.is_bold || line.is_italic != first.is_italic);

    let reconstructed_lines = reconstruct_pdf_lines(lines);
    let starts_with_split_list_marker = lines.get(1).is_some_and(|body| {
        is_bare_list_marker(&first.text)
            && (body.baseline_y - first.baseline_y).abs() <= INLINE_STYLE_BASELINE_TOLERANCE
            && !body.text.trim().is_empty()
    });
    let is_list_candidate = looks_like_list_item(trimmed) || starts_with_split_list_marker;

    let structure_tree_role = {
        let role_counts: std::collections::HashMap<u8, usize> =
            lines
                .iter()
                .filter_map(|l| l.assigned_role)
                .fold(std::collections::HashMap::new(), |mut acc, level| {
                    *acc.entry(level).or_default() += 1;
                    acc
                });
        role_counts
            .into_iter()
            .max_by_key(|(_, count)| *count)
            .map(|(level, _)| level)
    };
    if let Some(level) = structure_tree_role {
        let para_text = trimmed.to_string();
        let word_count = PdfParagraph::compute_word_count(&para_text, &reconstructed_lines);
        return Some(PdfParagraph {
            text: if has_mixed_inline_styles {
                String::new()
            } else {
                para_text
            },
            lines: reconstructed_lines,
            dominant_font_size: first.font_size,
            heading_level: Some(level),
            is_bold,
            is_list_item: is_list_candidate,
            is_code_block: first.is_monospace && lines.len() > 1,
            is_formula: false,
            is_page_furniture: false,
            layout_class: None,
            layout_region_path: None,
            caption_for: None,
            block_bbox: None,
            word_count,
        });
    }

    let page_number_like = word_count <= 10 && is_page_number_pattern(trimmed);

    let mut heading_level = super::classify::find_heading_level(first.font_size, heading_map, gap_info);
    if heading_level.is_some()
        && (word_count > 20 || super::layout_classify::is_separator_text(trimmed) || page_number_like)
    {
        heading_level = None;
    }

    let body_font_size = heading_map
        .iter()
        .find(|(_, level)| level.is_none())
        .map(|(centroid, _)| *centroid)
        .unwrap_or(0.0);

    // A bold, short, single-line paragraph is only a heading candidate when
    // its font is also meaningfully larger than the document's body font
    // (the same ratio/gap the font-size clustering path already requires via
    // `assign_heading_levels_smart`). Without this check, any bold one-word
    // line — including body-sized emphasis, or a stray oversized glyph from
    // a font-metric artifact — gets promoted regardless of scale, which is
    // exactly the pattern that over-promoted "Big"/"Text" in a 3-paragraph
    // document with no real headings. ~keep
    let clears_bold_font_gate = body_font_size > 0.0
        && first.font_size >= body_font_size * super::constants::MIN_HEADING_FONT_RATIO
        && first.font_size >= body_font_size + super::constants::MIN_HEADING_FONT_GAP;

    if heading_level.is_none()
        && is_bold
        && clears_bold_font_gate
        && (1..=8).contains(&word_count)
        && lines.len() == 1
        && !trimmed.ends_with('.')
        && !trimmed.ends_with(':')
        && !trimmed.ends_with(',')
        && !trimmed.ends_with(';')
        && !trimmed.contains('@')
        && !trimmed.contains('(')
        && !trimmed.contains(',')
        && trimmed
            .chars()
            .next()
            .is_some_and(|c| c.is_uppercase() || c.is_ascii_digit())
        && !super::layout_classify::is_separator_text(trimmed)
        && !super::regions::looks_like_figure_label(trimmed)
    {
        heading_level = Some(2);
    }

    if heading_level.is_none() {
        let min_heading_threshold = body_font_size * super::constants::MIN_HEADING_FONT_RATIO;
        if body_font_size > 0.0
            && first.font_size >= min_heading_threshold
            && first.font_size > body_font_size + 0.5
            && word_count <= super::constants::MAX_BOLD_HEADING_WORD_COUNT
            && lines.len() <= 2
            && !trimmed.ends_with(':')
            && !trimmed.contains('@')
            && (super::classify::is_section_pattern(trimmed) || is_structural_heading_word(trimmed))
            && !super::layout_classify::is_separator_text(trimmed)
            && !super::regions::looks_like_figure_label(trimmed)
            && !is_list_candidate
            && !page_number_like
        {
            heading_level = Some(2);
        }
    }

    let is_list_item = heading_level.is_none() && is_list_candidate;
    let is_code_block =
        heading_level.is_none() && !is_list_item && lines.iter().all(|l| l.is_monospace) && lines.len() >= 2;

    let is_page_furniture = heading_level.is_none()
        && !is_list_item
        && !is_code_block
        && word_count <= 10
        && is_page_number_pattern(trimmed);

    tracing::debug!(
        font_size = first.font_size,
        is_bold,
        word_count,
        heading_level = ?heading_level,
        is_list_item,
        is_code_block,
        is_page_furniture,
        text_preview = %&trimmed.chars().take(60).collect::<String>(),
        "classified paragraph"
    );

    let para_text = trimmed.to_string();
    let word_count = PdfParagraph::compute_word_count(&para_text, &reconstructed_lines);

    Some(PdfParagraph {
        text: if has_mixed_inline_styles {
            String::new()
        } else {
            para_text
        },
        lines: reconstructed_lines,
        dominant_font_size: first.font_size,
        heading_level,
        is_bold,
        is_list_item,
        is_code_block,
        is_formula: false,
        is_page_furniture,
        layout_class: None,
        layout_region_path: None,
        caption_for: None,
        block_bbox: Some({
            let left = lines.iter().map(|l| l.x).fold(f32::MAX, f32::min);
            let bottom = lines.iter().map(|l| l.baseline_y).fold(f32::MAX, f32::min);
            let right = lines.iter().map(|l| l.x + l.width).fold(f32::MIN, f32::max);
            let top = lines.iter().map(|l| l.baseline_y + l.height).fold(f32::MIN, f32::max);
            (left, bottom, right, top)
        }),
        word_count,
    })
}

/// Check if text is ENTIRELY a list marker with no item text after it.
///
/// Word processors often emit list numbering as its own text run, so the
/// marker ("1.", "a)", "(2)", "•") and the item body arrive as separate
/// spans on the same line. `looks_like_list_item` rejects those markers
/// because it requires trailing text; this predicate accepts them.
fn is_bare_list_marker(text: &str) -> bool {
    let t = text.trim();
    if t.is_empty() || t.chars().count() > 5 {
        return false;
    }
    if matches!(t, "•" | "·" | "◦" | "▪" | "–" | "—" | "-" | "*") {
        return true;
    }
    super::list_marker::parse_ordered_list_marker(t).is_some_and(|marker| !marker.has_content)
}

/// Check if text starts with a common list marker.
fn looks_like_list_item(text: &str) -> bool {
    let t = text.trim_start();

    if t.starts_with('•') || t.starts_with('·') || t.starts_with('◦') || t.starts_with('▪') {
        return true;
    }

    if let Some(rest) = t.strip_prefix('–').or_else(|| t.strip_prefix('—')) {
        if !rest.starts_with(' ') && !rest.starts_with('\t') {
            return false;
        }
        let body = rest.trim_start_matches([' ', '\t']);
        return !body.is_empty() && !body.starts_with('\r') && !body.starts_with('\n');
    }

    if let Some(rest) = t.strip_prefix("- ") {
        return rest.chars().next().is_some_and(|c| c.is_alphabetic());
    }

    if super::classify::is_numbered_section_heading(t) {
        return false;
    }
    let Some(marker) = super::list_marker::parse_ordered_list_marker(t) else {
        return false;
    };
    marker.has_content
        && marker.has_separator
        && !is_probable_author_byline(t)
        && t.get(marker.content_start..)
            .and_then(|content| content.chars().next())
            .is_some_and(char::is_alphabetic)
}

/// Whether a single-capital marker is more likely the first author initial.
///
/// The comma plus a second compact initial or journal-style slash supplies
/// the contextual evidence; a standalone `A. First item` remains a list.
pub(super) fn is_probable_author_byline(text: &str) -> bool {
    let mut chars = text.chars();
    if !chars.next().is_some_and(|c| c.is_ascii_uppercase()) || chars.next() != Some('.') {
        return false;
    }
    let remainder = chars.as_str().trim_start();
    let Some((surname, remainder)) = remainder.split_once(char::is_whitespace) else {
        return false;
    };
    surname.ends_with(',') && starts_with_author_initial_or_slash(remainder.trim_start())
}

fn starts_with_author_initial_or_slash(text: &str) -> bool {
    if text.starts_with('/') {
        return true;
    }
    let mut chars = text.chars().peekable();
    let mut initials = 0;
    while chars.peek().is_some_and(|c| c.is_ascii_uppercase()) {
        chars.next();
        if chars.next() != Some('.') {
            return false;
        }
        initials += 1;
    }
    initials > 0 && chars.peek().is_some_and(|c| c.is_whitespace())
}

/// Check if text is a well-known structural heading word.
///
/// These single-word headings appear frequently in academic papers and reports
/// and are reliable heading indicators when combined with a larger-than-body font.
fn is_structural_heading_word(text: &str) -> bool {
    let t = text.trim();
    matches!(
        t,
        "Abstract"
            | "References"
            | "Appendix"
            | "Acknowledgments"
            | "Acknowledgements"
            | "Conclusion"
            | "Conclusions"
            | "Bibliography"
            | "Contents"
            | "Index"
            | "Glossary"
            | "Summary"
            | "Discussion"
            | "Methods"
            | "Results"
            | "Methodology"
    )
}

/// Check if text matches common page number patterns.
///
/// Detects standalone page numbers, "Page X", "Page X of Y", Roman numerals,
/// and similar patterns that appear as page furniture.
fn is_page_number_pattern(text: &str) -> bool {
    let t = text.trim();
    if t.is_empty() {
        return false;
    }
    if t.chars().all(|c| c.is_ascii_digit()) && t.len() <= 4 {
        return true;
    }
    let lower = t.to_lowercase();
    if lower.starts_with("page ") {
        return true;
    }
    if (t.starts_with("- ") || t.starts_with("– ")) && (t.ends_with(" -") || t.ends_with(" –")) {
        let inner = t
            .trim_start_matches("- ")
            .trim_start_matches("– ")
            .trim_end_matches(" -")
            .trim_end_matches(" –")
            .trim();
        if inner.chars().all(|c| c.is_ascii_digit()) && inner.len() <= 4 {
            return true;
        }
    }
    if t.len() <= 5 && t.chars().all(|c| matches!(c, 'i' | 'v' | 'x' | 'I' | 'V' | 'X')) {
        return true;
    }
    false
}

/// Build a structured `InternalDocument` from pre-extracted per-page segments.
///
/// This is the oxide-backend entry point. It accepts segments already extracted
/// via `oxide::hierarchy::extract_all_segments` and runs the same font-clustering,
/// heading-classification, paragraph-assembly, and post-processing stages without
/// requiring a PDF document.
///
/// Image positions can be supplied to insert image placeholders into the document.
/// Layout hints (from RT-DETR layout detection) are optional; when present they
/// drive furniture marking, heading overrides, and table region detection.
///
/// Returns the assembled `InternalDocument`.
pub(crate) struct SegmentStructureConfig<'a> {
    pub k_clusters: usize,
    pub tables: &'a [crate::types::Table],
    pub outline_entries: &'a [PdfOutlineEntry],
    pub strip_repeating_text: bool,
    pub include_headers: bool,
    pub include_footers: bool,
    pub used_structure_tree: bool,
    pub image_positions: &'a [(u32, u32)],
    pub images: Option<&'a [crate::types::ExtractedImage]>,
    pub inject_placeholders: bool,
    pub layout_hints: Option<&'a [Vec<LayoutHint>]>,
    pub allow_single_column: bool,
    pub cancel_token: Option<&'a crate::cancellation::CancellationToken>,
    #[cfg(feature = "layout-detection")]
    pub layout_images: Option<&'a [image::RgbImage]>,
    #[cfg(feature = "layout-detection")]
    pub layout_results: Option<&'a [super::types::PageLayoutResult]>,
    #[cfg(feature = "layout-detection")]
    pub table_model: crate::core::config::layout::TableModel,
    #[cfg(feature = "layout-detection")]
    pub table_overlap_preference: crate::core::config::layout::TableOverlapPreference,
    #[cfg(feature = "layout-detection")]
    pub acceleration: Option<&'a crate::core::config::acceleration::AccelerationConfig>,
    #[cfg(feature = "layout-detection")]
    pub session_thread_budget: usize,
}

#[cfg(feature = "layout-detection")]
fn slanet_variant_for_table_model(table_model: crate::core::config::layout::TableModel) -> Option<&'static str> {
    use crate::core::config::layout::TableModel;

    match table_model {
        TableModel::SlanetWired | TableModel::SlanetAuto => Some("slanet_wired"),
        TableModel::SlanetWireless => Some("slanet_wireless"),
        TableModel::SlanetPlus => Some("slanet_plus"),
        TableModel::Tatr | TableModel::Disabled => None,
    }
}

pub(crate) fn extract_document_structure_from_segments(
    mut all_page_segments: Vec<Vec<SegmentData>>,
    config: SegmentStructureConfig<'_>,
) -> Result<crate::types::internal::InternalDocument> {
    let SegmentStructureConfig {
        k_clusters,
        tables,
        outline_entries,
        strip_repeating_text,
        include_headers,
        include_footers,
        used_structure_tree,
        image_positions,
        images,
        inject_placeholders,
        layout_hints,
        allow_single_column,
        cancel_token,
        #[cfg(feature = "layout-detection")]
        layout_images,
        #[cfg(feature = "layout-detection")]
        layout_results,
        #[cfg(feature = "layout-detection")]
        table_model,
        #[cfg(feature = "layout-detection")]
        table_overlap_preference,
        #[cfg(feature = "layout-detection")]
        acceleration,
        #[cfg(feature = "layout-detection")]
        session_thread_budget,
    } = config;
    let page_count = all_page_segments.len();
    tracing::debug!(
        page_count,
        used_structure_tree,
        "oxide structure pipeline: starting from pre-extracted segments"
    );

    let struct_tree_results: Vec<Option<Vec<PdfParagraph>>> = vec![None; page_count];
    let heuristic_pages: Vec<usize> = (0..page_count).collect();

    let (heading_map, doc_body_font_size) = if used_structure_tree {
        let mut heading_map = build_heading_map_from_assigned_roles(&all_page_segments);
        if !suppress_all_heading_roles_when_sparse_and_untrusted(&mut heading_map, &mut all_page_segments)
            && promote_untagged_document_title(&mut heading_map, &all_page_segments)
        {
            demote_assigned_roles(&mut all_page_segments);
        }
        let doc_body_font_size: Option<f32> = heading_map
            .iter()
            .find(|(_, level)| level.is_none())
            .map(|(size, _)| *size);
        tracing::debug!(
            heading_map_len = heading_map.len(),
            "oxide structure pipeline: heading map from structure tree"
        );
        (heading_map, doc_body_font_size)
    } else {
        let (heading_map, _struct_tree_needs_classify) =
            build_heading_map(&all_page_segments, &struct_tree_results, &heuristic_pages, k_clusters)?;
        let doc_body_font_size: Option<f32> = heading_map
            .iter()
            .find(|(_, level)| level.is_none())
            .map(|(size, _)| *size);
        (heading_map, doc_body_font_size)
    };

    let page_heights: Vec<f32> = all_page_segments
        .iter()
        .map(|segs| segs.iter().map(|s| s.y + s.height).fold(0.0_f32, f32::max).max(792.0))
        .collect();

    let mut layout_tables: Vec<crate::types::Table> = Vec::new();
    if let Some(hints_pages) = layout_hints {
        struct TablePageData {
            page_idx: usize,
            words: Vec<crate::pdf::table_reconstruct::HocrWord>,
            page_height: f32,
        }
        let mut table_pages: Vec<TablePageData> = Vec::new();

        #[allow(clippy::needless_range_loop)]
        for page_idx in 0..page_count {
            if cancel_token.is_some_and(|t| t.is_cancelled()) {
                tracing::debug!(page_idx, "oxide structure pipeline: cancelled during table page prep");
                break;
            }
            let Some(hints) = hints_pages.get(page_idx) else {
                continue;
            };
            if !hints
                .iter()
                .any(|h| h.class_name == super::types::LayoutHintClass::Table)
            {
                continue;
            }
            #[cfg(feature = "layout-detection")]
            let page_height = layout_results
                .and_then(|results| results.get(page_idx))
                .map(|pr| pr.page_height_pts)
                .unwrap_or(page_heights[page_idx]);
            #[cfg(not(feature = "layout-detection"))]
            let page_height = page_heights[page_idx];
            let words = crate::pdf::table_reconstruct::segments_to_words(&all_page_segments[page_idx], page_height);
            if words.is_empty() {
                tracing::trace!(
                    page = page_idx,
                    "oxide layout table extraction: no words from segments, skipping"
                );
                continue;
            }
            tracing::trace!(
                page = page_idx,
                word_count = words.len(),
                page_height,
                "oxide layout table extraction: page prepared"
            );
            table_pages.push(TablePageData {
                page_idx,
                words,
                page_height,
            });
        }

        #[cfg(feature = "layout-detection")]
        {
            use crate::core::config::layout::TableModel;

            let use_model_inference = table_model != TableModel::Disabled;

            let slanet_variant = slanet_variant_for_table_model(table_model);
            let is_auto = table_model == TableModel::SlanetAuto;

            let model_name = match table_model {
                TableModel::Tatr => "TATR",
                TableModel::SlanetWired | TableModel::SlanetWireless | TableModel::SlanetPlus => "SLANeXT",
                TableModel::SlanetAuto => "SLANeXT (auto)",
                TableModel::Disabled => "disabled",
            };

            let has_table_model = if use_model_inference {
                let available = match table_model {
                    TableModel::Tatr => crate::layout::is_tatr_available(acceleration, session_thread_budget),
                    TableModel::SlanetWired
                    | TableModel::SlanetWireless
                    | TableModel::SlanetPlus
                    | TableModel::SlanetAuto => slanet_variant.is_some_and(|variant| {
                        crate::layout::is_slanet_available(variant, acceleration, session_thread_budget)
                    }),
                    TableModel::Disabled => false,
                };

                if !available && !table_pages.is_empty() {
                    return Err(crate::pdf::error::PdfError::TextExtractionFailed(format!(
                        "Layout detection found table regions but {model_name} model is not available. \
                         Ensure the ONNX model is downloaded. Tables cannot be extracted without it."
                    )));
                }
                available
            } else {
                false
            };

            if has_table_model {
                if let (Some(images @ [_, ..]), Some(results @ [_, ..])) = (layout_images, layout_results) {
                    #[cfg(not(target_arch = "wasm32"))]
                    let recognized_tables: Vec<Vec<crate::types::Table>> = table_pages
                        .iter()
                        .map(|tp| {
                            if let Some(variant) = slanet_variant {
                                let Some(mut slanet) =
                                    crate::layout::take_or_create_slanet(variant, acceleration, session_thread_budget)
                                else {
                                    tracing::warn!("SLANeXT model unavailable in worker thread");
                                    return Vec::new();
                                };

                                if let (Some(page_image), Some(page_result)) =
                                    (images.get(tp.page_idx), results.get(tp.page_idx))
                                {
                                    let hints = &hints_pages[tp.page_idx];
                                    let mut classifier_pair = if is_auto {
                                        match (
                                            crate::layout::take_or_create_table_classifier(
                                                acceleration,
                                                session_thread_budget,
                                            ),
                                            crate::layout::take_or_create_slanet(
                                                "slanet_wireless",
                                                acceleration,
                                                session_thread_budget,
                                            ),
                                        ) {
                                            (Some(classifier), Some(alternate)) => Some((classifier, alternate)),
                                            _ => None,
                                        }
                                    } else {
                                        None
                                    };
                                    let classifier_arg = classifier_pair
                                        .as_mut()
                                        .map(|(classifier, alternate)| (&mut ***classifier, &mut ***alternate));
                                    let slanet_tables = super::regions::recognize_tables_slanet(
                                        page_image,
                                        hints,
                                        &tp.words,
                                        page_result,
                                        tp.page_height,
                                        tp.page_idx,
                                        &mut slanet,
                                        classifier_arg,
                                    );
                                    if !slanet_tables.is_empty() {
                                        return slanet_tables;
                                    }
                                }

                                let hints = &hints_pages[tp.page_idx];
                                super::regions::extract_tables_from_layout_hints(
                                    &tp.words,
                                    hints,
                                    tp.page_idx,
                                    tp.page_height,
                                    0.5,
                                    allow_single_column,
                                )
                            } else {
                                let Some(mut tatr) =
                                    crate::layout::take_or_create_tatr(acceleration, session_thread_budget)
                                else {
                                    tracing::warn!("TATR model unavailable in worker thread");
                                    return Vec::new();
                                };

                                if let (Some(page_image), Some(page_result)) =
                                    (images.get(tp.page_idx), results.get(tp.page_idx))
                                {
                                    let hints = &hints_pages[tp.page_idx];
                                    let tatr_tables = super::regions::recognize_tables_for_native_page(
                                        page_image,
                                        hints,
                                        &tp.words,
                                        page_result,
                                        tp.page_height,
                                        super::regions::NativeTatrRecognitionOptions {
                                            page_index: tp.page_idx,
                                            allow_single_column,
                                        },
                                        &mut tatr,
                                    );
                                    if !tatr_tables.is_empty() {
                                        return tatr_tables;
                                    }
                                }

                                let hints = &hints_pages[tp.page_idx];
                                super::regions::extract_tables_from_layout_hints(
                                    &tp.words,
                                    hints,
                                    tp.page_idx,
                                    tp.page_height,
                                    0.5,
                                    allow_single_column,
                                )
                            }
                        })
                        .collect();
                    #[cfg(target_arch = "wasm32")]
                    let recognized_tables: Vec<Vec<crate::types::Table>> = table_pages
                        .iter()
                        .map(|tp| {
                            if let (Some(page_image), Some(page_result)) =
                                (images.get(tp.page_idx), results.get(tp.page_idx))
                            {
                                let hints = &hints_pages[tp.page_idx];
                                let Some(mut tatr) =
                                    crate::layout::take_or_create_tatr(acceleration, session_thread_budget)
                                else {
                                    return Vec::new();
                                };
                                let tatr_tables = super::regions::recognize_tables_for_native_page(
                                    page_image,
                                    hints,
                                    &tp.words,
                                    page_result,
                                    tp.page_height,
                                    super::regions::NativeTatrRecognitionOptions {
                                        page_index: tp.page_idx,
                                        allow_single_column,
                                    },
                                    &mut tatr,
                                );
                                if !tatr_tables.is_empty() {
                                    return tatr_tables;
                                }
                                super::regions::extract_tables_from_layout_hints(
                                    &tp.words,
                                    hints,
                                    tp.page_idx,
                                    tp.page_height,
                                    0.5,
                                    allow_single_column,
                                )
                            } else {
                                Vec::new()
                            }
                        })
                        .collect();
                    layout_tables.extend(recognized_tables.into_iter().flatten());
                } else {
                    for tp in &table_pages {
                        if cancel_token.is_some_and(|t| t.is_cancelled()) {
                            tracing::debug!("oxide structure pipeline: cancelled during heuristic table extraction");
                            break;
                        }
                        let hints = &hints_pages[tp.page_idx];
                        layout_tables.extend(super::regions::extract_tables_from_layout_hints(
                            &tp.words,
                            hints,
                            tp.page_idx,
                            tp.page_height,
                            0.5,
                            allow_single_column,
                        ));
                    }
                }
            } else {
                for tp in &table_pages {
                    if cancel_token.is_some_and(|t| t.is_cancelled()) {
                        tracing::debug!("oxide structure pipeline: cancelled during heuristic table extraction");
                        break;
                    }
                    let hints = &hints_pages[tp.page_idx];
                    layout_tables.extend(super::regions::extract_tables_from_layout_hints(
                        &tp.words,
                        hints,
                        tp.page_idx,
                        tp.page_height,
                        0.5,
                        allow_single_column,
                    ));
                }
            }
        }

        #[cfg(not(feature = "layout-detection"))]
        for tp in &table_pages {
            if cancel_token.is_some_and(|t| t.is_cancelled()) {
                tracing::debug!("oxide structure pipeline: cancelled during heuristic table extraction");
                break;
            }
            let hints = &hints_pages[tp.page_idx];
            layout_tables.extend(super::regions::extract_tables_from_layout_hints(
                &tp.words,
                hints,
                tp.page_idx,
                tp.page_height,
                0.5,
                allow_single_column,
            ));
        }
    }

    tracing::debug!(
        layout_tables_found = layout_tables.len(),
        "oxide layout table extraction complete"
    );

    #[cfg(feature = "layout-detection")]
    let overlap_preference = table_overlap_preference;
    #[cfg(not(feature = "layout-detection"))]
    let overlap_preference = crate::core::config::layout::TableOverlapPreference::Content;
    let stitched_native_tables = stitch_fragmented_tables(tables.to_vec(), &all_page_segments);
    let emitted_tables = prepare_emitted_tables(&stitched_native_tables, layout_tables, overlap_preference);

    let extracted_table_bboxes_by_page = table_bboxes_by_page(&emitted_tables);
    tracing::debug!(
        native_tables = tables.len(),
        emitted_tables = emitted_tables.len(),
        pages_with_bboxes = extracted_table_bboxes_by_page.len(),
        "oxide table bbox suppression map built"
    );

    #[cfg(feature = "layout-detection")]
    let validations_by_page: ahash::AHashMap<usize, Vec<super::regions::layout_validation::RegionValidation>> = {
        let mut map = ahash::AHashMap::new();
        if let (Some(images), Some(results), Some(hints_pages)) = (layout_images, layout_results, layout_hints) {
            for page_idx in 0..page_count {
                if let (Some(img), Some(res), Some(hints)) =
                    (images.get(page_idx), results.get(page_idx), hints_pages.get(page_idx))
                {
                    let validations = super::regions::layout_validation::validate_page_regions(img, hints, res);
                    if validations.contains(&super::regions::layout_validation::RegionValidation::Empty) {
                        tracing::debug!(
                            page = page_idx,
                            empty_count = validations
                                .iter()
                                .filter(|v| **v == super::regions::layout_validation::RegionValidation::Empty)
                                .count(),
                            "oxide layout validation: found empty regions"
                        );
                    }
                    map.insert(page_idx, validations);
                }
            }
        }
        map
    };
    let effective_layout_hints = layout_hints;
    let page_inputs: Vec<PageInput> = (0..page_count)
        .map(|i| {
            let heuristic_segments = std::mem::take(&mut all_page_segments[i]);
            let paragraph_gap_ys = compute_paragraph_gap_ys(&heuristic_segments);
            PageInput {
                page_index: i,
                struct_paragraphs: None,
                heuristic_segments,
                page_hints: effective_layout_hints.and_then(|h| h.get(i)).cloned(),
                table_bboxes: extracted_table_bboxes_by_page.get(&i).cloned().unwrap_or_default(),
                #[cfg(feature = "layout-detection")]
                hint_validations: validations_by_page.get(&i).cloned().unwrap_or_default(),
                #[cfg(feature = "layout-detection")]
                page_width_pts: layout_results
                    .and_then(|results| results.get(i))
                    .map(|result| result.page_width_pts),
                needs_classify: false,
                paragraph_gap_ys,
                include_headers,
                include_footers,
            }
        })
        .collect();

    if cancel_token.is_some_and(|t| t.is_cancelled()) {
        return Err(crate::pdf::error::PdfError::TextExtractionFailed(
            "extraction cancelled".to_string(),
        ));
    }

    #[cfg(not(target_arch = "wasm32"))]
    let mut all_page_paragraphs: Vec<Vec<PdfParagraph>> = page_inputs
        .into_par_iter()
        .map(|input| process_single_page(input, &heading_map, doc_body_font_size))
        .collect();
    #[cfg(target_arch = "wasm32")]
    let mut all_page_paragraphs: Vec<Vec<PdfParagraph>> = page_inputs
        .into_iter()
        .map(|input| process_single_page(input, &heading_map, doc_body_font_size))
        .collect();

    refine_heading_hierarchy(&mut all_page_paragraphs);
    demote_unnumbered_subsections(&mut all_page_paragraphs);
    demote_heading_runs(&mut all_page_paragraphs);
    split_colon_semicolon_run_in_lists(&mut all_page_paragraphs);

    if strip_repeating_text {
        mark_cross_page_repeating_text(&mut all_page_paragraphs, &page_heights);
        mark_cross_page_repeating_short_text(&mut all_page_paragraphs);
    }
    mark_arxiv_noise(&mut all_page_paragraphs);
    recover_headings_from_outline(&mut all_page_paragraphs, outline_entries);
    for page in &mut all_page_paragraphs {
        retain_page_furniture_safely(page);
    }
    if strip_repeating_text {
        deduplicate_paragraphs(&mut all_page_paragraphs);
    }

    let total_paragraphs: usize = all_page_paragraphs.iter().map(|p| p.len()).sum();
    tracing::debug!(
        total_paragraphs,
        heading_map_len = heading_map.len(),
        "oxide structure pipeline: paragraph extraction complete, assembling document"
    );

    let effective_image_positions = if inject_placeholders { image_positions } else { &[] };
    let mut doc = assemble_internal_document(all_page_paragraphs, &emitted_tables, images, effective_image_positions);

    for elem in &mut doc.elements {
        if elem.text.is_empty() {
            continue;
        }
        let t1 = repair_contextual_ligatures(&elem.text);
        let t2 = expand_ligatures_with_space_absorption(&t1);
        let t3 = normalize_unicode_text(&t2);
        if let Cow::Owned(normalized) = t3 {
            elem.text = normalized;
        } else if let Cow::Owned(normalized) = t2 {
            elem.text = normalized;
        } else if let Cow::Owned(normalized) = t1 {
            elem.text = normalized;
        }
    }

    tracing::debug!(
        elements = doc.elements.len(),
        "oxide structure pipeline: assembly complete"
    );

    Ok(doc)
}

/// Maximum vertical gap (PDF points) between one fragment's bottom edge and the
/// next fragment's top edge for the two to be considered the same physical
/// table split by `oxide::table`'s row-gap clustering.
const TABLE_STITCH_Y_GAP_TOLERANCE_PTS: f64 = 4.0;
/// Maximum difference in a chain's shared left/right edge for two fragments to
/// be considered the same table (rather than two unrelated tables that happen
/// to sit close together vertically).
const TABLE_STITCH_X_TOLERANCE_PTS: f64 = 6.0;
/// Bound on fragments merged into one stitched chain. Real continuation splits
/// rarely exceed a handful of fragments; this caps the (already page-scoped,
/// already `oxide::table::MAX_REGIONS_PER_PAGE`-bounded) chain walk.
const TABLE_STITCH_MAX_CHAIN_FRAGMENTS: usize = 12;
/// Bound on additional data rows the trailing-continuation recovery pass will
/// attempt to pull from raw page segments below a stitched chain's last known
/// fragment. Keeps the scan from reading arbitrarily far down the page.
const TABLE_STITCH_TRAILING_RECOVERY_MAX_ROWS: usize = 6;
/// Row-gap multiplier used to split recovered trailing words into per-entity
/// bands. Mirrors `oxide::table::cluster_words_into_vertical_regions`'s
/// `row_gap_split`; reimplemented here because that clustering helper is
/// private to the `oxide::table` module, which this pass cannot depend on.
const TABLE_STITCH_TRAILING_ROW_GAP_MULTIPLIER: f32 = 1.8;

/// Stitch table fragments that `oxide::table`'s row-gap region clustering split
/// out of one physical table back into a single table.
///
/// `oxide::table::cluster_words_into_vertical_regions` splits a page's words
/// into regions at any row-gap exceeding `median_height * 1.8`. A table whose
/// header wraps onto several lines, or whose rows are visually separated by
/// generous line spacing, can land in several such regions — each one then
/// independently goes through header/data-row post-processing, which corrupts
/// a real multi-line header (see `post_process_table_inner`'s header cap) and
/// mis-promotes a lone data row to a fake header. This pass reassembles those
/// fragments after the fact: each fragment's own rows are themselves raw
/// word-wrapped sub-lines of a single logical row (there is no reliable way to
/// tell, post hoc, which fragment "really" had a header split correctly), so
/// stitching column-merges every fragment's rows into exactly one row — the
/// topmost fragment in a chain becomes the header, the rest become data rows —
/// and then attempts to recover any trailing data rows that fell below the
/// last known fragment without ever becoming a table fragment at all (e.g.
/// because the row-gap clustering merged them into an unrelated, rejected
/// region).
///
/// Bounded to avoid quadratic blowup: fragments are grouped by page first (an
/// `O(n)` pass), and each page's fragment list is walked once after an
/// `O(m log m)` sort, with the inner chain-adjacency check bounded by
/// `TABLE_STITCH_MAX_CHAIN_FRAGMENTS`. `oxide::table::MAX_REGIONS_PER_PAGE`
/// already caps how many fragments a single page can contribute.
fn stitch_fragmented_tables(
    tables: Vec<crate::types::Table>,
    all_page_segments: &[Vec<SegmentData>],
) -> Vec<crate::types::Table> {
    let mut by_page: ahash::AHashMap<u32, Vec<crate::types::Table>> = ahash::AHashMap::new();
    let mut unbboxed = Vec::new();
    for table in tables {
        if table.bounding_box.is_some() {
            by_page.entry(table.page_number).or_default().push(table);
        } else {
            unbboxed.push(table);
        }
    }

    let mut result = unbboxed;
    let mut page_numbers: Vec<u32> = by_page.keys().copied().collect();
    page_numbers.sort_unstable();
    for page_number in page_numbers {
        if let Some(page_tables) = by_page.remove(&page_number) {
            result.extend(stitch_page_tables(page_tables, all_page_segments));
        }
    }
    result
}

/// Assign a stable, deterministic `table_id` (and, when missing, `columns`) to
/// every table in `tables`, in the given order.
///
/// Ids are sequential (`"table-1"`, `"table-2"`, ...) rather than derived from
/// randomness or wall-clock time, so the same input document always produces
/// the same ids. Must run over the final, post-dedup set of tables a document
/// will actually emit (see [`prepare_emitted_tables`]) — running it any
/// earlier, e.g. over native tables alone, would leave layout-detected tables
/// that survive dedup without an id.
///
/// Fragments of one physical table that [`stitch_page_tables`] merged into a
/// single [`crate::types::Table`] naturally share one id, since by this point
/// they are already one entry; distinct tables receive distinct ids because
/// they remain distinct entries. Cross-page continuations of one physical
/// table are not linked: [`fragments_are_stitchable`] only merges fragments on
/// the same page, so a table split across a page boundary is intentionally
/// emitted as separate `tables[]` entries with separate ids today. Sharing an
/// id across page-boundary fragments is a known possible future extension,
/// not attempted here.
fn assign_deterministic_table_ids(tables: &mut [crate::types::Table]) {
    for (index, table) in tables.iter_mut().enumerate() {
        table.table_id = Some(format!("table-{}", index + 1));
        if table.columns.is_none() {
            table.columns = table.cells.first().cloned();
        }
    }
}

/// Stitch one page's table fragments. See [`stitch_fragmented_tables`].
fn stitch_page_tables(
    mut fragments: Vec<crate::types::Table>,
    all_page_segments: &[Vec<SegmentData>],
) -> Vec<crate::types::Table> {
    fragments.sort_by(|a, b| {
        let a_top = a.bounding_box.map_or(f64::MIN, |bbox| bbox.y1);
        let b_top = b.bounding_box.map_or(f64::MIN, |bbox| bbox.y1);
        b_top.total_cmp(&a_top)
    });

    let mut output = Vec::with_capacity(fragments.len());
    let mut index = 0;
    while index < fragments.len() {
        let mut chain_end = index + 1;
        while chain_end < fragments.len()
            && chain_end - index < TABLE_STITCH_MAX_CHAIN_FRAGMENTS
            && fragments_are_stitchable(&fragments[chain_end - 1], &fragments[chain_end])
        {
            chain_end += 1;
        }

        if chain_end - index >= 2 {
            let chain = fragments[index..chain_end].to_vec();
            output.push(merge_table_chain(chain, all_page_segments));
        } else {
            output.push(fragments[index].clone());
        }
        index = chain_end;
    }
    output
}

/// Whether `next` is the vertically-adjacent continuation of `prev` within one
/// stitch chain: same page, same column count, near-zero row gap, and matching
/// left/right edges.
fn fragments_are_stitchable(prev: &crate::types::Table, next: &crate::types::Table) -> bool {
    if prev.page_number != next.page_number {
        return false;
    }
    let (Some(a), Some(b)) = (prev.bounding_box, next.bounding_box) else {
        return false;
    };

    let prev_cols = prev.cells.first().map_or(0, Vec::len);
    let next_cols = next.cells.first().map_or(0, Vec::len);
    if prev_cols == 0 || prev_cols != next_cols {
        return false;
    }

    (a.y0 - b.y1).abs() <= TABLE_STITCH_Y_GAP_TOLERANCE_PTS
        && (a.x0 - b.x0).abs() <= TABLE_STITCH_X_TOLERANCE_PTS
        && (a.x1 - b.x1).abs() <= TABLE_STITCH_X_TOLERANCE_PTS
}

/// Merge a chain of >= 2 stitchable fragments into one table.
///
/// The topmost fragment's rows collapse into the header; every other
/// fragment's rows collapse into one data row apiece. See
/// [`stitch_fragmented_tables`] for why a whole-fragment column merge is used
/// instead of trying to re-derive a header/data split.
fn merge_table_chain(chain: Vec<crate::types::Table>, all_page_segments: &[Vec<SegmentData>]) -> crate::types::Table {
    let column_count = chain
        .iter()
        .filter_map(|table| table.cells.first())
        .map(Vec::len)
        .max()
        .unwrap_or(0);

    let page_number = chain[0].page_number;
    let mut bbox = chain
        .iter()
        .find_map(|table| table.bounding_box)
        .unwrap_or(crate::types::BoundingBox {
            x0: 0.0,
            y0: 0.0,
            x1: 0.0,
            y1: 0.0,
        });
    for table in &chain {
        if let Some(b) = table.bounding_box {
            bbox.x0 = bbox.x0.min(b.x0);
            bbox.x1 = bbox.x1.max(b.x1);
            bbox.y0 = bbox.y0.min(b.y0);
            bbox.y1 = bbox.y1.max(b.y1);
        }
    }

    let mut rows: Vec<Vec<String>> = chain
        .iter()
        .map(|table| crate::pdf::table_reconstruct::merge_rows_columnwise(&table.cells, column_count))
        .collect();

    if let Some(page_segments) = all_page_segments.get((page_number.saturating_sub(1)) as usize) {
        recover_trailing_continuation_rows(&mut rows, &mut bbox, column_count, page_segments);
    }

    let markdown = crate::pdf::table_reconstruct::table_to_markdown(&rows);
    let columns = rows.first().cloned();
    crate::types::Table {
        cells: rows,
        markdown,
        page_number,
        bounding_box: Some(bbox),
        columns,
        ..Default::default()
    }
}

/// Recover trailing data rows that never became their own table fragment.
///
/// `oxide::table`'s region clustering sometimes merges the last entities of a
/// fragmented table into a region with unrelated following content (or drops
/// them entirely when the merged region fails `post_process_table`
/// validation), so those rows leak into the document as plain paragraph text
/// instead of table data. This scans the raw page segments strictly below the
/// stitched chain's known bottom edge, within its column span, and — bounded
/// by [`TABLE_STITCH_TRAILING_RECOVERY_MAX_ROWS`] iterations — pulls one
/// row-gap-bounded entity band at a time. A band is only accepted if
/// reconstructing it independently yields the same column count as the
/// stitched table; any mismatch (e.g. the band actually contains an unrelated
/// heading below the table) stops recovery immediately rather than skipping
/// past it, since skipping risks pulling in arbitrary downstream content.
fn recover_trailing_continuation_rows(
    rows: &mut Vec<Vec<String>>,
    bbox: &mut crate::types::BoundingBox,
    column_count: usize,
    page_segments: &[SegmentData],
) {
    if column_count == 0 || page_segments.is_empty() {
        return;
    }

    let page_height = page_segments
        .iter()
        .map(|s| s.y + s.height)
        .fold(0.0_f32, f32::max)
        .max(792.0);
    let x_lo = (bbox.x0 - TABLE_STITCH_X_TOLERANCE_PTS) as f32;
    let x_hi = (bbox.x1 + TABLE_STITCH_X_TOLERANCE_PTS) as f32;
    let mut search_floor = bbox.y0 as f32;

    for _ in 0..TABLE_STITCH_TRAILING_RECOVERY_MAX_ROWS {
        let band_words: Vec<crate::pdf::table_reconstruct::HocrWord> = page_segments
            .iter()
            .filter(|seg| {
                !seg.text.trim().is_empty()
                    && seg.y + seg.height <= search_floor + TABLE_STITCH_Y_GAP_TOLERANCE_PTS as f32
                    && seg.x + seg.width >= x_lo
                    && seg.x <= x_hi
            })
            .flat_map(|seg| crate::pdf::table_reconstruct::split_segment_to_words(seg, page_height))
            .collect();
        if band_words.is_empty() {
            break;
        }

        let Some((entity_words, entity_bottom_image_y)) = take_next_entity_band(&band_words) else {
            break;
        };

        let col_gap = super::regions::tables::compute_adaptive_column_gap(&entity_words, (x_hi - x_lo).max(1.0));
        let grid = crate::pdf::table_reconstruct::reconstruct_table(&entity_words, col_gap, 0.5);
        if grid.is_empty() || grid[0].len() != column_count {
            break;
        }

        let merged_row = crate::pdf::table_reconstruct::merge_rows_columnwise(&grid, column_count);
        if merged_row.iter().all(|cell| cell.trim().is_empty()) {
            break;
        }

        let entity_bottom_pdf_y = page_height - entity_bottom_image_y as f32;
        rows.push(merged_row);
        bbox.y0 = bbox.y0.min(entity_bottom_pdf_y as f64);
        search_floor = entity_bottom_pdf_y;
    }
}

/// Take the topmost row-gap-bounded contiguous band of words from `words`
/// (which may span more than one logical entity), stopping at the first gap
/// larger than `median_height * TABLE_STITCH_TRAILING_ROW_GAP_MULTIPLIER`.
///
/// Returns the band's words and the image-coordinate bottom edge (`top +
/// height`, max across the band) of the last line included.
fn take_next_entity_band(
    words: &[crate::pdf::table_reconstruct::HocrWord],
) -> Option<(Vec<crate::pdf::table_reconstruct::HocrWord>, u32)> {
    if words.is_empty() {
        return None;
    }

    let mut heights: Vec<u32> = words.iter().map(|w| w.height).collect();
    heights.sort_unstable();
    let median_height = heights[heights.len() / 2].max(1);
    let row_gap_split = (median_height as f32 * TABLE_STITCH_TRAILING_ROW_GAP_MULTIPLIER) as u32;
    let row_tolerance = (median_height / 2).max(3);

    let mut sorted: Vec<&crate::pdf::table_reconstruct::HocrWord> = words.iter().collect();
    sorted.sort_by_key(|w| w.top);

    let mut band: Vec<crate::pdf::table_reconstruct::HocrWord> = Vec::new();
    let mut band_bottom = 0u32;
    let mut last_row_yc: Option<u32> = None;
    let mut idx = 0;
    while idx < sorted.len() {
        let row_yc = sorted[idx].top + sorted[idx].height / 2;
        let mut end = idx + 1;
        while end < sorted.len() {
            let yc = sorted[end].top + sorted[end].height / 2;
            if yc.abs_diff(row_yc) <= row_tolerance {
                end += 1;
            } else {
                break;
            }
        }

        if let Some(prev_yc) = last_row_yc
            && row_yc > prev_yc
            && row_yc - prev_yc > row_gap_split
            && !band.is_empty()
        {
            break;
        }

        for word in &sorted[idx..end] {
            band_bottom = band_bottom.max(word.top + word.height);
            band.push((*word).clone());
        }
        last_row_yc = Some(row_yc);
        idx = end;
    }

    if band.is_empty() {
        None
    } else {
        Some((band, band_bottom))
    }
}

/// Select the exact tables that final assembly will emit.
///
/// Suppression must consume this same set so a duplicate or empty table cannot
/// remove source text without contributing a corresponding table element.
fn prepare_emitted_tables(
    native_tables: &[crate::types::Table],
    layout_tables: Vec<crate::types::Table>,
    overlap_preference: crate::core::config::layout::TableOverlapPreference,
) -> Vec<crate::types::Table> {
    let mut emitted_tables: Vec<crate::types::Table> = native_tables.iter().cloned().chain(layout_tables).collect();
    emitted_tables.retain(|table| !table.markdown.trim().is_empty());
    let native_count = native_tables
        .iter()
        .filter(|table| !table.markdown.trim().is_empty())
        .count();
    deduplicate_overlapping_tables(&mut emitted_tables, native_count, overlap_preference);
    deduplicate_identical_tables(&mut emitted_tables);
    assign_deterministic_table_ids(&mut emitted_tables);
    emitted_tables
}

/// Collapse byte-identical table duplicates on the same page.
///
/// [`deduplicate_overlapping_tables`] only merges a pair when both tables carry
/// a `bounding_box`; a native/layout pair that detects the same physical table
/// but disagrees on bbox presence (e.g. native reconstruction leaves
/// `bounding_box: None` for some heuristic grids) can otherwise escape that
/// pass entirely. This pass is origin- and bbox-agnostic: any two tables on the
/// same page with byte-identical markdown are the same table by definition, so
/// the second (and any further) occurrence is dropped regardless of bbox state.
///
/// Runs in `O(n)` over the page's table count using a hash set keyed on
/// `(page_number, markdown)`.
fn deduplicate_identical_tables(tables: &mut Vec<crate::types::Table>) {
    if tables.len() < 2 {
        return;
    }

    let mut seen: ahash::AHashSet<(u32, &str)> = ahash::AHashSet::with_capacity(tables.len());
    let mut keep = vec![true; tables.len()];
    for (index, table) in tables.iter().enumerate() {
        if !seen.insert((table.page_number, table.markdown.as_str())) {
            keep[index] = false;
        }
    }

    let mut index = 0;
    tables.retain(|_| {
        let keep_this = keep[index];
        index += 1;
        keep_this
    });
}

fn table_bboxes_by_page(tables: &[crate::types::Table]) -> ahash::AHashMap<usize, Vec<crate::types::BoundingBox>> {
    let mut bboxes_by_page: ahash::AHashMap<usize, Vec<crate::types::BoundingBox>> = ahash::AHashMap::new();
    for table in tables {
        if let Some(bbox) = table.bounding_box {
            bboxes_by_page
                .entry(table.page_number.saturating_sub(1) as usize)
                .or_default()
                .push(bbox);
        }
    }
    bboxes_by_page
}

/// Filter out segments that overlap >=50% with any table bounding box.
///
/// Segments with zero area or empty text are always kept.
fn filter_segments_by_table_bboxes(
    segments: Vec<SegmentData>,
    table_bboxes: &[crate::types::BoundingBox],
) -> Vec<SegmentData> {
    if table_bboxes.is_empty() {
        return segments;
    }
    segments
        .into_iter()
        .filter(|seg| {
            let seg_area = seg.width * seg.height;
            if seg_area <= 0.0 || seg.text.trim().is_empty() {
                return true;
            }
            !table_bboxes.iter().any(|bb| {
                let inter_left = seg.x.max(bb.x0 as f32);
                let inter_right = (seg.x + seg.width).min(bb.x1 as f32);
                let inter_bottom = seg.y.max(bb.y0 as f32);
                let inter_top = (seg.y + seg.height).min(bb.y1 as f32);
                if inter_left >= inter_right || inter_bottom >= inter_top {
                    return false;
                }
                let inter_area = (inter_right - inter_left) * (inter_top - inter_bottom);
                inter_area / seg_area >= 0.5
            })
        })
        .collect()
}

/// Apply all 5 text repair passes in a single traversal over a segment's text.
///
/// Returns `Cow::Borrowed` if nothing changed, `Cow::Owned` otherwise.
fn fused_text_repairs(text: &str) -> Cow<'_, str> {
    let t1 = normalize_text_encoding(text);
    let t2 = repair_ligature_spaces(&t1);
    let t3 = expand_ligatures_with_space_absorption(&t2);
    let t3b = collapse_spaced_hyphens(&t3);
    let t4 = normalize_unicode_text(&t3b);
    let t5 = clean_duplicate_punctuation(&t4);
    match (&t1, &t2, &t3, &t3b, &t4, &t5) {
        (
            Cow::Borrowed(_),
            Cow::Borrowed(_),
            Cow::Borrowed(_),
            Cow::Borrowed(_),
            Cow::Borrowed(_),
            Cow::Borrowed(_),
        ) => Cow::Borrowed(text),
        _ => Cow::Owned(t5.into_owned()),
    }
}

/// Deduplicate tables that overlap on the same page.
///
/// When both native oxide detection and layout-based table extraction produce tables
/// for the same region, they can overlap. Tables at index `< native_count` are native;
/// the rest are layout (TATR/SLANeXT) tables. Complete side-by-side layout replacements
/// are selected atomically before ordinary pairwise arbitration. Outside those replacements,
/// `preference` decides mixed native/layout overlaps, while content weight decides same-origin
/// overlaps and [`TableOverlapPreference::Content`].
fn deduplicate_overlapping_tables(
    tables: &mut Vec<crate::types::Table>,
    native_count: usize,
    preference: crate::core::config::layout::TableOverlapPreference,
) {
    use crate::core::config::layout::TableOverlapPreference;

    if tables.len() < 2 {
        return;
    }

    let mut to_remove = ahash::AHashSet::new();
    let mut protected_layout_children = ahash::AHashSet::new();

    if preference != TableOverlapPreference::Native {
        for (parent, children) in side_by_side_layout_replacements(tables, native_count) {
            protected_layout_children.extend(children);
            to_remove.insert(parent);
        }
    }

    for i in 0..tables.len() {
        if to_remove.contains(&i) {
            continue;
        }
        for j in (i + 1)..tables.len() {
            if to_remove.contains(&j) {
                continue;
            }
            if tables[i].page_number != tables[j].page_number {
                continue;
            }
            if let (Some(a), Some(b)) = (&tables[i].bounding_box, &tables[j].bounding_box) {
                let inter_x = (a.x1.min(b.x1) - a.x0.max(b.x0)).max(0.0);
                let inter_y = (a.y1.min(b.y1) - a.y0.max(b.y0)).max(0.0);
                let intersection = inter_x * inter_y;
                let area_a = (a.x1 - a.x0) * (a.y1 - a.y0);
                let area_b = (b.x1 - b.x0) * (b.y1 - b.y0);
                let min_area = area_a.min(area_b);

                if min_area > 0.0 && intersection / min_area > 0.5 {
                    let i_is_native = i < native_count;
                    let j_is_native = j < native_count;
                    let mixed_origin = i_is_native != j_is_native;
                    let i_is_protected = protected_layout_children.contains(&i);
                    let j_is_protected = protected_layout_children.contains(&j);
                    let remove = match (i_is_protected, j_is_protected) {
                        (true, false) => Some(j),
                        (false, true) => Some(i),
                        (true, true) => {
                            let duplicate = intersection / area_a >= LAYOUT_CHILD_DUPLICATE_OVERLAP
                                && intersection / area_b >= LAYOUT_CHILD_DUPLICATE_OVERLAP;
                            duplicate.then(|| lower_content_table(tables, i, j))
                        }
                        (false, false) => Some(match preference {
                            TableOverlapPreference::Native if mixed_origin => {
                                if i_is_native {
                                    j
                                } else {
                                    i
                                }
                            }
                            TableOverlapPreference::Layout if mixed_origin => {
                                if i_is_native {
                                    i
                                } else {
                                    j
                                }
                            }
                            _ => lower_content_table(tables, i, j),
                        }),
                    };
                    let Some(remove) = remove else {
                        continue;
                    };
                    to_remove.insert(remove);
                    if remove == i {
                        break;
                    }
                }
            }
        }
    }

    let surviving_protected: Vec<_> = protected_layout_children
        .iter()
        .copied()
        .filter(|index| !to_remove.contains(index))
        .collect();
    let affected_rows: Vec<_> = surviving_protected
        .iter()
        .filter_map(|&index| tables[index].bounding_box.map(|bbox| (tables[index].page_number, bbox)))
        .collect();

    let mut idx = 0;
    tables.retain(|_| {
        let keep = !to_remove.contains(&idx);
        idx += 1;
        keep
    });
    canonicalize_affected_table_rows(tables, affected_rows);
}

/// A layout child must be almost entirely inside the native parent. This rejects
/// neighboring or weakly intersecting detections while allowing crop rounding.
const SIDE_BY_SIDE_CHILD_PARENT_OVERLAP: f64 = 0.8;
/// Both children must describe the same row band, rather than stacked tables.
const SIDE_BY_SIDE_VERTICAL_OVERLAP: f64 = 0.6;
/// The children together must account for most of the parent's horizontal span.
const SIDE_BY_SIDE_PARENT_WIDTH_COVERAGE: f64 = 0.75;
/// Every child must span most of the parent's height, rejecting shallow row fragments.
const SIDE_BY_SIDE_PARENT_HEIGHT_COVERAGE: f64 = 0.75;
/// Disjoint children must jointly explain most of the parent's total area.
const SIDE_BY_SIDE_PARENT_AREA_COVERAGE: f64 = 0.65;
/// Candidate detections that mutually cover nearly all of one another represent
/// the same layout table rather than distinct parts of a split table.
const LAYOUT_CHILD_DUPLICATE_OVERLAP: f64 = 0.9;

fn side_by_side_layout_replacements(tables: &[crate::types::Table], native_count: usize) -> Vec<(usize, Vec<usize>)> {
    (0..native_count.min(tables.len()))
        .filter_map(|parent| {
            let parent_bbox = tables[parent].bounding_box.as_ref()?;
            let children: Vec<_> = (native_count..tables.len())
                .filter(|&child| {
                    tables[child].page_number == tables[parent].page_number
                        && tables[child].bounding_box.as_ref().is_some_and(|bbox| {
                            bbox_overlap_fraction(bbox, parent_bbox) >= SIDE_BY_SIDE_CHILD_PARENT_OVERLAP
                        })
                })
                .collect();
            let children = deduplicate_layout_candidates(tables, children);
            is_side_by_side_replacement(tables, parent_bbox, &children).then_some((parent, children))
        })
        .collect()
}

fn deduplicate_layout_candidates(tables: &[crate::types::Table], mut candidates: Vec<usize>) -> Vec<usize> {
    candidates.sort_by(|&left, &right| table_left(tables, left).total_cmp(&table_left(tables, right)));
    let mut unique: Vec<usize> = Vec::with_capacity(candidates.len());
    for candidate in candidates {
        let duplicate = unique.iter().position(|&existing| {
            let candidate_bbox = tables[candidate].bounding_box.as_ref().expect("candidate has bbox");
            let existing_bbox = tables[existing].bounding_box.as_ref().expect("candidate has bbox");
            bbox_overlap_fraction(candidate_bbox, existing_bbox) >= LAYOUT_CHILD_DUPLICATE_OVERLAP
                && bbox_overlap_fraction(existing_bbox, candidate_bbox) >= LAYOUT_CHILD_DUPLICATE_OVERLAP
        });
        if let Some(position) = duplicate {
            let existing = unique[position];
            if table_content_weight(&tables[candidate]) > table_content_weight(&tables[existing]) {
                unique[position] = candidate;
            }
        } else {
            unique.push(candidate);
        }
    }
    unique.sort_by(|&left, &right| table_left(tables, left).total_cmp(&table_left(tables, right)));
    unique
}

fn lower_content_table(tables: &[crate::types::Table], left: usize, right: usize) -> usize {
    if table_content_weight(&tables[left]) >= table_content_weight(&tables[right]) {
        right
    } else {
        left
    }
}

fn table_content_weight(table: &crate::types::Table) -> usize {
    table.cells.len() + table.markdown.len()
}

fn is_side_by_side_replacement(
    tables: &[crate::types::Table],
    parent: &crate::types::BoundingBox,
    children: &[usize],
) -> bool {
    let parent_width = parent.x1 - parent.x0;
    let parent_height = parent.y1 - parent.y0;
    if children.len() < 2 || parent_width <= 0.0 || parent_height <= 0.0 {
        return false;
    }
    let horizontally_disjoint = children.windows(2).all(|pair| {
        let left = tables[pair[0]].bounding_box.as_ref().expect("candidate has bbox");
        let right = tables[pair[1]].bounding_box.as_ref().expect("candidate has bbox");
        left.x1 <= right.x0 && vertical_overlap_fraction(left, right) >= SIDE_BY_SIDE_VERTICAL_OVERLAP
    });
    if !horizontally_disjoint {
        return false;
    }
    let covers_parent_height = children.iter().all(|&index| {
        let bbox = tables[index].bounding_box.as_ref().expect("candidate has bbox");
        let covered_height = (bbox.y1.min(parent.y1) - bbox.y0.max(parent.y0)).max(0.0);
        covered_height / parent_height >= SIDE_BY_SIDE_PARENT_HEIGHT_COVERAGE
    });
    if !covers_parent_height {
        return false;
    }
    let covered_width: f64 = children
        .iter()
        .map(|&index| {
            let bbox = tables[index].bounding_box.as_ref().expect("candidate has bbox");
            (bbox.x1.min(parent.x1) - bbox.x0.max(parent.x0)).max(0.0)
        })
        .sum();
    let covered_area: f64 = children
        .iter()
        .map(|&index| bbox_intersection_area(tables[index].bounding_box.as_ref().expect("candidate has bbox"), parent))
        .sum();
    covered_width / parent_width >= SIDE_BY_SIDE_PARENT_WIDTH_COVERAGE
        && covered_area / (parent_width * parent_height) >= SIDE_BY_SIDE_PARENT_AREA_COVERAGE
}

fn bbox_overlap_fraction(child: &crate::types::BoundingBox, parent: &crate::types::BoundingBox) -> f64 {
    let child_area = (child.x1 - child.x0).max(0.0) * (child.y1 - child.y0).max(0.0);
    if child_area == 0.0 {
        return 0.0;
    }
    bbox_intersection_area(child, parent) / child_area
}

fn bbox_intersection_area(a: &crate::types::BoundingBox, b: &crate::types::BoundingBox) -> f64 {
    let intersection_width = (a.x1.min(b.x1) - a.x0.max(b.x0)).max(0.0);
    let intersection_height = (a.y1.min(b.y1) - a.y0.max(b.y0)).max(0.0);
    intersection_width * intersection_height
}

fn vertical_overlap_fraction(a: &crate::types::BoundingBox, b: &crate::types::BoundingBox) -> f64 {
    let overlap = (a.y1.min(b.y1) - a.y0.max(b.y0)).max(0.0);
    let min_height = (a.y1 - a.y0).min(b.y1 - b.y0);
    if min_height <= 0.0 { 0.0 } else { overlap / min_height }
}

fn table_left(tables: &[crate::types::Table], index: usize) -> f64 {
    tables[index]
        .bounding_box
        .as_ref()
        .map_or(f64::INFINITY, |bbox| bbox.x0)
}

fn canonicalize_affected_table_rows(
    tables: &mut Vec<crate::types::Table>,
    affected_rows: Vec<(u32, crate::types::BoundingBox)>,
) {
    let cohorts = affected_row_cohorts(affected_rows);
    if cohorts.is_empty() {
        return;
    }
    let assignments: Vec<_> = tables.iter().map(|table| table_row_cohort(table, &cohorts)).collect();
    let mut cohort_tables: Vec<Vec<_>> = (0..cohorts.len()).map(|_| Vec::new()).collect();
    let mut source: Vec<Option<_>> = std::mem::take(tables).into_iter().map(Some).collect();
    for (index, cohort) in assignments.iter().enumerate() {
        if let Some(cohort) = cohort {
            cohort_tables[*cohort].push(source[index].take().expect("assigned table is present"));
        }
    }
    for cohort in &mut cohort_tables {
        cohort.sort_by(canonical_table_order);
        cohort.reverse();
    }
    for (index, cohort) in assignments.iter().enumerate() {
        if let Some(cohort) = cohort {
            source[index] = Some(
                cohort_tables[*cohort]
                    .pop()
                    .expect("cohort table count matches assigned slots"),
            );
        }
    }
    tables.extend(source.into_iter().flatten());
}

fn affected_row_cohorts(mut rows: Vec<(u32, crate::types::BoundingBox)>) -> Vec<(u32, Vec<crate::types::BoundingBox>)> {
    let mut cohorts = Vec::new();
    while let Some((page, seed)) = rows.pop() {
        let mut cohort = vec![seed];
        let mut changed = true;
        while changed {
            changed = false;
            rows.retain(|(candidate_page, candidate)| {
                let connected = *candidate_page == page
                    && cohort
                        .iter()
                        .any(|member| vertical_overlap_fraction(member, candidate) >= SIDE_BY_SIDE_VERTICAL_OVERLAP);
                if connected {
                    cohort.push(*candidate);
                    changed = true;
                }
                !connected
            });
        }
        cohorts.push((page, cohort));
    }
    cohorts.sort_by(|(left_page, left_rows), (right_page, right_rows)| {
        left_page.cmp(right_page).then_with(|| {
            let left_y = left_rows.iter().map(|row| row.y0).fold(f64::INFINITY, f64::min);
            let right_y = right_rows.iter().map(|row| row.y0).fold(f64::INFINITY, f64::min);
            left_y.total_cmp(&right_y)
        })
    });
    cohorts
}

fn table_row_cohort(table: &crate::types::Table, cohorts: &[(u32, Vec<crate::types::BoundingBox>)]) -> Option<usize> {
    let bbox = table.bounding_box.as_ref()?;
    cohorts
        .iter()
        .enumerate()
        .filter(|(_, (page, _))| *page == table.page_number)
        .map(|(index, (_, rows))| {
            let overlap = rows
                .iter()
                .map(|row| vertical_overlap_fraction(bbox, row))
                .fold(0.0_f64, f64::max);
            (index, overlap)
        })
        .filter(|(_, overlap)| *overlap >= SIDE_BY_SIDE_VERTICAL_OVERLAP)
        .max_by(|left, right| left.1.total_cmp(&right.1).then_with(|| right.0.cmp(&left.0)))
        .map(|(index, _)| index)
}

fn canonical_table_order(left: &crate::types::Table, right: &crate::types::Table) -> std::cmp::Ordering {
    let left_bbox = left.bounding_box.as_ref();
    let right_bbox = right.bounding_box.as_ref();
    left.page_number
        .cmp(&right.page_number)
        .then_with(|| {
            left_bbox
                .map_or(f64::INFINITY, |bbox| bbox.y0)
                .total_cmp(&right_bbox.map_or(f64::INFINITY, |bbox| bbox.y0))
        })
        .then_with(|| {
            left_bbox
                .map_or(f64::INFINITY, |bbox| bbox.x0)
                .total_cmp(&right_bbox.map_or(f64::INFINITY, |bbox| bbox.x0))
        })
        .then_with(|| left.markdown.cmp(&right.markdown))
}

/// Clear `is_page_furniture` on paragraphs whose `layout_class` was set to
/// `PageHeader` or `PageFooter` by the layout model, when the caller has opted
/// in to keeping those regions via `include_headers` / `include_footers`.
///
/// This must run **before** `retain_page_furniture_safely`, which physically
/// removes furniture paragraphs via `.retain()`. Un-marking here ensures that
/// user-opted-in header/footer paragraphs survive that pass.
fn un_mark_layout_furniture_per_config(paragraphs: &mut [PdfParagraph], include_headers: bool, include_footers: bool) {
    if !include_headers && !include_footers {
        return;
    }
    for para in paragraphs.iter_mut() {
        if !para.is_page_furniture {
            continue;
        }
        match para.layout_class {
            Some(super::types::LayoutHintClass::PageHeader) if include_headers => {
                para.is_page_furniture = false;
            }
            Some(super::types::LayoutHintClass::PageFooter) if include_footers => {
                para.is_page_furniture = false;
            }
            _ => {}
        }
    }
}

const FOOTNOTE_MARKER_MAX_CHARS: usize = 3;
const FOOTNOTE_MARKER_MAX_FONT_RATIO: f32 = 0.8;
const FOOTNOTE_MARKER_MAX_GAP_EM: f32 = 0.5;
const FOOTNOTE_MARKER_MIN_VERTICAL_OVERLAP_RATIO: f32 = 0.8;
const FOOTNOTE_MARKER_MIN_RISE_EM: f32 = 0.1;
const FOOTNOTE_RUN_MIN_LENGTH: usize = 3;
const FOOTNOTE_RUN_MAX_FONT_DELTA: f32 = 0.5;
const FOOTNOTE_RUN_MAX_LEFT_DELTA_EM: f32 = 0.5;
const FOOTNOTE_RUN_MAX_VERTICAL_GAP_EM: f32 = 0.5;
const FOOTNOTE_RUN_MAX_GAP_SPREAD_EM: f32 = 0.25;

/// Rejoin a small raised footnote marker that was split from its body.
///
/// Standalone numeric markers are initially classified as page numbers. Only
/// strong same-line geometry can override that classification, so genuine page
/// numbers and numbered list items remain untouched.
fn merge_spatial_footnote_markers(paragraphs: &mut Vec<PdfParagraph>) {
    let mut merged_pairs = vec![false; paragraphs.len()];
    let mut index = 0;
    while index + 1 < paragraphs.len() {
        if !is_spatial_footnote_pair(&paragraphs[index], &paragraphs[index + 1]) {
            index += 1;
            continue;
        }

        let marker = paragraphs.remove(index);
        merged_pairs.remove(index);
        let body = &mut paragraphs[index];
        let mut lines = marker.lines;
        lines.append(&mut body.lines);
        body.lines = lines;
        body.text.clear();
        body.block_bbox = marker.block_bbox.zip(body.block_bbox).map(|(marker_bbox, body_bbox)| {
            (
                marker_bbox.0.min(body_bbox.0),
                marker_bbox.1.min(body_bbox.1),
                marker_bbox.2.max(body_bbox.2),
                marker_bbox.3.max(body_bbox.3),
            )
        });
        body.word_count = PdfParagraph::compute_word_count("", &body.lines);
        body.is_page_furniture = false;
        merged_pairs[index] = true;
        index += 1;
    }
    merge_consecutive_spatial_footnotes(paragraphs, &mut merged_pairs);
}

fn spatial_footnote_number(paragraph: &PdfParagraph) -> Option<u32> {
    if paragraph.lines.len() < 2 || paragraph.heading_level.is_some() || paragraph.is_list_item {
        return None;
    }
    let marker = paragraph.lines.first()?.segments.first()?;
    if marker.font_size > paragraph.dominant_font_size * FOOTNOTE_MARKER_MAX_FONT_RATIO {
        return None;
    }
    marker.text.trim().parse().ok()
}

fn spatial_footnote_gap(upper: &PdfParagraph, lower: &PdfParagraph) -> Option<f32> {
    let (_, upper_bottom, _, _) = upper.block_bbox?;
    let (_, _, _, lower_top) = lower.block_bbox?;
    Some(upper_bottom - lower_top)
}

fn spatial_footnotes_are_adjacent(upper: &PdfParagraph, lower: &PdfParagraph) -> bool {
    let Some(upper_number) = spatial_footnote_number(upper) else {
        return false;
    };
    let Some(lower_number) = spatial_footnote_number(lower) else {
        return false;
    };
    let Some((upper_left, _, _, _)) = upper.block_bbox else {
        return false;
    };
    let Some((lower_left, _, _, _)) = lower.block_bbox else {
        return false;
    };
    let Some(gap) = spatial_footnote_gap(upper, lower) else {
        return false;
    };
    upper_number.checked_add(1) == Some(lower_number)
        && upper.layout_region_path == lower.layout_region_path
        && (upper.dominant_font_size - lower.dominant_font_size).abs() < FOOTNOTE_RUN_MAX_FONT_DELTA
        && (upper_left - lower_left).abs() <= upper.dominant_font_size * FOOTNOTE_RUN_MAX_LEFT_DELTA_EM
        && gap >= 0.0
        && gap <= upper.dominant_font_size * FOOTNOTE_RUN_MAX_VERTICAL_GAP_EM
}

fn spatial_footnote_run_is_regular(run: &[PdfParagraph]) -> bool {
    if run.len() < FOOTNOTE_RUN_MIN_LENGTH
        || !run
            .windows(2)
            .all(|pair| spatial_footnotes_are_adjacent(&pair[0], &pair[1]))
    {
        return false;
    }
    let gaps = run
        .windows(2)
        .filter_map(|pair| spatial_footnote_gap(&pair[0], &pair[1]))
        .collect::<Vec<_>>();
    let minimum = gaps.iter().copied().fold(f32::INFINITY, f32::min);
    let maximum = gaps.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    maximum - minimum <= run[0].dominant_font_size * FOOTNOTE_RUN_MAX_GAP_SPREAD_EM
}

fn merge_spatial_footnote_run(run: Vec<PdfParagraph>) -> PdfParagraph {
    let mut iter = run.into_iter();
    let mut merged = iter.next().expect("footnote run is non-empty");
    for mut paragraph in iter {
        merged.lines.append(&mut paragraph.lines);
        merged.block_bbox = merged.block_bbox.zip(paragraph.block_bbox).map(|(left, right)| {
            (
                left.0.min(right.0),
                left.1.min(right.1),
                left.2.max(right.2),
                left.3.max(right.3),
            )
        });
    }
    merged.text.clear();
    merged.word_count = PdfParagraph::compute_word_count("", &merged.lines);
    merged
}

fn merge_consecutive_spatial_footnotes(paragraphs: &mut Vec<PdfParagraph>, merged_pairs: &mut Vec<bool>) {
    let mut start = 0;
    while start + FOOTNOTE_RUN_MIN_LENGTH <= paragraphs.len() {
        let minimum_end = start + FOOTNOTE_RUN_MIN_LENGTH;
        if !merged_pairs[start..minimum_end].iter().all(|merged| *merged)
            || !spatial_footnote_run_is_regular(&paragraphs[start..minimum_end])
        {
            start += 1;
            continue;
        }

        let mut end = minimum_end;
        while end < paragraphs.len() && merged_pairs[end] && spatial_footnote_run_is_regular(&paragraphs[start..=end]) {
            end += 1;
        }

        let merged = merge_spatial_footnote_run(paragraphs.drain(start..end).collect());
        paragraphs.insert(start, merged);
        merged_pairs.drain(start..end);
        merged_pairs.insert(start, false);
        start += 1;
    }
}

fn is_spatial_footnote_pair(marker: &PdfParagraph, body: &PdfParagraph) -> bool {
    if !marker.is_page_furniture
        || body.is_page_furniture
        || body.heading_level.is_some()
        || body.is_list_item
        || body.is_code_block
        || body.is_formula
        || body.caption_for.is_some()
        || marker.layout_region_path != body.layout_region_path
        || !is_compact_footnote_marker(&paragraph_text_raw(marker))
    {
        return false;
    }

    let Some((marker_left, marker_bottom, marker_right, marker_top)) = marker.block_bbox else {
        return false;
    };
    let Some((body_left, body_bottom, _, body_top)) = body.block_bbox else {
        return false;
    };
    let marker_height = marker_top - marker_bottom;
    let body_height = body_top - body_bottom;
    let overlap = marker_top.min(body_top) - marker_bottom.max(body_bottom);
    let minimum_height = marker_height.min(body_height);
    let horizontal_gap = body_left - marker_right;

    marker_left < body_left
        && horizontal_gap >= 0.0
        && horizontal_gap <= body.dominant_font_size * FOOTNOTE_MARKER_MAX_GAP_EM
        && marker.dominant_font_size <= body.dominant_font_size * FOOTNOTE_MARKER_MAX_FONT_RATIO
        && marker_bottom - body_bottom >= body.dominant_font_size * FOOTNOTE_MARKER_MIN_RISE_EM
        && minimum_height > 0.0
        && overlap >= minimum_height * FOOTNOTE_MARKER_MIN_VERTICAL_OVERLAP_RATIO
}

fn is_compact_footnote_marker(text: &str) -> bool {
    let marker = text.trim();
    let char_count = marker.chars().count();
    char_count > 0
        && char_count <= FOOTNOTE_MARKER_MAX_CHARS
        && (marker.chars().all(|character| character.is_ascii_digit())
            || marker
                .chars()
                .all(|character| matches!(character, '*' | '†' | '‡' | '§')))
}

/// Filter page furniture paragraphs with a safety valve.
///
/// Removes paragraphs marked as page furniture (headers/footers) by layout
/// detection. If removing ALL furniture-marked paragraphs would leave zero
/// content, the furniture markings are cleared instead — better to include
/// headers/footers than to produce empty output. This handles layout models
/// misclassifying body text as page furniture on non-standard document types
/// (e.g., legal transcripts, cover pages).
fn retain_page_furniture_safely(paragraphs: &mut Vec<PdfParagraph>) {
    let total = paragraphs.len();
    let furniture_count = paragraphs.iter().filter(|p| p.is_page_furniture).count();

    if furniture_count == 0 {
        return;
    }

    if furniture_count >= total {
        for para in paragraphs.iter_mut() {
            para.is_page_furniture = false;
        }
        return;
    }

    let total_alphanum: usize = paragraphs.iter().map(paragraph_alphanum_len).sum();

    if total_alphanum > 0 {
        let furniture_alphanum: usize = paragraphs
            .iter()
            .filter(|p| p.is_page_furniture)
            .map(paragraph_alphanum_len)
            .sum();

        if furniture_alphanum * 100 > total_alphanum * 30 {
            for para in paragraphs.iter_mut() {
                para.is_page_furniture = false;
            }
            return;
        }
    }

    const MIN_SUBSTANTIVE_CHARS: usize = 80;

    paragraphs.retain(|p| {
        if !p.is_page_furniture {
            return true;
        }
        paragraph_alphanum_len(p) > MIN_SUBSTANTIVE_CHARS
    });
}

/// Count alphanumeric characters in a paragraph's text content.
fn paragraph_alphanum_len(para: &PdfParagraph) -> usize {
    para.lines
        .iter()
        .flat_map(|line| line.segments.iter())
        .map(|seg| seg.text.bytes().filter(|b| b.is_ascii_alphanumeric()).count())
        .sum()
}

/// Dehyphenate paragraphs by rejoining words split across line boundaries.
///
/// When `has_positions` is true (heuristic extraction path), both explicit
/// trailing hyphens and implicit breaks (no hyphen, full line) are handled.
/// When false (structure tree path with x=0, width=0), only explicit trailing
/// hyphens are rejoined to avoid false positives.
fn dehyphenate_paragraphs(paragraphs: &mut [PdfParagraph], has_positions: bool) {
    for para in paragraphs.iter_mut() {
        if para.is_code_block || para.lines.len() < 2 {
            continue;
        }
        if has_positions {
            dehyphenate_paragraph_lines(para);
        } else {
            dehyphenate_hyphen_only(para);
        }
    }
}

/// High-confidence lexical compounds whose source hyphen must survive a line break.
///
/// A trailing ASCII hyphen is otherwise indistinguishable from a discretionary PDF
/// line-wrap hyphen. Exact pair matching is intentionally narrower than prefix or
/// suffix rules: it protects common compounds without suppressing repairs such as
/// `soft-` + `ware`.
const PRESERVED_LEXICAL_COMPOUNDS: &[(&str, &str)] = &[
    ("cost", "effective"),
    ("evidence", "based"),
    ("high", "level"),
    ("long", "term"),
    ("low", "level"),
    ("real", "time"),
    ("short", "term"),
    ("state", "of-the-art"),
    ("user", "defined"),
    ("well", "known"),
];

fn should_preserve_lexical_hyphen(trailing_word: &str, leading_word: &str) -> bool {
    let trim_non_lexical = |ch: char| !ch.is_alphanumeric() && ch != '-';
    let left = trailing_word.trim_matches(trim_non_lexical);
    let right = leading_word.trim_matches(trim_non_lexical);

    PRESERVED_LEXICAL_COMPOUNDS
        .iter()
        .any(|&(expected_left, expected_right)| {
            left.eq_ignore_ascii_case(expected_left) && right.eq_ignore_ascii_case(expected_right)
        })
}

/// Core dehyphenation with position-based full-line detection.
///
/// For each line boundary, checks whether the line extends close to the right
/// margin. If so, attempts to rejoin the trailing word of one line with the
/// leading word of the next.
fn dehyphenate_paragraph_lines(para: &mut PdfParagraph) {
    let max_right_edge = para
        .lines
        .iter()
        .flat_map(|l| l.segments.iter())
        .map(|s| s.x + s.width)
        .fold(0.0_f32, f32::max);

    if max_right_edge <= 0.0 {
        dehyphenate_hyphen_only(para);
        return;
    }

    let threshold = max_right_edge * FULL_LINE_FRACTION;

    let n = para.lines.len();
    for i in 0..(n - 1) {
        let trailing_right = para.lines[i].segments.last().map(|s| s.x + s.width).unwrap_or(0.0);
        if trailing_right < threshold {
            continue;
        }

        let trailing_text = match para.lines[i].segments.last() {
            Some(s) if !s.text.is_empty() => s.text.clone(),
            _ => continue,
        };
        let leading_text = match para.lines[i + 1].segments.first() {
            Some(s) if !s.text.is_empty() => s.text.clone(),
            _ => continue,
        };

        let has_trailing_hyphen = trailing_text.ends_with('-');
        if !has_trailing_hyphen {
            continue;
        }

        let leading_word = leading_text.split_whitespace().next().unwrap_or("");
        if leading_word.chars().next().is_some_and(|c| c.is_uppercase()) {
            continue;
        }

        let trailing_word = trailing_text
            .trim_end_matches('-')
            .split_whitespace()
            .last()
            .unwrap_or("");
        if trailing_word.chars().last().is_some_and(is_cjk_char) {
            continue;
        }

        let preserved_hyphen = if should_preserve_lexical_hyphen(trailing_word, leading_word) {
            "-"
        } else {
            ""
        };
        let joined_word = format!("{trailing_word}{preserved_hyphen}{leading_word}");

        if let Some(seg) = para.lines[i].segments.last_mut() {
            let text_without_word: String = seg
                .text
                .chars()
                .rev()
                .skip(trailing_word.len() + 1)
                .collect::<String>()
                .chars()
                .rev()
                .collect();
            seg.text = format!("{text_without_word}{joined_word}");
        }

        if let Some(seg) = para.lines[i + 1].segments.first_mut() {
            let after_leading_word = seg.text.trim_start_matches(leading_word).trim_start();
            seg.text = after_leading_word.to_string();
        }
    }
}

/// Hyphen-only dehyphenation (no position data required).
///
/// Only joins lines when the trailing segment ends with an explicit hyphen.
/// Used for structure tree pages where x/width may be zero.
fn dehyphenate_hyphen_only(para: &mut PdfParagraph) {
    let n = para.lines.len();
    for i in 0..(n - 1) {
        let trailing_text = match para.lines[i].segments.last() {
            Some(s) if s.text.ends_with('-') => s.text.clone(),
            _ => continue,
        };
        let leading_text = match para.lines[i + 1].segments.first() {
            Some(s) if !s.text.is_empty() => s.text.clone(),
            _ => continue,
        };

        let leading_word = leading_text.split_whitespace().next().unwrap_or("");
        if leading_word.chars().next().is_some_and(|c| c.is_uppercase()) {
            continue;
        }

        let trailing_word = trailing_text
            .trim_end_matches('-')
            .split_whitespace()
            .last()
            .unwrap_or("");
        if trailing_word.chars().last().is_some_and(is_cjk_char) {
            continue;
        }

        let preserved_hyphen = if should_preserve_lexical_hyphen(trailing_word, leading_word) {
            "-"
        } else {
            ""
        };
        let joined_word = format!("{trailing_word}{preserved_hyphen}{leading_word}");

        if let Some(seg) = para.lines[i].segments.last_mut() {
            let text_without_word: String = seg
                .text
                .chars()
                .rev()
                .skip(trailing_word.len() + 1)
                .collect::<String>()
                .chars()
                .rev()
                .collect();
            seg.text = format!("{text_without_word}{joined_word}");
        }

        if let Some(seg) = para.lines[i + 1].segments.first_mut() {
            let after_leading_word = seg.text.trim_start_matches(leading_word).trim_start();
            seg.text = after_leading_word.to_string();
        }
    }
}

/// Detect whether a set of paragraphs contains any font-size variation.
///
/// Variation is defined as any paragraph whose font size differs from the first
/// non-zero size by more than 0.5pt. Used to decide whether structure-tree pages
/// need font-size clustering for heading assignment.
fn has_font_size_variation(paragraphs: &[PdfParagraph]) -> bool {
    let mut first_size: Option<f32> = None;
    for para in paragraphs {
        let size = para.dominant_font_size;
        if size <= 0.0 {
            continue;
        }
        match first_size {
            None => first_size = Some(size),
            Some(fs) if (size - fs).abs() > 0.5 => return true,
            _ => {}
        }
    }
    false
}

/// Deduplicate paragraphs with identical text within each page.
///
/// Two-pass approach:
/// 1. Consecutive duplicates: remove back-to-back identical paragraphs
///    (catches bold/shadow rendering artifacts).
/// 2. Non-consecutive duplicates: remove body-text paragraphs whose
///    normalized text was already seen on the same page (catches table
///    content rendered as both table and body text).
///
/// Only deduplicates body text — headings, list items, code blocks,
/// formulas, and captions are preserved even if duplicated.
fn deduplicate_paragraphs(all_pages: &mut [Vec<PdfParagraph>]) {
    for page in all_pages.iter_mut() {
        if page.len() < 2 {
            continue;
        }

        let mut i = 0;
        while i + 1 < page.len() {
            let a_text = paragraph_text_normalized(&page[i]);
            let b_text = paragraph_text_normalized(&page[i + 1]);
            if page[i].layout_region_path == page[i + 1].layout_region_path && a_text.len() >= 5 && a_text == b_text {
                page.remove(i + 1);
            } else {
                i += 1;
            }
        }

        let mut seen = ahash::AHashSet::new();
        let mut to_remove = Vec::new();
        for (idx, para) in page.iter().enumerate() {
            if !is_dedup_candidate(para) {
                continue;
            }
            let text = paragraph_text_normalized(para);
            if text.len() < 15 {
                continue;
            }
            if !seen.insert((para.layout_region_path, text)) {
                to_remove.push(idx);
            }
        }

        for &idx in to_remove.iter().rev() {
            page.remove(idx);
        }
    }
}

const DEFAULT_OUTLINE_HEADING_OFFSET: i64 = 2;
const MIN_OUTLINE_CALIBRATION_ANCHORS: usize = 2;
const MIN_MARKDOWN_HEADING_LEVEL: i64 = 1;
const MAX_MARKDOWN_HEADING_LEVEL: i64 = 6;

#[derive(Debug, Clone, Copy)]
struct OutlineParagraphMatch {
    page_index: usize,
    paragraph_index: usize,
    depth: usize,
}

fn recover_headings_from_outline(all_pages: &mut [Vec<PdfParagraph>], outline_entries: &[PdfOutlineEntry]) {
    let matches = collect_unique_outline_matches(all_pages, outline_entries);
    let offset = calibrated_outline_heading_offset(all_pages, &matches);

    for matched in matches {
        let paragraph = &mut all_pages[matched.page_index][matched.paragraph_index];
        if paragraph.heading_level.is_some() || !outline_layout_allows_heading(paragraph) {
            continue;
        }
        let depth = i64::try_from(matched.depth).unwrap_or(i64::MAX);
        let level = depth
            .saturating_add(offset)
            .clamp(MIN_MARKDOWN_HEADING_LEVEL, MAX_MARKDOWN_HEADING_LEVEL);
        paragraph.heading_level = Some(level as u8);
        paragraph.is_list_item = false;
        paragraph.is_page_furniture = false;
    }
}

fn collect_unique_outline_matches(
    all_pages: &[Vec<PdfParagraph>],
    outline_entries: &[PdfOutlineEntry],
) -> Vec<OutlineParagraphMatch> {
    let mut outline_counts = ahash::AHashMap::<(usize, String), usize>::new();
    for entry in outline_entries {
        if let Some(key) = outline_match_key(entry, all_pages.len()) {
            *outline_counts.entry(key).or_default() += 1;
        }
    }
    let paragraph_matches = all_pages
        .iter()
        .map(|page| {
            let mut matches = ahash::AHashMap::<String, (usize, usize)>::new();
            for (index, paragraph) in page.iter().enumerate() {
                let title = normalize_outline_title(&paragraph_text_raw(paragraph));
                let entry = matches.entry(title).or_insert((0, index));
                entry.0 += 1;
            }
            matches
        })
        .collect::<Vec<_>>();

    outline_entries
        .iter()
        .filter_map(|entry| {
            let (page_index, title) = outline_match_key(entry, all_pages.len())?;
            if outline_counts.get(&(page_index, title.clone())) != Some(&1) {
                return None;
            }
            let &(paragraph_count, paragraph_index) = paragraph_matches[page_index].get(&title)?;
            (paragraph_count == 1).then_some(OutlineParagraphMatch {
                page_index,
                paragraph_index,
                depth: entry.depth,
            })
        })
        .collect()
}

fn outline_match_key(entry: &PdfOutlineEntry, page_count: usize) -> Option<(usize, String)> {
    let page_number = entry.page_number?;
    let page_index = usize::try_from(page_number.checked_sub(1)?).ok()?;
    let title = normalize_outline_title(&entry.title);
    (page_index < page_count && !title.is_empty()).then_some((page_index, title))
}

fn calibrated_outline_heading_offset(all_pages: &[Vec<PdfParagraph>], matches: &[OutlineParagraphMatch]) -> i64 {
    let mut counts = ahash::AHashMap::<i64, usize>::new();
    for matched in matches {
        let paragraph = &all_pages[matched.page_index][matched.paragraph_index];
        if !outline_layout_allows_heading(paragraph) {
            continue;
        }
        if let Some(level) = paragraph.heading_level {
            let depth = i64::try_from(matched.depth).unwrap_or(i64::MAX);
            *counts.entry(i64::from(level).saturating_sub(depth)).or_default() += 1;
        }
    }

    let max_count = counts.values().copied().max().unwrap_or_default();
    let mut winners = counts.into_iter().filter(|(_, count)| *count == max_count);
    let winner = winners.next();
    match (winner, winners.next(), max_count) {
        (Some((offset, _)), None, count) if count >= MIN_OUTLINE_CALIBRATION_ANCHORS => offset,
        _ => DEFAULT_OUTLINE_HEADING_OFFSET,
    }
}

fn outline_layout_allows_heading(paragraph: &PdfParagraph) -> bool {
    if paragraph.is_code_block || paragraph.is_formula || paragraph.caption_for.is_some() {
        return false;
    }
    matches!(
        paragraph.layout_class,
        None | Some(super::types::LayoutHintClass::Title)
            | Some(super::types::LayoutHintClass::SectionHeader)
            | Some(super::types::LayoutHintClass::Text)
            | Some(super::types::LayoutHintClass::Other)
    )
}

fn normalize_outline_title(text: &str) -> String {
    let text = strip_section_label(text.trim());
    let mut normalized = String::new();
    let mut pending_space = false;
    for character in text.chars().flat_map(char::to_lowercase) {
        if character.is_alphanumeric() {
            if pending_space && !normalized.is_empty() {
                normalized.push(' ');
            }
            normalized.push(character);
            pending_space = false;
        } else if !normalized.is_empty() {
            pending_space = true;
        }
    }
    normalized
}

fn strip_section_label(text: &str) -> &str {
    let Some((first, rest)) = text.split_once(char::is_whitespace) else {
        return text;
    };
    let punctuated = first
        .chars()
        .next()
        .is_some_and(|character| matches!(character, '(' | '['))
        || first
            .chars()
            .last()
            .is_some_and(|character| matches!(character, '.' | ')' | ']' | ':'));
    let core = first.trim_matches(|character| matches!(character, '(' | '[' | '.' | ')' | ']' | ':'));
    let decimal_parts = core.split('.').collect::<Vec<_>>();
    let decimal = !decimal_parts.is_empty()
        && decimal_parts
            .iter()
            .all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit()));
    let decimal_label = decimal && (punctuated || decimal_parts.len() > 1 || core.len() <= 3);
    let roman_label = punctuated
        && !core.is_empty()
        && core
            .chars()
            .all(|character| matches!(character.to_ascii_uppercase(), 'I' | 'V' | 'X' | 'L' | 'C' | 'D' | 'M'));
    let letter_label = punctuated && core.len() == 1 && core.chars().all(|character| character.is_ascii_alphabetic());

    if decimal_label || roman_label || letter_label {
        rest.trim_start()
    } else {
        text
    }
}

fn paragraph_text_raw(para: &PdfParagraph) -> String {
    if para.text.is_empty() {
        para.lines
            .iter()
            .flat_map(|line| line.segments.iter())
            .map(|segment| segment.text.as_str())
            .collect::<Vec<_>>()
            .join(" ")
    } else {
        para.text.clone()
    }
}

/// Normalize paragraph text for deduplication comparison.
///
/// Uses `para.text` when populated (heuristic path), otherwise assembles text
/// from segment data (structure tree path, used in tests).
fn paragraph_text_normalized(para: &PdfParagraph) -> String {
    paragraph_text_raw(para)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// Check if a paragraph is a candidate for non-consecutive deduplication.
fn is_dedup_candidate(p: &PdfParagraph) -> bool {
    p.heading_level.is_none()
        && !p.is_list_item
        && !p.is_code_block
        && !p.is_formula
        && !p.is_page_furniture
        && p.caption_for.is_none()
}

/// Minimum word count for the lead sentence before a run-in list's anchor
/// colon; guards against matching short labels (`"Note:"`, abbreviations)
/// that happen to be followed by a semicolon elsewhere in the text.
const RUN_IN_LIST_MIN_LEAD_WORDS: usize = 4;
/// Minimum semicolon-delimited clauses required to call a colon-introduced
/// run a "list" — a single clause is just a qualified sentence, not an
/// enumeration.
const RUN_IN_LIST_MIN_ITEMS: usize = 2;
/// Minimum word count per clause; guards against matching stray short
/// fragments (e.g. an abbreviation followed by `;`) as list items.
const RUN_IN_LIST_MIN_ITEM_WORDS: usize = 3;

/// Split a colon-introduced, semicolon-delimited "run-in" list — a prose
/// convention common in legal/contract text, e.g. "...is authorised to
/// exclude subscription rights: to exclude fractional amounts...; where the
/// new shares...;" — out of a single assembled paragraph into a lead
/// paragraph plus one list-item paragraph per clause.
///
/// These enumerations are frequently rendered with no distinguishing
/// indentation or line break from the surrounding prose (the source document
/// never used a real list, just semicolon-separated clauses within one
/// paragraph flow), so geometry-based list detection
/// (`classify::detect_indentation_based_lists`) never sees them — that pass
/// only promotes paragraphs already indented relative to the page's modal
/// left margin. This pass instead recognizes the enumeration from paragraph
/// text alone, after normal paragraph assembly, and works for both the
/// heuristic and structure-tree paragraph paths via [`paragraph_text_raw`].
///
/// xberg-io/xberg#1301.
fn split_colon_semicolon_run_in_lists(all_page_paragraphs: &mut [Vec<PdfParagraph>]) {
    for page_paragraphs in all_page_paragraphs.iter_mut() {
        let mut index = 0;
        while index < page_paragraphs.len() {
            match try_split_run_in_list(&page_paragraphs[index]) {
                Some(replacement) => {
                    let inserted = replacement.len();
                    page_paragraphs.splice(index..=index, replacement);
                    index += inserted;
                }
                None => index += 1,
            }
        }
    }
}

/// Attempt to split one paragraph into a lead paragraph plus run-in list
/// items. Returns `None` when the paragraph does not match the pattern, in
/// which case it is left untouched.
fn try_split_run_in_list(para: &PdfParagraph) -> Option<Vec<PdfParagraph>> {
    if para.heading_level.is_some()
        || para.is_list_item
        || para.is_code_block
        || para.is_formula
        || para.is_page_furniture
    {
        return None;
    }

    let normalized: String = paragraph_text_raw(para)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let colon_byte = normalized.rfind(':')?;
    let lead = normalized[..=colon_byte].trim();
    if lead.split_whitespace().count() < RUN_IN_LIST_MIN_LEAD_WORDS {
        return None;
    }

    let tail = normalized[colon_byte + 1..].trim_start();
    if tail.is_empty() {
        return None;
    }

    let items: Vec<&str> = tail
        .split_inclusive(';')
        .map(str::trim)
        .filter(|segment| !segment.is_empty())
        .collect();
    if items.len() < RUN_IN_LIST_MIN_ITEMS || !items.iter().all(|item| is_probable_run_in_list_item(item)) {
        return None;
    }

    let mut split = Vec::with_capacity(items.len() + 1);
    split.push(run_in_list_fragment(para, lead.to_string(), false));
    for item in items {
        split.push(run_in_list_fragment(para, item.to_string(), true));
    }
    Some(split)
}

/// Whether one semicolon-delimited clause reads as a genuine list item:
/// substantial (several words), a lowercase continuation of the lead
/// sentence (real clauses read as "to exclude...", "where...", not a new
/// capitalized sentence), and clause-terminated.
fn is_probable_run_in_list_item(item: &str) -> bool {
    item.split_whitespace().count() >= RUN_IN_LIST_MIN_ITEM_WORDS
        && item.chars().next().is_some_and(char::is_lowercase)
        && matches!(item.chars().last(), Some(';' | '.'))
}

/// Build one split-off fragment, inheriting the source paragraph's
/// non-textual attributes (font size, boldness, page association, etc.).
fn run_in_list_fragment(source: &PdfParagraph, text: String, is_list_item: bool) -> PdfParagraph {
    let word_count = text.split_whitespace().count();
    PdfParagraph {
        text,
        lines: Vec::new(),
        heading_level: None,
        is_list_item,
        is_code_block: false,
        is_formula: false,
        layout_class: if is_list_item {
            Some(super::types::LayoutHintClass::ListItem)
        } else {
            source.layout_class
        },
        word_count,
        ..source.clone()
    }
}

fn apply_text_repair_to_structure_tree_paragraphs(paragraphs: &mut Vec<PdfParagraph>, has_positions: bool) {
    apply_to_all_segments(paragraphs, fused_text_repairs);
    dehyphenate_paragraphs(paragraphs, has_positions);
    split_embedded_list_items(paragraphs);
    synchronize_paragraph_text_metadata(paragraphs);
}

/// Invalidate cached paragraph text after mutating segments and refresh derived metadata.
///
/// Assembly derives both the emitted text and inline annotation byte ranges from segments
/// when `text` is empty. Keeping that cache empty prevents repaired segment text from
/// diverging from the stale pre-repair string used by the heuristic path.
fn synchronize_paragraph_text_metadata(paragraphs: &mut [PdfParagraph]) {
    for paragraph in paragraphs {
        paragraph.text.clear();
        paragraph.word_count = PdfParagraph::compute_word_count("", &paragraph.lines);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pdf::hierarchy::SegmentData;
    use crate::pdf::structure::types::{PdfLine, PdfParagraph};

    #[cfg(feature = "layout-detection")]
    #[test]
    fn table_model_preflight_uses_selected_slanet_variant() {
        use crate::core::config::layout::TableModel;

        assert_eq!(
            slanet_variant_for_table_model(TableModel::SlanetWired),
            Some("slanet_wired")
        );
        assert_eq!(
            slanet_variant_for_table_model(TableModel::SlanetWireless),
            Some("slanet_wireless")
        );
        assert_eq!(
            slanet_variant_for_table_model(TableModel::SlanetPlus),
            Some("slanet_plus")
        );
        assert_eq!(
            slanet_variant_for_table_model(TableModel::SlanetAuto),
            Some("slanet_wired")
        );
        assert_eq!(slanet_variant_for_table_model(TableModel::Tatr), None);
        assert_eq!(slanet_variant_for_table_model(TableModel::Disabled), None);
    }

    /// Helper: a table at `bbox` on `page` whose only content is `markdown`
    /// (so content weight == markdown length; empty cells).
    fn ov_table(page: u32, bbox: (f64, f64, f64, f64), markdown: &str) -> crate::types::Table {
        let (x0, y0, x1, y1) = bbox;
        crate::types::Table {
            cells: Vec::new(),
            markdown: markdown.to_string(),
            page_number: page,
            bounding_box: Some(crate::types::BoundingBox { x0, y0, x1, y1 }),
            ..Default::default()
        }
    }

    /// Helper: a table fragment with real cell content (needed to satisfy
    /// `fragments_are_stitchable`'s column-count check) at `bbox` on `page`.
    fn cell_table(page: u32, bbox: (f64, f64, f64, f64), cells: &[&[&str]]) -> crate::types::Table {
        let (x0, y0, x1, y1) = bbox;
        let cells: Vec<Vec<String>> = cells
            .iter()
            .map(|row| row.iter().map(|s| s.to_string()).collect())
            .collect();
        let markdown = cells.iter().map(|row| row.join("|")).collect::<Vec<_>>().join("\n");
        crate::types::Table {
            cells,
            markdown,
            page_number: page,
            bounding_box: Some(crate::types::BoundingBox { x0, y0, x1, y1 }),
            ..Default::default()
        }
    }

    /// Run the same id/columns assignment the real pipeline performs: stitch
    /// same-page fragments, then run the final, post-dedup assignment pass in
    /// `prepare_emitted_tables` (see issue #1297 code review: assigning ids
    /// inside `stitch_fragmented_tables` alone misses layout-detected tables).
    fn stitch_and_emit(
        native_tables: Vec<crate::types::Table>,
        layout_tables: Vec<crate::types::Table>,
        all_page_segments: &[Vec<SegmentData>],
    ) -> Vec<crate::types::Table> {
        use crate::core::config::layout::TableOverlapPreference;
        let stitched = stitch_fragmented_tables(native_tables, all_page_segments);
        prepare_emitted_tables(&stitched, layout_tables, TableOverlapPreference::Content)
    }

    /// Issue #1297: fragments of one physical table (stitched into a single
    /// chain) collapse into one `tables[]` entry, which naturally carries one
    /// `table_id`. A separate, non-adjacent table gets a distinct id.
    #[test]
    fn stitched_fragments_share_one_table_id_distinct_tables_differ() {
        let frag_top = cell_table(1, (0.0, 90.0, 100.0, 110.0), &[&["H1", "H2"]]);
        let frag_bottom = cell_table(1, (0.0, 70.0, 100.0, 89.0), &[&["a", "b"]]);
        let other_page_table = cell_table(2, (0.0, 0.0, 100.0, 20.0), &[&["X", "Y"]]);

        let all_page_segments: Vec<Vec<SegmentData>> = Vec::new();
        let result = stitch_and_emit(
            vec![frag_top, frag_bottom, other_page_table],
            Vec::new(),
            &all_page_segments,
        );

        assert_eq!(result.len(), 2, "the two page-1 fragments must stitch into one table");

        let page_1_table = result
            .iter()
            .find(|t| t.page_number == 1)
            .expect("page 1 table present");
        let page_2_table = result
            .iter()
            .find(|t| t.page_number == 2)
            .expect("page 2 table present");

        assert_eq!(page_1_table.cells.len(), 2, "stitched chain has both fragments' rows");
        assert!(page_1_table.table_id.is_some(), "stitched table must have a table_id");
        assert!(page_2_table.table_id.is_some(), "unrelated table must have a table_id");
        assert_ne!(
            page_1_table.table_id, page_2_table.table_id,
            "distinct physical tables must have distinct ids"
        );
    }

    /// Issue #1297: `table_id` assignment must be deterministic across runs
    /// for the same input (no randomness, no wall-clock dependence).
    #[test]
    fn table_id_assignment_is_deterministic_across_runs() {
        let build_input = || {
            vec![
                cell_table(2, (0.0, 0.0, 100.0, 20.0), &[&["X", "Y"]]),
                cell_table(1, (0.0, 0.0, 100.0, 20.0), &[&["A", "B"]]),
            ]
        };
        let all_page_segments: Vec<Vec<SegmentData>> = Vec::new();

        let first_run = stitch_and_emit(build_input(), Vec::new(), &all_page_segments);
        let second_run = stitch_and_emit(build_input(), Vec::new(), &all_page_segments);

        let first_ids: Vec<_> = first_run.iter().map(|t| (t.page_number, t.table_id.clone())).collect();
        let second_ids: Vec<_> = second_run.iter().map(|t| (t.page_number, t.table_id.clone())).collect();
        assert_eq!(first_ids, second_ids, "table_id assignment must be deterministic");
    }

    /// Issue #1297: every emitted table fragment carries `columns` (its own
    /// header row), even a fragment that stitching left untouched.
    #[test]
    fn stitching_populates_columns_on_merged_and_standalone_fragments() {
        let frag_top = cell_table(1, (0.0, 90.0, 100.0, 110.0), &[&["H1", "H2"]]);
        let frag_bottom = cell_table(1, (0.0, 70.0, 100.0, 89.0), &[&["a", "b"]]);
        let standalone = cell_table(3, (0.0, 0.0, 100.0, 20.0), &[&["Name", "Age"], &["Alice", "30"]]);

        let all_page_segments: Vec<Vec<SegmentData>> = Vec::new();
        let result = stitch_and_emit(vec![frag_top, frag_bottom, standalone], Vec::new(), &all_page_segments);

        let stitched = result.iter().find(|t| t.page_number == 1).unwrap();
        assert_eq!(
            stitched.columns,
            Some(vec!["H1".to_string(), "H2".to_string()]),
            "stitched table's columns come from the topmost fragment's header row"
        );

        let standalone_result = result.iter().find(|t| t.page_number == 3).unwrap();
        assert_eq!(
            standalone_result.columns,
            Some(vec!["Name".to_string(), "Age".to_string()]),
            "a standalone fragment's columns come from its own first row"
        );
    }

    /// Issue #1297 code review (Finding 1): a layout-detected table (never
    /// passed through `stitch_fragmented_tables`, only appended in
    /// `prepare_emitted_tables`) must still receive a `table_id` and
    /// `columns` once it survives dedup into the final emitted set.
    #[test]
    fn layout_detected_table_surviving_dedup_gets_table_id_and_columns() {
        let native = cell_table(1, (0.0, 0.0, 100.0, 20.0), &[&["A", "B"]]);
        let layout_only = cell_table(2, (0.0, 0.0, 100.0, 20.0), &[&["Layout1", "Layout2"], &["x", "y"]]);

        let all_page_segments: Vec<Vec<SegmentData>> = Vec::new();
        let result = stitch_and_emit(vec![native], vec![layout_only], &all_page_segments);

        assert_eq!(
            result.len(),
            2,
            "both the native and layout-detected tables must be emitted"
        );
        let layout_result = result
            .iter()
            .find(|t| t.page_number == 2)
            .expect("layout-detected table survives into the emitted set");

        assert!(
            layout_result.table_id.is_some(),
            "a layout-detected table must receive a table_id, not just native tables"
        );
        assert_eq!(
            layout_result.columns,
            Some(vec!["Layout1".to_string(), "Layout2".to_string()]),
            "a layout-detected table must receive columns from its own header row"
        );
    }

    #[test]
    fn identical_markdown_tables_collapse_despite_missing_bbox() {
        use crate::core::config::layout::TableOverlapPreference;
        let native = vec![crate::types::Table {
            cells: vec![vec!["a".into(), "b".into()]],
            markdown: "| a | b |".to_string(),
            page_number: 1,
            bounding_box: None,
            ..Default::default()
        }];
        let layout = vec![ov_table(1, (0.0, 0.0, 100.0, 100.0), "| a | b |")];

        let emitted = prepare_emitted_tables(&native, layout, TableOverlapPreference::Content);

        assert_eq!(
            emitted.len(),
            1,
            "byte-identical markdown on the same page collapses even when one table has no bbox"
        );
    }

    #[test]
    fn dedup_content_preference_keeps_larger_table() {
        use crate::core::config::layout::TableOverlapPreference;
        let mut tables = vec![
            ov_table(1, (0.0, 0.0, 100.0, 100.0), "a"),
            ov_table(1, (0.0, 0.0, 100.0, 100.0), "bbbbbbbbbb"),
        ];
        deduplicate_overlapping_tables(&mut tables, 1, TableOverlapPreference::Content);
        assert_eq!(tables.len(), 1);
        assert_eq!(
            tables[0].markdown, "bbbbbbbbbb",
            "Content keeps the larger (layout) table"
        );
    }

    #[test]
    fn dedup_native_preference_keeps_native_even_when_smaller() {
        use crate::core::config::layout::TableOverlapPreference;
        let mut tables = vec![
            ov_table(1, (0.0, 0.0, 100.0, 100.0), "a"),
            ov_table(1, (0.0, 0.0, 100.0, 100.0), "bbbbbbbbbb"),
        ];
        deduplicate_overlapping_tables(&mut tables, 1, TableOverlapPreference::Native);
        assert_eq!(tables.len(), 1);
        assert_eq!(
            tables[0].markdown, "a",
            "Native preference keeps native over a larger layout table"
        );
    }

    #[test]
    fn dedup_layout_preference_keeps_layout_even_when_smaller() {
        use crate::core::config::layout::TableOverlapPreference;
        let mut tables = vec![
            ov_table(1, (0.0, 0.0, 100.0, 100.0), "aaaaaaaaaa"),
            ov_table(1, (0.0, 0.0, 100.0, 100.0), "b"),
        ];
        deduplicate_overlapping_tables(&mut tables, 1, TableOverlapPreference::Layout);
        assert_eq!(tables.len(), 1);
        assert_eq!(
            tables[0].markdown, "b",
            "Layout preference keeps layout over a larger native table"
        );
    }

    #[test]
    fn dedup_native_preference_falls_back_to_content_for_same_origin() {
        use crate::core::config::layout::TableOverlapPreference;
        let mut tables = vec![
            ov_table(1, (0.0, 0.0, 100.0, 100.0), "a"),
            ov_table(1, (0.0, 0.0, 100.0, 100.0), "bbbbbbbbbb"),
        ];
        deduplicate_overlapping_tables(&mut tables, 2, TableOverlapPreference::Native);
        assert_eq!(tables.len(), 1);
        assert_eq!(
            tables[0].markdown, "bbbbbbbbbb",
            "same-origin overlap falls back to content"
        );
    }

    #[test]
    fn dedup_non_overlapping_tables_both_kept() {
        use crate::core::config::layout::TableOverlapPreference;
        let mut tables = vec![
            ov_table(1, (0.0, 0.0, 100.0, 100.0), "a"),
            ov_table(1, (200.0, 200.0, 300.0, 300.0), "b"),
        ];
        deduplicate_overlapping_tables(&mut tables, 1, TableOverlapPreference::Native);
        assert_eq!(tables.len(), 2, "non-overlapping tables are both kept");
    }

    #[test]
    fn side_by_side_layout_children_replace_content_heavy_parent() {
        use crate::core::config::layout::TableOverlapPreference;
        let native = vec![ov_table(1, (0.0, 0.0, 200.0, 100.0), &"parent".repeat(100))];
        let layout = vec![
            ov_table(1, (105.0, 0.0, 200.0, 100.0), "right"),
            ov_table(1, (0.0, 0.0, 95.0, 100.0), "left"),
        ];

        let emitted = prepare_emitted_tables(&native, layout, TableOverlapPreference::Content);

        assert_eq!(
            emitted.iter().map(|table| table.markdown.as_str()).collect::<Vec<_>>(),
            ["left", "right"]
        );
    }

    #[test]
    fn side_by_side_replacement_is_atomic_against_native_duplicate() {
        use crate::core::config::layout::TableOverlapPreference;
        let native = vec![
            ov_table(1, (0.0, 0.0, 200.0, 100.0), &"parent".repeat(100)),
            ov_table(1, (0.0, 0.0, 95.0, 100.0), &"native duplicate".repeat(100)),
        ];
        let layout = vec![
            ov_table(1, (0.0, 0.0, 95.0, 100.0), "left"),
            ov_table(1, (105.0, 0.0, 200.0, 100.0), "right"),
        ];

        let emitted = prepare_emitted_tables(&native, layout, TableOverlapPreference::Content);

        assert_eq!(
            emitted.iter().map(|table| table.markdown.as_str()).collect::<Vec<_>>(),
            ["left", "right"]
        );
    }

    #[test]
    fn side_by_side_replacement_is_atomic_against_earlier_layout_duplicate() {
        use crate::core::config::layout::TableOverlapPreference;
        let native = vec![ov_table(1, (0.0, 0.0, 200.0, 100.0), &"parent".repeat(100))];
        let better_left = "layout duplicate with more content";
        let layout = vec![
            ov_table(1, (0.0, 0.0, 95.0, 100.0), better_left),
            ov_table(1, (0.0, 0.0, 95.0, 100.0), "left"),
            ov_table(1, (105.0, 0.0, 200.0, 100.0), "right"),
        ];

        let emitted = prepare_emitted_tables(&native, layout, TableOverlapPreference::Content);

        assert_eq!(
            emitted.iter().map(|table| table.markdown.as_str()).collect::<Vec<_>>(),
            [better_left, "right"]
        );
    }

    #[test]
    fn side_by_side_replacement_is_atomic_against_later_layout_duplicate() {
        use crate::core::config::layout::TableOverlapPreference;
        let native = vec![ov_table(1, (0.0, 0.0, 200.0, 100.0), &"parent".repeat(100))];
        let better_left = "layout duplicate with more content";
        let layout = vec![
            ov_table(1, (0.0, 0.0, 95.0, 100.0), "left"),
            ov_table(1, (105.0, 0.0, 200.0, 100.0), "right"),
            ov_table(1, (0.0, 0.0, 95.0, 100.0), better_left),
        ];

        let emitted = prepare_emitted_tables(&native, layout, TableOverlapPreference::Content);

        assert_eq!(
            emitted.iter().map(|table| table.markdown.as_str()).collect::<Vec<_>>(),
            [better_left, "right"]
        );
    }

    #[test]
    fn overlapping_protected_replacement_groups_survive() {
        use crate::core::config::layout::TableOverlapPreference;
        let native = vec![
            ov_table(1, (0.0, 0.0, 200.0, 100.0), "upper parent"),
            ov_table(1, (0.0, 40.0, 200.0, 140.0), "lower parent"),
        ];
        let layout = vec![
            ov_table(1, (0.0, 0.0, 95.0, 100.0), "upper left"),
            ov_table(1, (105.0, 0.0, 200.0, 100.0), "upper right"),
            ov_table(1, (0.0, 40.0, 95.0, 140.0), "lower left"),
            ov_table(1, (105.0, 40.0, 200.0, 140.0), "lower right"),
        ];

        let emitted = prepare_emitted_tables(&native, layout, TableOverlapPreference::Content);

        assert_eq!(
            emitted.iter().map(|table| table.markdown.as_str()).collect::<Vec<_>>(),
            ["upper left", "upper right", "lower left", "lower right"]
        );
    }

    #[test]
    fn partially_shared_replacement_groups_keep_canonical_table_order() {
        use crate::core::config::layout::TableOverlapPreference;
        let native = vec![
            ov_table(1, (0.0, 0.0, 200.0, 100.0), "parent a"),
            ov_table(1, (105.0, 0.0, 305.0, 100.0), "parent b"),
        ];
        let layout = vec![
            ov_table(1, (105.0, 0.0, 200.0, 100.0), "shared"),
            ov_table(1, (210.0, 0.0, 305.0, 100.0), "right"),
            ov_table(1, (0.0, 0.0, 95.0, 100.0), "left"),
        ];

        let emitted = prepare_emitted_tables(&native, layout, TableOverlapPreference::Content);

        assert_eq!(
            emitted.iter().map(|table| table.markdown.as_str()).collect::<Vec<_>>(),
            ["left", "shared", "right"]
        );
    }

    #[test]
    fn side_by_side_replacement_orders_complete_affected_row_cohort() {
        use crate::core::config::layout::TableOverlapPreference;
        let native = vec![ov_table(1, (0.0, 0.0, 200.0, 100.0), "parent")];
        let layout = vec![
            ov_table(1, (105.0, 0.0, 200.0, 100.0), "right"),
            ov_table(1, (300.0, 0.0, 350.0, 100.0), "unrelated"),
            ov_table(1, (0.0, 0.0, 95.0, 100.0), "left"),
        ];

        let emitted = prepare_emitted_tables(&native, layout, TableOverlapPreference::Content);

        assert_eq!(
            emitted.iter().map(|table| table.markdown.as_str()).collect::<Vec<_>>(),
            ["left", "right", "unrelated"]
        );
    }

    #[test]
    fn side_by_side_replacement_preserves_interleaved_different_row_slot() {
        use crate::core::config::layout::TableOverlapPreference;
        let native = vec![ov_table(1, (0.0, 0.0, 200.0, 100.0), "parent")];
        let layout = vec![
            ov_table(1, (105.0, 0.0, 200.0, 100.0), "right"),
            ov_table(1, (300.0, -100.0, 350.0, -10.0), "different row"),
            ov_table(1, (0.0, 0.0, 95.0, 100.0), "left"),
        ];

        let emitted = prepare_emitted_tables(&native, layout, TableOverlapPreference::Content);

        assert_eq!(
            emitted.iter().map(|table| table.markdown.as_str()).collect::<Vec<_>>(),
            ["left", "different row", "right"]
        );
    }

    #[test]
    fn one_layout_child_does_not_replace_parent() {
        use crate::core::config::layout::TableOverlapPreference;
        let native = vec![ov_table(1, (0.0, 0.0, 200.0, 100.0), &"parent".repeat(100))];
        let layout = vec![ov_table(1, (0.0, 0.0, 95.0, 100.0), "left")];

        let emitted = prepare_emitted_tables(&native, layout, TableOverlapPreference::Content);

        assert_eq!(emitted.len(), 1);
        assert!(emitted[0].markdown.starts_with("parent"));
    }

    #[test]
    fn overlapping_layout_children_do_not_replace_parent() {
        use crate::core::config::layout::TableOverlapPreference;
        let native = vec![ov_table(1, (0.0, 0.0, 200.0, 100.0), &"parent".repeat(100))];
        let layout = vec![
            ov_table(1, (0.0, 0.0, 120.0, 100.0), "left"),
            ov_table(1, (80.0, 0.0, 200.0, 100.0), "right"),
        ];

        let emitted = prepare_emitted_tables(&native, layout, TableOverlapPreference::Content);

        assert_eq!(emitted.len(), 1);
        assert!(emitted[0].markdown.starts_with("parent"));
    }

    #[test]
    fn stacked_layout_children_do_not_replace_parent() {
        use crate::core::config::layout::TableOverlapPreference;
        let native = vec![ov_table(1, (0.0, 0.0, 200.0, 100.0), &"parent".repeat(100))];
        let layout = vec![
            ov_table(1, (0.0, 0.0, 200.0, 45.0), "top"),
            ov_table(1, (0.0, 55.0, 200.0, 100.0), "bottom"),
        ];

        let emitted = prepare_emitted_tables(&native, layout, TableOverlapPreference::Content);

        assert_eq!(emitted.len(), 1);
        assert!(emitted[0].markdown.starts_with("parent"));
    }

    #[test]
    fn shallow_layout_children_do_not_replace_tall_parent() {
        use crate::core::config::layout::TableOverlapPreference;
        let native = vec![ov_table(1, (0.0, 0.0, 200.0, 100.0), &"parent".repeat(100))];
        let layout = vec![
            ov_table(1, (0.0, 0.0, 95.0, 20.0), "left"),
            ov_table(1, (105.0, 0.0, 200.0, 20.0), "right"),
        ];

        let emitted = prepare_emitted_tables(&native, layout, TableOverlapPreference::Content);

        assert_eq!(emitted.len(), 1);
        assert!(emitted[0].markdown.starts_with("parent"));
    }

    #[test]
    fn weakly_overlapping_layout_children_do_not_replace_parent() {
        use crate::core::config::layout::TableOverlapPreference;
        let native = vec![ov_table(1, (0.0, 0.0, 200.0, 100.0), &"parent".repeat(100))];
        let layout = vec![
            ov_table(1, (-70.0, 0.0, 80.0, 100.0), "left"),
            ov_table(1, (120.0, 0.0, 270.0, 100.0), "right"),
        ];

        let emitted = prepare_emitted_tables(&native, layout, TableOverlapPreference::Content);

        assert_eq!(emitted.len(), 1);
        assert!(emitted[0].markdown.starts_with("parent"));
    }

    #[test]
    fn side_by_side_replacement_preserves_unrelated_table() {
        use crate::core::config::layout::TableOverlapPreference;
        let native = vec![
            ov_table(1, (0.0, 0.0, 200.0, 100.0), &"parent".repeat(100)),
            ov_table(2, (10.0, 10.0, 80.0, 80.0), "unrelated"),
        ];
        let layout = vec![
            ov_table(1, (0.0, 0.0, 95.0, 100.0), "left"),
            ov_table(1, (105.0, 0.0, 200.0, 100.0), "right"),
        ];

        let emitted = prepare_emitted_tables(&native, layout, TableOverlapPreference::Content);

        assert_eq!(
            emitted.iter().map(|table| table.markdown.as_str()).collect::<Vec<_>>(),
            ["unrelated", "left", "right"]
        );
    }

    #[test]
    fn native_preference_keeps_parent_over_side_by_side_children() {
        use crate::core::config::layout::TableOverlapPreference;
        let native = vec![ov_table(1, (0.0, 0.0, 200.0, 100.0), "parent")];
        let layout = vec![
            ov_table(1, (0.0, 0.0, 95.0, 100.0), &"left".repeat(100)),
            ov_table(1, (105.0, 0.0, 200.0, 100.0), &"right".repeat(100)),
        ];

        let emitted = prepare_emitted_tables(&native, layout, TableOverlapPreference::Native);

        assert_eq!(emitted.len(), 1);
        assert_eq!(emitted[0].markdown, "parent");
    }

    #[test]
    fn dropped_duplicate_table_does_not_suppress_text() {
        use crate::core::config::layout::TableOverlapPreference;

        let native_tables = vec![ov_table(1, (0.0, 0.0, 100.0, 100.0), "native table content")];
        let layout_tables = vec![ov_table(1, (0.0, 0.0, 200.0, 100.0), "x")];
        let emitted_tables = prepare_emitted_tables(&native_tables, layout_tables, TableOverlapPreference::Content);
        let bboxes_by_page = table_bboxes_by_page(&emitted_tables);

        assert_eq!(emitted_tables.len(), 1);
        assert_eq!(emitted_tables[0].bounding_box.expect("kept table bbox").x1, 100.0);

        let segment = SegmentData {
            x: 150.0,
            y: 10.0,
            width: 20.0,
            height: 12.0,
            ..seg("text outside the emitted table", 150.0, 20.0)
        };
        let filtered = filter_segments_by_table_bboxes(
            vec![segment],
            bboxes_by_page.get(&0).map(Vec::as_slice).unwrap_or_default(),
        );
        assert_eq!(filtered.len(), 1, "a discarded duplicate bbox must not remove text");
    }

    #[test]
    fn empty_table_does_not_suppress_text() {
        use crate::core::config::layout::TableOverlapPreference;

        let native_tables = vec![ov_table(1, (0.0, 0.0, 100.0, 100.0), "  \n")];
        let emitted_tables = prepare_emitted_tables(&native_tables, Vec::new(), TableOverlapPreference::Content);
        let bboxes_by_page = table_bboxes_by_page(&emitted_tables);

        assert!(
            emitted_tables.is_empty(),
            "assembly would not emit whitespace-only markdown"
        );
        assert!(
            bboxes_by_page.is_empty(),
            "non-emitted tables must not contribute suppression boxes"
        );

        let segment = SegmentData {
            x: 10.0,
            y: 10.0,
            width: 20.0,
            height: 12.0,
            ..seg("text under an empty table", 10.0, 20.0)
        };
        let filtered = filter_segments_by_table_bboxes(
            vec![segment],
            bboxes_by_page.get(&0).map(Vec::as_slice).unwrap_or_default(),
        );
        assert_eq!(filtered.len(), 1);
    }

    #[test]
    fn empty_table_does_not_displace_valid_overlap() {
        use crate::core::config::layout::TableOverlapPreference;

        let native_tables = vec![ov_table(1, (0.0, 0.0, 100.0, 100.0), "  \n")];
        let layout_tables = vec![ov_table(1, (0.0, 0.0, 100.0, 100.0), "| valid |")];

        let emitted_tables = prepare_emitted_tables(&native_tables, layout_tables, TableOverlapPreference::Native);

        assert_eq!(emitted_tables.len(), 1);
        assert_eq!(emitted_tables[0].markdown, "| valid |");
    }

    #[cfg(feature = "layout-detection")]
    #[test]
    fn missing_wrapper_validation_is_treated_as_skipped() {
        use super::super::regions::layout_validation::RegionValidation;

        let hint = |class_name| LayoutHint {
            class_name,
            confidence: 0.9,
            left: 0.0,
            bottom: 0.0,
            right: 100.0,
            top: 100.0,
        };
        let hints = vec![
            hint(LayoutHintClass::Picture),
            hint(LayoutHintClass::Form),
            hint(LayoutHintClass::Text),
        ];
        let ownership = wrapper_ownership_by_hint(&hints, &[RegionValidation::Empty]);
        assert_eq!(ownership, [false, true, true]);
    }

    #[test]
    fn emitted_table_still_suppresses_covered_text() {
        use crate::core::config::layout::TableOverlapPreference;

        let native_tables = vec![ov_table(1, (0.0, 0.0, 100.0, 100.0), "| value |")];
        let emitted_tables = prepare_emitted_tables(&native_tables, Vec::new(), TableOverlapPreference::Content);
        let bboxes_by_page = table_bboxes_by_page(&emitted_tables);
        let segment = SegmentData {
            x: 10.0,
            y: 10.0,
            width: 20.0,
            height: 12.0,
            ..seg("duplicated table text", 10.0, 20.0)
        };

        let filtered = filter_segments_by_table_bboxes(
            vec![segment],
            bboxes_by_page.get(&0).map(Vec::as_slice).unwrap_or_default(),
        );
        assert!(
            filtered.is_empty(),
            "an emitted table must continue to suppress duplicate text"
        );
    }

    /// Helper: segment with font metadata for title-promotion tests.
    fn role_seg(text: &str, font_size: f32, is_bold: bool, assigned_role: Option<u8>) -> SegmentData {
        SegmentData {
            text: text.to_string(),
            x: 72.0,
            y: 700.0,
            width: 200.0,
            height: font_size,
            font_size,
            is_bold,
            is_italic: false,
            is_monospace: false,
            baseline_y: 700.0,
            assigned_role,
        }
    }

    /// A bold, first-page, larger-than-any-tagged-heading tier must be promoted
    /// to h1 with the tagged hierarchy shifted down one level.
    #[test]
    fn promote_title_shifts_tagged_heading_levels_down() {
        let pages = vec![vec![
            role_seg("Titre du document", 28.0, true, None),
            role_seg("Titre 1", 18.0, true, Some(1)),
            role_seg("Titre 2", 16.0, true, Some(2)),
            role_seg("body text", 12.0, false, None),
        ]];
        let mut map = build_heading_map_from_assigned_roles(&pages);
        assert!(promote_untagged_document_title(&mut map, &pages));

        let level_of = |font: f32| map.iter().find(|(f, _)| (*f - font).abs() < 0.05).and_then(|(_, l)| *l);
        assert_eq!(level_of(28.0), Some(1), "title tier must become h1");
        assert_eq!(level_of(18.0), Some(2), "tagged H1 must demote to h2");
        assert_eq!(level_of(16.0), Some(3), "tagged H2 must demote to h3");
        assert_eq!(level_of(12.0), None, "body must stay body");
    }

    /// No untagged tier above the largest tagged heading → no promotion.
    #[test]
    fn promote_title_no_candidate_leaves_map_unchanged() {
        let pages = vec![vec![
            role_seg("Heading", 18.0, true, Some(1)),
            role_seg("body", 12.0, false, None),
        ]];
        let mut map = build_heading_map_from_assigned_roles(&pages);
        let before = map.clone();
        assert!(!promote_untagged_document_title(&mut map, &pages));
        assert_eq!(map, before);
    }

    /// A non-bold large tier (e.g. a pull quote) must not be mistaken for a title.
    #[test]
    fn promote_title_requires_bold() {
        let pages = vec![vec![
            role_seg("large quote", 28.0, false, None),
            role_seg("Heading", 18.0, true, Some(1)),
        ]];
        let mut map = build_heading_map_from_assigned_roles(&pages);
        assert!(!promote_untagged_document_title(&mut map, &pages));
    }

    /// A large tier appearing only after page 0 is not a document title.
    #[test]
    fn promote_title_requires_first_page() {
        let pages = vec![
            vec![role_seg("Heading", 18.0, true, Some(1))],
            vec![role_seg("Big banner later", 28.0, true, None)],
        ];
        let mut map = build_heading_map_from_assigned_roles(&pages);
        assert!(!promote_untagged_document_title(&mut map, &pages));
    }

    /// A mid-word split (e.g. "Text" extracted as "Te" + "xt", same role and
    /// font size, immediately adjacent) must count as one logical block, not
    /// two — otherwise a font-encoding artifact inflates the apparent
    /// document size past the sparsity floor.
    #[test]
    fn count_logical_blocks_merges_same_role_same_size_runs() {
        let pages = vec![vec![
            role_seg("Big", 24.0, false, Some(1)),
            role_seg("Small Text", 12.0, true, Some(2)),
            role_seg("Te", 24.0, false, Some(1)),
            role_seg("xt", 24.0, false, Some(1)),
        ]];
        assert_eq!(
            count_logical_blocks(&pages),
            3,
            "the split \"Te\"+\"xt\" run must collapse into a single block"
        );
    }

    /// Segments with different assigned roles never merge, even at the same
    /// font size.
    #[test]
    fn count_logical_blocks_does_not_merge_different_roles() {
        let pages = vec![vec![
            role_seg("Heading", 18.0, true, Some(1)),
            role_seg("more heading text", 18.0, true, Some(2)),
        ]];
        assert_eq!(count_logical_blocks(&pages), 2);
    }

    /// Sparse document where the structure tree tags every block as a heading
    /// with no body tier at all (a document with just a couple of heading-tagged
    /// lines and nothing else) must have every role suppressed rather than trusted.
    #[test]
    fn suppress_all_heading_roles_fires_when_sparse_and_all_tagged() {
        let mut pages = vec![vec![
            role_seg("Big", 24.0, false, Some(1)),
            role_seg("Small Text", 12.0, true, Some(2)),
            role_seg("Te xt", 24.0, false, Some(1)),
        ]];
        let mut map = build_heading_map_from_assigned_roles(&pages);
        assert!(suppress_all_heading_roles_when_sparse_and_untrusted(
            &mut map, &mut pages
        ));

        assert!(
            map.iter().all(|(_, level)| level.is_none()),
            "heading map must be fully suppressed; got: {map:?}"
        );
        for page in &pages {
            for seg in page {
                assert_eq!(
                    seg.assigned_role, None,
                    "assigned_role must be cleared on every segment"
                );
            }
        }
    }

    /// A sparse document with one tagged heading and one untagged body
    /// paragraph (the `issue-987-test.pdf` shape: "Big"/"Te xt" tagged,
    /// "Small Text" untagged — 3 total blocks) must ALSO be suppressed: a mix
    /// of heading and body tiers on that few blocks is not enough evidence
    /// that the tagging is trustworthy, matching GT for that fixture (plain
    /// "Big Text"/"Small Text", no headings at all).
    #[test]
    fn suppress_all_heading_roles_fires_when_sparse_with_body_tier() {
        let mut pages = vec![vec![
            role_seg("Title", 24.0, true, Some(1)),
            role_seg("body text", 12.0, false, None),
        ]];
        let mut map = build_heading_map_from_assigned_roles(&pages);
        assert!(suppress_all_heading_roles_when_sparse_and_untrusted(
            &mut map, &mut pages
        ));
        assert_eq!(pages[0][0].assigned_role, None, "tagged role must be cleared");
    }

    /// A sparse document with no heading roles at all must not be touched —
    /// there is nothing to suppress.
    #[test]
    fn suppress_all_heading_roles_does_not_fire_with_no_headings() {
        let mut pages = vec![vec![
            role_seg("body text one", 12.0, false, None),
            role_seg("body text two", 12.0, false, None),
        ]];
        let mut map = build_heading_map_from_assigned_roles(&pages);
        assert!(!suppress_all_heading_roles_when_sparse_and_untrusted(
            &mut map, &mut pages
        ));
    }

    /// At or above the sparsity floor, an all-heading-tagged document is left
    /// alone even with no body tier — larger documents are trusted.
    #[test]
    fn suppress_all_heading_roles_does_not_fire_at_or_above_floor() {
        // Alternate heading/body role so each segment is a distinct logical
        // block under `count_logical_blocks` rather than collapsing into one. ~keep
        let mut pages = vec![
            (0..MIN_BLOCKS_FOR_FONT_HEADING)
                .map(|i| {
                    if i % 2 == 0 {
                        role_seg(&format!("Heading {i}"), 18.0, true, Some(1))
                    } else {
                        role_seg(&format!("Body paragraph {i}."), 12.0, false, None)
                    }
                })
                .collect(),
        ];
        let mut map = build_heading_map_from_assigned_roles(&pages);
        assert!(!suppress_all_heading_roles_when_sparse_and_untrusted(
            &mut map, &mut pages
        ));
        assert_eq!(
            pages[0][0].assigned_role,
            Some(1),
            "role must be untouched at/above the floor"
        );
    }

    /// Role demotion mirrors the map shift on segments (bridge.rs reads roles directly).
    #[test]
    fn demote_assigned_roles_shifts_and_caps() {
        let mut pages = vec![vec![
            role_seg("h1", 18.0, true, Some(1)),
            role_seg("h6", 8.0, true, Some(6)),
            role_seg("body", 12.0, false, None),
        ]];
        demote_assigned_roles(&mut pages);
        assert_eq!(pages[0][0].assigned_role, Some(2));
        assert_eq!(pages[0][1].assigned_role, Some(6), "level 6 must cap, not overflow");
        assert_eq!(pages[0][2].assigned_role, None);
    }

    #[test]
    fn assigned_sal_annotation_role_is_demoted() {
        let paragraphs = process_heuristic_segments(vec![role_seg("__inout_bcount_full(n)", 12.0, false, Some(2))]);
        assert_eq!(paragraphs[0].heading_level, None);
    }

    #[test]
    fn assigned_identifier_heading_role_is_preserved() {
        let paragraphs = blocks_to_paragraphs(
            vec![role_seg("__in_section", 12.0, false, Some(2))],
            &[(12.0, None)],
            &[],
        );
        assert_eq!(paragraphs[0].heading_level, Some(2));
    }

    /// Helper: create a segment with positional data.
    fn seg(text: &str, x: f32, width: f32) -> SegmentData {
        SegmentData {
            text: text.to_string(),
            x,
            y: 0.0,
            width,
            height: 12.0,
            font_size: 12.0,
            is_bold: false,
            is_italic: false,
            is_monospace: false,
            baseline_y: 0.0,
            assigned_role: None,
        }
    }

    fn inline_seg(text: &str, x: f32, baseline_y: f32, is_bold: bool) -> SegmentData {
        let mut segment = seg(text, x, 20.0);
        segment.baseline_y = baseline_y;
        segment.y = baseline_y - segment.height;
        segment.is_bold = is_bold;
        segment
    }

    #[test]
    fn inline_bold_runs_stay_in_one_paragraph() {
        let segments = vec![
            inline_seg("plain", 10.0, 100.0, false),
            inline_seg("bold", 31.0, 100.0, true),
            inline_seg("tail", 52.0, 100.0, false),
        ];

        let paragraphs = blocks_to_paragraphs(segments, &[], &[]);

        assert_eq!(paragraphs.len(), 1);
        assert_eq!(paragraphs[0].lines.len(), 1);
        assert_eq!(paragraphs[0].lines[0].segments.len(), 3);
        assert!(paragraphs[0].lines[0].segments[1].is_bold);
        assert_eq!(paragraph_text(&paragraphs[0]), "plain bold tail");

        let document = crate::pdf::structure::assembly::assemble_internal_document(vec![paragraphs], &[], None, &[]);
        let element = &document.elements[0];
        let bold = element
            .annotations
            .iter()
            .find(|annotation| matches!(annotation.kind, crate::types::AnnotationKind::Bold))
            .expect("inline bold annotation should be preserved");
        assert_eq!(element.text, "plain bold tail");
        assert_eq!((bold.start, bold.end), (6, 10));
    }

    #[test]
    fn inline_typographic_dash_does_not_split_a_paragraph() {
        let segments = vec![
            inline_seg("Figures 6", 10.0, 100.0, false),
            inline_seg("– 8 show the results", 31.0, 100.0, false),
        ];

        let paragraphs = blocks_to_paragraphs(segments, &[], &[]);

        assert_eq!(paragraphs.len(), 1);
        assert!(!paragraphs[0].is_list_item);
    }

    #[test]
    fn typographic_dash_on_a_new_line_still_starts_a_list() {
        let segments = vec![
            inline_seg("Introduction", 10.0, 100.0, false),
            inline_seg("– first item", 10.0, 80.0, false),
        ];

        let paragraphs = blocks_to_paragraphs(segments, &[], &[]);

        assert_eq!(paragraphs.len(), 2);
        assert!(paragraphs[1].is_list_item);
    }

    #[test]
    fn split_typographic_dash_and_same_line_body_stay_a_list() {
        let segments = vec![
            inline_seg("Introduction", 10.0, 100.0, false),
            inline_seg("–", 10.0, 80.0, false),
            inline_seg("quoted body", 31.0, 80.0, false),
        ];

        let paragraphs = blocks_to_paragraphs(segments, &[], &[]);

        assert_eq!(paragraphs.len(), 2);
        assert!(paragraphs[1].is_list_item);
    }

    #[test]
    fn split_typographic_dash_and_different_line_body_are_not_a_list() {
        let segments = vec![
            inline_seg("Figures 6", 10.0, 100.0, false),
            inline_seg("– ", 31.0, 100.0, false),
            inline_seg("8 show the results", 10.0, 80.0, false),
        ];

        let paragraphs = blocks_to_paragraphs(segments, &[], &[]);

        assert!(paragraphs.iter().all(|paragraph| !paragraph.is_list_item));
    }

    #[test]
    fn cross_line_bold_transition_remains_a_boundary() {
        let segments = vec![
            inline_seg("Heading", 10.0, 100.0, true),
            inline_seg("body", 10.0, 80.0, false),
        ];

        assert_eq!(blocks_to_paragraphs(segments, &[], &[]).len(), 2);
    }

    #[test]
    fn tagged_heading_and_body_stay_separate_on_the_same_line() {
        let mut heading = inline_seg("Heading", 10.0, 100.0, true);
        heading.assigned_role = Some(1);
        let body = inline_seg("body", 31.0, 100.0, false);

        let paragraphs = blocks_to_paragraphs(vec![heading, body], &[], &[]);

        assert_eq!(paragraphs.len(), 2);
        assert_eq!(paragraph_text(&paragraphs[0]), "Heading");
        assert_eq!(paragraphs[0].heading_level, Some(1));
        assert_eq!(paragraph_text(&paragraphs[1]), "body");
        assert_eq!(paragraphs[1].heading_level, None);
    }

    #[test]
    fn different_tagged_heading_levels_stay_separate_on_the_same_line() {
        let mut first = inline_seg("First", 10.0, 100.0, true);
        first.assigned_role = Some(1);
        let mut second = inline_seg("Second", 31.0, 100.0, false);
        second.assigned_role = Some(2);

        let paragraphs = blocks_to_paragraphs(vec![first, second], &[], &[]);

        assert_eq!(paragraphs.len(), 2);
        assert_eq!(paragraph_text(&paragraphs[0]), "First");
        assert_eq!(paragraphs[0].heading_level, Some(1));
        assert_eq!(paragraph_text(&paragraphs[1]), "Second");
        assert_eq!(paragraphs[1].heading_level, Some(2));
    }

    #[test]
    fn same_tagged_heading_role_keeps_inline_style_transitions_together() {
        let mut first = inline_seg("First", 10.0, 100.0, true);
        first.assigned_role = Some(1);
        let mut second = inline_seg("Second", 31.0, 100.0, false);
        second.assigned_role = Some(1);

        let paragraphs = blocks_to_paragraphs(vec![first, second], &[], &[]);

        assert_eq!(paragraphs.len(), 1);
        assert_eq!(paragraph_text(&paragraphs[0]), "First Second");
        assert_eq!(paragraphs[0].heading_level, Some(1));
    }

    #[test]
    fn distant_same_line_bold_transition_remains_a_boundary() {
        let segments = vec![
            inline_seg("left", 10.0, 100.0, false),
            inline_seg("right", 100.0, 100.0, true),
        ];

        assert_eq!(blocks_to_paragraphs(segments, &[], &[]).len(), 2);
    }

    #[test]
    fn overlapping_or_reverse_bold_transition_remains_a_boundary() {
        let overlapping = vec![
            inline_seg("first", 30.0, 100.0, false),
            inline_seg("second", 40.0, 100.0, true),
        ];
        let reversed = vec![
            inline_seg("first", 30.0, 100.0, false),
            inline_seg("second", 5.0, 100.0, true),
        ];

        assert_eq!(blocks_to_paragraphs(overlapping, &[], &[]).len(), 2);
        assert_eq!(blocks_to_paragraphs(reversed, &[], &[]).len(), 2);
    }

    #[test]
    fn slight_metric_overlap_is_still_inline() {
        let segments = vec![
            inline_seg("plain", 30.0, 100.0, false),
            inline_seg("bold", 49.0, 100.0, true),
        ];

        assert_eq!(blocks_to_paragraphs(segments, &[], &[]).len(), 1);
    }

    #[test]
    fn invalid_inline_geometry_remains_a_boundary() {
        let plain = inline_seg("plain", 10.0, 100.0, false);
        let mut zero_font = inline_seg("bold", 31.0, 100.0, true);
        zero_font.font_size = 0.0;
        let mut non_finite_x = inline_seg("bold", 31.0, 100.0, true);
        non_finite_x.x = f32::NAN;
        let mut non_finite_baseline = inline_seg("bold", 31.0, 100.0, true);
        non_finite_baseline.baseline_y = f32::NAN;

        assert_eq!(blocks_to_paragraphs(vec![plain.clone(), zero_font], &[], &[]).len(), 2);
        assert_eq!(
            blocks_to_paragraphs(vec![plain.clone(), non_finite_x], &[], &[]).len(),
            2
        );
        assert_eq!(
            blocks_to_paragraphs(vec![plain, non_finite_baseline], &[], &[]).len(),
            2
        );
    }

    #[test]
    fn later_line_inline_style_transition_does_not_absorb_prior_lines() {
        let segments = vec![
            inline_seg("first line", 10.0, 120.0, false),
            inline_seg("plain", 10.0, 100.0, false),
            inline_seg("bold", 31.0, 100.0, true),
        ];

        assert_eq!(blocks_to_paragraphs(segments, &[], &[]).len(), 2);
    }

    #[test]
    fn monospace_style_transition_remains_a_boundary() {
        let mut plain = inline_seg("let value =", 10.0, 100.0, false);
        plain.is_monospace = true;
        let mut bold = inline_seg("42", 31.0, 100.0, true);
        bold.is_monospace = true;

        assert_eq!(blocks_to_paragraphs(vec![plain, bold], &[], &[]).len(), 2);
    }

    fn line(segments: Vec<SegmentData>) -> PdfLine {
        PdfLine {
            segments,
            baseline_y: 0.0,
            dominant_font_size: 12.0,
            is_bold: false,
            is_monospace: false,
        }
    }

    fn para(lines: Vec<PdfLine>) -> PdfParagraph {
        let word_count = PdfParagraph::compute_word_count("", &lines);
        PdfParagraph {
            text: String::new(),
            lines,
            dominant_font_size: 12.0,
            heading_level: None,
            is_bold: false,
            is_list_item: false,
            is_code_block: false,
            is_formula: false,
            is_page_furniture: false,
            layout_class: None,
            layout_region_path: None,
            caption_for: None,
            block_bbox: None,
            word_count,
        }
    }

    fn outline_para(text: &str) -> PdfParagraph {
        let mut paragraph = para(vec![line(vec![seg(text, 0.0, 100.0)])]);
        paragraph.text = text.to_string();
        paragraph.word_count = text.split_whitespace().count();
        paragraph
    }

    /// Regression test for xberg-io/xberg#1301 (mode a): a colon-introduced,
    /// semicolon-delimited run-in list with no distinguishing indentation or
    /// line break — exactly how it is rendered from unstyled HTML — is split
    /// into a lead paragraph plus one list item per clause.
    #[test]
    fn run_in_colon_semicolon_list_is_split_into_lead_and_items() {
        let text = "Article 1. The management board is authorised to exclude subscription rights: \
to exclude fractional amounts from the shareholders' subscription right; \
where the new shares are issued against cash contributions at market price;";
        let mut pages = vec![vec![outline_para(text)]];

        split_colon_semicolon_run_in_lists(&mut pages);

        assert_eq!(pages[0].len(), 3, "lead paragraph + 2 list items");
        assert!(!pages[0][0].is_list_item);
        assert!(
            pages[0][0].text.ends_with("authorised to exclude subscription rights:"),
            "lead keeps everything up to and including the anchor colon: {}",
            pages[0][0].text
        );
        assert!(pages[0][1].is_list_item);
        assert_eq!(
            pages[0][1].text,
            "to exclude fractional amounts from the shareholders' subscription right;"
        );
        assert!(pages[0][2].is_list_item);
        assert_eq!(
            pages[0][2].text,
            "where the new shares are issued against cash contributions at market price;"
        );
    }

    #[test]
    fn run_in_list_split_requires_at_least_two_clauses() {
        let mut pages = vec![vec![outline_para("Note: see the appendix for full details.")]];

        split_colon_semicolon_run_in_lists(&mut pages);

        assert_eq!(
            pages[0].len(),
            1,
            "a single clause after the colon is not an enumeration"
        );
        assert!(!pages[0][0].is_list_item);
    }

    #[test]
    fn run_in_list_split_leaves_unrelated_paragraphs_untouched_and_in_order() {
        let list_text = "The board is authorised to exclude rights: to exclude fractional amounts; \
where new shares are issued;";
        let decoy = "- a bare dash-prefixed clause outside a list, unit #06-18 Tower 2, Singapore.";
        let mut pages = vec![vec![outline_para(list_text), outline_para(decoy)]];

        split_colon_semicolon_run_in_lists(&mut pages);

        assert_eq!(pages[0].len(), 4, "lead + 2 items + the untouched trailing paragraph");
        assert_eq!(
            pages[0][3].text, decoy,
            "trailing paragraph keeps its text and reading-order position"
        );
    }

    #[test]
    fn outline_recovery_is_page_scoped_and_uses_root_h2() {
        let mut intro = outline_para("1. Introduction");
        intro.is_list_item = true;
        intro.is_page_furniture = true;
        let mut pages = vec![vec![intro, outline_para("Methods")], vec![outline_para("Introduction")]];
        let entries = vec![
            PdfOutlineEntry::test_entry("Introduction", 0, 1),
            PdfOutlineEntry::test_entry("Methods", 1, 1),
        ];

        recover_headings_from_outline(&mut pages, &entries);

        assert_eq!(pages[0][0].heading_level, Some(2));
        assert_eq!(pages[0][1].heading_level, Some(3));
        assert_eq!(pages[1][0].heading_level, None);
        assert!(!pages[0][0].is_list_item);
        assert!(!pages[0][0].is_page_furniture);
    }

    #[test]
    fn outline_recovery_calibrates_from_two_consistent_anchors() {
        let mut first = outline_para("First anchor");
        first.heading_level = Some(1);
        let mut second = outline_para("Second anchor");
        second.heading_level = Some(2);
        let mut pages = vec![vec![first, second, outline_para("Recovered")]];
        let entries = vec![
            PdfOutlineEntry::test_entry("First anchor", 0, 1),
            PdfOutlineEntry::test_entry("Second anchor", 1, 1),
            PdfOutlineEntry::test_entry("Recovered", 2, 1),
        ];

        recover_headings_from_outline(&mut pages, &entries);

        assert_eq!(pages[0][2].heading_level, Some(3));
    }

    #[test]
    fn outline_recovery_ignores_singleton_bad_calibration_anchor() {
        let mut anchor = outline_para("Bad anchor");
        anchor.heading_level = Some(5);
        let mut pages = vec![vec![anchor, outline_para("Recovered")]];
        let entries = vec![
            PdfOutlineEntry::test_entry("Bad anchor", 0, 1),
            PdfOutlineEntry::test_entry("Recovered", 1, 1),
        ];

        recover_headings_from_outline(&mut pages, &entries);

        assert_eq!(pages[0][1].heading_level, Some(3));
    }

    #[test]
    fn outline_recovery_rejects_ambiguous_titles() {
        let mut pages = vec![vec![
            outline_para("Duplicate outline"),
            outline_para("Duplicate paragraph"),
            outline_para("Duplicate paragraph"),
        ]];
        let entries = vec![
            PdfOutlineEntry::test_entry("Duplicate outline", 0, 1),
            PdfOutlineEntry::test_entry("Duplicate outline", 1, 1),
            PdfOutlineEntry::test_entry("Duplicate paragraph", 0, 1),
        ];

        recover_headings_from_outline(&mut pages, &entries);

        assert!(pages[0].iter().all(|paragraph| paragraph.heading_level.is_none()));
    }

    #[test]
    fn outline_recovery_rejects_semantic_non_headings() {
        let mut header = outline_para("Header");
        header.layout_class = Some(LayoutHintClass::PageHeader);
        let mut list = outline_para("List");
        list.layout_class = Some(LayoutHintClass::ListItem);
        let mut formula = outline_para("Formula");
        formula.is_formula = true;
        let mut pages = vec![vec![header, list, formula]];
        let entries = vec![
            PdfOutlineEntry::test_entry("Header", 0, 1),
            PdfOutlineEntry::test_entry("List", 0, 1),
            PdfOutlineEntry::test_entry("Formula", 0, 1),
        ];

        recover_headings_from_outline(&mut pages, &entries);

        assert!(pages[0].iter().all(|paragraph| paragraph.heading_level.is_none()));
    }

    #[test]
    fn outline_title_normalization_handles_labels_without_aliasing_prose() {
        assert_eq!(
            normalize_outline_title("1. Introduction"),
            normalize_outline_title("Introduction")
        );
        assert_eq!(
            normalize_outline_title("IV. Results"),
            normalize_outline_title("Results")
        );
        assert_ne!(
            normalize_outline_title("A quick example"),
            normalize_outline_title("quick example")
        );
        assert_ne!(
            normalize_outline_title("2024 Report"),
            normalize_outline_title("Report")
        );
        assert_ne!(normalize_outline_title("v2 API"), normalize_outline_title("API"));
    }

    fn paragraph_text(paragraph: &PdfParagraph) -> String {
        paragraph
            .lines
            .iter()
            .flat_map(|line| line.segments.iter())
            .map(|segment| segment.text.as_str())
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn heuristic_segment(text: &str, baseline_y: f32, width: f32, is_monospace: bool) -> SegmentData {
        let mut segment = seg(text, 10.0, width);
        segment.y = baseline_y - segment.height;
        segment.baseline_y = baseline_y;
        segment.is_monospace = is_monospace;
        segment
    }

    fn process_heuristic_segments(segments: Vec<SegmentData>) -> Vec<PdfParagraph> {
        process_single_page(
            PageInput {
                page_index: 0,
                struct_paragraphs: None,
                heuristic_segments: segments,
                page_hints: None,
                table_bboxes: Vec::new(),
                #[cfg(feature = "layout-detection")]
                hint_validations: Vec::new(),
                #[cfg(feature = "layout-detection")]
                page_width_pts: None,
                needs_classify: false,
                paragraph_gap_ys: Vec::new(),
                include_headers: true,
                include_footers: true,
            },
            &[],
            None,
        )
    }

    #[cfg(feature = "layout-detection")]
    #[test]
    fn empty_or_ineligible_layout_hints_use_legacy_page_processing() {
        let segments = vec![
            heuristic_segment("First paragraph.", 700.0, 220.0, false),
            heuristic_segment("Second paragraph.", 600.0, 220.0, false),
        ];
        let paragraph_gap_ys = compute_paragraph_gap_ys(&segments);
        let process = |page_hints| {
            process_single_page(
                PageInput {
                    page_index: 0,
                    struct_paragraphs: None,
                    heuristic_segments: segments.clone(),
                    page_hints,
                    table_bboxes: Vec::new(),
                    hint_validations: Vec::new(),
                    page_width_pts: None,
                    needs_classify: false,
                    paragraph_gap_ys: paragraph_gap_ys.clone(),
                    include_headers: true,
                    include_footers: true,
                },
                &[],
                None,
            )
        };

        let legacy = process(None);
        let empty = process(Some(Vec::new()));
        let invalid = process(Some(vec![LayoutHint {
            class_name: crate::pdf::structure::types::LayoutHintClass::Text,
            confidence: 0.9,
            left: 0.0,
            bottom: 0.0,
            right: f32::INFINITY,
            top: 100.0,
        }]));
        let non_overlapping = process(Some(vec![LayoutHint {
            class_name: crate::pdf::structure::types::LayoutHintClass::Text,
            confidence: 0.9,
            left: 400.0,
            bottom: 0.0,
            right: 500.0,
            top: 100.0,
        }]));

        assert_eq!(format!("{empty:?}"), format!("{legacy:?}"));
        assert_eq!(format!("{invalid:?}"), format!("{legacy:?}"));
        assert_eq!(format!("{non_overlapping:?}"), format!("{legacy:?}"));
    }

    #[cfg(feature = "layout-detection")]
    #[test]
    fn page_width_reaches_layout_reading_order_graph() {
        let positioned_segment = |text: &str, x: f32, y: f32| {
            let mut segment = heuristic_segment(text, y + 10.0, 10.0, false);
            segment.x = x;
            segment.y = y;
            segment.height = 10.0;
            segment
        };
        let segments = vec![
            positioned_segment("bottom-left", 10.0, 205.0),
            positioned_segment("top-left", 90.0, 305.0),
            positioned_segment("top-right", 200.0, 305.0),
            positioned_segment("bottom-right", 250.0, 205.0),
        ];
        let hint = |left, bottom, right, top| LayoutHint {
            class_name: crate::pdf::structure::types::LayoutHintClass::Text,
            confidence: 0.95,
            left,
            bottom,
            right,
            top,
        };
        let hints = vec![
            hint(0.0, 200.0, 120.0, 220.0),
            hint(80.0, 300.0, 160.0, 320.0),
            hint(120.0, 300.0, 240.0, 320.0),
            hint(160.0, 200.0, 280.0, 220.0),
        ];
        let process = |page_width_pts| {
            process_single_page(
                PageInput {
                    page_index: 0,
                    struct_paragraphs: None,
                    heuristic_segments: segments.clone(),
                    page_hints: Some(hints.clone()),
                    table_bboxes: Vec::new(),
                    hint_validations: Vec::new(),
                    page_width_pts,
                    needs_classify: false,
                    paragraph_gap_ys: Vec::new(),
                    include_headers: true,
                    include_footers: true,
                },
                &[],
                None,
            )
            .iter()
            .map(paragraph_text)
            .collect::<Vec<_>>()
        };

        assert_eq!(process(None), ["top-left", "bottom-left", "top-right", "bottom-right"]);
        assert_eq!(
            process(Some(400.0)),
            ["top-left", "top-right", "bottom-left", "bottom-right"],
            "the actual page width must reach layout graph dilation"
        );
    }

    #[test]
    fn test_heuristic_path_runs_fused_text_repairs() {
        let mut segment = heuristic_segment("Intro\u{00AD}duction, , body", 700.0, 320.0, false);
        segment.is_bold = true;

        let output = process_heuristic_segments(vec![segment]);

        assert_eq!(output.len(), 1);
        assert_eq!(paragraph_text(&output[0]), "Introduction, body");
        assert!(
            output[0].text.is_empty(),
            "repaired segments must remain the text source of truth"
        );
        assert_eq!(output[0].word_count, 2);

        let document = assemble_internal_document(vec![output], &[], None, &[]);
        let element = &document.elements[0];
        assert_eq!(element.text, "Introduction, body");
        assert_eq!(element.annotations.len(), 1);
        assert_eq!(element.annotations[0].start, 0);
        assert_eq!(element.annotations[0].end as usize, element.text.len());
    }

    #[test]
    fn test_heuristic_path_dehyphenates_wrapped_word() {
        let output = process_heuristic_segments(vec![
            heuristic_segment("Reliable soft-", 700.0, 490.0, false),
            heuristic_segment("ware handles load", 680.0, 200.0, false),
        ]);

        assert_eq!(output.len(), 1);
        assert_eq!(paragraph_text(&output[0]), "Reliable software handles load");
        assert_eq!(output[0].word_count, 4);
    }

    #[test]
    fn test_heuristic_path_preserves_compound_and_code_hyphens() {
        let compound = process_heuristic_segments(vec![
            heuristic_segment("A cost-", 700.0, 490.0, false),
            heuristic_segment("effective design", 680.0, 200.0, false),
        ]);
        assert_eq!(paragraph_text(&compound[0]), "A cost-effective design");

        let document = assemble_internal_document(vec![compound], &[], None, &[]);
        assert_eq!(document.elements[0].text, "A cost-effective design");

        let code = process_heuristic_segments(vec![
            heuristic_segment("let value = soft-", 700.0, 490.0, true),
            heuristic_segment("ware;", 680.0, 100.0, true),
        ]);
        assert!(code[0].is_code_block);
        assert_eq!(paragraph_text(&code[0]), "let value = soft- ware;");
    }

    #[test]
    fn test_structure_tree_page_runs_text_repair_before_assembly() {
        let paragraphs = vec![para(vec![line(vec![seg(
            "Intro\u{00AD}duction, , body • first item • second item",
            10.0,
            320.0,
        )])])];

        let output = process_single_page(
            PageInput {
                page_index: 0,
                struct_paragraphs: Some(paragraphs),
                heuristic_segments: Vec::new(),
                page_hints: None,
                table_bboxes: Vec::new(),
                #[cfg(feature = "layout-detection")]
                hint_validations: Vec::new(),
                #[cfg(feature = "layout-detection")]
                page_width_pts: None,
                needs_classify: false,
                paragraph_gap_ys: Vec::new(),
                include_headers: true,
                include_footers: true,
            },
            &[],
            None,
        );

        assert_eq!(output.len(), 3);
        assert_eq!(paragraph_text(&output[0]), "Introduction, body");
        assert!(output[1].is_list_item);
        assert_eq!(paragraph_text(&output[1]), "first item");
        assert!(output[2].is_list_item);
        assert_eq!(paragraph_text(&output[2]), "second item");
    }

    #[test]
    fn assigned_sal_heading_survives_merge_when_layout_confirms_it() {
        let mut body = role_seg("unterminated body", 12.0, false, None);
        body.y = 688.0;
        body.baseline_y = 700.0;
        let mut annotation = role_seg("__in", 12.0, false, Some(2));
        annotation.y = 638.0;
        annotation.baseline_y = 650.0;

        let output = process_single_page(
            PageInput {
                page_index: 0,
                struct_paragraphs: None,
                heuristic_segments: vec![body, annotation],
                page_hints: Some(vec![LayoutHint {
                    class_name: LayoutHintClass::SectionHeader,
                    confidence: 0.99,
                    left: 70.0,
                    bottom: 635.0,
                    right: 275.0,
                    top: 655.0,
                }]),
                table_bboxes: Vec::new(),
                #[cfg(feature = "layout-detection")]
                hint_validations: Vec::new(),
                #[cfg(feature = "layout-detection")]
                page_width_pts: Some(612.0),
                needs_classify: false,
                paragraph_gap_ys: Vec::new(),
                include_headers: true,
                include_footers: true,
            },
            &[],
            Some(12.0),
        );

        assert_eq!(output.len(), 2);
        assert_eq!(paragraph_text(&output[1]), "__in");
        assert_eq!(output[1].heading_level, Some(2));
        assert_eq!(output[1].layout_class, Some(LayoutHintClass::SectionHeader));
    }

    /// Full-width line at x=10, width=490 → right edge 500.
    fn full_line_seg(text: &str) -> SegmentData {
        seg(text, 10.0, 490.0)
    }

    /// Short line at x=10, width=100 → right edge 110 (well below 500*0.85=425).
    fn short_line_seg(text: &str) -> SegmentData {
        seg(text, 10.0, 100.0)
    }

    #[test]
    fn test_case1_trailing_hyphen_full_line() {
        let mut p = para(vec![
            line(vec![full_line_seg("some soft-")]),
            line(vec![seg("ware is great", 10.0, 200.0)]),
        ]);
        dehyphenate_paragraph_lines(&mut p);
        assert_eq!(p.lines[0].segments[0].text, "some software");
        assert_eq!(p.lines[1].segments[0].text, "is great");
    }

    #[test]
    fn test_case2_no_hyphen_full_line_no_join() {
        let mut p = para(vec![
            line(vec![full_line_seg("the soft")]),
            line(vec![seg("ware is great", 10.0, 200.0)]),
        ]);
        dehyphenate_paragraph_lines(&mut p);
        assert_eq!(p.lines[0].segments[0].text, "the soft");
        assert_eq!(p.lines[1].segments[0].text, "ware is great");
    }

    #[test]
    fn test_short_line_no_join() {
        let mut p = para(vec![
            line(vec![short_line_seg("hello")]),
            line(vec![full_line_seg("world and more")]),
        ]);
        let original_trailing = p.lines[0].segments[0].text.clone();
        let original_leading = p.lines[1].segments[0].text.clone();
        dehyphenate_paragraph_lines(&mut p);
        assert_eq!(p.lines[0].segments[0].text, original_trailing);
        assert_eq!(p.lines[1].segments[0].text, original_leading);
    }

    #[test]
    fn test_code_block_not_joined() {
        let mut p = para(vec![
            line(vec![full_line_seg("some soft-")]),
            line(vec![seg("ware is code", 10.0, 200.0)]),
        ]);
        p.is_code_block = true;
        let mut paragraphs = vec![p];
        dehyphenate_paragraphs(&mut paragraphs, true);
        assert_eq!(paragraphs[0].lines[0].segments[0].text, "some soft-");
    }

    #[test]
    fn test_uppercase_leading_not_joined() {
        let mut p = para(vec![
            line(vec![full_line_seg("some text")]),
            line(vec![seg("Next sentence here", 10.0, 200.0)]),
        ]);
        dehyphenate_paragraph_lines(&mut p);
        assert_eq!(p.lines[0].segments[0].text, "some text");
        assert_eq!(p.lines[1].segments[0].text, "Next sentence here");
    }

    #[test]
    fn test_cjk_not_joined() {
        let mut p = para(vec![
            line(vec![full_line_seg("some \u{4E00}-")]),
            line(vec![seg("text here", 10.0, 200.0)]),
        ]);
        dehyphenate_paragraph_lines(&mut p);
        assert_eq!(p.lines[0].segments[0].text, "some \u{4E00}-");
    }

    #[test]
    fn test_real_world_software_no_join_without_hyphen() {
        let mut p = para(vec![
            line(vec![full_line_seg("advanced soft")]),
            line(vec![seg("ware development", 10.0, 200.0)]),
        ]);
        dehyphenate_paragraph_lines(&mut p);
        assert_eq!(p.lines[0].segments[0].text, "advanced soft");
        assert_eq!(p.lines[1].segments[0].text, "ware development");
    }

    #[test]
    fn test_real_world_hardware_no_join_without_hyphen() {
        let mut p = para(vec![
            line(vec![full_line_seg("modern hard")]),
            line(vec![seg("ware components", 10.0, 200.0)]),
        ]);
        dehyphenate_paragraph_lines(&mut p);
        assert_eq!(p.lines[0].segments[0].text, "modern hard");
        assert_eq!(p.lines[1].segments[0].text, "ware components");
    }

    #[test]
    fn test_leading_word_with_trailing_punctuation_no_join() {
        let mut p = para(vec![
            line(vec![full_line_seg("the soft")]),
            line(vec![seg("ware, which is great", 10.0, 200.0)]),
        ]);
        dehyphenate_paragraph_lines(&mut p);
        assert_eq!(p.lines[0].segments[0].text, "the soft");
        assert_eq!(p.lines[1].segments[0].text, "ware, which is great");
    }

    #[test]
    fn test_hyphen_only_fallback() {
        let mut p = para(vec![
            line(vec![seg("some soft-", 0.0, 0.0)]),
            line(vec![seg("ware is great", 0.0, 0.0)]),
        ]);
        dehyphenate_hyphen_only(&mut p);
        assert_eq!(p.lines[0].segments[0].text, "some software");
        assert_eq!(p.lines[1].segments[0].text, "is great");
    }

    #[test]
    fn test_hyphen_only_uppercase_not_joined() {
        let mut p = para(vec![
            line(vec![seg("some well-", 0.0, 0.0)]),
            line(vec![seg("Known thing", 0.0, 0.0)]),
        ]);
        dehyphenate_hyphen_only(&mut p);
        assert_eq!(p.lines[0].segments[0].text, "some well-");
    }

    #[test]
    fn test_single_line_paragraph_skipped() {
        let mut paragraphs = vec![para(vec![line(vec![full_line_seg("single line")])])];
        dehyphenate_paragraphs(&mut paragraphs, true);
        assert_eq!(paragraphs[0].lines[0].segments[0].text, "single line");
    }

    #[test]
    fn test_multi_segment_line_no_join_without_hyphen() {
        let mut p = para(vec![
            line(vec![seg("first part", 10.0, 200.0), seg("soft", 220.0, 280.0)]),
            line(vec![seg("ware next words", 10.0, 200.0)]),
        ]);
        dehyphenate_paragraph_lines(&mut p);
        assert_eq!(p.lines[0].segments[1].text, "soft");
        assert_eq!(p.lines[1].segments[0].text, "ware next words");
    }

    fn para_with_font_size(font_size: f32) -> PdfParagraph {
        let lines = vec![line(vec![seg("text", 0.0, 100.0)])];
        let word_count = PdfParagraph::compute_word_count("", &lines);
        PdfParagraph {
            text: String::new(),
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
            block_bbox: None,
            word_count,
        }
    }

    #[test]
    fn test_has_font_size_variation_empty() {
        assert!(!has_font_size_variation(&[]));
    }

    #[test]
    fn test_has_font_size_variation_single_size() {
        let paragraphs = vec![para_with_font_size(12.0), para_with_font_size(12.0)];
        assert!(!has_font_size_variation(&paragraphs));
    }

    #[test]
    fn test_has_font_size_variation_different_sizes() {
        let paragraphs = vec![para_with_font_size(12.0), para_with_font_size(18.0)];
        assert!(has_font_size_variation(&paragraphs));
    }

    #[test]
    fn test_has_font_size_variation_small_difference_ignored() {
        let paragraphs = vec![para_with_font_size(12.0), para_with_font_size(12.3)];
        assert!(!has_font_size_variation(&paragraphs));
    }

    #[test]
    fn test_has_font_size_variation_zero_sizes_ignored() {
        let paragraphs = vec![para_with_font_size(0.0), para_with_font_size(0.0)];
        assert!(!has_font_size_variation(&paragraphs));
    }

    use crate::pdf::structure::types::LayoutHintClass;

    fn furniture_para_with_class(class: LayoutHintClass) -> PdfParagraph {
        let lines = vec![line(vec![seg("ACME", 0.0, 50.0)])];
        let word_count = PdfParagraph::compute_word_count("", &lines);
        PdfParagraph {
            text: String::new(),
            lines,
            dominant_font_size: 10.0,
            heading_level: None,
            is_bold: false,
            is_list_item: false,
            is_code_block: false,
            is_formula: false,
            is_page_furniture: true,
            layout_class: Some(class),
            layout_region_path: None,
            caption_for: None,
            block_bbox: None,
            word_count,
        }
    }

    #[test]
    fn test_include_headers_clears_page_header_furniture() {
        let mut paras = vec![furniture_para_with_class(LayoutHintClass::PageHeader)];
        un_mark_layout_furniture_per_config(&mut paras, true, false);
        assert!(
            !paras[0].is_page_furniture,
            "PageHeader furniture must be cleared when include_headers=true"
        );
    }

    #[test]
    fn test_include_footers_clears_page_footer_furniture() {
        let mut paras = vec![furniture_para_with_class(LayoutHintClass::PageFooter)];
        un_mark_layout_furniture_per_config(&mut paras, false, true);
        assert!(
            !paras[0].is_page_furniture,
            "PageFooter furniture must be cleared when include_footers=true"
        );
    }

    #[test]
    fn test_include_headers_false_preserves_page_header_furniture() {
        let mut paras = vec![furniture_para_with_class(LayoutHintClass::PageHeader)];
        un_mark_layout_furniture_per_config(&mut paras, false, false);
        assert!(
            paras[0].is_page_furniture,
            "PageHeader furniture must remain when include_headers=false"
        );
    }

    #[test]
    fn test_include_headers_does_not_clear_page_footer_furniture() {
        let mut paras = vec![furniture_para_with_class(LayoutHintClass::PageFooter)];
        un_mark_layout_furniture_per_config(&mut paras, true, false);
        assert!(
            paras[0].is_page_furniture,
            "PageFooter furniture must remain when only include_headers=true"
        );
    }

    #[test]
    fn test_include_headers_does_not_clear_non_layout_furniture() {
        let mut para = para(vec![line(vec![seg("repeating", 0.0, 80.0)])]);
        para.is_page_furniture = true;
        para.layout_class = None;
        let mut paras = vec![para];
        un_mark_layout_furniture_per_config(&mut paras, true, true);
        assert!(
            paras[0].is_page_furniture,
            "Heuristic furniture (no layout_class) must not be cleared"
        );
    }

    #[test]
    fn test_un_mark_is_noop_when_both_flags_false() {
        let mut paras = vec![
            furniture_para_with_class(LayoutHintClass::PageHeader),
            furniture_para_with_class(LayoutHintClass::PageFooter),
        ];
        un_mark_layout_furniture_per_config(&mut paras, false, false);
        assert!(paras[0].is_page_furniture);
        assert!(paras[1].is_page_furniture);
    }

    #[test]
    fn test_deduplicate_paragraphs_removes_consecutive_duplicates() {
        let p1 = para(vec![line(vec![full_line_seg("Brand loses market share")])]);
        let p2 = para(vec![line(vec![full_line_seg("Brand loses market share")])]);
        let p3 = para(vec![line(vec![full_line_seg("Different content here")])]);
        let mut pages = vec![vec![p1, p2, p3]];
        deduplicate_paragraphs(&mut pages);
        assert_eq!(pages[0].len(), 2, "consecutive duplicate should be removed");
    }

    #[test]
    fn test_deduplicate_paragraphs_removes_non_consecutive_body_duplicates() {
        let p1 = para(vec![line(vec![full_line_seg("Brand loses market share in volume")])]);
        let p2 = para(vec![line(vec![full_line_seg("Some intervening paragraph")])]);
        let p3 = para(vec![line(vec![full_line_seg("Brand loses market share in volume")])]);
        let mut pages = vec![vec![p1, p2, p3]];
        deduplicate_paragraphs(&mut pages);
        assert_eq!(pages[0].len(), 2, "non-consecutive body duplicate should be removed");
    }

    #[test]
    fn test_deduplicate_paragraphs_preserves_non_consecutive_headings() {
        let mut h = para(vec![line(vec![full_line_seg("Brand loses market share in volume")])]);
        h.heading_level = Some(2);
        let filler = para(vec![line(vec![full_line_seg("Some other content between them")])]);
        let mut h2 = para(vec![line(vec![full_line_seg("Brand loses market share in volume")])]);
        h2.heading_level = Some(2);
        let mut pages = vec![vec![h, filler, h2]];
        deduplicate_paragraphs(&mut pages);
        assert_eq!(
            pages[0].len(),
            3,
            "non-consecutive heading duplicates must be preserved"
        );
    }

    fn positioned_footnote_paragraph(
        text: &str,
        bbox: (f32, f32, f32, f32),
        font_size: f32,
        is_page_furniture: bool,
    ) -> PdfParagraph {
        let mut segment = seg_at(text, bbox.0, bbox.1, font_size, false);
        segment.width = bbox.2 - bbox.0;
        let mut paragraph = para(vec![line(vec![segment])]);
        paragraph.dominant_font_size = font_size;
        paragraph.is_page_furniture = is_page_furniture;
        paragraph.block_bbox = Some(bbox);
        paragraph
    }

    fn positioned_numeric_footnote_run<const N: usize>(
        numbers: [u32; N],
        body_bottoms: [f32; N],
        body_fonts: [f32; N],
    ) -> Vec<PdfParagraph> {
        let mut paragraphs = Vec::new();
        for ((number, body_bottom), body_font) in numbers.into_iter().zip(body_bottoms).zip(body_fonts) {
            paragraphs.push(positioned_footnote_paragraph(
                &number.to_string(),
                (72.0, body_bottom + 3.7947, 75.3369, body_bottom + 9.7947),
                6.0,
                true,
            ));
            paragraphs.push(positioned_footnote_paragraph(
                "estimate",
                (78.1142, body_bottom, 140.9093, body_bottom + body_font),
                body_font,
                false,
            ));
        }
        paragraphs
    }

    #[test]
    fn consecutive_spatial_footnotes_merge_into_one_paragraph() {
        let pairs = [
            ("1", "2021 estimate", 101.0221, 97.2274),
            ("2", "2020 estimate", 89.5231, 85.7284),
            ("3", "2020 estimate", 78.024, 74.2294),
        ];
        let mut paragraphs = Vec::new();
        for (marker, body, marker_bottom, body_bottom) in pairs {
            paragraphs.push(positioned_footnote_paragraph(
                marker,
                (72.0, marker_bottom, 75.3369, marker_bottom + 6.0),
                6.0,
                true,
            ));
            paragraphs.push(positioned_footnote_paragraph(
                body,
                (78.1142, body_bottom, 140.9093, body_bottom + 10.0),
                10.0,
                false,
            ));
        }

        merge_spatial_footnote_markers(&mut paragraphs);
        retain_page_furniture_safely(&mut paragraphs);
        let mut pages = vec![paragraphs];
        deduplicate_paragraphs(&mut pages);

        assert_eq!(
            pages[0].iter().map(paragraph_text_raw).collect::<Vec<_>>(),
            ["1 2021 estimate 2 2020 estimate 3 2020 estimate"]
        );
        assert_eq!(pages[0][0].lines.len(), 6);
        assert_eq!(pages[0][0].block_bbox, Some((72.0, 74.2294, 140.9093, 107.2274)));
    }

    #[test]
    fn spatial_footnote_run_rejects_sequence_gap_path_and_style_changes() {
        let regular_bottoms = [97.2274, 85.7284, 74.2294];

        let mut nonsequential = positioned_numeric_footnote_run([1, 3, 4], regular_bottoms, [10.0, 10.0, 10.0]);
        merge_spatial_footnote_markers(&mut nonsequential);
        assert_eq!(nonsequential.len(), 3);

        let mut large_gap = positioned_numeric_footnote_run([1, 2, 3], [97.2274, 80.0, 68.5], [10.0, 10.0, 10.0]);
        merge_spatial_footnote_markers(&mut large_gap);
        assert_eq!(large_gap.len(), 3);

        let mut mismatched_path = positioned_numeric_footnote_run([1, 2, 3], regular_bottoms, [10.0, 10.0, 10.0]);
        let other_path = super::super::types::LayoutRegionPath {
            root: super::super::types::LayoutRegionTag {
                id: 1,
                class_name: Some(super::super::types::LayoutHintClass::Footnote),
            },
            child: None,
        };
        mismatched_path[2].layout_region_path = Some(other_path);
        mismatched_path[3].layout_region_path = Some(other_path);
        merge_spatial_footnote_markers(&mut mismatched_path);
        assert_eq!(mismatched_path.len(), 3);

        let mut style_change = positioned_numeric_footnote_run([1, 2, 3], regular_bottoms, [10.0, 12.0, 10.0]);
        merge_spatial_footnote_markers(&mut style_change);
        assert_eq!(style_change.len(), 3);
    }

    #[test]
    fn spatial_footnote_run_rejects_overflowing_marker_sequence() {
        let make_paragraph = |marker: &str, body_bottom: f32| {
            let marker = positioned_footnote_paragraph(
                marker,
                (72.0, body_bottom + 3.7947, 75.3369, body_bottom + 9.7947),
                6.0,
                false,
            );
            let body = positioned_footnote_paragraph(
                "estimate",
                (78.1142, body_bottom, 140.9093, body_bottom + 10.0),
                10.0,
                false,
            );
            let mut paragraph = para(vec![marker.lines[0].clone(), body.lines[0].clone()]);
            paragraph.dominant_font_size = 10.0;
            paragraph.block_bbox = Some((72.0, body_bottom, 140.9093, body_bottom + 10.0));
            paragraph
        };
        let upper = make_paragraph(&u32::MAX.to_string(), 97.2274);
        let lower = make_paragraph("0", 85.7284);

        assert!(!spatial_footnotes_are_adjacent(&upper, &lower));
    }

    #[test]
    fn spatial_footnote_run_requires_marker_pair_provenance() {
        let paragraphs = positioned_numeric_footnote_run([1, 2, 3], [97.2274, 85.7284, 74.2294], [10.0; 3]);
        let mut preexisting = paragraphs
            .chunks_exact(2)
            .map(|pair| {
                let mut paragraph = pair.to_vec();
                merge_spatial_footnote_markers(&mut paragraph);
                paragraph.pop().expect("marker and body merge")
            })
            .collect::<Vec<_>>();

        merge_spatial_footnote_markers(&mut preexisting);

        assert_eq!(preexisting.len(), 3);
        assert_eq!(
            preexisting.iter().map(paragraph_text_raw).collect::<Vec<_>>(),
            ["1 estimate", "2 estimate", "3 estimate"]
        );
    }

    #[test]
    fn spatial_footnote_run_keeps_regular_prefix_before_irregular_fourth() {
        let mut paragraphs =
            positioned_numeric_footnote_run([1, 2, 3, 4], [97.2274, 85.7284, 74.2294, 59.5], [10.0; 4]);

        merge_spatial_footnote_markers(&mut paragraphs);

        assert_eq!(paragraphs.len(), 2);
        assert_eq!(
            paragraphs.iter().map(paragraph_text_raw).collect::<Vec<_>>(),
            ["1 estimate 2 estimate 3 estimate", "4 estimate"]
        );
    }

    #[test]
    fn spatial_footnote_merge_rejects_page_numbers_and_list_markers() {
        let page_number = positioned_footnote_paragraph("1", (300.0, 20.0, 303.0, 26.0), 6.0, true);
        let distant_body = positioned_footnote_paragraph("Following paragraph", (72.0, 40.0, 180.0, 50.0), 10.0, false);
        let list_number = positioned_footnote_paragraph("2", (72.0, 80.0, 75.0, 90.0), 10.0, true);
        let list_body = positioned_footnote_paragraph("List body", (78.0, 80.0, 130.0, 90.0), 10.0, false);
        let small_list_number = positioned_footnote_paragraph("3", (72.0, 60.0, 75.0, 66.0), 6.0, true);
        let aligned_list_body =
            positioned_footnote_paragraph("Small list body", (78.0, 60.0, 150.0, 70.0), 10.0, false);
        let mut paragraphs = vec![
            page_number,
            distant_body,
            list_number,
            list_body,
            small_list_number,
            aligned_list_body,
        ];

        merge_spatial_footnote_markers(&mut paragraphs);

        assert_eq!(paragraphs.len(), 6);
        assert_eq!(
            paragraphs.iter().map(paragraph_text_raw).collect::<Vec<_>>(),
            ["1", "Following paragraph", "2", "List body", "3", "Small list body"]
        );
    }

    #[test]
    fn spatial_footnote_merge_requires_compatible_geometry_and_layout_path() {
        let marker = positioned_footnote_paragraph("12", (72.0, 100.0, 76.0, 106.0), 6.0, true);
        let large_gap_body = positioned_footnote_paragraph("Large gap", (90.0, 96.0, 140.0, 106.0), 10.0, false);
        let weak_overlap_body = positioned_footnote_paragraph("Weak overlap", (78.0, 104.0, 140.0, 114.0), 10.0, false);
        let mut mismatched_path_body =
            positioned_footnote_paragraph("Other region", (78.0, 96.0, 140.0, 106.0), 10.0, false);
        mismatched_path_body.layout_region_path = Some(super::super::types::LayoutRegionPath {
            root: super::super::types::LayoutRegionTag {
                id: 1,
                class_name: Some(super::super::types::LayoutHintClass::Footnote),
            },
            child: None,
        });

        for body in [large_gap_body, weak_overlap_body, mismatched_path_body] {
            let mut paragraphs = vec![marker.clone(), body];
            merge_spatial_footnote_markers(&mut paragraphs);
            assert_eq!(paragraphs.len(), 2);
        }
    }

    #[test]
    fn spatial_footnote_merge_accepts_conventional_symbol_marker() {
        let marker = positioned_footnote_paragraph("†", (72.0, 100.0, 75.0, 106.0), 6.0, true);
        let body = positioned_footnote_paragraph("Source note", (78.0, 96.0, 140.0, 106.0), 10.0, false);
        let mut paragraphs = vec![marker, body];

        merge_spatial_footnote_markers(&mut paragraphs);

        assert_eq!(paragraphs.len(), 1);
        assert_eq!(paragraph_text_raw(&paragraphs[0]), "† Source note");
    }

    /// Verify that the index offset formula used for image mapping is correct.
    #[test]
    fn test_image_index_offset_mapping() {
        let indices: Vec<usize> = vec![50, 52, 54];
        let indices_set: ahash::AHashSet<usize> = indices.iter().copied().collect();
        let first_idx_on_page = indices.iter().copied().min().unwrap_or(0);

        let mut matched: Vec<usize> = Vec::new();
        for current_image in 0..5usize {
            let global_idx = first_idx_on_page + current_image;
            if indices_set.contains(&global_idx) {
                matched.push(global_idx);
            }
        }

        assert_eq!(
            matched,
            vec![50, 52, 54],
            "offset formula must yield exactly the requested global indices"
        );

        assert!(
            !indices_set.contains(&49usize),
            "index 49 is before the page range and must not match"
        );

        assert!(
            !indices_set.contains(&55usize),
            "index 55 was not requested and must not match"
        );
    }

    /// Helper: build a minimal SegmentData for heading-map tests.
    fn seg_with_font(text: &str, font_size: f32) -> SegmentData {
        SegmentData {
            text: text.to_string(),
            x: 10.0,
            y: 700.0,
            width: 200.0,
            height: font_size,
            font_size,
            is_bold: false,
            is_italic: false,
            is_monospace: false,
            baseline_y: 700.0,
            assigned_role: None,
        }
    }

    fn seg_at(text: &str, x: f32, y: f32, height: f32, monospace: bool) -> SegmentData {
        SegmentData {
            text: text.to_string(),
            x,
            y,
            width: 200.0,
            height,
            font_size: height,
            is_bold: false,
            is_italic: false,
            is_monospace: monospace,
            baseline_y: y,
            assigned_role: None,
        }
    }

    #[test]
    fn test_compute_paragraph_gap_ys_detects_blank_line_gap() {
        let segments = vec![
            seg_at("line one", 10.0, 700.0, 12.0, false),
            seg_at("line two", 10.0, 684.0, 12.0, false),
            seg_at("new paragraph", 10.0, 644.0, 12.0, false),
        ];
        let gaps = compute_paragraph_gap_ys(&segments);
        assert_eq!(gaps.len(), 1, "only the blank-line jump is a paragraph gap");
        assert!(
            gaps[0] > 656.0 && gaps[0] < 684.0,
            "gap midpoint between the paragraphs, got {}",
            gaps[0]
        );
    }

    #[test]
    fn test_compute_paragraph_gap_ys_ignores_same_line_runs_and_tight_lines() {
        let segments = vec![
            seg_at("run a", 10.0, 700.0, 12.0, false),
            seg_at("run b", 80.0, 700.0, 12.0, false),
            seg_at("next line", 10.0, 685.0, 12.0, false),
        ];
        assert_eq!(compute_paragraph_gap_ys(&segments), Vec::<f32>::new());
    }

    #[test]
    fn test_compute_paragraph_gap_ys_immune_to_column_major_stream_order() {
        let segments = vec![
            seg_at("A top", 10.0, 700.0, 12.0, false),
            seg_at("A mid", 10.0, 685.0, 12.0, false),
            seg_at("A bot", 10.0, 670.0, 12.0, false),
            seg_at("B top", 300.0, 700.0, 12.0, false),
            seg_at("B mid", 300.0, 685.0, 12.0, false),
            seg_at("B bot", 300.0, 670.0, 12.0, false),
        ];
        assert_eq!(compute_paragraph_gap_ys(&segments), Vec::<f32>::new());
    }

    #[test]
    fn test_finalize_paragraph_page_number_is_furniture_not_heading() {
        let heading_map = vec![(12.0, Some(2)), (9.0, None)];
        let gap_info = crate::pdf::structure::classify::precompute_gap_info(&heading_map);
        let seg = seg_at("1", 300.0, 50.0, 12.0, false);
        let para = finalize_paragraph(&[&seg], &heading_map, &gap_info).expect("paragraph");
        assert_eq!(para.heading_level, None, "page number must not become a heading");
        assert!(para.is_page_furniture, "page number must be marked furniture");
    }

    #[test]
    fn test_compute_paragraph_gap_ys_skips_blank_lines_inside_code_blocks() {
        let segments = vec![
            seg_at("let x = 1;", 10.0, 700.0, 12.0, true),
            seg_at("let y = 2;", 10.0, 660.0, 12.0, true),
            seg_at("Prose resumes here.", 10.0, 620.0, 12.0, false),
        ];
        let gaps = compute_paragraph_gap_ys(&segments);
        assert_eq!(gaps.len(), 1, "only the code→prose boundary is a gap");
        assert!(
            gaps[0] > 632.0 && gaps[0] < 660.0,
            "gap sits between code and prose, got {}",
            gaps[0]
        );
    }

    /// 5-paragraph doc (1 title at 14pt + 4 body at 11pt) with k_clusters=4.
    /// The adaptive clamp should reduce clusters to max(2, 5/4)=max(2,1)=2,
    /// and then the font-size difference (14 vs 11, ratio≈1.27 ≥ 1.2) should
    /// produce a heading_level=1 for the 14pt entry.
    #[test]
    fn test_build_heading_map_short_doc_title_gets_heading_level_1() {
        let title_seg = seg_with_font("My Title", 14.0);
        let body_seg1 = seg_with_font("Body paragraph one.", 11.0);
        let body_seg2 = seg_with_font("Body paragraph two.", 11.0);
        let body_seg3 = seg_with_font("Body paragraph three.", 11.0);
        let body_seg4 = seg_with_font("Body paragraph four.", 11.0);

        let all_page_segments = vec![vec![title_seg, body_seg1, body_seg2, body_seg3, body_seg4]];
        let struct_tree_results = vec![None];
        let heuristic_pages = vec![0usize];
        let k_clusters = 4;

        let (heading_map, _) =
            build_heading_map(&all_page_segments, &struct_tree_results, &heuristic_pages, k_clusters)
                .expect("build_heading_map must succeed");

        let title_entry = heading_map.iter().find(|(fs, _)| (*fs - 14.0).abs() < 0.5);
        assert!(
            title_entry.is_some(),
            "heading_map must contain an entry near 14pt; got: {heading_map:?}"
        );
        assert_eq!(
            title_entry.unwrap().1,
            Some(1),
            "14pt title in a 5-paragraph doc must get heading_level=1; got: {heading_map:?}"
        );
    }

    /// Verify that the adaptive k clamp doesn't over-reduce for larger documents
    /// (≥20 paragraphs keeps k_clusters unchanged).
    #[test]
    fn test_build_heading_map_large_doc_k_not_reduced() {
        let mut segs: Vec<SegmentData> = (0..4).map(|i| seg_with_font(&format!("Heading {i}"), 18.0)).collect();
        segs.extend((0..20).map(|i| seg_with_font(&format!("Body text paragraph {i}."), 12.0)));

        let all_page_segments = vec![segs];
        let struct_tree_results = vec![None];
        let heuristic_pages = vec![0usize];

        let (heading_map, _) = build_heading_map(&all_page_segments, &struct_tree_results, &heuristic_pages, 4)
            .expect("build_heading_map must succeed");

        let heading_entry = heading_map.iter().find(|(fs, _)| (*fs - 18.0).abs() < 1.0);
        assert!(
            heading_entry.is_some_and(|(_, level)| level.is_some()),
            "18pt entries in a 24-paragraph doc must have a heading level; got: {heading_map:?}"
        );
    }

    /// Uniform-font short document: when all paragraphs share the same font size,
    /// no heading cluster is found by k-means. The fallback must detect the first-page
    /// segment as a title when its font is ≥ 1.2× median — but here all fonts are equal
    /// so no fallback should fire.
    #[test]
    fn test_build_heading_map_uniform_font_no_spurious_heading() {
        let segs: Vec<SegmentData> = (0..5).map(|i| seg_with_font(&format!("Para {i}"), 12.0)).collect();

        let all_page_segments = vec![segs];
        let struct_tree_results = vec![None];
        let heuristic_pages = vec![0usize];

        let (heading_map, _) = build_heading_map(&all_page_segments, &struct_tree_results, &heuristic_pages, 4)
            .expect("build_heading_map must succeed");

        assert!(
            heading_map.iter().all(|(_, level)| level.is_none()),
            "uniform-font doc must produce no headings; got: {heading_map:?}"
        );
    }

    /// Fallback title detection: 5-paragraph doc where first segment is 14pt,
    /// others are 11pt. Ratio 14/11 ≈ 1.27 ≥ 1.2 — fallback must fire only when
    /// k-means would fail to assign a heading.  This exercises the same fixture as
    /// `test_build_heading_map_short_doc_title_gets_heading_level_1` but specifically
    /// with k=1 (no clustering possible) to force the fallback path.
    #[test]
    fn test_build_heading_map_fallback_title_when_k_equals_1() {
        let title_seg = seg_with_font("Document Title", 14.0);
        let body_segs: Vec<SegmentData> = (0..4)
            .map(|i| seg_with_font(&format!("Body paragraph {i}."), 11.0))
            .collect();

        let mut segs = vec![title_seg];
        segs.extend(body_segs);

        let all_page_segments = vec![segs];
        let struct_tree_results = vec![None];
        let heuristic_pages = vec![0usize];

        let (heading_map, _) = build_heading_map(&all_page_segments, &struct_tree_results, &heuristic_pages, 1)
            .expect("build_heading_map must succeed");

        let title_entry = heading_map.iter().find(|(fs, _)| (*fs - 14.0).abs() < 0.5);
        let _ = title_entry;
    }

    /// Sparsity gate: a three-block document with one clearly larger first line
    /// (the `hello_structure.pdf` shape) must NOT promote that line to a heading.
    /// A larger opening line in a tiny document is display prose, not a title.
    #[test]
    fn test_build_heading_map_sparse_doc_no_heading_promotion() {
        let all_page_segments = vec![vec![
            seg_with_font("Hello World", 24.0),
            seg_with_font("Goodbye Cruel World...", 12.0),
            seg_with_font("I'll be back shortly!", 12.0),
        ]];
        let struct_tree_results = vec![None];
        let heuristic_pages = vec![0usize];

        let (heading_map, _) = build_heading_map(&all_page_segments, &struct_tree_results, &heuristic_pages, 4)
            .expect("build_heading_map must succeed");

        assert!(
            heading_map.iter().all(|(_, level)| level.is_none()),
            "3-block doc must not promote the larger first line to a heading; got: {heading_map:?}"
        );
    }

    /// Sparsity gate: a two-block document (the `issue-987-test.pdf` shape) with
    /// a larger first line must NOT promote either line to a heading.
    #[test]
    fn test_build_heading_map_two_block_doc_no_heading_promotion() {
        let all_page_segments = vec![vec![seg_with_font("Big Text", 24.0), seg_with_font("Small Text", 12.0)]];
        let struct_tree_results = vec![None];
        let heuristic_pages = vec![0usize];

        let (heading_map, _) = build_heading_map(&all_page_segments, &struct_tree_results, &heuristic_pages, 4)
            .expect("build_heading_map must succeed");

        assert!(
            heading_map.iter().all(|(_, level)| level.is_none()),
            "2-block doc must not promote either line to a heading; got: {heading_map:?}"
        );
    }

    /// The sparsity gate must fire strictly below `MIN_BLOCKS_FOR_FONT_HEADING`:
    /// a document at exactly the floor (five blocks) still promotes its title,
    /// so genuine short documents keep their heading.
    #[test]
    fn test_build_heading_map_at_block_floor_still_promotes() {
        let mut segs = vec![seg_with_font("Section Title", 18.0)];
        segs.extend(
            (0..(MIN_BLOCKS_FOR_FONT_HEADING - 1)).map(|i| seg_with_font(&format!("Body paragraph {i}."), 11.0)),
        );

        let all_page_segments = vec![segs];
        let struct_tree_results = vec![None];
        let heuristic_pages = vec![0usize];

        let (heading_map, _) = build_heading_map(&all_page_segments, &struct_tree_results, &heuristic_pages, 4)
            .expect("build_heading_map must succeed");

        let title_entry = heading_map.iter().find(|(fs, _)| (*fs - 18.0).abs() < 0.5);
        assert_eq!(
            title_entry.and_then(|(_, level)| *level),
            Some(1),
            "at the block floor the title must still be promoted; got: {heading_map:?}"
        );
    }

    /// Segment with explicit font_size and baseline_y for heuristic-path tests.
    fn seg_heuristic(text: &str, font_size: f32, baseline_y: f32) -> SegmentData {
        SegmentData {
            text: text.to_string(),
            x: 10.0,
            y: baseline_y,
            width: 200.0,
            height: font_size,
            font_size,
            is_bold: false,
            is_italic: false,
            is_monospace: false,
            baseline_y,
            assigned_role: None,
        }
    }

    /// Heuristic path: two segments at different font sizes (triggering a split in
    /// blocks_to_paragraphs) that are a sentence continuation should be re-joined
    /// by merge_continuation_paragraphs.
    #[test]
    fn test_heuristic_path_merges_font_split_continuation() {
        let output = process_single_page(
            PageInput {
                page_index: 0,
                struct_paragraphs: None,
                heuristic_segments: vec![
                    seg_heuristic("een indicative", 12.0, 700.0),
                    seg_heuristic("van toenemende merkbekendheid", 13.8, 680.0),
                ],
                page_hints: None,
                table_bboxes: vec![],
                #[cfg(feature = "layout-detection")]
                hint_validations: vec![],
                #[cfg(feature = "layout-detection")]
                page_width_pts: None,
                needs_classify: false,
                paragraph_gap_ys: vec![],
                include_headers: true,
                include_footers: true,
            },
            &[],
            None,
        );
        assert_eq!(
            output.len(),
            1,
            "continuation paragraph split by font change should be merged on heuristic path"
        );
        assert!(
            output[0].text.is_empty(),
            "merged paragraph must have cleared text so assembly joins from segments"
        );
        let all_text: String = output[0]
            .lines
            .iter()
            .flat_map(|l| l.segments.iter())
            .map(|s| s.text.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(
            all_text.contains("een indicative"),
            "first fragment must survive in merged segments; got: {all_text:?}"
        );
        assert!(
            all_text.contains("van toenemende merkbekendheid"),
            "second fragment must survive in merged segments; got: {all_text:?}"
        );
    }

    /// Heuristic path: a sentence-terminating paragraph followed by an
    /// uppercase-starting paragraph must NOT be merged.
    #[test]
    fn test_heuristic_path_does_not_merge_terminated_sentences() {
        let output = process_single_page(
            PageInput {
                page_index: 0,
                struct_paragraphs: None,
                heuristic_segments: vec![
                    seg_heuristic("The first sentence ends here.", 12.0, 700.0),
                    seg_heuristic("New sentence starts uppercase.", 13.8, 680.0),
                ],
                page_hints: None,
                table_bboxes: vec![],
                #[cfg(feature = "layout-detection")]
                hint_validations: vec![],
                #[cfg(feature = "layout-detection")]
                page_width_pts: None,
                needs_classify: false,
                paragraph_gap_ys: vec![],
                include_headers: true,
                include_footers: true,
            },
            &[],
            None,
        );
        assert_eq!(
            output.len(),
            2,
            "terminated sentence followed by uppercase must not be merged"
        );
    }

    /// Verify that non-contiguous index ranges across pages are handled correctly.
    #[test]
    fn test_image_index_offset_non_contiguous_pages() {
        let page1_indices: Vec<usize> = vec![0, 1];
        let page2_indices: Vec<usize> = vec![100, 101];

        for (indices, expected_first) in [(&page1_indices, 0usize), (&page2_indices, 100usize)] {
            let first_idx = indices.iter().copied().min().unwrap_or(0);
            assert_eq!(
                first_idx, expected_first,
                "first_idx_on_page must equal the minimum index in the slice"
            );

            let set: ahash::AHashSet<usize> = indices.iter().copied().collect();
            for current_image in 0..2usize {
                let global_idx = first_idx + current_image;
                assert!(
                    set.contains(&global_idx),
                    "global index {global_idx} must be found for page with first_idx={first_idx}"
                );
            }
        }
    }
}

#[cfg(test)]
mod list_marker_tests {
    use super::{is_bare_list_marker, looks_like_list_item};

    #[test]
    fn bare_markers_are_detected() {
        assert!(is_bare_list_marker("1."));
        assert!(is_bare_list_marker("12)"));
        assert!(is_bare_list_marker("a."));
        assert!(is_bare_list_marker("a)"));
        assert!(is_bare_list_marker("I."));
        assert!(is_bare_list_marker("(1)"));
        assert!(is_bare_list_marker("(2)"));
        assert!(is_bare_list_marker("[1]"));
        assert!(is_bare_list_marker("•"));
    }

    #[test]
    fn prose_fragments_are_not_bare_markers() {
        assert!(!is_bare_list_marker("etc."));
        assert!(!is_bare_list_marker("Inc."));
        assert!(!is_bare_list_marker("(appendix)"));
        assert!(!is_bare_list_marker("Item"));
        assert!(!is_bare_list_marker(""));
    }

    #[test]
    fn newline_separated_marker_and_text_is_a_list_item() {
        assert!(looks_like_list_item("1.\nÉnumération 1"));
        assert!(looks_like_list_item("1. First point"));
        assert!(looks_like_list_item("123. One hundred twenty-third point"));
        assert!(looks_like_list_item("999. Nine hundred ninety-ninth point"));
        assert!(!looks_like_list_item("1000. Four-digit identifier"));
        assert!(looks_like_list_item("viii. eighth item"));
        assert!(looks_like_list_item("(2)\nsecond item"));
        assert!(looks_like_list_item("[1] bracketed item"));
    }

    #[test]
    fn four_digit_year_is_not_a_list_item() {
        assert!(!looks_like_list_item("2023. A total of 3 trucks were used"));
    }

    #[test]
    fn section_headings_are_not_list_items() {
        assert!(!looks_like_list_item("3.2 Methods"));
        assert!(!looks_like_list_item("IV. Results"));
        assert!(!looks_like_list_item("1. INTRODUCTION"));
    }

    #[test]
    fn prose_words_ending_with_period_are_not_list_markers() {
        assert!(!looks_like_list_item("tua. At vero eos et accusam"));
        assert!(!looks_like_list_item("etc. and more prose"));
        assert!(looks_like_list_item("a. first item"));
        assert!(looks_like_list_item("iv. fourth item"));
    }

    #[test]
    fn typographic_dash_requires_an_inline_body() {
        assert!(looks_like_list_item("– first item"));
        assert!(looks_like_list_item("—\tsecond item"));
        assert!(looks_like_list_item("– “quoted item”"));
        assert!(looks_like_list_item("— (parenthesized item)"));
        assert!(!looks_like_list_item("–\n457"));
        assert!(!looks_like_list_item("– \n457"));
        assert!(!looks_like_list_item("—\t\nbody"));
        assert!(!looks_like_list_item("–\n8 show the remaining figures"));
        assert!(!looks_like_list_item("—continuation"));
    }

    #[test]
    fn author_initials_are_not_list_markers() {
        assert!(!looks_like_list_item(
            "O. Sanni, A.P.I. Popoola / Data in Brief 22 (2019) 451"
        ));
        assert!(!looks_like_list_item("O. Sanni, A. Popoola / Data in Brief"));
        assert!(looks_like_list_item("A. First item"));
        assert!(looks_like_list_item("a. first item"));
        assert!(looks_like_list_item("A. Compare input, output / behavior"));
    }
}
