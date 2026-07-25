//! Typed structural sidecar and the canonical SF1 structural metric.
//!
//! A [`StructuralSidecar`] is a deterministic, typed description of a document's
//! *structure* — headings with hierarchy, nested list items, tables with a full
//! cell grid, figure/caption/footnote binding edges, formulas, images, and a
//! global reading order. It is persisted per document as `<id>.structural.json`
//! (see `scripts/build_structural_sidecar.py`) and derived here from GFM
//! markdown using the **same** pulldown-cmark options as
//! [`crate::markdown_quality::parse_markdown_blocks`]
//! ([`crate::markdown_quality::md_parser_options`]).
//!
//! # Spans
//!
//! GFM pipe tables cannot express row/column spans, so a sidecar derived from
//! markdown has every cell `rowspan == colspan == 1` and
//! `spans_recoverable == false`. HTML/ParseBench-sourced tables (built by the
//! Python builder from the source HTML) can carry real spans with
//! `spans_recoverable == true`.
//!
//! # The metric: [`score_structural`]
//!
//! Six dimensions are scored, then rolled up with structural weights (heading
//! 2.0 / table 1.5 / list 1.0 / paragraph 0.5, plus binding-edges 0.5),
//! normalized over the dimensions actually present in either document, and
//! finally folded with the LIS reading-order score via
//! [`crate::markdown_quality::fold_order_into_sf1`]:
//!
//! - **D0** paragraph content-F1
//! - **D1** heading hierarchy (level agreement + ancestry Jaccard)
//! - **D2** list nesting (depth + ordered agreement)
//! - **D3** table topology, a GriTS-like grid F1 (a fabricated table scores 0)
//! - **D4** caption / footnote binding-edge F1
//! - **D5** reading order via longest-increasing-subsequence
//! - **D6** table cell-content F1 (GriTS-Con, position-independent) —
//!   report-only, not folded into SF1 (see Phase 2 of the metrics plan)
//!
//! A fabricated table — a predicted table where the GT has none — scores 0 on
//! D3, which then pulls the whole SF1 down (it can no longer hide as matched
//! prose after the D3 § metric fix in [`crate::markdown_quality`]).

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::markdown_quality::{compute_order_score, fold_order_into_sf1};
use crate::quality::{compute_f1, tokenize};

const WEIGHT_HEADING: f64 = 2.0;
const WEIGHT_TABLE: f64 = 1.5;
const WEIGHT_LIST: f64 = 1.0;
const WEIGHT_PARAGRAPH: f64 = 0.5;
const WEIGHT_EDGES: f64 = 0.5;

/// Per-heading-level partial credit: score drops by this per level of distance.
const HEADING_LEVEL_STEP: f64 = 0.25;
/// Per-list-depth partial credit: score drops by this per level of depth distance.
const LIST_DEPTH_STEP: f64 = 0.34;
/// Weight split between the two structural sub-scores of headings and lists.
const STRUCT_SPLIT: f64 = 0.5;
/// Per-grid-dimension credit when only one of {rowspan, colspan} agrees.
const SPAN_HALF_CREDIT: f64 = 0.5;

/// A single cell in a table's grid.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Cell {
    pub row: usize,
    pub col: usize,
    pub rowspan: usize,
    pub colspan: usize,
    pub is_header: bool,
    pub text: String,
}

/// A table node: a full cell grid plus span-recoverability metadata.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct TableNode {
    pub n_rows: usize,
    pub n_cols: usize,
    pub header_rows: usize,
    pub cells: Vec<Cell>,
    /// `false` for GFM pipe tables (spans cannot be expressed); `true` when the
    /// grid was recovered from source HTML and may carry real spans.
    pub spans_recoverable: bool,
}

/// A typed structural node. Serialized with an internal `"kind"` tag.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StructuralNode {
    Heading {
        level: u8,
        /// Node index of the immediately enclosing heading, if any.
        parent: Option<usize>,
        /// Ancestor heading texts, outermost first.
        path: Vec<String>,
        text: String,
    },
    ListItem {
        /// 0-based nesting depth.
        depth: usize,
        ordered: bool,
        /// Node index of the enclosing list item, if any.
        parent_item: Option<usize>,
        text: String,
    },
    Table(TableNode),
    Figure {
        /// Node index of the bound caption, if any.
        caption: Option<usize>,
        text: String,
    },
    Caption {
        /// Node index this caption binds to (figure/table/image), if any.
        binds_to: Option<usize>,
        text: String,
    },
    Footnote {
        /// Node index this footnote annotates, if any.
        binds_to: Option<usize>,
        text: String,
    },
    Formula {
        display: bool,
        text: String,
    },
    Image {
        alt: String,
    },
    Paragraph {
        text: String,
    },
}

impl StructuralNode {
    /// A representative text for content-similarity matching.
    pub(crate) fn repr_text(&self) -> String {
        match self {
            StructuralNode::Heading { text, .. }
            | StructuralNode::ListItem { text, .. }
            | StructuralNode::Figure { text, .. }
            | StructuralNode::Caption { text, .. }
            | StructuralNode::Footnote { text, .. }
            | StructuralNode::Formula { text, .. }
            | StructuralNode::Paragraph { text } => text.clone(),
            StructuralNode::Image { alt } => alt.clone(),
            StructuralNode::Table(t) => t.cells.iter().map(|c| c.text.as_str()).collect::<Vec<_>>().join(" "),
        }
    }

    pub(crate) fn kind_name(&self) -> &'static str {
        match self {
            StructuralNode::Heading { .. } => "heading",
            StructuralNode::ListItem { .. } => "list_item",
            StructuralNode::Table(_) => "table",
            StructuralNode::Figure { .. } => "figure",
            StructuralNode::Caption { .. } => "caption",
            StructuralNode::Footnote { .. } => "footnote",
            StructuralNode::Formula { .. } => "formula",
            StructuralNode::Image { .. } => "image",
            StructuralNode::Paragraph { .. } => "paragraph",
        }
    }
}

/// A typed structural description of one document.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct StructuralSidecar {
    pub nodes: Vec<StructuralNode>,
    /// Node indices in reading order. Derived from GFM in document order, but
    /// kept explicit so an HTML-sourced builder can supply a corrected order.
    pub reading_order: Vec<usize>,
}

/// Precision/recall/F1 split for one structural dimension. Report-only: it does
/// not feed the SF1 rollup, but lets a low dimension F1 be read as fabrication
/// (low precision) vs omission (low recall) — the crux of the #36 table work.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Default)]
pub struct DimBreakdown {
    pub f1: f64,
    pub precision: f64,
    pub recall: f64,
}

