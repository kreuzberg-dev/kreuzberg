//! Convert an `InternalDocument` into a comrak AST.
//!
//! This module builds a comrak `AstNode` tree from our internal flat element
//! representation.  The resulting tree can be serialized to CommonMark, HTML,
//! or any other format comrak supports via `comrak::format_commonmark` etc.

use std::borrow::Cow;
use std::cell::RefCell;

use comrak::nodes::{
    Ast, AstNode, LineColumn, NodeAlert, NodeCode, NodeCodeBlock, NodeFootnoteDefinition, NodeFootnoteReference,
    NodeHeading, NodeLink, NodeList, NodeMath, NodeTable, NodeValue, TableAlignment,
};

use crate::types::document_structure::{AnnotationKind, ContentLayer, TextAnnotation};
use crate::types::internal::{ElementKind, InternalDocument, InternalElement};

use super::common::{
    FootnoteCollector, NestingKind, RenderState, handle_container_end, is_body_element, is_container_end,
    parse_metadata_entries,
};

/// Allocate a comrak AST node in the arena with the given `NodeValue`.
fn mk<'a>(arena: &'a comrak::Arena<'a>, value: NodeValue) -> &'a AstNode<'a> {
    let ast = Ast::new(value, LineColumn { line: 0, column: 0 });
    arena.alloc(AstNode::new(RefCell::new(ast)))
}

/// Create an inline `Text` node with normalized whitespace.
///
/// Collapses multiple consecutive spaces into one (fixes MD064) and trims
/// leading/trailing whitespace from emphasis spans (fixes MD037).
fn mk_text<'a>(arena: &'a comrak::Arena<'a>, text: &str) -> &'a AstNode<'a> {
    let normalized = normalize_text(text);
    mk(arena, NodeValue::Text(Cow::Owned(normalized)))
}

/// Collapse multiple consecutive spaces into a single space.
fn normalize_text(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut prev_space = false;
    for ch in text.chars() {
        if ch == '\n' || ch == ' ' {
            if !prev_space {
                result.push(' ');
            }
            prev_space = true;
        } else if ch < '\u{20}' && ch != '\t' {
        } else {
            prev_space = false;
            result.push(ch);
        }
    }
    result
}

/// Check whether ALL annotations on an element cover the full text with the
/// same annotation kind.  Returns the kind if so.
fn uniform_annotation_kind(elem: &InternalElement) -> Option<&AnnotationKind> {
    let mut formatting_ann: Option<&AnnotationKind> = None;
    for ann in &elem.annotations {
        match &ann.kind {
            AnnotationKind::Bold | AnnotationKind::Italic | AnnotationKind::Strikethrough => {
                if ann.start == 0 && ann.end as usize >= elem.text.len() {
                    if formatting_ann.is_some() {
                        return None;
                    }
                    formatting_ann = Some(&ann.kind);
                } else {
                    return None;
                }
            }
            _ => {}
        }
    }
    formatting_ann
}

/// Check if text ends at a sentence boundary (period, exclamation, question mark, colon).
fn ends_at_sentence_boundary(text: &str) -> bool {
    let trimmed = text.trim_end();
    trimmed.ends_with('.') || trimmed.ends_with('!') || trimmed.ends_with('?') || trimmed.ends_with(':')
}

/// Returns true if two annotation kinds are "merge-compatible" (same variant).
fn same_annotation_variant(a: &AnnotationKind, b: &AnnotationKind) -> bool {
    std::mem::discriminant(a) == std::mem::discriminant(b)
}

/// Pre-process elements: merge consecutive fully-annotated paragraphs of the
/// same kind into a single paragraph.  Returns a new element list with the
/// consolidated paragraphs.
fn consolidate_paragraphs(elements: &[InternalElement]) -> Vec<ConsolidatedElement> {
    let mut result: Vec<ConsolidatedElement> = Vec::with_capacity(elements.len());
    let mut i = 0;

    while i < elements.len() {
        let elem = &elements[i];

        if elem.kind == ElementKind::Paragraph && elem.layer == ContentLayer::Body && !elem.text.is_empty() {
            let uniform = uniform_annotation_kind(elem);
            tracing::trace!(
                idx = i,
                text_len = elem.text.len(),
                annotation_count = elem.annotations.len(),
                uniform_kind = ?uniform,
                text_preview = elem.text.chars().take(60).collect::<String>(),
                "paragraph consolidation candidate"
            );
        }
        if elem.kind == ElementKind::Paragraph
            && elem.layer == ContentLayer::Body
            && !elem.text.is_empty()
            && let Some(kind) = uniform_annotation_kind(elem)
        {
            let mut merged_text = elem.text.clone();
            let mut j = i + 1;
            while j < elements.len() {
                if ends_at_sentence_boundary(&merged_text) {
                    break;
                }
                let next = &elements[j];
                if next.kind != ElementKind::Paragraph || next.layer != ContentLayer::Body || next.text.is_empty() {
                    break;
                }
                if let Some(next_kind) = uniform_annotation_kind(next)
                    && same_annotation_variant(kind, next_kind)
                {
                    merged_text.push(' ');
                    merged_text.push_str(&next.text);
                    j += 1;
                    continue;
                }
                break;
            }

            if j > i + 1 {
                tracing::debug!(
                    start_idx = i,
                    end_idx = j,
                    merged_count = j - i,
                    kind = ?kind,
                    merged_text_len = merged_text.len(),
                    "consolidated paragraphs"
                );
                let ann = TextAnnotation {
                    start: 0,
                    end: merged_text.len() as u32,
                    kind: kind.clone(),
                };
                result.push(ConsolidatedElement::Merged {
                    text: merged_text,
                    annotations: vec![ann],
                });
                i = j;
                continue;
            }
        }

        result.push(ConsolidatedElement::Original(i));
        i += 1;
    }

    result
}

/// Either a reference to an original element (by index) or a merged paragraph.
enum ConsolidatedElement {
    Original(usize),
    Merged {
        text: String,
        annotations: Vec<TextAnnotation>,
    },
}

impl ConsolidatedElement {
    fn resolve<'b>(&'b self, elements: &'b [InternalElement]) -> ElementView<'b> {
        match self {
            ConsolidatedElement::Original(idx) => ElementView::Ref(&elements[*idx]),
            ConsolidatedElement::Merged { text, annotations, .. } => ElementView::Merged { text, annotations },
        }
    }

    fn original_index(&self) -> Option<usize> {
        match self {
            ConsolidatedElement::Original(idx) => Some(*idx),
            ConsolidatedElement::Merged { .. } => None,
        }
    }
}

