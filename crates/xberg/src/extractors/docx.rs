//! DOCX extractor for high-performance text extraction.
//!
//! Supports: Microsoft Word (.docx)

use crate::Result;
use crate::core::config::ExtractionConfig;
use crate::extraction::{cells_to_markdown, office_metadata};
use crate::extractors::security::SecurityBudget;
use crate::plugins::{InternalDocumentExtractor, Plugin};
use crate::types::ExtractedImage;
use crate::types::internal::InternalDocument;
use crate::types::internal_builder::InternalDocumentBuilder;
use crate::types::{
    DocxMetadata, FormatMetadata, Metadata, PageBoundary, PageContent, PageInfo, PageStructure, PageUnitType, Table,
};
use ahash::AHashMap;
use async_trait::async_trait;
use bytes::Bytes;
use std::borrow::Cow;
use std::io::Cursor;
use std::sync::Arc;
#[cfg_attr(alef, alef(skip))]
/// High-performance DOCX extractor.
///
/// This extractor provides:
/// - Fast text extraction via streaming XML parsing
/// - Comprehensive metadata extraction (core.xml, app.xml, custom.xml)
pub struct DocxExtractor;

impl DocxExtractor {
    /// Create a new DOCX extractor.
    pub(crate) fn new() -> Self {
        Self
    }
}

impl Default for DocxExtractor {
    fn default() -> Self {
        Self::new()
    }
}

/// Resolve a drawing's alt text: `wp:docPr/@descr`, falling back to `@name` (#81).
///
/// Word writes `@descr` only when the author fills in the description field, but always
/// writes `@name`. Without the fallback an image the author named but never described
/// reaches output carrying no alt text at all.
///
/// Shared by the placeholder element path and the `ExtractedImage` path so the two
/// cannot disagree about what a given image is called.
fn drawing_alt_text(drawing: &crate::extraction::docx::drawing::Drawing) -> Option<String> {
    let properties = drawing.doc_properties.as_ref()?;
    properties
        .description
        .clone()
        .filter(|description| !description.is_empty())
        .or_else(|| properties.name.clone().filter(|name| !name.is_empty()))
}

/// Build an `InternalDocument` from parsed DOCX data.
///
/// Creates a flat element list with headings, paragraphs, lists, tables, images,
/// footnotes/endnotes (with relationships), and hyperlinks (as InternalLink relationships).
///
/// When `inject_placeholders` is `false`, `Drawing` elements are **not** pushed into the
/// returned `InternalDocument`, so they will not appear in `ExtractedDocument::elements`.
/// Image data is still extracted separately by the caller.
fn build_internal_document(
    doc: &crate::extraction::docx::parser::Document,
    inject_placeholders: bool,
) -> InternalDocument {
    use crate::types::document_structure::ContentLayer;
    use crate::types::extraction::BoundingBox;
    use crate::types::internal::{ElementKind, InternalElement, RelationshipKind, RelationshipTarget};
    use crate::types::uri::ExtractedUri;

    let mut builder = InternalDocumentBuilder::new("docx");

    let mut current_list_numbering_id: Option<i64> = None;
    let mut current_list_ordered: bool = false;
    let mut current_list_nesting_level: i64 = 0;
    let mut open_list_count: i64 = 0;

    for element in &doc.elements {
        match element {
            crate::extraction::docx::parser::DocumentElement::Paragraph(idx) => {
                let paragraph = &doc.paragraphs[*idx];

                let (text, annotations, math_formulas) = collect_run_annotations(&paragraph.runs);

                if text.is_empty() && math_formulas.is_empty() {
                    if current_list_numbering_id.is_some() {
                        for _ in 0..open_list_count {
                            builder.end_list();
                        }
                        current_list_numbering_id = None;
                        open_list_count = 0;
                    }
                    continue;
                }

                let heading_level = paragraph.style.as_deref().and_then(|s| doc.resolve_heading_level(s));

                let is_quote_style = paragraph.style.as_deref().is_some_and(|s| {
                    let lower = s.to_ascii_lowercase();
                    lower == "quote"
                        || lower == "blockquote"
                        || lower == "intenseq"
                        || lower == "intensequote"
                        || lower.contains("quote")
                });

                let element_idx: Option<u32> = if let Some(level) = heading_level {
                    if current_list_numbering_id.is_some() {
                        for _ in 0..open_list_count {
                            builder.end_list();
                        }
                        current_list_numbering_id = None;
                        open_list_count = 0;
                    }
                    let heading_text = if text.is_empty() {
                        paragraph.runs_to_markdown()
                    } else {
                        text.clone()
                    };
                    let idx = builder.push_heading(level, &heading_text, None, None);
                    if !annotations.is_empty() {
                        builder.set_annotations(idx, annotations.clone());
                    }
                    Some(idx)
                } else if is_quote_style {
                    if current_list_numbering_id.is_some() {
                        for _ in 0..open_list_count {
                            builder.end_list();
                        }
                        current_list_numbering_id = None;
                        open_list_count = 0;
                    }
                    builder.push_quote_start();
                    let para_idx = builder.push_paragraph(&text, annotations.clone(), None, None);
                    builder.push_quote_end();
                    Some(para_idx)
                } else if let Some(nid) = paragraph.numbering_id {
                    for formula in &math_formulas {
                        builder.push_formula(formula, None, None);
                    }
                    if !text.is_empty() {
                        let nlvl = paragraph.numbering_level.unwrap_or(0);
                        let is_ordered = paragraph
                            .numbering_id
                            .zip(paragraph.numbering_level)
                            .and_then(|(nid, nlvl)| doc.numbering_defs.get(&(nid, nlvl)))
                            .is_some_and(|lt| *lt == crate::extraction::docx::parser::ListType::Numbered);
                        if current_list_numbering_id != Some(nid) {
                            if current_list_numbering_id.is_some() {
                                for _ in 0..open_list_count {
                                    builder.end_list();
                                }
                            }
                            builder.push_list(is_ordered);
                            current_list_numbering_id = Some(nid);
                            current_list_ordered = is_ordered;
                            current_list_nesting_level = nlvl;
                            open_list_count = 1;
                        } else if nlvl > current_list_nesting_level {
                            let depth_increase = nlvl - current_list_nesting_level;
                            for _ in 0..depth_increase {
                                builder.push_list(is_ordered);
                                open_list_count += 1;
                            }
                            current_list_nesting_level = nlvl;
                        } else if nlvl < current_list_nesting_level {
                            let depth_decrease = current_list_nesting_level - nlvl;
                            for _ in 0..depth_decrease {
                                builder.end_list();
                                open_list_count = open_list_count.saturating_sub(1);
                            }
                            current_list_nesting_level = nlvl;
                        }
                        let li_idx =
                            builder.push_list_item(&text, current_list_ordered, annotations.clone(), None, None);
                        Some(li_idx)
                    } else {
                        None
                    }
                } else {
                    if current_list_numbering_id.is_some() {
                        for _ in 0..open_list_count {
                            builder.end_list();
                        }
                        current_list_numbering_id = None;
                        open_list_count = 0;
                    }
                    for formula in &math_formulas {
                        builder.push_formula(formula, None, None);
                    }
                    if !text.is_empty() {
                        let para_idx = builder.push_paragraph(&text, annotations.clone(), None, None);
                        Some(para_idx)
                    } else {
                        None
                    }
                };

                if let Some(elem_idx) = element_idx {
                    for run in &paragraph.runs {
                        if run.math_latex.is_some() || run.text.is_empty() {
                            continue;
                        }
                        if let Some(ref url) = run.hyperlink_url {
                            if url.starts_with('#') {
                                let anchor_key = url.trim_start_matches('#').to_string();
                                builder.push_relationship(
                                    elem_idx,
                                    RelationshipTarget::Key(anchor_key),
                                    RelationshipKind::InternalLink,
                                );
                            }
                            builder.push_uri(ExtractedUri::hyperlink(url.as_str(), Some(run.text.clone())));
                        }
                    }

                    let mut search_start = 0;
                    while let Some(start) = text[search_start..].find("[^") {
                        let abs_start = search_start + start;
                        if let Some(end) = text[abs_start..].find(']') {
                            let ref_id = &text[abs_start + 2..abs_start + end];
                            if !ref_id.is_empty() && ref_id.chars().all(|c| c.is_ascii_digit()) {
                                let key = format!("fn{}", ref_id);
                                builder.push_footnote_ref(ref_id, &key, None);
                            }
                            search_start = abs_start + end + 1;
                        } else {
                            break;
                        }
                    }

                    // Comment reference markers (#82, #300). Structurally a comment is
                    // the same shape as a footnote (a marker in the body, a definition
                    // elsewhere), sourced from `word/comments.xml` instead of
                    // `word/footnotes.xml`, but it is routed through the dedicated
                    // `CommentRef`/`NodeContent::Comment` machinery so a consumer can
                    // tell a reviewer comment apart from an authored footnote.
                    let mut search_start = 0;
                    while let Some(start) = text[search_start..].find("[cmt:") {
                        let abs_start = search_start + start;
                        if let Some(end) = text[abs_start..].find(']') {
                            let comment_id = &text[abs_start + 5..abs_start + end];
                            if !comment_id.is_empty() {
                                let key = format!("cmt{}", comment_id);
                                builder.push_comment_ref(comment_id, &key, None);
                            }
                            search_start = abs_start + end + 1;
                        } else {
                            break;
                        }
                    }
                }
            }
            crate::extraction::docx::parser::DocumentElement::Table(idx) => {
                if current_list_numbering_id.is_some() {
                    builder.end_list();
                    current_list_numbering_id = None;
                }
                let table = &doc.tables[*idx];
                if let Some(ref props) = table.properties
                    && let Some(ref caption) = props.caption
                    && !caption.is_empty()
                {
                    builder.push_paragraph(caption, vec![], None, None);
                }
                let mut cells: Vec<Vec<String>> = Vec::new();
                for row in &table.rows {
                    let mut row_cells = Vec::new();
                    for cell in &row.cells {
                        let text = cell
                            .paragraphs
                            .iter()
                            .map(|p| p.runs_to_markdown())
                            .collect::<Vec<_>>()
                            .join(" ")
                            .trim()
                            .to_string();
                        let span = cell.properties.as_ref().and_then(|p| p.grid_span).unwrap_or(1);
                        for _ in 0..span {
                            row_cells.push(text.clone());
                        }
                    }
                    cells.push(row_cells);
                }
                for row_idx in 1..table.rows.len() {
                    let mut col = 0usize;
                    for cell in &table.rows[row_idx].cells {
                        let span = cell.properties.as_ref().and_then(|p| p.grid_span).unwrap_or(1) as usize;
                        let is_vmerge_continue = cell.properties.as_ref().is_some_and(|p| {
                            matches!(p.v_merge, Some(crate::extraction::docx::table::VerticalMerge::Continue))
                        });
                        if is_vmerge_continue {
                            for c in col..col + span {
                                if c < cells[row_idx].len() && c < cells[row_idx - 1].len() {
                                    cells[row_idx][c] = cells[row_idx - 1][c].clone();
                                }
                            }
                        }
                        col += span;
                    }
                }
                if !cells.is_empty() {
                    builder.push_table_from_cells(&cells, None, None);
                }
            }
            crate::extraction::docx::parser::DocumentElement::Drawing(idx) => {
                let drawing = &doc.drawings[*idx];

                if let Some(ref textbox_text) = drawing.text_box_content
                    && !textbox_text.trim().is_empty()
                {
                    if current_list_numbering_id.is_some() {
                        builder.end_list();
                        current_list_numbering_id = None;
                    }
                    builder.push_paragraph(textbox_text, vec![], None, None);
                }

                if drawing.image_ref.is_none() {
                    continue;
                }

                if !inject_placeholders {
                    continue;
                }

                if current_list_numbering_id.is_some() {
                    builder.end_list();
                    current_list_numbering_id = None;
                }
                let description = drawing_alt_text(drawing);

                let bbox = match &drawing.drawing_type {
                    crate::extraction::docx::drawing::DrawingType::Anchored(anchor) => {
                        let x = anchor.position_h.as_ref().and_then(|p| p.offset).unwrap_or(0);
                        let y = anchor.position_v.as_ref().and_then(|p| p.offset).unwrap_or(0);
                        let (cx, cy) = drawing.extent.as_ref().map(|e| (e.cx, e.cy)).unwrap_or((0, 0));
                        if x != 0 || y != 0 || cx != 0 || cy != 0 {
                            const EMU_PER_PT: f64 = 914_400.0 / 72.0;
                            Some(BoundingBox {
                                x0: x as f64 / EMU_PER_PT,
                                y0: y as f64 / EMU_PER_PT,
                                x1: (x + cx) as f64 / EMU_PER_PT,
                                y1: (y + cy) as f64 / EMU_PER_PT,
                            })
                        } else {
                            None
                        }
                    }
                    _ => None,
                };

                let kind = ElementKind::Image {
                    image_index: *idx as u32,
                };
                let text_val = description.as_deref().unwrap_or("");
                let elem = InternalElement::text(kind, text_val, 0);
                let elem = if let Some(b) = bbox { elem.with_bbox(b) } else { elem };
                let img_elem_idx = builder.push_element(elem);

                let mut attrs = AHashMap::new();
                if let Some(ref rid) = drawing.image_ref
                    && let Some(path) = doc.image_relationships.get(rid)
                {
                    attrs.insert("image_uri".to_string(), path.clone());
                }
                // Wire the drawing's physical size (#81) into output attributes so
                // consumers can lay out the image without re-deriving it from EMUs.
                if let Some(ref extent) = drawing.extent {
                    attrs.insert("width_inches".to_string(), format!("{:.2}", extent.width_inches()));
                    attrs.insert("height_inches".to_string(), format!("{:.2}", extent.height_inches()));
                }
                if !attrs.is_empty() {
                    builder.set_attributes(img_elem_idx, attrs);
                }
            }
            crate::extraction::docx::parser::DocumentElement::PageBreak => {}
        }
    }

    if current_list_numbering_id.is_some() {
        for _ in 0..open_list_count {
            builder.end_list();
        }
    }

    for hf in &doc.headers {
        push_header_footer_content(&mut builder, hf, ContentLayer::Header);
    }
    for hf in &doc.footers {
        push_header_footer_content(&mut builder, hf, ContentLayer::Footer);
    }

    for note in doc.footnotes.iter().chain(doc.endnotes.iter()) {
        let text: String = note
            .paragraphs
            .iter()
            .map(|p| p.runs_to_markdown())
            .collect::<Vec<_>>()
            .join(" ");
        if !text.is_empty() {
            let key = format!("fn{}", note.id);
            let idx = builder.push_footnote_definition(&text, &key, None);
            builder.set_layer(idx, ContentLayer::Footnote);
        }
    }

    // Comment definitions (#82, #300) — see the comment-reference scan above.
    for comment in &doc.comments {
        let text: String = comment
            .paragraphs
            .iter()
            .map(|p| p.runs_to_markdown())
            .collect::<Vec<_>>()
            .join(" ");
        if !text.is_empty() {
            let key = format!("cmt{}", comment.id);
            let idx = builder.push_comment_definition(&text, &key, None);
            builder.set_layer(idx, ContentLayer::Footnote);
        }
    }

    builder.build()
}

