//! Per-page bounding-box aggregation for chunk `page_spans` (#1295) and node-id backreferences
//! (#1296).
//!
//! [`crate::chunking::boundaries::calculate_page_spans`] derives the *page numbers*
//! a chunk covers from byte-range overlap against [`PageBoundary`](crate::types::PageBoundary)
//! markers — the same mechanism already used for `first_page`/`last_page`. This module adds two
//! things once a document's structured node tree ([`DocumentStructure`]) is available:
//! [`populate_page_span_bboxes`] fills each [`PageSpan`](crate::types::PageSpan)'s `bbox` with the
//! union of the bounding boxes of the body-layer nodes on that page whose text appears in the
//! chunk, and — reusing the same node/chunk match — collects those nodes' ids into the chunk's
//! [`ChunkMetadata::node_ids`](crate::types::ChunkMetadata::node_ids).
//!
//! There is currently no byte-offset mapping from rendered output back to individual
//! [`DocumentNode`](crate::types::document_structure::DocumentNode)s (tracked under #1294;
//! see [`DocumentStructure::node_rendered_offset`](crate::types::document_structure::DocumentStructure::node_rendered_offset)).
//! In its absence, node-to-chunk membership on a given page is determined by a textual
//! containment check — the same substring-matching approach
//! [`locate_page_boundaries`](crate::core::pipeline::features) already uses to align raw page
//! text with rendered content — rather than a byte-exact intersection.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};

use crate::types::document_structure::{ContentLayer, DocumentNode, DocumentStructure, NodeContent};
use crate::types::{BoundingBox, Chunk};

/// Minimum trimmed node-text length considered for containment matching.
///
/// Short text (e.g. a label, a number like "1.2.", or a common word like "Data") is likely to
/// appear verbatim in chunks it has nothing to do with, which would pollute that page's union
/// bbox with an unrelated node's coordinates. Ten characters is long enough that a coincidental
/// substring match across unrelated content is unlikely, while still covering most short
/// headings and list items. Nodes with shorter text are skipped for bbox aggregation — their
/// page still appears in `page_spans`, just without a bbox contribution from that node.
const MIN_NODE_TEXT_MATCH_LEN: usize = 10;

/// Fill in `bbox` on each chunk's `page_spans`, and `metadata.node_ids`, using the document's
/// structured node tree.
///
/// For every chunk and every [`PageSpan`](crate::types::PageSpan) already present on it (as
/// produced by `calculate_page_spans`), this unions the bounding boxes of all body-layer nodes
/// that:
/// - fall on that page (`node.page..=node.page_end` covers the span's page), and
/// - carry a `bbox`, and
/// - carry matching text (see [`node_text_for_matching`]) found verbatim within the chunk's
///   `content`.
///
/// The same per-page match additionally collects the `id` of every matching body-layer node
/// (regardless of whether it carries a `bbox`) into the chunk's
/// [`ChunkMetadata::node_ids`](crate::types::ChunkMetadata::node_ids), deduplicated and in
/// node-traversal order, without a second sweep over nodes × chunks.
///
/// Chunks with empty `page_spans` (no page-boundary provenance) are skipped entirely — their
/// `node_ids` stays empty. A span's `bbox` stays `None` when no node in `structure` matches.
///
/// Nodes are bucketed by page once, up front (see [`index_body_nodes_by_page`]), so each
/// chunk/page_span pair only scans the (typically small) set of nodes on its own page instead of
/// the entire node tree — this keeps the cost roughly linear in document size rather than
/// quadratic in `chunks.len() * structure.nodes.len()`.
pub(crate) fn populate_page_span_bboxes(chunks: &mut [Chunk], structure: &DocumentStructure) {
    let nodes_by_page = index_body_nodes_by_page(&structure.nodes);

    for chunk in chunks.iter_mut() {
        if chunk.metadata.page_spans.is_empty() {
            continue;
        }

        let mut matched_node_ids = Vec::new();
        for span in chunk.metadata.page_spans.iter_mut() {
            let Some(nodes) = nodes_by_page.get(&span.page) else {
                span.bbox = None;
                continue;
            };
            let (bbox, ids) = match_page_nodes(&chunk.content, nodes);
            span.bbox = bbox;
            matched_node_ids.extend(ids);
        }
        chunk.metadata.node_ids = dedup_preserve_order(matched_node_ids);
    }
}

