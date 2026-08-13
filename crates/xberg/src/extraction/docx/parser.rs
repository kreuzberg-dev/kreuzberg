//! Inline DOCX XML parser.
//!
//! Vendored and adapted from [docx-lite](https://github.com/v-lawyer/docx-lite) v0.2.0
//! (MIT OR Apache-2.0, V-Lawyer Team). See ATTRIBUTIONS.md for details.
//!
//! Changes from upstream:
//! - `Paragraph::to_text()` joins runs with `" "` instead of `""` (fixes #359)
//! - Adapted to use xberg's existing `quick-xml` and `zip` versions
//! - Removed file-path based APIs (we only need bytes/reader)
//! - Added markdown rendering and formatting support (fixes #376)

use crate::extractors::security::{SecurityBudget, SecurityError};
use ahash::AHashMap;
use serde::{Deserialize, Serialize};
use std::io::{Cursor, Read, Seek};

use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};

/// Tracks document element ordering (paragraphs, tables, and drawings interleaved).
#[cfg_attr(alef, alef(skip))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) enum DocumentElement {
    Paragraph(usize),
    Table(usize),
    Drawing(usize),
    PageBreak,
}

#[cfg_attr(alef, alef(skip))]
#[derive(Debug, Clone, Default)]
pub(crate) struct Document {
    pub paragraphs: Vec<Paragraph>,
    pub tables: Vec<Table>,
    pub headers: Vec<HeaderFooter>,
    pub footers: Vec<HeaderFooter>,
    pub footnotes: Vec<Note>,
    pub endnotes: Vec<Note>,
    pub numbering_defs: AHashMap<(i64, i64), ListType>,
    /// Document elements in their original order.
    pub elements: Vec<DocumentElement>,
    /// Parsed style catalog from `word/styles.xml`, if available.
    pub style_catalog: Option<super::styles::StyleCatalog>,
    /// Parsed theme from `word/theme/theme1.xml`, if available.
    pub theme: Option<super::theme::Theme>,
    /// Section properties parsed from `w:sectPr` elements.
    pub sections: Vec<super::section::SectionProperties>,
    /// Drawing objects parsed from `w:drawing` elements.
    pub drawings: Vec<super::drawing::Drawing>,
    /// Image relationships (rId → target path) for image extraction.
    pub image_relationships: AHashMap<String, String>,
    /// Track-changes revisions captured from `w:ins`, `w:del`, and `w:rPrChange` elements.
    pub revisions: Vec<crate::types::revisions::DocumentRevision>,
    /// Comments parsed from `word/comments.xml`, joined to the body via
    /// `w:commentRangeStart`/`w:commentRangeEnd` and `w:commentReference` (#82).
    pub comments: Vec<Comment>,
    /// Non-fatal degradation warnings accumulated while parsing (#171).
    pub warnings: Vec<crate::types::ProcessingWarning>,
}

/// A DOCX paragraph containing formatted text runs.
#[cfg_attr(alef, alef(skip))]
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Paragraph {
    /// Formatted text runs that make up this paragraph.
    pub runs: Vec<Run>,
    /// Style ID applied to this paragraph (e.g. `"Heading1"`, `"Normal"`).
    pub style: Option<String>,
    /// Numbering definition ID for bulleted or numbered lists.
    pub numbering_id: Option<i64>,
    /// Indentation level within the numbering definition (0-based).
    pub numbering_level: Option<i64>,
}

/// A formatted text run within a DOCX paragraph.
#[cfg_attr(alef, alef(skip))]
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Run {
    /// Plain text content of this run.
    pub text: String,
    /// Bold formatting flag.
    pub bold: bool,
    /// Italic formatting flag.
    pub italic: bool,
    /// Underline formatting flag.
    pub underline: bool,
    /// Strikethrough formatting flag.
    pub strikethrough: bool,
    /// Subscript vertical alignment flag.
    pub subscript: bool,
    /// Superscript vertical alignment flag.
    pub superscript: bool,
    /// Font size in half-points (from `w:sz`).
    pub font_size: Option<u32>,
    /// Font color as "RRGGBB" hex (from `w:color`).
    pub font_color: Option<String>,
    /// Highlight color name (from `w:highlight`).
    pub highlight: Option<String>,
    /// Hyperlink URL, if this run is wrapped in a `<w:hyperlink>`.
    pub hyperlink_url: Option<String>,
    /// LaTeX math content: (latex_source, is_display_math).
    /// When set, this run represents an equation and `text` is ignored.
    pub math_latex: Option<(String, bool)>,
}

/// A DOCX table parsed from `<w:tbl>`.
#[cfg_attr(alef, alef(skip))]
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Table {
    /// Ordered list of table rows.
    pub rows: Vec<TableRow>,
    /// Table-level properties from `<w:tblPr>`.
    pub properties: Option<super::table::TableProperties>,
    /// Column width definitions from `<w:tblGrid>`.
    pub grid: Option<super::table::TableGrid>,
}

/// A single row within a DOCX table.
#[cfg_attr(alef, alef(skip))]
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TableRow {
    /// Cells in this row.
    pub cells: Vec<TableCell>,
    /// Row-level properties from `<w:trPr>`.
    pub properties: Option<super::table::RowProperties>,
}

/// A single cell within a DOCX table row.
#[cfg_attr(alef, alef(skip))]
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TableCell {
    /// Paragraphs contained in this cell.
    pub paragraphs: Vec<Paragraph>,
    /// Cell-level properties from `<w:tcPr>`.
    pub properties: Option<super::table::CellProperties>,
}

#[cfg_attr(alef, alef(skip))]
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum ListType {
    Bullet,
    Numbered,
}

/// A header or footer block from a DOCX section.
#[cfg_attr(alef, alef(skip))]
#[derive(Debug, Clone, Default)]
pub struct HeaderFooter {
    /// Paragraphs in this header or footer.
    pub paragraphs: Vec<Paragraph>,
    /// Tables in this header or footer.
    pub tables: Vec<Table>,
    /// Which pages this header/footer applies to.
    pub header_type: HeaderFooterType,
}

/// Specifies which pages a header or footer applies to.
#[cfg_attr(alef, alef(skip))]
#[derive(Debug, Clone, Default, PartialEq)]
pub enum HeaderFooterType {
    /// Default header/footer (applies to all pages not covered by First or Even).
    #[default]
    Default,
    /// First-page header/footer.
    First,
    /// Even-page header/footer.
    Even,
    /// Odd-page header/footer.
    Odd,
}

/// A footnote or endnote from a DOCX document.
#[cfg_attr(alef, alef(skip))]
#[derive(Debug, Clone)]
pub struct Note {
    /// Note identifier matching the in-text reference mark.
    pub id: String,
    /// Whether this is a footnote or an endnote.
    pub note_type: NoteType,
    /// Paragraphs of note content.
    pub paragraphs: Vec<Paragraph>,
}

/// Distinguishes footnotes from endnotes in DOCX documents.
#[cfg_attr(alef, alef(skip))]
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum NoteType {
    /// Note appears at the bottom of the page.
    Footnote,
    /// Note appears at the end of the document.
    Endnote,
}

/// A comment from `word/comments.xml` (#82).
///
/// Joined to the body text via `w:commentRangeStart`/`w:commentRangeEnd` (anchors,
/// not carrying content) and `w:commentReference` (the marker matched to this `id`).
#[cfg_attr(alef, alef(skip))]
#[derive(Debug, Clone, Default)]
pub struct Comment {
    /// Comment identifier matching `w:commentReference/@w:id`.
    pub id: String,
    /// Comment author, if present.
    pub author: Option<String>,
    /// Paragraphs of comment content.
    pub paragraphs: Vec<Paragraph>,
}

/// Check if a formatting element is enabled (not explicitly set to false/0/none).
fn is_format_enabled(e: &BytesStart) -> bool {
    for attr in e.attributes().flatten() {
        if attr.key.as_ref() == b"w:val"
            && let Ok(val) = std::str::from_utf8(&attr.value)
        {
            return !matches!(val, "false" | "0" | "none");
        }
    }
    true
}

/// Read `w:val` attribute as i64.
fn get_val_attr(e: &BytesStart) -> Option<i64> {
    for attr in e.attributes().flatten() {
        if attr.key.as_ref() == b"w:val"
            && let Ok(val) = std::str::from_utf8(&attr.value)
        {
            return val.parse().ok();
        }
    }
    None
}

/// Read `w:val` attribute as String.
fn get_val_attr_string(e: &BytesStart) -> Option<String> {
    for attr in e.attributes().flatten() {
        if attr.key.as_ref() == b"w:val"
            && let Ok(val) = std::str::from_utf8(&attr.value)
        {
            return Some(val.to_string());
        }
    }
    None
}

/// Extract the target URL from a `HYPERLINK` field instruction (#88, #239).
///
/// Matches instruction text such as `HYPERLINK "https://example.com"` or
/// `HYPERLINK "https://example.com" \o "tooltip"` (from a `w:fldSimple/@w:instr`
/// attribute or accumulated `w:instrText` run text), returning the first quoted
/// argument. Returns `None` for other field types (PAGE, REF, TOC, ...) or
/// malformed instructions.
fn extract_hyperlink_field_url(instr: &str) -> Option<String> {
    let trimmed = instr.trim();
    let head = trimmed.get(..9)?;
    if !head.eq_ignore_ascii_case("HYPERLINK") {
        return None;
    }
    let rest = trimmed.get(9..)?.trim_start();
    let rest = rest.strip_prefix('"')?;
    let end = rest.find('"')?;
    let url = rest[..end].trim();
    if url.is_empty() { None } else { Some(url.to_string()) }
}

/// Collect the standard revision-mark attributes from a `w:ins`, `w:del`, or
/// `w:rPrChange` start element: `w:id`, `w:author`, and `w:date`.
///
/// Returns `(id_opt, author_opt, date_opt)` where `id_opt` is the raw string
/// value of the `w:id` attribute (a numeric string like `"42"`).
fn collect_revision_attrs(e: &BytesStart) -> (Option<String>, Option<String>, Option<String>) {
    let mut id: Option<String> = None;
    let mut author: Option<String> = None;
    let mut date: Option<String> = None;
    for attr in e.attributes().flatten() {
        match attr.key.as_ref() {
            b"w:id" => {
                id = std::str::from_utf8(&attr.value).ok().map(String::from);
            }
            b"w:author" => {
                author = std::str::from_utf8(&attr.value).ok().map(String::from);
                if author.as_deref() == Some("") {
                    author = None;
                }
            }
            b"w:date" => {
                date = std::str::from_utf8(&attr.value).ok().map(String::from);
                if date.as_deref() == Some("") {
                    date = None;
                }
            }
            _ => {}
        }
    }
    (id, author, date)
}

/// Map heading style name to markdown heading level (fallback for docs without styles.xml).
fn heading_level_from_style_name(style: &str) -> Option<u8> {
    match style {
        "Title" => Some(1),
        s if s.starts_with("Heading") || s.starts_with("heading") => {
            let num_part = s.trim_start_matches("Heading").trim_start_matches("heading");
            if let Ok(n) = num_part.parse::<u8>()
                && (1..=6).contains(&n)
            {
                return Some((n + 1).min(6));
            }
            None
        }
        _ => None,
    }
}

impl Document {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Render the document and return both the text and accurate page boundaries.
    ///
    /// `inject_placeholders` controls whether image reference markers (`![](image_N)`) are
    /// emitted in the markdown text. Passing `false` suppresses them, which is honoured when
    /// `ImageExtractionConfig::inject_placeholders` is set to `false` by the caller.
    pub fn extract_text_with_boundaries(
        &self,
        is_markdown: bool,
        inject_placeholders: bool,
    ) -> (String, Vec<crate::types::PageBoundary>) {
        let text = if is_markdown {
            self.to_markdown(inject_placeholders)
        } else {
            self.to_plain_text()
        };

        let mut boundaries = Vec::new();
        let mut start_idx = 0;
        let mut page_num = 1;

        for (idx, _) in text.match_indices('\x0c') {
            boundaries.push(crate::types::PageBoundary {
                byte_start: start_idx,
                byte_end: idx,
                page_number: page_num,
            });
            start_idx = idx + 1;
            page_num += 1;
        }

        boundaries.push(crate::types::PageBoundary {
            byte_start: start_idx,
            byte_end: text.len(),
            page_number: page_num,
        });

        (text, boundaries)
    }

    /// Return the 1-based page number for each top-level table in the document.
    pub fn table_page_numbers(&self) -> Vec<usize> {
        let mut table_page_numbers = Vec::new();
        let mut current_page = 1;

        for element in &self.elements {
            match element {
                DocumentElement::PageBreak => current_page += 1,
                DocumentElement::Table(_) => {
                    table_page_numbers.push(current_page);
                }
                _ => {}
            }
        }

        table_page_numbers
    }

    /// Internal helper to ensure a blank line before appending new content.
    fn ensure_blank_line(output: &mut String) {
        if !output.is_empty() && !output.ends_with("\n\n") {
            if output.ends_with('\n') {
                output.push('\n');
            } else {
                output.push_str("\n\n");
            }
        }
    }

    /// Resolve heading level for a paragraph style using the StyleCatalog.
    ///
    /// Walks the style inheritance chain to find `outline_level`.
    /// Falls back to string-matching on style name/ID if no StyleCatalog is available.
    /// Returns 1-6 (markdown heading levels).
    pub(crate) fn resolve_heading_level(&self, style_id: &str) -> Option<u8> {
        if let Some(ref catalog) = self.style_catalog {
            let mut current_id = Some(style_id);
            let mut visited = 0;
            while let Some(id) = current_id {
                if visited > 20 {
                    break;
                }
                visited += 1;
                if let Some(style_def) = catalog.styles.get(id) {
                    if let Some(level) = style_def.paragraph_properties.outline_level {
                        return Some((level + 1).min(6));
                    }
                    if let Some(ref name) = style_def.name
                        && (name == "Title" || name == "title")
                    {
                        return Some(1);
                    }
                    current_id = style_def.based_on.as_deref();
                } else {
                    break;
                }
            }
        }
        heading_level_from_style_name(style_id)
    }

    #[cfg(test)]
    pub(crate) fn extract_text(&self) -> String {
        let mut text = String::new();

        for paragraph in &self.paragraphs {
            let para_text = paragraph.to_text();
            if !para_text.is_empty() {
                text.push_str(&para_text);
                text.push('\n');
            }
        }

        for table in &self.tables {
            for row in &table.rows {
                for cell in &row.cells {
                    for paragraph in &cell.paragraphs {
                        let para_text = paragraph.to_text();
                        if !para_text.is_empty() {
                            text.push_str(&para_text);
                            text.push('\t');
                        }
                    }
                }
                text.push('\n');
            }
            text.push('\n');
        }

        text
    }

    /// Render the document as markdown.
    ///
    /// When `inject_placeholders` is `true`, drawings that reference an image
    /// emit `![alt](image)` placeholders. When `false` they are silently
    /// skipped, which is useful when the caller only wants text.
    pub(crate) fn to_markdown(&self, inject_placeholders: bool) -> String {
        use std::fmt::Write;

        let mut output = String::new();
        let mut list_counters: AHashMap<(i64, i64), usize> = AHashMap::new();
        let mut prev_was_list = false;

        if !self.elements.is_empty() {
            for element in &self.elements {
                match element {
                    DocumentElement::Paragraph(idx) => {
                        let Some(paragraph) = self.paragraphs.get(*idx) else {
                            continue;
                        };
                        self.append_paragraph_markdown(paragraph, &mut output, &mut list_counters, &mut prev_was_list);
                    }
                    DocumentElement::Table(idx) => {
                        let Some(table) = self.tables.get(*idx) else { continue };
                        Self::ensure_blank_line(&mut output);
                        if let Some(ref props) = table.properties
                            && let Some(ref caption) = props.caption
                        {
                            output.push_str(caption);
                            output.push_str("\n\n");
                        }
                        output.push_str(&table.to_markdown());
                        prev_was_list = false;
                    }
                    DocumentElement::Drawing(idx) => {
                        let Some(drawing) = self.drawings.get(*idx) else {
                            continue;
                        };
                        if drawing.image_ref.is_none() {
                            continue;
                        }
                        if inject_placeholders {
                            let alt = drawing
                                .doc_properties
                                .as_ref()
                                .and_then(|dp| dp.description.as_deref())
                                .unwrap_or("");
                            Self::ensure_blank_line(&mut output);
                            let _ = writeln!(output, "![{}](image)", alt);
                        }
                        prev_was_list = false;
                    }
                    DocumentElement::PageBreak => {
                        output.push('\x0c');
                        prev_was_list = false;
                    }
                }
            }
        } else {
            for paragraph in &self.paragraphs {
                self.append_paragraph_markdown(paragraph, &mut output, &mut list_counters, &mut prev_was_list);
            }
        }

        if !self.footnotes.is_empty() {
            output.push_str("\n\n");
            for note in &self.footnotes {
                let note_text: String = note
                    .paragraphs
                    .iter()
                    .map(|p| p.runs_to_markdown())
                    .collect::<Vec<_>>()
                    .join(" ");
                if !note_text.is_empty() {
                    let _ = writeln!(output, "[^{}]: {}", note.id, note_text);
                }
            }
        }

        if !self.endnotes.is_empty() {
            output.push_str("\n\n");
            for note in &self.endnotes {
                let note_text: String = note
                    .paragraphs
                    .iter()
                    .map(|p| p.runs_to_markdown())
                    .collect::<Vec<_>>()
                    .join(" ");
                if !note_text.is_empty() {
                    let _ = writeln!(output, "[^{}]: {}", note.id, note_text);
                }
            }
        }

        let trimmed_end = output.trim_end().len();
        output.truncate(trimmed_end);
        let trimmed_start = output.len() - output.trim_start().len();
        if trimmed_start > 0 {
            output.drain(..trimmed_start);
        }
        output
    }

    /// Render the document as plain text (no markdown formatting).
    pub(crate) fn to_plain_text(&self) -> String {
        let mut output = String::new();

        if !self.elements.is_empty() {
            for element in &self.elements {
                match element {
                    DocumentElement::Paragraph(idx) => {
                        let Some(paragraph) = self.paragraphs.get(*idx) else {
                            continue;
                        };
                        let text = paragraph.to_text();
                        if !text.is_empty() {
                            Self::ensure_blank_line(&mut output);
                            output.push_str(&text);
                        }
                    }
                    DocumentElement::Table(idx) => {
                        let Some(table) = self.tables.get(*idx) else { continue };
                        Self::ensure_blank_line(&mut output);
                        if let Some(ref props) = table.properties
                            && let Some(ref caption) = props.caption
                        {
                            output.push_str(caption);
                            output.push_str("\n\n");
                        }
                        output.push_str(&table.to_plain_text());
                    }
                    DocumentElement::Drawing(idx) => {
                        let Some(drawing) = self.drawings.get(*idx) else {
                            continue;
                        };
                        if let Some(alt) = drawing.doc_properties.as_ref().and_then(|dp| dp.description.as_deref())
                            && !alt.is_empty()
                        {
                            Self::ensure_blank_line(&mut output);
                            output.push_str(alt);
                        }
                    }
                    DocumentElement::PageBreak => {
                        output.push('\x0c');
                    }
                }
            }
        } else {
            for paragraph in &self.paragraphs {
                let text = paragraph.to_text();
                if !text.is_empty() {
                    Self::ensure_blank_line(&mut output);
                    output.push_str(&text);
                }
            }
        }

        if !self.footnotes.is_empty() {
            output.push_str("\n\n");
            for note in &self.footnotes {
                let note_text: String = note
                    .paragraphs
                    .iter()
                    .map(|p| p.to_text())
                    .collect::<Vec<_>>()
                    .join(" ");
                if !note_text.is_empty() {
                    output.push_str(&note.id);
                    output.push_str(": ");
                    output.push_str(&note_text);
                    output.push('\n');
                }
            }
        }

        if !self.endnotes.is_empty() {
            output.push_str("\n\n");
            for note in &self.endnotes {
                let note_text: String = note
                    .paragraphs
                    .iter()
                    .map(|p| p.to_text())
                    .collect::<Vec<_>>()
                    .join(" ");
                if !note_text.is_empty() {
                    output.push_str(&note.id);
                    output.push_str(": ");
                    output.push_str(&note_text);
                    output.push('\n');
                }
            }
        }

        let trimmed_end = output.trim_end().len();
        output.truncate(trimmed_end);
        let trimmed_start = output.len() - output.trim_start().len();
        if trimmed_start > 0 {
            output.drain(..trimmed_start);
        }
        output
    }

    /// Helper: append a paragraph's markdown to output, managing list transitions.
    fn append_paragraph_markdown(
        &self,
        paragraph: &Paragraph,
        output: &mut String,
        list_counters: &mut AHashMap<(i64, i64), usize>,
        prev_was_list: &mut bool,
    ) {
        let is_list = paragraph.numbering_id.is_some();

        if is_list && !*prev_was_list {
            Self::ensure_blank_line(output);
        }

        if !is_list && *prev_was_list {
            Self::ensure_blank_line(output);
        }

        let heading_level = paragraph.style.as_deref().and_then(|s| self.resolve_heading_level(s));
        let md = paragraph.to_markdown(&self.numbering_defs, list_counters, heading_level);
        if md.is_empty() {
            *prev_was_list = is_list;
            return;
        }

        let is_quote = paragraph.style.as_deref().is_some_and(|s| {
            let lower = s.to_ascii_lowercase();
            lower == "quote" || lower == "blockquote" || lower.contains("quote")
        });

        if is_list {
            if *prev_was_list {
                output.push('\n');
            }
            output.push_str(&md);
        } else if is_quote {
            Self::ensure_blank_line(output);
            output.push_str("> ");
            output.push_str(&md);
        } else {
            Self::ensure_blank_line(output);
            output.push_str(&md);
        }

        *prev_was_list = is_list;
    }
}

impl Paragraph {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Concatenate text runs to produce paragraph text.
    ///
    /// In DOCX, whitespace between words is stored inside `<w:t>` elements
    /// (e.g. `<w:t>Hello </w:t><w:t>World</w:t>`), so runs are joined
    /// directly without adding extra separators. The parser must use
    /// `trim_text(false)` to preserve this whitespace.
    pub(crate) fn to_text(&self) -> String {
        let mut text = String::new();
        for run in &self.runs {
            if let Some((ref latex, _)) = run.math_latex {
                text.push_str(latex);
            } else {
                text.push_str(&run.text);
            }
        }
        text
    }