/// Push a header's or footer's paragraphs and tables (#85 — headers/footers now
/// parse tables via the shared body element loop, where they previously couldn't).
fn push_header_footer_content(
    builder: &mut InternalDocumentBuilder,
    hf: &crate::extraction::docx::parser::HeaderFooter,
    layer: crate::types::document_structure::ContentLayer,
) {
    let text: String = hf
        .paragraphs
        .iter()
        .map(|p| p.runs_to_markdown())
        .collect::<Vec<_>>()
        .join("\n");
    if !text.is_empty() {
        let idx = builder.push_paragraph(&text, vec![], None, None);
        builder.set_layer(idx, layer);
    }

    for table in &hf.tables {
        let cells: Vec<Vec<String>> = table
            .rows
            .iter()
            .map(|row| {
                row.cells
                    .iter()
                    .map(|cell| {
                        cell.paragraphs
                            .iter()
                            .map(|p| p.runs_to_markdown())
                            .collect::<Vec<_>>()
                            .join(" ")
                            .trim()
                            .to_string()
                    })
                    .collect()
            })
            .collect();
        if !cells.is_empty() {
            let idx = builder.push_table_from_cells(&cells, None, None);
            builder.set_layer(idx, layer);
        }
    }
}

/// Collect plain text, annotations, and math formulas from a slice of Runs.
///
/// Returns `(plain_text, annotations, math_formulas)` where:
/// - `plain_text` is the concatenated non-math run text
/// - `annotations` are byte-offset-based formatting annotations for the plain text
/// - `math_formulas` are LaTeX strings from math runs (to be emitted as Formula nodes)
fn collect_run_annotations(
    runs: &[crate::extraction::docx::parser::Run],
) -> (String, Vec<crate::types::TextAnnotation>, Vec<String>) {
    use crate::types::builder;

    let mut text = String::new();
    let mut annotations = Vec::new();
    let mut math_formulas = Vec::new();

    for run in runs {
        if let Some((ref latex, _is_display)) = run.math_latex {
            if !latex.is_empty() {
                math_formulas.push(latex.clone());
            }
            continue;
        }

        if run.text.is_empty() {
            continue;
        }

        let start = text.len() as u32;
        text.push_str(&run.text);
        let end = text.len() as u32;

        if run.bold {
            annotations.push(builder::bold(start, end));
        }
        if run.italic {
            annotations.push(builder::italic(start, end));
        }
        if run.underline {
            annotations.push(builder::underline(start, end));
        }
        if run.strikethrough {
            annotations.push(builder::strikethrough(start, end));
        }
        if run.subscript {
            annotations.push(builder::subscript(start, end));
        }
        if run.superscript {
            annotations.push(builder::superscript(start, end));
        }
        if let Some(sz) = run.font_size {
            let pts = sz as f64 / 2.0;
            let value = if pts.fract() == 0.0 {
                format!("{}pt", pts as u32)
            } else {
                format!("{:.1}pt", pts)
            };
            annotations.push(builder::font_size(start, end, &value));
        }
        if let Some(ref color_val) = run.font_color {
            annotations.push(builder::color(start, end, &format!("#{}", color_val)));
        }
        if run.highlight.is_some() {
            annotations.push(builder::highlight(start, end));
        }
        if let Some(ref url) = run.hyperlink_url {
            annotations.push(builder::link(start, end, url, None));
        }
    }

    merge_adjacent_annotations(&mut annotations);

    (text, annotations, math_formulas)
}

/// Merge adjacent or overlapping annotations of the same kind.
///
/// When consecutive DOCX runs have the same formatting (e.g. bold), each run produces
/// its own annotation. Without merging, the markdown renderer would close and immediately
/// reopen markers, producing `**text1****text2**` instead of `**text1text2**`.
fn merge_adjacent_annotations(annotations: &mut Vec<crate::types::TextAnnotation>) {
    use crate::types::document_structure::AnnotationKind;

    if annotations.len() < 2 {
        return;
    }

    /// Check if two annotation kinds are the same for merging purposes.
    /// Simple kinds match by discriminant; Link kinds match if they have the same URL.
    fn same_kind_for_merge(a: &AnnotationKind, b: &AnnotationKind) -> bool {
        match (a, b) {
            (AnnotationKind::Bold, AnnotationKind::Bold)
            | (AnnotationKind::Italic, AnnotationKind::Italic)
            | (AnnotationKind::Underline, AnnotationKind::Underline)
            | (AnnotationKind::Strikethrough, AnnotationKind::Strikethrough)
            | (AnnotationKind::Subscript, AnnotationKind::Subscript)
            | (AnnotationKind::Superscript, AnnotationKind::Superscript)
            | (AnnotationKind::Highlight, AnnotationKind::Highlight)
            | (AnnotationKind::Code, AnnotationKind::Code) => true,
            (
                AnnotationKind::Link {
                    url: url_a,
                    title: title_a,
                },
                AnnotationKind::Link {
                    url: url_b,
                    title: title_b,
                },
            ) => url_a == url_b && title_a == title_b,
            _ => false,
        }
    }

    fn is_mergeable(kind: &AnnotationKind) -> bool {
        matches!(
            kind,
            AnnotationKind::Bold
                | AnnotationKind::Italic
                | AnnotationKind::Underline
                | AnnotationKind::Strikethrough
                | AnnotationKind::Subscript
                | AnnotationKind::Superscript
                | AnnotationKind::Highlight
                | AnnotationKind::Code
                | AnnotationKind::Link { .. }
        )
    }

    let kind_key = |kind: &AnnotationKind| -> u8 {
        match kind {
            AnnotationKind::Bold => 0,
            AnnotationKind::Italic => 1,
            AnnotationKind::Underline => 2,
            AnnotationKind::Strikethrough => 3,
            AnnotationKind::Subscript => 4,
            AnnotationKind::Superscript => 5,
            AnnotationKind::Highlight => 6,
            AnnotationKind::Code => 7,
            AnnotationKind::Link { .. } => 8,
            _ => 255,
        }
    };

    annotations.sort_by(|a, b| kind_key(&a.kind).cmp(&kind_key(&b.kind)).then(a.start.cmp(&b.start)));

    let mut merged = Vec::with_capacity(annotations.len());
    let mut i = 0;
    while i < annotations.len() {
        let mut ann = annotations[i].clone();
        if is_mergeable(&ann.kind) {
            let mut j = i + 1;
            while j < annotations.len()
                && same_kind_for_merge(&annotations[j].kind, &ann.kind)
                && annotations[j].start <= ann.end
            {
                ann.end = ann.end.max(annotations[j].end);
                j += 1;
            }
            merged.push(ann);
            i = j;
        } else {
            merged.push(ann);
            i += 1;
        }
    }

    *annotations = merged;
}

type DocxParseResult = (
    String,
    Vec<Table>,
    Option<Vec<PageBoundary>>,
    Vec<crate::extraction::docx::drawing::Drawing>,
    AHashMap<String, String>,
    InternalDocument,
);

/// Parse DOCX document content and extract text, tables, page boundaries, drawings, image
/// relationships, and an `InternalDocument`.
///
/// `inject_placeholders` is threaded into both `extract_text_with_boundaries` (controls
/// whether `![…](image)` links appear in the markdown text) and `build_internal_document`
/// (controls whether `Image` elements are added to the returned `InternalDocument`).
fn parse_docx_core(
    content: &[u8],
    output_format: crate::core::config::OutputFormat,
    inject_placeholders: bool,
    mut budget: SecurityBudget,
) -> crate::error::Result<DocxParseResult> {
    let mut doc = crate::extraction::docx::parser::parse_document(content, &mut budget)?;
    let (text, page_boundaries) = doc.extract_text_with_boundaries(
        matches!(output_format, crate::core::config::OutputFormat::Markdown),
        inject_placeholders,
    );

    let table_page_nums = doc.table_page_numbers();
    let tables: Vec<Table> = doc
        .tables
        .iter()
        .enumerate()
        .map(|(idx, table)| {
            let page_number = table_page_nums.get(idx).copied().unwrap_or(1) as u32;
            convert_docx_table_to_table(table, page_number)
        })
        .collect();

    let page_boundaries = if page_boundaries.len() > 1 {
        Some(page_boundaries)
    } else {
        None
    };

    let mut internal_doc = build_internal_document(&doc, inject_placeholders);
    if !doc.revisions.is_empty() {
        internal_doc.revisions = Some(std::mem::take(&mut doc.revisions));
    }
    if !doc.warnings.is_empty() {
        internal_doc
            .processing_warnings
            .extend(std::mem::take(&mut doc.warnings));
    }
    let drawings = std::mem::take(&mut doc.drawings);
    let image_rels = std::mem::take(&mut doc.image_relationships);
    Ok((text, tables, page_boundaries, drawings, image_rels, internal_doc))
}

impl Plugin for DocxExtractor {
    fn name(&self) -> &str {
        "docx-extractor"
    }

    fn version(&self) -> String {
        env!("CARGO_PKG_VERSION").to_string()
    }

    fn initialize(&self) -> Result<()> {
        Ok(())
    }

    fn shutdown(&self) -> Result<()> {
        Ok(())
    }

    fn description(&self) -> &str {
        "High-performance DOCX text extraction with metadata support"
    }

    fn author(&self) -> &str {
        "Xberg Team"
    }
}