/// The scored structural dimensions and the rolled-up SF1. D0–D5 feed the
/// weighted, order-folded rollup; D6 (table content) is report-only for now.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct StructuralScore {
    /// D0 — paragraph/content F1.
    pub d0_paragraph: f64,
    /// D1 — heading hierarchy.
    pub d1_heading: f64,
    /// D2 — list nesting.
    pub d2_list: f64,
    /// D3 — table topology (GriTS-like).
    pub d3_table: f64,
    /// D4 — caption/footnote binding-edge F1.
    pub d4_edges: f64,
    /// D5 — reading order (LIS).
    pub d5_order: f64,
    /// D6 — table cell-content F1 (GriTS-Con, position-independent). Report-only:
    /// it does NOT feed the SF1 rollup yet (see Phase 2). Defaults to 1.0 for
    /// scores deserialized from data written before this dimension existed.
    #[serde(default = "one")]
    pub d6_table_content: f64,
    /// Weighted, order-folded SF1 rollup.
    pub sf1: f64,
    /// Per-dimension precision/recall split, parallel to [`Self::dimensions`].
    /// Report-only diagnostics; absent on scores deserialized from older data.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub breakdown: Vec<DimBreakdown>,
}

/// Serde default for [`StructuralScore::d6_table_content`] on legacy data.
fn one() -> f64 {
    1.0
}