    /// Render inline runs as markdown (no paragraph-level wrapping).
    ///
    /// Uses a two-level grouping strategy to avoid spurious marker sequences like `****`:
    /// 1. Groups consecutive runs that share the same bold/italic/hyperlink properties.
    /// 2. Within each group, opens bold/italic once and toggles underline/strikethrough per run.
    pub(crate) fn runs_to_markdown(&self) -> String {
        let mut text = String::new();
        let mut i = 0;
        while i < self.runs.len() {
            let run = &self.runs[i];

            if run.math_latex.is_some() || run.text.is_empty() {
                text.push_str(&run.to_markdown());
                i += 1;
                continue;
            }

            let group_start = i;
            let mut j = i + 1;
            while j < self.runs.len() {
                let next = &self.runs[j];
                if next.math_latex.is_some()
                    || next.text.is_empty()
                    || next.bold != run.bold
                    || next.italic != run.italic
                    || next.hyperlink_url != run.hyperlink_url
                {
                    break;
                }
                j += 1;
            }
            let group_end = j;

            let all_same_inner = self.runs[group_start..group_end]
                .iter()
                .all(|r| r.underline == run.underline && r.strikethrough == run.strikethrough);

            if all_same_inner {
                let mut merged_text = String::new();
                for r in &self.runs[group_start..group_end] {
                    merged_text.push_str(&r.text);
                }
                let merged_run = Run {
                    text: merged_text,
                    bold: run.bold,
                    italic: run.italic,
                    underline: run.underline,
                    strikethrough: run.strikethrough,
                    hyperlink_url: run.hyperlink_url.clone(),
                    ..Default::default()
                };
                text.push_str(&merged_run.to_markdown());
            } else {
                if run.hyperlink_url.is_some() {
                    text.push('[');
                }
                if run.bold && run.italic {
                    text.push_str("***");
                } else if run.bold {
                    text.push_str("**");
                } else if run.italic {
                    text.push('*');
                }

                for r in &self.runs[group_start..group_end] {
                    if r.underline {
                        text.push_str("<u>");
                    }
                    if r.strikethrough {
                        text.push_str("~~");
                    }
                    text.push_str(&r.text);
                    if r.strikethrough {
                        text.push_str("~~");
                    }
                    if r.underline {
                        text.push_str("</u>");
                    }
                }

                if run.bold && run.italic {
                    text.push_str("***");
                } else if run.bold {
                    text.push_str("**");
                } else if run.italic {
                    text.push('*');
                }
                if let Some(ref url) = run.hyperlink_url {
                    text.push_str("](");
                    text.push_str(url);
                    text.push(')');
                }
            }

            i = group_end;
        }
        text
    }

    /// Render as markdown with heading/list context.
    ///
    /// If `heading_level` is provided (resolved via `Document::resolve_heading_level`),
    /// it takes precedence over style name matching.
    pub(crate) fn to_markdown(
        &self,
        numbering_defs: &AHashMap<(i64, i64), ListType>,
        list_counters: &mut AHashMap<(i64, i64), usize>,
        heading_level: Option<u8>,
    ) -> String {
        let inline = self.runs_to_markdown();

        if let Some(level) = heading_level {
            let hashes = "#".repeat(level as usize);
            return format!("{} {}", hashes, inline);
        }

        if let (Some(num_id), Some(level)) = (self.numbering_id, self.numbering_level) {
            let indent = "  ".repeat(level as usize);
            let key = (num_id, level);
            let list_type = numbering_defs.get(&key).copied().unwrap_or(ListType::Bullet);

            match list_type {
                ListType::Bullet => {
                    return format!("{}- {}", indent, inline);
                }
                ListType::Numbered => {
                    let counter = list_counters.entry(key).or_insert(0);
                    *counter += 1;
                    return format!("{}{}. {}", indent, *counter, inline);
                }
            }
        }

        inline
    }

    pub(crate) fn add_run(&mut self, run: Run) {
        self.runs.push(run);
    }
}

impl Run {
    #[cfg(test)]
    pub(crate) fn new(text: String) -> Self {
        Self {
            text,
            ..Default::default()
        }
    }

    /// Render this run as markdown with formatting markers.
    pub(crate) fn to_markdown(&self) -> String {
        if let Some((ref latex, is_display)) = self.math_latex {
            if latex.is_empty() {
                return String::new();
            }
            return if is_display {
                format!("$${}$$", latex)
            } else {
                format!("${}$", latex)
            };
        }

        if self.text.is_empty() {
            return String::new();
        }

        let extra = (if self.bold && self.italic {
            6
        } else if self.bold || self.italic {
            4
        } else {
            0
        }) + (if self.strikethrough { 4 } else { 0 })
            + (if self.underline { 7 } else { 0 })
            + self.hyperlink_url.as_ref().map_or(0, |u| u.len() + 4);
        let mut out = String::with_capacity(self.text.len() + extra);

        if self.hyperlink_url.is_some() {
            out.push('[');
        }
        if self.underline {
            out.push_str("<u>");
        }
        if self.strikethrough {
            out.push_str("~~");
        }
        if self.bold && self.italic {
            out.push_str("***");
        } else if self.bold {
            out.push_str("**");
        } else if self.italic {
            out.push('*');
        }

        out.push_str(&self.text);

        if self.bold && self.italic {
            out.push_str("***");
        } else if self.bold {
            out.push_str("**");
        } else if self.italic {
            out.push('*');
        }
        if self.strikethrough {
            out.push_str("~~");
        }
        if self.underline {
            out.push_str("</u>");
        }
        if let Some(ref url) = self.hyperlink_url {
            out.push_str("](");
            out.push_str(url);
            out.push(')');
        }

        out
    }
}

impl Table {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Render this table as a markdown table.
    ///
    /// Uses table row and cell properties to improve formatting:
    /// - Respects `RowProperties.is_header` to identify header rows
    /// - Handles `CellProperties.grid_span` to account for merged cells
    ///
    /// If no explicit header row is marked, treats the first row as the header.
    pub(crate) fn to_markdown(&self) -> String {
        if self.rows.is_empty() {
            return String::new();
        }

        let mut cells: Vec<Vec<String>> = Vec::new();
        for row in &self.rows {
            let mut row_cells = Vec::new();
            for cell in &row.cells {
                let is_vmerge_continue = cell
                    .properties
                    .as_ref()
                    .is_some_and(|p| matches!(p.v_merge, Some(super::table::VerticalMerge::Continue)));

                let cell_text = if is_vmerge_continue {
                    String::new()
                } else {
                    cell.paragraphs
                        .iter()
                        .map(|para| para.runs_to_markdown())
                        .collect::<Vec<_>>()
                        .join(" ")
                        .trim()
                        .to_string()
                };
                row_cells.push(cell_text);

                let span = cell.properties.as_ref().and_then(|p| p.grid_span).unwrap_or(1);
                for _ in 1..span {
                    row_cells.push(String::new());
                }
            }
            cells.push(row_cells);
        }

        if cells.is_empty() {
            return String::new();
        }

        let num_cols = cells.iter().map(|r| r.len()).max().unwrap_or(0);
        if num_cols == 0 {
            return String::new();
        }

        let mut col_widths = vec![3usize; num_cols];
        for row in &cells {
            for (i, cell) in row.iter().enumerate() {
                col_widths[i] = col_widths[i].max(cell.len());
            }
        }

        let header_row_index = self
            .rows
            .iter()
            .position(|row| row.properties.as_ref().map(|p| p.is_header).unwrap_or(false))
            .unwrap_or(0);

        let mut md = String::new();

        for (row_idx, row) in cells.iter().enumerate() {
            md.push('|');
            for (i, cell) in row.iter().enumerate() {
                let width = col_widths.get(i).copied().unwrap_or(3);
                md.push_str(&format!(" {:width$} |", cell, width = width));
            }
            for i in row.len()..num_cols {
                let width = col_widths.get(i).copied().unwrap_or(3);
                md.push_str(&format!(" {:width$} |", "", width = width));
            }
            md.push('\n');

            if row_idx == header_row_index {
                md.push('|');
                for i in 0..num_cols {
                    let width = col_widths.get(i).copied().unwrap_or(3);
                    md.push_str(&format!(" {} |", "-".repeat(width)));
                }
                md.push('\n');
            }
        }

        md.trim_end().to_string()
    }

    /// Render this table as plain text with tab-separated cells.
    pub(crate) fn to_plain_text(&self) -> String {
        if self.rows.is_empty() {
            return String::new();
        }

        let mut cells: Vec<Vec<String>> = Vec::new();
        for row in &self.rows {
            let mut row_cells = Vec::new();
            for cell in &row.cells {
                let is_vmerge_continue = cell
                    .properties
                    .as_ref()
                    .is_some_and(|p| matches!(p.v_merge, Some(super::table::VerticalMerge::Continue)));

                let cell_text = if is_vmerge_continue {
                    String::new()
                } else {
                    cell.paragraphs
                        .iter()
                        .map(|para| para.to_text())
                        .collect::<Vec<_>>()
                        .join(" ")
                        .trim()
                        .to_string()
                };
                row_cells.push(cell_text);

                let span = cell.properties.as_ref().and_then(|p| p.grid_span).unwrap_or(1);
                for _ in 1..span {
                    row_cells.push(String::new());
                }
            }
            cells.push(row_cells);
        }

        crate::extraction::cells_to_text(&cells)
    }
}

/// Context for tracking nested table parsing state.
///
/// Each level of table nesting gets its own context on the stack,
/// allowing arbitrary nesting depth (e.g. tables within table cells).
struct TableContext {
    table: Table,
    current_row: Option<TableRow>,
    current_cell: Option<TableCell>,
    paragraph: Option<Paragraph>,
}

impl TableContext {
    fn new() -> Self {
        Self {
            table: Table::new(),
            current_row: None,
            current_cell: None,
            paragraph: None,
        }
    }
}

/// Output of [`DocxParser::parse_body_elements`], the element-dispatch loop shared by
/// the main document body, headers/footers, and footnotes/endnotes (#85).
///
/// Callers pick the fields relevant to their content type; e.g. `HeaderFooter` only
/// keeps `paragraphs`/`tables`, while `Note` keeps just `paragraphs` (any top-level
/// table content is flattened into it, matching how a table nested in a table cell
/// is already flattened elsewhere in this parser).
#[derive(Debug, Default)]
struct BodyParseOutputs {
    paragraphs: Vec<Paragraph>,
    tables: Vec<Table>,
    drawings: Vec<super::drawing::Drawing>,
    elements: Vec<DocumentElement>,
    sections: Vec<super::section::SectionProperties>,
    revisions: Vec<crate::types::revisions::DocumentRevision>,
    /// `w:id` values from `w:commentReference` markers encountered in this content,
    /// for validation against `word/comments.xml` (#82).
    comment_ref_ids: Vec<String>,
}

/// Apply run-level formatting from run property child elements.
///
/// Handles `<w:b>`, `<w:i>`, `<w:u>`, `<w:strike>`, `<w:dstrike>`,
/// `<w:vertAlign>`, `<w:sz>`, `<w:color>`, and `<w:highlight>`.
/// Works for both `Event::Start` and `Event::Empty` events.
fn apply_run_formatting(e: &BytesStart, current_run: &mut Option<Run>) {
    if let Some(run) = current_run {
        match e.name().as_ref() as &[u8] {
            b"w:b" => run.bold = is_format_enabled(e),
            b"w:i" => run.italic = is_format_enabled(e),
            b"w:u" => run.underline = is_format_enabled(e),
            b"w:strike" | b"w:dstrike" => run.strikethrough = is_format_enabled(e),
            b"w:vertAlign" => {
                if let Some(val) = get_val_attr_string(e) {
                    match val.as_str() {
                        "subscript" => {
                            run.subscript = true;
                            run.superscript = false;
                        }
                        "superscript" => {
                            run.superscript = true;
                            run.subscript = false;
                        }
                        _ => {
                            run.subscript = false;
                            run.superscript = false;
                        }
                    }
                }
            }
            b"w:sz" => {
                if let Some(val) = get_val_attr(e) {
                    run.font_size = Some(val as u32);
                }
            }
            b"w:color" => {
                if let Some(val) = get_val_attr_string(e)
                    && val != "auto"
                    && val.len() == 6
                    && val.chars().all(|c| c.is_ascii_hexdigit())
                {
                    run.font_color = Some(val);
                }
            }
            b"w:highlight" => {
                if let Some(val) = get_val_attr_string(e) {
                    const VALID_HIGHLIGHTS: &[&str] = &[
                        "yellow",
                        "green",
                        "cyan",
                        "magenta",
                        "blue",
                        "red",
                        "darkBlue",
                        "darkCyan",
                        "darkGreen",
                        "darkMagenta",
                        "darkRed",
                        "darkYellow",
                        "darkGray",
                        "lightGray",
                        "black",
                        "none",
                    ];
                    if VALID_HIGHLIGHTS.contains(&val.as_str()) {
                        run.highlight = Some(val);
                    }
                }
            }
            _ => {}
        }
    }
}

fn collect_run_property_change(e: &BytesStart, changes: &mut Vec<crate::types::revisions::PropertyChange>) {
    let (name, from) = match e.name().as_ref() as &[u8] {
        b"w:b" => ("bold", Some(is_format_enabled(e).to_string())),
        b"w:i" => ("italic", Some(is_format_enabled(e).to_string())),
        b"w:u" => ("underline", Some(is_format_enabled(e).to_string())),
        b"w:strike" | b"w:dstrike" => ("strikethrough", Some(is_format_enabled(e).to_string())),
        b"w:vertAlign" => ("vertical_align", get_val_attr_string(e)),
        b"w:sz" => ("font_size", get_val_attr(e).map(|v| v.to_string())),
        b"w:color" => ("font_color", get_val_attr_string(e)),
        b"w:highlight" => ("highlight", get_val_attr_string(e)),
        _ => return,
    };

    if let Some(change) = changes.iter_mut().find(|change| change.name == name) {
        change.from = from;
        return;
    }

    changes.push(crate::types::revisions::PropertyChange {
        name: name.to_string(),
        from,
        to: None,
    });
}

fn run_property_value(run: &Run, name: &str) -> Option<String> {
    match name {
        "bold" => Some(run.bold.to_string()),
        "italic" => Some(run.italic.to_string()),
        "underline" => Some(run.underline.to_string()),
        "strikethrough" => Some(run.strikethrough.to_string()),
        "vertical_align" => {
            if run.subscript {
                Some("subscript".to_string())
            } else if run.superscript {
                Some("superscript".to_string())
            } else {
                None
            }
        }
        "font_size" => run.font_size.map(|v| v.to_string()),
        "font_color" => run.font_color.clone(),
        "highlight" => run.highlight.clone(),
        _ => None,
    }
}

fn push_current_run_property_changes(run: &Run, changes: &mut Vec<crate::types::revisions::PropertyChange>) {
    let current_properties = [
        ("bold", run.bold.then_some("true".to_string())),
        ("italic", run.italic.then_some("true".to_string())),
        ("underline", run.underline.then_some("true".to_string())),
        ("strikethrough", run.strikethrough.then_some("true".to_string())),
        (
            "vertical_align",
            if run.subscript {
                Some("subscript".to_string())
            } else if run.superscript {
                Some("superscript".to_string())
            } else {
                None
            },
        ),
        ("font_size", run.font_size.map(|v| v.to_string())),
        ("font_color", run.font_color.clone()),
        ("highlight", run.highlight.clone()),
    ];

    for (name, to) in current_properties {
        let Some(to) = to else {
            continue;
        };
        if changes.iter().any(|change| change.name == name) {
            continue;
        }
        changes.push(crate::types::revisions::PropertyChange {
            name: name.to_string(),
            from: None,
            to: Some(to),
        });
    }
}

fn finalize_run_property_changes(
    mut changes: Vec<crate::types::revisions::PropertyChange>,
    current_run: Option<&Run>,
) -> Vec<crate::types::revisions::PropertyChange> {
    if let Some(run) = current_run {
        for change in &mut changes {
            change.to = run_property_value(run, &change.name);
        }
        push_current_run_property_changes(run, &mut changes);
    }

    changes.into_iter().filter(|change| change.from != change.to).collect()
}

fn push_format_revision(
    revisions: &mut Vec<crate::types::revisions::DocumentRevision>,
    attrs: (Option<String>, Option<String>, Option<String>),
    property_changes: Vec<crate::types::revisions::PropertyChange>,
    current_run: Option<&Run>,
    current_paragraph_index: usize,
    revision_id_counter: &mut usize,
) {
    let (id_opt, author, timestamp) = attrs;
    let revision_id = id_opt.unwrap_or_else(|| {
        let fallback = format!("docx-fmt-{}", *revision_id_counter);
        *revision_id_counter += 1;
        fallback
    });

    revisions.push(crate::types::revisions::DocumentRevision {
        revision_id,
        author,
        timestamp,
        kind: crate::types::revisions::RevisionKind::FormatChange,
        anchor: Some(crate::types::revisions::RevisionAnchor::Paragraph {
            index: current_paragraph_index,
        }),
        delta: crate::types::revisions::RevisionDelta {
            property_changes: finalize_run_property_changes(property_changes, current_run),
            ..Default::default()
        },
    });
}

/// Apply paragraph-level properties from a `<w:pStyle>`, `<w:ilvl>`, or `<w:numId>` element.
///
/// Resolves the correct paragraph (table context vs top-level) automatically.
fn apply_paragraph_property(
    e: &BytesStart,
    table_stack: &mut [TableContext],
    current_paragraph: &mut Option<Paragraph>,
) {
    let para = if let Some(ctx) = table_stack.last_mut() {
        ctx.paragraph.as_mut()
    } else {
        current_paragraph.as_mut()
    };

    if let Some(para) = para {
        match e.name().as_ref() as &[u8] {
            b"w:pStyle" => para.style = get_val_attr_string(e),
            b"w:ilvl" => para.numbering_level = get_val_attr(e),
            b"w:numId" => para.numbering_id = get_val_attr(e),
            _ => {}
        }
    }
}

/// Push a run into whichever paragraph is currently open: a table cell's last
/// paragraph, an in-progress table-cell paragraph, or the top-level paragraph.
/// Shared by math-formula runs and by the `</w:r>` end-tag handler.
fn push_run_to_current(table_stack: &mut [TableContext], current_paragraph: &mut Option<Paragraph>, run: Run) {
    if let Some(ctx) = table_stack.last_mut() {
        if let Some(ref mut para) = ctx.paragraph {
            para.add_run(run);
        } else if let Some(ref mut cell) = ctx.current_cell {
            if cell.paragraphs.is_empty() {
                cell.paragraphs.push(Paragraph::new());
            }
            if let Some(para) = cell.paragraphs.last_mut() {
                para.add_run(run);
            }
        }
    } else if let Some(para) = current_paragraph {
        para.add_run(run);
    }
}

/// Handle a `<w:fldChar>` event (identical whether it arrives as `Event::Start` or
/// `Event::Empty`; `w:fldChar` never has children).
///
/// Tracks the begin/separate/end field-character state machine and, on `separate`,
/// extracts a `HYPERLINK` field's URL from the instruction text accumulated in
/// `field_instruction` so the following result run(s) pick it up via
/// `current_hyperlink_url`, the same mechanism `<w:hyperlink>` uses (#88, #239).
fn apply_fld_char(
    e: &BytesStart,
    in_field_instruction: &mut bool,
    field_instruction: &mut String,
    current_hyperlink_url: &mut Option<String>,
    field_hyperlink_stack: &mut Vec<Option<String>>,
) {
    for attr in e.attributes().flatten() {
        if attr.key.as_ref() == b"w:fldCharType" {
            match attr.value.as_ref() as &[u8] {
                b"begin" => {
                    *in_field_instruction = true;
                    field_instruction.clear();
                }
                b"separate" => {
                    *in_field_instruction = false;
                    let url = extract_hyperlink_field_url(field_instruction);
                    field_hyperlink_stack.push(current_hyperlink_url.clone());
                    if url.is_some() {
                        *current_hyperlink_url = url;
                    }
                }
                b"end" => {
                    *in_field_instruction = false;
                    if let Some(saved) = field_hyperlink_stack.pop() {
                        *current_hyperlink_url = saved;
                    }
                }
                _ => {}
            }
        }
    }
}

/// Page-break bookkeeping threaded through the `<w:br>` and `<w:lastRenderedPageBreak>`
/// handlers.
///
/// These three fields are one piece of state: every handler that touches any of them
/// touches all of them, and they are only meaningful relative to one another.
#[derive(Debug, Default)]
struct PageBreakState {
    /// Breaks seen inside a table, flushed once the outermost `</w:tbl>` closes (#1419).
    pending_table: u32,
    /// Breaks seen after text in the current paragraph, flushed once its `</w:p>` closes.
    pending_paragraph: u32,
    /// Whether real text has been emitted since the last recorded break (#1416).
    ///
    /// Starts `true` so a break with nothing before it is still recorded.
    text_since_break: bool,
}

/// Push a page-break marker, or defer it, depending on where it was encountered.
///
/// Inside a table it is always deferred to [`PageBreakState::pending_table`], flushed
/// once the outermost `</w:tbl>` closes and the table element has been pushed (#1419),
/// since a form feed cannot be written into the middle of a table that renders as a
/// single markdown block.
///
/// Outside a table, `elements` only gains a `DocumentElement::Paragraph` entry for
/// the paragraph currently being parsed once its `</w:p>` closes — so a break
/// encountered before any text has been collected into that paragraph can be pushed
/// immediately (it correctly precedes the not-yet-pushed paragraph), but a break
/// encountered *after* text already collected in that same paragraph must not jump
/// ahead of its own paragraph's entry. That case is deferred to
/// [`PageBreakState::pending_paragraph`] and flushed right after the paragraph is
/// pushed, mirroring the table deferral above.
fn push_or_defer_page_break(
    table_stack: &[TableContext],
    current_run: Option<&Run>,
    current_paragraph: &Option<Paragraph>,
    elements: &mut Vec<DocumentElement>,
    page_breaks: &mut PageBreakState,
) {
    if !table_stack.is_empty() {
        page_breaks.pending_table += 1;
        return;
    }

    let paragraph_has_content = current_run.is_some_and(|run| !run.text.is_empty())
        || current_paragraph
            .as_ref()
            .is_some_and(|paragraph| !paragraph.runs.is_empty());

    if paragraph_has_content {
        page_breaks.pending_paragraph += 1;
    } else {
        elements.push(DocumentElement::PageBreak);
    }
}

/// Handle a `<w:br>` event (`Event::Start` or `Event::Empty`).
///
/// A `page`-type break outside a table records a `DocumentElement::PageBreak`; any
/// other break type (`column`, `textWrapping`, or no `w:type` at all) inserts a
/// newline into the current run instead (#224).
///
/// See [`push_or_defer_page_break`] for how (and when) the marker is placed relative
/// to its enclosing table or paragraph.
///
/// This is the author's own explicit break, so it is always recorded — unlike
/// `w:lastRenderedPageBreak` (see [`apply_last_rendered_page_break`]), it is never
/// suppressed as a duplicate.
fn apply_break(
    e: &BytesStart,
    table_stack: &[TableContext],
    current_run: &mut Option<Run>,
    current_paragraph: &Option<Paragraph>,
    elements: &mut Vec<DocumentElement>,
    page_breaks: &mut PageBreakState,
) {
    let mut is_page_break = false;
    for attr in e.attributes().flatten() {
        if attr.key.as_ref() == b"w:type" && attr.value.as_ref() == b"page" {
            is_page_break = true;
            break;
        }
    }

    if is_page_break {
        push_or_defer_page_break(
            table_stack,
            current_run.as_ref(),
            current_paragraph,
            elements,
            page_breaks,
        );
        page_breaks.text_since_break = false;
    } else if let Some(run) = current_run {
        run.text.push('\n');
    }
}

/// Handle a `<w:lastRenderedPageBreak/>` event (`Event::Start` or `Event::Empty`).
///
/// Word writes this hint at the start of the first run on a page *it* rendered —
/// including, redundantly, right after an authored `<w:br w:type="page"/>` that
/// already recorded the same transition (#1416). It is the only page-break signal
/// in documents Word paginated itself with no manual breaks at all, so it cannot be
/// dropped outright; instead, it is only recorded when real text has been emitted
/// since the previous break, which is exactly the case where it is *not* a redundant
/// echo of a break already counted.
///
/// See [`push_or_defer_page_break`] for how (and when) the marker is placed relative
/// to its enclosing table or paragraph, including the [`PageBreakState::pending_table`]
/// deferral for a hint seen inside a table (#1419).
fn apply_last_rendered_page_break(
    table_stack: &[TableContext],
    current_run: &Option<Run>,
    current_paragraph: &Option<Paragraph>,
    elements: &mut Vec<DocumentElement>,
    page_breaks: &mut PageBreakState,
) {
    if !page_breaks.text_since_break {
        return;
    }
    push_or_defer_page_break(
        table_stack,
        current_run.as_ref(),
        current_paragraph,
        elements,
        page_breaks,
    );
    page_breaks.text_since_break = false;
}

