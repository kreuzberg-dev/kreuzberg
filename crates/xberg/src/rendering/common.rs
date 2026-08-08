//! Shared rendering infrastructure for `InternalDocument`-based renderers.
//!
//! Provides nesting state tracking, annotated text rendering, footnote collection,
//! table formatting helpers, and HTML escaping.

use std::borrow::Cow;

use crate::types::document_structure::{AnnotationKind, ContentLayer, TextAnnotation};
use crate::types::internal::{ElementKind, InternalDocument, InternalElement, RelationshipKind, RelationshipTarget};

/// Kind of container on the nesting stack.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum NestingKind {
    List { ordered: bool, item_count: u32 },
    BlockQuote,
    Group,
}

/// Tracks nesting depth during a linear pass over elements.
#[derive(Debug, Default)]
pub(crate) struct RenderState {
    /// Stack of `(depth, kind)` entries.
    stack: Vec<(u16, NestingKind)>,
}

impl RenderState {
    /// Push a container onto the nesting stack.
    pub(crate) fn push_container(&mut self, kind: NestingKind, depth: u16) {
        self.stack.push((depth, kind));
    }

    /// Pop the top container if it matches the given kind category.
    pub(crate) fn pop_container(&mut self, kind: &NestingKind) {
        for i in (0..self.stack.len()).rev() {
            if matches!(
                (&self.stack[i].1, kind),
                (NestingKind::List { .. }, NestingKind::List { .. })
                    | (NestingKind::BlockQuote, NestingKind::BlockQuote)
                    | (NestingKind::Group, NestingKind::Group)
            ) {
                self.stack.remove(i);
                return;
            }
        }
    }

    /// Pop entries whose depth >= the given depth (fallback for missing end markers).
    pub(crate) fn pop_to_depth(&mut self, depth: u16) {
        while let Some(&(d, _)) = self.stack.last() {
            if d >= depth {
                self.stack.pop();
            } else {
                break;
            }
        }
    }

    /// Current list nesting depth.
    pub(crate) fn list_depth(&self) -> usize {
        self.stack
            .iter()
            .filter(|(_, k)| matches!(k, NestingKind::List { .. }))
            .count()
    }

    /// Current blockquote nesting depth.
    pub(crate) fn blockquote_depth(&self) -> usize {
        self.stack
            .iter()
            .filter(|(_, k)| matches!(k, NestingKind::BlockQuote))
            .count()
    }

    /// Increment and return the next list item number for the innermost list.
    pub(crate) fn next_list_number(&mut self) -> u32 {
        for (_, kind) in self.stack.iter_mut().rev() {
            if let NestingKind::List {
                ordered: true,
                item_count,
            } = kind
            {
                *item_count += 1;
                return *item_count;
            }
            if let NestingKind::List { ordered: false, .. } = kind {
                break;
            }
        }
        1
    }
}

/// Render text with byte-range annotations, calling `emit` for each annotated span.
///
/// Annotations are sorted by `(start, end)`. Overlapping annotations (where
/// `start < current_pos`) are skipped, matching the existing renderer behavior.
///
/// Plain (unannotated) text segments are passed through without transformation.
#[cfg(test)]
pub(crate) fn render_annotated_text(
    text: &str,
    annotations: &[TextAnnotation],
    emit: impl Fn(&str, &AnnotationKind) -> String,
) -> String {
    render_annotated_text_with_plain(text, annotations, emit, |s| s.to_string())
}

pub(crate) fn render_annotated_text_with_plain(
    text: &str,
    annotations: &[TextAnnotation],
    emit: impl Fn(&str, &AnnotationKind) -> String,
    plain: impl Fn(&str) -> String,
) -> String {
    if annotations.is_empty() {
        return plain(text);
    }

    let mut sorted: Vec<&TextAnnotation> = annotations.iter().collect();
    sorted.sort_by_key(|a| (a.start, a.end));

    let bytes = text.as_bytes();
    let len = bytes.len() as u32;
    let mut pos: u32 = 0;
    let mut out = String::with_capacity(text.len() + 64);

    for ann in &sorted {
        let start = ann.start.min(len);
        let end = ann.end.min(len);
        if start < pos {
            continue;
        }
        if start > pos {
            out.push_str(&plain(&text[pos as usize..start as usize]));
        }
        let span = &text[start as usize..end as usize];
        out.push_str(&emit(span, &ann.kind));
        pos = end;
    }

    if (pos as usize) < bytes.len() {
        out.push_str(&plain(&text[pos as usize..]));
    }

    out
}

