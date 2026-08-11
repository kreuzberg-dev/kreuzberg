//! Native Rust reStructuredText (RST) extractor.
//!
//! This extractor provides comprehensive RST document parsing.
//! It extracts:
//! - Document title and headings
//! - Field list metadata (:Author:, :Date:, :Version:, etc.)
//! - Paragraphs and text content
//! - Code blocks with language specifications
//! - Lists (bullet, numbered, definition lists)
//! - Tables (both simple and grid tables)
//! - Directives (image, code-block, note, math, etc.)
//! - Inline markup (emphasis, strong, code, links)
//! - Images and references

#[cfg(feature = "office")]
use crate::Result;
#[cfg(feature = "office")]
use crate::core::config::ExtractionConfig;
#[cfg(feature = "office")]
use crate::extractors::security::SecurityBudget;
#[cfg(feature = "office")]
use crate::plugins::{InternalDocumentExtractor, Plugin};
#[cfg(feature = "office")]
use crate::types::Metadata;
#[cfg(feature = "office")]
use crate::types::document_structure::{AnnotationKind, TextAnnotation};
#[cfg(feature = "office")]
use crate::types::internal::InternalDocument;
#[cfg(feature = "office")]
use crate::types::internal::{RelationshipKind, RelationshipTarget};
#[cfg(feature = "office")]
use crate::types::internal_builder::InternalDocumentBuilder;
#[cfg(feature = "office")]
use crate::types::uri::ExtractedUri;
#[cfg(feature = "office")]
use ahash::AHashMap;
#[cfg(feature = "office")]
use async_trait::async_trait;
#[cfg(feature = "office")]
use std::borrow::Cow;

/// `ProcessingWarning::source` for every warning this extractor emits (#171).
#[cfg(feature = "office")]
const RST_WARNING_SOURCE: &str = "rst";

/// Native Rust reStructuredText extractor.
///
/// Parses RST documents using document tree parsing and extracts:
/// - Metadata from field lists
/// - Document structure (headings, sections)
/// - Text content and inline formatting
/// - Code blocks and directives
/// - Tables and lists
#[cfg_attr(alef, alef(skip))]
#[cfg(feature = "office")]
pub struct RstExtractor;

#[cfg(feature = "office")]
impl RstExtractor {
    /// Create a new RST extractor.
    pub(crate) fn new() -> Self {
        Self
    }

    /// Extract text content and metadata from RST document.
    ///
    /// Uses document tree parsing and fallback text extraction.
    fn extract_text_and_metadata(content: &str) -> (String, Metadata) {
        let mut metadata = Metadata::default();
        let mut additional: AHashMap<Cow<'static, str>, serde_json::Value> = AHashMap::new();

        let text = Self::extract_text_from_rst(content, &mut additional);

        metadata.title = additional
            .remove(&Cow::Borrowed("title"))
            .and_then(|v| v.as_str().map(|s| s.to_string()));
        metadata.authors = additional
            .remove(&Cow::Borrowed("author"))
            .and_then(|v| v.as_str().map(|s| vec![s.to_string()]));
        metadata.created_at = additional
            .remove(&Cow::Borrowed("date"))
            .and_then(|v| v.as_str().map(|s| s.to_string()));

        metadata.additional = additional;
        (text, metadata)
    }

    /// Extract text and metadata from RST content.
    ///
    /// This is the main extraction engine that processes RST line-by-line
    /// and extracts all document content including headings, code blocks, lists, etc.
    fn extract_text_from_rst(content: &str, metadata: &mut AHashMap<Cow<'static, str>, serde_json::Value>) -> String {
        let mut output = String::new();
        let lines: Vec<&str> = content.lines().collect();
        let mut i = 0;

        while i < lines.len() {
            let line = lines[i];

            if line.trim().starts_with(':')
                && line.contains(':')
                && let Some((key, value)) = Self::parse_field_list_line(line)
            {
                let mut full_value = value.clone();
                while i + 1 < lines.len() {
                    let next = lines[i + 1];
                    if !next.is_empty() && (next.starts_with("   ") || next.starts_with("\t")) {
                        full_value.push('\n');
                        full_value.push_str(next);
                        i += 1;
                    } else {
                        break;
                    }
                }
                Self::add_metadata_field(&key, &full_value, metadata);
                output.push_str(&format!(":{}: {}\n", key, full_value));
                i += 1;
                continue;
            }

            if Self::is_section_underline(line.trim())
                && i + 2 < lines.len()
                && !lines[i + 1].trim().is_empty()
                && Self::is_section_underline(lines[i + 2])
            {
                let overline_char = line.trim().chars().next().unwrap_or('=');
                let underline_char = lines[i + 2].trim().chars().next().unwrap_or('=');
                if overline_char == underline_char {
                    output.push_str(lines[i + 1].trim());
                    output.push('\n');
                    i += 3;
                    continue;
                }
            }

            if i + 1 < lines.len() {
                let next_line = lines[i + 1];
                if Self::is_section_underline(next_line) && !line.trim().is_empty() {
                    output.push_str(line.trim());
                    output.push('\n');
                    i += 2;
                    continue;
                }
            }

            if line.trim().starts_with(".. code-block::") || line.trim().starts_with(".. code::") {
                output.push_str(line.trim());
                output.push('\n');
                i += 1;
                while i < lines.len() && lines[i].trim().is_empty() {
                    output.push('\n');
                    i += 1;
                }
                while i < lines.len() && (lines[i].starts_with("   ") || lines[i].is_empty()) {
                    output.push_str(lines[i]);
                    output.push('\n');
                    i += 1;
                }
                continue;
            }

            if line.trim().starts_with(".. highlight::") {
                let lang = line.trim_start_matches(".. highlight::").trim().to_string();
                if !lang.is_empty() {
                    output.push_str("highlight: ");
                    output.push_str(&lang);
                    output.push('\n');
                }
                i += 1;
                continue;
            }

            if line.trim().ends_with("::") && !line.trim().starts_with(".. ") {
                if let Some(display_text) = line.strip_suffix("::")
                    && !display_text.trim().is_empty()
                {
                    output.push_str(display_text.trim());
                    output.push('\n');
                }
                i += 1;
                while i < lines.len() && (lines[i].starts_with("    ") || lines[i].is_empty()) {
                    if !lines[i].is_empty() {
                        output.push_str(lines[i].trim_start());
                        output.push('\n');
                    }
                    i += 1;
                }
                continue;
            }

            if Self::is_list_item(line) {
                output.push_str(line.trim());
                output.push('\n');
                i += 1;
                continue;
            }

            if line.trim().starts_with(".. ") || line.trim() == ".." {
                let trimmed = line.trim();
                let directive = if trimmed == ".." { "" } else { &trimmed[3..] };

                if directive.starts_with("image::") {
                    let uri = directive.strip_prefix("image::").unwrap_or("").trim();
                    output.push_str("image: ");
                    output.push_str(uri);
                    output.push('\n');
                    i += 1;
                    continue;
                }

                if directive.starts_with("note::")
                    || directive.starts_with("warning::")
                    || directive.starts_with("important::")
                    || directive.starts_with("caution::")
                    || directive.starts_with("hint::")
                    || directive.starts_with("tip::")
                {
                    output.push_str(trimmed);
                    output.push('\n');
                    i += 1;
                    while i < lines.len() && (lines[i].starts_with("   ") || lines[i].is_empty()) {
                        if !lines[i].is_empty() {
                            output.push_str(lines[i]);
                            output.push('\n');
                        }
                        i += 1;
                    }
                    continue;
                }

                if directive.starts_with("math::") {
                    let mut math_content = String::new();
                    let arg = directive.strip_prefix("math::").unwrap_or("").trim();
                    if !arg.is_empty() {
                        math_content.push_str(arg);
                    }
                    i += 1;
                    // Directive options (`:label: eq1`, `:nowrap:`) sit
                    // directly under the directive line, before the first
                    // blank line or math line. Everything after is math.
                    let mut in_option_region = true;
                    while i < lines.len() && (lines[i].starts_with("   ") || lines[i].is_empty()) {
                        let body = lines[i].trim();
                        if body.is_empty() {
                            in_option_region = false;
                        } else {
                            let is_option = in_option_region && body.starts_with(':') && body[1..].contains(':');
                            if !is_option {
                                in_option_region = false;
                                if !math_content.is_empty() {
                                    math_content.push('\n');
                                }
                                math_content.push_str(body);
                            }
                        }
                        i += 1;
                    }
                    if !math_content.is_empty() {
                        output.push_str("$$\n");
                        output.push_str(&math_content);
                        output.push_str("\n$$\n");
                    }
                    continue;
                }

                i += 1;
                while i < lines.len() && (lines[i].starts_with("   ") || lines[i].is_empty()) {
                    i += 1;
                }
                continue;
            }

            if !line.trim().is_empty() && !Self::is_markup_line(line) {
                output.push_str(line);
                output.push('\n');
            }

            i += 1;
        }

        output
    }