enum ElementView<'a> {
    Ref(&'a InternalElement),
    Merged {
        text: &'a str,
        annotations: &'a [TextAnnotation],
    },
}

/// Build inline comrak nodes from text with byte-range annotations.
///
/// Sorts annotations by start position, preferring wider spans first and `Link`
/// before formatting annotations when ranges coincide.  Annotations fully
/// contained within an outer annotation are nested recursively rather than
/// skipped, so `Bold[0,5] + Link[0,5]` produces `[**text**](url)` instead of
/// dropping the link.
fn build_inlines<'a>(
    arena: &'a comrak::Arena<'a>,
    parent: &'a AstNode<'a>,
    text: &str,
    annotations: &[TextAnnotation],
) {
    if annotations.is_empty() {
        if !text.is_empty() {
            parent.append(mk_text(arena, text));
        }
        return;
    }

    let mut sorted: Vec<&TextAnnotation> = annotations.iter().collect();
    sorted.sort_by_key(|a| {
        let width = a.end.saturating_sub(a.start);
        let priority: u8 = match &a.kind {
            AnnotationKind::Link { .. } => 0,
            _ => 1,
        };
        (a.start, std::cmp::Reverse(width), priority)
    });

    let len = text.len() as u32;
    let mut pos: u32 = 0;
    let mut i = 0;

    while i < sorted.len() {
        let ann = sorted[i];
        let start = text.ceil_char_boundary(ann.start.min(len) as usize) as u32;
        let end = text.floor_char_boundary(ann.end.min(len) as usize) as u32;

        if start < pos {
            tracing::trace!(
                ann_start = start,
                ann_end = end,
                current_pos = pos,
                "skipping annotation already consumed as inner"
            );
            i += 1;
            continue;
        }

        if start >= end {
            i += 1;
            continue;
        }

        if start > pos {
            let gap = &text[pos as usize..start as usize];
            if !gap.is_empty() {
                parent.append(mk_text(arena, gap));
            }
        }

        let span = &text[start as usize..end as usize];

        let inner: Vec<TextAnnotation> = sorted[i + 1..]
            .iter()
            .filter_map(|ia| {
                let is = text.ceil_char_boundary(ia.start.min(len) as usize) as u32;
                let ie = text.floor_char_boundary(ia.end.min(len) as usize) as u32;
                if is >= start && ie <= end && is < ie {
                    Some(TextAnnotation {
                        start: is - start,
                        end: ie - start,
                        kind: ia.kind.clone(),
                    })
                } else {
                    None
                }
            })
            .collect();

        append_annotated_span(arena, parent, span, &ann.kind, &inner);
        pos = end;

        i += 1;
        while i < sorted.len() {
            let ns = text.ceil_char_boundary(sorted[i].start.min(len) as usize) as u32;
            if ns < end {
                i += 1;
            } else {
                break;
            }
        }
    }

    if (pos as usize) < text.len() {
        let tail = &text[pos as usize..];
        if !tail.is_empty() {
            parent.append(mk_text(arena, tail));
        }
    }
}

/// Create a comrak inline wrapper node for the given annotation kind and
/// append it to `parent`.
///
/// `inner` contains annotations (with offsets relative to `span`) that are
/// fully contained within this span and should be rendered as nested nodes
/// rather than plain text.
///
/// Trims leading/trailing whitespace from emphasis/strong spans to avoid
/// MD037 (spaces inside emphasis markers). Whitespace outside the markers
/// is emitted as separate Text nodes.
fn append_annotated_span<'a>(
    arena: &'a comrak::Arena<'a>,
    parent: &'a AstNode<'a>,
    span: &str,
    kind: &AnnotationKind,
    inner: &[TextAnnotation],
) {
    let (leading_ws, trimmed, trailing_ws) = if matches!(
        kind,
        AnnotationKind::Bold | AnnotationKind::Italic | AnnotationKind::Strikethrough
    ) {
        let trimmed = span.trim();
        if trimmed.is_empty() {
            if !span.is_empty() {
                parent.append(mk_text(arena, span));
            }
            return;
        }
        let leading = &span[..span.len() - span.trim_start().len()];
        let trailing = &span[span.trim_end().len()..];
        (leading, trimmed, trailing)
    } else {
        ("", span, "")
    };

    let leading_len = leading_ws.len() as u32;
    let trimmed_len = trimmed.len() as u32;
    let adjusted_inner: Vec<TextAnnotation>;
    let inner_for_node: &[TextAnnotation] = if leading_len == 0 {
        inner
    } else {
        adjusted_inner = inner
            .iter()
            .filter_map(|ia| {
                let is = ia.start.saturating_sub(leading_len);
                let ie = ia.end.saturating_sub(leading_len).min(trimmed_len);
                if is < ie {
                    Some(TextAnnotation {
                        start: is,
                        end: ie,
                        kind: ia.kind.clone(),
                    })
                } else {
                    None
                }
            })
            .collect();
        &adjusted_inner
    };

    if !leading_ws.is_empty() {
        parent.append(mk_text(arena, leading_ws));
    }

    match kind {
        AnnotationKind::Bold => {
            let strong = mk(arena, NodeValue::Strong);
            build_inlines(arena, strong, trimmed, inner_for_node);
            parent.append(strong);
        }
        AnnotationKind::Italic => {
            let emph = mk(arena, NodeValue::Emph);
            build_inlines(arena, emph, trimmed, inner_for_node);
            parent.append(emph);
        }
        AnnotationKind::Code => {
            if !trimmed.is_empty() {
                let code = mk(
                    arena,
                    NodeValue::Code(NodeCode {
                        num_backticks: 1,
                        literal: normalize_text(trimmed),
                    }),
                );
                parent.append(code);
            }
        }
        AnnotationKind::Strikethrough => {
            let strike = mk(arena, NodeValue::Strikethrough);
            build_inlines(arena, strike, trimmed, inner_for_node);
            parent.append(strike);
        }
        AnnotationKind::Underline => {
            let underline = mk(arena, NodeValue::Underline);
            build_inlines(arena, underline, trimmed, inner_for_node);
            parent.append(underline);
        }
        AnnotationKind::Subscript => {
            let sub = mk(arena, NodeValue::Subscript);
            build_inlines(arena, sub, trimmed, inner_for_node);
            parent.append(sub);
        }
        AnnotationKind::Superscript => {
            let sup = mk(arena, NodeValue::Superscript);
            build_inlines(arena, sup, trimmed, inner_for_node);
            parent.append(sup);
        }
        AnnotationKind::Highlight => {
            let hl = mk(arena, NodeValue::Highlight);
            build_inlines(arena, hl, trimmed, inner_for_node);
            parent.append(hl);
        }
        AnnotationKind::Link { url, title } => {
            let link = mk(
                arena,
                NodeValue::Link(Box::new(NodeLink {
                    url: url.clone(),
                    title: title.as_deref().unwrap_or("").to_string(),
                })),
            );
            build_inlines(arena, link, trimmed, inner);
            parent.append(link);
        }
        AnnotationKind::Color { .. } | AnnotationKind::FontSize { .. } | AnnotationKind::Custom { .. } => {
            parent.append(mk_text(arena, trimmed));
        }
    }

    if !trailing_ws.is_empty() {
        parent.append(mk_text(arena, trailing_ws));
    }
}