/// Collected footnote data: definition text and assigned number.
#[derive(Debug)]
pub(crate) struct FootnoteEntry {
    pub(crate) text: String,
    pub(crate) number: u32,
}

/// Pre-scans elements and relationships to build a sequential footnote numbering.
#[derive(Debug)]
pub(crate) struct FootnoteCollector {
    /// Map from element index (FootnoteRef) -> assigned number.
    ref_numbers: ahash::AHashMap<u32, u32>,
    /// Ordered definitions.
    definitions: Vec<FootnoteEntry>,
}

impl FootnoteCollector {
    /// Scan the document and build footnote mappings.
    pub(crate) fn new(doc: &InternalDocument) -> Self {
        let mut def_by_anchor: ahash::AHashMap<String, (u32, String)> = ahash::AHashMap::new();
        for (i, elem) in doc.elements.iter().enumerate() {
            if elem.kind == ElementKind::FootnoteDefinition
                && let Some(ref anchor) = elem.anchor
            {
                def_by_anchor.insert(anchor.clone(), (i as u32, elem.text.clone()));
            }
        }

        let mut ref_to_def_anchor: ahash::AHashMap<u32, String> = ahash::AHashMap::new();
        for rel in &doc.relationships {
            if rel.kind == RelationshipKind::FootnoteReference {
                match &rel.target {
                    RelationshipTarget::Key(key) => {
                        ref_to_def_anchor.insert(rel.source, key.clone());
                    }
                    RelationshipTarget::Index(idx) => {
                        if let Some(elem) = doc.elements.get(*idx as usize)
                            && let Some(ref anchor) = elem.anchor
                        {
                            ref_to_def_anchor.insert(rel.source, anchor.clone());
                        }
                    }
                }
            }
        }

        for (i, elem) in doc.elements.iter().enumerate() {
            if elem.kind == ElementKind::FootnoteRef {
                let idx = i as u32;
                if !ref_to_def_anchor.contains_key(&idx) {
                    if let Some(ref anchor) = elem.anchor {
                        ref_to_def_anchor.insert(idx, anchor.clone());
                    } else if !elem.text.is_empty() {
                        ref_to_def_anchor.insert(idx, elem.text.clone());
                    }
                }
            }
        }

        let mut ref_numbers: ahash::AHashMap<u32, u32> = ahash::AHashMap::new();
        let mut anchor_to_number: ahash::AHashMap<String, u32> = ahash::AHashMap::new();
        let mut next_number: u32 = 1;
        let mut definitions = Vec::new();

        for (i, elem) in doc.elements.iter().enumerate() {
            if elem.kind == ElementKind::FootnoteRef {
                let idx = i as u32;
                if let Some(anchor) = ref_to_def_anchor.get(&idx) {
                    let number = *anchor_to_number.entry(anchor.clone()).or_insert_with(|| {
                        let n = next_number;
                        next_number += 1;
                        let text = def_by_anchor.get(anchor).map(|(_, t)| t.clone()).unwrap_or_default();
                        definitions.push(FootnoteEntry { text, number: n });
                        n
                    });
                    ref_numbers.insert(idx, number);
                }
            }
        }

        // A definition that no reference points at is still authored content.
        // `definitions` was previously populated only from inside the FootnoteRef
        // loop above, so an unreferenced definition never reached any renderer and
        // was silently lost. Append the orphans after the referenced ones, in
        // document order, continuing the same numbering. See #68.
        for elem in &doc.elements {
            if elem.kind != ElementKind::FootnoteDefinition {
                continue;
            }
            let Some(anchor) = elem.anchor.as_ref() else {
                continue;
            };
            if anchor_to_number.contains_key(anchor) {
                continue;
            }
            let number = next_number;
            next_number += 1;
            anchor_to_number.insert(anchor.clone(), number);
            definitions.push(FootnoteEntry {
                text: elem.text.clone(),
                number,
            });
        }

        Self {
            ref_numbers,
            definitions,
        }
    }

    /// Get the footnote number for a FootnoteRef element at the given index.
    pub(crate) fn ref_number(&self, elem_index: u32) -> Option<u32> {
        self.ref_numbers.get(&elem_index).copied()
    }

    /// Get ordered footnote definitions.
    pub(crate) fn definitions(&self) -> &[FootnoteEntry] {
        &self.definitions
    }
}