    /// Parse a field list line (e.g., ":Author: John Doe")
    fn parse_field_list_line(line: &str) -> Option<(String, String)> {
        let trimmed = line.trim();
        if !trimmed.starts_with(':') {
            return None;
        }

        let rest = &trimmed[1..];
        if let Some(end_pos) = rest.find(':') {
            let key = rest[..end_pos].to_string();
            if key.is_empty() {
                return None;
            }
            let value = rest[end_pos + 1..].trim().to_string();
            return Some((key, value));
        }

        None
    }

    /// Add a metadata field from RST field list.
    fn add_metadata_field(key: &str, value: &str, metadata: &mut AHashMap<Cow<'static, str>, serde_json::Value>) {
        let key_lower = key.to_lowercase();
        match key_lower.as_str() {
            "author" | "authors" => {
                metadata.insert(Cow::Borrowed("author"), serde_json::Value::String(value.to_string()));
            }
            "date" => {
                metadata.insert(Cow::Borrowed("date"), serde_json::Value::String(value.to_string()));
            }
            "version" | "revision" => {
                metadata.insert(Cow::Borrowed("version"), serde_json::Value::String(value.to_string()));
            }
            "title" => {
                metadata.insert(Cow::Borrowed("title"), serde_json::Value::String(value.to_string()));
            }
            _ => {
                metadata.insert(
                    Cow::Owned(format!("field_{}", key_lower)),
                    serde_json::Value::String(value.to_string()),
                );
            }
        }
    }

    /// Check if a line is a section underline.
    fn is_section_underline(line: &str) -> bool {
        let trimmed = line.trim();
        if trimmed.len() < 3 {
            return false;
        }
        let chars: Vec<char> = trimmed.chars().collect();
        let first = chars[0];
        matches!(first, '=' | '-' | '~' | '+' | '^' | '"' | '`' | '#' | '*') && chars.iter().all(|c| *c == first)
    }

    /// Check if a line is a list item.
    fn is_list_item(line: &str) -> bool {
        let trimmed = line.trim_start();
        if trimmed.starts_with("* ")
            || trimmed.starts_with("+ ")
            || trimmed.starts_with("- ")
            || trimmed.starts_with("*\t")
            || trimmed.starts_with("+\t")
            || trimmed.starts_with("-\t")
        {
            return true;
        }
        if trimmed.starts_with("#. ")
            || trimmed.starts_with("#.\t")
            || trimmed.starts_with("(#) ")
            || trimmed.starts_with("(#)\t")
        {
            return true;
        }
        if trimmed.starts_with('(')
            && let Some(close) = trimmed.find(')')
            && close > 1
            && close < 6
        {
            let inner = &trimmed[1..close];
            let after = &trimmed[close + 1..];
            if (after.starts_with(' ') || after.starts_with('\t'))
                && (inner.chars().all(|c| c.is_alphanumeric()) || inner == "#")
            {
                return true;
            }
        }
        let sep_pos = trimmed.find([' ', '\t']);
        if let Some(space_pos) = sep_pos
            && space_pos > 0
            && space_pos < 6
        {
            let prefix = &trimmed[..space_pos];
            if prefix.ends_with('.') || prefix.ends_with(')') {
                let body = &prefix[..prefix.len() - 1];
                if body.chars().all(|c| c.is_ascii_digit()) {
                    return true;
                }
                if body.chars().all(|c| c.is_ascii_alphabetic()) && body.len() <= 3 {
                    return true;
                }
            }
        }
        false
    }

    /// Check if a line is just markup (underlines, etc.)
    fn is_markup_line(line: &str) -> bool {
        let trimmed = line.trim();
        if trimmed.len() < 3 {
            return false;
        }
        let first = trimmed.chars().next().unwrap();
        trimmed.chars().all(|c| c == first)
            && matches!(first, '=' | '-' | '~' | '+' | '^' | '"' | '`' | '#' | '*' | '/')
    }