/// Bucket body-layer nodes by every page they cover.
///
/// A node with `page_end` set (spanning pages `page..=page_end`) is indexed under every page in
/// that range, matching the membership test `populate_page_span_bboxes` used before this index
/// existed (`node.page..=node.page_end` covers the span's page). Non-body-layer nodes (headers,
/// footers, footnotes) and nodes without a `page` are omitted entirely, since they never
/// contribute to a bbox union.
fn index_body_nodes_by_page(nodes: &[DocumentNode]) -> HashMap<u32, Vec<&DocumentNode>> {
    let mut index: HashMap<u32, Vec<&DocumentNode>> = HashMap::new();

    for node in nodes {
        if node.content_layer != ContentLayer::Body {
            continue;
        }
        let Some(start) = node.page else { continue };
        let end = node.page_end.unwrap_or(start);

        for page in start..=end {
            index.entry(page).or_default().push(node);
        }
    }

    index
}

/// Match all `nodes` (already scoped to a single page) whose text is found in `chunk_content`,
/// returning both the union of their bounding boxes and the ids of the matched nodes.
///
/// A node contributes its `id` whenever its text matches, independent of whether it carries a
/// `bbox` — `node_ids` link chunks back to structure nodes and shouldn't be gated on layout
/// coordinates being available.
fn match_page_nodes<'a>(chunk_content: &str, nodes: &[&'a DocumentNode]) -> (Option<BoundingBox>, Vec<&'a str>) {
    let mut union = None;
    let mut ids = Vec::new();

    for node in nodes.iter().filter(|node| node_text_matches_chunk(node, chunk_content)) {
        if let Some(bbox) = node.bbox {
            union = Some(union_bbox(union, bbox));
        }
        if !node.id.is_empty() {
            ids.push(node.id.as_str());
        }
    }

    (union, ids)
}

/// Deduplicate `ids` while preserving the order of first occurrence, allocating each retained id
/// exactly once. The borrowed `&str` inputs are tied to the document's node tree, so the set can
/// test membership without cloning.
fn dedup_preserve_order(ids: Vec<&str>) -> Vec<String> {
    let mut seen = HashSet::with_capacity(ids.len());
    ids.into_iter()
        .filter(|id| seen.insert(*id))
        .map(str::to_string)
        .collect()
}

/// Check whether any of `node`'s matchable text fragments (see [`node_text_for_matching`])
/// appear verbatim in `chunk_content`, after trimming and enforcing [`MIN_NODE_TEXT_MATCH_LEN`].
fn node_text_matches_chunk(node: &DocumentNode, chunk_content: &str) -> bool {
    node_text_for_matching(&node.content).into_iter().any(|text| {
        let trimmed = text.trim();
        trimmed.len() >= MIN_NODE_TEXT_MATCH_LEN && chunk_content.contains(trimmed)
    })
}

/// Extract the text fragment(s) of a node usable for chunk-containment matching.
///
/// Most node kinds carry a single, directly comparable text string. `Table` instead yields
/// one fragment per cell: a table's rendered form (pipe-delimited rows with separators) never
/// appears as a single contiguous run of its cell text, so matching per-cell lets a table-only
/// chunk still contribute a bbox/node_id when at least one of its cells appears verbatim in the
/// chunk (#212). `DefinitionItem` yields its term and definition as separate fragments for the
/// same reason — the rendered form is unlikely to place them contiguously.
///
/// Pure container nodes (`List`, `Quote`, `DefinitionList`, `PageBreak`, `RawBlock`,
/// `MetadataBlock`) have no comparable text of their own and yield nothing; their children
/// (which are matched individually) carry the text.
fn node_text_for_matching(content: &NodeContent) -> Vec<Cow<'_, str>> {
    match content {
        NodeContent::Title { text }
        | NodeContent::Heading { text, .. }
        | NodeContent::Paragraph { text }
        | NodeContent::ListItem { text }
        | NodeContent::Code { text, .. }
        | NodeContent::Formula { text }
        | NodeContent::Footnote { text } => vec![Cow::Borrowed(text.as_str())],
        NodeContent::Citation { text, .. } => vec![Cow::Borrowed(text.as_str())],
        NodeContent::Image { description, .. } => description.as_deref().map(Cow::Borrowed).into_iter().collect(),
        NodeContent::Group { heading_text, .. } => heading_text.as_deref().map(Cow::Borrowed).into_iter().collect(),
        NodeContent::Slide { title, .. } => title.as_deref().map(Cow::Borrowed).into_iter().collect(),
        NodeContent::DefinitionItem { term, definition } => {
            vec![Cow::Borrowed(term.as_str()), Cow::Borrowed(definition.as_str())]
        }
        NodeContent::Admonition { title, .. } => title.as_deref().map(Cow::Borrowed).into_iter().collect(),
        NodeContent::Table { grid } => grid
            .cells
            .iter()
            .map(|cell| Cow::Borrowed(cell.content.as_str()))
            .collect(),
        _ => Vec::new(),
    }
}