/// Render a table (from `Table.cells`) as a GFM pipe table.
///
/// This is the crate's single table-to-markdown renderer: every extractor and
/// every output renderer routes through it, so the same table serialises
/// identically whether it came from a PDF, a DOCX, an HTML page or OCR
/// (xberg-io/xberg#220).
///
/// The grid is normalised to the width of its *widest* row, so a row carrying
/// more cells than the header keeps every one of them instead of having the
/// overflow silently dropped (xberg-io/xberg#221); short rows are padded so the
/// pipe columns stay aligned with the header.
///
/// Cell content is escaped so no cell can break out of the row it lives in:
/// `|` becomes `\|` and any line break becomes `<br>` (xberg-io/xberg#163).
pub(crate) fn render_table_markdown(cells: &[Vec<String>]) -> String {
    let mut out = String::new();
    render_table_markdown_into(&mut out, cells);
    out
}

/// Render a GFM pipe table into an existing buffer.
///
/// Identical contract to [`render_table_markdown`]; callers that can pre-size
/// the buffer from a capacity estimate use this to skip the intermediate
/// allocation.
pub(crate) fn render_table_markdown_into(out: &mut String, cells: &[Vec<String>]) {
    if cells.is_empty() {
        return;
    }
    let num_cols = cells.iter().map(|r| r.len()).max().unwrap_or(0);
    if num_cols == 0 {
        return;
    }

    if let Some(header) = cells.first() {
        push_table_row(out, header, num_cols);

        out.push('|');
        for _ in 0..num_cols {
            out.push_str(" --- |");
        }
        out.push('\n');
    }

    for row in cells.iter().skip(1) {
        push_table_row(out, row, num_cols);
    }
}

/// Push one pipe-delimited row, padded out to `num_cols` columns.
fn push_table_row(out: &mut String, row: &[String], num_cols: usize) {
    out.push('|');
    for col in 0..num_cols {
        out.push(' ');
        let content = row.get(col).map(String::as_str).unwrap_or("");
        push_escaped_cell(out, content);
        out.push_str(" |");
    }
    out.push('\n');
}

/// Stand-in for a line break inside a table cell. A raw newline ends the table
/// row, splitting one cell's content across two rows (xberg-io/xberg#163).
const CELL_LINE_BREAK: &str = "<br>";

/// Push `content` into `out`, escaping every character that would let a cell
/// break out of its row. Avoids allocation when there is nothing to escape (the
/// common case for table cell content).
fn push_escaped_cell(out: &mut String, content: &str) {
    if memchr::memchr3(b'|', b'\n', b'\r', content.as_bytes()).is_none() {
        out.push_str(content);
        return;
    }
    let mut chars = content.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '|' => out.push_str("\\|"),
            '\r' => {
                // Consume the LF of a CRLF pair so it yields one break, not two.
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                out.push_str(CELL_LINE_BREAK);
            }
            '\n' => out.push_str(CELL_LINE_BREAK),
            _ => out.push(ch),
        }
    }
}

/// Render a table as plain space-separated text.
pub(crate) fn render_table_plain(cells: &[Vec<String>]) -> String {
    if cells.is_empty() {
        return String::new();
    }

    let mut out = String::new();
    for row in cells {
        out.push_str(&row.join(" "));
        out.push('\n');
    }
    out
}

/// Render a table as djot pipe table (same syntax as GFM).
pub(crate) fn render_table_djot(cells: &[Vec<String>]) -> String {
    render_table_markdown(cells)
}

/// Normalize inline text for consistent output across renderers.
///
/// - Collapses multiple consecutive whitespace (spaces, tabs) into a single space
/// - Replaces newlines with spaces (mid-paragraph line breaks from PDF extraction)
/// - Strips control characters (< 0x20) except tab
pub(crate) fn normalize_inline_text(text: &str) -> String {
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

/// Ensure the output has a trailing newline (but not doubled).
pub(crate) fn ensure_trailing_newline(out: &mut String) {
    if !out.ends_with('\n') {
        out.push('\n');
    }
}

/// Trim trailing whitespace, then ensure exactly one trailing newline.
pub(crate) fn finalize_output(mut out: String) -> String {
    let trimmed_len = out.trim_end().len();
    if trimmed_len == 0 {
        return String::new();
    }
    out.truncate(trimmed_len);
    out.push('\n');
    out
}

/// Prefix every line of `text` with the blockquote prefix (`> ` repeated N times).
pub(crate) fn apply_blockquote_prefix(text: &str, depth: usize) -> Cow<'_, str> {
    if depth == 0 {
        return Cow::Borrowed(text);
    }
    let prefix = "> ".repeat(depth);
    let mut out = String::with_capacity(text.len() + prefix.len() * text.lines().count());
    for line in text.lines() {
        out.push_str(&prefix);
        out.push_str(line);
        out.push('\n');
    }
    Cow::Owned(out)
}