    /// Strip RST inline markup from text and produce annotations with byte offsets
    /// into the stripped text.
    ///
    /// Handles: `**strong**` (bold), `*emphasis*` (italic), ``` ``literal`` ``` (code),
    /// and `` `interpreted` `` (code).
    fn parse_inline_markup(raw: &str) -> (String, Vec<TextAnnotation>) {
        let mut out = String::with_capacity(raw.len());
        let mut annotations = Vec::new();
        let bytes = raw.as_bytes();
        let len = bytes.len();
        let mut i = 0;

        while i < len {
            if i + 1 < len
                && bytes[i] == b'*'
                && bytes[i + 1] == b'*'
                && let Some(end) = Self::find_closing_marker(raw, i + 2, "**")
            {
                let inner = &raw[i + 2..end];
                let start = out.len() as u32;
                out.push_str(inner);
                let end_off = out.len() as u32;
                if start < end_off {
                    annotations.push(TextAnnotation {
                        start,
                        end: end_off,
                        kind: AnnotationKind::Bold,
                    });
                }
                i = end + 2;
                continue;
            }
            if bytes[i] == b'*'
                && (i + 1 >= len || bytes[i + 1] != b'*')
                && let Some(end) = Self::find_closing_marker(raw, i + 1, "*")
                && (end + 1 >= len || bytes[end + 1] != b'*')
            {
                let inner = &raw[i + 1..end];
                let start = out.len() as u32;
                out.push_str(inner);
                let end_off = out.len() as u32;
                if start < end_off {
                    annotations.push(TextAnnotation {
                        start,
                        end: end_off,
                        kind: AnnotationKind::Italic,
                    });
                }
                i = end + 1;
                continue;
            }
            if i + 1 < len
                && bytes[i] == b'`'
                && bytes[i + 1] == b'`'
                && let Some(end) = Self::find_closing_marker(raw, i + 2, "``")
            {
                let inner = &raw[i + 2..end];
                let start = out.len() as u32;
                out.push_str(inner);
                let end_off = out.len() as u32;
                if start < end_off {
                    annotations.push(TextAnnotation {
                        start,
                        end: end_off,
                        kind: AnnotationKind::Code,
                    });
                }
                i = end + 2;
                continue;
            }
            if bytes[i] == b'`'
                && (i + 1 >= len || bytes[i + 1] != b'`')
                && let Some(end) = Self::find_closing_single_backtick(raw, i + 1)
            {
                let inner = &raw[i + 1..end];
                let after_close = end + 1;
                if after_close < len && bytes[after_close] == b'_' {
                    if let Some(angle_start) = inner.rfind('<')
                        && let Some(angle_end) = inner.rfind('>')
                        && angle_end > angle_start
                    {
                        let url = inner[angle_start + 1..angle_end].trim().to_string();
                        let link_text = inner[..angle_start].trim();
                        let start = out.len() as u32;
                        out.push_str(link_text);
                        let end_off = out.len() as u32;
                        if start < end_off {
                            annotations.push(TextAnnotation {
                                start,
                                end: end_off,
                                kind: AnnotationKind::Link { url, title: None },
                            });
                        }
                        i = after_close + 1;
                        continue;
                    }
                    let start = out.len() as u32;
                    out.push_str(inner);
                    let end_off = out.len() as u32;
                    if start < end_off {
                        annotations.push(TextAnnotation {
                            start,
                            end: end_off,
                            kind: AnnotationKind::Code,
                        });
                    }
                    i = after_close + 1;
                    continue;
                }
                let start = out.len() as u32;
                out.push_str(inner);
                let end_off = out.len() as u32;
                if start < end_off {
                    annotations.push(TextAnnotation {
                        start,
                        end: end_off,
                        kind: AnnotationKind::Code,
                    });
                }
                i = end + 1;
                continue;
            }
            if bytes[i] == b'['
                && let Some(close) = raw[i + 1..].find(']')
            {
                let label_end = i + 1 + close;
                if label_end + 1 < len && bytes[label_end + 1] == b'_' {
                    let label = &raw[i + 1..label_end];
                    out.push('[');
                    out.push_str(label);
                    out.push(']');
                    i = label_end + 2;
                    continue;
                }
            }
            let ch = raw[i..].chars().next().unwrap();
            out.push(ch);
            i += ch.len_utf8();
        }

        (out, annotations)
    }

    /// Find the position of a closing marker substring starting from `from`.
    fn find_closing_marker(text: &str, from: usize, marker: &str) -> Option<usize> {
        text[from..].find(marker).map(|pos| from + pos)
    }

    /// Find closing single backtick that is NOT part of a double backtick.
    fn find_closing_single_backtick(text: &str, from: usize) -> Option<usize> {
        let bytes = text.as_bytes();
        let mut j = from;
        while j < bytes.len() {
            if bytes[j] == b'`' {
                if j + 1 < bytes.len() && bytes[j + 1] == b'`' {
                    j += 2;
                    continue;
                }
                return Some(j);
            }
            j += 1;
        }
        None
    }