/// Handle a `<w:sym w:font="…" w:char="…"/>` element (#224).
///
/// `w:char` is a hex code point, interpreted against the named symbol font's own
/// glyph mapping (commonly the U+F0xx Private Use Area for fonts like Wingdings or
/// Symbol). Without per-font glyph tables, the hex value is mapped directly to a
/// Unicode scalar; when that's not a valid scalar, a placeholder is inserted and a
/// `ProcessingWarning` records the degradation (#171).
fn apply_symbol(e: &BytesStart, current_run: &mut Option<Run>, warnings: &mut Vec<crate::types::ProcessingWarning>) {
    let mut font: Option<String> = None;
    let mut code: Option<String> = None;
    for attr in e.attributes().flatten() {
        match attr.key.as_ref() {
            b"w:font" => font = std::str::from_utf8(&attr.value).ok().map(String::from),
            b"w:char" => code = std::str::from_utf8(&attr.value).ok().map(String::from),
            _ => {}
        }
    }

    let mapped = code
        .as_deref()
        .and_then(|c| u32::from_str_radix(c.trim_start_matches("0x").trim_start_matches("0X"), 16).ok())
        .and_then(char::from_u32);

    let Some(run) = current_run else {
        return;
    };

    match mapped {
        Some(ch) => run.text.push(ch),
        None => {
            run.text.push('\u{FFFD}');
            crate::core::diagnostics::push_warning(
                warnings,
                "docx",
                format!(
                    "Could not map w:sym character code {:?} in font {:?}; inserted a placeholder",
                    code, font
                ),
            );
        }
    }
}

/// Validate archive against ZIP bomb attacks and resource exhaustion.
///
/// Checks:
/// - Maximum uncompressed size per file (100 MB default)
/// - Maximum total number of entries (10,000 default)
/// - Maximum total uncompressed size (500 MB default)
fn validate_archive_security(archive: &mut zip::ZipArchive<impl Read + Seek>) -> Result<(), DocxParseError> {
    use super::{MAX_TOTAL_UNCOMPRESSED_SIZE, MAX_UNCOMPRESSED_FILE_SIZE, MAX_ZIP_ENTRIES};

    if archive.len() > MAX_ZIP_ENTRIES {
        return Err(DocxParseError::SecurityLimit(format!(
            "Archive contains {} entries, exceeds limit of {}",
            archive.len(),
            MAX_ZIP_ENTRIES
        )));
    }

    let mut total_uncompressed: u64 = 0;
    for i in 0..archive.len() {
        let file = archive
            .by_index_raw(i)
            .map_err(|e| DocxParseError::SecurityLimit(format!("Failed to read ZIP entry {}: {}", i, e)))?;
        let size = file.size();
        if size > MAX_UNCOMPRESSED_FILE_SIZE {
            return Err(DocxParseError::SecurityLimit(format!(
                "File '{}' uncompressed size {} bytes exceeds limit of {} bytes",
                file.name(),
                size,
                MAX_UNCOMPRESSED_FILE_SIZE
            )));
        }
        total_uncompressed = total_uncompressed.saturating_add(size);
    }

    if total_uncompressed > MAX_TOTAL_UNCOMPRESSED_SIZE {
        return Err(DocxParseError::SecurityLimit(format!(
            "Total uncompressed size {} bytes exceeds limit of {} bytes",
            total_uncompressed, MAX_TOTAL_UNCOMPRESSED_SIZE
        )));
    }

    Ok(())
}

/// Recognise a conventionally-named header/footer part, e.g. `word/header2.xml`.
///
/// Returns `(is_header, index)`. Used only as the fallback sweep in
/// [`DocxParser::parse_headers_footers`], for parts no relationship declares.
fn conventional_header_footer_part(name: &str) -> Option<(bool, u32)> {
    let stem = name.strip_prefix("word/")?.strip_suffix(".xml")?;
    let (is_header, digits) = match stem.strip_prefix("header") {
        Some(rest) => (true, rest),
        None => (false, stem.strip_prefix("footer")?),
    };
    // Reject `word/headerReference.xml` and friends: only a bare numeric suffix is
    // the conventional part naming.
    digits.parse::<u32>().ok().map(|index| (is_header, index))
}

#[derive(Debug)]
struct DocxParser<R: Read + Seek> {
    archive: zip::ZipArchive<R>,
    relationships: AHashMap<String, String>,
    styles: Option<super::styles::StyleCatalog>,
    theme: Option<super::theme::Theme>,
}

impl<R: Read + Seek> DocxParser<R> {
    fn new(reader: R) -> Result<Self, DocxParseError> {
        let mut archive = zip::ZipArchive::new(reader)?;
        validate_archive_security(&mut archive)?;

        let styles = {
            let mut styles_result = None;
            if let Ok(file) = archive.by_name("word/styles.xml") {
                let mut xml = String::new();
                if file
                    .take(super::MAX_UNCOMPRESSED_FILE_SIZE)
                    .read_to_string(&mut xml)
                    .is_ok()
                {
                    styles_result = super::styles::parse_styles_xml(&xml).ok();
                }
            }
            styles_result
        };

        let theme = {
            let mut theme_result = None;
            if let Ok(file) = archive.by_name("word/theme/theme1.xml") {
                let mut xml = String::new();
                if file
                    .take(super::MAX_UNCOMPRESSED_FILE_SIZE)
                    .read_to_string(&mut xml)
                    .is_ok()
                {
                    theme_result = super::theme::parse_theme_xml(&xml).ok();
                }
            }
            theme_result
        };

        Ok(Self {
            archive,
            relationships: AHashMap::new(),
            styles,
            theme,
        })
    }

    fn parse(mut self, budget: &mut SecurityBudget) -> Result<Document, DocxParseError> {
        let mut document = Document::new();

        if let Ok(rels_xml) = self.read_file("word/_rels/document.xml.rels") {
            self.relationships = Self::parse_relationships_xml(&rels_xml);
        }

        let document_xml = self.read_file("word/document.xml")?;
        let comment_ref_ids = self.parse_document_xml(&document_xml, &mut document, budget)?;

        if let Ok(comments_xml) = self.read_file("word/comments.xml") {
            self.parse_comments(&comments_xml, &mut document.comments, budget, &mut document.warnings)?;
        }
        if !comment_ref_ids.is_empty() {
            let known_ids: ahash::AHashSet<&str> = document.comments.iter().map(|c| c.id.as_str()).collect();
            for id in &comment_ref_ids {
                if !known_ids.contains(id.as_str()) {
                    crate::core::diagnostics::push_warning(
                        &mut document.warnings,
                        "docx",
                        format!(
                            "Comment reference id {id} has no matching entry in comments.xml; the comment text was dropped"
                        ),
                    );
                }
            }
        }

        if let Ok(numbering_xml) = self.read_file("word/numbering.xml") {
            let numbering_defs = self.parse_numbering(&numbering_xml, budget)?;
            document.numbering_defs = numbering_defs;
        }

        self.parse_headers_footers(&mut document, budget)?;

        if let Ok(footnotes_xml) = self.read_file("word/footnotes.xml") {
            self.parse_notes(
                &footnotes_xml,
                &mut document.footnotes,
                NoteType::Footnote,
                budget,
                &mut document.warnings,
            )?;
        }

        if let Ok(endnotes_xml) = self.read_file("word/endnotes.xml") {
            self.parse_notes(
                &endnotes_xml,
                &mut document.endnotes,
                NoteType::Endnote,
                budget,
                &mut document.warnings,
            )?;
        }

        document.style_catalog = self.styles.take();
        document.theme = self.theme.take();
        document.image_relationships = self
            .relationships
            .iter()
            .filter(|(_, target)| !target.starts_with("http://") && !target.starts_with("https://"))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();

        Ok(document)
    }

    /// Parse relationship file to get rId → target mappings for hyperlinks and images.
    fn parse_relationships_xml(xml: &str) -> AHashMap<String, String> {
        let mut rels = AHashMap::new();
        let mut reader = Reader::from_str(xml);
        reader.config_mut().trim_text(true);
        let mut buf = Vec::new();

        loop {
            match reader.read_event_into(&mut buf) {
                Ok(Event::Empty(ref e)) | Ok(Event::Start(ref e)) if e.name().as_ref() as &[u8] == b"Relationship" => {
                    let mut id = None;
                    let mut target = None;
                    let mut rel_type_matches = false;
                    for attr in e.attributes().flatten() {
                        match attr.key.as_ref() {
                            b"Id" => id = std::str::from_utf8(&attr.value).ok().map(String::from),
                            b"Target" => {
                                target = std::str::from_utf8(&attr.value).ok().map(String::from);
                            }
                            b"Type" => {
                                rel_type_matches = std::str::from_utf8(&attr.value)
                                    .ok()
                                    .is_some_and(|t| t.contains("hyperlink") || t.contains("image"));
                            }
                            _ => {}
                        }
                    }
                    if let (Some(id_val), Some(target_val)) = (id, target)
                        && rel_type_matches
                    {
                        rels.insert(id_val, target_val);
                    }
                }
                Ok(Event::Eof) => break,
                _ => {}
            }
            buf.clear();
        }

        rels
    }

    fn read_file(&mut self, path: &str) -> Result<String, DocxParseError> {
        let read_limit = super::MAX_UNCOMPRESSED_FILE_SIZE;

        let file = self
            .archive
            .by_name(path)
            .map_err(|_| DocxParseError::FileNotFound(path.to_string()))?;

        let mut contents = String::new();
        file.take(read_limit).read_to_string(&mut contents)?;
        Ok(contents)
    }

    /// Parse `word/document.xml`'s body content and populate `document` from it.
    ///
    /// Delegates to [`Self::parse_body_elements`] (shared with headers/footers and
    /// footnotes/endnotes/comments, #85) for the element vocabulary common to all
    /// DOCX body-like content, then additionally keeps the page-break/section/
    /// revision output that's only meaningful at the main-body level.
    ///
    /// Returns the `w:commentReference` ids seen in the body, for the caller to
    /// validate against `word/comments.xml`.
    fn parse_document_xml(
        &self,
        xml: &str,
        document: &mut Document,
        budget: &mut SecurityBudget,
    ) -> Result<Vec<String>, DocxParseError> {
        let mut reader = Reader::from_str(xml);
        reader.config_mut().trim_text(false);
        let out = self.parse_body_elements(&mut reader, None, budget, &mut document.warnings)?;
        document.paragraphs = out.paragraphs;
        document.tables = out.tables;
        document.drawings = out.drawings;
        document.elements = out.elements;
        document.sections = out.sections;
        document.revisions = out.revisions;
        Ok(out.comment_ref_ids)
    }

    /// Shared element-dispatch loop for DOCX body-like content (#85): paragraphs,
    /// runs and their formatting, tables, hyperlinks, drawings — including
    /// DrawingML text boxes and the VML `v:textbox` fallback (#81) — field codes
    /// (`HYPERLINK` via both `w:fldSimple` and `w:fldChar`/`w:instrText`, #88/#239),
    /// symbols and non-breaking hyphens (#224), and footnote/endnote/comment
    /// reference markers (#82).
    ///
    /// Used by the main document body, headers/footers, and footnotes/endnotes/
    /// comments so all four content types recognize the same element vocabulary
    /// instead of drifting through separately-maintained reduced copies.
    ///
    /// `reader` must already be positioned at the first event *inside* the content
    /// to parse (e.g. right after `<w:document><w:body>`, or right after a
    /// `<w:footnote w:id="…">`/`<w:comment w:id="…">` start tag). When `stop_tag` is
    /// `Some(name)`, the loop returns as soon as the matching (depth-tracked)
    /// `</name>` end tag is consumed — used to carve one footnote/comment subtree
    /// out of a shared reader without a two-pass parse. When `stop_tag` is `None`,
    /// the loop runs to `Event::Eof` — used for whole-part parses (document body,
    /// headers, footers).
    fn parse_body_elements(
        &self,
        reader: &mut Reader<&[u8]>,
        stop_tag: Option<&[u8]>,
        budget: &mut SecurityBudget,
        warnings: &mut Vec<crate::types::ProcessingWarning>,
    ) -> Result<BodyParseOutputs, DocxParseError> {
        use crate::types::revisions::{
            DiffLine, DocumentRevision, PropertyChange, RevisionAnchor, RevisionDelta, RevisionKind,
        };

        let mut out = BodyParseOutputs::default();

        let mut buf = Vec::new();
        let mut current_paragraph: Option<Paragraph> = None;
        let mut current_run: Option<Run> = None;
        let mut in_text = false;
        let mut in_field_instruction = false;
        let mut in_instr_text = false;
        let mut field_instruction = String::new();
        let mut field_hyperlink_stack: Vec<Option<String>> = Vec::new();
        let mut current_hyperlink_url: Option<String> = None;
        let mut table_stack: Vec<TableContext> = Vec::new();
        let mut mc_fallback_depth: u32 = 0;
        let mut stop_depth: u32 = if stop_tag.is_some() { 1 } else { 0 };
        // Page-break bookkeeping shared by `w:br` and `w:lastRenderedPageBreak` (#1416, #1419).
        // `text_since_break` starts `true` so a break with nothing before it (including the
        // very first one in the document) is never treated as a spurious duplicate.
        let mut page_breaks = PageBreakState {
            text_since_break: true,
            ..PageBreakState::default()
        };

        let mut revision_kind: Option<RevisionKind> = None;
        let mut revision_attrs: (Option<String>, Option<String>, Option<String>) = (None, None, None);
        let mut revision_text = String::new();
        let mut revision_id_counter: usize = 0;
        let mut in_del_text = false;
        let mut current_paragraph_index: usize = 0;
        let mut in_run_property_change = false;
        let mut pending_format_revision_attrs: Option<(Option<String>, Option<String>, Option<String>)> = None;
        let mut pending_property_changes: Vec<PropertyChange> = Vec::new();

        loop {
            budget.step()?;
            match reader.read_event_into(&mut buf) {
                Ok(Event::Start(ref e)) => {
                    budget.enter()?;
                    let name = e.name();
                    if Some(name.as_ref()) == stop_tag {
                        stop_depth += 1;
                    }
                    match name.as_ref() as &[u8] {
                        b"w:p" => {
                            if let Some(ctx) = table_stack.last_mut() {
                                ctx.paragraph = Some(Paragraph::new());
                            } else {
                                current_paragraph_index = out.paragraphs.len();
                                current_paragraph = Some(Paragraph::new());
                            }
                        }
                        b"w:r" => {
                            let mut run = Run::default();
                            if let Some(ref url) = current_hyperlink_url {
                                run.hyperlink_url = Some(url.clone());
                            }
                            current_run = Some(run);
                        }
                        b"w:t" if !in_field_instruction => {
                            in_text = true;
                        }
                        b"w:fldChar" => {
                            apply_fld_char(
                                e,
                                &mut in_field_instruction,
                                &mut field_instruction,
                                &mut current_hyperlink_url,
                                &mut field_hyperlink_stack,
                            );
                        }
                        b"w:instrText" => {
                            in_instr_text = true;
                        }
                        b"w:fldSimple" => {
                            // Normalized, not raw: Word writes the instruction's quotes
                            // as `&quot;`, and the URL extractor looks for real quote
                            // characters. OOXML parts are XML 1.0.
                            let instr = e
                                .attributes()
                                .flatten()
                                .find(|a| a.key.as_ref() == b"w:instr")
                                .and_then(|a| {
                                    a.normalized_value(quick_xml::XmlVersion::Explicit1_0)
                                        .ok()
                                        .map(|v| v.into_owned())
                                });
                            let url = instr.as_deref().and_then(extract_hyperlink_field_url);
                            field_hyperlink_stack.push(current_hyperlink_url.clone());
                            if url.is_some() {
                                current_hyperlink_url = url;
                            }
                        }
                        b"mc:Fallback" => {
                            mc_fallback_depth += 1;
                        }
                        b"w:pict" => {
                            // `parse_vml_pict` now threads `budget` through and balances
                            // its own `</w:pict>` end tag against the `enter()` above
                            // internally, so no manual `budget.leave()` is needed here. ~keep
                            let parsed = super::drawing::parse_vml_pict(reader, budget)?;
                            if mc_fallback_depth == 0
                                && let Some(drawing) = parsed
                                && drawing.text_box_content.is_some()
                            {
                                let idx = out.drawings.len();
                                out.drawings.push(drawing);
                                out.elements.push(DocumentElement::Drawing(idx));
                                page_breaks.text_since_break = true;
                            }
                        }
                        b"m:oMathPara" => {
                            let latex = super::math::collect_and_convert_omath_para(reader, budget)?;
                            if !latex.is_empty() {
                                let run = Run {
                                    math_latex: Some((latex, true)),
                                    ..Default::default()
                                };
                                push_run_to_current(&mut table_stack, &mut current_paragraph, run);
                                page_breaks.text_since_break = true;
                            }
                        }
                        b"m:oMath" => {
                            let latex = super::math::collect_and_convert_omath(reader, budget)?;
                            if !latex.is_empty() {
                                let run = Run {
                                    math_latex: Some((latex, false)),
                                    ..Default::default()
                                };
                                push_run_to_current(&mut table_stack, &mut current_paragraph, run);
                                page_breaks.text_since_break = true;
                            }
                        }
                        b"w:tbl" => {
                            table_stack.push(TableContext::new());
                        }
                        b"w:tblPr" => {
                            if let Some(ctx) = table_stack.last_mut() {
                                // `parse_table_properties` now threads `budget` through and
                                // balances its own `</w:tblPr>` end tag against the `enter()`
                                // above internally, so no manual `budget.leave()` is needed here. ~keep
                                ctx.table.properties = Some(super::table::parse_table_properties(reader, budget)?);
                            }
                        }
                        b"w:tblGrid" => {
                            if let Some(ctx) = table_stack.last_mut() {
                                ctx.table.grid = Some(super::table::parse_table_grid(reader, budget)?);
                            }
                        }
                        b"w:tr" => {
                            if let Some(ctx) = table_stack.last_mut() {
                                ctx.current_row = Some(TableRow::default());
                            }
                        }
                        b"w:trPr" => {
                            if let Some(ctx) = table_stack.last_mut()
                                && let Some(ref mut row) = ctx.current_row
                            {
                                row.properties = Some(super::table::parse_row_properties(reader, budget)?);
                            }
                        }
                        b"w:tc" => {
                            if let Some(ctx) = table_stack.last_mut() {
                                ctx.current_cell = Some(TableCell::default());
                            }
                        }
                        b"w:tcPr" => {
                            if let Some(ctx) = table_stack.last_mut()
                                && let Some(ref mut cell) = ctx.current_cell
                            {
                                cell.properties = Some(super::table::parse_cell_properties(reader, budget)?);
                            }
                        }
                        b"w:b" | b"w:i" | b"w:u" | b"w:strike" | b"w:dstrike" | b"w:vertAlign" | b"w:sz"
                        | b"w:color" | b"w:highlight" => {
                            if in_run_property_change {
                                collect_run_property_change(e, &mut pending_property_changes);
                            } else {
                                apply_run_formatting(e, &mut current_run);
                            }
                        }
                        b"w:pStyle" | b"w:ilvl" | b"w:numId" => {
                            apply_paragraph_property(e, &mut table_stack, &mut current_paragraph);
                        }
                        b"w:hyperlink" => {
                            for attr in e.attributes().flatten() {
                                if attr.key.as_ref() == b"r:id"
                                    && let Ok(rid) = std::str::from_utf8(&attr.value)
                                {
                                    current_hyperlink_url = self.relationships.get(rid).cloned();
                                }
                            }
                        }
                        b"w:drawing" => {
                            // `parse_drawing` now threads `budget` through and balances its
                            // own `</w:drawing>` end tag against the `enter()` above
                            // internally, so no manual `budget.leave()` is needed here. ~keep
                            let drawing = super::drawing::parse_drawing(reader, budget)?;
                            let idx = out.drawings.len();
                            out.drawings.push(drawing);
                            out.elements.push(DocumentElement::Drawing(idx));
                            page_breaks.text_since_break = true;
                        }
                        b"w:br" => {
                            apply_break(
                                e,
                                &table_stack,
                                &mut current_run,
                                &current_paragraph,
                                &mut out.elements,
                                &mut page_breaks,
                            );
                        }
                        b"w:lastRenderedPageBreak" => {
                            apply_last_rendered_page_break(
                                &table_stack,
                                &current_run,
                                &current_paragraph,
                                &mut out.elements,
                                &mut page_breaks,
                            );
                        }
                        b"w:sectPr" => {
                            // `parse_section_properties_streaming` now threads `budget`
                            // through and balances its own `</w:sectPr>` end tag against the
                            // `enter()` above internally, so no manual `budget.leave()` is
                            // needed here. ~keep
                            let sect_props = super::section::parse_section_properties_streaming(reader, budget)?;
                            out.sections.push(sect_props);
                        }
                        b"w:ins" => {
                            revision_kind = Some(RevisionKind::Insertion);
                            revision_attrs = collect_revision_attrs(e);
                            revision_text.clear();
                        }
                        b"w:del" => {
                            revision_kind = Some(RevisionKind::Deletion);
                            revision_attrs = collect_revision_attrs(e);
                            revision_text.clear();
                        }
                        b"w:rPrChange" if revision_kind.is_none() => {
                            in_run_property_change = true;
                            pending_format_revision_attrs = Some(collect_revision_attrs(e));
                            pending_property_changes.clear();
                        }
                        b"w:delText" => {
                            in_del_text = true;
                        }
                        _ => {}
                    }
                }
                Ok(Event::Empty(ref e)) => {
                    let name = e.name();
                    match name.as_ref() as &[u8] {
                        b"w:fldChar" => {
                            apply_fld_char(
                                e,
                                &mut in_field_instruction,
                                &mut field_instruction,
                                &mut current_hyperlink_url,
                                &mut field_hyperlink_stack,
                            );
                        }
                        b"w:b" | b"w:i" | b"w:u" | b"w:strike" | b"w:dstrike" | b"w:vertAlign" | b"w:sz"
                        | b"w:color" | b"w:highlight" => {
                            if in_run_property_change {
                                collect_run_property_change(e, &mut pending_property_changes);
                            } else {
                                apply_run_formatting(e, &mut current_run);
                            }
                        }
                        b"w:pStyle" | b"w:ilvl" | b"w:numId" => {
                            apply_paragraph_property(e, &mut table_stack, &mut current_paragraph);
                        }
                        b"w:br" => {
                            apply_break(
                                e,
                                &table_stack,
                                &mut current_run,
                                &current_paragraph,
                                &mut out.elements,
                                &mut page_breaks,
                            );
                        }
                        b"w:tab" => {
                            if let Some(ref mut run) = current_run {
                                run.text.push('\t');
                                page_breaks.text_since_break = true;
                            }
                        }
                        b"w:noBreakHyphen" => {
                            if let Some(ref mut run) = current_run {
                                run.text.push('\u{2011}');
                                page_breaks.text_since_break = true;
                            }
                        }
                        b"w:sym" => {
                            apply_symbol(e, &mut current_run, warnings);
                            page_breaks.text_since_break = true;
                        }
                        b"w:lastRenderedPageBreak" => {
                            apply_last_rendered_page_break(
                                &table_stack,
                                &current_run,
                                &current_paragraph,
                                &mut out.elements,
                                &mut page_breaks,
                            );
                        }
                        b"w:footnoteReference" | b"w:endnoteReference" => {
                            if let Some(ref mut run) = current_run {
                                for attr in e.attributes().flatten() {
                                    if attr.key.as_ref() == b"w:id"
                                        && let Ok(id) = std::str::from_utf8(&attr.value)
                                        && id != "0"
                                        && id != "1"
                                    {
                                        run.text.push_str(&format!("[^{}]", id));
                                        page_breaks.text_since_break = true;
                                    }
                                }
                            }
                        }
                        b"w:commentReference" => {
                            for attr in e.attributes().flatten() {
                                if attr.key.as_ref() == b"w:id"
                                    && let Ok(id) = std::str::from_utf8(&attr.value)
                                {
                                    out.comment_ref_ids.push(id.to_string());
                                    if let Some(ref mut run) = current_run {
                                        run.text.push_str(&format!("[cmt:{}]", id));
                                    }
                                }
                            }
                        }
                        b"w:sectPr" => {
                            out.sections.push(super::section::SectionProperties::default());
                        }
                        b"w:tblPr" => {
                            if let Some(ctx) = table_stack.last_mut() {
                                ctx.table.properties = Some(super::table::TableProperties::default());
                            }
                        }
                        b"w:tblGrid" => {
                            if let Some(ctx) = table_stack.last_mut() {
                                ctx.table.grid = Some(super::table::TableGrid::default());
                            }
                        }
                        b"w:trPr" => {
                            if let Some(ctx) = table_stack.last_mut()
                                && let Some(ref mut row) = ctx.current_row
                            {
                                row.properties = Some(super::table::RowProperties::default());
                            }
                        }
                        b"w:tcPr" => {
                            if let Some(ctx) = table_stack.last_mut()
                                && let Some(ref mut cell) = ctx.current_cell
                            {
                                cell.properties = Some(super::table::CellProperties::default());
                            }
                        }
                        _ => {}
                    }
                }
                Ok(Event::Text(e)) => {
                    if in_instr_text {
                        let text = e.decode()?;
                        budget.check_entity(&text)?;
                        budget.account_text(text.len())?;
                        field_instruction.push_str(&text);
                    } else if in_text && let Some(ref mut run) = current_run {
                        let text = e.decode()?;
                        budget.check_entity(&text)?;
                        budget.account_text(text.len())?;
                        run.text.push_str(&text);
                        if !text.is_empty() {
                            page_breaks.text_since_break = true;
                        }
                        if revision_kind == Some(RevisionKind::Insertion) {
                            revision_text.push_str(&text);
                        }
                    } else if in_del_text {
                        let text = e.decode()?;
                        budget.check_entity(&text)?;
                        budget.account_text(text.len())?;
                        revision_text.push_str(&text);
                    }
                }
                Ok(Event::GeneralRef(e)) => {
                    if in_text && let Some(ref mut run) = current_run {
                        let text = crate::utils::xml_utils::resolve_general_ref(&e);
                        budget.account_text(text.len())?;
                        run.text.push_str(&text);
                        if !text.is_empty() {
                            page_breaks.text_since_break = true;
                        }
                        if revision_kind == Some(RevisionKind::Insertion) {
                            revision_text.push_str(&text);
                        }
                    } else if in_del_text {
                        let text = crate::utils::xml_utils::resolve_general_ref(&e);
                        budget.account_text(text.len())?;
                        revision_text.push_str(&text);
                    }
                }
                Ok(Event::End(ref e)) => {
                    budget.leave();
                    let name = e.name();
                    if Some(name.as_ref()) == stop_tag {
                        stop_depth = stop_depth.saturating_sub(1);
                    }
                    match name.as_ref() as &[u8] {
                        b"w:t" => {
                            in_text = false;
                        }
                        b"w:instrText" => {
                            in_instr_text = false;
                        }
                        b"w:fldSimple" => {
                            if let Some(saved) = field_hyperlink_stack.pop() {
                                current_hyperlink_url = saved;
                            }
                        }
                        b"mc:Fallback" => {
                            mc_fallback_depth = mc_fallback_depth.saturating_sub(1);
                        }
                        b"w:rPrChange" => {
                            in_run_property_change = false;
                        }
                        b"w:rPr" if !in_run_property_change => {
                            if let Some(attrs) = pending_format_revision_attrs.take() {
                                push_format_revision(
                                    &mut out.revisions,
                                    attrs,
                                    std::mem::take(&mut pending_property_changes),
                                    current_run.as_ref(),
                                    current_paragraph_index,
                                    &mut revision_id_counter,
                                );
                            }
                        }
                        b"w:r" => {
                            if let Some(attrs) = pending_format_revision_attrs.take() {
                                push_format_revision(
                                    &mut out.revisions,
                                    attrs,
                                    std::mem::take(&mut pending_property_changes),
                                    current_run.as_ref(),
                                    current_paragraph_index,
                                    &mut revision_id_counter,
                                );
                            }
                            if let Some(run) = current_run.take() {
                                push_run_to_current(&mut table_stack, &mut current_paragraph, run);
                            }
                        }
                        b"w:p" => {
                            if let Some(ctx) = table_stack.last_mut() {
                                if let Some(para) = ctx.paragraph.take()
                                    && let Some(ref mut cell) = ctx.current_cell
                                {
                                    cell.paragraphs.push(para);
                                }
                            } else if let Some(para) = current_paragraph.take() {
                                let idx = out.paragraphs.len();
                                out.paragraphs.push(para);
                                out.elements.push(DocumentElement::Paragraph(idx));
                                for _ in 0..std::mem::take(&mut page_breaks.pending_paragraph) {
                                    out.elements.push(DocumentElement::PageBreak);
                                }
                            }
                        }
                        b"w:tc" => {
                            if let Some(ctx) = table_stack.last_mut()
                                && let Some(cell) = ctx.current_cell.take()
                                && let Some(ref mut row) = ctx.current_row
                            {
                                budget.add_cells(1)?;
                                row.cells.push(cell);
                            }
                        }
                        b"w:tr" => {
                            if let Some(ctx) = table_stack.last_mut()
                                && let Some(row) = ctx.current_row.take()
                            {
                                ctx.table.rows.push(row);
                            }
                        }
                        b"w:tbl" => {
                            if let Some(completed_ctx) = table_stack.pop() {
                                let completed_table = completed_ctx.table;
                                if let Some(parent_ctx) = table_stack.last_mut() {
                                    if let Some(ref mut cell) = parent_ctx.current_cell {
                                        for row in completed_table.rows {
                                            for table_cell in row.cells {
                                                for para in table_cell.paragraphs {
                                                    cell.paragraphs.push(para);
                                                }
                                            }
                                        }
                                    }
                                } else {
                                    let idx = out.tables.len();
                                    out.tables.push(completed_table);
                                    out.elements.push(DocumentElement::Table(idx));
                                    // The outermost table close is the only point that flushes
                                    // page breaks deferred while inside a table (#1419); a nested
                                    // instead, so breaks in an inner table stay pending until the
                                    // outer one closes and only ever flush once.
                                    page_breaks.text_since_break = true;
                                    let deferred_breaks = std::mem::take(&mut page_breaks.pending_table);
                                    for _ in 0..deferred_breaks {
                                        out.elements.push(DocumentElement::PageBreak);
                                    }
                                    if deferred_breaks > 0 {
                                        page_breaks.text_since_break = false;
                                    }
                                }
                            }
                        }
                        b"w:hyperlink" => {
                            current_hyperlink_url = None;
                        }
                        b"w:ins" if revision_kind == Some(RevisionKind::Insertion) => {
                            let (id_opt, author_opt, date_opt) = (
                                revision_attrs.0.take(),
                                revision_attrs.1.take(),
                                revision_attrs.2.take(),
                            );
                            let revision_id = id_opt.unwrap_or_else(|| {
                                let fallback = format!("docx-ins-{}", revision_id_counter);
                                revision_id_counter += 1;
                                fallback
                            });
                            let delta = if revision_text.is_empty() {
                                RevisionDelta::default()
                            } else {
                                RevisionDelta {
                                    content: vec![DiffLine::Added(std::mem::take(&mut revision_text))],
                                    ..Default::default()
                                }
                            };
                            out.revisions.push(DocumentRevision {
                                revision_id,
                                author: author_opt,
                                timestamp: date_opt,
                                kind: RevisionKind::Insertion,
                                anchor: Some(RevisionAnchor::Paragraph {
                                    index: current_paragraph_index,
                                }),
                                delta,
                            });
                            revision_kind = None;
                            revision_text.clear();
                        }
                        b"w:del" if revision_kind == Some(RevisionKind::Deletion) => {
                            let (id_opt, author_opt, date_opt) = (
                                revision_attrs.0.take(),
                                revision_attrs.1.take(),
                                revision_attrs.2.take(),
                            );
                            let revision_id = id_opt.unwrap_or_else(|| {
                                let fallback = format!("docx-del-{}", revision_id_counter);
                                revision_id_counter += 1;
                                fallback
                            });
                            let delta = if revision_text.is_empty() {
                                RevisionDelta::default()
                            } else {
                                RevisionDelta {
                                    content: vec![DiffLine::Removed(std::mem::take(&mut revision_text))],
                                    ..Default::default()
                                }
                            };
                            out.revisions.push(DocumentRevision {
                                revision_id,
                                author: author_opt,
                                timestamp: date_opt,
                                kind: RevisionKind::Deletion,
                                anchor: Some(RevisionAnchor::Paragraph {
                                    index: current_paragraph_index,
                                }),
                                delta,
                            });
                            revision_kind = None;
                            revision_text.clear();
                        }
                        b"w:delText" => {
                            in_del_text = false;
                        }
                        _ => {}
                    }
                    if stop_tag.is_some() && stop_depth == 0 {
                        break;
                    }
                }
                Ok(Event::Eof) => {
                    if in_field_instruction {
                        crate::core::diagnostics::push_warning(
                            warnings,
                            "docx",
                            "A DOCX field instruction (w:fldChar begin) was never closed with a \
                             matching separate/end; trailing body content may have been treated \
                             as field instruction text and dropped",
                        );
                    }
                    break;
                }
                Err(e) => return Err(e.into()),
                _ => {}
            }
            buf.clear();
        }

        Ok(out)
    }