impl StructuralScore {
    /// Named dimension scores used by benchmark comparison reports. `order` and
    /// `table_content` are reported here but scored/folded separately from the
    /// weighted rollup.
    pub fn dimensions(&self) -> [(&'static str, f64); 7] {
        [
            ("paragraph", self.d0_paragraph),
            ("heading", self.d1_heading),
            ("list", self.d2_list),
            ("table", self.d3_table),
            ("edges", self.d4_edges),
            ("order", self.d5_order),
            ("table_content", self.d6_table_content),
        ]
    }

    /// Named dimensions paired with their precision/recall breakdown. Empty
    /// breakdowns (older data) fall back to a zeroed split carrying the F1.
    pub fn dimensions_pr(&self) -> [(&'static str, DimBreakdown); 7] {
        let dims = self.dimensions();
        std::array::from_fn(|i| {
            let (name, f1) = dims[i];
            let bd = self.breakdown.get(i).copied().unwrap_or(DimBreakdown {
                f1,
                precision: f1,
                recall: f1,
            });
            (name, bd)
        })
    }
}

impl StructuralSidecar {
    /// Serialize to pretty JSON (`<id>.structural.json`).
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    /// Derive a sidecar deterministically from GFM markdown, using the same
    /// pulldown-cmark options as [`crate::markdown_quality::parse_markdown_blocks`].
    pub fn from_markdown(md: &str) -> Self {
        use pulldown_cmark::{Event, Parser, Tag, TagEnd};

        let mut nodes: Vec<StructuralNode> = Vec::new();

        // Heading hierarchy stack: (level, node_index, text). ~keep
        let mut heading_stack: Vec<(u8, usize, String)> = Vec::new();

        let mut list_ordered: Vec<bool> = Vec::new();
        let mut item_text: Vec<String> = Vec::new();
        let mut item_ordered: Vec<bool> = Vec::new();

        struct TableBuild {
            cells: Vec<Cell>,
            row: usize,
            col: usize,
            n_cols: usize,
            header_rows: usize,
            in_header: bool,
        }
        let mut table: Option<TableBuild> = None;

        let mut current_text = String::new();
        let mut in_heading: Option<u8> = None;
        let mut in_code_block = false;

        let flush_paragraph = |text: &mut String, nodes: &mut Vec<StructuralNode>| {
            let content = std::mem::take(text);
            let trimmed = content.trim();
            if !trimmed.is_empty() {
                nodes.push(StructuralNode::Paragraph {
                    text: trimmed.to_string(),
                });
            }
        };

        for event in Parser::new_ext(md, crate::markdown_quality::md_parser_options()) {
            match event {
                Event::Start(Tag::Heading { level, .. }) => {
                    flush_paragraph(&mut current_text, &mut nodes);
                    in_heading = Some(level as u8);
                }
                Event::End(TagEnd::Heading(_)) => {
                    if let Some(level) = in_heading.take() {
                        let text = std::mem::take(&mut current_text).trim().to_string();
                        if !text.is_empty() {
                            while heading_stack.last().is_some_and(|(l, _, _)| *l >= level) {
                                heading_stack.pop();
                            }
                            let parent = heading_stack.last().map(|(_, idx, _)| *idx);
                            let path: Vec<String> = heading_stack.iter().map(|(_, _, t)| t.clone()).collect();
                            let idx = nodes.len();
                            nodes.push(StructuralNode::Heading {
                                level,
                                parent,
                                path,
                                text: text.clone(),
                            });
                            heading_stack.push((level, idx, text));
                        }
                    }
                }
                Event::Start(Tag::CodeBlock(_)) => {
                    flush_paragraph(&mut current_text, &mut nodes);
                    in_code_block = true;
                }
                Event::End(TagEnd::CodeBlock) => {
                    in_code_block = false;
                    let content = std::mem::take(&mut current_text);
                    let trimmed = content.trim();
                    if !trimmed.is_empty() {
                        if is_formula(trimmed) {
                            nodes.push(StructuralNode::Formula {
                                display: true,
                                text: trimmed.to_string(),
                            });
                        } else {
                            // Code blocks are content — fold into the paragraph pool. ~keep
                            nodes.push(StructuralNode::Paragraph {
                                text: trimmed.to_string(),
                            });
                        }
                    }
                }
                Event::Start(Tag::Table(_)) => {
                    flush_paragraph(&mut current_text, &mut nodes);
                    table = Some(TableBuild {
                        cells: Vec::new(),
                        row: 0,
                        col: 0,
                        n_cols: 0,
                        header_rows: 0,
                        in_header: false,
                    });
                }
                Event::End(TagEnd::Table) => {
                    if let Some(tb) = table.take() {
                        let n_rows = if tb.cells.is_empty() {
                            0
                        } else {
                            tb.cells.iter().map(|c| c.row).max().unwrap_or(0) + 1
                        };
                        nodes.push(StructuralNode::Table(TableNode {
                            n_rows,
                            n_cols: tb.n_cols,
                            header_rows: tb.header_rows,
                            cells: tb.cells,
                            spans_recoverable: false,
                        }));
                    }
                }
                Event::Start(Tag::TableHead) => {
                    if let Some(tb) = table.as_mut() {
                        tb.in_header = true;
                    }
                }
                Event::End(TagEnd::TableHead) => {
                    if let Some(tb) = table.as_mut() {
                        tb.in_header = false;
                        tb.n_cols = tb.n_cols.max(tb.col);
                        tb.row += 1;
                        tb.col = 0;
                    }
                }
                Event::Start(Tag::TableRow) => {
                    if let Some(tb) = table.as_mut() {
                        tb.col = 0;
                    }
                }
                Event::End(TagEnd::TableRow) => {
                    if let Some(tb) = table.as_mut() {
                        tb.n_cols = tb.n_cols.max(tb.col);
                        tb.row += 1;
                    }
                }
                Event::End(TagEnd::TableCell) => {
                    if let Some(tb) = table.as_mut() {
                        let text = std::mem::take(&mut current_text).trim().to_string();
                        if tb.in_header {
                            tb.header_rows = tb.header_rows.max(tb.row + 1);
                        }
                        tb.cells.push(Cell {
                            row: tb.row,
                            col: tb.col,
                            rowspan: 1,
                            colspan: 1,
                            is_header: tb.in_header,
                            text,
                        });
                        tb.col += 1;
                    }
                }
                Event::Start(Tag::List(start)) => {
                    if list_ordered.is_empty() {
                        flush_paragraph(&mut current_text, &mut nodes);
                    }
                    list_ordered.push(start.is_some());
                }
                Event::End(TagEnd::List(_)) => {
                    list_ordered.pop();
                }
                Event::Start(Tag::Item) => {
                    item_text.push(String::new());
                    item_ordered.push(list_ordered.last().copied().unwrap_or(false));
                }
                Event::End(TagEnd::Item) => {
                    let text = item_text.pop().unwrap_or_default().trim().to_string();
                    let ordered = item_ordered.pop().unwrap_or(false);
                    let depth = item_text.len(); // remaining open items = ancestor count ~keep
                    if !text.is_empty() {
                        nodes.push(StructuralNode::ListItem {
                            depth,
                            ordered,
                            parent_item: None, // filled in post-pass (not scored) ~keep
                            text,
                        });
                    }
                }
                Event::Start(Tag::Image { .. }) => {
                    flush_paragraph(&mut current_text, &mut nodes);
                    current_text.push_str("\u{0}IMG\u{0}"); // sentinel so image alt is captured separately ~keep
                }
                Event::End(TagEnd::Image) => {
                    if let Some(rest) = current_text.strip_prefix("\u{0}IMG\u{0}") {
                        let alt = rest.trim().to_string();
                        current_text.clear();
                        nodes.push(StructuralNode::Image { alt });
                    }
                }
                Event::Start(Tag::Paragraph) if table.is_none() && item_text.is_empty() => {
                    flush_paragraph(&mut current_text, &mut nodes);
                }
                Event::End(TagEnd::Paragraph) if table.is_none() && item_text.is_empty() => {
                    flush_paragraph(&mut current_text, &mut nodes);
                }
                Event::Text(text) | Event::Code(text) => {
                    if let Some(buf) = item_text.last_mut() {
                        push_spaced(buf, &text);
                    } else if current_text.starts_with("\u{0}IMG\u{0}") {
                        current_text.push_str(&text);
                    } else {
                        push_spaced(&mut current_text, &text);
                    }
                }
                Event::SoftBreak => {
                    if in_code_block {
                        current_text.push('\n');
                    } else if let Some(buf) = item_text.last_mut() {
                        buf.push(' ');
                    } else {
                        current_text.push(' ');
                    }
                }
                Event::HardBreak => {
                    if let Some(buf) = item_text.last_mut() {
                        buf.push(' ');
                    } else {
                        current_text.push('\n');
                    }
                }
                Event::InlineMath(text) => {
                    if let Some(buf) = item_text.last_mut() {
                        push_spaced(buf, &text);
                    } else {
                        push_spaced(&mut current_text, &text);
                    }
                }
                Event::DisplayMath(text) => {
                    flush_paragraph(&mut current_text, &mut nodes);
                    let trimmed = text.trim();
                    if !trimmed.is_empty() {
                        nodes.push(StructuralNode::Formula {
                            display: true,
                            text: trimmed.to_string(),
                        });
                    }
                }
                _ => {}
            }
        }
        flush_paragraph(&mut current_text, &mut nodes);

        fill_list_parents(&mut nodes);
        bind_captions_and_footnotes(&mut nodes);

        let reading_order = (0..nodes.len()).collect();
        StructuralSidecar { nodes, reading_order }
    }
}

/// Append `text` to `buf`, inserting a separating space when needed (mirrors the
/// spacing rule in `parse_markdown_blocks`).
fn push_spaced(buf: &mut String, text: &str) {
    if !buf.is_empty() && !buf.ends_with(' ') && !buf.ends_with('\n') {
        buf.push(' ');
    }
    buf.push_str(text);
}

/// True if a fenced block's content looks like a LaTeX formula.
fn is_formula(content: &str) -> bool {
    content.trim_start().starts_with('\\')
        || content.contains("\\frac")
        || content.contains("\\sum")
        || content.contains("\\int")
        || content.contains("\\begin{")
}

/// Fill `parent_item` for list items by scanning backwards for the nearest
/// earlier item at `depth - 1`. Informational only (not scored).
fn fill_list_parents(nodes: &mut [StructuralNode]) {
    for i in 0..nodes.len() {
        let depth = match &nodes[i] {
            StructuralNode::ListItem { depth, .. } => *depth,
            _ => continue,
        };
        if depth == 0 {
            continue;
        }
        let mut parent = None;
        for j in (0..i).rev() {
            if let StructuralNode::ListItem { depth: d, .. } = &nodes[j]
                && *d == depth - 1
            {
                parent = Some(j);
                break;
            }
        }
        if let StructuralNode::ListItem { parent_item, .. } = &mut nodes[i] {
            *parent_item = parent;
        }
    }
}

/// Convert caption-like and footnote-like paragraphs into [`StructuralNode::Caption`]
/// / [`StructuralNode::Footnote`] nodes, binding each to the nearest preceding
/// figure/table/image (captions) or content node (footnotes).
fn bind_captions_and_footnotes(nodes: &mut [StructuralNode]) {
    for i in 0..nodes.len() {
        let text = match &nodes[i] {
            StructuralNode::Paragraph { text } => text.clone(),
            _ => continue,
        };
        if is_footnote_text(&text) {
            let target = nearest_preceding(nodes, i, |n| {
                matches!(n, StructuralNode::Paragraph { .. } | StructuralNode::Table(_))
            });
            nodes[i] = StructuralNode::Footnote { binds_to: target, text };
        } else if is_caption_text(&text) {
            let target = nearest_preceding(nodes, i, |n| {
                matches!(
                    n,
                    StructuralNode::Image { .. } | StructuralNode::Table(_) | StructuralNode::Figure { .. }
                )
            });
            nodes[i] = StructuralNode::Caption { binds_to: target, text };
        }
    }
}

fn nearest_preceding(nodes: &[StructuralNode], from: usize, pred: impl Fn(&StructuralNode) -> bool) -> Option<usize> {
    (0..from).rev().find(|&j| pred(&nodes[j]))
}

fn is_caption_text(text: &str) -> bool {
    let lower = text.trim_start().to_ascii_lowercase();
    const PREFIXES: [&str; 7] = ["figure", "fig.", "table", "chart", "diagram", "scheme", "plate"];
    PREFIXES.iter().any(|p| {
        lower.strip_prefix(p).is_some_and(|rest| {
            rest.trim_start()
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_digit() || c == ':')
        })
    })
}

fn is_footnote_text(text: &str) -> bool {
    // GFM footnote definition `[^id]: …` rendered as literal text without the option enabled. ~keep
    let t = text.trim_start();
    t.starts_with("[^") && t.contains("]:")
}

fn content_sim(a: &str, b: &str) -> f64 {
    compute_f1(&tokenize(a), &tokenize(b))
}

/// Greedy highest-score-first bipartite match over content texts.
/// Returns `(pred_idx, gt_idx, sim)` for each matched pair.
fn greedy_match(pred: &[String], gt: &[String]) -> Vec<(usize, usize, f64)> {
    let mut cands: Vec<(usize, usize, f64)> = Vec::new();
    for (i, p) in pred.iter().enumerate() {
        for (j, g) in gt.iter().enumerate() {
            let s = content_sim(p, g);
            if s > 0.0 {
                cands.push((i, j, s));
            }
        }
    }
    cands.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));
    let mut used_pred = vec![false; pred.len()];
    let mut used_gt = vec![false; gt.len()];
    let mut out = Vec::new();
    for (i, j, s) in cands {
        if used_pred[i] || used_gt[j] {
            continue;
        }
        used_pred[i] = true;
        used_gt[j] = true;
        out.push((i, j, s));
    }
    out
}