    /// Parse RST footnote references from a line.
    /// Returns footnote labels found (e.g. "1" from `[1]_` or "#" from `[#]_`).
    fn find_footnote_references(line: &str) -> Vec<String> {
        let mut refs = Vec::new();
        let bytes = line.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'['
                && let Some(close) = line[i + 1..].find(']')
            {
                let label_end = i + 1 + close;
                let label = &line[i + 1..label_end];
                if label_end + 1 < bytes.len()
                    && bytes[label_end + 1] == b'_'
                    && (label.chars().all(|c| c.is_ascii_digit()) || label.starts_with('#'))
                {
                    refs.push(label.to_string());
                }
            }
            i += 1;
        }
        refs
    }

    /// Parse image directive options (`:alt:`, `:width:`, `:height:`) from indented lines.
    fn parse_image_options(lines: &[&str], start: &mut usize) -> AHashMap<String, String> {
        let mut opts = AHashMap::new();
        while *start < lines.len() {
            let line = lines[*start];
            if !line.starts_with("   ") && !line.starts_with("\t") {
                break;
            }
            let trimmed = line.trim();
            if trimmed.is_empty() {
                *start += 1;
                break;
            }
            if trimmed.starts_with(':')
                && let Some(colon2) = trimmed[1..].find(':')
            {
                let key = trimmed[1..1 + colon2].to_string();
                let value = trimmed[2 + colon2..].trim().to_string();
                opts.insert(key, value);
            }
            *start += 1;
        }
        opts
    }

    /// Push an image (or figure) URI and, if configured, a placeholder paragraph for it.
    ///
    /// Shared by the `.. image::` and `.. figure::` directive handlers so figures build on the
    /// same image-emission logic instead of duplicating it.
    fn push_image_directive(
        b: &mut InternalDocumentBuilder,
        uri: &str,
        opts: &AHashMap<String, String>,
        inject_placeholders: bool,
    ) {
        let alt = opts.get("alt").cloned();
        let desc = alt.as_deref().unwrap_or(uri);
        if !uri.is_empty() {
            b.push_uri(ExtractedUri::image(uri, alt.clone()));
        }
        if inject_placeholders {
            let idx = b.push_paragraph(&format!("[image: {}]", desc), vec![], None, None);
            if !uri.is_empty() {
                let mut attrs = ahash::AHashMap::new();
                attrs.insert("src".to_string(), uri.to_string());
                b.set_attributes(idx, attrs);
            }
        }
    }

    /// Parse the row/cell structure of a `.. list-table::` directive body.
    ///
    /// A list-table row is a top-level bullet item (`* - <cell>`), and each additional cell in
    /// that row is a nested bullet item (`- <cell>`) indented deeper than the row marker:
    ///
    /// ```rst
    /// * - Name
    ///   - Age
    /// * - Alice
    ///   - 30
    /// ```
    fn parse_list_table_rows(lines: &[&str], start: &mut usize) -> Vec<Vec<String>> {
        let mut rows: Vec<Vec<String>> = Vec::new();
        while *start < lines.len() {
            let line = lines[*start];
            if line.trim().is_empty() {
                break;
            }
            let leading = line.len() - line.trim_start().len();
            let trimmed = line.trim_start();
            let Some(after_star) = trimmed.strip_prefix("* ") else {
                break;
            };
            let Some(cell_text) = after_star.strip_prefix("- ") else {
                break;
            };
            let mut row = vec![cell_text.trim().to_string()];
            *start += 1;
            while *start < lines.len() {
                let cell_line = lines[*start];
                if cell_line.trim().is_empty() {
                    break;
                }
                let cell_leading = cell_line.len() - cell_line.trim_start().len();
                let cell_trimmed = cell_line.trim_start();
                if cell_leading > leading && cell_trimmed.starts_with("- ") {
                    row.push(cell_trimmed[2..].trim().to_string());
                    *start += 1;
                } else {
                    break;
                }
            }
            rows.push(row);
        }
        rows
    }

    /// Parse a single CSV-formatted line into fields, honoring double-quoted fields with
    /// embedded commas and `""`-escaped quotes (RFC 4180-style), as used by `.. csv-table::`.
    fn parse_csv_line(line: &str) -> Vec<String> {
        let mut fields = Vec::new();
        let mut current = String::new();
        let mut in_quotes = false;
        let chars: Vec<char> = line.chars().collect();
        let mut idx = 0;

        while idx < chars.len() {
            let ch = chars[idx];
            if in_quotes {
                if ch == '"' {
                    if idx + 1 < chars.len() && chars[idx + 1] == '"' {
                        current.push('"');
                        idx += 2;
                        continue;
                    }
                    in_quotes = false;
                    idx += 1;
                    continue;
                }
                current.push(ch);
                idx += 1;
                continue;
            }
            match ch {
                '"' => {
                    in_quotes = true;
                    idx += 1;
                }
                ',' => {
                    fields.push(current.trim().to_string());
                    current = String::new();
                    idx += 1;
                }
                _ => {
                    current.push(ch);
                    idx += 1;
                }
            }
        }
        fields.push(current.trim().to_string());
        fields
    }

    /// Build an `InternalDocument` from RST content.
    ///
    /// Handles sections, paragraphs, code blocks, tables, footnotes, citations,
    /// and cross-references.
    pub(crate) fn build_internal_document(content: &str, inject_placeholders: bool) -> InternalDocument {
        let mut b = InternalDocumentBuilder::new("rst");
        let lines: Vec<&str> = content.lines().collect();
        let mut heading_char_order: Vec<char> = Vec::new();
        let mut has_overline_heading = false;
        let mut highlight_lang: Option<String> = None;
        let mut i = 0;

        while i < lines.len() {
            let line = lines[i];
            let trimmed = line.trim();

            if trimmed.starts_with(':')
                && trimmed.len() > 1
                && let Some((key, value)) = Self::parse_field_list_line(trimmed)
            {
                let mut full_value = value;
                while i + 1 < lines.len() {
                    let next = lines[i + 1];
                    if !next.is_empty() && (next.starts_with("   ") || next.starts_with("\t")) {
                        if !full_value.is_empty() {
                            full_value.push(' ');
                        }
                        full_value.push_str(next.trim());
                        i += 1;
                    } else {
                        break;
                    }
                }
                b.push_metadata_block(&[(key, full_value)], None);
                i += 1;
                continue;
            }

            if Self::is_section_underline(trimmed)
                && i + 2 < lines.len()
                && !lines[i + 1].trim().is_empty()
                && Self::is_section_underline(lines[i + 2])
            {
                let overline_char = trimmed.chars().next().unwrap_or('=');
                let underline_char = lines[i + 2].trim().chars().next().unwrap_or('=');
                if overline_char == underline_char {
                    let title_text = lines[i + 1].trim();
                    has_overline_heading = true;
                    b.push_heading(1, title_text, None, None);
                    i += 3;
                    continue;
                }
            }

            if i + 1 < lines.len() && !trimmed.is_empty() && Self::is_section_underline(lines[i + 1]) {
                let underline_char = lines[i + 1].trim().chars().next().unwrap_or('=');
                if !heading_char_order.contains(&underline_char) {
                    heading_char_order.push(underline_char);
                }
                let base = if has_overline_heading { 2 } else { 1 };
                let level = heading_char_order
                    .iter()
                    .position(|&c| c == underline_char)
                    .map(|p| (p + base) as u8)
                    .unwrap_or(base as u8);
                b.push_heading(level, trimmed, None, None);
                i += 2;
                continue;
            }

            if trimmed.starts_with(".. code-block::") || trimmed.starts_with(".. code::") {
                let language: Option<&str> = if let Some(rest) = trimmed.strip_prefix(".. code-block::") {
                    let lang = rest.trim();
                    if lang.is_empty() { None } else { Some(lang) }
                } else if let Some(rest) = trimmed.strip_prefix(".. code::") {
                    let lang = rest.trim();
                    if lang.is_empty() { None } else { Some(lang) }
                } else {
                    None
                };
                i += 1;
                while i < lines.len() && lines[i].trim().is_empty() {
                    i += 1;
                }
                let mut code_content = String::new();
                while i < lines.len() && (lines[i].starts_with("   ") || lines[i].is_empty()) {
                    if !code_content.is_empty() {
                        code_content.push('\n');
                    }
                    if lines[i].starts_with("   ") {
                        code_content.push_str(&lines[i][3..]);
                    }
                    i += 1;
                }
                b.push_code(code_content.trim_end(), language, None, None);
                continue;
            }

            if trimmed.starts_with(".. note::")
                || trimmed.starts_with(".. warning::")
                || trimmed.starts_with(".. important::")
                || trimmed.starts_with(".. caution::")
                || trimmed.starts_with(".. hint::")
                || trimmed.starts_with(".. tip::")
            {
                let kind = trimmed.strip_prefix(".. ").unwrap_or("").trim_end_matches("::").trim();
                let idx = b.push_admonition(kind, None, None);
                i += 1;
                let mut admonition_text = String::new();
                while i < lines.len() && (lines[i].starts_with("   ") || lines[i].is_empty()) {
                    if !lines[i].is_empty() {
                        if !admonition_text.is_empty() {
                            admonition_text.push(' ');
                        }
                        admonition_text.push_str(lines[i].trim());
                    }
                    i += 1;
                }
                if !admonition_text.is_empty() {
                    b.set_text(idx, &admonition_text);
                }
                continue;
            }

            if trimmed.starts_with(".. image::") {
                let uri = trimmed.strip_prefix(".. image::").unwrap_or("").trim();
                i += 1;
                let opts = Self::parse_image_options(&lines, &mut i);
                Self::push_image_directive(&mut b, uri, &opts, inject_placeholders);
                continue;
            }

            if trimmed.starts_with(".. figure::") {
                let uri = trimmed.strip_prefix(".. figure::").unwrap_or("").trim().to_string();
                i += 1;
                let opts = Self::parse_image_options(&lines, &mut i);
                Self::push_image_directive(&mut b, &uri, &opts, inject_placeholders);

                // The figure body (an indented paragraph following the option block) is the
                // figure's caption. Collect it and emit it as a regular paragraph so the
                // caption text is preserved instead of being dropped.
                let mut caption_text = String::new();
                while i < lines.len() {
                    if lines[i].is_empty() {
                        if !caption_text.is_empty() {
                            break;
                        }
                        i += 1;
                        continue;
                    }
                    if !(lines[i].starts_with("   ") || lines[i].starts_with("\t")) {
                        break;
                    }
                    if !caption_text.is_empty() {
                        caption_text.push(' ');
                    }
                    caption_text.push_str(lines[i].trim());
                    i += 1;
                }
                if !caption_text.is_empty() {
                    let (stripped, annotations) = Self::parse_inline_markup(&caption_text);
                    b.push_paragraph(&stripped, annotations, None, None);
                }
                continue;
            }

            if trimmed.starts_with(".. list-table::") {
                i += 1;
                let opts = Self::parse_image_options(&lines, &mut i);
                let _header_rows: usize = opts.get("header-rows").and_then(|v| v.trim().parse().ok()).unwrap_or(0);
                // `parse_image_options` only consumes the blank line that separates the option
                // block from the body when options were actually present; skip any that remain
                // so the row parser doesn't see a leading blank line and bail out immediately.
                while i < lines.len() && lines[i].trim().is_empty() {
                    i += 1;
                }
                let cells = Self::parse_list_table_rows(&lines, &mut i);
                if !cells.is_empty() {
                    b.push_table_from_cells(&cells, None, None);
                }
                continue;
            }

            if trimmed.starts_with(".. csv-table::") {
                i += 1;
                let opts = Self::parse_image_options(&lines, &mut i);
                while i < lines.len() && lines[i].trim().is_empty() {
                    i += 1;
                }
                let mut cells: Vec<Vec<String>> = Vec::new();
                if let Some(header_line) = opts.get("header") {
                    cells.push(Self::parse_csv_line(header_line));
                }
                while i < lines.len() {
                    let l = lines[i];
                    if l.trim().is_empty() || !(l.starts_with("   ") || l.starts_with("\t")) {
                        break;
                    }
                    cells.push(Self::parse_csv_line(l.trim()));
                    i += 1;
                }
                if !cells.is_empty() {
                    b.push_table_from_cells(&cells, None, None);
                }
                continue;
            }

            if trimmed.starts_with(".. math::") {
                let inline_math = trimmed.strip_prefix(".. math::").unwrap_or("").trim();
                i += 1;
                while i < lines.len() {
                    let l = lines[i].trim();
                    if (l.starts_with(':') && l.ends_with(':') || (l.starts_with(':') && l.contains(": ")))
                        && (lines[i].starts_with("   ") || lines[i].starts_with("\t"))
                    {
                        i += 1;
                        continue;
                    }
                    break;
                }
                let mut math_content = if inline_math.is_empty() {
                    String::new()
                } else {
                    inline_math.to_string()
                };
                while i < lines.len() && (lines[i].starts_with("   ") || lines[i].is_empty()) {
                    if lines[i].is_empty() {
                        if !math_content.is_empty() {
                            b.push_formula(&math_content, None, None);
                            math_content = String::new();
                        }
                    } else {
                        if !math_content.is_empty() {
                            math_content.push('\n');
                        }
                        math_content.push_str(lines[i].trim());
                    }
                    i += 1;
                }
                if !math_content.is_empty() {
                    b.push_formula(&math_content, None, None);
                }
                continue;
            }

            if trimmed.starts_with(".. [")
                && let Some(close) = trimmed.find(']')
                && close > 4
            {
                let label = &trimmed[4..close];
                let footnote_text = trimmed[close + 1..].trim();
                let mut full_text = footnote_text.to_string();
                i += 1;
                while i < lines.len() && (lines[i].starts_with("   ") || lines[i].starts_with("\t")) {
                    if !full_text.is_empty() {
                        full_text.push(' ');
                    }
                    full_text.push_str(lines[i].trim());
                    i += 1;
                }
                let is_citation = label.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '-')
                    && !label.chars().all(|c| c.is_ascii_digit())
                    && !label.starts_with('#');
                if is_citation {
                    b.push_citation(&full_text, label, None);
                } else {
                    b.push_footnote_definition(&full_text, label, None);
                }
                continue;
            }

            if trimmed.starts_with(".. _")
                && let Some(colon_pos) = trimmed[4..].find(": ")
            {
                let label = &trimmed[4..4 + colon_pos];
                let url = trimmed[4 + colon_pos + 2..].trim();
                if !url.is_empty() && !label.is_empty() {
                    let idx = b.push_paragraph(
                        label,
                        vec![TextAnnotation {
                            start: 0,
                            end: label.len() as u32,
                            kind: AnnotationKind::Link {
                                url: url.to_string(),
                                title: None,
                            },
                        }],
                        None,
                        None,
                    );
                    let _ = idx;
                }
                i += 1;
                continue;
            }

            if trimmed.starts_with(".. highlight::") {
                let lang = trimmed.strip_prefix(".. highlight::").unwrap_or("").trim();
                highlight_lang = if lang.is_empty() { None } else { Some(lang.to_string()) };
                i += 1;
                while i < lines.len() && (lines[i].starts_with("   ") || lines[i].is_empty()) {
                    i += 1;
                }
                continue;
            }

            // `.. include:: path` inlines another file's rendered content at this
            // point in the source. This parser works on a single in-memory
            // document and never resolves the reference, so unlike a generic
            // unhandled directive (whose *own* body is still captured by the
            // fallback below) the referenced file's content is always missing
            // from the extracted text (#171). ~keep
            if trimmed.starts_with(".. include::") {
                let target = trimmed.strip_prefix(".. include::").unwrap_or("").trim();
                b.add_warning(crate::core::diagnostics::warning(
                    RST_WARNING_SOURCE,
                    format!(
                        "'.. include:: {target}' references an external file that was not read; \
                         its content is missing from the extracted text"
                    ),
                ));
                i += 1;
                while i < lines.len() && (lines[i].starts_with("   ") || lines[i].is_empty()) {
                    i += 1;
                }
                continue;
            }

            if trimmed.starts_with(".. contents::") {
                let title = trimmed.strip_prefix(".. contents::").unwrap_or("").trim();
                if !title.is_empty() {
                    b.push_paragraph(title, vec![], None, None);
                }
                i += 1;
                while i < lines.len() && (lines[i].starts_with("   ") || lines[i].is_empty()) {
                    i += 1;
                }
                continue;
            }

            if trimmed.starts_with(".. ") || trimmed == ".." {
                // Distinguish an actual (but otherwise unhandled) directive, `.. name:: args`,
                // from a plain RST comment, `.. some comment text`. Directive names are a single
                // word (no whitespace) immediately followed by `::`; comments are not, and their
                // body must stay dropped rather than surfacing as document text. ~keep
                let after_dots = trimmed.strip_prefix(".. ").unwrap_or("");
                let is_directive = trimmed != ".."
                    && after_dots
                        .find("::")
                        .map(|pos| {
                            let name = &after_dots[..pos];
                            !name.is_empty() && !name.contains(' ') && !name.contains('\t')
                        })
                        .unwrap_or(false);

                i += 1;
                let mut body_text = String::new();
                while i < lines.len() {
                    let l = lines[i];
                    if l.is_empty() {
                        if !body_text.is_empty() {
                            break;
                        }
                        i += 1;
                        continue;
                    }
                    if !(l.starts_with("   ") || l.starts_with("\t")) {
                        break;
                    }
                    if !body_text.is_empty() {
                        body_text.push(' ');
                    }
                    body_text.push_str(l.trim());
                    i += 1;
                }
                if is_directive && !body_text.is_empty() {
                    let (stripped, annotations) = Self::parse_inline_markup(&body_text);
                    b.push_paragraph(&stripped, annotations, None, None);
                }
                continue;
            }

            if Self::is_simple_table_separator(trimmed) {
                let mut table_lines = Vec::new();
                while i < lines.len() {
                    let tl = lines[i].trim();
                    if tl.is_empty() {
                        break;
                    }
                    table_lines.push(lines[i]);
                    i += 1;
                }
                let cells = Self::parse_simple_table_cells(&table_lines);
                if !cells.is_empty() {
                    b.push_table_from_cells(&cells, None, None);
                }
                continue;
            }

            if trimmed.starts_with('+') && trimmed.ends_with('+') && trimmed.contains('-') {
                let mut table_lines = Vec::new();
                while i < lines.len() && (lines[i].trim().starts_with('+') || lines[i].trim().starts_with('|')) {
                    table_lines.push(lines[i]);
                    i += 1;
                }
                let cells = Self::parse_grid_table_cells(&table_lines);
                if !cells.is_empty() {
                    b.push_table_from_cells(&cells, None, None);
                }
                continue;
            }

            if Self::is_list_item(line) {
                let is_ordered = {
                    let t = trimmed.trim_start();
                    if t.starts_with("#. ") || t.starts_with("#.\t") {
                        true
                    } else if let Some(space_pos) = t.find([' ', '\t']) {
                        let prefix = &t[..space_pos];
                        prefix.ends_with('.') || prefix.ends_with(')')
                    } else {
                        false
                    }
                };
                b.push_list(is_ordered);
                while i < lines.len() && Self::is_list_item(lines[i]) {
                    let item_trimmed = lines[i].trim();
                    let text = if let Some(rest) = item_trimmed
                        .strip_prefix("* ")
                        .or_else(|| item_trimmed.strip_prefix("*\t"))
                        .or_else(|| item_trimmed.strip_prefix("+ "))
                        .or_else(|| item_trimmed.strip_prefix("+\t"))
                        .or_else(|| item_trimmed.strip_prefix("- "))
                        .or_else(|| item_trimmed.strip_prefix("-\t"))
                        .or_else(|| item_trimmed.strip_prefix("#. "))
                        .or_else(|| item_trimmed.strip_prefix("#.\t"))
                    {
                        rest
                    } else if let Some(space_pos) = item_trimmed.find([' ', '\t']) {
                        &item_trimmed[space_pos + 1..]
                    } else {
                        item_trimmed
                    };
                    let mut full_text = text.to_string();
                    i += 1;
                    while i < lines.len()
                        && !lines[i].trim().is_empty()
                        && (lines[i].starts_with("   ") || lines[i].starts_with("\t"))
                        && !Self::is_list_item(lines[i])
                    {
                        full_text.push(' ');
                        full_text.push_str(lines[i].trim());
                        i += 1;
                    }
                    let (parsed_text, item_annotations) = Self::parse_inline_markup(&full_text);
                    b.push_list_item(&parsed_text, is_ordered, item_annotations, None, None);
                }
                b.end_list();
                continue;
            }

            if trimmed.ends_with("::") && !trimmed.starts_with(".. ") {
                if let Some(display_text) = trimmed.strip_suffix("::")
                    && !display_text.is_empty()
                {
                    let (stripped, annotations) = Self::parse_inline_markup(display_text);
                    b.push_paragraph(&stripped, annotations, None, None);
                }
                i += 1;
                while i < lines.len() && lines[i].trim().is_empty() {
                    i += 1;
                }
                let indent = {
                    let mut indent_len = 3usize;
                    let mut j = i;
                    while j < lines.len() {
                        let l = lines[j];
                        if !l.trim().is_empty() {
                            indent_len = l.len() - l.trim_start().len();
                            if indent_len == 0 {
                                indent_len = 3;
                            }
                            break;
                        }
                        j += 1;
                    }
                    indent_len
                };
                let mut code_content = String::new();
                while i < lines.len() {
                    let l = lines[i];
                    let is_indented = l.starts_with("\t")
                        || (l.len() >= indent && l.as_bytes().iter().take(indent).all(|&b| b == b' '));
                    if !is_indented && !l.is_empty() {
                        break;
                    }
                    if !code_content.is_empty() {
                        code_content.push('\n');
                    }
                    if let Some(stripped) = l.strip_prefix('\t') {
                        code_content.push_str(stripped);
                    } else if is_indented && !l.is_empty() {
                        code_content.push_str(&l[indent..]);
                    }
                    i += 1;
                }
                if !code_content.is_empty() {
                    b.push_code(code_content.trim_end(), highlight_lang.as_deref(), None, None);
                }
                continue;
            }

            if !trimmed.is_empty() && !Self::is_markup_line(line) {
                let mut para_text = trimmed.to_string();
                while i + 1 < lines.len() {
                    let next = lines[i + 1];
                    let next_trimmed = next.trim();
                    if next_trimmed.is_empty() {
                        break;
                    }
                    if next.starts_with(' ') || next.starts_with('\t') {
                        break;
                    }
                    if Self::is_section_underline(next_trimmed) {
                        break;
                    }
                    if Self::is_markup_line(next) {
                        break;
                    }
                    if next_trimmed.starts_with(".. ") || next_trimmed == ".." {
                        break;
                    }
                    if Self::is_list_item(next) {
                        break;
                    }
                    if next_trimmed.starts_with(':')
                        && next_trimmed.len() > 1
                        && Self::parse_field_list_line(next_trimmed).is_some()
                    {
                        break;
                    }
                    if Self::is_simple_table_separator(next_trimmed) {
                        break;
                    }
                    if next_trimmed.starts_with('+') && next_trimmed.ends_with('+') && next_trimmed.contains('-') {
                        break;
                    }
                    para_text.push(' ');
                    para_text.push_str(next_trimmed);
                    i += 1;
                }
                let footnote_refs = Self::find_footnote_references(&para_text);
                let (stripped, annotations) = Self::parse_inline_markup(&para_text);
                let idx = b.push_paragraph(&stripped, annotations, None, None);

                for fref in &footnote_refs {
                    let ref_idx = b.push_footnote_ref(&format!("[{}]", fref), fref, None);
                    let _ = ref_idx;
                }

                Self::extract_rst_cross_refs(&para_text, idx, &mut b);
            }

            i += 1;
        }

        b.build()
    }

    /// Extract RST cross-reference roles (`:ref:`, `:doc:`, etc.) and emit relationships.
    fn extract_rst_cross_refs(line: &str, source_idx: u32, b: &mut InternalDocumentBuilder) {
        let roles = [":ref:", ":doc:", ":numref:"];
        for role in &roles {
            let mut search_from = 0;
            while let Some(pos) = line[search_from..].find(role) {
                let abs_pos = search_from + pos;
                let after = &line[abs_pos + role.len()..];
                if after.starts_with('`')
                    && let Some(close) = after[1..].find('`')
                {
                    let target = &after[1..1 + close];
                    let key = if let Some(angle_pos) = target.find('<') {
                        let end = target.find('>').unwrap_or(target.len());
                        &target[angle_pos + 1..end]
                    } else {
                        target
                    };
                    if !key.is_empty() {
                        b.push_relationship(
                            source_idx,
                            RelationshipTarget::Key(key.to_string()),
                            RelationshipKind::CrossReference,
                        );
                    }
                    search_from = abs_pos + role.len() + 1 + close + 1;
                    continue;
                }
                search_from = abs_pos + role.len();
            }
        }
    }

    /// Parse cells from grid table lines (for DocumentStructure).
    fn parse_grid_table_cells(lines: &[&str]) -> Vec<Vec<String>> {
        let mut cells = Vec::new();
        for line in lines {
            let content = line.trim().trim_matches('|');
            if content.is_empty() {
                continue;
            }
            if content
                .chars()
                .all(|c| c == '-' || c == '=' || c == '+' || c == '|' || c == ' ')
            {
                continue;
            }
            let row: Vec<String> = content
                .split('|')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            if !row.is_empty() {
                cells.push(row);
            }
        }
        cells
    }

    /// Check if a line is a simple RST table separator (e.g. `=====  =====  =====`).
    fn is_simple_table_separator(line: &str) -> bool {
        let trimmed = line.trim();
        if trimmed.len() < 3 {
            return false;
        }
        if !trimmed.chars().all(|c| c == '=' || c == ' ') {
            return false;
        }
        trimmed.contains('=')
    }

    /// Parse a simple RST table into cell rows.
    ///
    /// Simple tables use `=====  =====` separator lines. Column boundaries
    /// are determined by the positions of whitespace gaps in the first separator.
    fn parse_simple_table_cells(lines: &[&str]) -> Vec<Vec<String>> {
        if lines.is_empty() {
            return Vec::new();
        }

        let separator = lines[0];
        let col_ranges = Self::simple_table_column_ranges(separator);
        if col_ranges.is_empty() {
            return Vec::new();
        }

        let mut cells = Vec::new();
        for line in lines {
            let trimmed = line.trim();
            if Self::is_simple_table_separator(trimmed) {
                continue;
            }
            let row: Vec<String> = col_ranges
                .iter()
                .map(|&(start, end)| {
                    let end = end.min(line.len());
                    let start = start.min(line.len());
                    if start >= line.len() {
                        String::new()
                    } else {
                        line[start..end].trim().to_string()
                    }
                })
                .collect();
            if row.iter().any(|c| !c.is_empty()) {
                cells.push(row);
            }
        }
        cells
    }

    /// Determine column start/end byte positions from a simple table separator line.
    fn simple_table_column_ranges(separator: &str) -> Vec<(usize, usize)> {
        let mut ranges = Vec::new();
        let bytes = separator.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'=' {
                let start = i;
                while i < bytes.len() && bytes[i] == b'=' {
                    i += 1;
                }
                ranges.push((start, i));
            } else {
                i += 1;
            }
        }
        ranges
    }
}