    fn parse_numbering(
        &self,
        xml: &str,
        budget: &mut SecurityBudget,
    ) -> Result<AHashMap<(i64, i64), ListType>, DocxParseError> {
        let mut numbering_defs: AHashMap<(i64, i64), ListType> = AHashMap::new();
        let mut abstract_num_formats: AHashMap<i64, AHashMap<i64, ListType>> = AHashMap::new();
        let mut num_to_abstract: AHashMap<i64, i64> = AHashMap::new();

        let mut reader = Reader::from_str(xml);
        reader.config_mut().trim_text(false);

        let mut buf = Vec::new();
        let mut current_abstract_num_id: Option<i64> = None;
        let mut current_num_id: Option<i64> = None;
        let mut current_lvl: Option<i64> = None;

        loop {
            budget.step()?;
            match reader.read_event_into(&mut buf) {
                Ok(Event::Start(ref e)) => {
                    budget.enter()?;
                    match e.name().as_ref() as &[u8] {
                        b"w:abstractNum" => {
                            for attr in e.attributes().flatten() {
                                if attr.key.as_ref() == b"w:abstractNumId"
                                    && let Ok(id_str) = std::str::from_utf8(&attr.value)
                                {
                                    current_abstract_num_id = id_str.parse().ok();
                                }
                            }
                        }
                        b"w:num" => {
                            for attr in e.attributes().flatten() {
                                if attr.key.as_ref() == b"w:numId"
                                    && let Ok(id_str) = std::str::from_utf8(&attr.value)
                                {
                                    current_num_id = id_str.parse().ok();
                                }
                            }
                        }
                        b"w:lvl" => {
                            for attr in e.attributes().flatten() {
                                if attr.key.as_ref() == b"w:ilvl"
                                    && let Ok(id_str) = std::str::from_utf8(&attr.value)
                                {
                                    current_lvl = id_str.parse().ok();
                                }
                            }
                        }
                        b"w:numFmt" => {
                            if let (Some(abstract_id), Some(lvl)) = (current_abstract_num_id, current_lvl) {
                                let fmt = get_val_attr_string(e);
                                let list_type = match fmt.as_deref() {
                                    Some("decimal") | Some("decimalZero") | Some("lowerLetter")
                                    | Some("upperLetter") | Some("lowerRoman") | Some("upperRoman") => {
                                        ListType::Numbered
                                    }
                                    _ => ListType::Bullet,
                                };
                                abstract_num_formats
                                    .entry(abstract_id)
                                    .or_default()
                                    .insert(lvl, list_type);
                            }
                        }
                        _ => {}
                    }
                }
                Ok(Event::Empty(ref e)) => match e.name().as_ref() as &[u8] {
                    b"w:abstractNumId" => {
                        if let Some(num_id) = current_num_id
                            && let Some(abstract_id) = get_val_attr(e)
                        {
                            num_to_abstract.insert(num_id, abstract_id);
                        }
                    }
                    b"w:numFmt" => {
                        if let (Some(abstract_id), Some(lvl)) = (current_abstract_num_id, current_lvl) {
                            let fmt = get_val_attr_string(e);
                            let list_type = match fmt.as_deref() {
                                Some("decimal") | Some("decimalZero") | Some("lowerLetter") | Some("upperLetter")
                                | Some("lowerRoman") | Some("upperRoman") => ListType::Numbered,
                                _ => ListType::Bullet,
                            };
                            abstract_num_formats
                                .entry(abstract_id)
                                .or_default()
                                .insert(lvl, list_type);
                        }
                    }
                    _ => {}
                },
                Ok(Event::End(ref e)) => {
                    budget.leave();
                    match e.name().as_ref() as &[u8] {
                        b"w:abstractNum" => {
                            current_abstract_num_id = None;
                            current_lvl = None;
                        }
                        b"w:lvl" => {
                            current_lvl = None;
                        }
                        b"w:num" => {
                            current_num_id = None;
                        }
                        _ => {}
                    }
                }
                Ok(Event::Eof) => break,
                _ => {}
            }
            buf.clear();
        }

        for (num_id, abstract_id) in &num_to_abstract {
            if let Some(formats) = abstract_num_formats.get(abstract_id) {
                for (lvl, list_type) in formats {
                    numbering_defs.insert((*num_id, *lvl), *list_type);
                }
            }
        }

        Ok(numbering_defs)
    }

    /// Discover and parse every header/footer part in the package (#83).
    ///
    /// Earlier versions of this parser only looked for `word/header{1,2,3}.xml` and
    /// `word/footer{1,2,3}.xml`, silently dropping any part beyond index 3. The
    /// relationships in `word/_rels/document.xml.rels` are the authoritative index, so
    /// they are read first and in declaration order.
    ///
    /// The conventional filenames are then swept as a *fallback*, not a replacement:
    /// a package can carry a header/footer part that no relationship points at, and
    /// dropping those would trade one silent loss (index > 3) for another (unreferenced
    /// part). Anything already named by a relationship is skipped, so a well-formed
    /// document is unaffected.
    fn parse_headers_footers(
        &mut self,
        document: &mut Document,
        budget: &mut SecurityBudget,
    ) -> Result<(), DocxParseError> {
        let rels_xml = self.read_file("word/_rels/document.xml.rels").unwrap_or_default();
        let mut targets = Self::parse_header_footer_relationship_targets(&rels_xml);

        // Collect before touching `self.archive` mutably in the read loop below.
        let mut orphans: Vec<(bool, u32, String)> = self
            .archive
            .file_names()
            .filter_map(|name| {
                let (is_header, index) = conventional_header_footer_part(name)?;
                (!targets.iter().any(|(_, seen)| seen == name)).then(|| (is_header, index, name.to_string()))
            })
            .collect();
        // Headers before footers, each in numeric order, so output does not depend on
        // the order the zip central directory happens to list parts in.
        orphans.sort_by_key(|(is_header, index, _)| (!is_header, *index));
        targets.extend(orphans.into_iter().map(|(is_header, _, path)| (is_header, path)));

        for (is_header, path) in targets {
            match self.read_file(&path) {
                Ok(xml) => {
                    let mut header_footer = HeaderFooter::default();
                    self.parse_header_footer_content(&xml, &mut header_footer, budget, &mut document.warnings)?;
                    if is_header {
                        document.headers.push(header_footer);
                    } else {
                        document.footers.push(header_footer);
                    }
                }
                Err(_) => {
                    crate::core::diagnostics::push_warning(
                        &mut document.warnings,
                        "docx",
                        format!(
                            "{} relationship target '{}' could not be read; that {} was dropped",
                            if is_header { "Header" } else { "Footer" },
                            path,
                            if is_header { "header" } else { "footer" }
                        ),
                    );
                }
            }
        }

        Ok(())
    }

    /// Scan a relationships XML document for header/footer part targets.
    ///
    /// Returns `(is_header, resolved_zip_path)` pairs in document order. Relationship
    /// targets are resolved relative to `word/` (the directory containing
    /// `document.xml`, per OPC convention), or treated as package-root-absolute when
    /// they start with `/`.
    fn parse_header_footer_relationship_targets(xml: &str) -> Vec<(bool, String)> {
        let mut targets = Vec::new();
        let mut reader = Reader::from_str(xml);
        reader.config_mut().trim_text(true);
        let mut buf = Vec::new();

        loop {
            match reader.read_event_into(&mut buf) {
                Ok(Event::Empty(ref e)) | Ok(Event::Start(ref e)) if e.name().as_ref() as &[u8] == b"Relationship" => {
                    let mut target: Option<String> = None;
                    let mut kind: Option<bool> = None;
                    for attr in e.attributes().flatten() {
                        match attr.key.as_ref() {
                            b"Target" => target = std::str::from_utf8(&attr.value).ok().map(String::from),
                            b"Type" => {
                                kind = std::str::from_utf8(&attr.value).ok().and_then(|t| {
                                    if t.ends_with("/header") {
                                        Some(true)
                                    } else if t.ends_with("/footer") {
                                        Some(false)
                                    } else {
                                        None
                                    }
                                });
                            }
                            _ => {}
                        }
                    }
                    if let (Some(target), Some(is_header)) = (target, kind) {
                        let resolved = match target.strip_prefix('/') {
                            Some(stripped) => stripped.to_string(),
                            None => format!("word/{}", target),
                        };
                        targets.push((is_header, resolved));
                    }
                }
                Ok(Event::Eof) => break,
                _ => {}
            }
            buf.clear();
        }

        targets
    }

    /// Parse one header or footer part, sharing the same element vocabulary as the
    /// main document body and footnotes/endnotes (#85).
    fn parse_header_footer_content(
        &self,
        xml: &str,
        header_footer: &mut HeaderFooter,
        budget: &mut SecurityBudget,
        warnings: &mut Vec<crate::types::ProcessingWarning>,
    ) -> Result<(), DocxParseError> {
        let mut reader = Reader::from_str(xml);
        reader.config_mut().trim_text(false);
        let out = self.parse_body_elements(&mut reader, None, budget, warnings)?;
        header_footer.paragraphs = out.paragraphs;
        header_footer.tables = out.tables;
        Ok(())
    }

    /// Parse `word/footnotes.xml` or `word/endnotes.xml`, delegating each individual
    /// `<w:footnote>`/`<w:endnote>` subtree to the shared body element loop (#85) so
    /// notes recognize the same tables/hyperlinks/fields/symbols as the main body.
    ///
    /// A top-level table inside a note (rare, but structurally possible) is
    /// flattened into the note's paragraph list, the same way a table nested inside
    /// a table cell is flattened by the shared loop itself.
    fn parse_notes(
        &self,
        xml: &str,
        notes: &mut Vec<Note>,
        note_type: NoteType,
        budget: &mut SecurityBudget,
        warnings: &mut Vec<crate::types::ProcessingWarning>,
    ) -> Result<(), DocxParseError> {
        let mut reader = Reader::from_str(xml);
        reader.config_mut().trim_text(false);
        let mut buf = Vec::new();

        loop {
            budget.step()?;
            match reader.read_event_into(&mut buf) {
                Ok(Event::Start(ref e)) if matches!(e.name().as_ref() as &[u8], b"w:footnote" | b"w:endnote") => {
                    let mut id = String::new();
                    for attr in e.attributes().flatten() {
                        if attr.key.as_ref() == b"w:id" {
                            id = String::from_utf8_lossy(&attr.value).to_string();
                        }
                    }
                    let stop_tag = e.name().as_ref().to_vec();
                    let out = self.parse_body_elements(&mut reader, Some(stop_tag.as_slice()), budget, warnings)?;

                    if id != "-1" && id != "0" && id != "1" {
                        let mut paragraphs = out.paragraphs;
                        for table in out.tables {
                            for row in table.rows {
                                for cell in row.cells {
                                    paragraphs.extend(cell.paragraphs);
                                }
                            }
                        }
                        notes.push(Note {
                            id,
                            note_type,
                            paragraphs,
                        });
                    }
                }
                Ok(Event::Eof) => break,
                Err(e) => return Err(e.into()),
                _ => {}
            }
            buf.clear();
        }

        Ok(())
    }