/// Precision, recall, and F1 from a sum of matched credit against pred and gt
/// cardinalities. Precision reads as "how much of what we emitted was right"
/// (over-fabrication when low), recall as "how much of the truth we recovered".
fn f1_parts_from(matched_credit: f64, n_pred: usize, n_gt: usize) -> (f64, f64, f64) {
    if n_pred == 0 && n_gt == 0 {
        return (1.0, 1.0, 1.0);
    }
    if n_pred == 0 || n_gt == 0 {
        return (0.0, 0.0, 0.0);
    }
    let precision = matched_credit / n_pred as f64;
    let recall = matched_credit / n_gt as f64;
    let f1 = if precision + recall > 0.0 {
        2.0 * precision * recall / (precision + recall)
    } else {
        0.0
    };
    (f1, precision, recall)
}

/// F1 from a sum of matched credit against pred and gt cardinalities.
fn f1_from(matched_credit: f64, n_pred: usize, n_gt: usize) -> f64 {
    f1_parts_from(matched_credit, n_pred, n_gt).0
}

fn jaccard(a: &HashSet<String>, b: &HashSet<String>) -> f64 {
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    let inter = a.intersection(b).count() as f64;
    let union = a.union(b).count() as f64;
    if union > 0.0 { inter / union } else { 1.0 }
}

/// Score the six structural dimensions of `pred` against `gt` and roll them up
/// into SF1.
pub fn score_structural(pred: &StructuralSidecar, gt: &StructuralSidecar) -> StructuralScore {
    let d0 = score_paragraphs(pred, gt);
    let d1 = score_headings(pred, gt);
    let d2 = score_lists(pred, gt);
    let d3 = score_tables(pred, gt);
    let d4 = score_edges(pred, gt);
    let (d5, matched) = score_order(pred, gt);
    let d6 = score_tables_content(pred, gt);

    let mut weight_sum = 0.0;
    let mut score_sum = 0.0;
    let dims = [
        (present_paragraphs(pred, gt), WEIGHT_PARAGRAPH, d0.value),
        (present_headings(pred, gt), WEIGHT_HEADING, d1.value),
        (present_lists(pred, gt), WEIGHT_LIST, d2.value),
        (present_tables(pred, gt), WEIGHT_TABLE, d3.value),
        (present_edges(pred, gt), WEIGHT_EDGES, d4.value),
    ];
    for (present, weight, value) in dims {
        if present {
            weight_sum += weight;
            score_sum += weight * value;
        }
    }
    let base = if weight_sum > 0.0 { score_sum / weight_sum } else { 1.0 };
    let sf1 = fold_order_into_sf1(base, d5, matched);

    // Report-only P/R split, parallel to `dimensions()`. Reading order (D5) is a
    // sequence agreement, not a set match, so its "precision"/"recall" mirror the
    // score rather than being computed independently.
    let breakdown = vec![
        d0.breakdown(),
        d1.breakdown(),
        d2.breakdown(),
        d3.breakdown(),
        d4.breakdown(),
        DimBreakdown {
            f1: d5,
            precision: d5,
            recall: d5,
        },
        d6.breakdown(),
    ];

    StructuralScore {
        d0_paragraph: d0.value,
        d1_heading: d1.value,
        d2_list: d2.value,
        d3_table: d3.value,
        d4_edges: d4.value,
        d5_order: d5,
        d6_table_content: d6.value,
        sf1,
        breakdown,
    }
}

/// Parse two Markdown documents and compute canonical SF1.
pub fn score_markdown(predicted: &str, ground_truth: &str) -> StructuralScore {
    score_structural(
        &StructuralSidecar::from_markdown(predicted),
        &StructuralSidecar::from_markdown(ground_truth),
    )
}

/// Content-based node matches used only to explain a canonical SF1 score.
pub(crate) fn diagnostic_matches(pred: &StructuralSidecar, gt: &StructuralSidecar) -> Vec<(usize, usize, f64)> {
    let pred_text: Vec<String> = pred.nodes.iter().map(StructuralNode::repr_text).collect();
    let gt_text: Vec<String> = gt.nodes.iter().map(StructuralNode::repr_text).collect();
    greedy_match(&pred_text, &gt_text)
}

/// A dimension score: the F1 `value` that feeds the SF1 rollup, plus the
/// precision/recall split kept for report-only diagnostics.
struct Dim {
    value: f64,
    precision: f64,
    recall: f64,
}

impl Dim {
    /// Build a dimension score from matched credit against pred/gt cardinalities.
    fn from_credit(matched_credit: f64, n_pred: usize, n_gt: usize) -> Self {
        let (value, precision, recall) = f1_parts_from(matched_credit, n_pred, n_gt);
        Self {
            value,
            precision,
            recall,
        }
    }

    /// A dimension both sides agree is absent (or a perfect match): all 1.0.
    fn perfect() -> Self {
        Self {
            value: 1.0,
            precision: 1.0,
            recall: 1.0,
        }
    }