#[cfg(feature = "office")]
impl Default for RstExtractor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(feature = "office")]
impl Plugin for RstExtractor {
    fn name(&self) -> &str {
        "rst-extractor"
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
        "Native Rust extractor for reStructuredText (RST) documents"
    }

    fn author(&self) -> &str {
        "Xberg Team"
    }
}

#[cfg(feature = "office")]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
impl InternalDocumentExtractor for RstExtractor {
    #[cfg_attr(
        feature = "otel",
        tracing::instrument(
            skip(self, content, config),
            fields(
                extractor.name = self.name(),
                content.size_bytes = content.len(),
            )
        )
    )]
    async fn extract_content(
        &self,
        content: &[u8],
        mime_type: &str,
        config: &ExtractionConfig,
    ) -> Result<InternalDocument> {
        tracing::debug!(format = "rst", size_bytes = content.len(), "extraction starting");
        let mut budget = SecurityBudget::from_config(config);
        budget.account_text(content.len())?;
        let inject_placeholders = config
            .images
            .as_ref()
            .map(|img| img.inject_placeholders)
            .unwrap_or(true);
        let text = String::from_utf8_lossy(content).into_owned();

        let (_extracted_text, metadata) = Self::extract_text_and_metadata(&text);

        // Tables are parsed in place inside `build_internal_document` (via
        // `push_table_from_cells`), which produces correctly-positioned table elements. A second
        // `extract_tables` pass used to raw-push the same tables again, adding an unreferenced
        // (and, for grid tables, less accurate) entry to `doc.tables` for every table without
        // contributing anything to rendered output.
        let mut doc = Self::build_internal_document(&text, inject_placeholders);
        doc.mime_type = mime_type.to_string();
        doc.metadata = metadata;

        tracing::debug!(
            element_count = doc.elements.len(),
            format = "rst",
            "extraction complete"
        );
        Ok(doc)
    }

    async fn extract_path(
        &self,
        path: &std::path::Path,
        mime_type: &str,
        config: &ExtractionConfig,
    ) -> Result<InternalDocument> {
        crate::core::path_resolver::extract_file_with_image_resolution(self, path, mime_type, config).await
    }

    fn supported_mime_types(&self) -> &[&str] {
        &["text/x-rst", "text/prs.fallenstein.rst"]
    }

    fn priority(&self) -> i32 {
        50
    }
}