/// Push a block of text, optionally applying blockquote prefixes.
pub(crate) fn push_with_bq(out: &mut String, text: &str, bq_depth: usize) {
    if bq_depth > 0 {
        out.push_str(&apply_blockquote_prefix(text, bq_depth));
    } else {
        out.push_str(text);
    }
}

/// Handle container end elements (ListEnd/QuoteEnd/GroupEnd) by popping the
/// corresponding entry from the nesting state. Returns `true` if a container
/// was handled.
pub(crate) fn handle_container_end(kind: &ElementKind, state: &mut RenderState) -> bool {
    match kind {
        ElementKind::ListEnd => {
            state.pop_container(&NestingKind::List {
                ordered: false,
                item_count: 0,
            });
            true
        }
        ElementKind::QuoteEnd => {
            state.pop_container(&NestingKind::BlockQuote);
            true
        }
        ElementKind::GroupEnd => {
            state.pop_container(&NestingKind::Group);
            true
        }
        _ => false,
    }
}

/// Check if an element should be rendered in the body pass.
pub(crate) fn is_body_element(elem: &InternalElement) -> bool {
    elem.layer == ContentLayer::Body
}

/// Check if an element is a container end marker.
pub(crate) fn is_container_end(elem: &InternalElement) -> bool {
    elem.kind.is_container_end()
}

/// Get the language attribute from an element's attributes map.
pub(crate) fn get_language(elem: &InternalElement) -> Option<&str> {
    elem.attributes
        .as_ref()
        .and_then(|attrs| attrs.get("language").map(|s| s.as_str()))
}

/// Get the admonition kind from attributes.
pub(crate) fn get_admonition_kind(elem: &InternalElement) -> &str {
    elem.attributes
        .as_ref()
        .and_then(|attrs| attrs.get("kind").map(|s| s.as_str()))
        .unwrap_or("note")
}

/// Get the admonition title from attributes.
pub(crate) fn get_admonition_title(elem: &InternalElement) -> Option<&str> {
    elem.attributes
        .as_ref()
        .and_then(|attrs| attrs.get("title").map(|s| s.as_str()))
}

/// Get metadata entries from the text (stored as `key: value` lines).
pub(crate) fn parse_metadata_entries(text: &str) -> Vec<(&str, &str)> {
    text.lines()
        .filter_map(|line| {
            let idx = line.find(':')?;
            let key = line[..idx].trim();
            let value = line[idx + 1..].trim();
            if key.is_empty() { None } else { Some((key, value)) }
        })
        .collect()
}

/// Human-readable label for a [`PdfAnnotationType`], shared by every
/// renderer's annotation appendix (issue #63).
pub(crate) fn annotation_type_label(kind: crate::types::annotations::PdfAnnotationType) -> &'static str {
    use crate::types::annotations::PdfAnnotationType;
    match kind {
        PdfAnnotationType::Text => "Text",
        PdfAnnotationType::Highlight => "Highlight",
        PdfAnnotationType::Link => "Link",
        PdfAnnotationType::Stamp => "Stamp",
        PdfAnnotationType::Underline => "Underline",
        PdfAnnotationType::StrikeOut => "StrikeOut",
        PdfAnnotationType::Squiggly => "Squiggly",
        PdfAnnotationType::Ink => "Ink",
        PdfAnnotationType::Square => "Square",
        PdfAnnotationType::Circle => "Circle",
        PdfAnnotationType::Polygon => "Polygon",
        PdfAnnotationType::PolyLine => "PolyLine",
        PdfAnnotationType::Line => "Line",
        PdfAnnotationType::Caret => "Caret",
        PdfAnnotationType::FileAttachment => "FileAttachment",
        PdfAnnotationType::Sound => "Sound",
        PdfAnnotationType::Movie => "Movie",
        PdfAnnotationType::Other => "Other",
    }
}