    /// A dimension present on exactly one side (fabricated or dropped): all 0.0.
    fn zero() -> Self {
        Self {
            value: 0.0,
            precision: 0.0,
            recall: 0.0,
        }
    }

    /// The report-only precision/recall/F1 view of this dimension.
    fn breakdown(&self) -> DimBreakdown {
        DimBreakdown {
            f1: self.value,
            precision: self.precision,
            recall: self.recall,
        }
    }
}

fn paragraph_texts(s: &StructuralSidecar) -> Vec<String> {
    s.nodes
        .iter()
        .filter_map(|n| match n {
            StructuralNode::Paragraph { text } | StructuralNode::Formula { text, .. } => Some(text.clone()),
            StructuralNode::Image { alt } => Some(alt.clone()),
            StructuralNode::Figure { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

fn present_paragraphs(p: &StructuralSidecar, g: &StructuralSidecar) -> bool {
    !paragraph_texts(p).is_empty() || !paragraph_texts(g).is_empty()
}

fn score_paragraphs(pred: &StructuralSidecar, gt: &StructuralSidecar) -> Dim {
    let pp = paragraph_texts(pred);
    let gg = paragraph_texts(gt);
    let credit: f64 = greedy_match(&pp, &gg).iter().map(|(_, _, s)| *s).sum();
    Dim::from_credit(credit, pp.len(), gg.len())
}

struct HeadingInfo {
    text: String,
    level: u8,
    ancestors: HashSet<String>,
}

fn heading_infos(s: &StructuralSidecar) -> Vec<HeadingInfo> {
    s.nodes
        .iter()
        .filter_map(|n| match n {
            StructuralNode::Heading { level, path, text, .. } => Some(HeadingInfo {
                text: text.clone(),
                level: *level,
                ancestors: path.iter().map(|t| t.to_ascii_lowercase()).collect(),
            }),
            _ => None,
        })
        .collect()
}

fn present_headings(p: &StructuralSidecar, g: &StructuralSidecar) -> bool {
    !heading_infos(p).is_empty() || !heading_infos(g).is_empty()
}

fn score_headings(pred: &StructuralSidecar, gt: &StructuralSidecar) -> Dim {
    let ph = heading_infos(pred);
    let gh = heading_infos(gt);
    let ptext: Vec<String> = ph.iter().map(|h| h.text.clone()).collect();
    let gtext: Vec<String> = gh.iter().map(|h| h.text.clone()).collect();
    let credit: f64 = greedy_match(&ptext, &gtext)
        .iter()
        .map(|(i, j, sim)| {
            let a = &ph[*i];
            let b = &gh[*j];
            let level_score = if a.level == b.level {
                1.0
            } else {
                (1.0 - HEADING_LEVEL_STEP * (a.level as i16 - b.level as i16).unsigned_abs() as f64).max(0.0)
            };
            let ancestor_sim = jaccard(&a.ancestors, &b.ancestors);
            sim * (STRUCT_SPLIT * level_score + STRUCT_SPLIT * ancestor_sim)
        })
        .sum();
    Dim::from_credit(credit, ph.len(), gh.len())
}

struct ListInfo {
    text: String,
    depth: usize,
    ordered: bool,
}

fn list_infos(s: &StructuralSidecar) -> Vec<ListInfo> {
    s.nodes
        .iter()
        .filter_map(|n| match n {
            StructuralNode::ListItem {
                depth, ordered, text, ..
            } => Some(ListInfo {
                text: text.clone(),
                depth: *depth,
                ordered: *ordered,
            }),
            _ => None,
        })
        .collect()
}

fn present_lists(p: &StructuralSidecar, g: &StructuralSidecar) -> bool {
    !list_infos(p).is_empty() || !list_infos(g).is_empty()
}

fn score_lists(pred: &StructuralSidecar, gt: &StructuralSidecar) -> Dim {
    let pl = list_infos(pred);
    let gl = list_infos(gt);
    let ptext: Vec<String> = pl.iter().map(|l| l.text.clone()).collect();
    let gtext: Vec<String> = gl.iter().map(|l| l.text.clone()).collect();
    let credit: f64 = greedy_match(&ptext, &gtext)
        .iter()
        .map(|(i, j, sim)| {
            let a = &pl[*i];
            let b = &gl[*j];
            let depth_score = if a.depth == b.depth {
                1.0
            } else {
                (1.0 - LIST_DEPTH_STEP * (a.depth as i64 - b.depth as i64).unsigned_abs() as f64).max(0.0)
            };
            let ordered_score = if a.ordered == b.ordered { 1.0 } else { 0.0 };
            sim * (STRUCT_SPLIT * depth_score + STRUCT_SPLIT * ordered_score)
        })
        .sum();
    Dim::from_credit(credit, pl.len(), gl.len())
}

fn tables(s: &StructuralSidecar) -> Vec<&TableNode> {
    s.nodes
        .iter()
        .filter_map(|n| match n {
            StructuralNode::Table(t) => Some(t),
            _ => None,
        })
        .collect()
}

fn present_tables(p: &StructuralSidecar, g: &StructuralSidecar) -> bool {
    !tables(p).is_empty() || !tables(g).is_empty()
}

/// GriTS-like grid F1 between two tables: cells sharing a `(row, col)` origin
/// contribute `content_sim * span_agreement`.
fn grits(pred: &TableNode, gt: &TableNode) -> f64 {
    let gt_by_pos: HashMap<(usize, usize), &Cell> = gt.cells.iter().map(|c| ((c.row, c.col), c)).collect();
    let mut credit = 0.0;
    for pc in &pred.cells {
        if let Some(gc) = gt_by_pos.get(&(pc.row, pc.col)) {
            let sim = content_sim(&pc.text, &gc.text);
            let row_ok = pc.rowspan == gc.rowspan;
            let col_ok = pc.colspan == gc.colspan;
            let span = match (row_ok, col_ok) {
                (true, true) => 1.0,
                (true, false) | (false, true) => SPAN_HALF_CREDIT,
                (false, false) => 0.0,
            };
            credit += sim * span;
        }
    }
    f1_from(credit, pred.cells.len(), gt.cells.len())
}

fn score_tables(pred: &StructuralSidecar, gt: &StructuralSidecar) -> Dim {
    let pt = tables(pred);
    let gt_tables = tables(gt);
    if pt.is_empty() && gt_tables.is_empty() {
        return Dim::perfect();
    }
    if pt.is_empty() || gt_tables.is_empty() {
        // A fabricated table (pred-only) or a dropped table (gt-only) scores 0. ~keep
        return Dim::zero();
    }
    let ptext: Vec<String> = pt
        .iter()
        .map(|t| t.cells.iter().map(|c| c.text.as_str()).collect::<Vec<_>>().join(" "))
        .collect();
    let gtext: Vec<String> = gt_tables
        .iter()
        .map(|t| t.cells.iter().map(|c| c.text.as_str()).collect::<Vec<_>>().join(" "))
        .collect();
    let credit: f64 = greedy_match(&ptext, &gtext)
        .iter()
        .map(|(i, j, _)| grits(pt[*i], gt_tables[*j]))
        .sum();
    Dim::from_credit(credit, pt.len(), gt_tables.len())
}

/// GriTS-Con: position-independent cell-content F1 between two tables. Unlike
/// [`grits`] (which credits only cells sharing a `(row, col)` origin), this
/// greedily matches cells by text similarity, so it measures whether the right
/// *content* was recovered regardless of where the grid placed it.
fn grits_con(pred: &TableNode, gt: &TableNode) -> f64 {
    let ptext: Vec<String> = pred.cells.iter().map(|c| c.text.clone()).collect();
    let gtext: Vec<String> = gt.cells.iter().map(|c| c.text.clone()).collect();
    let credit: f64 = greedy_match(&ptext, &gtext).iter().map(|(_, _, s)| *s).sum();
    f1_from(credit, pred.cells.len(), gt.cells.len())
}

/// D6 — table cell-content F1 (GriTS-Con). Mirrors [`score_tables`]'s table
/// matching but scores each matched pair on content recovery ([`grits_con`])
/// rather than grid topology. Report-only; not folded into SF1 (see Phase 2).
fn score_tables_content(pred: &StructuralSidecar, gt: &StructuralSidecar) -> Dim {
    let pt = tables(pred);
    let gt_tables = tables(gt);
    if pt.is_empty() && gt_tables.is_empty() {
        return Dim::perfect();
    }
    if pt.is_empty() || gt_tables.is_empty() {
        return Dim::zero();
    }
    let ptext: Vec<String> = pt
        .iter()
        .map(|t| t.cells.iter().map(|c| c.text.as_str()).collect::<Vec<_>>().join(" "))
        .collect();
    let gtext: Vec<String> = gt_tables
        .iter()
        .map(|t| t.cells.iter().map(|c| c.text.as_str()).collect::<Vec<_>>().join(" "))
        .collect();
    let credit: f64 = greedy_match(&ptext, &gtext)
        .iter()
        .map(|(i, j, _)| grits_con(pt[*i], gt_tables[*j]))
        .sum();
    Dim::from_credit(credit, pt.len(), gt_tables.len())
}

struct Edge {
    caption: String,
    target: String,
}

/// Collect caption/footnote binding edges (only bound ones — an unbound caption
/// contributes no edge).
fn edges(s: &StructuralSidecar) -> Vec<Edge> {
    s.nodes
        .iter()
        .filter_map(|n| match n {
            StructuralNode::Caption {
                binds_to: Some(t),
                text,
            }
            | StructuralNode::Footnote {
                binds_to: Some(t),
                text,
            } => Some(Edge {
                caption: text.clone(),
                target: s.nodes.get(*t).map(|n| n.repr_text()).unwrap_or_default(),
            }),
            _ => None,
        })
        .collect()
}

fn present_edges(p: &StructuralSidecar, g: &StructuralSidecar) -> bool {
    !edges(p).is_empty() || !edges(g).is_empty()
}

fn score_edges(pred: &StructuralSidecar, gt: &StructuralSidecar) -> Dim {
    let pe = edges(pred);
    let ge = edges(gt);
    // Match edges by caption similarity; credit weights in target agreement too. ~keep
    let ptext: Vec<String> = pe.iter().map(|e| e.caption.clone()).collect();
    let gtext: Vec<String> = ge.iter().map(|e| e.caption.clone()).collect();
    let credit: f64 = greedy_match(&ptext, &gtext)
        .iter()
        .map(|(i, j, sim)| {
            let target_sim = content_sim(&pe[*i].target, &ge[*j].target);
            sim * target_sim
        })
        .sum();
    Dim::from_credit(credit, pe.len(), ge.len())
}

/// D5: match every node by content, then score reading order via LIS.
/// Returns `(order_score, matched_pair_count)`.
fn score_order(pred: &StructuralSidecar, gt: &StructuralSidecar) -> (f64, usize) {
    let ptext: Vec<String> = pred.nodes.iter().map(|n| n.repr_text()).collect();
    let gtext: Vec<String> = gt.nodes.iter().map(|n| n.repr_text()).collect();
    let matches = greedy_match(&ptext, &gtext);

    let pred_pos = order_positions(&pred.reading_order, pred.nodes.len());
    let gt_pos = order_positions(&gt.reading_order, gt.nodes.len());

    let order_pairs: Vec<(usize, usize)> = matches.iter().map(|(i, j, _)| (gt_pos[*j], pred_pos[*i])).collect();
    (compute_order_score(&order_pairs), order_pairs.len())
}

/// Map node index → its position in `reading_order` (identity fallback).
fn order_positions(reading_order: &[usize], n: usize) -> Vec<usize> {
    let mut pos = vec![0usize; n];
    if reading_order.len() == n {
        for (p, &idx) in reading_order.iter().enumerate() {
            if idx < n {
                pos[idx] = p;
            }
        }
    } else {
        for (i, slot) in pos.iter_mut().enumerate() {
            *slot = i;
        }
    }
    pos
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markdown_quality::ORDER_SCORE_FLOOR;

    /// A representative document exercising every scored dimension.
    const SAMPLE: &str = "\
# Introduction

Opening paragraph with several words describing the overall context here.

## Background

Background prose paragraph explaining the prior work and the motivation clearly.

### Details

- First bullet point item
- Second bullet point item
    1. Nested ordered alpha
    2. Nested ordered beta

| Name | Age | City |
|------|-----|------|
| Alice | 30 | Berlin |
| Bob | 25 | Munich |

![architecture](arch.png)

Figure 1: The overall system architecture and its components.
";

    fn baseline() -> StructuralSidecar {
        StructuralSidecar::from_markdown(SAMPLE)
    }

    fn sf1(pred: &StructuralSidecar, gt: &StructuralSidecar) -> f64 {
        score_structural(pred, gt).sf1
    }

    #[test]
    fn f1_parts_from_splits_precision_and_recall() {
        // Empty on both sides is a perfect (vacuous) match.
        assert_eq!(f1_parts_from(0.0, 0, 0), (1.0, 1.0, 1.0));
        // Present on one side only scores zero across the board.
        assert_eq!(f1_parts_from(0.0, 3, 0), (0.0, 0.0, 0.0));
        // Over-emission: 1 unit of credit against 4 predicted / 1 truth ⇒ p<r.
        let (f1, precision, recall) = f1_parts_from(1.0, 4, 1);
        assert!((precision - 0.25).abs() < 1e-9, "precision {precision}");
        assert!((recall - 1.0).abs() < 1e-9, "recall {recall}");
        assert!(precision < recall, "over-emission should read as low precision");
        assert!(f1 > 0.0 && f1 < 1.0);
    }

    #[test]
    fn breakdown_reads_fabrication_as_low_precision() {
        // GT has one heading; prediction fabricates three extra unmatched ones.
        let gt = StructuralSidecar::from_markdown("# Real\n\nBody paragraph text here.\n");
        let pred = StructuralSidecar::from_markdown(
            "# Real\n\n## Fabricated one\n\n## Fabricated two\n\n## Fabricated three\n\nBody paragraph text here.\n",
        );
        let score = score_structural(&pred, &gt);
        // dimensions()/breakdown are parallel; heading is index 1.
        let (name, heading) = score.dimensions_pr()[1];
        assert_eq!(name, "heading");
        assert!(
            heading.precision < heading.recall,
            "fabricated headings should depress precision below recall: p{} r{}",
            heading.precision,
            heading.recall
        );
    }

    #[test]
    fn breakdown_reads_omission_as_low_recall() {
        // Mirror image: GT has four headings, prediction recovers only one.
        let gt = StructuralSidecar::from_markdown(
            "# Real\n\n## Second\n\n## Third\n\n## Fourth\n\nBody paragraph text here.\n",
        );
        let pred = StructuralSidecar::from_markdown("# Real\n\nBody paragraph text here.\n");
        let heading = score_structural(&pred, &gt).dimensions_pr()[1].1;
        assert!(
            heading.recall < heading.precision,
            "dropped headings should depress recall below precision: p{} r{}",
            heading.precision,
            heading.recall
        );
    }

    #[test]
    fn grits_con_ignores_cell_position() {
        let cell = |row, col, text: &str| Cell {
            row,
            col,
            rowspan: 1,
            colspan: 1,
            is_header: false,
            text: text.to_string(),
        };
        let gt = TableNode {
            n_rows: 1,
            n_cols: 2,
            header_rows: 0,
            cells: vec![cell(0, 0, "alpha"), cell(0, 1, "beta")],
            spans_recoverable: false,
        };
        // Same content, cells transposed to different (row, col) origins.
        let pred = TableNode {
            n_rows: 2,
            n_cols: 1,
            header_rows: 0,
            cells: vec![cell(0, 0, "beta"), cell(1, 0, "alpha")],
            spans_recoverable: false,
        };
        let topology = grits(&pred, &gt);
        let content = grits_con(&pred, &gt);
        assert!(
            content > topology,
            "content F1 {content} should exceed topology F1 {topology} for correct-but-misplaced cells"
        );
        assert!(
            (content - 1.0).abs() < 1e-9,
            "identical content should score 1.0, got {content}"
        );
    }

    #[test]
    fn table_content_dimension_is_reported_but_unfolded() {
        let score = score_markdown(SAMPLE, SAMPLE);
        // A 7th named dimension carrying the GriTS-Con content F1.
        let dims = score.dimensions();
        assert_eq!(dims.len(), 7);
        assert_eq!(dims[6].0, "table_content");
        assert!(
            (score.d6_table_content - 1.0).abs() < 1e-9,
            "identical doc ⇒ content F1 1.0"
        );
        // Report-only: an identical doc still scores a perfect SF1, and d6 lives
        // outside the weighted rollup entirely.
        assert!((score.sf1 - 1.0).abs() < 1e-9);
    }

    #[test]
    fn test_parser_extracts_all_dimensions() {
        let s = baseline();
        assert!(
            heading_infos(&s).len() >= 3,
            "expected >=3 headings, got {}",
            heading_infos(&s).len()
        );
        assert!(
            list_infos(&s).len() >= 4,
            "expected >=4 list items, got {}",
            list_infos(&s).len()
        );
        assert_eq!(tables(&s).len(), 1, "expected exactly one table");
        assert!(!edges(&s).is_empty(), "expected a caption binding edge");
        let max_depth = list_infos(&s).iter().map(|l| l.depth).max().unwrap();
        assert!(max_depth >= 1, "nested list depth not captured: {max_depth}");
        assert!(list_infos(&s).iter().any(|l| l.ordered), "ordered item not captured");
    }

    #[test]
    fn test_gfm_table_spans_not_recoverable() {
        let s = baseline();
        let t = tables(&s)[0];
        assert!(!t.spans_recoverable, "GFM pipe tables cannot express spans");
        assert!(t.cells.iter().all(|c| c.rowspan == 1 && c.colspan == 1));
        assert_eq!((t.n_rows, t.n_cols), (3, 3));
        let positions: HashSet<(usize, usize)> = t.cells.iter().map(|c| (c.row, c.col)).collect();
        assert_eq!(positions.len(), t.cells.len(), "table cell origins must be unique");
    }

    #[test]
    fn test_identity_is_one() {
        let s = baseline();
        assert!(
            (sf1(&s, &s) - 1.0).abs() < 1e-9,
            "identity must be 1.0, got {}",
            sf1(&s, &s)
        );
    }

    #[test]
    fn test_identity_via_json_roundtrip() {
        let s = baseline();
        let json = s.to_json().unwrap();
        let back: StructuralSidecar = serde_json::from_str(&json).unwrap();
        assert!((sf1(&back, &s) - 1.0).abs() < 1e-9);
    }

    fn assert_drops(perturbed: &StructuralSidecar, label: &str) {
        let gt = baseline();
        let score = sf1(perturbed, &gt);
        assert!(score < 1.0 - 1e-9, "{label} must score below identity, got {score}");
    }

    fn drop_first_heading(mut s: StructuralSidecar) -> StructuralSidecar {
        let idx = s
            .nodes
            .iter()
            .position(|n| matches!(n, StructuralNode::Heading { .. }))
            .unwrap();
        s.nodes.remove(idx);
        s.reading_order = (0..s.nodes.len()).collect();
        s
    }

    fn flatten_headings(mut s: StructuralSidecar) -> StructuralSidecar {
        for n in &mut s.nodes {
            if let StructuralNode::Heading {
                level, path, parent, ..
            } = n
            {
                *level = 1;
                path.clear();
                *parent = None;
            }
        }
        s
    }

    fn unnest_list(mut s: StructuralSidecar) -> StructuralSidecar {
        for n in &mut s.nodes {
            if let StructuralNode::ListItem { depth, parent_item, .. } = n {
                *depth = 0;
                *parent_item = None;
            }
        }
        s
    }

    fn flip_ordered(mut s: StructuralSidecar) -> StructuralSidecar {
        for n in &mut s.nodes {
            if let StructuralNode::ListItem { ordered, .. } = n {
                *ordered = !*ordered;
            }
        }
        s
    }

    fn merge_table_row(mut s: StructuralSidecar) -> StructuralSidecar {
        for n in &mut s.nodes {
            if let StructuralNode::Table(t) = n {
                let last_row = t.n_rows.saturating_sub(1);
                t.cells.retain(|c| c.row != last_row);
                t.n_rows = last_row;
            }
        }
        s
    }

    fn corrupt_rowspan(mut s: StructuralSidecar) -> StructuralSidecar {
        for n in &mut s.nodes {
            if let StructuralNode::Table(t) = n
                && let Some(cell) = t.cells.first_mut()
            {
                cell.rowspan = 2;
                t.spans_recoverable = true;
            }
        }
        s
    }

    fn fabricate_table(mut s: StructuralSidecar) -> StructuralSidecar {
        s.nodes.push(StructuralNode::Table(TableNode {
            n_rows: 2,
            n_cols: 2,
            header_rows: 1,
            spans_recoverable: false,
            cells: vec![
                Cell {
                    row: 0,
                    col: 0,
                    rowspan: 1,
                    colspan: 1,
                    is_header: true,
                    text: "Q".into(),
                },
                Cell {
                    row: 0,
                    col: 1,
                    rowspan: 1,
                    colspan: 1,
                    is_header: true,
                    text: "R".into(),
                },
                Cell {
                    row: 1,
                    col: 0,
                    rowspan: 1,
                    colspan: 1,
                    is_header: false,
                    text: "99".into(),
                },
                Cell {
                    row: 1,
                    col: 1,
                    rowspan: 1,
                    colspan: 1,
                    is_header: false,
                    text: "88".into(),
                },
            ],
        }));
        s.reading_order = (0..s.nodes.len()).collect();
        s
    }

    fn transpose_table(mut s: StructuralSidecar) -> StructuralSidecar {
        for n in &mut s.nodes {
            if let StructuralNode::Table(t) = n {
                for c in &mut t.cells {
                    std::mem::swap(&mut c.row, &mut c.col);
                }
                std::mem::swap(&mut t.n_rows, &mut t.n_cols);
            }
        }
        s
    }

    fn unbind_caption(mut s: StructuralSidecar) -> StructuralSidecar {
        for n in &mut s.nodes {
            if let StructuralNode::Caption { binds_to, .. } = n {
                *binds_to = None;
            }
        }
        s
    }

    fn scramble_reading_order(mut s: StructuralSidecar) -> StructuralSidecar {
        s.reading_order.reverse();
        s
    }

    #[test]
    fn test_drop_heading_drops() {
        assert_drops(&drop_first_heading(baseline()), "drop-heading");
    }

    #[test]
    fn test_flatten_headings_drops() {
        assert_drops(&flatten_headings(baseline()), "flatten-headings");
    }

    #[test]
    fn test_unnest_list_drops() {
        assert_drops(&unnest_list(baseline()), "un-nest-list");
    }

    #[test]
    fn test_flip_ordered_drops() {
        assert_drops(&flip_ordered(baseline()), "flip-ordered");
    }

    #[test]
    fn test_merge_table_row_drops() {
        assert_drops(&merge_table_row(baseline()), "merge-table-row");
    }

    #[test]
    fn test_corrupt_rowspan_drops() {
        assert_drops(&corrupt_rowspan(baseline()), "corrupt-rowspan");
    }

    #[test]
    fn test_fabricate_table_drops() {
        let gt = baseline();
        let fabricated = fabricate_table(baseline());
        let base = sf1(&gt, &gt);
        let after = sf1(&fabricated, &gt);
        assert!(
            after < base - 1e-6,
            "fabricated table must drop the score: base={base} after={after}"
        );
    }

    #[test]
    fn test_transpose_table_drops() {
        assert_drops(&transpose_table(baseline()), "transpose-table");
    }

    #[test]
    fn test_unbind_caption_drops() {
        assert_drops(&unbind_caption(baseline()), "unbind-caption");
    }

    #[test]
    fn test_scramble_reading_order_is_raw_times_floor() {
        // Only reading order changes, so the content-structure base is untouched;
        // a fully reversed order collapses the LIS toward the ORDER_SCORE_FLOOR. ~keep
        let gt = baseline();
        let scrambled = scramble_reading_order(baseline());
        let ordered = sf1(&gt, &gt);
        let scrambled_score = sf1(&scrambled, &gt);
        assert!(
            scrambled_score < ordered,
            "scramble must lower the score: {scrambled_score} !< {ordered}"
        );
        // Lands at the floor (raw · 0.8), modulo the 1/n residue of the LIS. ~keep
        let floor = ordered * ORDER_SCORE_FLOOR;
        assert!(
            scrambled_score >= floor - 1e-9 && scrambled_score <= floor + 0.06,
            "scrambled ({scrambled_score}) should approximate raw·{ORDER_SCORE_FLOOR} = {floor}"
        );
    }

    #[test]
    fn test_monotonic_degradation_chain() {
        let gt = baseline();
        let steps: Vec<StructuralSidecar> = {
            let s0 = baseline();
            let s1 = flatten_headings(s0.clone());
            let s2 = unnest_list(s1.clone());
            let s3 = flip_ordered(s2.clone());
            let s4 = merge_table_row(s3.clone());
            let s5 = unbind_caption(s4.clone());
            vec![s0, s1, s2, s3, s4, s5]
        };
        let scores: Vec<f64> = steps.iter().map(|s| sf1(s, &gt)).collect();
        for w in scores.windows(2) {
            assert!(w[1] <= w[0] + 1e-9, "monotonicity violated: {:?} then {:?}", w[0], w[1]);
        }
        assert!((scores[0] - 1.0).abs() < 1e-9, "chain must start at identity");
        assert!(scores.last().unwrap() < &scores[0], "chain must end below identity");
    }

    #[test]
    fn test_fabricated_table_d3_is_zero_when_gt_has_none() {
        let gt_md = "# Title\n\nJust prose here, no tables at all in the ground truth.\n";
        let pred_md =
            "# Title\n\nJust prose here, no tables at all in the ground truth.\n\n| A | B |\n|---|---|\n| 1 | 2 |\n";
        let gt = StructuralSidecar::from_markdown(gt_md);
        let pred = StructuralSidecar::from_markdown(pred_md);
        let score = score_structural(&pred, &gt);
        assert_eq!(score.d3_table, 0.0, "fabricated table must score D3=0");
        assert!(score.sf1 < score_structural(&gt, &gt).sf1, "fabrication must lower SF1");
    }

    #[test]
    fn test_empty_docs_score_one() {
        let empty = StructuralSidecar::default();
        assert!((sf1(&empty, &empty) - 1.0).abs() < 1e-9);
    }
}