#[cfg(all(test, feature = "office"))]
mod tests {
    use super::*;

    #[test]
    fn test_rst_extractor_plugin_interface() {
        let extractor = RstExtractor::new();
        assert_eq!(extractor.name(), "rst-extractor");
        assert_eq!(extractor.version(), env!("CARGO_PKG_VERSION"));
        assert_eq!(extractor.priority(), 50);
        assert!(!extractor.supported_mime_types().is_empty());
    }

    #[test]
    fn test_rst_extractor_supports_text_x_rst() {
        let extractor = RstExtractor::new();
        assert!(extractor.supported_mime_types().contains(&"text/x-rst"));
    }

    #[test]
    fn test_rst_extractor_supports_fallenstein_rst() {
        let extractor = RstExtractor::new();
        assert!(extractor.supported_mime_types().contains(&"text/prs.fallenstein.rst"));
    }

    #[test]
    fn test_extract_text_from_rst_simple_document() {
        let content = r#"
Title
=====

This is a paragraph.

Another paragraph.
"#;

        let mut metadata = AHashMap::new();
        let output = RstExtractor::extract_text_from_rst(content, &mut metadata);
        assert!(output.contains("Title"));
        assert!(output.contains("This is a paragraph"));
        assert!(output.contains("Another paragraph"));
    }