    /// Parse `word/comments.xml`, delegating each `<w:comment>` subtree to the
    /// shared body element loop (#82, #85).
    fn parse_comments(
        &self,
        xml: &str,
        comments: &mut Vec<Comment>,
        budget: &mut SecurityBudget,
        warnings: &mut Vec<crate::types::ProcessingWarning>,
    ) -> Result<(), DocxParseError> {
        let mut reader = Reader::from_str(xml);
        reader.config_mut().trim_text(false);
        let mut buf = Vec::new();

        loop {
            budget.step()?;
            match reader.read_event_into(&mut buf) {
                Ok(Event::Start(ref e)) if e.name().as_ref() as &[u8] == b"w:comment" => {
                    let mut id = String::new();
                    let mut author: Option<String> = None;
                    for attr in e.attributes().flatten() {
                        match attr.key.as_ref() {
                            b"w:id" => id = String::from_utf8_lossy(&attr.value).to_string(),
                            b"w:author" => {
                                author = std::str::from_utf8(&attr.value)
                                    .ok()
                                    .map(String::from)
                                    .filter(|s| !s.is_empty());
                            }
                            _ => {}
                        }
                    }
                    let out = self.parse_body_elements(&mut reader, Some(b"w:comment".as_slice()), budget, warnings)?;
                    let mut paragraphs = out.paragraphs;
                    for table in out.tables {
                        for row in table.rows {
                            for cell in row.cells {
                                paragraphs.extend(cell.paragraphs);
                            }
                        }
                    }
                    comments.push(Comment { id, author, paragraphs });
                }
                Ok(Event::Eof) => break,
                Err(e) => return Err(e.into()),
                _ => {}
            }
            buf.clear();
        }

        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
enum DocxParseError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("ZIP error: {0}")]
    Zip(#[from] zip::result::ZipError),

    #[error("XML parsing error: {0}")]
    Xml(#[from] quick_xml::Error),

    #[error("Required file not found in DOCX: {0}")]
    FileNotFound(String),

    #[error("Security limit exceeded: {0}")]
    SecurityLimit(String),
}

impl From<quick_xml::encoding::EncodingError> for DocxParseError {
    fn from(e: quick_xml::encoding::EncodingError) -> Self {
        DocxParseError::Xml(quick_xml::Error::Encoding(e))
    }
}

impl From<SecurityError> for DocxParseError {
    fn from(e: SecurityError) -> Self {
        DocxParseError::SecurityLimit(e.to_string())
    }
}

/// Parse a DOCX document from bytes and return the structured document.
pub(crate) fn parse_document(bytes: &[u8], budget: &mut SecurityBudget) -> crate::error::Result<Document> {
    let cursor = Cursor::new(bytes);
    let parser = DocxParser::new(cursor)
        .map_err(|e| crate::error::XbergError::parsing(format!("DOCX parsing failed: {}", e)))?;
    parser
        .parse(budget)
        .map_err(|e| crate::error::XbergError::parsing(format!("DOCX parsing failed: {}", e)))
}

/// Extract text from DOCX bytes.
#[cfg(test)]
pub(crate) fn extract_text_from_bytes(bytes: &[u8]) -> crate::error::Result<String> {
    let mut budget = SecurityBudget::with_defaults();
    let doc = parse_document(bytes, &mut budget)?;
    Ok(doc.extract_text())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extractors::security::SecurityBudget;

    /// Runs are concatenated directly; whitespace comes from the XML text content.
    #[test]
    fn test_paragraph_to_text_concatenates_runs() {
        let mut para = Paragraph::new();
        para.add_run(Run::new("Hello ".to_string()));
        para.add_run(Run::new("World".to_string()));
        assert_eq!(para.to_text(), "Hello World");
    }

    /// Mid-word run splits (e.g. drop caps) must not insert extra spaces.
    #[test]
    fn test_paragraph_to_text_mid_word_split() {
        let mut para = Paragraph::new();
        para.add_run(Run::new("S".to_string()));
        para.add_run(Run::new("ermocination".to_string()));
        assert_eq!(para.to_text(), "Sermocination");
    }

    #[test]
    fn test_paragraph_to_text_single_run() {
        let mut para = Paragraph::new();
        para.add_run(Run::new("Hello".to_string()));
        assert_eq!(para.to_text(), "Hello");
    }

    #[test]
    fn test_paragraph_to_text_no_runs() {
        let para = Paragraph::new();
        assert_eq!(para.to_text(), "");
    }

    /// Whitespace between words is stored in the run text, not added by join.
    #[test]
    fn test_paragraph_to_text_whitespace_in_runs() {
        let mut para = Paragraph::new();
        para.add_run(Run::new("The ".to_string()));
        para.add_run(Run::new("quick ".to_string()));
        para.add_run(Run::new("fox".to_string()));
        assert_eq!(para.to_text(), "The quick fox");
    }

    #[test]
    fn test_run_bold_to_markdown() {
        let run = Run {
            text: "hello".to_string(),
            bold: true,
            ..Default::default()
        };
        assert_eq!(run.to_markdown(), "**hello**");
    }

    #[test]
    fn test_run_italic_to_markdown() {
        let run = Run {
            text: "hello".to_string(),
            italic: true,
            ..Default::default()
        };
        assert_eq!(run.to_markdown(), "*hello*");
    }

    #[test]
    fn test_run_bold_italic_to_markdown() {
        let run = Run {
            text: "hello".to_string(),
            bold: true,
            italic: true,
            ..Default::default()
        };
        assert_eq!(run.to_markdown(), "***hello***");
    }

    #[test]
    fn test_run_strikethrough_to_markdown() {
        let run = Run {
            text: "hello".to_string(),
            strikethrough: true,
            ..Default::default()
        };
        assert_eq!(run.to_markdown(), "~~hello~~");
    }

    #[test]
    fn test_run_hyperlink_to_markdown() {
        let run = Run {
            text: "click here".to_string(),
            hyperlink_url: Some("https://example.com".to_string()),
            ..Default::default()
        };
        assert_eq!(run.to_markdown(), "[click here](https://example.com)");
    }

    #[test]
    fn test_run_bold_hyperlink_to_markdown() {
        let run = Run {
            text: "click".to_string(),
            bold: true,
            hyperlink_url: Some("https://example.com".to_string()),
            ..Default::default()
        };
        assert_eq!(run.to_markdown(), "[**click**](https://example.com)");
    }

    #[test]
    fn test_run_empty_text_to_markdown() {
        let run = Run {
            text: String::new(),
            bold: true,
            ..Default::default()
        };
        assert_eq!(run.to_markdown(), "");
    }

    /// Adjacent bold runs must be merged to avoid spurious `****` sequences.
    #[test]
    fn test_adjacent_bold_runs_merged() {
        let mut para = Paragraph::new();
        let mut r1 = Run::new("Shuishang".to_string());
        r1.bold = true;
        let mut r2 = Run::new(" Township".to_string());
        r2.bold = true;
        para.add_run(r1);
        para.add_run(r2);
        assert_eq!(para.runs_to_markdown(), "**Shuishang Township**");
    }

    /// Adjacent italic runs must be merged to avoid spurious `**` sequences.
    #[test]
    fn test_adjacent_italic_runs_merged() {
        let mut para = Paragraph::new();
        let mut r1 = Run::new("he".to_string());
        r1.italic = true;
        let mut r2 = Run::new("llo".to_string());
        r2.italic = true;
        para.add_run(r1);
        para.add_run(r2);
        assert_eq!(para.runs_to_markdown(), "*hello*");
    }

    /// Runs with different formatting must NOT be merged.
    #[test]
    fn test_different_formatting_runs_not_merged() {
        let mut para = Paragraph::new();
        let mut r1 = Run::new("bold".to_string());
        r1.bold = true;
        let r2 = Run::new(" normal".to_string());
        para.add_run(r1);
        para.add_run(r2);
        assert_eq!(para.runs_to_markdown(), "**bold** normal");
    }

    /// Three adjacent bold runs produce a single merged bold span.
    #[test]
    fn test_three_adjacent_bold_runs_merged() {
        let mut para = Paragraph::new();
        for text in &["i", "l", "l"] {
            let mut r = Run::new(text.to_string());
            r.bold = true;
            para.add_run(r);
        }
        assert_eq!(para.runs_to_markdown(), "**ill**");
    }

    #[test]
    fn test_paragraph_heading_to_markdown() {
        let mut para = Paragraph::new();
        para.style = Some("Title".to_string());
        para.add_run(Run::new("My Title".to_string()));
        let defs = AHashMap::new();
        let mut counters = AHashMap::new();
        assert_eq!(para.to_markdown(&defs, &mut counters, Some(1)), "# My Title");
    }

    #[test]
    fn test_paragraph_heading1_to_markdown() {
        let mut para = Paragraph::new();
        para.style = Some("Heading1".to_string());
        para.add_run(Run::new("Section".to_string()));
        let defs = AHashMap::new();
        let mut counters = AHashMap::new();
        assert_eq!(para.to_markdown(&defs, &mut counters, Some(2)), "## Section");
    }

    #[test]
    fn test_paragraph_heading2_to_markdown() {
        let mut para = Paragraph::new();
        para.style = Some("Heading2".to_string());
        para.add_run(Run::new("Subsection".to_string()));
        let defs = AHashMap::new();
        let mut counters = AHashMap::new();
        assert_eq!(para.to_markdown(&defs, &mut counters, Some(3)), "### Subsection");
    }

    #[test]
    fn test_paragraph_bullet_list_to_markdown() {
        let mut para = Paragraph::new();
        para.numbering_id = Some(1);
        para.numbering_level = Some(0);
        para.add_run(Run::new("Item".to_string()));
        let mut defs = AHashMap::new();
        defs.insert((1, 0), ListType::Bullet);
        let mut counters = AHashMap::new();
        assert_eq!(para.to_markdown(&defs, &mut counters, None), "- Item");
    }

    #[test]
    fn test_paragraph_numbered_list_to_markdown() {
        let mut para = Paragraph::new();
        para.numbering_id = Some(2);
        para.numbering_level = Some(0);
        para.add_run(Run::new("Item".to_string()));
        let mut defs = AHashMap::new();
        defs.insert((2, 0), ListType::Numbered);
        let mut counters = AHashMap::new();
        assert_eq!(para.to_markdown(&defs, &mut counters, None), "1. Item");
    }

    #[test]
    fn test_paragraph_nested_list_to_markdown() {
        let mut para = Paragraph::new();
        para.numbering_id = Some(1);
        para.numbering_level = Some(1);
        para.add_run(Run::new("Nested".to_string()));
        let mut defs = AHashMap::new();
        defs.insert((1, 1), ListType::Bullet);
        let mut counters = AHashMap::new();
        assert_eq!(para.to_markdown(&defs, &mut counters, None), "  - Nested");
    }

    #[test]
    fn test_heading_level_from_style_name() {
        assert_eq!(heading_level_from_style_name("Title"), Some(1));
        assert_eq!(heading_level_from_style_name("Heading1"), Some(2));
        assert_eq!(heading_level_from_style_name("Heading2"), Some(3));
        assert_eq!(heading_level_from_style_name("Heading3"), Some(4));
        assert_eq!(heading_level_from_style_name("Heading6"), Some(6));
        assert_eq!(heading_level_from_style_name("Normal"), None);
    }

    #[test]
    fn test_resolve_heading_level_with_style_catalog() {
        use super::super::styles::{ParagraphProperties, StyleCatalog, StyleDefinition, StyleType};

        let mut doc = Document::new();
        let mut catalog = StyleCatalog::default();

        catalog.styles.insert(
            "CustomHeading".to_string(),
            StyleDefinition {
                id: "CustomHeading".to_string(),
                name: Some("Custom Heading".to_string()),
                style_type: StyleType::Paragraph,
                based_on: None,
                next_style: None,
                is_default: false,
                paragraph_properties: ParagraphProperties {
                    outline_level: Some(2),
                    ..Default::default()
                },
                run_properties: Default::default(),
            },
        );

        doc.style_catalog = Some(catalog);
        assert_eq!(doc.resolve_heading_level("CustomHeading"), Some(3));
    }

    #[test]
    fn test_resolve_heading_level_inheritance_chain() {
        use super::super::styles::{ParagraphProperties, StyleCatalog, StyleDefinition, StyleType};

        let mut doc = Document::new();
        let mut catalog = StyleCatalog::default();

        catalog.styles.insert(
            "ParentStyle".to_string(),
            StyleDefinition {
                id: "ParentStyle".to_string(),
                name: Some("Parent".to_string()),
                style_type: StyleType::Paragraph,
                based_on: None,
                next_style: None,
                is_default: false,
                paragraph_properties: ParagraphProperties {
                    outline_level: Some(0),
                    ..Default::default()
                },
                run_properties: Default::default(),
            },
        );

        catalog.styles.insert(
            "ChildStyle".to_string(),
            StyleDefinition {
                id: "ChildStyle".to_string(),
                name: Some("Child".to_string()),
                style_type: StyleType::Paragraph,
                based_on: Some("ParentStyle".to_string()),
                next_style: None,
                is_default: false,
                paragraph_properties: ParagraphProperties::default(),
                run_properties: Default::default(),
            },
        );

        doc.style_catalog = Some(catalog);
        assert_eq!(doc.resolve_heading_level("ChildStyle"), Some(1));
    }

    #[test]
    fn test_underline_rendering() {
        let mut run = Run::new("underlined text".to_string());
        run.underline = true;
        assert_eq!(run.to_markdown(), "<u>underlined text</u>");
    }

    #[test]
    fn test_underline_combined_with_bold_italic() {
        let mut run = Run::new("styled".to_string());
        run.bold = true;
        run.italic = true;
        run.underline = true;
        let md = run.to_markdown();
        assert!(md.contains("<u>"));
        assert!(md.contains("</u>"));
        assert!(md.contains("**"));
        assert!(md.contains("*"));
    }

    #[test]
    fn test_header_footer_excluded_from_output() {
        let mut doc = Document::new();

        let mut header = HeaderFooter::default();
        let mut para = Paragraph::new();
        para.add_run(Run::new("Header Text".to_string()));
        header.paragraphs.push(para);
        doc.headers.push(header);

        let mut body_para = Paragraph::new();
        body_para.add_run(Run::new("Body content".to_string()));
        let idx = doc.paragraphs.len();
        doc.paragraphs.push(body_para);
        doc.elements.push(DocumentElement::Paragraph(idx));

        let mut footer = HeaderFooter::default();
        let mut footer_para = Paragraph::new();
        footer_para.add_run(Run::new("Footer Text".to_string()));
        footer.paragraphs.push(footer_para);
        doc.footers.push(footer);

        let md = doc.to_markdown(true);
        assert!(!md.contains("Header Text"), "Header should not be in markdown output");
        assert!(md.contains("Body content"), "Should contain body content");
        assert!(!md.contains("Footer Text"), "Footer should not be in markdown output");

        let plain = doc.to_plain_text();
        assert!(
            !plain.contains("Header Text"),
            "Header should not be in plain text output"
        );
        assert!(plain.contains("Body content"), "Should contain body content");
        assert!(
            !plain.contains("Footer Text"),
            "Footer should not be in plain text output"
        );

        assert_eq!(doc.headers.len(), 1);
        assert_eq!(doc.footers.len(), 1);
        assert_eq!(doc.headers[0].paragraphs[0].runs[0].text, "Header Text");
        assert_eq!(doc.footers[0].paragraphs[0].runs[0].text, "Footer Text");
    }

    #[test]
    fn test_footnote_reference_in_parsing() {
        let xml = r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
            <w:body>
                <w:p>
                    <w:r>
                        <w:t>See note</w:t>
                    </w:r>
                    <w:r>
                        <w:footnoteReference w:id="2"/>
                    </w:r>
                </w:p>
            </w:body>
        </w:document>"#;

        let parser_struct = DocxParser {
            archive: zip::ZipArchive::new(std::io::Cursor::new(create_minimal_zip())).unwrap(),
            relationships: AHashMap::new(),
            styles: None,
            theme: None,
        };
        let mut document = Document::new();
        {
            let mut budget = crate::extractors::security::SecurityBudget::with_defaults();
            parser_struct
                .parse_document_xml(xml, &mut document, &mut budget)
                .unwrap();
        }

        assert_eq!(document.paragraphs.len(), 1);
        let full_text = document.paragraphs[0].to_text();
        assert!(
            full_text.contains("[^2]"),
            "Should contain footnote reference [^2], got: {}",
            full_text
        );
    }

    #[test]
    fn test_inline_tab_preserved_between_words() {
        let xml = r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
            <w:body>
                <w:p>
                    <w:r>
                        <w:t>Alpha</w:t>
                        <w:tab/>
                        <w:t>Beta</w:t>
                    </w:r>
                </w:p>
            </w:body>
        </w:document>"#;

        let parser_struct = DocxParser {
            archive: zip::ZipArchive::new(std::io::Cursor::new(create_minimal_zip())).unwrap(),
            relationships: AHashMap::new(),
            styles: None,
            theme: None,
        };
        let mut document = Document::new();
        {
            let mut budget = crate::extractors::security::SecurityBudget::with_defaults();
            parser_struct
                .parse_document_xml(xml, &mut document, &mut budget)
                .unwrap();
        }

        assert_eq!(document.paragraphs.len(), 1);
        let full_text = document.paragraphs[0].to_text();
        assert_eq!(
            full_text, "Alpha\tBeta",
            "Tab character inside a run must separate the words, got: {:?}",
            full_text
        );
    }

    #[test]
    fn test_tab_stop_definition_does_not_emit_tab_character() {
        let xml = r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
            <w:body>
                <w:p>
                    <w:pPr>
                        <w:tabs>
                            <w:tab w:val="left" w:pos="720"/>
                        </w:tabs>
                    </w:pPr>
                    <w:r>
                        <w:t>Alpha</w:t>
                    </w:r>
                </w:p>
            </w:body>
        </w:document>"#;

        let parser_struct = DocxParser {
            archive: zip::ZipArchive::new(std::io::Cursor::new(create_minimal_zip())).unwrap(),
            relationships: AHashMap::new(),
            styles: None,
            theme: None,
        };
        let mut document = Document::new();
        {
            let mut budget = crate::extractors::security::SecurityBudget::with_defaults();
            parser_struct
                .parse_document_xml(xml, &mut document, &mut budget)
                .unwrap();
        }

        assert_eq!(document.paragraphs.len(), 1);
        let full_text = document.paragraphs[0].to_text();
        assert_eq!(
            full_text, "Alpha",
            "Tab-stop definition in w:pPr/w:tabs must not emit a tab character, got: {:?}",
            full_text
        );
    }

    #[test]
    fn test_separator_footnotes_filtered() {
        let xml = r#"<w:footnotes xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
            <w:footnote w:id="0">
                <w:p><w:r><w:t>separator</w:t></w:r></w:p>
            </w:footnote>
            <w:footnote w:id="1">
                <w:p><w:r><w:t>continuation</w:t></w:r></w:p>
            </w:footnote>
            <w:footnote w:id="2">
                <w:p><w:r><w:t>Actual footnote</w:t></w:r></w:p>
            </w:footnote>
        </w:footnotes>"#;

        let parser_struct = DocxParser {
            archive: zip::ZipArchive::new(std::io::Cursor::new(create_minimal_zip())).unwrap(),
            relationships: AHashMap::new(),
            styles: None,
            theme: None,
        };
        let mut notes = Vec::new();
        {
            let mut budget = crate::extractors::security::SecurityBudget::with_defaults();
            let mut warnings = Vec::new();
            parser_struct
                .parse_notes(xml, &mut notes, NoteType::Footnote, &mut budget, &mut warnings)
                .unwrap();
        }

        assert_eq!(notes.len(), 1, "Only actual footnote should remain");
        assert_eq!(notes[0].id, "2");
    }

    fn create_minimal_zip() -> Vec<u8> {
        use std::io::Write;
        let buf = Vec::new();
        let cursor = std::io::Cursor::new(buf);
        let mut zip = zip::ZipWriter::new(cursor);
        let options: zip::write::FileOptions<()> = zip::write::FileOptions::default();
        zip.start_file("word/document.xml", options).unwrap();
        zip.write_all(b"<w:document/>").unwrap();
        zip.finish().unwrap().into_inner()
    }

    #[test]
    fn test_is_format_enabled_no_val() {
        let xml = r#"<w:b/>"#;
        let mut reader = Reader::from_str(xml);
        let mut buf = Vec::new();
        if let Ok(Event::Empty(ref e)) = reader.read_event_into(&mut buf) {
            assert!(is_format_enabled(e));
        }
    }

    #[test]
    fn test_security_valid_minimal_archive() {
        use std::io::Cursor;
        let zip_data = vec![
            0x50, 0x4b, 0x05, 0x06, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00,
        ];
        let cursor = Cursor::new(zip_data);
        let result = DocxParser::new(cursor);
        assert!(
            result.is_ok(),
            "Empty valid ZIP should pass security checks: {:?}",
            result.err()
        );
    }

    #[test]
    fn test_security_constants_are_reasonable() {
        use super::super::{MAX_TOTAL_UNCOMPRESSED_SIZE, MAX_UNCOMPRESSED_FILE_SIZE, MAX_ZIP_ENTRIES};

        const {
            assert!(MAX_ZIP_ENTRIES >= 1_000, "Entry limit must be at least 1,000");
            assert!(
                MAX_UNCOMPRESSED_FILE_SIZE >= 10 * 1024 * 1024,
                "Per-file size limit must be at least 10 MB"
            );
            assert!(
                MAX_TOTAL_UNCOMPRESSED_SIZE >= MAX_UNCOMPRESSED_FILE_SIZE,
                "Total size limit must be >= per-file limit"
            );
        }
    }

    #[test]
    fn test_security_normal_docx_passes() {
        use std::io::{Cursor, Write};

        let buffer = Vec::new();
        let cursor = Cursor::new(buffer);
        let mut zip = zip::ZipWriter::new(cursor);
        let options = zip::write::FileOptions::<()>::default().compression_method(zip::CompressionMethod::Stored);

        zip.start_file("word/document.xml", options).unwrap();
        zip.write_all(b"<w:document/>").unwrap();

        zip.start_file("docProps/core.xml", options).unwrap();
        zip.write_all(b"<cp:coreProperties/>").unwrap();

        let cursor = zip.finish().unwrap();
        let data = cursor.into_inner();

        let mut archive = zip::ZipArchive::new(Cursor::new(data)).unwrap();
        let result = validate_archive_security(&mut archive);
        assert!(
            result.is_ok(),
            "A normal small archive must pass security validation: {:?}",
            result.err()
        );
    }

    #[test]
    fn test_security_rejects_too_many_entries() {
        use std::io::{Cursor, Write};

        let buffer = Vec::new();
        let cursor = Cursor::new(buffer);
        let mut zip = zip::ZipWriter::new(cursor);
        let options = zip::write::FileOptions::<()>::default().compression_method(zip::CompressionMethod::Stored);

        for i in 0..10_001 {
            zip.start_file(format!("file_{}.txt", i), options).unwrap();
            zip.write_all(b"").unwrap();
        }

        let cursor = zip.finish().unwrap();
        let data = cursor.into_inner();

        let mut archive = zip::ZipArchive::new(Cursor::new(data)).unwrap();
        let result = validate_archive_security(&mut archive);
        assert!(result.is_err(), "Archive with >10,000 entries must be rejected");

        let err_msg = format!("{}", result.unwrap_err());
        assert!(
            err_msg.contains("10001") && err_msg.contains("10000"),
            "Error should mention actual and limit counts, got: {}",
            err_msg
        );
    }

    #[test]
    fn test_security_rejects_oversized_file() {
        use std::io::{Cursor, Write};

        let buffer = Vec::new();
        let cursor = Cursor::new(buffer);
        let mut zip = zip::ZipWriter::new(cursor);
        let options = zip::write::FileOptions::<()>::default().compression_method(zip::CompressionMethod::Stored);

        zip.start_file("word/document.xml", options).unwrap();
        zip.write_all(&[b'x'; 1024]).unwrap();

        let cursor = zip.finish().unwrap();
        let data = cursor.into_inner();

        let mut archive = zip::ZipArchive::new(Cursor::new(data)).unwrap();
        let result = validate_archive_security(&mut archive);
        assert!(
            result.is_ok(),
            "A 1 KB file must pass size validation: {:?}",
            result.err()
        );
    }

    /// Helper: create a minimal DOCX ZIP with the given XML as word/document.xml.
    fn create_test_docx(document_xml: &str) -> Vec<u8> {
        use std::io::{Cursor, Write};

        let buffer = Vec::new();
        let cursor = Cursor::new(buffer);
        let mut zip = zip::ZipWriter::new(cursor);
        let options = zip::write::FileOptions::<()>::default().compression_method(zip::CompressionMethod::Stored);

        zip.start_file("word/document.xml", options).unwrap();
        zip.write_all(document_xml.as_bytes()).unwrap();

        let cursor = zip.finish().unwrap();
        cursor.into_inner()
    }

    #[test]
    fn test_nested_table_parsing() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:body>
    <w:tbl>
      <w:tr>
        <w:tc>
          <w:p><w:r><w:t>Outer Cell 1</w:t></w:r></w:p>
          <w:tbl>
            <w:tr>
              <w:tc>
                <w:p><w:r><w:t>Inner Cell</w:t></w:r></w:p>
              </w:tc>
            </w:tr>
          </w:tbl>
        </w:tc>
        <w:tc>
          <w:p><w:r><w:t>Outer Cell 2</w:t></w:r></w:p>
        </w:tc>
      </w:tr>
    </w:tbl>
  </w:body>
</w:document>"#;

        let bytes = create_test_docx(xml);
        let mut budget = SecurityBudget::with_defaults();
        let doc = parse_document(&bytes, &mut budget).expect("parse_document should succeed");

        assert_eq!(doc.tables.len(), 1, "Expected exactly 1 (outer) table");

        let table = &doc.tables[0];
        assert_eq!(table.rows.len(), 1, "Outer table should have 1 row");
        assert_eq!(table.rows[0].cells.len(), 2, "Outer row should have 2 cells");

        let cell0 = &table.rows[0].cells[0];
        let cell0_texts: Vec<String> = cell0.paragraphs.iter().map(|p| p.to_text()).collect();
        assert!(
            cell0_texts.iter().any(|t| t.contains("Outer Cell 1")),
            "First cell must contain 'Outer Cell 1', got: {:?}",
            cell0_texts
        );
        assert!(
            cell0_texts.iter().any(|t| t.contains("Inner Cell")),
            "First cell must contain flattened 'Inner Cell', got: {:?}",
            cell0_texts
        );

        let cell1 = &table.rows[0].cells[1];
        let cell1_texts: Vec<String> = cell1.paragraphs.iter().map(|p| p.to_text()).collect();
        assert!(
            cell1_texts.iter().any(|t| t.contains("Outer Cell 2")),
            "Second cell must contain 'Outer Cell 2', got: {:?}",
            cell1_texts
        );
    }

    #[test]
    fn test_parser_loads_styles() {
        use std::io::{Cursor, Write};

        let styles_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<w:styles xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:style w:type="paragraph" w:styleId="Heading1">
    <w:name w:val="heading 1"/>
    <w:basedOn w:val="Normal"/>
    <w:pPr><w:outlineLvl w:val="0"/></w:pPr>
    <w:rPr><w:b/><w:sz w:val="32"/></w:rPr>
  </w:style>
  <w:style w:type="paragraph" w:default="1" w:styleId="Normal">
    <w:name w:val="Normal"/>
  </w:style>
</w:styles>"#;

        let doc_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:body>
    <w:p>
      <w:pPr><w:pStyle w:val="Heading1"/></w:pPr>
      <w:r><w:t>Hello</w:t></w:r>
    </w:p>
  </w:body>
</w:document>"#;

        let buffer = Vec::new();
        let cursor = Cursor::new(buffer);
        let mut zip = zip::ZipWriter::new(cursor);
        let options = zip::write::FileOptions::<()>::default().compression_method(zip::CompressionMethod::Stored);

        zip.start_file("word/document.xml", options).unwrap();
        zip.write_all(doc_xml.as_bytes()).unwrap();
        zip.start_file("word/styles.xml", options).unwrap();
        zip.write_all(styles_xml.as_bytes()).unwrap();

        let cursor = zip.finish().unwrap();
        let bytes = cursor.into_inner();

        let mut budget = SecurityBudget::with_defaults();
        let doc = parse_document(&bytes, &mut budget).expect("should parse");

        assert!(doc.style_catalog.is_some(), "Style catalog should be loaded");
        let catalog = doc.style_catalog.as_ref().unwrap();
        assert!(catalog.styles.contains_key("Heading1"));
        assert!(catalog.styles.contains_key("Normal"));

        let h1 = &catalog.styles["Heading1"];
        assert_eq!(h1.run_properties.bold, Some(true));
        assert_eq!(h1.run_properties.font_size_half_points, Some(32));
        assert_eq!(h1.paragraph_properties.outline_level, Some(0));
    }

    #[test]
    fn test_table_properties_integration() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:body>
    <w:tbl>
      <w:tblPr>
        <w:tblStyle w:val="TableGrid"/>
        <w:tblW w:w="5000" w:type="dxa"/>
        <w:jc w:val="center"/>
      </w:tblPr>
      <w:tblGrid>
        <w:gridCol w:w="2500"/>
        <w:gridCol w:w="2500"/>
      </w:tblGrid>
      <w:tr>
        <w:trPr>
          <w:tblHeader/>
        </w:trPr>
        <w:tc>
          <w:tcPr>
            <w:tcW w:w="2500" w:type="dxa"/>
            <w:shd w:val="clear" w:fill="D9E2F3"/>
          </w:tcPr>
          <w:p><w:r><w:t>Header 1</w:t></w:r></w:p>
        </w:tc>
        <w:tc>
          <w:tcPr>
            <w:tcW w:w="2500" w:type="dxa"/>
            <w:gridSpan w:val="1"/>
          </w:tcPr>
          <w:p><w:r><w:t>Header 2</w:t></w:r></w:p>
        </w:tc>
      </w:tr>
      <w:tr>
        <w:tc>
          <w:tcPr>
            <w:vMerge w:val="restart"/>
          </w:tcPr>
          <w:p><w:r><w:t>Merged</w:t></w:r></w:p>
        </w:tc>
        <w:tc>
          <w:p><w:r><w:t>Data</w:t></w:r></w:p>
        </w:tc>
      </w:tr>
    </w:tbl>
  </w:body>
</w:document>"#;

        let bytes = create_test_docx(xml);
        let mut budget = SecurityBudget::with_defaults();
        let doc = parse_document(&bytes, &mut budget).expect("parse should succeed");

        assert_eq!(doc.tables.len(), 1);
        let table = &doc.tables[0];

        let tbl_props = table.properties.as_ref().expect("table should have properties");
        assert_eq!(tbl_props.style_id.as_deref(), Some("TableGrid"));
        assert_eq!(tbl_props.alignment.as_deref(), Some("center"));
        assert!(tbl_props.width.is_some());
        assert_eq!(tbl_props.width.as_ref().unwrap().value, 5000);

        let grid = table.grid.as_ref().expect("table should have grid");
        assert_eq!(grid.columns, vec![2500, 2500]);

        let row0 = &table.rows[0];
        let row_props = row0.properties.as_ref().expect("header row should have properties");
        assert!(row_props.is_header);

        let cell00 = &row0.cells[0];
        let cell_props = cell00.properties.as_ref().expect("cell should have properties");
        assert!(cell_props.shading.is_some());
        assert_eq!(cell_props.shading.as_ref().unwrap().fill.as_deref(), Some("D9E2F3"));

        let cell10 = &table.rows[1].cells[0];
        let cell10_props = cell10.properties.as_ref().expect("merged cell should have properties");
        assert_eq!(
            cell10_props.v_merge,
            Some(crate::extraction::docx::table::VerticalMerge::Restart)
        );
    }