/// Build a comrak `Table` subtree from a 2-D cell grid.
fn build_table<'a>(arena: &'a comrak::Arena<'a>, cells: &[Vec<String>], has_header: bool) -> &'a AstNode<'a> {
    let num_cols = cells.iter().map(|r| r.len()).max().unwrap_or(0);
    let synthetic_header_rows = usize::from(!has_header);

    let table_node = mk(
        arena,
        NodeValue::Table(Box::new(NodeTable {
            alignments: vec![TableAlignment::None; num_cols],
            num_columns: num_cols,
            num_rows: cells.len() + synthetic_header_rows,
            num_nonempty_cells: cells.iter().flat_map(|r| r.iter()).filter(|c| !c.is_empty()).count(),
        })),
    );

    if !has_header {
        let header_row = mk(arena, NodeValue::TableRow(true));
        for _ in 0..num_cols {
            header_row.append(mk(arena, NodeValue::TableCell));
        }
        table_node.append(header_row);
    }

    for (row_idx, row) in cells.iter().enumerate() {
        let is_header = has_header && row_idx == 0;
        let row_node = mk(arena, NodeValue::TableRow(is_header));

        for col in 0..num_cols {
            let cell_node = mk(arena, NodeValue::TableCell);
            let content = row.get(col).map(|s| s.as_str()).unwrap_or("");
            if !content.is_empty() {
                cell_node.append(mk_text(arena, content));
            }
            row_node.append(cell_node);
        }

        table_node.append(row_node);
    }

    table_node
}

/// An entry on the container stack, tracking what comrak node to append
/// children into.
struct ContainerEntry<'a> {
    node: &'a AstNode<'a>,
    kind: ContainerKind,
}

#[derive(Clone, Copy)]
enum ContainerKind {
    List,
    BlockQuote,
    Group,
}