    #[test]
    fn test_extract_text_from_rst_math_directive_keeps_math_markup() {
        let content = r#"
Intro paragraph.

.. math::

   E = mc^2

Closing paragraph.
"#;

        let mut metadata = AHashMap::new();
        let output = RstExtractor::extract_text_from_rst(content, &mut metadata);
        assert!(
            output.contains("$$\nE = mc^2\n$$"),
            "math directive must emit display-math delimiters; got: {output}"
        );
        assert!(!output.contains("math: "), "the old prose marker must be gone");
    }

    #[test]
    fn test_extract_text_from_rst_math_inline_argument_and_options() {
        let content = "\nBefore.\n\n.. math:: E = mc^2\n\nAfter.\n\n.. math::\n   :label: eq2\n\n   a + b\n";

        let mut metadata = AHashMap::new();
        let output = RstExtractor::extract_text_from_rst(content, &mut metadata);
        assert!(
            output.contains("$$\nE = mc^2\n$$"),
            "inline-argument form must emit math; got: {output}"
        );
        assert!(
            output.contains("$$\na + b\n$$"),
            "block form must emit math without option lines; got: {output}"
        );
        assert!(
            !output.contains(":label:"),
            "directive options are not math; got: {output}"
        );
    }

    #[test]
    fn test_extract_text_from_rst_math_colon_line_after_options() {
        let content = "\n.. math::\n   :label: eq3\n\n   :\\mathbb{R}: \\to \\mathbb{R}\n";

        let mut metadata = AHashMap::new();
        let output = RstExtractor::extract_text_from_rst(content, &mut metadata);
        assert!(
            output.contains(":\\mathbb{R}: \\to \\mathbb{R}"),
            "math after the option region must survive; got: {output}"
        );
        assert!(!output.contains(":label:"), "options stay out; got: {output}");
    }