/// Convert parsed DOCX table to Xberg Table struct with markdown representation.
///
/// # Arguments
/// * `docx_table` - The parsed DOCX table
/// * `page_number` - 1-based page number the table appears on
///
/// # Returns
/// * `Table` - Converted table with cells and markdown representation
fn convert_docx_table_to_table(docx_table: &crate::extraction::docx::parser::Table, page_number: u32) -> Table {
    let mut cells: Vec<Vec<String>> = Vec::new();
    for row in &docx_table.rows {
        let mut row_cells = Vec::new();
        for cell in &row.cells {
            let cell_text = cell
                .paragraphs
                .iter()
                .map(|para| para.runs_to_markdown())
                .collect::<Vec<_>>()
                .join(" ")
                .trim()
                .to_string();
            let span = cell.properties.as_ref().and_then(|p| p.grid_span).unwrap_or(1);
            for _ in 0..span {
                row_cells.push(cell_text.clone());
            }
        }
        cells.push(row_cells);
    }
    for row_idx in 1..docx_table.rows.len() {
        let mut col = 0usize;
        for cell in &docx_table.rows[row_idx].cells {
            let span = cell.properties.as_ref().and_then(|p| p.grid_span).unwrap_or(1) as usize;
            let is_vmerge_continue = cell
                .properties
                .as_ref()
                .is_some_and(|p| matches!(p.v_merge, Some(crate::extraction::docx::table::VerticalMerge::Continue)));
            if is_vmerge_continue {
                for c in col..col + span {
                    if c < cells[row_idx].len() && c < cells[row_idx - 1].len() {
                        cells[row_idx][c] = cells[row_idx - 1][c].clone();
                    }
                }
            }
            col += span;
        }
    }

    let markdown = cells_to_markdown(&cells);

    Table {
        cells,
        markdown,
        page_number,
        bounding_box: None,
        ..Default::default()
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
impl InternalDocumentExtractor for DocxExtractor {
    async fn extract_content(
        &self,
        content: &[u8],
        mime_type: &str,
        config: &ExtractionConfig,
    ) -> Result<InternalDocument> {
        tracing::debug!("extract_docx: starting");

        let output_format = if config.images.as_ref().is_some_and(|i| i.extract_images) {
            crate::core::config::OutputFormat::Markdown
        } else {
            config.output_format.clone()
        };

        let inject_placeholders = config.images.as_ref().map(|i| i.inject_placeholders).unwrap_or(true);
        let budget = SecurityBudget::from_config(config);
        let content_owned: Arc<[u8]> = Arc::from(content);
        let (text, tables, page_boundaries, drawings, image_rels, mut internal_doc) = {
            #[cfg(feature = "tokio-runtime")]
            if crate::core::batch_mode::is_batch_mode() {
                if config.cancel_token.as_ref().map(|t| t.is_cancelled()).unwrap_or(false) {
                    return Err(crate::error::XbergError::Cancelled);
                }
                let parse_content = Arc::clone(&content_owned);
                let span = tracing::Span::current();
                tokio::task::spawn_blocking(move || {
                    let _guard = span.entered();
                    parse_docx_core(&parse_content, output_format, inject_placeholders, budget)
                })
                .await
                .map_err(|e| crate::error::XbergError::parsing(format!("DOCX extraction task failed: {}", e)))??
            } else {
                parse_docx_core(&content_owned, output_format, inject_placeholders, budget)?
            }

            #[cfg(not(feature = "tokio-runtime"))]
            parse_docx_core(&content_owned, output_format, inject_placeholders, budget)?
        };

        let mut archive = {
            #[cfg(feature = "tokio-runtime")]
            if crate::core::batch_mode::is_batch_mode() {
                let archive_content = Arc::clone(&content_owned);
                let span = tracing::Span::current();
                tokio::task::spawn_blocking(move || -> crate::error::Result<_> {
                    let _guard = span.entered();
                    let cursor = Cursor::new(archive_content);
                    zip::ZipArchive::new(cursor)
                        .map_err(|e| crate::error::XbergError::parsing(format!("Failed to open ZIP archive: {}", e)))
                })
                .await
                .map_err(|e| crate::error::XbergError::parsing(format!("Task join error: {}", e)))??
            } else {
                let cursor = Cursor::new(Arc::clone(&content_owned));
                zip::ZipArchive::new(cursor)
                    .map_err(|e| crate::error::XbergError::parsing(format!("Failed to open ZIP archive: {}", e)))?
            }

            #[cfg(not(feature = "tokio-runtime"))]
            {
                let cursor = Cursor::new(Arc::clone(&content_owned));
                zip::ZipArchive::new(cursor)
                    .map_err(|e| crate::error::XbergError::parsing(format!("Failed to open ZIP archive: {}", e)))?
            }
        };

        let mut metadata_map = AHashMap::new();
        let mut parsed_keywords: Option<Vec<String>> = None;
        let mut docx_core_properties = None;
        let mut docx_app_properties = None;
        let mut docx_custom_properties: Option<std::collections::HashMap<String, serde_json::Value>> = None;

        if let Ok(core) = office_metadata::extract_core_properties(&mut archive) {
            if let Some(ref title) = core.title {
                metadata_map.insert(Cow::Borrowed("title"), serde_json::Value::String(title.clone()));
            }
            if let Some(ref creator) = core.creator {
                metadata_map.insert(
                    Cow::Borrowed("authors"),
                    serde_json::Value::Array(vec![serde_json::Value::String(creator.clone())]),
                );
                metadata_map.insert(Cow::Borrowed("created_by"), serde_json::Value::String(creator.clone()));
            }
            if let Some(ref subject) = core.subject {
                metadata_map.insert(Cow::Borrowed("subject"), serde_json::Value::String(subject.clone()));
            }
            if let Some(ref keywords) = core.keywords {
                parsed_keywords = Some(
                    keywords
                        .split(',')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect(),
                );
            }
            if let Some(ref description) = core.description {
                metadata_map.insert(
                    Cow::Borrowed("description"),
                    serde_json::Value::String(description.clone()),
                );
            }
            if let Some(ref modified_by) = core.last_modified_by {
                metadata_map.insert(
                    Cow::Borrowed("modified_by"),
                    serde_json::Value::String(modified_by.clone()),
                );
            }
            if let Some(ref created) = core.created {
                metadata_map.insert(Cow::Borrowed("created_at"), serde_json::Value::String(created.clone()));
            }
            if let Some(ref modified) = core.modified {
                metadata_map.insert(
                    Cow::Borrowed("modified_at"),
                    serde_json::Value::String(modified.clone()),
                );
            }
            if let Some(ref revision) = core.revision {
                metadata_map.insert(Cow::Borrowed("revision"), serde_json::Value::String(revision.clone()));
            }
            if let Some(ref category) = core.category {
                metadata_map.insert(Cow::Borrowed("category"), serde_json::Value::String(category.clone()));
            }
            if let Some(ref content_status) = core.content_status {
                metadata_map.insert(
                    Cow::Borrowed("content_status"),
                    serde_json::Value::String(content_status.clone()),
                );
            }
            if let Some(ref language) = core.language {
                metadata_map.insert(Cow::Borrowed("language"), serde_json::Value::String(language.clone()));
            }
            docx_core_properties = Some(core);
        }

        if let Ok(app) = office_metadata::extract_docx_app_properties(&mut archive) {
            if let Some(pages) = app.pages {
                metadata_map.insert(Cow::Borrowed("page_count"), serde_json::Value::Number(pages.into()));
            }
            if let Some(words) = app.words {
                metadata_map.insert(Cow::Borrowed("word_count"), serde_json::Value::Number(words.into()));
            }
            if let Some(chars) = app.characters {
                metadata_map.insert(
                    Cow::Borrowed("character_count"),
                    serde_json::Value::Number(chars.into()),
                );
            }
            if let Some(lines) = app.lines {
                metadata_map.insert(Cow::Borrowed("line_count"), serde_json::Value::Number(lines.into()));
            }
            if let Some(paragraphs) = app.paragraphs {
                metadata_map.insert(
                    Cow::Borrowed("paragraph_count"),
                    serde_json::Value::Number(paragraphs.into()),
                );
            }
            if let Some(ref template) = app.template {
                metadata_map.insert(Cow::Borrowed("template"), serde_json::Value::String(template.clone()));
            }
            if let Some(ref company) = app.company {
                metadata_map.insert(Cow::Borrowed("company"), serde_json::Value::String(company.clone()));
            }
            if let Some(time) = app.total_time {
                metadata_map.insert(
                    Cow::Borrowed("total_editing_time_minutes"),
                    serde_json::Value::Number(time.into()),
                );
            }
            if let Some(ref application) = app.application {
                metadata_map.insert(
                    Cow::Borrowed("application"),
                    serde_json::Value::String(application.clone()),
                );
            }
            // #230: DocSecurity was parsed into `app.doc_security` and then only ever
            // reachable as an opaque integer buried in the format-specific metadata.
            // Surface both the raw value and the decoded ECMA-376 flags so a consumer
            // can tell a read-only-recommended or password-protected document apart
            // without knowing the bit layout.
            if let Some(raw) = app.doc_security {
                metadata_map.insert(
                    Cow::Borrowed(office_metadata::app_properties::DOC_SECURITY_KEY),
                    serde_json::Value::Number(raw.into()),
                );
                for (key, value) in office_metadata::app_properties::decode_doc_security_flags(raw) {
                    metadata_map.insert(Cow::Borrowed(key), serde_json::Value::Bool(value));
                }
            }
            docx_app_properties = Some(app);
        }

        if let Ok(custom) = office_metadata::extract_custom_properties(&mut archive) {
            for (key, value) in &custom {
                metadata_map.insert(Cow::Owned(format!("custom_{}", key)), value.clone());
            }
            docx_custom_properties = Some(custom);
        }

        let page_structure = if let Some(boundaries) = page_boundaries {
            let total_count = boundaries.len();
            Some(PageStructure {
                total_count: total_count as u32,
                unit_type: PageUnitType::Page,
                boundaries: Some(boundaries),
                pages: Some(
                    (1..=total_count)
                        .map(|page_num| PageInfo {
                            number: page_num as u32,
                            title: None,
                            dimensions: None,
                            image_count: None,
                            table_count: None,
                            hidden: None,
                            is_blank: None,
                            has_vector_graphics: false,
                        })
                        .collect(),
                ),
            })
        } else {
            None
        };

        let extract_image_data = config.needs_image_data();
        let mut extracted_images = Vec::with_capacity(drawings.len());
        for (idx, drawing) in drawings.iter().enumerate() {
            let description = drawing_alt_text(drawing);
            let source_path = drawing.image_ref.as_ref().and_then(|rid| image_rels.get(rid)).cloned();

            let mut image_data = None;
            if extract_image_data
                && let Some(ref rid) = drawing.image_ref
                && let Some(target) = image_rels.get(rid)
                && !crate::extractors::security::has_path_traversal(target)
            {
                let zip_path = if let Some(stripped) = target.strip_prefix('/') {
                    stripped.to_string()
                } else {
                    format!("word/{}", target)
                };
                if let Ok(mut file) = archive.by_name(&zip_path)
                    && file.size() <= crate::extraction::docx::MAX_IMAGE_FILE_SIZE
                {
                    let mut data = Vec::with_capacity(file.size() as usize);
                    if std::io::Read::read_to_end(&mut file, &mut data).is_ok() {
                        image_data = Some(data);
                    }
                }
            }

            let (data, format, width, height) = if let Some(data) = image_data {
                let format = crate::extraction::image_format::detect_image_format(&data);
                let emus_per_px = crate::extraction::docx::EMUS_PER_PIXEL_96DPI;
                let (w, h) = drawing
                    .extent
                    .as_ref()
                    .map(|e| {
                        (
                            Some(u32::try_from(e.cx.max(0) / emus_per_px).unwrap_or(0)),
                            Some(u32::try_from(e.cy.max(0) / emus_per_px).unwrap_or(0)),
                        )
                    })
                    .unwrap_or((None, None));
                (Bytes::from(data), format, w, h)
            } else {
                let format = source_path
                    .as_ref()
                    .and_then(|p| p.rsplit('.').next())
                    .map(|ext| Cow::Owned(ext.to_lowercase()))
                    .unwrap_or(Cow::Borrowed("png"));
                (Bytes::new(), format, None, None)
            };

            let page_number = {
                let placeholder = format!("![](image_{})", idx);
                let placeholder_with_desc = description.as_ref().map(|d| format!("![{}](image_{})", d, idx));

                let byte_pos = text
                    .find(&placeholder)
                    .or_else(|| placeholder_with_desc.as_deref().and_then(|p| text.find(p)));

                if let Some(pos) = byte_pos {
                    if let Some(ref ps) = page_structure
                        && let Some(ref boundaries) = ps.boundaries
                    {
                        boundaries
                            .iter()
                            .find(|b| pos >= b.byte_start && pos < b.byte_end)
                            .map(|b| b.page_number)
                    } else {
                        Some(1)
                    }
                } else {
                    Some(1)
                }
            };

            let (image_kind, kind_confidence) =
                crate::extraction::image_kind::classify(&data, format.as_ref(), width, height, None, None, false);

            extracted_images.push(ExtractedImage {
                data,
                format,
                image_index: idx as u32,
                page_number,
                width,
                height,
                colorspace: None,
                bits_per_component: None,
                is_mask: false,
                description,
                ocr_result: None,
                bounding_box: None,
                source_path,
                image_kind: Some(image_kind),
                kind_confidence: Some(kind_confidence),
                cluster_id: None,
                caption: None,
                qr_codes: None,
                data_base64: None,
            });
        }

        let page_contents = {
            let arc_tables: Vec<Arc<Table>> = tables.iter().map(|t| Arc::new(t.clone())).collect();

            if let Some(ref ps) = page_structure
                && let Some(ref boundaries) = ps.boundaries
                && !boundaries.is_empty()
            {
                let mut pages = Vec::with_capacity(boundaries.len());
                for boundary in boundaries {
                    let page_num = boundary.page_number;
                    let page_text = if boundary.byte_start < text.len() {
                        let mut start = boundary.byte_start.min(text.len());
                        while start < text.len() && !text.is_char_boundary(start) {
                            start += 1;
                        }
                        let mut end = boundary.byte_end.min(text.len());
                        while end > start && !text.is_char_boundary(end) {
                            end -= 1;
                        }
                        text[start..end].trim().to_string()
                    } else {
                        String::new()
                    };

                    let page_tables: Vec<Arc<Table>> = arc_tables
                        .iter()
                        .filter(|t| t.page_number == page_num)
                        .cloned()
                        .collect();

                    let page_image_indices: Vec<u32> = extracted_images
                        .iter()
                        .enumerate()
                        .filter(|(_, i)| i.page_number == Some(page_num))
                        .map(|(i, _)| i as u32)
                        .collect();

                    let is_blank = page_text.chars().filter(|c| !c.is_whitespace()).count() < 3
                        && page_tables.is_empty()
                        && page_image_indices.is_empty();

                    pages.push(PageContent {
                        page_number: page_num,
                        content: page_text,
                        tables: page_tables,
                        image_indices: page_image_indices,
                        hierarchy: None,
                        is_blank: Some(is_blank),
                        layout_regions: None,
                        speaker_notes: None,
                        section_name: None,
                        sheet_name: None,
                    });
                }
                Some(pages)
            } else {
                Some(vec![PageContent {
                    page_number: 1,
                    content: text.clone(),
                    tables: arc_tables,
                    image_indices: (0..extracted_images.len() as u32).collect(),
                    hierarchy: None,
                    is_blank: Some(text.chars().filter(|c| !c.is_whitespace()).count() < 3),
                    layout_regions: None,
                    speaker_notes: None,
                    section_name: None,
                    sheet_name: None,
                }])
            }
        };
        internal_doc.prebuilt_pages = page_contents;

        let meta_title: Option<String> = metadata_map
            .remove(&Cow::Borrowed("title"))
            .and_then(|v| v.as_str().map(|s| s.to_string()));
        let meta_subject: Option<String> = metadata_map
            .remove(&Cow::Borrowed("subject"))
            .and_then(|v| v.as_str().map(|s| s.to_string()));
        let meta_authors: Option<Vec<String>> = metadata_map.remove(&Cow::Borrowed("authors")).and_then(|v| {
            v.as_array()
                .map(|arr| arr.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect())
        });
        let meta_created_by = metadata_map
            .remove(&Cow::Borrowed("created_by"))
            .and_then(|v| v.as_str().map(|s| s.to_string()));
        let meta_modified_by = metadata_map
            .remove(&Cow::Borrowed("modified_by"))
            .and_then(|v| v.as_str().map(|s| s.to_string()));
        let meta_created_at = metadata_map
            .remove(&Cow::Borrowed("created_at"))
            .and_then(|v| v.as_str().map(|s| s.to_string()));
        let meta_modified_at = metadata_map
            .remove(&Cow::Borrowed("modified_at"))
            .and_then(|v| v.as_str().map(|s| s.to_string()));
        let meta_language = metadata_map
            .remove(&Cow::Borrowed("language"))
            .and_then(|v| v.as_str().map(|s| s.to_string()));

        internal_doc.metadata = Metadata {
            title: meta_title,
            subject: meta_subject,
            authors: meta_authors,
            keywords: parsed_keywords,
            language: meta_language,
            created_at: meta_created_at,
            modified_at: meta_modified_at,
            created_by: meta_created_by,
            modified_by: meta_modified_by,
            pages: page_structure,
            format: Some(FormatMetadata::Docx(Box::new(DocxMetadata {
                core_properties: docx_core_properties,
                app_properties: docx_app_properties,
                custom_properties: docx_custom_properties,
            }))),
            additional: metadata_map,
            ..Default::default()
        };

        if let Some(ref filter) = config.content_filter {
            use crate::types::document_structure::ContentLayer;
            internal_doc.elements.retain(|elem| match elem.layer {
                ContentLayer::Header => filter.include_headers,
                ContentLayer::Footer => filter.include_footers,
                _ => true,
            });
        }

        internal_doc.images = extracted_images;
        internal_doc.mime_type = mime_type.to_string();

        if config.max_archive_depth > 0 {
            let (children, embed_warnings) = crate::extraction::ooxml_embedded::extract_ooxml_embedded_objects(
                content,
                "word/embeddings/",
                "docx",
                config,
            )
            .await;
            if !children.is_empty() {
                internal_doc.children = Some(children);
            }
            internal_doc.processing_warnings.extend(embed_warnings);
        }

        tracing::debug!(element_count = internal_doc.elements.len(), "extract_docx: complete");

        Ok(internal_doc)
    }

    fn supported_mime_types(&self) -> &[&str] {
        &[
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
            "application/docx",
            "application/vnd.ms-word.document.macroEnabled.12",
            "application/vnd.openxmlformats-officedocument.wordprocessingml.template",
            "application/vnd.ms-word.template.macroEnabled.12",
        ]
    }

    fn priority(&self) -> i32 {
        50
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::config::extraction::ImageExtractionConfig;
    use crate::types::document_structure::NodeContent;

    #[tokio::test]
    async fn test_docx_extractor_plugin_interface() {
        let extractor = DocxExtractor::new();
        assert_eq!(extractor.name(), "docx-extractor");
        assert_eq!(extractor.version(), env!("CARGO_PKG_VERSION"));
        assert_eq!(extractor.priority(), 50);
        assert_eq!(extractor.supported_mime_types().len(), 5);
    }

    #[tokio::test]
    async fn test_docx_extractor_supports_docx() {
        let extractor = DocxExtractor::new();
        assert!(
            extractor
                .supported_mime_types()
                .contains(&"application/vnd.openxmlformats-officedocument.wordprocessingml.document")
        );
    }

    #[tokio::test]
    async fn test_docx_extractor_default() {
        let extractor = DocxExtractor;
        assert_eq!(extractor.name(), "docx-extractor");
    }

    #[tokio::test]
    async fn test_docx_extractor_initialize_shutdown() {
        let extractor = DocxExtractor::new();
        assert!(extractor.initialize().is_ok());
        assert!(extractor.shutdown().is_ok());
    }

    #[test]
    fn test_convert_docx_table_to_table() {
        use crate::extraction::docx::parser::{Paragraph, Run, Table as DocxTable, TableCell, TableRow};

        let mut table = DocxTable::new();

        let mut header_row = TableRow::default();
        let mut cell1 = TableCell::default();
        let mut para1 = Paragraph::new();
        para1.add_run(Run::new("Name".to_string()));
        cell1.paragraphs.push(para1);
        header_row.cells.push(cell1);

        let mut cell2 = TableCell::default();
        let mut para2 = Paragraph::new();
        para2.add_run(Run::new("Age".to_string()));
        cell2.paragraphs.push(para2);
        header_row.cells.push(cell2);

        table.rows.push(header_row);

        let mut data_row = TableRow::default();
        let mut cell3 = TableCell::default();
        let mut para3 = Paragraph::new();
        para3.add_run(Run::new("Alice".to_string()));
        cell3.paragraphs.push(para3);
        data_row.cells.push(cell3);

        let mut cell4 = TableCell::default();
        let mut para4 = Paragraph::new();
        para4.add_run(Run::new("30".to_string()));
        cell4.paragraphs.push(para4);
        data_row.cells.push(cell4);

        table.rows.push(data_row);

        let result = convert_docx_table_to_table(&table, 1);

        assert_eq!(result.page_number, 1);
        assert_eq!(result.cells.len(), 2);
        assert_eq!(result.cells[0], vec!["Name", "Age"]);
        assert_eq!(result.cells[1], vec!["Alice", "30"]);
        assert!(result.markdown.contains("| Name | Age |"));
        assert!(result.markdown.contains("| Alice | 30 |"));
    }

    /// Helper: build a minimal DOCX ZIP in memory with given document.xml content.
    fn build_test_docx(document_xml: &str) -> Vec<u8> {
        build_test_docx_with_parts(document_xml, None, None, None, None, None, None)
    }

    /// Helper: build a DOCX ZIP with optional parts.
    fn build_test_docx_with_parts(
        document_xml: &str,
        styles_xml: Option<&str>,
        footnotes_xml: Option<&str>,
        endnotes_xml: Option<&str>,
        header_xml: Option<&str>,
        footer_xml: Option<&str>,
        rels_xml: Option<&str>,
    ) -> Vec<u8> {
        use std::io::Write;
        let buf = Vec::new();
        let cursor = std::io::Cursor::new(buf);
        let mut zip = zip::ZipWriter::new(cursor);
        let options: zip::write::FileOptions<()> = zip::write::FileOptions::default();

        let content_types = r#"<?xml version="1.0" encoding="UTF-8"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
  <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
  <Default Extension="xml" ContentType="application/xml"/>
  <Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>
</Types>"#;
        zip.start_file("[Content_Types].xml", options).unwrap();
        zip.write_all(content_types.as_bytes()).unwrap();

        zip.start_file("word/document.xml", options).unwrap();
        zip.write_all(document_xml.as_bytes()).unwrap();

        if let Some(styles) = styles_xml {
            zip.start_file("word/styles.xml", options).unwrap();
            zip.write_all(styles.as_bytes()).unwrap();
        }

        if let Some(fn_xml) = footnotes_xml {
            zip.start_file("word/footnotes.xml", options).unwrap();
            zip.write_all(fn_xml.as_bytes()).unwrap();
        }

        if let Some(en_xml) = endnotes_xml {
            zip.start_file("word/endnotes.xml", options).unwrap();
            zip.write_all(en_xml.as_bytes()).unwrap();
        }

        if let Some(h_xml) = header_xml {
            zip.start_file("word/header1.xml", options).unwrap();
            zip.write_all(h_xml.as_bytes()).unwrap();
        }

        if let Some(f_xml) = footer_xml {
            zip.start_file("word/footer1.xml", options).unwrap();
            zip.write_all(f_xml.as_bytes()).unwrap();
        }

        if let Some(rels) = rels_xml {
            zip.start_file("word/_rels/document.xml.rels", options).unwrap();
            zip.write_all(rels.as_bytes()).unwrap();
        }

        zip.finish().unwrap().into_inner()
    }

    /// Helper: build a DOCX ZIP from `word/document.xml` plus an arbitrary list of
    /// additional package parts (path, content). Unlike [`build_test_docx_with_parts`],
    /// this isn't limited to one header/footer/rels part — used for synthetic
    /// fixtures needing several header/footer parts (#83), `word/comments.xml`
    /// (#82), or a custom `word/_rels/document.xml.rels`.
    fn build_test_docx_with_files(document_xml: &str, extra_files: &[(&str, &str)]) -> Vec<u8> {
        use std::io::Write;
        let buf = Vec::new();
        let cursor = std::io::Cursor::new(buf);
        let mut zip = zip::ZipWriter::new(cursor);
        let options: zip::write::FileOptions<()> = zip::write::FileOptions::default();

        let content_types = r#"<?xml version="1.0" encoding="UTF-8"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
  <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
  <Default Extension="xml" ContentType="application/xml"/>
  <Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>
</Types>"#;
        zip.start_file("[Content_Types].xml", options).unwrap();
        zip.write_all(content_types.as_bytes()).unwrap();

        zip.start_file("word/document.xml", options).unwrap();
        zip.write_all(document_xml.as_bytes()).unwrap();

        for (path, xml) in extra_files {
            zip.start_file(*path, options).unwrap();
            zip.write_all(xml.as_bytes()).unwrap();
        }

        zip.finish().unwrap().into_inner()
    }

    #[tokio::test]
    async fn should_match_single_extraction_in_batch_mode() {
        let data = build_test_docx(TRACK_CHANGES_XML);
        let extractor = DocxExtractor::new();
        let config = ExtractionConfig {
            output_format: crate::core::config::OutputFormat::Markdown,
            include_document_structure: true,
            ..Default::default()
        };
        let mime_type = "application/vnd.openxmlformats-officedocument.wordprocessingml.document";

        let single = extractor.extract_content(&data, mime_type, &config).await.unwrap();
        let batch = crate::core::batch_mode::with_batch_mode(extractor.extract_content(&data, mime_type, &config))
            .await
            .unwrap();

        assert_eq!(
            serde_json::to_value(batch).unwrap(),
            serde_json::to_value(single).unwrap(),
            "batch-mode ownership changes must preserve the exact internal document"
        );
    }

    #[tokio::test]
    async fn test_full_extraction_with_headings_paragraphs() {
        let document_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:body>
    <w:p><w:pPr><w:pStyle w:val="Title"/></w:pPr><w:r><w:t>Document Title</w:t></w:r></w:p>
    <w:p><w:r><w:t>First paragraph content.</w:t></w:r></w:p>
    <w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>Section One</w:t></w:r></w:p>
    <w:p><w:r><w:t>Section one body text.</w:t></w:r></w:p>
  </w:body>
</w:document>"#;

        let data = build_test_docx(document_xml);
        let extractor = DocxExtractor::new();
        let config = ExtractionConfig {
            output_format: crate::core::config::OutputFormat::Markdown,
            ..Default::default()
        };
        let result = extractor
            .extract_content(
                &data,
                "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
                &config,
            )
            .await
            .unwrap();
        let result =
            crate::extraction::derive::derive_extraction_result(result, true, crate::core::config::OutputFormat::Plain);

        assert!(
            result.content.contains("Document Title"),
            "Title should be present: {}",
            result.content
        );
        assert!(
            result.content.contains("Section One"),
            "Heading1 should be present: {}",
            result.content
        );
        assert!(result.content.contains("First paragraph content."));
        assert!(result.content.contains("Section one body text."));

        let doc = result.document.as_ref().expect("DocumentStructure should be present");
        use crate::types::NodeContent;
        let headings: Vec<_> = doc
            .nodes
            .iter()
            .filter(|n| matches!(n.content, NodeContent::Heading { .. }))
            .collect();
        assert!(!headings.is_empty(), "Should have heading nodes in DocumentStructure");
    }

    #[tokio::test]
    async fn test_full_extraction_with_formatting() {
        let document_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:body>
    <w:p>
      <w:r><w:rPr><w:b/></w:rPr><w:t>Bold text</w:t></w:r>
      <w:r><w:t> and </w:t></w:r>
      <w:r><w:rPr><w:i/></w:rPr><w:t>italic text</w:t></w:r>
      <w:r><w:t> and </w:t></w:r>
      <w:r><w:rPr><w:u/></w:rPr><w:t>underlined text</w:t></w:r>
    </w:p>
  </w:body>
</w:document>"#;

        let data = build_test_docx(document_xml);
        let extractor = DocxExtractor::new();
        let config = ExtractionConfig {
            output_format: crate::core::config::OutputFormat::Markdown,
            ..Default::default()
        };
        let result = extractor
            .extract_content(
                &data,
                "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
                &config,
            )
            .await
            .unwrap();
        let result =
            crate::extraction::derive::derive_extraction_result(result, true, crate::core::config::OutputFormat::Plain);

        assert!(result.content.contains("Bold text"), "Bold: {}", result.content);
        assert!(result.content.contains("italic text"), "Italic: {}", result.content);
        assert!(
            result.content.contains("underlined text"),
            "Underline: {}",
            result.content
        );

        let doc = result.document.as_ref().expect("DocumentStructure should be present");
        let all_annotations: Vec<_> = doc.nodes.iter().flat_map(|n| &n.annotations).collect();
        assert!(
            all_annotations
                .iter()
                .any(|a| a.kind == crate::types::document_structure::AnnotationKind::Bold),
            "Should have bold annotation"
        );
        assert!(
            all_annotations
                .iter()
                .any(|a| a.kind == crate::types::document_structure::AnnotationKind::Italic),
            "Should have italic annotation"
        );
        assert!(
            all_annotations
                .iter()
                .any(|a| a.kind == crate::types::document_structure::AnnotationKind::Underline),
            "Should have underline annotation"
        );
    }

    #[tokio::test]
    async fn test_docx_inject_placeholders_true() {
        let drawing_xml = r#"<w:p xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"
                             xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing"
                             xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"
                             xmlns:pic="http://schemas.openxmlformats.org/drawingml/2006/picture"
                             xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
          <w:r>
            <w:drawing>
              <wp:inline>
                <wp:extent cx="914400" cy="457200"/>
                <wp:docPr id="1" name="Picture 1" descr="A test image"/>
                <a:graphic>
                  <a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/picture">
                    <pic:pic>
                      <pic:blipFill>
                        <a:blip r:embed="rId5"/>
                      </pic:blipFill>
                    </pic:pic>
                  </a:graphicData>
                </a:graphic>
              </wp:inline>
            </w:drawing>
          </w:r>
        </w:p>"#;

        let rels_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId5" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="media/image1.png"/>
</Relationships>"#;

        let document_xml = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:body>
    {}
  </w:body>
</w:document>"#,
            drawing_xml
        );

        let data = build_test_docx_with_parts(&document_xml, None, None, None, None, None, Some(rels_xml));
        let extractor = DocxExtractor::new();
        let config = ExtractionConfig {
            images: Some(crate::core::config::extraction::ImageExtractionConfig {
                extract_images: false,
                inject_placeholders: true,
                ..Default::default()
            }),
            include_document_structure: true,
            ..Default::default()
        };

        let internal_doc = extractor
            .extract_content(
                &data,
                "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
                &config,
            )
            .await
            .expect("Extraction failed");

        let result = crate::extraction::derive::derive_extraction_result(
            internal_doc,
            true,
            crate::core::config::OutputFormat::Markdown,
        );

        let doc = result.document.as_ref().expect("DocumentStructure should be present");
        let has_image = doc.nodes.iter().any(|n| matches!(n.content, NodeContent::Image { .. }));
        assert!(
            has_image,
            "Image node should be present when inject_placeholders is true"
        );

        let formatted = result
            .formatted_content
            .as_ref()
            .expect("Formatted content should be present");
        assert!(
            formatted.contains("![A test image](media/image1.png)"),
            "Markdown should contain image placeholder. Content: {}",
            formatted
        );
    }

    #[tokio::test]
    async fn test_docx_inject_placeholders_false() {
        let drawing_xml = r#"<w:p xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"
                             xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing"
                             xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"
                             xmlns:pic="http://schemas.openxmlformats.org/drawingml/2006/picture"
                             xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
          <w:r>
            <w:drawing>
              <wp:inline>
                <wp:extent cx="914400" cy="457200"/>
                <wp:docPr id="1" name="Picture 1" descr="A test image"/>
                <a:graphic>
                  <a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/picture">
                    <pic:pic>
                      <pic:blipFill>
                        <a:blip r:embed="rId5"/>
                      </pic:blipFill>
                    </pic:pic>
                  </a:graphicData>
                </a:graphic>
              </wp:inline>
            </w:drawing>
          </w:r>
        </w:p>"#;

        let rels_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId5" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="media/image1.png"/>
</Relationships>"#;

        let document_xml = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:body>
    <w:p><w:r><w:t>Before image</w:t></w:r></w:p>
    {}
    <w:p><w:r><w:t>After image</w:t></w:r></w:p>
  </w:body>
</w:document>"#,
            drawing_xml
        );

        let data = build_test_docx_with_parts(&document_xml, None, None, None, None, None, Some(rels_xml));
        let extractor = DocxExtractor::new();
        let config = ExtractionConfig {
            images: Some(ImageExtractionConfig {
                extract_images: false,
                inject_placeholders: false,
                ..Default::default()
            }),
            include_document_structure: true,
            ..Default::default()
        };

        let internal_doc = extractor
            .extract_content(
                &data,
                "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
                &config,
            )
            .await
            .expect("Extraction failed");

        let result = crate::extraction::derive::derive_extraction_result(
            internal_doc,
            true,
            crate::core::config::OutputFormat::Markdown,
        );

        let doc = result.document.as_ref().expect("DocumentStructure should be present");
        let has_image = doc.nodes.iter().any(|n| matches!(n.content, NodeContent::Image { .. }));
        assert!(
            !has_image,
            "Image node should NOT be present when inject_placeholders is false"
        );

        let formatted = result
            .formatted_content
            .as_ref()
            .expect("Formatted content should be present");
        assert!(
            !formatted.contains("![A test image](media/image1.png)"),
            "Markdown should NOT contain image placeholder. Content: {}",
            formatted
        );
        assert!(result.content.contains("Before image"));
        assert!(result.content.contains("After image"));
    }

    #[tokio::test]
    async fn test_full_extraction_with_headers_footers() {
        let document_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:body>
    <w:p><w:r><w:t>Body content here.</w:t></w:r></w:p>
  </w:body>
</w:document>"#;

        let header_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<w:hdr xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:p><w:r><w:t>Page Header</w:t></w:r></w:p>
</w:hdr>"#;

        let footer_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<w:ftr xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:p><w:r><w:t>Page Footer</w:t></w:r></w:p>
</w:ftr>"#;

        let data = build_test_docx_with_parts(document_xml, None, None, None, Some(header_xml), Some(footer_xml), None);
        let extractor = DocxExtractor::new();
        let config = ExtractionConfig {
            output_format: crate::core::config::OutputFormat::Markdown,
            ..Default::default()
        };
        let result = extractor
            .extract_content(
                &data,
                "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
                &config,
            )
            .await
            .unwrap();
        let result =
            crate::extraction::derive::derive_extraction_result(result, true, crate::core::config::OutputFormat::Plain);

        assert!(
            result.content.contains("Body content here."),
            "Body: {}",
            result.content
        );

        let doc = result.document.as_ref().expect("DocumentStructure should be present");
        use crate::types::ContentLayer;
        let header_nodes: Vec<_> = doc
            .nodes
            .iter()
            .filter(|n| n.content_layer == ContentLayer::Header)
            .collect();
        assert!(!header_nodes.is_empty(), "Should have header layer nodes");
        let footer_nodes: Vec<_> = doc
            .nodes
            .iter()
            .filter(|n| n.content_layer == ContentLayer::Footer)
            .collect();
        assert!(!footer_nodes.is_empty(), "Should have footer layer nodes");
    }

    #[tokio::test]
    async fn test_full_extraction_with_footnotes() {
        let document_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:body>
    <w:p>
      <w:r><w:t>Text with note</w:t></w:r>
      <w:r><w:footnoteReference w:id="2"/></w:r>
    </w:p>
  </w:body>
</w:document>"#;

        let footnotes_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<w:footnotes xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:footnote w:id="0"><w:p><w:r><w:t>separator</w:t></w:r></w:p></w:footnote>
  <w:footnote w:id="1"><w:p><w:r><w:t>continuation</w:t></w:r></w:p></w:footnote>
  <w:footnote w:id="2"><w:p><w:r><w:t>This is the footnote content.</w:t></w:r></w:p></w:footnote>
</w:footnotes>"#;

        let data = build_test_docx_with_parts(document_xml, None, Some(footnotes_xml), None, None, None, None);
        let extractor = DocxExtractor::new();
        let config = ExtractionConfig {
            output_format: crate::core::config::OutputFormat::Markdown,
            ..Default::default()
        };
        let result = extractor
            .extract_content(
                &data,
                "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
                &config,
            )
            .await
            .unwrap();
        let result =
            crate::extraction::derive::derive_extraction_result(result, true, crate::core::config::OutputFormat::Plain);

        assert!(
            result.content.contains("[^2]"),
            "Should have footnote ref: {}",
            result.content
        );
        let doc = result.document.as_ref().expect("should have document structure");
        let has_footnote = doc.nodes.iter().any(
            |n| matches!(&n.content, crate::types::NodeContent::Footnote { text } if text.contains("footnote content")),
        );
        assert!(has_footnote, "DocumentStructure should contain footnote node");
        assert!(!result.content.contains("separator"), "Separator should be filtered");
        assert!(
            !result.content.contains("continuation"),
            "Continuation should be filtered"
        );
        let doc = result.document.as_ref().expect("DocumentStructure should be present");
        assert!(
            !doc.relationships.is_empty(),
            "Should have footnote relationships in DocumentStructure"
        );
    }

    #[tokio::test]
    async fn test_full_extraction_with_style_based_headings() {
        let document_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:body>
    <w:p><w:pPr><w:pStyle w:val="CustomTitle"/></w:pPr><w:r><w:t>Custom Title</w:t></w:r></w:p>
    <w:p><w:r><w:t>Body text.</w:t></w:r></w:p>
  </w:body>
</w:document>"#;

        let styles_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<w:styles xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:style w:type="paragraph" w:styleId="CustomTitle">
    <w:name w:val="Custom Title"/>
    <w:pPr><w:outlineLvl w:val="0"/></w:pPr>
  </w:style>
</w:styles>"#;

        let data = build_test_docx_with_parts(document_xml, Some(styles_xml), None, None, None, None, None);
        let extractor = DocxExtractor::new();
        let config = ExtractionConfig {
            output_format: crate::core::config::OutputFormat::Markdown,
            ..Default::default()
        };
        let result = extractor
            .extract_content(
                &data,
                "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
                &config,
            )
            .await
            .unwrap();
        let result =
            crate::extraction::derive::derive_extraction_result(result, true, crate::core::config::OutputFormat::Plain);

        assert!(
            result.content.contains("Custom Title"),
            "Style-based heading text should be present: {}",
            result.content
        );
        let doc = result.document.as_ref().expect("DocumentStructure should be present");
        use crate::types::NodeContent;
        let h1_nodes: Vec<_> = doc
            .nodes
            .iter()
            .filter(|n| matches!(n.content, NodeContent::Heading { level: 1, .. }))
            .collect();
        assert!(
            !h1_nodes.is_empty(),
            "Should have h1 heading node from style-based heading"
        );
    }

    #[tokio::test]
    async fn test_document_structure_generation() {
        let document_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:body>
    <w:p><w:pPr><w:pStyle w:val="Title"/></w:pPr><w:r><w:t>Doc Title</w:t></w:r></w:p>
    <w:p><w:r><w:t>A paragraph.</w:t></w:r></w:p>
    <w:tbl>
      <w:tr><w:tc><w:p><w:r><w:t>Cell 1</w:t></w:r></w:p></w:tc></w:tr>
    </w:tbl>
  </w:body>
</w:document>"#;

        let header_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<w:hdr xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:p><w:r><w:t>Header</w:t></w:r></w:p>
</w:hdr>"#;

        let data = build_test_docx_with_parts(document_xml, None, None, None, Some(header_xml), None, None);
        let extractor = DocxExtractor::new();
        let config = ExtractionConfig {
            include_document_structure: true,
            ..Default::default()
        };
        let result = extractor
            .extract_content(
                &data,
                "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
                &config,
            )
            .await
            .unwrap();
        let result =
            crate::extraction::derive::derive_extraction_result(result, true, crate::core::config::OutputFormat::Plain);

        assert!(result.document.is_some(), "DocumentStructure should be populated");
        let doc = result.document.unwrap();

        assert!(!doc.nodes.is_empty(), "Should have document nodes");

        assert!(doc.validate().is_ok(), "DocumentStructure should be valid");

        use crate::types::NodeContent;
        let headings: Vec<_> = doc
            .nodes
            .iter()
            .filter(|n| matches!(n.content, NodeContent::Heading { .. }))
            .collect();
        assert!(!headings.is_empty(), "Should have heading nodes");

        let paragraphs: Vec<_> = doc
            .nodes
            .iter()
            .filter(|n| matches!(n.content, NodeContent::Paragraph { .. }))
            .collect();
        assert!(!paragraphs.is_empty(), "Should have paragraph nodes");

        let tables: Vec<_> = doc
            .nodes
            .iter()
            .filter(|n| matches!(n.content, NodeContent::Table { .. }))
            .collect();
        assert!(!tables.is_empty(), "Should have table nodes");

        use crate::types::ContentLayer;
        let headers: Vec<_> = doc
            .nodes
            .iter()
            .filter(|n| n.content_layer == ContentLayer::Header)
            .collect();
        assert!(!headers.is_empty(), "Should have header nodes");
    }

    #[tokio::test]
    async fn test_pages_populated_single_page() {
        let document_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:body>
    <w:p><w:r><w:t>Simple single page document.</w:t></w:r></w:p>
  </w:body>
</w:document>"#;

        let data = build_test_docx(document_xml);
        let extractor = DocxExtractor::new();
        let config = ExtractionConfig {
            output_format: crate::core::config::OutputFormat::Markdown,
            ..Default::default()
        };
        let result = extractor
            .extract_content(
                &data,
                "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
                &config,
            )
            .await
            .unwrap();
        let result =
            crate::extraction::derive::derive_extraction_result(result, true, crate::core::config::OutputFormat::Plain);

        assert!(
            result.content.contains("Simple single page document."),
            "Content should contain the document text: {}",
            result.content
        );
    }

    #[tokio::test]
    async fn test_full_extraction_with_endnotes() {
        let document_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:body>
    <w:p>
      <w:r><w:t>Text with endnote</w:t></w:r>
      <w:r><w:endnoteReference w:id="2"/></w:r>
    </w:p>
  </w:body>
</w:document>"#;

        let endnotes_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<w:endnotes xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:endnote w:id="0"><w:p><w:r><w:t>separator</w:t></w:r></w:p></w:endnote>
  <w:endnote w:id="1"><w:p><w:r><w:t>continuation</w:t></w:r></w:p></w:endnote>
  <w:endnote w:id="2"><w:p><w:r><w:t>This is the endnote.</w:t></w:r></w:p></w:endnote>
</w:endnotes>"#;

        let data = build_test_docx_with_parts(document_xml, None, None, Some(endnotes_xml), None, None, None);
        let extractor = DocxExtractor::new();
        let config = ExtractionConfig {
            output_format: crate::core::config::OutputFormat::Markdown,
            ..Default::default()
        };
        let result = extractor
            .extract_content(
                &data,
                "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
                &config,
            )
            .await
            .unwrap();
        let result =
            crate::extraction::derive::derive_extraction_result(result, true, crate::core::config::OutputFormat::Plain);

        assert!(
            result.content.contains("[^2]"),
            "Should have endnote ref: {}",
            result.content
        );
        assert!(
            result.document.as_ref().is_some_and(|doc| doc.nodes.iter().any(
                |n| matches!(&n.content, crate::types::NodeContent::Footnote { text } if text.contains("endnote"))
            )),
            "DocumentStructure should contain endnote node"
        );
        assert!(!result.content.contains("separator"), "Separator should be filtered");
    }

    #[tokio::test]
    async fn test_typed_metadata_fields_populated() {
        use std::io::Write;
        let buf = Vec::new();
        let cursor = std::io::Cursor::new(buf);
        let mut zip = zip::ZipWriter::new(cursor);
        let options: zip::write::FileOptions<()> = zip::write::FileOptions::default();

        zip.start_file("[Content_Types].xml", options).unwrap();
        zip.write_all(br#"<?xml version="1.0" encoding="UTF-8"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
  <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
  <Default Extension="xml" ContentType="application/xml"/>
  <Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>
</Types>"#).unwrap();

        zip.start_file("word/document.xml", options).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:body><w:p><w:r><w:t>Content</w:t></w:r></w:p></w:body>
</w:document>"#,
        )
        .unwrap();

        zip.start_file("docProps/core.xml", options).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties"
                   xmlns:dc="http://purl.org/dc/elements/1.1/"
                   xmlns:dcterms="http://purl.org/dc/terms/">
  <dc:title>My Document</dc:title>
  <dc:creator>Jane Doe</dc:creator>
  <dc:subject>Test Subject</dc:subject>
  <cp:lastModifiedBy>John Smith</cp:lastModifiedBy>
  <dcterms:created>2024-01-15T10:30:00Z</dcterms:created>
  <dcterms:modified>2024-02-20T14:45:00Z</dcterms:modified>
  <dc:language>en-US</dc:language>
</cp:coreProperties>"#,
        )
        .unwrap();

        let data = zip.finish().unwrap().into_inner();

        let extractor = DocxExtractor::new();
        let config = ExtractionConfig {
            output_format: crate::core::config::OutputFormat::Markdown,
            ..Default::default()
        };
        let result = extractor
            .extract_content(
                &data,
                "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
                &config,
            )
            .await
            .unwrap();
        let result =
            crate::extraction::derive::derive_extraction_result(result, true, crate::core::config::OutputFormat::Plain);

        assert_eq!(result.metadata.title.as_deref(), Some("My Document"));
        assert_eq!(result.metadata.subject.as_deref(), Some("Test Subject"));
        assert_eq!(result.metadata.authors, Some(vec!["Jane Doe".to_string()]));
        assert_eq!(result.metadata.created_by.as_deref(), Some("Jane Doe"));
        assert_eq!(result.metadata.modified_by.as_deref(), Some("John Smith"));
        assert_eq!(result.metadata.created_at.as_deref(), Some("2024-01-15T10:30:00Z"));
        assert_eq!(result.metadata.modified_at.as_deref(), Some("2024-02-20T14:45:00Z"));
        assert_eq!(result.metadata.language.as_deref(), Some("en-US"));

        assert!(
            result.metadata.additional.get("title").is_none(),
            "title should not be in additional"
        );
        assert!(
            result.metadata.additional.get("created_by").is_none(),
            "created_by should not be in additional"
        );
    }

    #[tokio::test]
    async fn test_images_none_when_extraction_disabled() {
        let document_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:body><w:p><w:r><w:t>No images.</w:t></w:r></w:p></w:body>
</w:document>"#;

        let data = build_test_docx(document_xml);
        let extractor = DocxExtractor::new();
        let config = ExtractionConfig::default();
        let result = extractor
            .extract_content(
                &data,
                "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
                &config,
            )
            .await
            .unwrap();
        let result =
            crate::extraction::derive::derive_extraction_result(result, true, crate::core::config::OutputFormat::Plain);

        assert!(
            result.images.is_none(),
            "Images should be None when extraction is disabled"
        );
    }

    #[test]
    fn test_vertical_merge_renders_empty_cells() {
        use crate::extraction::docx::parser::{Paragraph, Run, Table as DocxTable, TableCell, TableRow};
        use crate::extraction::docx::table::{CellProperties, RowProperties, VerticalMerge};

        let mut table = DocxTable::new();

        let mut row1 = TableRow {
            properties: Some(RowProperties {
                is_header: true,
                ..Default::default()
            }),
            ..Default::default()
        };

        let mut cell1 = TableCell::default();
        let mut p1 = Paragraph::new();
        p1.add_run(Run::new("Name".to_string()));
        cell1.paragraphs.push(p1);
        row1.cells.push(cell1);

        let mut cell2 = TableCell {
            properties: Some(CellProperties {
                v_merge: Some(VerticalMerge::Restart),
                ..Default::default()
            }),
            ..Default::default()
        };
        let mut p2 = Paragraph::new();
        p2.add_run(Run::new("Score".to_string()));
        cell2.paragraphs.push(p2);
        row1.cells.push(cell2);
        table.rows.push(row1);

        let mut row2 = TableRow::default();
        let mut cell3 = TableCell::default();
        let mut p3 = Paragraph::new();
        p3.add_run(Run::new("Alice".to_string()));
        cell3.paragraphs.push(p3);
        row2.cells.push(cell3);

        let mut cell4 = TableCell {
            properties: Some(CellProperties {
                v_merge: Some(VerticalMerge::Continue),
                ..Default::default()
            }),
            ..Default::default()
        };
        let mut p4 = Paragraph::new();
        p4.add_run(Run::new("Should be hidden".to_string()));
        cell4.paragraphs.push(p4);
        row2.cells.push(cell4);
        table.rows.push(row2);

        let md = table.to_markdown();
        assert!(md.contains("Score"), "Restart cell should show content");
        assert!(
            !md.contains("Should be hidden"),
            "Continue cell should be empty: {}",
            md
        );
        assert!(md.contains("Alice"), "Normal cell should show content");
    }

    #[tokio::test]
    async fn test_drawing_image_placeholder_in_markdown() {
        use crate::extraction::docx::drawing::{DocProperties, Drawing, DrawingType};
        use crate::extraction::docx::parser::{Document, DocumentElement, Paragraph, Run};

        let mut doc = Document::new();

        let mut para = Paragraph::new();
        para.add_run(Run::new("Before image.".to_string()));
        let p_idx = doc.paragraphs.len();
        doc.paragraphs.push(para);
        doc.elements.push(DocumentElement::Paragraph(p_idx));

        let drawing = Drawing {
            drawing_type: DrawingType::Inline,
            extent: None,
            doc_properties: Some(DocProperties {
                id: Some("1".to_string()),
                name: Some("Picture 1".to_string()),
                description: Some("A test image".to_string()),
            }),
            image_ref: Some("rId1".to_string()),
            text_box_content: None,
        };
        let d_idx = doc.drawings.len();
        doc.drawings.push(drawing);
        doc.elements.push(DocumentElement::Drawing(d_idx));

        let mut para2 = Paragraph::new();
        para2.add_run(Run::new("After image.".to_string()));
        let p2_idx = doc.paragraphs.len();
        doc.paragraphs.push(para2);
        doc.elements.push(DocumentElement::Paragraph(p2_idx));

        let md = doc.to_markdown(true);
        assert!(
            md.contains("![A test image](image)"),
            "Should have image placeholder: {}",
            md
        );
        assert!(md.contains("Before image."), "Should have text before");
        assert!(md.contains("After image."), "Should have text after");
    }

    /// Regression test for issue #484: image placeholders must appear even with
    /// default (Plain) output format when extract_images is enabled.
    #[tokio::test]
    async fn test_image_placeholder_with_default_output_format() {
        let document_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"
            xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing"
            xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"
            xmlns:pic="http://schemas.openxmlformats.org/drawingml/2006/picture"
            xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
  <w:body>
    <w:p><w:r><w:t>Text before image.</w:t></w:r></w:p>
    <w:p><w:r>
      <w:drawing>
        <wp:inline>
          <wp:extent cx="914400" cy="914400"/>
          <wp:docPr id="1" name="Picture 1" descr="Test image"/>
          <a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/picture">
            <pic:pic><pic:blipFill><a:blip r:embed="rId5"/></pic:blipFill></pic:pic>
          </a:graphicData></a:graphic>
        </wp:inline>
      </w:drawing>
    </w:r></w:p>
    <w:p><w:r><w:t>Text after image.</w:t></w:r></w:p>
  </w:body>
</w:document>"#;

        let docx_bytes = build_test_docx(document_xml);

        let config = ExtractionConfig {
            images: Some(crate::core::config::ImageExtractionConfig {
                extract_images: true,
                ..Default::default()
            }),
            ..Default::default()
        };

        let extractor = DocxExtractor::new();
        let result = extractor
            .extract_content(
                &docx_bytes,
                "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
                &config,
            )
            .await
            .unwrap();
        let result =
            crate::extraction::derive::derive_extraction_result(result, true, crate::core::config::OutputFormat::Plain);

        assert!(
            result.content.contains("Text before image."),
            "Should contain text before image: {}",
            result.content
        );
        assert!(
            result.content.contains("Text after image."),
            "Should contain text after image: {}",
            result.content
        );
        let doc = result.document.as_ref().expect("DocumentStructure should be present");
        use crate::types::NodeContent;
        let image_nodes: Vec<_> = doc
            .nodes
            .iter()
            .filter(|n| matches!(n.content, NodeContent::Image { .. }))
            .collect();
        assert!(!image_nodes.is_empty(), "Should have image nodes in DocumentStructure");
    }

    #[tokio::test]
    async fn test_docx_metadata_format_field() {
        use std::io::Write;
        let buf = Vec::new();
        let cursor = std::io::Cursor::new(buf);
        let mut zip = zip::ZipWriter::new(cursor);
        let options: zip::write::FileOptions<()> = zip::write::FileOptions::default();

        zip.start_file("[Content_Types].xml", options).unwrap();
        zip.write_all(br#"<?xml version="1.0" encoding="UTF-8"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
  <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
  <Default Extension="xml" ContentType="application/xml"/>
  <Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>
</Types>"#).unwrap();

        zip.start_file("word/document.xml", options).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:body><w:p><w:r><w:t>Content</w:t></w:r></w:p></w:body>
</w:document>"#,
        )
        .unwrap();

        zip.start_file("docProps/core.xml", options).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties"
                   xmlns:dc="http://purl.org/dc/elements/1.1/">
  <dc:title>Format Test</dc:title>
</cp:coreProperties>"#,
        )
        .unwrap();

        zip.start_file("docProps/app.xml", options).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<Properties xmlns="http://schemas.openxmlformats.org/officeDocument/2006/extended-properties">
  <Pages>3</Pages>
  <Words>500</Words>
</Properties>"#,
        )
        .unwrap();

        let data = zip.finish().unwrap().into_inner();

        let extractor = DocxExtractor::new();
        let config = ExtractionConfig {
            output_format: crate::core::config::OutputFormat::Markdown,
            ..Default::default()
        };
        let result = extractor
            .extract_content(
                &data,
                "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
                &config,
            )
            .await
            .unwrap();
        let result =
            crate::extraction::derive::derive_extraction_result(result, true, crate::core::config::OutputFormat::Plain);

        assert!(result.metadata.format.is_some(), "Format should be populated");
        match result.metadata.format.as_ref().unwrap() {
            FormatMetadata::Docx(docx_meta) => {
                assert!(docx_meta.core_properties.is_some(), "Core properties should be present");
                let core = docx_meta.core_properties.as_ref().unwrap();
                assert_eq!(core.title.as_deref(), Some("Format Test"));

                assert!(docx_meta.app_properties.is_some(), "App properties should be present");
                let app = docx_meta.app_properties.as_ref().unwrap();
                assert_eq!(app.pages, Some(3));
                assert_eq!(app.words, Some(500));
            }
            _ => panic!("Expected FormatMetadata::Docx"),
        }
    }

    /// Document XML with one insertion (w:ins), one deletion (w:del), and one
    /// format change (w:rPrChange), each carrying w:id / w:author / w:date.
    const TRACK_CHANGES_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:body>
    <w:p>
      <w:r><w:t xml:space="preserve">Original text. </w:t></w:r>
      <w:ins w:id="1" w:author="Alice" w:date="2024-03-15T10:00:00Z">
        <w:r><w:t>inserted content</w:t></w:r>
      </w:ins>
    </w:p>
    <w:p>
      <w:del w:id="2" w:author="Bob" w:date="2024-03-16T14:30:00Z">
        <w:r><w:delText>deleted text</w:delText></w:r>
      </w:del>
      <w:r><w:t>Remaining text.</w:t></w:r>
    </w:p>
    <w:p>
      <w:r>
        <w:rPr>
          <w:rPrChange w:id="3" w:author="Carol" w:date="2024-03-17T09:15:00Z">
            <w:rPr><w:b/></w:rPr>
          </w:rPrChange>
          <w:i/>
        </w:rPr>
        <w:t>Format-changed text.</w:t>
      </w:r>
    </w:p>
  </w:body>