/// Build a comrak AST from an `InternalDocument`.
///
/// The returned node is a `Document` root whose children mirror the document
/// body content.  Footnotes are appended after body elements.  Non-body
/// elements (headers, footers) are excluded.
pub(crate) fn build_comrak_ast<'a>(doc: &InternalDocument, arena: &'a comrak::Arena<'a>) -> &'a AstNode<'a> {
    let root = mk(arena, NodeValue::Document);
    let footnotes = FootnoteCollector::new(doc);
    let mut state = RenderState::default();
    let consolidated = consolidate_paragraphs(&doc.elements);

    tracing::debug!(
        total_elements = doc.elements.len(),
        consolidated_elements = consolidated.len(),
        tables = doc.tables.len(),
        images = doc.images.len(),
        "building comrak AST"
    );

    let mut container_stack: Vec<ContainerEntry<'a>> = Vec::new();

    /// Return the current parent node (top of container stack, or root).
    fn current_parent<'b, 'a>(root: &'b &'a AstNode<'a>, stack: &'b [ContainerEntry<'a>]) -> &'a AstNode<'a> {
        stack.last().map(|e| e.node).unwrap_or(*root)
    }

    for consolidated_elem in &consolidated {
        let orig_idx = consolidated_elem.original_index();
        let view = consolidated_elem.resolve(&doc.elements);

        let (elem_kind, elem_text, elem_annotations, elem_depth, _elem_anchor, elem_attributes, render_image_ocr) =
            match &view {
                ElementView::Ref(elem) => {
                    if !is_body_element(elem) {
                        continue;
                    }
                    if is_container_end(elem) {
                        handle_container_end(&elem.kind, &mut state);
                        match elem.kind {
                            ElementKind::ListEnd => pop_container(&mut container_stack, ContainerKind::List),
                            ElementKind::QuoteEnd => pop_container(&mut container_stack, ContainerKind::BlockQuote),
                            ElementKind::GroupEnd => pop_container(&mut container_stack, ContainerKind::Group),
                            _ => {}
                        }
                        continue;
                    }
                    state.pop_to_depth(elem.depth);
                    (
                        elem.kind,
                        elem.text.as_str(),
                        elem.annotations.as_slice(),
                        elem.depth,
                        elem.anchor.as_deref(),
                        elem.attributes.as_ref(),
                        elem.should_render_image_ocr(),
                    )
                }
                ElementView::Merged { text, annotations, .. } => {
                    (ElementKind::Paragraph, *text, *annotations, 0u16, None, None, true)
                }
            };

        let parent = current_parent(&root, &container_stack);

        let parent = if matches!(parent.data.borrow().value, NodeValue::List(..))
            && !matches!(elem_kind, ElementKind::ListItem { .. } | ElementKind::ListEnd)
        {
            let last_item = parent
                .children()
                .filter(|c| matches!(c.data.borrow().value, NodeValue::Item(..) | NodeValue::TaskItem(..)))
                .last();
            match last_item {
                Some(item) => item,
                None => {
                    let list_meta = match parent.data.borrow().value {
                        NodeValue::List(ref m) => *m,
                        _ => unreachable!(),
                    };
                    let implicit_item = mk(arena, NodeValue::Item(list_meta));
                    parent.append(implicit_item);
                    implicit_item
                }
            }
        } else {
            parent
        };

        match elem_kind {
            ElementKind::Title => {
                let heading = mk(
                    arena,
                    NodeValue::Heading(NodeHeading {
                        level: 1,
                        setext: false,
                        closed: false,
                    }),
                );
                build_inlines(arena, heading, elem_text, elem_annotations);
                parent.append(heading);
            }

            ElementKind::Heading { level } => {
                let heading = mk(
                    arena,
                    NodeValue::Heading(NodeHeading {
                        level,
                        setext: false,
                        closed: false,
                    }),
                );
                build_inlines(arena, heading, elem_text, elem_annotations);
                parent.append(heading);
            }

            ElementKind::Paragraph => {
                if elem_text.is_empty() && elem_annotations.is_empty() {
                    tracing::trace!(index = orig_idx, "skipping empty paragraph");
                    continue;
                }
                let para = mk(arena, NodeValue::Paragraph);
                build_inlines(arena, para, elem_text, elem_annotations);
                parent.append(para);
            }

            ElementKind::ListItem { ordered } => {
                let item_list = comrak::nodes::NodeList {
                    list_type: if ordered {
                        comrak::nodes::ListType::Ordered
                    } else {
                        comrak::nodes::ListType::Bullet
                    },
                    bullet_char: b'-',
                    start: 1,
                    tight: true,
                    ..Default::default()
                };
                let item = mk(arena, NodeValue::Item(item_list));
                let item_para = mk(arena, NodeValue::Paragraph);
                build_inlines(arena, item_para, elem_text, elem_annotations);
                item.append(item_para);

                let list_parent = if matches!(parent.data.borrow().value, NodeValue::List(..)) {
                    parent
                } else {
                    let implicit_list = mk(
                        arena,
                        NodeValue::List(comrak::nodes::NodeList {
                            list_type: if ordered {
                                comrak::nodes::ListType::Ordered
                            } else {
                                comrak::nodes::ListType::Bullet
                            },
                            bullet_char: b'-',
                            start: 1,
                            tight: true,
                            ..Default::default()
                        }),
                    );
                    parent.append(implicit_list);
                    implicit_list
                };
                list_parent.append(item);
            }

            ElementKind::Code => {
                let lang = elem_attributes
                    .and_then(|attrs| attrs.get("language").map(|s| s.as_str()))
                    .unwrap_or("");
                let code_block = mk(
                    arena,
                    NodeValue::CodeBlock(Box::new(NodeCodeBlock {
                        fenced: true,
                        fence_char: b'`',
                        fence_length: 3,
                        fence_offset: 0,
                        info: lang.to_string(),
                        literal: elem_text.to_string(),
                        closed: true,
                    })),
                );
                parent.append(code_block);
            }

            ElementKind::Formula => {
                let math = mk(
                    arena,
                    NodeValue::Math(NodeMath {
                        dollar_math: true,
                        display_math: true,
                        literal: elem_text.to_string(),
                    }),
                );
                let para = mk(arena, NodeValue::Paragraph);
                para.append(math);
                parent.append(para);
            }

            ElementKind::Table { table_index } => {
                if let Some(table) = doc.tables.get(table_index as usize) {
                    if doc.table_anchors
                        && let Some(table_id) = table.table_id.as_deref()
                    {
                        let anchor = mk(arena, NodeValue::Paragraph);
                        anchor.append(mk_text(arena, &format!("[TABLE:{table_id}]")));
                        parent.append(anchor);
                    }
                    if !table.cells.is_empty() {
                        tracing::trace!(table_index, rows = table.cells.len(), "rendering table");
                        let has_header = doc.source_format != "csv" || table.columns.is_some();
                        let table_node = build_table(arena, &table.cells, has_header);
                        parent.append(table_node);
                    } else if !table.markdown.trim().is_empty() {
                        let para = mk(arena, NodeValue::Paragraph);
                        para.append(mk_text(arena, &table.markdown));
                        parent.append(para);
                    }
                }
            }

            ElementKind::Image { image_index } => {
                let image = doc.images.get(image_index as usize);
                let desc = image.and_then(|img| img.description.as_deref()).unwrap_or("");
                let url = match image {
                    None => {
                        if desc.is_empty() {
                            continue;
                        }
                        String::new()
                    }
                    Some(img) => {
                        if !img.data.is_empty() {
                            format!("image_{}.{}", image_index, img.format)
                        } else if let Some(ref path) = img.source_path {
                            path.clone()
                        } else {
                            format!("image_{}.bin", image_index)
                        }
                    }
                };

                let has_ocr = render_image_ocr
                    && image
                        .and_then(|img| img.ocr_result.as_ref())
                        .is_some_and(|result| !result.content.is_empty());

                if doc.ocr_text_only && has_ocr {
                    let ocr_result = image.and_then(|img| img.ocr_result.as_ref()).unwrap();
                    let ocr_para = mk(arena, NodeValue::Paragraph);
                    ocr_para.append(mk_text(arena, &ocr_result.content));
                    parent.append(ocr_para);
                } else {
                    let para = mk(arena, NodeValue::Paragraph);
                    let img_node = mk(
                        arena,
                        NodeValue::Image(Box::new(NodeLink {
                            url,
                            title: String::new(),
                        })),
                    );
                    img_node.append(mk_text(arena, desc));
                    para.append(img_node);
                    parent.append(para);

                    if render_image_ocr
                        && doc.append_ocr_text
                        && let Some(ocr_result) = image.and_then(|img| img.ocr_result.as_ref())
                        && !ocr_result.content.is_empty()
                    {
                        let ocr_para = mk(arena, NodeValue::Paragraph);
                        ocr_para.append(mk_text(arena, &ocr_result.content));
                        parent.append(ocr_para);
                    }
                }
            }

            ElementKind::FootnoteRef => {
                if let Some(n) = orig_idx.and_then(|idx| footnotes.ref_number(idx as u32)) {
                    let label = n.to_string();
                    let fnref = mk(
                        arena,
                        NodeValue::FootnoteReference(Box::new(NodeFootnoteReference {
                            name: label.clone(),
                            texts: Vec::new(),
                            ref_num: n,
                            ix: n,
                        })),
                    );
                    let inline_parent = if let Some(last) = parent.last_child() {
                        if matches!(last.data.borrow().value, NodeValue::Paragraph) {
                            last
                        } else {
                            let p = mk(arena, NodeValue::Paragraph);
                            parent.append(p);
                            p
                        }
                    } else {
                        let p = mk(arena, NodeValue::Paragraph);
                        parent.append(p);
                        p
                    };
                    inline_parent.append(fnref);
                }
            }

            ElementKind::FootnoteDefinition => {}

            ElementKind::Citation => {}

            ElementKind::PageBreak => {}

            ElementKind::Slide { .. } => {
                parent.append(mk(arena, NodeValue::ThematicBreak));
                if !elem_text.is_empty() {
                    let heading = mk(
                        arena,
                        NodeValue::Heading(NodeHeading {
                            level: 2,
                            setext: false,
                            closed: false,
                        }),
                    );
                    build_inlines(arena, heading, elem_text, elem_annotations);
                    parent.append(heading);
                }
            }

            ElementKind::DefinitionTerm => {
                let dt = mk(arena, NodeValue::Paragraph);
                build_inlines(arena, dt, elem_text, elem_annotations);
                parent.append(dt);
            }

            ElementKind::DefinitionDescription => {
                let dd = mk(arena, NodeValue::Paragraph);
                let prefix = format!(": {}", elem_text);
                build_inlines(arena, dd, &prefix, &[]);
                parent.append(dd);
            }

            ElementKind::Admonition => {
                let kind_str = elem_attributes
                    .and_then(|attrs| attrs.get("kind").map(|s| s.as_str()))
                    .unwrap_or("note");
                let title = elem_attributes.and_then(|attrs| attrs.get("title").map(|s| s.as_str()));

                let alert_type = match kind_str.to_lowercase().as_str() {
                    "note" => Some(comrak::nodes::AlertType::Note),
                    "tip" | "hint" => Some(comrak::nodes::AlertType::Tip),
                    "important" => Some(comrak::nodes::AlertType::Important),
                    "warning" | "warn" => Some(comrak::nodes::AlertType::Warning),
                    "caution" | "danger" | "error" => Some(comrak::nodes::AlertType::Caution),
                    _ => None,
                };

                if let Some(at) = alert_type {
                    let alert = mk(
                        arena,
                        NodeValue::Alert(Box::new(NodeAlert {
                            alert_type: at,
                            title: title.map(|s| s.to_string()),
                            multiline: false,
                            fence_length: 0,
                            fence_offset: 0,
                        })),
                    );
                    if !elem_text.is_empty() {
                        let para = mk(arena, NodeValue::Paragraph);
                        build_inlines(arena, para, elem_text, elem_annotations);
                        alert.append(para);
                    }
                    parent.append(alert);
                } else {
                    let bq = mk(arena, NodeValue::BlockQuote);
                    let title_display = title.unwrap_or(kind_str);
                    let title_para = mk(arena, NodeValue::Paragraph);
                    let strong = mk(arena, NodeValue::Strong);
                    strong.append(mk_text(arena, title_display));
                    title_para.append(strong);
                    bq.append(title_para);

                    if !elem_text.is_empty() {
                        let body_para = mk(arena, NodeValue::Paragraph);
                        build_inlines(arena, body_para, elem_text, elem_annotations);
                        bq.append(body_para);
                    }
                    parent.append(bq);
                }
            }

            ElementKind::RawBlock => {
                let raw = mk(arena, NodeValue::Raw(elem_text.to_string()));
                parent.append(raw);
            }

            ElementKind::MetadataBlock => {
                let entries = parse_metadata_entries(elem_text);
                if !entries.is_empty() {
                    for (key, value) in &entries {
                        let para = mk(arena, NodeValue::Paragraph);
                        let strong = mk(arena, NodeValue::Strong);
                        strong.append(mk_text(arena, key));
                        para.append(strong);
                        para.append(mk_text(arena, &format!(": {}", value)));
                        parent.append(para);
                    }
                } else if !elem_text.is_empty() {
                    let para = mk(arena, NodeValue::Paragraph);
                    para.append(mk_text(arena, elem_text));
                    parent.append(para);
                }
            }

            ElementKind::OcrText { .. } => {
                if !elem_text.is_empty() {
                    let para = mk(arena, NodeValue::Paragraph);
                    build_inlines(arena, para, elem_text, elem_annotations);
                    parent.append(para);
                }
            }

            ElementKind::ListStart { ordered } => {
                state.push_container(NestingKind::List { ordered, item_count: 0 }, elem_depth);

                let list_meta = NodeList {
                    list_type: if ordered {
                        comrak::nodes::ListType::Ordered
                    } else {
                        comrak::nodes::ListType::Bullet
                    },
                    bullet_char: b'-',
                    start: 1,
                    tight: true,
                    ..Default::default()
                };
                let list_node = mk(arena, NodeValue::List(list_meta));

                let target = if matches!(parent.data.borrow().value, NodeValue::List(..)) {
                    let last_item = parent
                        .children()
                        .filter(|c| matches!(c.data.borrow().value, NodeValue::Item(..) | NodeValue::TaskItem(..)))
                        .last();
                    match last_item {
                        Some(item) => item,
                        None => {
                            let item = mk(arena, NodeValue::Item(list_meta));
                            parent.append(item);
                            item
                        }
                    }
                } else {
                    parent
                };
                target.append(list_node);

                container_stack.push(ContainerEntry {
                    node: list_node,
                    kind: ContainerKind::List,
                });
            }

            ElementKind::ListEnd => {}

            ElementKind::QuoteStart => {
                state.push_container(NestingKind::BlockQuote, elem_depth);
                let bq = mk(arena, NodeValue::BlockQuote);
                parent.append(bq);
                container_stack.push(ContainerEntry {
                    node: bq,
                    kind: ContainerKind::BlockQuote,
                });
            }

            ElementKind::QuoteEnd => {}

            ElementKind::GroupStart => {
                state.push_container(NestingKind::Group, elem_depth);
                container_stack.push(ContainerEntry {
                    node: parent,
                    kind: ContainerKind::Group,
                });
            }

            ElementKind::GroupEnd => {}
        }
    }

    let defs = footnotes.definitions();
    for entry in defs {
        let label = entry.number.to_string();
        let fndef = mk(
            arena,
            NodeValue::FootnoteDefinition(NodeFootnoteDefinition {
                name: label,
                total_references: 1,
            }),
        );
        let para = mk(arena, NodeValue::Paragraph);
        para.append(mk_text(arena, &entry.text));
        fndef.append(para);
        root.append(fndef);
    }

    for elem in &doc.elements {
        if elem.kind == ElementKind::Citation {
            let key = elem.anchor.as_deref().unwrap_or("?");
            let fndef = mk(
                arena,
                NodeValue::FootnoteDefinition(NodeFootnoteDefinition {
                    name: key.to_string(),
                    total_references: 1,
                }),
            );
            let para = mk(arena, NodeValue::Paragraph);
            para.append(mk_text(arena, &elem.text));
            fndef.append(para);
            root.append(fndef);
        }
    }

    #[cfg(debug_assertions)]
    if let Err(e) = root.validate() {
        tracing::warn!(?e, "comrak AST validation failed — output may be malformed");
    }

    root
}