/// Union two bounding boxes into the smallest box that encloses both, or just `next` if `acc` is
/// absent.
fn union_bbox(acc: Option<BoundingBox>, next: BoundingBox) -> BoundingBox {
    match acc {
        None => next,
        Some(acc) => BoundingBox {
            x0: acc.x0.min(next.x0),
            y0: acc.y0.min(next.y0),
            x1: acc.x1.max(next.x1),
            y1: acc.y1.max(next.y1),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ChunkMetadata, ChunkType, PageSpan};

    fn body_node(text: &str, page: u32, bbox: Option<BoundingBox>) -> DocumentNode {
        DocumentNode {
            id: String::new(),
            content: NodeContent::Paragraph { text: text.to_string() },
            parent: None,
            children: Vec::new(),
            content_layer: ContentLayer::Body,
            page: Some(page),
            page_end: None,
            bbox,
            annotations: Vec::new(),
            attributes: None,
        }
    }

    fn body_node_with_id(id: &str, text: &str, page: u32, bbox: Option<BoundingBox>) -> DocumentNode {
        DocumentNode {
            id: id.to_string(),
            content: NodeContent::Paragraph { text: text.to_string() },
            parent: None,
            children: Vec::new(),
            content_layer: ContentLayer::Body,
            page: Some(page),
            page_end: None,
            bbox,
            annotations: Vec::new(),
            attributes: None,
        }
    }

    fn bbox(x0: f64, y0: f64, x1: f64, y1: f64) -> BoundingBox {
        BoundingBox { x0, y0, x1, y1 }
    }

    fn chunk_with_spans(content: &str, spans: Vec<PageSpan>) -> Chunk {
        Chunk {
            content: content.to_string(),
            chunk_type: ChunkType::default(),
            embedding: None,
            sparse_embedding: None,
            late_interaction: None,
            metadata: ChunkMetadata {
                byte_start: 0,
                byte_end: content.len(),
                token_count: None,
                chunk_index: 0,
                total_chunks: 1,
                first_page: spans.first().map(|s| s.page),
                last_page: spans.last().map(|s| s.page),
                heading_context: None,
                heading_path: Vec::new(),
                image_indices: Vec::new(),
                node_ids: Vec::new(),
                page_spans: spans,
                classifications: Vec::new(),
            },
        }
    }

    #[test]
    fn should_populate_bbox_when_single_page_node_text_found_in_chunk() {
        let structure = DocumentStructure {
            nodes: vec![body_node(
                "Hello world, this is page one.",
                1,
                Some(bbox(10.0, 20.0, 100.0, 200.0)),
            )],
            source_format: None,
            relationships: Vec::new(),
            node_types: Vec::new(),
        };
        let mut chunks = vec![chunk_with_spans(
            "Hello world, this is page one.",
            vec![PageSpan { page: 1, bbox: None }],
        )];

        populate_page_span_bboxes(&mut chunks, &structure);

        assert_eq!(chunks[0].metadata.page_spans.len(), 1);
        assert_eq!(chunks[0].metadata.page_spans[0].page, 1);
        assert_eq!(
            chunks[0].metadata.page_spans[0].bbox,
            Some(bbox(10.0, 20.0, 100.0, 200.0))
        );
    }

    #[test]
    fn should_union_bboxes_when_multi_page_chunk_has_a_node_on_each_page() {
        let structure = DocumentStructure {
            nodes: vec![
                body_node("Content from page three.", 3, Some(bbox(72.2, 255.8, 400.0, 400.0))),
                body_node("Content from page four.", 4, Some(bbox(56.6, 610.1, 530.3, 766.1))),
            ],
            source_format: None,
            relationships: Vec::new(),
            node_types: Vec::new(),
        };
        let mut chunks = vec![chunk_with_spans(
            "Content from page three.\n\nContent from page four.",
            vec![PageSpan { page: 3, bbox: None }, PageSpan { page: 4, bbox: None }],
        )];

        populate_page_span_bboxes(&mut chunks, &structure);

        let spans = &chunks[0].metadata.page_spans;
        assert_eq!(
            spans.len(),
            2,
            "chunk spanning pages 3-4 must keep one PageSpan per page"
        );
        assert_eq!(spans[0].page, 3);
        assert_eq!(spans[0].bbox, Some(bbox(72.2, 255.8, 400.0, 400.0)));
        assert_eq!(spans[1].page, 4);
        assert_eq!(spans[1].bbox, Some(bbox(56.6, 610.1, 530.3, 766.1)));
    }

    #[test]
    fn should_union_multiple_node_bboxes_on_the_same_page() {
        let structure = DocumentStructure {
            nodes: vec![
                body_node("First paragraph on the page.", 1, Some(bbox(10.0, 10.0, 50.0, 50.0))),
                body_node("Second paragraph on the page.", 1, Some(bbox(60.0, 60.0, 120.0, 120.0))),
            ],
            source_format: None,
            relationships: Vec::new(),
            node_types: Vec::new(),
        };
        let mut chunks = vec![chunk_with_spans(
            "First paragraph on the page.\n\nSecond paragraph on the page.",
            vec![PageSpan { page: 1, bbox: None }],
        )];

        populate_page_span_bboxes(&mut chunks, &structure);

        assert_eq!(
            chunks[0].metadata.page_spans[0].bbox,
            Some(bbox(10.0, 10.0, 120.0, 120.0))
        );
    }

    #[test]
    fn should_leave_bbox_none_when_no_node_bbox_available() {
        let structure = DocumentStructure {
            nodes: vec![body_node("Hello world, this is page one.", 1, None)],
            source_format: None,
            relationships: Vec::new(),
            node_types: Vec::new(),
        };
        let mut chunks = vec![chunk_with_spans(
            "Hello world, this is page one.",
            vec![PageSpan { page: 1, bbox: None }],
        )];

        populate_page_span_bboxes(&mut chunks, &structure);

        assert_eq!(chunks[0].metadata.page_spans[0].bbox, None);
    }

    #[test]
    fn should_leave_bbox_none_when_node_text_not_found_in_chunk() {
        let structure = DocumentStructure {
            nodes: vec![body_node(
                "Unrelated content elsewhere.",
                1,
                Some(bbox(1.0, 1.0, 2.0, 2.0)),
            )],
            source_format: None,
            relationships: Vec::new(),
            node_types: Vec::new(),
        };
        let mut chunks = vec![chunk_with_spans(
            "Completely different chunk text.",
            vec![PageSpan { page: 1, bbox: None }],
        )];

        populate_page_span_bboxes(&mut chunks, &structure);

        assert_eq!(chunks[0].metadata.page_spans[0].bbox, None);
    }

    #[test]
    fn should_skip_non_body_layer_nodes() {
        let mut header = body_node("Running header text.", 1, Some(bbox(1.0, 1.0, 2.0, 2.0)));
        header.content_layer = ContentLayer::Header;
        let structure = DocumentStructure {
            nodes: vec![header],
            source_format: None,
            relationships: Vec::new(),
            node_types: Vec::new(),
        };
        let mut chunks = vec![chunk_with_spans(
            "Running header text.",
            vec![PageSpan { page: 1, bbox: None }],
        )];

        populate_page_span_bboxes(&mut chunks, &structure);

        assert_eq!(
            chunks[0].metadata.page_spans[0].bbox, None,
            "header/footer/footnote nodes must not contribute bboxes"
        );
    }

    #[test]
    fn should_noop_when_chunk_has_no_page_spans() {
        let structure = DocumentStructure {
            nodes: vec![body_node("Some text.", 1, Some(bbox(1.0, 1.0, 2.0, 2.0)))],
            source_format: None,
            relationships: Vec::new(),
            node_types: Vec::new(),
        };
        let mut chunks = vec![chunk_with_spans("Some text.", Vec::new())];

        populate_page_span_bboxes(&mut chunks, &structure);

        assert!(chunks[0].metadata.page_spans.is_empty());
    }

    #[test]
    fn should_match_node_that_spans_a_page_range() {
        let mut node = body_node("Table caption spanning pages.", 2, Some(bbox(5.0, 5.0, 15.0, 15.0)));
        node.page_end = Some(3);
        let structure = DocumentStructure {
            nodes: vec![node],
            source_format: None,
            relationships: Vec::new(),
            node_types: Vec::new(),
        };
        let mut chunks = vec![chunk_with_spans(
            "Table caption spanning pages.",
            vec![PageSpan { page: 3, bbox: None }],
        )];

        populate_page_span_bboxes(&mut chunks, &structure);

        assert_eq!(
            chunks[0].metadata.page_spans[0].bbox,
            Some(bbox(5.0, 5.0, 15.0, 15.0)),
            "node with page_end covering the span page must match"
        );
    }

    #[test]
    fn should_not_match_short_text_below_min_length_threshold() {
        let structure = DocumentStructure {
            nodes: vec![body_node("Hi", 1, Some(bbox(1.0, 1.0, 2.0, 2.0)))],
            source_format: None,
            relationships: Vec::new(),
            node_types: Vec::new(),
        };
        let mut chunks = vec![chunk_with_spans(
            "Hi there, this chunk contains Hi as a substring.",
            vec![PageSpan { page: 1, bbox: None }],
        )];

        populate_page_span_bboxes(&mut chunks, &structure);

        assert_eq!(
            chunks[0].metadata.page_spans[0].bbox, None,
            "text shorter than MIN_NODE_TEXT_MATCH_LEN must not be treated as a membership signal"
        );
    }

    #[test]
    fn should_not_match_nine_character_text_just_below_the_threshold() {
        let structure = DocumentStructure {
            nodes: vec![body_node("Data 1.2.", 1, Some(bbox(1.0, 1.0, 2.0, 2.0)))],
            source_format: None,
            relationships: Vec::new(),
            node_types: Vec::new(),
        };
        let mut chunks = vec![chunk_with_spans(
            "See Data 1.2. for the summary table on this page.",
            vec![PageSpan { page: 1, bbox: None }],
        )];

        populate_page_span_bboxes(&mut chunks, &structure);

        assert_eq!(
            chunks[0].metadata.page_spans[0].bbox, None,
            "9-char text is below MIN_NODE_TEXT_MATCH_LEN (10) and must not match"
        );
    }

    #[test]
    fn union_bbox_expands_to_enclose_both_boxes() {
        let result = union_bbox(Some(bbox(0.0, 0.0, 10.0, 10.0)), bbox(5.0, -5.0, 20.0, 8.0));
        assert_eq!(result, bbox(0.0, -5.0, 20.0, 10.0));
    }

    #[test]
    fn union_bbox_returns_next_when_acc_is_none() {
        let result = union_bbox(None, bbox(1.0, 2.0, 3.0, 4.0));
        assert_eq!(result, bbox(1.0, 2.0, 3.0, 4.0));
    }

    #[test]
    fn index_body_nodes_by_page_buckets_multi_page_node_under_every_covered_page() {
        let mut spanning = body_node("Table caption spanning pages.", 2, Some(bbox(5.0, 5.0, 15.0, 15.0)));
        spanning.page_end = Some(4);
        let single = body_node("Single page paragraph text.", 3, Some(bbox(1.0, 1.0, 2.0, 2.0)));
        let mut header = body_node("Running header text on page.", 3, Some(bbox(9.0, 9.0, 9.0, 9.0)));
        header.content_layer = ContentLayer::Header;

        let nodes = [spanning, single, header];
        let index = index_body_nodes_by_page(&nodes);

        assert_eq!(
            index.get(&2).map(Vec::len),
            Some(1),
            "page 2 must only see the spanning node"
        );
        assert_eq!(
            index.get(&3).map(Vec::len),
            Some(2),
            "page 3 must see both the spanning node and the single-page node, but not the header"
        );
        assert_eq!(
            index.get(&4).map(Vec::len),
            Some(1),
            "page 4 must only see the spanning node"
        );
        assert!(!index.contains_key(&1), "page 1 has no covering nodes");
    }

    #[test]
    fn should_populate_node_ids_for_matching_nodes_and_leave_empty_when_no_node_matches() {
        let structure = DocumentStructure {
            nodes: vec![
                body_node_with_id(
                    "node-1",
                    "First paragraph on page one.",
                    1,
                    Some(bbox(10.0, 10.0, 50.0, 50.0)),
                ),
                body_node_with_id(
                    "node-2",
                    "Second paragraph on page one.",
                    1,
                    Some(bbox(60.0, 60.0, 120.0, 120.0)),
                ),
                body_node_with_id(
                    "node-3",
                    "Unrelated paragraph on page two.",
                    2,
                    Some(bbox(1.0, 1.0, 2.0, 2.0)),
                ),
            ],
            source_format: None,
            relationships: Vec::new(),
            node_types: Vec::new(),
        };
        let mut chunks = vec![
            chunk_with_spans(
                "First paragraph on page one.\n\nSecond paragraph on page one.",
                vec![PageSpan { page: 1, bbox: None }],
            ),
            chunk_with_spans(
                "Completely different chunk text that matches no node.",
                vec![PageSpan { page: 2, bbox: None }],
            ),
        ];

        populate_page_span_bboxes(&mut chunks, &structure);

        assert_eq!(
            chunks[0].metadata.node_ids,
            vec!["node-1".to_string(), "node-2".to_string()],
            "chunk must reference exactly the ids of the nodes whose text it covers, in traversal order"
        );
        assert!(
            chunks[1].metadata.node_ids.is_empty(),
            "chunk matching no node must have empty node_ids"
        );
    }

    #[test]
    fn should_dedupe_node_id_when_the_same_multi_page_node_matches_more_than_one_span() {
        let mut node = body_node_with_id(
            "node-multi",
            "Table caption spanning pages.",
            2,
            Some(bbox(5.0, 5.0, 15.0, 15.0)),
        );
        node.page_end = Some(3);
        let structure = DocumentStructure {
            nodes: vec![node],
            source_format: None,
            relationships: Vec::new(),
            node_types: Vec::new(),
        };
        let mut chunks = vec![chunk_with_spans(
            "Table caption spanning pages.",
            vec![PageSpan { page: 2, bbox: None }, PageSpan { page: 3, bbox: None }],
        )];

        populate_page_span_bboxes(&mut chunks, &structure);

        assert_eq!(
            chunks[0].metadata.node_ids,
            vec!["node-multi".to_string()],
            "a node spanning multiple pages must contribute its id once, not once per page"
        );
    }

    #[test]
    fn should_populate_node_id_even_when_the_matching_node_has_no_bbox() {
        let structure = DocumentStructure {
            nodes: vec![body_node_with_id(
                "node-no-bbox",
                "Hello world, this is page one.",
                1,
                None,
            )],
            source_format: None,
            relationships: Vec::new(),
            node_types: Vec::new(),
        };
        let mut chunks = vec![chunk_with_spans(
            "Hello world, this is page one.",
            vec![PageSpan { page: 1, bbox: None }],
        )];

        populate_page_span_bboxes(&mut chunks, &structure);

        assert_eq!(chunks[0].metadata.page_spans[0].bbox, None);
        assert_eq!(chunks[0].metadata.node_ids, vec!["node-no-bbox".to_string()]);
    }

    fn node_with_content(id: &str, content: NodeContent, page: u32, bbox: Option<BoundingBox>) -> DocumentNode {
        DocumentNode {
            id: id.to_string(),
            content,
            parent: None,
            children: Vec::new(),
            content_layer: ContentLayer::Body,
            page: Some(page),
            page_end: None,
            bbox,
            annotations: Vec::new(),
            attributes: None,
        }
    }

    /// Regression test for #212: `Table { grid }` never contributed a bbox/node_id
    /// because `node_text_for_matching` had no arm for it — matching per-cell now lets
    /// a table-only chunk pick up its structure backlink and viewer highlight.
    #[test]
    fn should_populate_bbox_and_node_id_for_table_node_via_cell_text() {
        use crate::types::document_structure::{GridCell, TableGrid};

        let grid = TableGrid {
            rows: 1,
            cols: 2,
            cells: vec![
                GridCell {
                    content: "Revenue by region".to_string(),
                    row: 0,
                    col: 0,
                    row_span: 1,
                    col_span: 1,
                    is_header: true,
                    bbox: None,
                },
                GridCell {
                    content: "Q3".to_string(),
                    row: 0,
                    col: 1,
                    row_span: 1,
                    col_span: 1,
                    is_header: true,
                    bbox: None,
                },
            ],
        };
        let structure = DocumentStructure {
            nodes: vec![node_with_content(
                "table-1",
                NodeContent::Table { grid },
                1,
                Some(bbox(1.0, 1.0, 50.0, 50.0)),
            )],
            source_format: None,
            relationships: Vec::new(),
            node_types: Vec::new(),
        };
        let mut chunks = vec![chunk_with_spans(
            "| Revenue by region | Q3 |\n|---|---|\n",
            vec![PageSpan { page: 1, bbox: None }],
        )];

        populate_page_span_bboxes(&mut chunks, &structure);

        assert_eq!(chunks[0].metadata.page_spans[0].bbox, Some(bbox(1.0, 1.0, 50.0, 50.0)));
        assert_eq!(chunks[0].metadata.node_ids, vec!["table-1".to_string()]);
    }

    /// A table whose cell text does not appear in the chunk must not match.
    #[test]
    fn should_not_match_table_node_when_no_cell_text_found_in_chunk() {
        use crate::types::document_structure::{GridCell, TableGrid};

        let grid = TableGrid {
            rows: 1,
            cols: 1,
            cells: vec![GridCell {
                content: "Completely unrelated cell content".to_string(),
                row: 0,
                col: 0,
                row_span: 1,
                col_span: 1,
                is_header: false,
                bbox: None,
            }],
        };
        let structure = DocumentStructure {
            nodes: vec![node_with_content(
                "table-2",
                NodeContent::Table { grid },
                1,
                Some(bbox(1.0, 1.0, 2.0, 2.0)),
            )],
            source_format: None,
            relationships: Vec::new(),
            node_types: Vec::new(),
        };
        let mut chunks = vec![chunk_with_spans(
            "Some other paragraph entirely.",
            vec![PageSpan { page: 1, bbox: None }],
        )];

        populate_page_span_bboxes(&mut chunks, &structure);

        assert_eq!(chunks[0].metadata.page_spans[0].bbox, None);
        assert!(chunks[0].metadata.node_ids.is_empty());
    }

    /// Regression test for #212: `Image { description }` never contributed a bbox/node_id.
    #[test]
    fn should_populate_bbox_and_node_id_for_image_node_via_description() {
        let structure = DocumentStructure {
            nodes: vec![node_with_content(
                "image-1",
                NodeContent::Image {
                    description: Some("Diagram of the quarterly revenue funnel".to_string()),
                    image_index: Some(0),
                    src: None,
                },
                2,
                Some(bbox(4.0, 4.0, 40.0, 40.0)),
            )],
            source_format: None,
            relationships: Vec::new(),
            node_types: Vec::new(),
        };
        let mut chunks = vec![chunk_with_spans(
            "![Diagram of the quarterly revenue funnel](img.png)",
            vec![PageSpan { page: 2, bbox: None }],
        )];

        populate_page_span_bboxes(&mut chunks, &structure);

        assert_eq!(chunks[0].metadata.page_spans[0].bbox, Some(bbox(4.0, 4.0, 40.0, 40.0)));
        assert_eq!(chunks[0].metadata.node_ids, vec!["image-1".to_string()]);
    }

    /// An `Image` with no `description` has nothing to match against and must not
    /// contribute a bbox/node_id (there is no fragment to search for).
    #[test]
    fn should_not_match_image_node_without_description() {
        let structure = DocumentStructure {
            nodes: vec![node_with_content(
                "image-2",
                NodeContent::Image {
                    description: None,
                    image_index: Some(0),
                    src: None,
                },
                1,
                Some(bbox(1.0, 1.0, 2.0, 2.0)),
            )],
            source_format: None,
            relationships: Vec::new(),
            node_types: Vec::new(),
        };
        let mut chunks = vec![chunk_with_spans(
            "Any content at all.",
            vec![PageSpan { page: 1, bbox: None }],
        )];

        populate_page_span_bboxes(&mut chunks, &structure);

        assert_eq!(chunks[0].metadata.page_spans[0].bbox, None);
        assert!(chunks[0].metadata.node_ids.is_empty());
    }

    /// Regression test for #212: `Group { heading_text }` never contributed a bbox/node_id.
    #[test]
    fn should_populate_bbox_and_node_id_for_group_node_via_heading_text() {
        let structure = DocumentStructure {
            nodes: vec![node_with_content(
                "group-1",
                NodeContent::Group {
                    label: None,
                    heading_level: Some(2),
                    heading_text: Some("Key-value configuration section".to_string()),
                },
                1,
                Some(bbox(1.0, 1.0, 20.0, 20.0)),
            )],
            source_format: None,
            relationships: Vec::new(),
            node_types: Vec::new(),
        };
        let mut chunks = vec![chunk_with_spans(
            "## Key-value configuration section\n\nSetting: value",
            vec![PageSpan { page: 1, bbox: None }],
        )];

        populate_page_span_bboxes(&mut chunks, &structure);

        assert_eq!(chunks[0].metadata.page_spans[0].bbox, Some(bbox(1.0, 1.0, 20.0, 20.0)));
        assert_eq!(chunks[0].metadata.node_ids, vec!["group-1".to_string()]);
    }

    /// Regression test for #212: `Slide { title }` never contributed a bbox/node_id.
    #[test]
    fn should_populate_bbox_and_node_id_for_slide_node_via_title() {
        let structure = DocumentStructure {
            nodes: vec![node_with_content(
                "slide-1",
                NodeContent::Slide {
                    number: 3,
                    title: Some("Roadmap for next quarter".to_string()),
                },
                3,
                Some(bbox(0.0, 0.0, 100.0, 100.0)),
            )],
            source_format: None,
            relationships: Vec::new(),
            node_types: Vec::new(),
        };
        let mut chunks = vec![chunk_with_spans(
            "# Roadmap for next quarter\n\nSlide body text.",
            vec![PageSpan { page: 3, bbox: None }],
        )];

        populate_page_span_bboxes(&mut chunks, &structure);

        assert_eq!(
            chunks[0].metadata.page_spans[0].bbox,
            Some(bbox(0.0, 0.0, 100.0, 100.0))
        );
        assert_eq!(chunks[0].metadata.node_ids, vec!["slide-1".to_string()]);
    }

    /// Regression test for #212: `DefinitionItem { term, definition }` never contributed
    /// a bbox/node_id; both the term and the definition are checked as independent
    /// candidate fragments since the rendered form rarely places them contiguously.
    #[test]
    fn should_populate_bbox_and_node_id_for_definition_item_via_term_or_definition() {
        let structure = DocumentStructure {
            nodes: vec![node_with_content(
                "def-1",
                NodeContent::DefinitionItem {
                    term: "Throughput".to_string(),
                    definition: "The number of requests processed per second".to_string(),
                },
                1,
                Some(bbox(2.0, 2.0, 30.0, 30.0)),
            )],
            source_format: None,
            relationships: Vec::new(),
            node_types: Vec::new(),
        };
        // Chunk contains only the definition, not the term verbatim next to it.
        let mut chunks = vec![chunk_with_spans(
            "The number of requests processed per second is a key metric.",
            vec![PageSpan { page: 1, bbox: None }],
        )];

        populate_page_span_bboxes(&mut chunks, &structure);

        assert_eq!(chunks[0].metadata.page_spans[0].bbox, Some(bbox(2.0, 2.0, 30.0, 30.0)));
        assert_eq!(chunks[0].metadata.node_ids, vec!["def-1".to_string()]);
    }

    /// Regression test for #212: `Admonition { title }` never contributed a bbox/node_id.
    #[test]
    fn should_populate_bbox_and_node_id_for_admonition_node_via_title() {
        let structure = DocumentStructure {
            nodes: vec![node_with_content(
                "admonition-1",
                NodeContent::Admonition {
                    kind: "warning".to_string(),
                    title: Some("Deprecated configuration option".to_string()),
                },
                1,
                Some(bbox(3.0, 3.0, 33.0, 33.0)),
            )],
            source_format: None,
            relationships: Vec::new(),
            node_types: Vec::new(),
        };
        let mut chunks = vec![chunk_with_spans(
            "> **Deprecated configuration option**\n>\n> Use the new flag instead.",
            vec![PageSpan { page: 1, bbox: None }],
        )];

        populate_page_span_bboxes(&mut chunks, &structure);

        assert_eq!(chunks[0].metadata.page_spans[0].bbox, Some(bbox(3.0, 3.0, 33.0, 33.0)));
        assert_eq!(chunks[0].metadata.node_ids, vec!["admonition-1".to_string()]);
    }

    /// A pure container node with no directly comparable text (`Quote`) must not match
    /// anything, confirming the `_ => Vec::new()` fallback arm.
    #[test]
    fn should_not_match_pure_container_node_with_no_text() {
        let structure = DocumentStructure {
            nodes: vec![node_with_content(
                "quote-1",
                NodeContent::Quote,
                1,
                Some(bbox(1.0, 1.0, 2.0, 2.0)),
            )],
            source_format: None,
            relationships: Vec::new(),
            node_types: Vec::new(),
        };
        let mut chunks = vec![chunk_with_spans(
            "> Some quoted text that a Quote container cannot match on its own.",
            vec![PageSpan { page: 1, bbox: None }],
        )];

        populate_page_span_bboxes(&mut chunks, &structure);

        assert_eq!(chunks[0].metadata.page_spans[0].bbox, None);
        assert!(chunks[0].metadata.node_ids.is_empty());
    }
}