    #[test]
    fn test_extract_text_from_rst_with_code_block() {
        let content = r#"
.. code-block:: python

   def hello():
       print("world")

Some text after.
"#;

        let mut metadata = AHashMap::new();
        let output = RstExtractor::extract_text_from_rst(content, &mut metadata);
        assert!(output.contains("code-block"));
        assert!(output.contains("def hello"));
        assert!(output.contains("Some text after"));
    }

    #[test]
    fn test_extract_text_from_rst_with_metadata() {
        let content = r#"
:Author: John Doe
:Date: 2024-01-15

First paragraph.

Second paragraph.
"#;

        let mut metadata = AHashMap::new();
        let output = RstExtractor::extract_text_from_rst(content, &mut metadata);
        assert!(output.contains("First paragraph"));
        assert!(output.contains("Second paragraph"));
        assert!(metadata.contains_key("author"));
        assert_eq!(metadata.get("author").and_then(|v| v.as_str()), Some("John Doe"));
    }

    #[test]
    fn test_rst_extractor_default() {
        let extractor = RstExtractor;
        assert_eq!(extractor.name(), "rst-extractor");
    }

    #[test]
    fn test_rst_extractor_initialize_shutdown() {
        let extractor = RstExtractor::new();
        assert!(extractor.initialize().is_ok());
        assert!(extractor.shutdown().is_ok());
    }

    /// Regression test: the trait-level `extract_content` used to additionally re-push every
    /// table via the raw, element-less `InternalDocument::push_table`, on top of the correctly
    /// created table element from `build_internal_document`. That created a duplicate,
    /// unreferenced (and sometimes incomplete, since the removed `extract_tables` grid-table
    /// parser was less accurate) entry in `doc.tables` for every table without changing
    /// rendered output. Assert there is exactly one table, not two.
    #[tokio::test]
    async fn test_rst_table_is_not_duplicated_in_structured_output() {
        let rst = b"Intro.\n\n===== =====\nName  Age\n===== =====\nAlice 30\n===== =====\n\nOutro.\n";
        let extractor = RstExtractor::new();
        let config = ExtractionConfig::default();

        let doc = extractor
            .extract_content(rst, "text/x-rst", &config)
            .await
            .expect("extraction should succeed");

        assert_eq!(doc.tables.len(), 1, "table should not be duplicated: {:?}", doc.tables);
        let table_element_count = doc
            .elements
            .iter()
            .filter(|e| matches!(e.kind, crate::types::internal::ElementKind::Table { .. }))
            .count();
        assert_eq!(table_element_count, 1);
    }

    #[test]
    fn test_rst_inject_placeholders_true() {
        let rst = "Some text\n\n.. image:: photo.png\n   :alt: A photo\n\nMore text\n";
        let doc = RstExtractor::build_internal_document(rst, true);
        let has_image = doc.elements.iter().any(|e| e.text.contains("[image:"));
        assert!(has_image, "expected image placeholder with inject_placeholders=true");
    }

    #[test]
    fn test_rst_inject_placeholders_false() {
        let rst = "Some text\n\n.. image:: photo.png\n   :alt: A photo\n\nMore text\n";
        let doc = RstExtractor::build_internal_document(rst, false);
        let has_image = doc.elements.iter().any(|e| e.text.contains("[image:"));
        assert!(
            !has_image,
            "expected no image placeholder with inject_placeholders=false"
        );
    }

    fn rst_warnings(doc: &crate::types::internal::InternalDocument) -> Vec<String> {
        doc.processing_warnings
            .iter()
            .filter(|w| w.source == RST_WARNING_SOURCE)
            .map(|w| w.message.to_string())
            .collect()
    }

    /// #171: `.. include:: path` inlines another file's rendered content, which
    /// this single-file, line-based parser has no way to read.
    #[test]
    fn should_warn_when_rst_include_directive_is_skipped() {
        let rst = "Intro text\n\n.. include:: chapter1.rst\n\nMore text\n";
        let doc = RstExtractor::build_internal_document(rst, true);

        let warnings = rst_warnings(&doc);
        assert_eq!(warnings.len(), 1, "expected exactly one rst warning, got {warnings:?}");
        assert!(
            warnings[0].contains("chapter1.rst") && warnings[0].contains("was not read"),
            "warning must name the skipped include target, got {warnings:?}"
        );
    }

    /// A document with no `include` directive must not warn.
    #[test]
    fn should_not_warn_for_rst_document_without_include() {
        let rst = "Intro text\n\n.. note::\n   A note body.\n\nMore text\n";
        let doc = RstExtractor::build_internal_document(rst, true);

        assert!(
            rst_warnings(&doc).is_empty(),
            "a document without .. include:: must not warn, got {:?}",
            rst_warnings(&doc)
        );
    }
}