/// The best available text for a rendered annotation: the QuadPoints-derived
/// marked-up text (Highlight/Underline/StrikeOut/Squiggly) takes priority
/// over the free-form comment/URL in `content`, since the marked text is what
/// the annotation is actually about.
pub(crate) fn annotation_display_text(annotation: &crate::types::annotations::PdfAnnotation) -> Option<&str> {
    annotation
        .marked_text
        .as_deref()
        .or(annotation.content.as_deref())
        .filter(|s| !s.is_empty())
}

/// Escape a string for safe inclusion in HTML text content (not attributes).
///
/// Only the three characters that matter for text nodes are escaped, matching
/// the minimal escaping `comrak`'s own HTML formatter performs for body text.
pub(crate) fn escape_html_text(input: &str) -> Cow<'_, str> {
    if !input.contains(['&', '<', '>']) {
        return Cow::Borrowed(input);
    }
    let mut out = String::with_capacity(input.len() + 16);
    for c in input.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(c),
        }
    }
    Cow::Owned(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::document_structure::{AnnotationKind, TextAnnotation};

    #[test]
    fn test_finalize_output_trims_and_adds_newline() {
        assert_eq!(finalize_output("Hello\n\n\n".to_string()), "Hello\n");
    }

    #[test]
    fn test_finalize_output_empty_input() {
        assert_eq!(finalize_output("".to_string()), "");
    }

    #[test]
    fn test_finalize_output_whitespace_only() {
        assert_eq!(finalize_output("   \n\n  ".to_string()), "");
    }

    #[test]
    fn test_ensure_trailing_newline_adds_when_missing() {
        let mut s = "hello".to_string();
        ensure_trailing_newline(&mut s);
        assert_eq!(s, "hello\n");
    }

    #[test]
    fn test_ensure_trailing_newline_no_double() {
        let mut s = "hello\n".to_string();
        ensure_trailing_newline(&mut s);
        assert_eq!(s, "hello\n");
    }

    #[test]
    fn test_blockquote_prefix_depth_zero() {
        let result = apply_blockquote_prefix("hello\n", 0);
        assert_eq!(result.as_ref(), "hello\n");
        assert!(matches!(result, Cow::Borrowed(_)));
    }

    #[test]
    fn test_blockquote_prefix_depth_one() {
        let result = apply_blockquote_prefix("hello\n", 1);
        assert_eq!(result.as_ref(), "> hello\n");
    }

    #[test]
    fn test_blockquote_prefix_depth_two() {
        let result = apply_blockquote_prefix("hello\n", 2);
        assert_eq!(result.as_ref(), "> > hello\n");
    }

    #[test]
    fn test_blockquote_prefix_multiline() {
        let result = apply_blockquote_prefix("line1\nline2\n", 1);
        assert_eq!(result.as_ref(), "> line1\n> line2\n");
    }

    #[test]
    fn test_parse_metadata_entries_basic() {
        let entries = parse_metadata_entries("Author: Alice\nDate: 2024-01-01");
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0], ("Author", "Alice"));
        assert_eq!(entries[1], ("Date", "2024-01-01"));
    }

    #[test]
    fn test_parse_metadata_entries_empty() {
        let entries = parse_metadata_entries("");
        assert!(entries.is_empty());
    }

    #[test]
    fn test_parse_metadata_entries_no_colon() {
        let entries = parse_metadata_entries("no colon here");
        assert!(entries.is_empty());
    }

    #[test]
    fn test_parse_metadata_entries_empty_key() {
        let entries = parse_metadata_entries(": value");
        assert!(entries.is_empty());
    }

    #[test]
    fn test_render_annotated_text_no_annotations() {
        let result = render_annotated_text("Hello", &[], |span, _| span.to_string());
        assert_eq!(result, "Hello");
    }

    #[test]
    fn test_render_annotated_text_single_annotation() {
        let ann = vec![TextAnnotation {
            start: 0,
            end: 5,
            kind: AnnotationKind::Bold,
        }];
        let result = render_annotated_text("Hello world", &ann, |span, kind| match kind {
            AnnotationKind::Bold => format!("[B:{}]", span),
            _ => span.to_string(),
        });
        assert_eq!(result, "[B:Hello] world");
    }

    #[test]
    fn test_render_annotated_text_multiple_non_overlapping() {
        let ann = vec![
            TextAnnotation {
                start: 0,
                end: 5,
                kind: AnnotationKind::Bold,
            },
            TextAnnotation {
                start: 6,
                end: 11,
                kind: AnnotationKind::Italic,
            },
        ];
        let result = render_annotated_text("Hello world", &ann, |span, kind| match kind {
            AnnotationKind::Bold => format!("[B:{}]", span),
            AnnotationKind::Italic => format!("[I:{}]", span),
            _ => span.to_string(),
        });
        assert_eq!(result, "[B:Hello] [I:world]");
    }

    #[test]
    fn test_render_annotated_text_overlapping_skips_inner() {
        let ann = vec![
            TextAnnotation {
                start: 0,
                end: 11,
                kind: AnnotationKind::Bold,
            },
            TextAnnotation {
                start: 6,
                end: 11,
                kind: AnnotationKind::Italic,
            },
        ];
        let result = render_annotated_text("Hello world", &ann, |span, kind| match kind {
            AnnotationKind::Bold => format!("[B:{}]", span),
            AnnotationKind::Italic => format!("[I:{}]", span),
            _ => span.to_string(),
        });
        assert_eq!(result, "[B:Hello world]");
    }

    #[test]
    fn test_render_state_blockquote_depth() {
        let mut state = RenderState::default();
        assert_eq!(state.blockquote_depth(), 0);
        state.push_container(NestingKind::BlockQuote, 0);
        assert_eq!(state.blockquote_depth(), 1);
        state.push_container(NestingKind::BlockQuote, 1);
        assert_eq!(state.blockquote_depth(), 2);
        state.pop_container(&NestingKind::BlockQuote);
        assert_eq!(state.blockquote_depth(), 1);
    }

    #[test]
    fn test_render_state_list_depth() {
        let mut state = RenderState::default();
        assert_eq!(state.list_depth(), 0);
        state.push_container(
            NestingKind::List {
                ordered: false,
                item_count: 0,
            },
            0,
        );
        assert_eq!(state.list_depth(), 1);
        state.push_container(
            NestingKind::List {
                ordered: true,
                item_count: 0,
            },
            1,
        );
        assert_eq!(state.list_depth(), 2);
    }

    #[test]
    fn test_render_state_next_list_number() {
        let mut state = RenderState::default();
        state.push_container(
            NestingKind::List {
                ordered: true,
                item_count: 0,
            },
            0,
        );
        assert_eq!(state.next_list_number(), 1);
        assert_eq!(state.next_list_number(), 2);
        assert_eq!(state.next_list_number(), 3);
    }

    #[test]
    fn test_render_table_markdown_basic() {
        let cells = vec![
            vec!["A".to_string(), "B".to_string()],
            vec!["1".to_string(), "2".to_string()],
        ];
        let out = render_table_markdown(&cells);
        assert!(out.contains("| A | B |"), "got: {}", out);
        assert!(out.contains("| --- | --- |"), "got: {}", out);
        assert!(out.contains("| 1 | 2 |"), "got: {}", out);
    }

    #[test]
    fn test_render_table_markdown_empty() {
        let out = render_table_markdown(&[]);
        assert_eq!(out, "");
    }

    #[test]
    fn test_render_table_markdown_escapes_pipe() {
        let cells = vec![vec!["A|B".to_string()], vec!["C|D".to_string()]];
        let out = render_table_markdown(&cells);
        assert!(out.contains("A\\|B"), "pipe should be escaped, got: {}", out);
    }

    /// xberg-io/xberg#221: the grid is sized from the widest row, not the header.
    #[test]
    fn should_size_the_grid_from_the_widest_row() {
        let cells = vec![
            vec!["A".to_string(), "B".to_string()],
            vec!["1".to_string(), "2".to_string(), "3".to_string()],
        ];
        let out = render_table_markdown(&cells);
        assert_eq!(out, "| A | B |  |\n| --- | --- | --- |\n| 1 | 2 | 3 |\n");
    }

    /// xberg-io/xberg#163: a line break inside a cell must not end the row.
    #[test]
    fn should_replace_cell_line_breaks_with_a_break_tag() {
        let cells = vec![
            vec!["H".to_string()],
            vec!["x\ny".to_string()],
            vec!["p\r\nq".to_string()],
        ];
        let out = render_table_markdown(&cells);
        assert_eq!(out, "| H |\n| --- |\n| x<br>y |\n| p<br>q |\n");
        assert_eq!(out.lines().count(), 4, "embedded newlines must not add rows: {out}");
    }

    #[test]
    fn test_render_table_plain_basic() {
        let cells = vec![
            vec!["A".to_string(), "B".to_string()],
            vec!["1".to_string(), "2".to_string()],
        ];
        let out = render_table_plain(&cells);
        assert!(out.contains("A B"), "got: {}", out);
        assert!(out.contains("1 2"), "got: {}", out);
    }

    #[test]
    fn test_render_table_plain_empty() {
        let out = render_table_plain(&[]);
        assert_eq!(out, "");
    }

    #[test]
    fn test_footnote_collector_basic() {
        use crate::types::internal_builder::InternalDocumentBuilder;
        let mut b = InternalDocumentBuilder::new("test");
        b.push_footnote_ref("1", "fn1", None);
        let def = b.push_footnote_definition("Note text.", "fn1", None);
        b.set_layer(def, ContentLayer::Footnote);
        let doc = b.build();

        let collector = FootnoteCollector::new(&doc);
        assert_eq!(collector.ref_number(0), Some(1));
        let defs = collector.definitions();
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].text, "Note text.");
        assert_eq!(defs[0].number, 1);
    }

    #[test]
    fn test_footnote_collector_multiple() {
        use crate::types::internal_builder::InternalDocumentBuilder;
        let mut b = InternalDocumentBuilder::new("test");
        b.push_footnote_ref("a", "fn1", None);
        b.push_footnote_ref("b", "fn2", None);
        let d1 = b.push_footnote_definition("First.", "fn1", None);
        let d2 = b.push_footnote_definition("Second.", "fn2", None);
        b.set_layer(d1, ContentLayer::Footnote);
        b.set_layer(d2, ContentLayer::Footnote);
        let doc = b.build();

        let collector = FootnoteCollector::new(&doc);
        assert_eq!(collector.ref_number(0), Some(1));
        assert_eq!(collector.ref_number(1), Some(2));
        let defs = collector.definitions();
        assert_eq!(defs.len(), 2);
        assert_eq!(defs[0].number, 1);
        assert_eq!(defs[1].number, 2);
    }

    /// Regression for #68: a footnote definition that no `FootnoteRef` points at
    /// is still authored content and must be emitted, numbered after the
    /// referenced ones. Before the fix `definitions` was filled only from inside
    /// the reference loop, so "Orphaned." never reached a renderer.
    #[test]
    fn footnote_collector_emits_unreferenced_definitions() {
        use crate::types::internal_builder::InternalDocumentBuilder;
        let mut b = InternalDocumentBuilder::new("test");
        b.push_footnote_ref("a", "fn1", None);
        let d1 = b.push_footnote_definition("Referenced.", "fn1", None);
        let d2 = b.push_footnote_definition("Orphaned.", "fn2", None);
        b.set_layer(d1, ContentLayer::Footnote);
        b.set_layer(d2, ContentLayer::Footnote);
        let doc = b.build();

        let collector = FootnoteCollector::new(&doc);
        assert_eq!(collector.ref_number(0), Some(1));
        let defs = collector.definitions();
        assert_eq!(defs.len(), 2, "the unreferenced definition must still be emitted");
        assert_eq!(defs[0].text, "Referenced.");
        assert_eq!(defs[0].number, 1);
        assert_eq!(defs[1].text, "Orphaned.");
        assert_eq!(defs[1].number, 2);
    }

    /// A document with footnote definitions but no references at all still emits
    /// every definition, numbered from 1 in document order.
    #[test]
    fn footnote_collector_emits_definitions_when_no_references_exist() {
        use crate::types::internal_builder::InternalDocumentBuilder;
        let mut b = InternalDocumentBuilder::new("test");
        let d1 = b.push_footnote_definition("First orphan.", "fn1", None);
        let d2 = b.push_footnote_definition("Second orphan.", "fn2", None);
        b.set_layer(d1, ContentLayer::Footnote);
        b.set_layer(d2, ContentLayer::Footnote);
        let doc = b.build();

        let collector = FootnoteCollector::new(&doc);
        let defs = collector.definitions();
        assert_eq!(defs.len(), 2);
        assert_eq!(defs[0].text, "First orphan.");
        assert_eq!(defs[0].number, 1);
        assert_eq!(defs[1].text, "Second orphan.");
        assert_eq!(defs[1].number, 2);
    }

    /// Two definitions sharing one anchor must not be double-numbered by the
    /// orphan tail pass.
    #[test]
    fn footnote_collector_does_not_duplicate_shared_anchor_definitions() {
        use crate::types::internal_builder::InternalDocumentBuilder;
        let mut b = InternalDocumentBuilder::new("test");
        let d1 = b.push_footnote_definition("Only once.", "fn1", None);
        let d2 = b.push_footnote_definition("Duplicate anchor.", "fn1", None);
        b.set_layer(d1, ContentLayer::Footnote);
        b.set_layer(d2, ContentLayer::Footnote);
        let doc = b.build();

        let collector = FootnoteCollector::new(&doc);
        let defs = collector.definitions();
        assert_eq!(defs.len(), 1, "a repeated anchor must be numbered once");
        assert_eq!(defs[0].text, "Only once.");
        assert_eq!(defs[0].number, 1);
    }

    #[test]
    fn test_footnote_collector_no_footnotes() {
        use crate::types::internal_builder::InternalDocumentBuilder;
        let mut b = InternalDocumentBuilder::new("test");
        b.push_paragraph("No footnotes here", vec![], None, None);
        let doc = b.build();

        let collector = FootnoteCollector::new(&doc);
        assert!(collector.definitions().is_empty());
        assert_eq!(collector.ref_number(0), None);
    }

    #[test]
    fn test_normalize_inline_text_collapses_spaces() {
        assert_eq!(normalize_inline_text("Hello   world"), "Hello world");
    }

    #[test]
    fn test_normalize_inline_text_newlines_to_spaces() {
        assert_eq!(normalize_inline_text("Hello\nworld"), "Hello world");
    }

    #[test]
    fn test_normalize_inline_text_mixed_whitespace() {
        assert_eq!(normalize_inline_text("Hello \n  world"), "Hello world");
    }

    #[test]
    fn test_normalize_inline_text_strips_control_chars() {
        assert_eq!(normalize_inline_text("Hello\x02world"), "Helloworld");
    }

    #[test]
    fn test_normalize_inline_text_preserves_tabs() {
        assert_eq!(normalize_inline_text("Hello\tworld"), "Hello\tworld");
    }

    #[test]
    fn test_normalize_inline_text_empty() {
        assert_eq!(normalize_inline_text(""), "");
    }

    #[test]
    fn test_normalize_inline_text_no_change() {
        assert_eq!(normalize_inline_text("Hello world"), "Hello world");
    }

    #[test]
    fn test_annotation_type_label_highlight() {
        assert_eq!(
            annotation_type_label(crate::types::annotations::PdfAnnotationType::Highlight),
            "Highlight"
        );
    }

    #[test]
    fn test_annotation_type_label_previously_collapsed_variant() {
        assert_eq!(
            annotation_type_label(crate::types::annotations::PdfAnnotationType::Squiggly),
            "Squiggly"
        );
        assert_eq!(
            annotation_type_label(crate::types::annotations::PdfAnnotationType::FileAttachment),
            "FileAttachment"
        );
    }

    fn make_annotation(content: Option<&str>, marked_text: Option<&str>) -> crate::types::annotations::PdfAnnotation {
        crate::types::annotations::PdfAnnotation {
            annotation_type: crate::types::annotations::PdfAnnotationType::Highlight,
            content: content.map(str::to_string),
            page_number: 1,
            bounding_box: None,
            author: None,
            modified: None,
            color: None,
            subject: None,
            quad_points: None,
            marked_text: marked_text.map(str::to_string),
        }
    }

    #[test]
    fn test_annotation_display_text_prefers_marked_text() {
        let annotation = make_annotation(Some("a comment"), Some("the highlighted words"));
        assert_eq!(annotation_display_text(&annotation), Some("the highlighted words"));
    }

    #[test]
    fn test_annotation_display_text_falls_back_to_content() {
        let annotation = make_annotation(Some("a comment"), None);
        assert_eq!(annotation_display_text(&annotation), Some("a comment"));
    }

    #[test]
    fn test_annotation_display_text_none_when_both_absent() {
        let annotation = make_annotation(None, None);
        assert_eq!(annotation_display_text(&annotation), None);
    }

    #[test]
    fn test_escape_html_text_no_special_chars_returns_borrowed() {
        let result = escape_html_text("plain text");
        assert!(matches!(result, Cow::Borrowed(_)));
        assert_eq!(result, "plain text");
    }

    #[test]
    fn test_escape_html_text_escapes_ampersand_and_angle_brackets() {
        let result = escape_html_text("a < b & c > d");
        assert_eq!(result, "a &lt; b &amp; c &gt; d");
    }
}