</w:document>"#;

    #[tokio::test]
    async fn should_extract_correct_revision_count_from_track_changes_docx() {
        let data = build_test_docx(TRACK_CHANGES_XML);
        let extractor = DocxExtractor::new();
        let config = ExtractionConfig::default();
        let internal_doc = extractor
            .extract_content(
                &data,
                "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
                &config,
            )
            .await
            .unwrap();
        let result = crate::extraction::derive::derive_extraction_result(
            internal_doc,
            false,
            crate::core::config::OutputFormat::Plain,
        );

        let revisions = result
            .revisions
            .expect("revisions should be Some for a doc with track changes");
        assert_eq!(revisions.len(), 3, "expected 3 revisions (1 ins + 1 del + 1 rPrChange)");
    }

    #[tokio::test]
    async fn should_extract_revision_authors_timestamps_and_kinds() {
        use crate::types::revisions::RevisionKind;

        let data = build_test_docx(TRACK_CHANGES_XML);
        let extractor = DocxExtractor::new();
        let config = ExtractionConfig::default();
        let internal_doc = extractor
            .extract_content(
                &data,
                "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
                &config,
            )
            .await
            .unwrap();
        let result = crate::extraction::derive::derive_extraction_result(
            internal_doc,
            false,
            crate::core::config::OutputFormat::Plain,
        );

        let revisions = result.revisions.unwrap();

        let ins = revisions.iter().find(|r| r.kind == RevisionKind::Insertion).unwrap();
        assert_eq!(ins.author.as_deref(), Some("Alice"));
        assert_eq!(ins.timestamp.as_deref(), Some("2024-03-15T10:00:00Z"));
        assert_eq!(ins.revision_id, "1");

        let del = revisions.iter().find(|r| r.kind == RevisionKind::Deletion).unwrap();
        assert_eq!(del.author.as_deref(), Some("Bob"));
        assert_eq!(del.timestamp.as_deref(), Some("2024-03-16T14:30:00Z"));
        assert_eq!(del.revision_id, "2");

        let fmt = revisions.iter().find(|r| r.kind == RevisionKind::FormatChange).unwrap();
        assert_eq!(fmt.author.as_deref(), Some("Carol"));
        assert_eq!(fmt.timestamp.as_deref(), Some("2024-03-17T09:15:00Z"));
        assert_eq!(fmt.revision_id, "3");
    }

    #[tokio::test]
    async fn should_capture_format_change_property_delta() {
        use crate::types::revisions::RevisionKind;

        let data = build_test_docx(TRACK_CHANGES_XML);
        let extractor = DocxExtractor::new();
        let config = ExtractionConfig::default();
        let internal_doc = extractor
            .extract_content(
                &data,
                "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
                &config,
            )
            .await
            .unwrap();
        let result = crate::extraction::derive::derive_extraction_result(
            internal_doc,
            false,
            crate::core::config::OutputFormat::Plain,
        );

        let revisions = result.revisions.unwrap();
        let fmt = revisions.iter().find(|r| r.kind == RevisionKind::FormatChange).unwrap();
        assert!(fmt.delta.content.is_empty());
        assert!(fmt.delta.table_changes.is_empty());
        assert!(
            fmt.delta.property_changes.iter().any(|change| {
                change.name == "bold" && change.from.as_deref() == Some("true") && change.to.as_deref() == Some("false")
            }),
            "expected bold delta in {:?}",
            fmt.delta.property_changes
        );
        assert!(
            fmt.delta.property_changes.iter().any(|change| {
                change.name == "italic" && change.from.is_none() && change.to.as_deref() == Some("true")
            }),
            "expected italic delta in {:?}",
            fmt.delta.property_changes
        );
    }

    #[tokio::test]
    async fn should_include_inserted_text_and_exclude_deleted_text_in_content() {
        let data = build_test_docx(TRACK_CHANGES_XML);
        let extractor = DocxExtractor::new();
        let config = ExtractionConfig::default();
        let internal_doc = extractor
            .extract_content(
                &data,
                "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
                &config,
            )
            .await
            .unwrap();
        let result = crate::extraction::derive::derive_extraction_result(
            internal_doc,
            false,
            crate::core::config::OutputFormat::Plain,
        );

        assert!(
            result.content.contains("inserted content"),
            "inserted text must appear in accepted-changes content: {}",
            result.content
        );
        assert!(
            !result.content.contains("deleted text"),
            "deleted text must not appear in accepted-changes content: {}",
            result.content
        );
    }

    #[tokio::test]
    async fn should_capture_insertion_delta_text_in_revision() {
        use crate::types::revisions::{DiffLine, RevisionKind};

        let data = build_test_docx(TRACK_CHANGES_XML);
        let extractor = DocxExtractor::new();
        let config = ExtractionConfig::default();
        let internal_doc = extractor
            .extract_content(
                &data,
                "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
                &config,
            )
            .await
            .unwrap();
        let result = crate::extraction::derive::derive_extraction_result(
            internal_doc,
            false,
            crate::core::config::OutputFormat::Plain,
        );

        let revisions = result.revisions.unwrap();
        let ins = revisions.iter().find(|r| r.kind == RevisionKind::Insertion).unwrap();
        assert!(
            ins.delta
                .content
                .iter()
                .any(|l| matches!(l, DiffLine::Added(t) if t == "inserted content")),
            "insertion delta should contain Added(\"inserted content\")"
        );
    }

    #[tokio::test]
    async fn should_capture_deletion_delta_text_in_revision() {
        use crate::types::revisions::DiffLine;

        let data = build_test_docx(TRACK_CHANGES_XML);
        let extractor = DocxExtractor::new();
        let config = ExtractionConfig::default();
        let internal_doc = extractor
            .extract_content(
                &data,
                "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
                &config,
            )
            .await
            .unwrap();
        let result = crate::extraction::derive::derive_extraction_result(
            internal_doc,
            false,
            crate::core::config::OutputFormat::Plain,
        );

        let revisions = result.revisions.unwrap();
        let del = revisions
            .iter()
            .find(|r| r.kind == crate::types::revisions::RevisionKind::Deletion)
            .unwrap();
        assert!(
            del.delta
                .content
                .iter()
                .any(|l| matches!(l, DiffLine::Removed(t) if t == "deleted text")),
            "deletion delta should contain Removed(\"deleted text\")"
        );
    }

    #[tokio::test]
    async fn should_assign_paragraph_anchor_indices_to_revisions() {
        use crate::types::revisions::{RevisionAnchor, RevisionKind};

        let data = build_test_docx(TRACK_CHANGES_XML);
        let extractor = DocxExtractor::new();
        let config = ExtractionConfig::default();
        let internal_doc = extractor
            .extract_content(
                &data,
                "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
                &config,
            )
            .await
            .unwrap();
        let result = crate::extraction::derive::derive_extraction_result(
            internal_doc,
            false,
            crate::core::config::OutputFormat::Plain,
        );

        let revisions = result.revisions.unwrap();

        let ins = revisions.iter().find(|r| r.kind == RevisionKind::Insertion).unwrap();
        assert!(
            matches!(ins.anchor, Some(RevisionAnchor::Paragraph { index: 0 })),
            "insertion anchor should be Paragraph {{ index: 0 }}, got {:?}",
            ins.anchor
        );

        let del = revisions.iter().find(|r| r.kind == RevisionKind::Deletion).unwrap();
        assert!(
            matches!(del.anchor, Some(RevisionAnchor::Paragraph { index: 1 })),
            "deletion anchor should be Paragraph {{ index: 1 }}, got {:?}",
            del.anchor
        );

        let fmt = revisions.iter().find(|r| r.kind == RevisionKind::FormatChange).unwrap();
        assert!(
            matches!(fmt.anchor, Some(RevisionAnchor::Paragraph { index: 2 })),
            "format-change anchor should be Paragraph {{ index: 2 }}, got {:?}",
            fmt.anchor
        );
    }

    #[tokio::test]
    async fn should_return_none_revisions_for_document_without_track_changes() {
        let document_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:body>
    <w:p><w:r><w:t>No track changes here.</w:t></w:r></w:p>
  </w:body>
</w:document>"#;

        let data = build_test_docx(document_xml);
        let extractor = DocxExtractor::new();
        let config = ExtractionConfig::default();
        let internal_doc = extractor
            .extract_content(
                &data,
                "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
                &config,
            )
            .await
            .unwrap();
        let result = crate::extraction::derive::derive_extraction_result(
            internal_doc,
            false,
            crate::core::config::OutputFormat::Plain,
        );

        assert!(
            result.revisions.is_none(),
            "revisions should be None for a document without track-changes markup"
        );
    }

    // --- Issue #81: text-box text, drawing alt-text fallback, and drawing dimensions ---

    #[tokio::test]
    async fn test_issue_81_textbox_alt_text_fallback_and_dimensions() {
        let document_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"
            xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing"
            xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"
            xmlns:pic="http://schemas.openxmlformats.org/drawingml/2006/picture"
            xmlns:wps="http://schemas.microsoft.com/office/word/2010/wordprocessingShape"
            xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
  <w:body>
    <w:p><w:r><w:drawing>
      <wp:inline>
        <wp:extent cx="914400" cy="457200"/>
        <wp:docPr id="1" name="My Picture"/>
        <a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/picture">
          <pic:pic><pic:blipFill><a:blip r:embed="rId5"/></pic:blipFill></pic:pic>
        </a:graphicData></a:graphic>
      </wp:inline>
    </w:drawing></w:r></w:p>
    <w:p><w:r><w:drawing>
      <wp:inline>
        <wp:extent cx="100000" cy="100000"/>
        <wp:docPr id="2" name="Text Box 1"/>
        <a:graphic><a:graphicData uri="http://schemas.microsoft.com/office/word/2010/wordprocessingShape">
          <wps:wsp><wps:txbx><w:txbxContent>
            <w:p><w:r><w:t>Textbox message here.</w:t></w:r></w:p>
          </w:txbxContent></wps:txbx></wps:wsp>
        </a:graphicData></a:graphic>
      </wp:inline>
    </w:drawing></w:r></w:p>
  </w:body>
</w:document>"#;

        let rels_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId5" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="media/image1.png"/>
</Relationships>"#;

        let data = build_test_docx_with_parts(document_xml, None, None, None, None, None, Some(rels_xml));
        let extractor = DocxExtractor::new();
        let config = ExtractionConfig {
            images: Some(ImageExtractionConfig {
                extract_images: false,
                inject_placeholders: true,
                ..Default::default()
            }),
            include_document_structure: true,
            ..Default::default()
        };
        let internal_doc = extractor
            .extract_content(
                &data,
                "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
                &config,
            )
            .await
            .expect("extraction should succeed");
        let result = crate::extraction::derive::derive_extraction_result(
            internal_doc,
            true,
            crate::core::config::OutputFormat::Plain,
        );

        let doc = result.document.as_ref().expect("DocumentStructure should be present");

        let image_node = doc
            .nodes
            .iter()
            .find(|n| matches!(&n.content, NodeContent::Image { .. }))
            .expect("Image node should be present");
        match &image_node.content {
            NodeContent::Image { description, .. } => {
                assert_eq!(
                    description.as_deref(),
                    Some("My Picture"),
                    "alt text should fall back to docPr/@name when @descr is absent"
                );
            }
            _ => unreachable!(),
        }
        let attrs = image_node
            .attributes
            .as_ref()
            .expect("image node should carry attributes");
        assert_eq!(attrs.get("width_inches").map(String::as_str), Some("1.00"));
        assert_eq!(attrs.get("height_inches").map(String::as_str), Some("0.50"));

        let has_textbox_paragraph = doc
            .nodes
            .iter()
            .any(|n| matches!(&n.content, NodeContent::Paragraph { text } if text == "Textbox message here."));
        assert!(
            has_textbox_paragraph,
            "w:txbxContent text should be extracted as a paragraph; nodes: {:?}",
            doc.nodes
        );
    }

    #[tokio::test]
    async fn test_issue_81_vml_textbox_fallback_not_duplicated_with_choice() {
        // mc:Choice carries the DrawingML text box; mc:Fallback carries the VML
        // equivalent for older readers. Both must not surface the text twice.
        let document_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"
            xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006"
            xmlns:wps="http://schemas.microsoft.com/office/word/2010/wordprocessingShape"
            xmlns:v="urn:schemas-microsoft-com:vml">
  <w:body>
    <w:p><w:r>
      <mc:AlternateContent>
        <mc:Choice Requires="wps">
          <w:drawing><wps:wsp><wps:txbx><w:txbxContent>
            <w:p><w:r><w:t>Shared text box body.</w:t></w:r></w:p>
          </w:txbxContent></wps:txbx></wps:wsp></w:drawing>
        </mc:Choice>
        <mc:Fallback>
          <w:pict><v:shape><v:textbox><w:txbxContent>
            <w:p><w:r><w:t>Shared text box body.</w:t></w:r></w:p>
          </w:txbxContent></v:textbox></v:shape></w:pict>
        </mc:Fallback>
      </mc:AlternateContent>
    </w:r></w:p>
    <w:p><w:r>
      <w:pict><v:shape><v:textbox><w:txbxContent>
        <w:p><w:r><w:t>Standalone VML text box.</w:t></w:r></w:p>
      </w:txbxContent></v:textbox></v:shape></w:pict>
    </w:r></w:p>
  </w:body>
</w:document>"#;

        let data = build_test_docx(document_xml);
        let extractor = DocxExtractor::new();
        let config = ExtractionConfig::default();
        let internal_doc = extractor
            .extract_content(
                &data,
                "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
                &config,
            )
            .await
            .expect("extraction should succeed");
        let result = crate::extraction::derive::derive_extraction_result(
            internal_doc,
            false,
            crate::core::config::OutputFormat::Plain,
        );

        let occurrences = result.content.matches("Shared text box body.").count();
        assert_eq!(
            occurrences, 1,
            "mc:Choice and mc:Fallback must not both surface the same text box text; content: {}",
            result.content
        );
        assert!(
            result.content.contains("Standalone VML text box."),
            "a bare (non-AlternateContent) VML text box should still be extracted; content: {}",
            result.content
        );
    }

    // --- Issue #82: DOCX comments ---

    #[tokio::test]
    async fn test_issue_82_comment_extracted_and_joined_to_reference() {
        let document_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:body>
    <w:p>
      <w:commentRangeStart w:id="0"/>
      <w:r><w:t>flagged text</w:t></w:r>
      <w:commentRangeEnd w:id="0"/>
      <w:r><w:commentReference w:id="0"/></w:r>
    </w:p>
  </w:body>
</w:document>"#;
        let comments_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<w:comments xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:comment w:id="0" w:author="Alice"><w:p><w:r><w:t>This needs revision.</w:t></w:r></w:p></w:comment>
</w:comments>"#;

        let data = build_test_docx_with_files(document_xml, &[("word/comments.xml", comments_xml)]);
        let extractor = DocxExtractor::new();
        let config = ExtractionConfig {
            include_document_structure: true,
            ..Default::default()
        };
        let internal_doc = extractor
            .extract_content(
                &data,
                "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
                &config,
            )
            .await
            .expect("extraction should succeed");
        assert!(
            internal_doc.processing_warnings.is_empty(),
            "a resolvable comment reference should not produce a warning: {:?}",
            internal_doc.processing_warnings
        );
        let result = crate::extraction::derive::derive_extraction_result(
            internal_doc,
            true,
            crate::core::config::OutputFormat::Plain,
        );

        assert!(
            result.content.contains("flagged text"),
            "body text should still be present: {}",
            result.content
        );

        let doc = result.document.as_ref().expect("DocumentStructure should be present");
        let has_comment_definition = doc
            .nodes
            .iter()
            .any(|n| matches!(&n.content, NodeContent::Comment { text } if text.contains("This needs revision.")));
        assert!(
            has_comment_definition,
            "comment body should be joined to the reference; nodes: {:?}",
            doc.nodes
        );
    }

    /// Regression for #300: a DOCX reviewer comment must produce
    /// `NodeContent::Comment`, not `NodeContent::Footnote` — the two share the same
    /// marker/definition machinery internally, but a consumer needs to be able to
    /// tell them apart. This also proves the fix does not over-fire: a real
    /// footnote in the same document must still surface as `NodeContent::Footnote`.
    #[tokio::test]
    async fn test_issue_300_docx_comment_produces_comment_not_footnote_node() {
        let document_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:body>
    <w:p>
      <w:commentRangeStart w:id="0"/>
      <w:r><w:t>flagged text</w:t></w:r>
      <w:commentRangeEnd w:id="0"/>
      <w:r><w:commentReference w:id="0"/></w:r>
    </w:p>
    <w:p><w:r><w:t>See note</w:t></w:r><w:r><w:footnoteReference w:id="2"/></w:r></w:p>
  </w:body>
</w:document>"#;
        let comments_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<w:comments xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:comment w:id="0" w:author="Alice"><w:p><w:r><w:t>This needs revision.</w:t></w:r></w:p></w:comment>
</w:comments>"#;
        let footnotes_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<w:footnotes xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:footnote w:id="0"><w:p><w:r><w:t>separator</w:t></w:r></w:p></w:footnote>
  <w:footnote w:id="1"><w:p><w:r><w:t>continuation</w:t></w:r></w:p></w:footnote>
  <w:footnote w:id="2"><w:p><w:r><w:t>This is a real footnote.</w:t></w:r></w:p></w:footnote>
</w:footnotes>"#;

        let data = build_test_docx_with_files(
            document_xml,
            &[
                ("word/comments.xml", comments_xml),
                ("word/footnotes.xml", footnotes_xml),
            ],
        );
        let extractor = DocxExtractor::new();
        let config = ExtractionConfig {
            include_document_structure: true,
            ..Default::default()
        };
        let internal_doc = extractor
            .extract_content(
                &data,
                "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
                &config,
            )
            .await
            .expect("extraction should succeed");
        let result = crate::extraction::derive::derive_extraction_result(
            internal_doc,
            true,
            crate::core::config::OutputFormat::Plain,
        );

        let doc = result.document.as_ref().expect("DocumentStructure should be present");

        let comment_node = doc
            .nodes
            .iter()
            .find(|n| matches!(&n.content, NodeContent::Comment { text } if text.contains("This needs revision.")));
        assert_eq!(
            comment_node.map(|n| &n.content),
            Some(&NodeContent::Comment {
                text: "This needs revision.".to_string()
            }),
            "a DOCX reviewer comment must produce NodeContent::Comment; nodes: {:?}",
            doc.nodes
        );

        let footnote_node = doc.nodes.iter().find(
            |n| matches!(&n.content, NodeContent::Footnote { text } if text.contains("This is a real footnote.")),
        );
        assert_eq!(
            footnote_node.map(|n| &n.content),
            Some(&NodeContent::Footnote {
                text: "This is a real footnote.".to_string()
            }),
            "a real footnote must still produce NodeContent::Footnote (no over-fire); nodes: {:?}",
            doc.nodes
        );

        assert!(
            !doc.nodes
                .iter()
                .any(|n| matches!(&n.content, NodeContent::Footnote { text } if text.contains("This needs revision."))),
            "the comment body must not also surface as a Footnote node; nodes: {:?}",
            doc.nodes
        );
    }

    #[tokio::test]
    async fn test_issue_82_dangling_comment_reference_warns() {
        let document_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:body>
    <w:p><w:r><w:t>text</w:t></w:r><w:r><w:commentReference w:id="7"/></w:r></w:p>
  </w:body>
</w:document>"#;

        let data = build_test_docx(document_xml);
        let extractor = DocxExtractor::new();
        let config = ExtractionConfig::default();
        let internal_doc = extractor
            .extract_content(
                &data,
                "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
                &config,
            )
            .await
            .expect("extraction should succeed even with a dangling comment reference");

        assert!(
            internal_doc
                .processing_warnings
                .iter()
                .any(|w| w.source == "docx" && w.message.contains('7')),
            "a comment reference with no matching comments.xml entry should warn: {:?}",
            internal_doc.processing_warnings
        );
    }

    // --- Issue #83: headers/footers beyond the old hardcoded 1..=3 range ---

    #[tokio::test]
    async fn test_issue_83_fourth_header_and_footer_discovered_via_relationships() {
        let document_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:body><w:p><w:r><w:t>Body content.</w:t></w:r></w:p></w:body>
</w:document>"#;

        let rels_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/header" Target="header1.xml"/>
  <Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/header" Target="header2.xml"/>
  <Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/header" Target="header3.xml"/>
  <Relationship Id="rId4" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/header" Target="header4.xml"/>
  <Relationship Id="rId5" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/footer" Target="footer4.xml"/>
</Relationships>"#;

        fn hdr(text: &str) -> String {
            format!(
                r#"<?xml version="1.0" encoding="UTF-8"?><w:hdr xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:p><w:r><w:t>{}</w:t></w:r></w:p></w:hdr>"#,
                text
            )
        }
        fn ftr(text: &str) -> String {
            format!(
                r#"<?xml version="1.0" encoding="UTF-8"?><w:ftr xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:p><w:r><w:t>{}</w:t></w:r></w:p></w:ftr>"#,
                text
            )
        }

        let h1 = hdr("Header 1 text");
        let h2 = hdr("Header 2 text");
        let h3 = hdr("Header 3 text");
        let h4 = hdr("Header 4 text");
        let f4 = ftr("Footer 4 text");

        let data = build_test_docx_with_files(
            document_xml,
            &[
                ("word/_rels/document.xml.rels", rels_xml),
                ("word/header1.xml", &h1),
                ("word/header2.xml", &h2),
                ("word/header3.xml", &h3),
                ("word/header4.xml", &h4),
                ("word/footer4.xml", &f4),
            ],
        );
        let extractor = DocxExtractor::new();
        let config = ExtractionConfig {
            include_document_structure: true,
            ..Default::default()
        };
        let internal_doc = extractor
            .extract_content(
                &data,
                "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
                &config,
            )
            .await
            .expect("extraction should succeed");
        let result = crate::extraction::derive::derive_extraction_result(
            internal_doc,
            true,
            crate::core::config::OutputFormat::Plain,
        );
        let doc = result.document.as_ref().expect("DocumentStructure should be present");

        for expected in ["Header 1 text", "Header 2 text", "Header 3 text", "Header 4 text"] {
            assert!(
                doc.nodes.iter().any(|n| {
                    n.content_layer == crate::types::ContentLayer::Header
                        && matches!(&n.content, NodeContent::Paragraph { text } if text.contains(expected))
                }),
                "missing header layer node for {:?}; nodes: {:?}",
                expected,
                doc.nodes
            );
        }
        assert!(
            doc.nodes.iter().any(|n| {
                n.content_layer == crate::types::ContentLayer::Footer
                    && matches!(&n.content, NodeContent::Paragraph { text } if text.contains("Footer 4 text"))
            }),
            "missing footer layer node for the 4th footer; nodes: {:?}",
            doc.nodes
        );
    }

    // --- Issue #85: headers/footers/notes converge on the shared body element loop ---

    #[tokio::test]
    async fn test_issue_85_header_table_extracted_via_shared_loop() {
        let document_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:body><w:p><w:r><w:t>Body.</w:t></w:r></w:p></w:body>
</w:document>"#;
        let header_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<w:hdr xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:tbl><w:tr><w:tc><w:p><w:r><w:t>Cell A</w:t></w:r></w:p></w:tc></w:tr></w:tbl>
</w:hdr>"#;

        let data = build_test_docx_with_parts(document_xml, None, None, None, Some(header_xml), None, None);
        let extractor = DocxExtractor::new();
        let config = ExtractionConfig {
            include_document_structure: true,
            ..Default::default()
        };
        let internal_doc = extractor
            .extract_content(
                &data,
                "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
                &config,
            )
            .await
            .expect("extraction should succeed");
        let result = crate::extraction::derive::derive_extraction_result(
            internal_doc,
            true,
            crate::core::config::OutputFormat::Plain,
        );
        let doc = result.document.as_ref().expect("DocumentStructure should be present");

        let header_table = doc.nodes.iter().find(|n| {
            n.content_layer == crate::types::ContentLayer::Header && matches!(&n.content, NodeContent::Table { .. })
        });
        assert!(
            header_table.is_some(),
            "a table inside a header must now be extracted (was previously dropped entirely); nodes: {:?}",
            doc.nodes
        );
        if let Some(NodeContent::Table { grid }) = header_table.map(|n| &n.content) {
            assert_eq!(grid.cells.first().map(|c| c.content.as_str()), Some("Cell A"));
        }
    }

    #[tokio::test]
    async fn test_issue_85_footnote_table_flattened_via_shared_loop() {
        let document_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:body>
    <w:p><w:r><w:t>Text with note</w:t></w:r><w:r><w:footnoteReference w:id="2"/></w:r></w:p>
  </w:body>
</w:document>"#;
        let footnotes_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<w:footnotes xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:footnote w:id="0"><w:p><w:r><w:t>separator</w:t></w:r></w:p></w:footnote>
  <w:footnote w:id="1"><w:p><w:r><w:t>continuation</w:t></w:r></w:p></w:footnote>
  <w:footnote w:id="2">
    <w:tbl><w:tr><w:tc><w:p><w:r><w:t>Note cell text</w:t></w:r></w:p></w:tc></w:tr></w:tbl>
  </w:footnote>
</w:footnotes>"#;

        let data = build_test_docx_with_parts(document_xml, None, Some(footnotes_xml), None, None, None, None);
        let extractor = DocxExtractor::new();
        let config = ExtractionConfig {
            include_document_structure: true,
            ..Default::default()
        };
        let internal_doc = extractor
            .extract_content(
                &data,
                "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
                &config,
            )
            .await
            .expect("extraction should succeed");
        let result = crate::extraction::derive::derive_extraction_result(
            internal_doc,
            true,
            crate::core::config::OutputFormat::Plain,
        );
        let doc = result.document.as_ref().expect("DocumentStructure should be present");

        assert!(
            doc.nodes
                .iter()
                .any(|n| { matches!(&n.content, NodeContent::Footnote { text } if text.contains("Note cell text")) }),
            "a table inside a footnote must be flattened into its text (was previously dropped entirely); nodes: {:?}",
            doc.nodes
        );
    }

    // --- Issues #88 / #239: field-code hyperlinks and general field parsing ---

    #[tokio::test]
    async fn test_issue_88_fldchar_hyperlink_url_recovered() {
        let document_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:body>
    <w:p>
      <w:r><w:fldChar w:fldCharType="begin"/></w:r>
      <w:r><w:instrText xml:space="preserve"> HYPERLINK "https://example.com/page" </w:instrText></w:r>
      <w:r><w:fldChar w:fldCharType="separate"/></w:r>
      <w:r><w:t>Example Link</w:t></w:r>
      <w:r><w:fldChar w:fldCharType="end"/></w:r>
    </w:p>
  </w:body>
</w:document>"#;

        let data = build_test_docx(document_xml);
        let extractor = DocxExtractor::new();
        let config = ExtractionConfig {
            include_document_structure: true,
            ..Default::default()
        };
        let internal_doc = extractor
            .extract_content(
                &data,
                "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
                &config,
            )
            .await
            .expect("extraction should succeed");
        let result = crate::extraction::derive::derive_extraction_result(
            internal_doc,
            true,
            crate::core::config::OutputFormat::Plain,
        );

        assert!(
            result.content.contains("Example Link"),
            "visible result text should be kept: {}",
            result.content
        );
        assert!(
            !result.content.contains("HYPERLINK"),
            "field instruction text must not leak into output: {}",
            result.content
        );

        let doc = result.document.as_ref().expect("DocumentStructure should be present");
        let has_url_annotation = doc.nodes.iter().any(|n| {
            n.annotations.iter().any(|a| {
                matches!(&a.kind, crate::types::document_structure::AnnotationKind::Link { url, .. } if url == "https://example.com/page")
            })
        });
        assert!(
            has_url_annotation,
            "the HYPERLINK field's URL should be recovered onto the result run; nodes: {:?}",
            doc.nodes
        );
    }

    #[tokio::test]
    async fn test_issue_239_fldsimple_hyperlink_and_generic_field() {
        let document_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:body>
    <w:p><w:fldSimple w:instr="HYPERLINK &quot;https://example.org/simple&quot;">
      <w:r><w:t>Simple Link</w:t></w:r>
    </w:fldSimple></w:p>
    <w:p><w:fldSimple w:instr="PAGE">
      <w:r><w:t>1</w:t></w:r>
    </w:fldSimple></w:p>
  </w:body>
</w:document>"#;

        let data = build_test_docx(document_xml);
        let extractor = DocxExtractor::new();
        let config = ExtractionConfig {
            include_document_structure: true,
            ..Default::default()
        };
        let internal_doc = extractor
            .extract_content(
                &data,
                "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
                &config,
            )
            .await
            .expect("extraction of w:fldSimple fields should not crash");
        let result = crate::extraction::derive::derive_extraction_result(
            internal_doc,
            true,
            crate::core::config::OutputFormat::Plain,
        );

        assert_eq!(
            result.content.matches('1').count(),
            1,
            "the PAGE field's cached result text must appear exactly once, not be duplicated: {}",
            result.content
        );
        assert!(
            result.content.contains("Simple Link"),
            "the HYPERLINK fldSimple's visible text should be kept: {}",
            result.content
        );

        let doc = result.document.as_ref().expect("DocumentStructure should be present");
        let has_url_annotation = doc.nodes.iter().any(|n| {
            n.annotations.iter().any(|a| {
                matches!(&a.kind, crate::types::document_structure::AnnotationKind::Link { url, .. } if url == "https://example.org/simple")
            })
        });
        assert!(
            has_url_annotation,
            "the fldSimple HYPERLINK's URL should be recovered; nodes: {:?}",
            doc.nodes
        );
    }

    // --- Issue #224: w:sym, w:noBreakHyphen, and w:br (column/textWrapping) ---

    #[tokio::test]
    async fn test_issue_224_symbol_and_nobreakhyphen_and_column_break() {
        let document_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:body>
    <w:p>
      <w:r><w:t>Value:</w:t></w:r>
      <w:r><w:sym w:font="Wingdings" w:char="F0E0"/></w:r>
      <w:r><w:noBreakHyphen/></w:r>
      <w:r><w:t>after</w:t></w:r>
    </w:p>
    <w:p>
      <w:r><w:t>Col1</w:t></w:r>
      <w:r><w:br w:type="column"/></w:r>
      <w:r><w:t>Col2</w:t></w:r>
    </w:p>
  </w:body>
</w:document>"#;

        let data = build_test_docx(document_xml);
        let extractor = DocxExtractor::new();
        let config = ExtractionConfig::default();
        let internal_doc = extractor
            .extract_content(
                &data,
                "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
                &config,
            )
            .await
            .expect("extraction should succeed");
        assert!(
            internal_doc.processing_warnings.is_empty(),
            "a well-formed w:sym char code should not produce a warning: {:?}",
            internal_doc.processing_warnings
        );
        let result = crate::extraction::derive::derive_extraction_result(
            internal_doc,
            false,
            crate::core::config::OutputFormat::Plain,
        );

        let expected = "Value:\u{F0E0}\u{2011}after";
        assert!(
            result.content.contains(expected),
            "w:sym should map to its Unicode scalar and w:noBreakHyphen to U+2011: {}",
            result.content
        );
        assert!(
            result.content.contains("Col1\nCol2"),
            "a non-page w:br (column/textWrapping) should insert a newline: {}",
            result.content
        );
    }

    #[tokio::test]
    async fn test_issue_224_unmappable_symbol_warns_and_inserts_placeholder() {
        let document_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:body>
    <w:p><w:r><w:sym w:font="Wingdings" w:char="ZZZZ"/></w:r></w:p>
  </w:body>
</w:document>"#;

        let data = build_test_docx(document_xml);
        let extractor = DocxExtractor::new();
        let config = ExtractionConfig::default();
        let internal_doc = extractor
            .extract_content(
                &data,
                "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
                &config,
            )
            .await
            .expect("an unmappable w:sym must degrade gracefully, not fail extraction");

        assert!(
            internal_doc
                .processing_warnings
                .iter()
                .any(|w| w.source == "docx" && w.message.contains("ZZZZ")),
            "an unmappable w:sym char code should produce a ProcessingWarning: {:?}",
            internal_doc.processing_warnings
        );
    }
}