    #[test]
    fn test_table_with_explicit_header_row_renders_correctly() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:body>
    <w:tbl>
      <w:tr>
        <w:trPr>
          <w:tblHeader/>
        </w:trPr>
        <w:tc>
          <w:p><w:r><w:t>Name</w:t></w:r></w:p>
        </w:tc>
        <w:tc>
          <w:p><w:r><w:t>Age</w:t></w:r></w:p>
        </w:tc>
      </w:tr>
      <w:tr>
        <w:tc>
          <w:p><w:r><w:t>Alice</w:t></w:r></w:p>
        </w:tc>
        <w:tc>
          <w:p><w:r><w:t>30</w:t></w:r></w:p>
        </w:tc>
      </w:tr>
    </w:tbl>
  </w:body>
</w:document>"#;

        let bytes = create_test_docx(xml);
        let mut budget = SecurityBudget::with_defaults();
        let doc = parse_document(&bytes, &mut budget).expect("parse should succeed");

        assert_eq!(doc.tables.len(), 1);
        let table = &doc.tables[0];

        let row0_props = table.rows[0]
            .properties
            .as_ref()
            .expect("first row should have properties");
        assert!(row0_props.is_header, "First row should be marked as header");

        let markdown = table.to_markdown();
        let lines: Vec<&str> = markdown.lines().collect();

        assert!(
            lines.len() >= 3,
            "Table should have at least 3 lines, got: {}",
            markdown
        );

        assert!(
            lines[1].contains("---"),
            "Second line should be separator, got: {}",
            lines[1]
        );
    }

    #[test]
    fn test_table_with_merged_cells_expands_columns() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:body>
    <w:tbl>
      <w:tr>
        <w:tc>
          <w:p><w:r><w:t>A</w:t></w:r></w:p>
        </w:tc>
        <w:tc>
          <w:p><w:r><w:t>B</w:t></w:r></w:p>
        </w:tc>
      </w:tr>
      <w:tr>
        <w:tc>
          <w:tcPr>
            <w:gridSpan w:val="2"/>
          </w:tcPr>
          <w:p><w:r><w:t>Merged</w:t></w:r></w:p>
        </w:tc>
      </w:tr>
    </w:tbl>
  </w:body>