/// Pop the innermost container matching the given kind from the stack.
fn pop_container(stack: &mut Vec<ContainerEntry<'_>>, target: ContainerKind) {
    for i in (0..stack.len()).rev() {
        if matches!(
            (&stack[i].kind, &target),
            (ContainerKind::List, ContainerKind::List)
                | (ContainerKind::BlockQuote, ContainerKind::BlockQuote)
                | (ContainerKind::Group, ContainerKind::Group)
        ) {
            stack.remove(i);
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::document_structure::{AnnotationKind, ContentLayer, TextAnnotation};
    use crate::types::internal_builder::InternalDocumentBuilder;
    use comrak::{Options, format_commonmark};

    /// Helper: build AST from doc and render to CommonMark string.
    fn render(doc: &InternalDocument) -> String {
        let arena = comrak::Arena::new();
        let root = build_comrak_ast(doc, &arena);
        let mut output = String::new();
        format_commonmark(root, &Options::default(), &mut output).unwrap();
        output
    }

    #[test]
    fn test_title() {
        let mut b = InternalDocumentBuilder::new("test");
        b.push_title("My Document", None, None);
        let doc = b.build();
        let out = render(&doc);
        assert!(out.contains("# My Document"), "got: {}", out);
    }

    #[test]
    fn test_heading_levels() {
        for level in 1u8..=6 {
            let mut b = InternalDocumentBuilder::new("test");
            b.push_heading(level, "Heading", None, None);
            let doc = b.build();
            let out = render(&doc);
            let hashes = "#".repeat(level as usize);
            assert!(
                out.contains(&format!("{} Heading", hashes)),
                "level {}: got {}",
                level,
                out
            );
        }
    }

    #[test]
    fn test_paragraph() {
        let mut b = InternalDocumentBuilder::new("test");
        b.push_paragraph("Hello world.", vec![], None, None);
        let doc = b.build();
        let out = render(&doc);
        assert!(out.contains("Hello world."), "got: {}", out);
    }

    #[test]
    fn test_empty_paragraph_skipped() {
        let mut b = InternalDocumentBuilder::new("test");
        b.push_paragraph("", vec![], None, None);
        let doc = b.build();
        let out = render(&doc);
        assert!(out.trim().is_empty(), "expected empty, got: {}", out);
    }

    #[test]
    fn test_bold_annotation() {
        let mut b = InternalDocumentBuilder::new("test");
        let ann = vec![TextAnnotation {
            start: 0,
            end: 5,
            kind: AnnotationKind::Bold,
        }];
        b.push_paragraph("Hello world", ann, None, None);
        let doc = b.build();
        let out = render(&doc);
        assert!(out.contains("**Hello**"), "got: {}", out);
        assert!(out.contains("world"), "got: {}", out);
    }

    #[test]
    fn test_italic_annotation() {
        let mut b = InternalDocumentBuilder::new("test");
        let ann = vec![TextAnnotation {
            start: 0,
            end: 5,
            kind: AnnotationKind::Italic,
        }];
        b.push_paragraph("Hello world", ann, None, None);
        let doc = b.build();
        let out = render(&doc);
        assert!(out.contains("*Hello*"), "got: {}", out);
    }

    #[test]
    fn test_code_block() {
        let mut b = InternalDocumentBuilder::new("test");
        b.push_code("fn main() {}", Some("rust"), None, None);
        let doc = b.build();
        let out = render(&doc);
        assert!(out.contains("```rust"), "got: {}", out);
        assert!(out.contains("fn main() {}"), "got: {}", out);
    }

    #[test]
    fn test_table() {
        let mut b = InternalDocumentBuilder::new("test");
        let cells = vec![
            vec!["Name".to_string(), "Age".to_string()],
            vec!["Alice".to_string(), "30".to_string()],
        ];
        b.push_table_from_cells(&cells, None, None);
        let doc = b.build();
        let out = render(&doc);
        assert!(out.contains("Name"), "got: {}", out);
        assert!(out.contains("Alice"), "got: {}", out);
    }

    #[test]
    fn test_list_items() {
        let mut b = InternalDocumentBuilder::new("test");
        b.push_list(false);
        b.push_list_item("Alpha", false, vec![], None, None);
        b.push_list_item("Beta", false, vec![], None, None);
        b.end_list();
        let doc = b.build();
        let out = render(&doc);
        assert!(out.contains("Alpha"), "got: {}", out);
        assert!(out.contains("Beta"), "got: {}", out);
    }

    #[test]
    fn test_blockquote() {
        let mut b = InternalDocumentBuilder::new("test");
        b.push_quote_start();
        b.push_paragraph("Quoted text.", vec![], None, None);
        b.push_quote_end();
        let doc = b.build();
        let out = render(&doc);
        assert!(out.contains("> Quoted text."), "got: {}", out);
    }

    #[test]
    fn test_paragraph_consolidation() {
        let mut b = InternalDocumentBuilder::new("test");
        let ann1 = vec![TextAnnotation {
            start: 0,
            end: 5,
            kind: AnnotationKind::Italic,
        }];
        let ann2 = vec![TextAnnotation {
            start: 0,
            end: 5,
            kind: AnnotationKind::Italic,
        }];
        b.push_paragraph("Hello", ann1, None, None);
        b.push_paragraph("World", ann2, None, None);
        let doc = b.build();
        let out = render(&doc);
        assert!(out.contains("*Hello World*"), "got: {}", out);
    }

    #[test]
    fn test_annotation_on_multibyte_char_boundary() {
        let mut b = InternalDocumentBuilder::new("test");
        let ann = vec![TextAnnotation {
            start: 1,
            end: 11,
            kind: AnnotationKind::Bold,
        }];
        b.push_paragraph("«ярко»", ann, None, None);
        let doc = b.build();
        let out = render(&doc);
        assert!(out.contains("ярко"), "Cyrillic content should be present, got: {}", out);
    }

    #[test]
    fn test_annotation_on_valid_multibyte_boundaries() {
        let mut b = InternalDocumentBuilder::new("test");
        let ann = vec![TextAnnotation {
            start: 0,
            end: 12,
            kind: AnnotationKind::Bold,
        }];
        b.push_paragraph("Привет мир", ann, None, None);
        let doc = b.build();
        let out = render(&doc);
        assert!(out.contains("**Привет**"), "got: {}", out);
        assert!(out.contains("мир"), "got: {}", out);
    }

    #[test]
    fn test_footnote() {
        let mut b = InternalDocumentBuilder::new("test");
        b.push_paragraph("See note", vec![], None, None);
        let _ref_idx = b.push_footnote_ref("1", "fn1", None);
        let def_idx = b.push_footnote_definition("This is the footnote text.", "fn1", None);
        b.set_layer(def_idx, ContentLayer::Footnote);
        let doc = b.build();
        let out = render(&doc);
        assert!(out.contains("footnote"), "should contain footnote marker, got: {}", out);
    }

    /// Regression test for issue #762: image links must appear in markdown when image
    /// data is available via pdf_oxide extraction (non-empty `data` field).
    #[test]
    fn test_image_with_data_renders_link() {
        use crate::types::ExtractedImage;
        use crate::types::internal::ElementKind;

        let mut b = InternalDocumentBuilder::new("test");
        b.push_paragraph("Before image.", vec![], None, None);
        b.push_element(crate::types::internal::InternalElement::text(
            ElementKind::Image { image_index: 0 },
            "",
            0,
        ));
        let mut doc = b.build();
        doc.images.push(ExtractedImage {
            data: bytes::Bytes::from_static(b"\x89PNG"),
            format: std::borrow::Cow::Borrowed("png"),
            image_index: 0,
            page_number: Some(1),
            width: Some(100),
            height: Some(100),
            colorspace: None,
            bits_per_component: None,
            is_mask: false,
            description: None,
            ocr_result: None,
            bounding_box: None,
            source_path: None,
            image_kind: None,
            kind_confidence: None,
            cluster_id: None,
            caption: None,
            qr_codes: None,
            data_base64: None,
        });
        let out = render(&doc);
        assert!(out.contains("image_0.png"), "image link must appear; got: {}", out);
        assert!(out.contains("!["), "must use image markdown syntax; got: {}", out);
    }

    /// Regression test for issue #762: image with no data but known to exist (placeholder)
    /// must still emit a link with a `.bin` fallback URL rather than being silently dropped.
    #[test]
    fn test_image_placeholder_with_empty_data_renders_fallback_link() {
        use crate::types::ExtractedImage;
        use crate::types::internal::ElementKind;

        let mut b = InternalDocumentBuilder::new("test");
        b.push_element(crate::types::internal::InternalElement::text(
            ElementKind::Image { image_index: 0 },
            "",
            0,
        ));
        let mut doc = b.build();
        doc.images.push(ExtractedImage {
            data: bytes::Bytes::new(),
            format: std::borrow::Cow::Borrowed("unknown"),
            image_index: 0,
            page_number: Some(1),
            width: None,
            height: None,
            colorspace: None,
            bits_per_component: None,
            is_mask: false,
            description: None,
            ocr_result: None,
            bounding_box: None,
            source_path: None,
            image_kind: None,
            kind_confidence: None,
            cluster_id: None,
            caption: None,
            qr_codes: None,
            data_base64: None,
        });
        let out = render(&doc);
        assert!(
            out.contains("image_0.bin"),
            "empty-data image must emit placeholder link; got: {:?}",
            out
        );
    }

    /// Image element with out-of-bounds index and no description must be silently dropped.
    #[test]
    fn test_image_out_of_bounds_index_dropped() {
        use crate::types::internal::ElementKind;

        let mut b = InternalDocumentBuilder::new("test");
        b.push_paragraph("Only text.", vec![], None, None);
        b.push_element(crate::types::internal::InternalElement::text(
            ElementKind::Image { image_index: 99 },
            "",
            0,
        ));
        let doc = b.build();
        let out = render(&doc);
        assert!(
            !out.contains("image_99"),
            "out-of-bounds image must be dropped; got: {}",
            out
        );
        assert!(out.contains("Only text."), "paragraph must still render; got: {}", out);
    }

    #[test]
    fn test_heading_directly_under_list_does_not_panic() {
        let mut b = InternalDocumentBuilder::new("test");
        b.push_list(false);
        b.push_heading(3, "1 (7)", None, None);
        b.end_list();
        let doc = b.build();
        let out = render(&doc);
        assert!(out.contains("1 (7)"), "heading text must appear in output; got: {out}");
    }

    #[test]
    fn test_paragraph_directly_under_list_does_not_panic() {
        let mut b = InternalDocumentBuilder::new("test");
        b.push_list(false);
        b.push_paragraph("Orphan paragraph.", vec![], None, None);
        b.end_list();
        let doc = b.build();
        let out = render(&doc);
        assert!(
            out.contains("Orphan paragraph."),
            "paragraph text must appear in output; got: {out}"
        );
    }

    #[test]
    fn test_heading_before_first_list_item_then_real_item() {
        let mut b = InternalDocumentBuilder::new("test");
        b.push_list(false);
        b.push_heading(3, "Stray label", None, None);
        b.push_list_item("Real item", false, vec![], None, None);
        b.end_list();
        let doc = b.build();
        let out = render(&doc);
        assert!(out.contains("Stray label"), "stray heading must appear; got: {out}");
        assert!(out.contains("Real item"), "real list item must appear; got: {out}");
    }

    #[test]
    fn test_ordered_list_with_stray_heading_does_not_panic() {
        let mut b = InternalDocumentBuilder::new("test");
        b.push_list(true);
        b.push_heading(3, "Stray ordered label", None, None);
        b.push_list_item("First ordered item", true, vec![], None, None);
        b.end_list();
        let doc = b.build();
        let out = render(&doc);
        assert!(
            out.contains("Stray ordered label"),
            "stray heading must appear; got: {out}"
        );
        assert!(
            out.contains("First ordered item"),
            "ordered item must appear; got: {out}"
        );
    }

    #[test]
    fn test_link_annotation() {
        let mut b = InternalDocumentBuilder::new("test");
        b.push_paragraph(
            "click here",
            vec![TextAnnotation {
                start: 0,
                end: 10,
                kind: AnnotationKind::Link {
                    url: "https://example.com".to_string(),
                    title: None,
                },
            }],
            None,
            None,
        );
        let doc = b.build();
        let out = render(&doc);
        assert!(
            out.contains("[click here](https://example.com)"),
            "plain link must render; got: {out}"
        );
    }

    /// Regression for #1086: bold+hyperlink on the same byte range must produce
    /// `[**text**](url)`, not `**text**` with the link silently dropped.
    #[test]
    fn test_bold_link_same_range() {
        let mut b = InternalDocumentBuilder::new("test");
        b.push_paragraph(
            "click",
            vec![
                TextAnnotation {
                    start: 0,
                    end: 5,
                    kind: AnnotationKind::Bold,
                },
                TextAnnotation {
                    start: 0,
                    end: 5,
                    kind: AnnotationKind::Link {
                        url: "https://example.com".to_string(),
                        title: None,
                    },
                },
            ],
            None,
            None,
        );
        let doc = b.build();
        let out = render(&doc);
        assert!(
            out.contains("[**click**](https://example.com)"),
            "bold link must render as [**text**](url); got: {out}"
        );
    }

    /// Italic + hyperlink same range → `[*text*](url)`.
    #[test]
    fn test_italic_link_same_range() {
        let mut b = InternalDocumentBuilder::new("test");
        b.push_paragraph(
            "click",
            vec![
                TextAnnotation {
                    start: 0,
                    end: 5,
                    kind: AnnotationKind::Italic,
                },
                TextAnnotation {
                    start: 0,
                    end: 5,
                    kind: AnnotationKind::Link {
                        url: "https://example.com".to_string(),
                        title: None,
                    },
                },
            ],
            None,
            None,
        );
        let doc = b.build();
        let out = render(&doc);
        assert!(
            out.contains("[*click*](https://example.com)"),
            "italic link must render as [*text*](url); got: {out}"
        );
    }

    /// Bold + italic + hyperlink same range → `[***text***](url)`.
    #[test]
    fn test_bold_italic_link_same_range() {
        let mut b = InternalDocumentBuilder::new("test");
        b.push_paragraph(
            "click",
            vec![
                TextAnnotation {
                    start: 0,
                    end: 5,
                    kind: AnnotationKind::Bold,
                },
                TextAnnotation {
                    start: 0,
                    end: 5,
                    kind: AnnotationKind::Italic,
                },
                TextAnnotation {
                    start: 0,
                    end: 5,
                    kind: AnnotationKind::Link {
                        url: "https://example.com".to_string(),
                        title: None,
                    },
                },
            ],
            None,
            None,
        );
        let doc = b.build();
        let out = render(&doc);
        assert!(
            out.contains("[***click***](https://example.com)"),
            "bold italic link must render as [***text***](url); got: {out}"
        );
    }

    /// Formatting annotation contained within (but shorter than) the link span.
    /// "Visit **docs** here" where the link covers the whole phrase.
    #[test]
    fn test_bold_contained_within_link_span() {
        let mut b = InternalDocumentBuilder::new("test");
        b.push_paragraph(
            "Visit docs here",
            vec![
                TextAnnotation {
                    start: 6,
                    end: 10,
                    kind: AnnotationKind::Bold,
                },
                TextAnnotation {
                    start: 0,
                    end: 15,
                    kind: AnnotationKind::Link {
                        url: "https://example.com/docs".to_string(),
                        title: None,
                    },
                },
            ],
            None,
            None,
        );
        let doc = b.build();
        let out = render(&doc);
        assert!(
            out.contains("[Visit **docs** here](https://example.com/docs)"),
            "bold contained within link must nest; got: {out}"
        );
    }

    /// Partial overlap: Link[0,7] + Bold[5,10] on "Hello world".
    ///
    /// The Bold annotation starts inside the Link span but ends outside it.
    /// There is no unambiguous nesting semantics for this case, so the Bold is
    /// intentionally dropped. This test documents that behavior so a future
    /// refactor does not accidentally recurse infinitely trying to nest them.
    #[test]
    fn test_partial_overlap_annotation_is_dropped() {
        let mut b = InternalDocumentBuilder::new("test");
        b.push_paragraph(
            "Hello world",
            vec![
                TextAnnotation {
                    start: 0,
                    end: 7,
                    kind: AnnotationKind::Link {
                        url: "https://example.com".to_string(),
                        title: None,
                    },
                },
                TextAnnotation {
                    start: 6,
                    end: 11,
                    kind: AnnotationKind::Bold,
                },
            ],
            None,
            None,
        );
        let doc = b.build();
        let out = render(&doc);
        assert!(
            out.contains("[Hello w](https://example.com)"),
            "partial-overlap Bold must be dropped; got: {out}"
        );
        assert!(
            out.contains("orld"),
            "text after partial overlap must still render; got: {out}"
        );
        assert!(
            !out.contains("**"),
            "no bold markers expected for partial overlap; got: {out}"
        );
    }

    /// Inline code contained within a link span: `[\`code\`](url)`.
    ///
    /// Code spans are CommonMark literals — formatting cannot nest inside them —
    /// so inner_for_node is ignored for `AnnotationKind::Code`. This test verifies
    /// that code-within-link still renders correctly despite that exception.
    #[test]
    fn test_code_within_link() {
        let mut b = InternalDocumentBuilder::new("test");
        b.push_paragraph(
            "code",
            vec![
                TextAnnotation {
                    start: 0,
                    end: 4,
                    kind: AnnotationKind::Link {
                        url: "https://example.com".to_string(),
                        title: None,
                    },
                },
                TextAnnotation {
                    start: 0,
                    end: 4,
                    kind: AnnotationKind::Code,
                },
            ],
            None,
            None,
        );
        let doc = b.build();
        let out = render(&doc);
        assert!(
            out.contains("[`code`](https://example.com)"),
            "code within link must render as [`code`](url); got: {out}"
        );
    }

    /// Strikethrough contained within a link span.
    ///
    /// `append_annotated_span` now recurses via `build_inlines` for Strikethrough.
    /// Verify this produces `[~~text~~](url)` rather than dropping the strikethrough.
    #[test]
    fn test_strikethrough_within_link() {
        let mut b = InternalDocumentBuilder::new("test");
        b.push_paragraph(
            "dead",
            vec![
                TextAnnotation {
                    start: 0,
                    end: 4,
                    kind: AnnotationKind::Link {
                        url: "https://example.com".to_string(),
                        title: None,
                    },
                },
                TextAnnotation {
                    start: 0,
                    end: 4,
                    kind: AnnotationKind::Strikethrough,
                },
            ],
            None,
            None,
        );
        let doc = b.build();
        let out = render(&doc);
        assert!(
            out.contains("[~~dead~~](https://example.com)"),
            "strikethrough within link must render as [~~text~~](url); got: {out}"
        );
    }

    #[test]
    fn test_non_overlapping_link_and_bold_unaffected() {
        let mut b = InternalDocumentBuilder::new("test");
        b.push_paragraph(
            "Hello world",
            vec![
                TextAnnotation {
                    start: 0,
                    end: 5,
                    kind: AnnotationKind::Bold,
                },
                TextAnnotation {
                    start: 6,
                    end: 11,
                    kind: AnnotationKind::Link {
                        url: "https://example.com".to_string(),
                        title: None,
                    },
                },
            ],
            None,
            None,
        );
        let doc = b.build();
        let out = render(&doc);
        assert!(out.contains("**Hello**"), "bold must still render; got: {out}");
        assert!(
            out.contains("[world](https://example.com)"),
            "link must still render; got: {out}"
        );
    }
}