</w:document>"#;

        let bytes = create_test_docx(xml);
        let mut budget = SecurityBudget::with_defaults();
        let doc = parse_document(&bytes, &mut budget).expect("parse should succeed");

        assert_eq!(doc.tables.len(), 1);
        let table = &doc.tables[0];

        let merged_cell = &table.rows[1].cells[0];
        let cell_props = merged_cell.properties.as_ref().expect("cell should have properties");
        assert_eq!(cell_props.grid_span, Some(2), "Cell should have grid_span=2");

        let markdown = table.to_markdown();
        let lines: Vec<&str> = markdown.lines().collect();

        let pipes_row0 = lines[0].matches('|').count();
        let pipes_row1 = lines[2].matches('|').count();

        assert_eq!(
            pipes_row0, pipes_row1,
            "All rows should have same column count in markdown"
        );
    }

    /// Helper: parse document XML through DocxParser and return the Document.
    fn parse_xml(xml: &str) -> Document {
        let parser_struct = DocxParser {
            archive: zip::ZipArchive::new(std::io::Cursor::new(create_minimal_zip())).unwrap(),
            relationships: AHashMap::new(),
            styles: None,
            theme: None,
        };
        let mut document = Document::new();
        {
            let mut budget = crate::extractors::security::SecurityBudget::with_defaults();
            parser_struct
                .parse_document_xml(xml, &mut document, &mut budget)
                .unwrap();
        }
        document
    }

    /// Helper: parse document XML through DocxParser, threading a caller-supplied
    /// budget instead of a fresh default one, so the caller can inspect the budget's
    /// residual state (e.g. leaked nesting depth, GH#1395) after parsing completes.
    fn parse_xml_with_budget(xml: &str, budget: &mut SecurityBudget) -> Document {
        let parser_struct = DocxParser {
            archive: zip::ZipArchive::new(std::io::Cursor::new(create_minimal_zip())).unwrap(),
            relationships: AHashMap::new(),
            styles: None,
            theme: None,
        };
        let mut document = Document::new();
        parser_struct.parse_document_xml(xml, &mut document, budget).unwrap();
        document
    }

    /// Helper: parse document XML through DocxParser, returning the raw `Result`
    /// instead of unwrapping (GH#384) — needed to assert on the exact error variant
    /// produced once nesting inside a delegating helper's subtree trips the depth cap.
    fn try_parse_xml_with_budget(xml: &str, budget: &mut SecurityBudget) -> Result<Document, DocxParseError> {
        let parser_struct = DocxParser {
            archive: zip::ZipArchive::new(std::io::Cursor::new(create_minimal_zip())).unwrap(),
            relationships: AHashMap::new(),
            styles: None,
            theme: None,
        };
        let mut document = Document::new();
        parser_struct.parse_document_xml(xml, &mut document, budget)?;
        Ok(document)
    }

    /// Test-only probe (GH#1395): count how many more `budget.enter()` calls succeed
    /// before the depth cap trips. A freshly-built budget accepts exactly `max_depth`
    /// more entries; if parsing leaked N depth levels, only `max_depth - N` succeed.
    /// This makes the *value* of the private depth counter observable through the
    /// budget's existing public API, without touching `extractors/security.rs`.
    fn probe_remaining_depth(budget: &mut SecurityBudget, max_depth: usize) -> usize {
        let mut successes = 0usize;
        for _ in 0..=max_depth {
            if budget.enter().is_ok() {
                successes += 1;
            } else {
                break;
            }
        }
        successes
    }

    /// GH#1395: a single `w:tblPr` inside a table must not leave the depth counter
    /// above zero after parsing — `parse_table_properties` consumes its own
    /// `</w:tblPr>`, so the outer loop's `Event::End` arm never runs for it.
    #[test]
    fn should_reset_depth_counter_to_zero_after_parsing_tblpr() {
        let limits = crate::extractors::security::SecurityLimits {
            max_nesting_depth: 64,
            max_xml_depth: 64,
            ..Default::default()
        };
        let mut budget = SecurityBudget::from_limits(&limits);
        let xml = wrap_body(
            r#"<w:tbl>
                <w:tblPr><w:tblStyle w:val="TableGrid"/></w:tblPr>
                <w:tr><w:tc><w:p><w:r><w:t>x</w:t></w:r></w:p></w:tc></w:tr>
            </w:tbl>"#,
        );
        parse_xml_with_budget(&xml, &mut budget);
        assert_eq!(
            probe_remaining_depth(&mut budget, 64),
            64,
            "w:tblPr must not leak a depth level"
        );
    }

    /// GH#1395: same as `w:tblPr` — `parse_table_grid` consumes its own `</w:tblGrid>`.
    #[test]
    fn should_reset_depth_counter_to_zero_after_parsing_tblgrid() {
        let limits = crate::extractors::security::SecurityLimits {
            max_nesting_depth: 64,
            max_xml_depth: 64,
            ..Default::default()
        };
        let mut budget = SecurityBudget::from_limits(&limits);
        let xml = wrap_body(
            r#"<w:tbl>
                <w:tblGrid><w:gridCol w:w="2500"/></w:tblGrid>
                <w:tr><w:tc><w:p><w:r><w:t>x</w:t></w:r></w:p></w:tc></w:tr>
            </w:tbl>"#,
        );
        parse_xml_with_budget(&xml, &mut budget);
        assert_eq!(
            probe_remaining_depth(&mut budget, 64),
            64,
            "w:tblGrid must not leak a depth level"
        );
    }

    /// GH#1395: `parse_row_properties` consumes its own `</w:trPr>`.
    #[test]
    fn should_reset_depth_counter_to_zero_after_parsing_trpr() {
        let limits = crate::extractors::security::SecurityLimits {
            max_nesting_depth: 64,
            max_xml_depth: 64,
            ..Default::default()
        };
        let mut budget = SecurityBudget::from_limits(&limits);
        let xml = wrap_body(
            r#"<w:tbl>
                <w:tr>
                    <w:trPr><w:tblHeader/></w:trPr>
                    <w:tc><w:p><w:r><w:t>x</w:t></w:r></w:p></w:tc>
                </w:tr>
            </w:tbl>"#,
        );
        parse_xml_with_budget(&xml, &mut budget);
        assert_eq!(
            probe_remaining_depth(&mut budget, 64),
            64,
            "w:trPr must not leak a depth level"
        );
    }

    /// GH#1395: `parse_cell_properties` consumes its own `</w:tcPr>`.
    #[test]
    fn should_reset_depth_counter_to_zero_after_parsing_tcpr() {
        let limits = crate::extractors::security::SecurityLimits {
            max_nesting_depth: 64,
            max_xml_depth: 64,
            ..Default::default()
        };
        let mut budget = SecurityBudget::from_limits(&limits);
        let xml = wrap_body(
            r#"<w:tbl>
                <w:tr>
                    <w:tc>
                        <w:tcPr><w:tcW w:w="2500" w:type="dxa"/></w:tcPr>
                        <w:p><w:r><w:t>x</w:t></w:r></w:p>
                    </w:tc>
                </w:tr>
            </w:tbl>"#,
        );
        parse_xml_with_budget(&xml, &mut budget);
        assert_eq!(
            probe_remaining_depth(&mut budget, 64),
            64,
            "w:tcPr must not leak a depth level"
        );
    }

    /// GH#1395: `parse_drawing` consumes its own `</w:drawing>`.
    #[test]
    fn should_reset_depth_counter_to_zero_after_parsing_drawing() {
        let limits = crate::extractors::security::SecurityLimits {
            max_nesting_depth: 64,
            max_xml_depth: 64,
            ..Default::default()
        };
        let mut budget = SecurityBudget::from_limits(&limits);
        let xml = wrap_body(
            r#"<w:p><w:r>
                <w:drawing>
                    <wp:inline xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing">
                        <wp:extent cx="914400" cy="914400"/>
                        <wp:docPr id="1" name="Picture 1"/>
                    </wp:inline>
                </w:drawing>
            </w:r></w:p>"#,
        );
        parse_xml_with_budget(&xml, &mut budget);
        assert_eq!(
            probe_remaining_depth(&mut budget, 64),
            64,
            "w:drawing must not leak a depth level"
        );
    }

    /// GH#1395: `parse_section_properties_streaming` consumes its own `</w:sectPr>`.
    #[test]
    fn should_reset_depth_counter_to_zero_after_parsing_sectpr() {
        let limits = crate::extractors::security::SecurityLimits {
            max_nesting_depth: 64,
            max_xml_depth: 64,
            ..Default::default()
        };
        let mut budget = SecurityBudget::from_limits(&limits);
        let xml = wrap_body(
            r#"<w:p><w:r><w:t>Content</w:t></w:r></w:p>
            <w:sectPr>
                <w:pgSz w:w="12240" w:h="15840"/>
            </w:sectPr>"#,
        );
        parse_xml_with_budget(&xml, &mut budget);
        assert_eq!(
            probe_remaining_depth(&mut budget, 64),
            64,
            "w:sectPr must not leak a depth level"
        );
    }

    /// GH#1395: `m:oMath` and `m:oMathPara` already thread `budget` through to
    /// `collect_and_convert_omath{,_para}`, which recursively balance every
    /// `enter()`/`leave()` pair themselves — including the enter made by the
    /// outer loop for the `m:oMath`/`m:oMathPara` start tag itself, refunded when
    /// the recursive `collect_children` consumes the matching end tag. Confirm
    /// they are NOT double-leaking (over-refunding would also be a bug: it would
    /// silently mask genuinely deep documents by under-counting).
    #[test]
    fn should_reset_depth_counter_to_zero_after_parsing_omath_para() {
        let limits = crate::extractors::security::SecurityLimits {
            max_nesting_depth: 64,
            max_xml_depth: 64,
            ..Default::default()
        };
        let mut budget = SecurityBudget::from_limits(&limits);
        let xml = wrap_body(
            r#"<w:p><w:r>
                <m:oMathPara xmlns:m="http://schemas.openxmlformats.org/officeDocument/2006/math">
                    <m:oMath>
                        <m:r><m:rPr><m:sty m:val="p"/></m:rPr><m:t>x</m:t></m:r>
                        <m:sSup>
                            <m:e><m:r><m:t>y</m:t></m:r></m:e>
                            <m:sup><m:r><m:t>2</m:t></m:r></m:sup>
                        </m:sSup>
                    </m:oMath>
                </m:oMathPara>
            </w:r></w:p>"#,
        );
        parse_xml_with_budget(&xml, &mut budget);
        assert_eq!(
            probe_remaining_depth(&mut budget, 64),
            64,
            "m:oMathPara/m:oMath (with nested m:rPr and m:sSup) must not leak a depth level"
        );
    }

    /// GH#1395: the reporter's reproducer — a flat one-column table whose real
    /// nesting depth is small (~8) leaked `2*rows + 3` levels because every row's
    /// `w:tcPr` and `w:trPr` each leaked one level. At 600 rows that is 1203 leaked
    /// levels, enough to trip the default 1024-level cap outright with no partial
    /// result. Assert the fixed parser extracts the full table at both 400 and 600
    /// rows, with identical, exact structural counts — proving the row count no
    /// longer influences whether extraction succeeds.
    #[test]
    fn should_extract_full_table_regardless_of_row_count_after_fixing_depth_leak() {
        fn build_table_xml(rows: usize) -> String {
            // Deliberately `<w:tblPr></w:tblPr>` (Start+End events), not the
            // self-closing `<w:tblPr/>` form (a single Empty event) — only the
            // Start/End form goes through the leaking `parse_table_properties`
            // delegation path this test is guarding against.
            let mut body = String::from("<w:tbl><w:tblPr></w:tblPr><w:tblGrid><w:gridCol w:w=\"2500\"/></w:tblGrid>");
            for i in 0..rows {
                body.push_str(&format!(
                    "<w:tr><w:trPr></w:trPr><w:tc><w:tcPr></w:tcPr><w:p><w:r><w:t>row{i}</w:t></w:r></w:p></w:tc></w:tr>"
                ));
            }
            body.push_str("</w:tbl>");
            wrap_body(&body)
        }

        for rows in [400usize, 600usize] {
            let mut budget = SecurityBudget::with_defaults();
            let xml = build_table_xml(rows);
            let doc = parse_xml_with_budget(&xml, &mut budget);

            assert_eq!(doc.tables.len(), 1, "rows={rows}: expected exactly 1 table");
            assert_eq!(doc.tables[0].rows.len(), rows, "rows={rows}: row count mismatch");
            assert_eq!(
                doc.elements.len(),
                1,
                "rows={rows}: a single top-level table must yield exactly 1 document element"
            );
            assert_eq!(
                probe_remaining_depth(&mut budget, 1024),
                1024,
                "rows={rows}: depth counter must return to zero after a {rows}-row table"
            );
        }
    }

    /// GH#384: the GH#1395 fix only stopped the depth counter from *leaking* at each
    /// delegating call site — it left the content *inside* those helpers completely
    /// unmeasured. `parse_drawing` is one of them: before this fix it read through its
    /// own `</w:drawing>` without ever calling `budget.enter()`/`leave()` for anything
    /// nested inside. Confirm a genuinely deeply-nested drawing subtree now trips the
    /// depth cap instead of sailing through unaccounted, and that the exact error
    /// variant surfaces (not just "parsing failed somehow").
    #[test]
    fn should_trip_nesting_too_deep_when_drawing_subtree_nests_beyond_the_depth_cap() {
        let limits = crate::extractors::security::SecurityLimits {
            max_nesting_depth: 10,
            max_xml_depth: 10,
            ..Default::default()
        };
        let mut budget = SecurityBudget::from_limits(&limits);

        let nest_depth = 20;
        let mut inner = String::from("<a:sp>");
        for _ in 0..nest_depth {
            inner.push_str("<a:grp>");
        }
        for _ in 0..nest_depth {
            inner.push_str("</a:grp>");
        }
        inner.push_str("</a:sp>");

        let xml = wrap_body(&format!(
            "<w:p><w:r><w:drawing><wp:inline>{inner}</wp:inline></w:drawing></w:r></w:p>"
        ));

        let result = try_parse_xml_with_budget(&xml, &mut budget);

        match result {
            Err(DocxParseError::SecurityLimit(msg)) => {
                assert!(
                    msg.contains("Nesting too deep"),
                    "expected a nesting-depth security error, got: {msg}"
                );
            }
            other => {
                panic!("expected Err(DocxParseError::SecurityLimit(_)) for a deeply-nested drawing, got {other:?}")
            }
        }
    }

    /// GH#384: same gap as `parse_drawing`, but for `parse_cell_properties` — nesting
    /// inside a `w:tcPr` subtree was completely invisible to the depth budget before
    /// this fix. Confirm it now trips the depth cap with the exact `SecurityLimit`
    /// error variant.
    #[test]
    fn should_trip_nesting_too_deep_when_tcpr_subtree_nests_beyond_the_depth_cap() {
        let limits = crate::extractors::security::SecurityLimits {
            max_nesting_depth: 10,
            max_xml_depth: 10,
            ..Default::default()
        };
        let mut budget = SecurityBudget::from_limits(&limits);

        let nest_depth = 20;
        let mut inner = String::new();
        for _ in 0..nest_depth {
            inner.push_str("<evil>");
        }
        for _ in 0..nest_depth {
            inner.push_str("</evil>");
        }

        let xml = wrap_body(&format!(
            "<w:tbl><w:tr><w:tc><w:tcPr>{inner}</w:tcPr><w:p><w:r><w:t>x</w:t></w:r></w:p></w:tc></w:tr></w:tbl>"
        ));

        let result = try_parse_xml_with_budget(&xml, &mut budget);

        match result {
            Err(DocxParseError::SecurityLimit(msg)) => {
                assert!(
                    msg.contains("Nesting too deep"),
                    "expected a nesting-depth security error, got: {msg}"
                );
            }
            other => panic!("expected Err(DocxParseError::SecurityLimit(_)) for a deeply-nested tcPr, got {other:?}"),
        }
    }

    /// a25335db0a threaded `budget` through every DOCX delegating helper except
    /// `parse_vml_pict`: it consumes its own `</w:pict>` end tag with a private
    /// `depth` counter, so the main loop's `Event::End` arm never sees the closing
    /// tag and `budget.leave()` is never called for it. Confirm the fixed parser
    /// leaves the depth counter at exactly zero after parsing a single `w:pict`.
    #[test]
    fn should_reset_depth_counter_to_zero_after_parsing_pict() {
        let limits = crate::extractors::security::SecurityLimits {
            max_nesting_depth: 64,
            max_xml_depth: 64,
            ..Default::default()
        };
        let mut budget = SecurityBudget::from_limits(&limits);
        let xml = wrap_body(
            r#"<w:p><w:r>
                <w:pict><v:shape xmlns:v="urn:schemas-microsoft-com:vml"><v:textbox><w:txbxContent>
                    <w:p><w:r><w:t>Legacy VML text box.</w:t></w:r></w:p>
                </w:txbxContent></v:textbox></v:shape></w:pict>
            </w:r></w:p>"#,
        );
        parse_xml_with_budget(&xml, &mut budget);
        assert_eq!(
            probe_remaining_depth(&mut budget, 64),
            64,
            "w:pict must not leak a depth level"
        );
    }

    /// Regression test for the `w:pict` depth leak (a25335db0a): before this fix,
    /// each `<w:pict>` element permanently leaked one depth unit (the main loop's
    /// `enter()` for its opening tag was never matched by a `leave()`), even though
    /// each element's *real* nesting is trivial and never overlaps with the next.
    /// Legacy `.doc`-to-`.docx` conversions and documents saved by older Word
    /// versions commonly contain many `w:pict` elements, so this leak would
    /// eventually trip `SecurityError::NestingTooDeep` on a perfectly legitimate
    /// document. Use a count comfortably above the default `max_nesting_depth`
    /// (1024) so the unfixed code genuinely fails this test.
    #[test]
    fn should_extract_all_pict_textboxes_regardless_of_element_count_after_fixing_depth_leak() {
        fn build_pict_xml(count: usize) -> String {
            let mut body = String::new();
            for i in 0..count {
                body.push_str(&format!(
                    r#"<w:p><w:r><w:pict><v:shape xmlns:v="urn:schemas-microsoft-com:vml"><v:textbox><w:txbxContent>
                        <w:p><w:r><w:t>pict{i}</w:t></w:r></w:p>
                    </w:txbxContent></v:textbox></v:shape></w:pict></w:r></w:p>"#
                ));
            }
            wrap_body(&body)
        }

        let count = 1500;
        let mut budget = SecurityBudget::with_defaults();
        let xml = build_pict_xml(count);
        let doc = try_parse_xml_with_budget(&xml, &mut budget)
            .expect("legacy VML w:pict content must not trip the depth cap");

        assert_eq!(
            doc.drawings.len(),
            count,
            "expected exactly {count} extracted text boxes"
        );
        assert_eq!(
            doc.paragraphs.len(),
            count,
            "each w:pict is wrapped in its own top-level w:p, so paragraph count must match"
        );
        assert_eq!(
            probe_remaining_depth(&mut budget, 1024),
            1024,
            "depth counter must return to zero after {count} w:pict elements"
        );
    }

    /// GH#384-style gap for `w:pict`: before this fix, none of `parse_vml_pict`,
    /// `parse_vml_textbox`, or `collect_txbx_content_text` called `budget.step()`,
    /// so content nested inside a `w:pict` was completely exempt from the
    /// iteration-count limit — a single `w:pict` padded with many trivial elements
    /// was processed with zero budget enforcement. Confirm the fixed parser now
    /// trips `SecurityError::TooManyIterations` once padding inside `w:pict`
    /// exceeds the configured cap, with the exact error variant surfaced.
    #[test]
    fn should_trip_too_many_iterations_when_pict_subtree_is_padded_beyond_the_iteration_cap() {
        let limits = crate::extractors::security::SecurityLimits {
            max_iterations: 50,
            ..Default::default()
        };
        let mut budget = SecurityBudget::from_limits(&limits);

        let padding_count = 200;
        let mut inner = String::new();
        for _ in 0..padding_count {
            inner.push_str("<v:fill/>");
        }

        let xml = wrap_body(&format!(
            r#"<w:p><w:r><w:pict><v:shape xmlns:v="urn:schemas-microsoft-com:vml">
                {inner}
            </v:shape></w:pict></w:r></w:p>"#
        ));

        let result = try_parse_xml_with_budget(&xml, &mut budget);

        match result {
            Err(DocxParseError::SecurityLimit(msg)) => {
                assert!(
                    msg.contains("Too many iterations"),
                    "expected an iteration-count security error, got: {msg}"
                );
            }
            other => {
                panic!("expected Err(DocxParseError::SecurityLimit(_)) for a padded w:pict subtree, got {other:?}")
            }
        }
    }

    /// GH#384: threading `budget` into the table/drawing/section delegating helpers
    /// must not turn ordinary, real-world-shaped documents into false rejections.
    /// Parses a table with populated `tblPr`/`tblGrid`/`trPr`/`tcPr`, an inline
    /// drawing, and a section — all well within the default depth cap — and asserts
    /// the structural results are still fully populated, not just that parsing
    /// returned `Ok`.
    #[test]
    fn should_extract_realistic_table_and_drawing_without_false_rejection() {
        let xml = wrap_body(
            r#"<w:tbl>
                <w:tblPr><w:tblStyle w:val="TableGrid"/><w:tblW w:w="5000" w:type="dxa"/></w:tblPr>
                <w:tblGrid><w:gridCol w:w="2500"/><w:gridCol w:w="2500"/></w:tblGrid>
                <w:tr>
                    <w:trPr><w:tblHeader/></w:trPr>
                    <w:tc>
                        <w:tcPr>
                            <w:tcW w:w="2500" w:type="dxa"/>
                            <w:shd w:val="clear" w:color="auto" w:fill="D9E2F3"/>
                        </w:tcPr>
                        <w:p><w:r><w:t>Header A</w:t></w:r></w:p>
                    </w:tc>
                    <w:tc>
                        <w:tcPr><w:tcW w:w="2500" w:type="dxa"/></w:tcPr>
                        <w:p><w:r><w:t>Header B</w:t></w:r></w:p>
                    </w:tc>
                </w:tr>
                <w:tr>
                    <w:tc>
                        <w:tcPr><w:tcW w:w="2500" w:type="dxa"/></w:tcPr>
                        <w:p><w:r><w:t>Row 1</w:t></w:r></w:p>
                    </w:tc>
                    <w:tc>
                        <w:tcPr><w:tcW w:w="2500" w:type="dxa"/></w:tcPr>
                        <w:p><w:r><w:t>Value</w:t></w:r></w:p>
                    </w:tc>
                </w:tr>
            </w:tbl>
            <w:p><w:r>
                <w:drawing>
                    <wp:inline xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing">
                        <wp:extent cx="914400" cy="457200"/>
                        <wp:docPr id="1" name="Picture 1"/>
                    </wp:inline>
                </w:drawing>
            </w:r></w:p>
            <w:sectPr>
                <w:pgSz w:w="12240" w:h="15840"/>
                <w:pgMar w:top="1440" w:right="1440" w:bottom="1440" w:left="1440"
                         w:header="720" w:footer="720" w:gutter="0"/>
            </w:sectPr>"#,
        );

        let mut budget = SecurityBudget::with_defaults();
        let doc = try_parse_xml_with_budget(&xml, &mut budget)
            .expect("a realistic table + drawing + section must not trip the default depth cap");

        assert_eq!(doc.tables.len(), 1, "expected exactly one table");
        assert_eq!(doc.tables[0].rows.len(), 2, "expected a header row and a data row");
        assert!(
            doc.tables[0].properties.is_some(),
            "table properties must still be populated"
        );
        assert!(doc.tables[0].grid.is_some(), "table grid must still be populated");
        assert_eq!(doc.drawings.len(), 1, "expected exactly one drawing");
        assert_eq!(doc.sections.len(), 1, "expected exactly one section");
        assert_eq!(
            doc.sections[0].page_width_twips,
            Some(12240),
            "section properties must still be populated"
        );
    }

    /// Helper: parse document XML with custom relationships.
    fn parse_xml_with_rels(xml: &str, rels: AHashMap<String, String>) -> Document {
        let parser_struct = DocxParser {
            archive: zip::ZipArchive::new(std::io::Cursor::new(create_minimal_zip())).unwrap(),
            relationships: rels,
            styles: None,
            theme: None,
        };
        let mut document = Document::new();
        {
            let mut budget = crate::extractors::security::SecurityBudget::with_defaults();
            parser_struct
                .parse_document_xml(xml, &mut document, &mut budget)
                .unwrap();
        }
        document
    }

    /// Wrap body XML in a w:document envelope.
    fn wrap_body(body: &str) -> String {
        format!(
            r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><w:body>{}</w:body></w:document>"#,
            body
        )
    }

    #[test]
    fn test_plain_paragraph_text() {
        let xml = wrap_body(r#"<w:p><w:r><w:t>Hello World</w:t></w:r></w:p>"#);
        let doc = parse_xml(&xml);
        assert_eq!(doc.paragraphs.len(), 1);
        assert_eq!(doc.paragraphs[0].to_text(), "Hello World");
    }

    #[test]
    fn test_multiple_paragraphs() {
        let xml = wrap_body(
            r#"<w:p><w:r><w:t>First</w:t></w:r></w:p>
               <w:p><w:r><w:t>Second</w:t></w:r></w:p>
               <w:p><w:r><w:t>Third</w:t></w:r></w:p>"#,
        );
        let doc = parse_xml(&xml);
        assert_eq!(doc.paragraphs.len(), 3);
        assert_eq!(doc.paragraphs[0].to_text(), "First");
        assert_eq!(doc.paragraphs[1].to_text(), "Second");
        assert_eq!(doc.paragraphs[2].to_text(), "Third");

        let plain = doc.to_plain_text();
        assert!(plain.contains("First"));
        assert!(plain.contains("Second"));
        assert!(plain.contains("Third"));
    }

    #[test]
    fn test_empty_paragraph() {
        let xml = wrap_body(r#"<w:p></w:p>"#);
        let doc = parse_xml(&xml);
        assert_eq!(doc.paragraphs.len(), 1);
        assert_eq!(doc.paragraphs[0].to_text(), "");
    }

    #[test]
    fn test_multiple_runs_in_paragraph() {
        let xml = wrap_body(
            r#"<w:p>
                <w:r><w:t>Hello </w:t></w:r>
                <w:r><w:t>World</w:t></w:r>
            </w:p>"#,
        );
        let doc = parse_xml(&xml);
        assert_eq!(doc.paragraphs[0].to_text(), "Hello World");
    }

    #[test]
    fn test_line_break_in_run() {
        let xml = wrap_body(r#"<w:p><w:r><w:t>Before</w:t><w:br/><w:t>After</w:t></w:r></w:p>"#);
        let doc = parse_xml(&xml);
        let text = doc.paragraphs[0].to_text();
        assert!(text.contains("Before"));
        assert!(text.contains("After"));
        assert!(text.contains('\n'));
    }

    /// Word writes both `<w:br w:type="page"/>` (the author's break) and, at the
    /// start of the first run on the new page, `<w:lastRenderedPageBreak/>` (its own
    /// record of the same transition). Counting both inflates the page total and
    /// produces a spurious zero-length page between them (GH#1416).
    #[test]
    fn should_yield_one_page_boundary_when_manual_break_is_followed_by_last_rendered_hint() {
        let xml = wrap_body(
            r#"<w:p><w:r><w:t>Page one text</w:t><w:br w:type="page"/></w:r></w:p>
               <w:p><w:r><w:lastRenderedPageBreak/><w:t>Page two text</w:t></w:r></w:p>"#,
        );
        let doc = parse_xml(&xml);

        let page_break_count = doc
            .elements
            .iter()
            .filter(|e| matches!(e, DocumentElement::PageBreak))
            .count();
        assert_eq!(
            page_break_count, 1,
            "the render hint duplicates the manual break and must not be counted again"
        );

        let (text, boundaries) = doc.extract_text_with_boundaries(true, true);
        assert_eq!(boundaries.len(), 2, "one break must produce exactly two pages");
        assert_eq!(boundaries[0].page_number, 1);
        assert_eq!(boundaries[1].page_number, 2);
        assert_ne!(
            boundaries[0].byte_start, boundaries[0].byte_end,
            "page 1 must not be reported as a zero-length blank page"
        );
        assert!(text[boundaries[0].byte_start..boundaries[0].byte_end].contains("Page one text"));
        assert!(text[boundaries[1].byte_start..boundaries[1].byte_end].contains("Page two text"));
    }

    /// A document Word paginated itself (no manual `w:br` at all) carries only
    /// `w:lastRenderedPageBreak` hints. The fix for GH#1416 must not drop those
    /// outright — that's the only page-break signal such a document has.
    #[test]
    fn should_paginate_when_document_has_only_last_rendered_page_break_hints() {
        let xml = wrap_body(
            r#"<w:p><w:r><w:t>Page one text</w:t></w:r></w:p>
               <w:p><w:r><w:lastRenderedPageBreak/><w:t>Page two text</w:t></w:r></w:p>"#,
        );
        let doc = parse_xml(&xml);

        let page_break_count = doc
            .elements
            .iter()
            .filter(|e| matches!(e, DocumentElement::PageBreak))
            .count();
        assert_eq!(
            page_break_count, 1,
            "the sole render hint must still register a page break"
        );

        let (text, boundaries) = doc.extract_text_with_boundaries(true, true);
        assert_eq!(boundaries.len(), 2);
        assert_eq!(boundaries[0].page_number, 1);
        assert_eq!(boundaries[1].page_number, 2);
        assert!(text[boundaries[0].byte_start..boundaries[0].byte_end].contains("Page one text"));
        assert!(text[boundaries[1].byte_start..boundaries[1].byte_end].contains("Page two text"));
    }

    /// A page break inside a table cannot be written into the rendered markdown
    /// without splitting the table in half, so it must be deferred to right after
    /// the completed `DocumentElement::Table` — not dropped (GH#1419).
    #[test]
    fn should_place_deferred_break_after_table_when_break_occurs_inside_table() {
        let xml = wrap_body(
            r#"<w:p><w:r><w:t>Before table</w:t></w:r></w:p>
               <w:tbl>
                 <w:tblPr></w:tblPr>
                 <w:tblGrid><w:gridCol w:w="2000"/></w:tblGrid>
                 <w:tr><w:tc><w:tcPr><w:tcW w:w="2000" w:type="dxa"/></w:tcPr>
                     <w:p><w:r><w:t>Cell one</w:t><w:br w:type="page"/></w:r></w:p>
                 </w:tc></w:tr>
                 <w:tr><w:tc><w:tcPr><w:tcW w:w="2000" w:type="dxa"/></w:tcPr>
                     <w:p><w:r><w:t>Cell two</w:t></w:r></w:p>
                 </w:tc></w:tr>
               </w:tbl>
               <w:p><w:r><w:t>After table</w:t></w:r></w:p>"#,
        );
        let doc = parse_xml(&xml);

        assert_eq!(doc.tables.len(), 1);
        let table_markdown = doc.tables[0].to_markdown();
        assert!(
            !table_markdown.contains('\x0c'),
            "the table markdown must not contain a form feed: {table_markdown}"
        );
        assert!(table_markdown.contains("Cell one"));
        assert!(table_markdown.contains("Cell two"));

        // Elements must read: Paragraph, Table, PageBreak, Paragraph — the break comes
        // *after* the table, not inside it and not dropped.
        let kinds: Vec<&str> = doc
            .elements
            .iter()
            .map(|e| match e {
                DocumentElement::Paragraph(_) => "paragraph",
                DocumentElement::Table(_) => "table",
                DocumentElement::PageBreak => "page_break",
                _ => "other",
            })
            .collect();
        assert_eq!(kinds, vec!["paragraph", "table", "page_break", "paragraph"]);

        let (text, boundaries) = doc.extract_text_with_boundaries(true, true);
        assert_eq!(
            boundaries.len(),
            2,
            "the table-internal break must still produce two pages"
        );
        let page_one = &text[boundaries[0].byte_start..boundaries[0].byte_end];
        let page_two = &text[boundaries[1].byte_start..boundaries[1].byte_end];
        assert!(page_one.contains("Before table"));
        assert!(page_one.contains("Cell one"));
        assert!(
            page_one.contains("Cell two"),
            "the table must not be split across the boundary"
        );
        assert!(page_two.contains("After table"));
    }

    /// A break inside a *nested* table must be deferred just like a top-level one,
    /// but flushed exactly once when the outermost `</w:tbl>` closes — not once per
    /// nesting level (GH#1419).
    #[test]
    fn should_not_double_count_page_break_inside_nested_table() {
        let xml = wrap_body(
            r#"<w:p><w:r><w:t>Before table</w:t></w:r></w:p>
               <w:tbl>
                 <w:tblPr></w:tblPr>
                 <w:tblGrid><w:gridCol w:w="2000"/></w:tblGrid>
                 <w:tr><w:tc><w:tcPr><w:tcW w:w="2000" w:type="dxa"/></w:tcPr>
                     <w:tbl>
                       <w:tblPr></w:tblPr>
                       <w:tblGrid><w:gridCol w:w="1000"/></w:tblGrid>
                       <w:tr><w:tc><w:tcPr><w:tcW w:w="1000" w:type="dxa"/></w:tcPr>
                           <w:p><w:r><w:t>Inner cell</w:t><w:br w:type="page"/></w:r></w:p>
                       </w:tc></w:tr>
                     </w:tbl>
                 </w:tc></w:tr>
               </w:tbl>
               <w:p><w:r><w:t>After table</w:t></w:r></w:p>"#,
        );
        let doc = parse_xml(&xml);

        assert_eq!(
            doc.tables.len(),
            1,
            "the inner table is flattened into the outer cell, not a separate element"
        );
        let page_break_count = doc
            .elements
            .iter()
            .filter(|e| matches!(e, DocumentElement::PageBreak))
            .count();
        assert_eq!(
            page_break_count, 1,
            "the break must be flushed once at the outermost table close, not once per nesting level"
        );

        let (_, boundaries) = doc.extract_text_with_boundaries(true, true);
        assert_eq!(boundaries.len(), 2);
    }

    /// Word writes the render hint for a break it already saw even when that break
    /// was inside a table: the hint appears right after the table in the next
    /// paragraph, with no real content between the deferred break and the hint.
    /// That must still collapse to a single boundary (GH#1416 + GH#1419 combined).
    #[test]
    fn should_dedupe_last_rendered_hint_immediately_following_deferred_table_break() {
        let xml = wrap_body(
            r#"<w:p><w:r><w:t>Before table</w:t></w:r></w:p>
               <w:tbl>
                 <w:tblPr></w:tblPr>
                 <w:tblGrid><w:gridCol w:w="2000"/></w:tblGrid>
                 <w:tr><w:tc><w:tcPr><w:tcW w:w="2000" w:type="dxa"/></w:tcPr>
                     <w:p><w:r><w:t>Cell one</w:t><w:br w:type="page"/></w:r></w:p>
                 </w:tc></w:tr>
               </w:tbl>
               <w:p><w:r><w:lastRenderedPageBreak/><w:t>After table</w:t></w:r></w:p>"#,
        );
        let doc = parse_xml(&xml);

        let page_break_count = doc
            .elements
            .iter()
            .filter(|e| matches!(e, DocumentElement::PageBreak))
            .count();
        assert_eq!(
            page_break_count, 1,
            "the hint right after the table duplicates the deferred table break"
        );

        let (text, boundaries) = doc.extract_text_with_boundaries(true, true);
        assert_eq!(boundaries.len(), 2);
        assert_ne!(
            boundaries[0].byte_start, boundaries[0].byte_end,
            "page 1 must not be reported as a zero-length blank page"
        );
        assert!(text[boundaries[1].byte_start..boundaries[1].byte_end].contains("After table"));
    }

    #[test]
    fn test_bold_formatting() {
        let xml = wrap_body(r#"<w:p><w:r><w:rPr><w:b/></w:rPr><w:t>Bold</w:t></w:r></w:p>"#);
        let doc = parse_xml(&xml);
        assert!(doc.paragraphs[0].runs[0].bold);
        let md = doc.to_markdown(true);
        assert!(md.contains("**Bold**"), "Markdown: {}", md);
    }

    #[test]
    fn test_bold_disabled_with_val_0() {
        let xml = wrap_body(r#"<w:p><w:r><w:rPr><w:b w:val="0"/></w:rPr><w:t>Not Bold</w:t></w:r></w:p>"#);
        let doc = parse_xml(&xml);
        assert!(!doc.paragraphs[0].runs[0].bold);
    }

    #[test]
    fn test_italic_formatting() {
        let xml = wrap_body(r#"<w:p><w:r><w:rPr><w:i/></w:rPr><w:t>Italic</w:t></w:r></w:p>"#);
        let doc = parse_xml(&xml);
        assert!(doc.paragraphs[0].runs[0].italic);
        let md = doc.to_markdown(true);
        assert!(md.contains("*Italic*"), "Markdown: {}", md);
    }

    #[test]
    fn test_bold_italic_combined() {
        let xml = wrap_body(r#"<w:p><w:r><w:rPr><w:b/><w:i/></w:rPr><w:t>Both</w:t></w:r></w:p>"#);
        let doc = parse_xml(&xml);
        let run = &doc.paragraphs[0].runs[0];
        assert!(run.bold);
        assert!(run.italic);
        let md = doc.to_markdown(true);
        assert!(md.contains("***Both***"), "Markdown: {}", md);
    }

    #[test]
    fn test_underline_formatting() {
        let xml = wrap_body(r#"<w:p><w:r><w:rPr><w:u w:val="single"/></w:rPr><w:t>Underlined</w:t></w:r></w:p>"#);
        let doc = parse_xml(&xml);
        assert!(doc.paragraphs[0].runs[0].underline);
    }

    #[test]
    fn test_underline_none_disabled() {
        let xml = wrap_body(r#"<w:p><w:r><w:rPr><w:u w:val="none"/></w:rPr><w:t>No Underline</w:t></w:r></w:p>"#);
        let doc = parse_xml(&xml);
        assert!(!doc.paragraphs[0].runs[0].underline);
    }

    #[test]
    fn test_strikethrough_formatting() {
        let xml = wrap_body(r#"<w:p><w:r><w:rPr><w:strike/></w:rPr><w:t>Struck</w:t></w:r></w:p>"#);
        let doc = parse_xml(&xml);
        assert!(doc.paragraphs[0].runs[0].strikethrough);
        let md = doc.to_markdown(true);
        assert!(md.contains("~~Struck~~"), "Markdown: {}", md);
    }

    #[test]
    fn test_double_strikethrough() {
        let xml = wrap_body(r#"<w:p><w:r><w:rPr><w:dstrike/></w:rPr><w:t>DStruck</w:t></w:r></w:p>"#);
        let doc = parse_xml(&xml);
        assert!(doc.paragraphs[0].runs[0].strikethrough);
    }

    #[test]
    fn test_external_hyperlink() {
        let mut rels = AHashMap::new();
        rels.insert("rId1".to_string(), "https://example.com".to_string());

        let xml = wrap_body(r#"<w:p><w:hyperlink r:id="rId1"><w:r><w:t>Click here</w:t></w:r></w:hyperlink></w:p>"#);
        let doc = parse_xml_with_rels(&xml, rels);
        assert_eq!(doc.paragraphs.len(), 1);
        let run = &doc.paragraphs[0].runs[0];
        assert_eq!(run.text, "Click here");
        assert_eq!(run.hyperlink_url.as_deref(), Some("https://example.com"));

        let md = doc.to_markdown(true);
        assert!(md.contains("[Click here](https://example.com)"), "Markdown: {}", md);
    }

    #[test]
    fn test_hyperlink_with_no_relationship() {
        let xml = wrap_body(r#"<w:p><w:hyperlink r:id="rId99"><w:r><w:t>Broken link</w:t></w:r></w:hyperlink></w:p>"#);
        let doc = parse_xml(&xml);
        let run = &doc.paragraphs[0].runs[0];
        assert_eq!(run.text, "Broken link");
        assert!(run.hyperlink_url.is_none());
    }

    #[test]
    fn test_multiple_hyperlinks() {
        let mut rels = AHashMap::new();
        rels.insert("rId1".to_string(), "https://one.com".to_string());
        rels.insert("rId2".to_string(), "https://two.com".to_string());

        let xml = wrap_body(
            r#"<w:p>
                <w:hyperlink r:id="rId1"><w:r><w:t>First</w:t></w:r></w:hyperlink>
                <w:r><w:t> and </w:t></w:r>
                <w:hyperlink r:id="rId2"><w:r><w:t>Second</w:t></w:r></w:hyperlink>
            </w:p>"#,
        );
        let doc = parse_xml_with_rels(&xml, rels);
        let md = doc.to_markdown(true);
        assert!(md.contains("[First](https://one.com)"), "Markdown: {}", md);
        assert!(md.contains("[Second](https://two.com)"), "Markdown: {}", md);
    }

    #[test]
    fn test_basic_2x2_table() {
        let xml = wrap_body(
            r#"<w:tbl>
                <w:tr>
                    <w:tc><w:p><w:r><w:t>A1</w:t></w:r></w:p></w:tc>
                    <w:tc><w:p><w:r><w:t>B1</w:t></w:r></w:p></w:tc>
                </w:tr>
                <w:tr>
                    <w:tc><w:p><w:r><w:t>A2</w:t></w:r></w:p></w:tc>
                    <w:tc><w:p><w:r><w:t>B2</w:t></w:r></w:p></w:tc>
                </w:tr>
            </w:tbl>"#,
        );
        let doc = parse_xml(&xml);
        assert_eq!(doc.tables.len(), 1);
        let table = &doc.tables[0];
        assert_eq!(table.rows.len(), 2);
        assert_eq!(table.rows[0].cells.len(), 2);

        let md = doc.to_markdown(true);
        assert!(md.contains("A1"), "Markdown: {}", md);
        assert!(md.contains("B2"), "Markdown: {}", md);

        let plain = doc.to_plain_text();
        assert!(plain.contains("A1"), "Plain: {}", plain);
        assert!(plain.contains("B2"), "Plain: {}", plain);
    }

    #[test]
    fn test_table_with_caption() {
        let xml = wrap_body(
            r#"<w:tbl>
                <w:tblPr>
                    <w:tblCaption w:val="My Table Caption"/>
                </w:tblPr>
                <w:tr>
                    <w:tc><w:p><w:r><w:t>Cell</w:t></w:r></w:p></w:tc>
                </w:tr>
                <w:tr>
                    <w:tc><w:p><w:r><w:t>Data</w:t></w:r></w:p></w:tc>
                </w:tr>
            </w:tbl>"#,
        );
        let doc = parse_xml(&xml);
        assert_eq!(doc.tables.len(), 1);
        let caption = doc.tables[0].properties.as_ref().and_then(|p| p.caption.as_deref());
        assert_eq!(caption, Some("My Table Caption"));

        let md = doc.to_markdown(true);
        assert!(md.contains("My Table Caption"), "Caption should be in markdown: {}", md);

        let plain = doc.to_plain_text();
        assert!(
            plain.contains("My Table Caption"),
            "Caption should be in plain text: {}",
            plain
        );
    }

    #[test]
    fn test_table_column_span() {
        let xml = wrap_body(
            r#"<w:tbl>
                <w:tr>
                    <w:tc>
                        <w:tcPr><w:gridSpan w:val="2"/></w:tcPr>
                        <w:p><w:r><w:t>Spanning</w:t></w:r></w:p>
                    </w:tc>
                </w:tr>
                <w:tr>
                    <w:tc><w:p><w:r><w:t>Left</w:t></w:r></w:p></w:tc>
                    <w:tc><w:p><w:r><w:t>Right</w:t></w:r></w:p></w:tc>
                </w:tr>
            </w:tbl>"#,
        );
        let doc = parse_xml(&xml);
        let table = &doc.tables[0];
        let first_cell = &table.rows[0].cells[0];
        assert_eq!(first_cell.properties.as_ref().and_then(|p| p.grid_span), Some(2));
    }

    #[test]
    fn test_table_vertical_merge() {
        let xml = wrap_body(
            r#"<w:tbl>
                <w:tr>
                    <w:tc>
                        <w:tcPr><w:vMerge w:val="restart"/></w:tcPr>
                        <w:p><w:r><w:t>Merged</w:t></w:r></w:p>
                    </w:tc>
                    <w:tc><w:p><w:r><w:t>Right1</w:t></w:r></w:p></w:tc>
                </w:tr>
                <w:tr>
                    <w:tc>
                        <w:tcPr><w:vMerge/></w:tcPr>
                        <w:p></w:p>
                    </w:tc>
                    <w:tc><w:p><w:r><w:t>Right2</w:t></w:r></w:p></w:tc>
                </w:tr>
            </w:tbl>"#,
        );
        let doc = parse_xml(&xml);
        let table = &doc.tables[0];
        let cell_0_0 = &table.rows[0].cells[0];
        assert_eq!(
            cell_0_0.properties.as_ref().and_then(|p| p.v_merge.as_ref()),
            Some(&super::super::table::VerticalMerge::Restart)
        );
        let cell_1_0 = &table.rows[1].cells[0];
        assert_eq!(
            cell_1_0.properties.as_ref().and_then(|p| p.v_merge.as_ref()),
            Some(&super::super::table::VerticalMerge::Continue)
        );
    }

    #[test]
    fn test_table_empty_cells() {
        let xml = wrap_body(
            r#"<w:tbl>
                <w:tr>
                    <w:tc><w:p><w:r><w:t>Has content</w:t></w:r></w:p></w:tc>
                    <w:tc><w:p></w:p></w:tc>
                </w:tr>
            </w:tbl>"#,
        );
        let doc = parse_xml(&xml);
        let table = &doc.tables[0];
        assert_eq!(table.rows[0].cells.len(), 2);
        let md = doc.to_markdown(true);
        assert!(md.contains("Has content"), "Markdown: {}", md);
    }

    #[test]
    fn test_bullet_list_extraction() {
        let xml = wrap_body(
            r#"<w:p>
                <w:pPr>
                    <w:numId w:val="1"/>
                    <w:ilvl w:val="0"/>
                </w:pPr>
                <w:r><w:t>Bullet item</w:t></w:r>
            </w:p>"#,
        );
        let doc = parse_xml(&xml);
        assert_eq!(doc.paragraphs.len(), 1);
        assert_eq!(doc.paragraphs[0].to_text(), "Bullet item");
        assert!(doc.paragraphs[0].numbering_id.is_some());
    }

    #[test]
    fn test_heading_style() {
        let xml = wrap_body(
            r#"<w:p>
                <w:pPr><w:pStyle w:val="Heading1"/></w:pPr>
                <w:r><w:t>My Heading</w:t></w:r>
            </w:p>"#,
        );
        let doc = parse_xml(&xml);
        assert_eq!(doc.paragraphs[0].style.as_deref(), Some("Heading1"));
        let md = doc.to_markdown(true);
        assert!(md.contains("# My Heading"), "Markdown: {}", md);
    }

    #[test]
    fn test_heading2_style() {
        let xml = wrap_body(
            r#"<w:p>
                <w:pPr><w:pStyle w:val="Heading2"/></w:pPr>
                <w:r><w:t>Sub Heading</w:t></w:r>
            </w:p>"#,
        );
        let doc = parse_xml(&xml);
        let md = doc.to_markdown(true);
        assert!(md.contains("## Sub Heading"), "Markdown: {}", md);
    }

    #[test]
    fn test_title_style() {
        let xml = wrap_body(
            r#"<w:p>
                <w:pPr><w:pStyle w:val="Title"/></w:pPr>
                <w:r><w:t>Document Title</w:t></w:r>
            </w:p>"#,
        );
        let doc = parse_xml(&xml);
        let md = doc.to_markdown(true);
        assert!(md.contains("Document Title"), "Markdown: {}", md);
    }

    #[test]
    fn test_inline_drawing_with_alt_text() {
        let xml = wrap_body(
            r#"<w:p><w:r>
                <w:drawing>
                    <wp:inline xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing">
                        <wp:extent cx="914400" cy="914400"/>
                        <wp:docPr id="1" name="Picture 1" descr="A logo image"/>
                        <a:graphic xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main">
                            <a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/picture">
                                <pic:pic xmlns:pic="http://schemas.openxmlformats.org/drawingml/2006/picture">
                                    <pic:blipFill>
                                        <a:blip r:embed="rId5"/>
                                    </pic:blipFill>
                                </pic:pic>
                            </a:graphicData>
                        </a:graphic>
                    </wp:inline>
                </w:drawing>
            </w:r></w:p>"#,
        );
        let doc = parse_xml(&xml);
        assert_eq!(doc.drawings.len(), 1);
        let drawing = &doc.drawings[0];
        assert_eq!(
            drawing.doc_properties.as_ref().and_then(|dp| dp.description.as_deref()),
            Some("A logo image")
        );
        assert_eq!(drawing.image_ref.as_deref(), Some("rId5"));

        let md = doc.to_markdown(true);
        assert!(md.contains("![A logo image]"), "Markdown: {}", md);
    }

    #[test]
    fn test_drawing_dimensions() {
        let xml = wrap_body(
            r#"<w:p><w:r>
                <w:drawing>
                    <wp:inline xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing">
                        <wp:extent cx="1828800" cy="914400"/>
                        <wp:docPr id="1" name="Pic"/>
                    </wp:inline>
                </w:drawing>
            </w:r></w:p>"#,
        );
        let doc = parse_xml(&xml);
        let extent = doc.drawings[0].extent.as_ref().unwrap();
        assert_eq!(extent.cx, 1828800);
        assert_eq!(extent.cy, 914400);
    }

    #[test]
    fn test_section_properties_parsed() {
        let xml = wrap_body(
            r#"<w:p><w:r><w:t>Content</w:t></w:r></w:p>
            <w:sectPr>
                <w:pgSz w:w="12240" w:h="15840"/>
                <w:pgMar w:top="1440" w:right="1800" w:bottom="1440" w:left="1800"/>
            </w:sectPr>"#,
        );
        let doc = parse_xml(&xml);
        assert!(!doc.sections.is_empty(), "Should have sections");
        let sect = &doc.sections[0];
        assert_eq!(sect.page_width_twips, Some(12240));
        assert_eq!(sect.page_height_twips, Some(15840));
        assert_eq!(sect.margins.top, Some(1440));
        assert_eq!(sect.margins.left, Some(1800));
    }

    #[test]
    fn test_footnote_reference_marker() {
        let xml = wrap_body(
            r#"<w:p>
                <w:r><w:t>Main text</w:t></w:r>
                <w:r><w:footnoteReference w:id="2"/></w:r>
            </w:p>"#,
        );
        let doc = parse_xml(&xml);
        let text = doc.paragraphs[0].to_text();
        assert!(text.contains("[^2]"), "Should contain footnote marker: {}", text);
    }

    #[test]
    fn test_footnote_separator_ids_filtered() {
        let xml = wrap_body(
            r#"<w:p>
                <w:r><w:footnoteReference w:id="0"/></w:r>
                <w:r><w:footnoteReference w:id="1"/></w:r>
                <w:r><w:t>text</w:t></w:r>
                <w:r><w:footnoteReference w:id="2"/></w:r>
            </w:p>"#,
        );
        let doc = parse_xml(&xml);
        let text = doc.paragraphs[0].to_text();
        assert!(!text.contains("[^0]"), "Separator id 0 should be filtered");
        assert!(!text.contains("[^1]"), "Separator id 1 should be filtered");
        assert!(text.contains("[^2]"), "Real footnote 2 should be present");
    }

    #[test]
    fn test_field_instruction_skipped_result_kept() {
        let xml = wrap_body(
            r#"<w:p>
                <w:r><w:t>Before </w:t></w:r>
                <w:r><w:fldChar w:fldCharType="begin"/></w:r>
                <w:r><w:instrText> SEQ Figure \* ARABIC </w:instrText></w:r>
                <w:r><w:fldChar w:fldCharType="separate"/></w:r>
                <w:r><w:t>2</w:t></w:r>
                <w:r><w:fldChar w:fldCharType="end"/></w:r>
                <w:r><w:t> After</w:t></w:r>
            </w:p>"#,
        );
        let doc = parse_xml(&xml);
        let text = doc.paragraphs[0].to_text();
        assert!(text.contains("Before"), "Text: {}", text);
        assert!(text.contains("After"), "Text: {}", text);
        assert!(text.contains("2"), "Field result '2' should be kept: {}", text);
        assert!(!text.contains("SEQ"), "Field instruction should be skipped: {}", text);
    }

    #[test]
    fn test_page_field_result_kept() {
        let xml = wrap_body(
            r#"<w:p>
                <w:r><w:t>Page </w:t></w:r>
                <w:r><w:fldChar w:fldCharType="begin"/></w:r>
                <w:r><w:instrText> PAGE </w:instrText></w:r>
                <w:r><w:fldChar w:fldCharType="separate"/></w:r>
                <w:r><w:t>1</w:t></w:r>
                <w:r><w:fldChar w:fldCharType="end"/></w:r>
                <w:r><w:t> of 5</w:t></w:r>
            </w:p>"#,
        );
        let doc = parse_xml(&xml);
        let text = doc.paragraphs[0].to_text();
        assert_eq!(text.trim(), "Page 1 of 5", "Text: '{}'", text);
    }

    #[test]
    fn test_text_after_field_resumes() {
        let xml = wrap_body(
            r#"<w:p>
                <w:r><w:fldChar w:fldCharType="begin"/></w:r>
                <w:r><w:instrText> NUMPAGES </w:instrText></w:r>
                <w:r><w:fldChar w:fldCharType="separate"/></w:r>
                <w:r><w:t>10</w:t></w:r>
                <w:r><w:fldChar w:fldCharType="end"/></w:r>
                <w:r><w:t>Normal text</w:t></w:r>
            </w:p>"#,
        );
        let doc = parse_xml(&xml);
        let text = doc.paragraphs[0].to_text();
        assert!(text.contains("Normal text"), "Text: {}", text);
        assert!(text.contains("10"), "Field result should be kept: {}", text);
    }

    #[test]
    fn test_math_text_extracted() {
        let xml = wrap_body(
            r#"<w:p>
                <m:oMath xmlns:m="http://schemas.openxmlformats.org/officeDocument/2006/math">
                    <m:r>
                        <m:t>E=mc</m:t>
                    </m:r>
                    <m:sSup>
                        <m:e><m:r><m:t></m:t></m:r></m:e>
                        <m:sup><m:r><m:t>2</m:t></m:r></m:sup>
                    </m:sSup>
                </m:oMath>
            </w:p>"#,
        );
        let doc = parse_xml(&xml);
        let text = doc.paragraphs[0].to_text();
        assert!(text.contains("E=mc"), "Math text should contain E=mc: {}", text);
        assert!(text.contains("^{2}"), "Math text should contain ^{{2}}: {}", text);
        let md = doc.paragraphs[0].runs_to_markdown();
        assert!(md.starts_with('$'), "Inline math should start with $: {}", md);
        assert!(md.ends_with('$'), "Inline math should end with $: {}", md);
    }

    #[test]
    fn test_element_ordering_preserved() {
        let xml = wrap_body(
            r#"<w:p><w:r><w:t>Para 1</w:t></w:r></w:p>
            <w:tbl>
                <w:tr><w:tc><w:p><w:r><w:t>Cell</w:t></w:r></w:p></w:tc></w:tr>
                <w:tr><w:tc><w:p><w:r><w:t>Data</w:t></w:r></w:p></w:tc></w:tr>
            </w:tbl>
            <w:p><w:r><w:t>Para 2</w:t></w:r></w:p>"#,
        );
        let doc = parse_xml(&xml);
        assert_eq!(doc.elements.len(), 3);
        assert!(matches!(doc.elements[0], DocumentElement::Paragraph(0)));
        assert!(matches!(doc.elements[1], DocumentElement::Table(0)));
        assert!(matches!(doc.elements[2], DocumentElement::Paragraph(1)));

        let md = doc.to_markdown(true);
        let para1_pos = md.find("Para 1").unwrap();
        let cell_pos = md.find("Cell").unwrap();
        let para2_pos = md.find("Para 2").unwrap();
        assert!(para1_pos < cell_pos, "Para 1 before table");
        assert!(cell_pos < para2_pos, "Table before Para 2");
    }

    #[test]
    fn test_empty_document() {
        let xml = wrap_body("");
        let doc = parse_xml(&xml);
        assert!(doc.paragraphs.is_empty());
        assert!(doc.tables.is_empty());
        let md = doc.to_markdown(true);
        assert!(md.trim().is_empty(), "Empty doc markdown: '{}'", md);
    }

    #[test]
    fn test_paragraph_with_only_whitespace() {
        let xml = wrap_body(r#"<w:p><w:r><w:t>   </w:t></w:r></w:p>"#);
        let doc = parse_xml(&xml);
        assert_eq!(doc.paragraphs[0].to_text(), "   ");
    }

    #[test]
    fn test_extract_lorem_ipsum_docx() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test_documents/docx/lorem_ipsum.docx");
        if let Ok(bytes) = std::fs::read(&path) {
            let text = super::super::extract_text(&bytes).unwrap();
            assert!(!text.is_empty(), "Should extract text from lorem ipsum");
            assert!(
                text.contains("Lorem"),
                "Should contain 'Lorem': {}",
                &text[..100.min(text.len())]
            );
        }
    }

    #[test]
    fn test_extract_word_tables_docx() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test_documents/docx/word_tables.docx");
        if let Ok(bytes) = std::fs::read(&path) {
            let text = super::super::extract_text(&bytes).unwrap();
            assert!(!text.is_empty(), "Should extract text from word tables");
        }
    }

    #[test]
    fn test_extract_unit_test_lists_docx() {
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test_documents/docx/unit_test_lists.docx");
        if let Ok(bytes) = std::fs::read(&path) {
            let text = super::super::extract_text(&bytes).unwrap();
            assert!(!text.is_empty(), "Should extract text from list document");
        }
    }

    #[test]
    fn test_extract_python_docx_test_file() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../test_documents/vendored/python-docx/test.docx");
        if let Ok(bytes) = std::fs::read(&path) {
            let text = super::super::extract_text(&bytes).unwrap();
            assert!(!text.is_empty(), "Should extract text from python-docx test.docx");
        }
    }

    #[test]
    fn test_extract_python_docx_having_images() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../test_documents/vendored/python-docx/having-images.docx");
        if let Ok(bytes) = std::fs::read(&path) {
            let text = super::super::extract_text(&bytes).unwrap();
            let _ = text;
        }
    }

    #[test]
    fn test_extract_word_sample_no_field_leaks() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test_documents/docx/word_sample.docx");
        if let Ok(bytes) = std::fs::read(&path) {
            let text = super::super::extract_text(&bytes).unwrap();
            assert!(!text.is_empty());
        }
    }

    /// Regression: adjacent bold runs in textbox.docx must not produce `****`.
    #[test]
    fn test_textbox_no_spurious_bold_markers() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test_documents/docx/textbox.docx");
        if let Ok(bytes) = std::fs::read(&path) {
            let mut budget = SecurityBudget::with_defaults();
            let doc = super::parse_document(&bytes, &mut budget).unwrap();
            let md = doc.to_markdown(true);
            assert!(
                !md.contains("****"),
                "Markdown output should not contain spurious '****' sequences. Got:\n{}",
                md
            );
        }
    }

    #[test]
    fn test_extract_unit_test_formatting_no_headers() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../test_documents/docx/unit_test_formatting.docx");
        if let Ok(bytes) = std::fs::read(&path) {
            let text = super::super::extract_text(&bytes).unwrap();
            assert!(!text.is_empty());
        }
    }

    #[test]
    fn test_to_markdown_inject_placeholders_true() {
        use crate::extraction::docx::drawing::{DocProperties, Drawing, DrawingType};

        let mut doc = Document::new();

        let mut para = Paragraph::new();
        para.add_run(Run::new("Hello world".to_string()));
        let p_idx = doc.paragraphs.len();
        doc.paragraphs.push(para);
        doc.elements.push(DocumentElement::Paragraph(p_idx));

        let drawing = Drawing {
            drawing_type: DrawingType::Inline,
            extent: None,
            doc_properties: Some(DocProperties {
                id: Some("1".to_string()),
                name: Some("Pic".to_string()),
                description: Some("alt text".to_string()),
            }),
            image_ref: Some("rId1".to_string()),
            text_box_content: None,
        };
        let d_idx = doc.drawings.len();
        doc.drawings.push(drawing);
        doc.elements.push(DocumentElement::Drawing(d_idx));

        let md = doc.to_markdown(true);
        assert!(
            md.contains("![alt text](image)"),
            "Expected image placeholder, got: {md}"
        );
        assert!(md.contains("Hello world"));
    }

    #[test]
    fn test_to_markdown_inject_placeholders_false() {
        use crate::extraction::docx::drawing::{DocProperties, Drawing, DrawingType};

        let mut doc = Document::new();

        let mut para = Paragraph::new();
        para.add_run(Run::new("Hello world".to_string()));
        let p_idx = doc.paragraphs.len();
        doc.paragraphs.push(para);
        doc.elements.push(DocumentElement::Paragraph(p_idx));

        let drawing = Drawing {
            drawing_type: DrawingType::Inline,
            extent: None,
            doc_properties: Some(DocProperties {
                id: Some("1".to_string()),
                name: Some("Pic".to_string()),
                description: Some("alt text".to_string()),
            }),
            image_ref: Some("rId1".to_string()),
            text_box_content: None,
        };
        let d_idx = doc.drawings.len();
        doc.drawings.push(drawing);
        doc.elements.push(DocumentElement::Drawing(d_idx));

        let md = doc.to_markdown(false);
        assert!(!md.contains("!["), "Should NOT contain image placeholder, got: {md}");
        assert!(md.contains("Hello world"), "Text content must be preserved");
    }
}
