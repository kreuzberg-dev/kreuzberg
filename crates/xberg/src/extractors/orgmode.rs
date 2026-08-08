//! Native Org Mode extractor using the `org` library.
//!
//! This extractor provides comprehensive Org Mode document parsing and extraction.
//! It extracts:
//!
//! - **Metadata**: #+TITLE, #+AUTHOR, #+DATE, #+KEYWORDS from document preamble
//! - **Properties**: :PROPERTIES: drawers with additional metadata
//! - **Headings**: Multi-level headings with proper hierarchy (* to *****)
//! - **Content**: Paragraphs and text blocks
//! - **Lists**: Ordered, unordered, and nested lists
//! - **Code blocks**: #+BEGIN_SRC...#+END_SRC with language specification
//! - **Tables**: Pipe tables (| cell | cell |) converted to Table structs
//! - **Inline formatting**: *bold*, /italic/, =code=, ~verbatim~, `[[links]]`
//!
//! Requires the `office` feature.

#[cfg(feature = "office")]
use crate::Result;
#[cfg(feature = "office")]
use crate::core::config::ExtractionConfig;
#[cfg(feature = "office")]
use crate::extractors::security::SecurityBudget;
#[cfg(feature = "office")]
use crate::plugins::{InternalDocumentExtractor, Plugin};
#[cfg(feature = "office")]
use crate::types::document_structure::{AnnotationKind, TextAnnotation};
#[cfg(feature = "office")]
use crate::types::internal::InternalDocument;
#[cfg(feature = "office")]
use crate::types::internal::RelationshipKind;
#[cfg(feature = "office")]
use crate::types::internal::RelationshipTarget;
#[cfg(feature = "office")]
use crate::types::internal_builder::InternalDocumentBuilder;
#[cfg(feature = "office")]
use crate::types::uri::ExtractedUri;
#[cfg(feature = "office")]
use crate::types::{Metadata, Table};
#[cfg(feature = "office")]
use ahash::AHashMap;
#[cfg(feature = "office")]
use async_trait::async_trait;
#[cfg(feature = "office")]
use org::Org;
#[cfg(feature = "office")]
use std::borrow::Cow;

/// Org Mode document extractor.
///
/// Provides native Rust-based Org Mode extraction using the `org` library,
/// extracting structured content and metadata.
/// `ProcessingWarning::source` for every warning this extractor emits (#171).
#[cfg(feature = "office")]
const ORGMODE_WARNING_SOURCE: &str = "orgmode";

#[cfg_attr(alef, alef(skip))]
#[cfg(feature = "office")]
pub struct OrgModeExtractor;

#[cfg(feature = "office")]
impl OrgModeExtractor {
    /// Create a new Org Mode extractor.
    pub(crate) fn new() -> Self {
        Self
    }

    /// Extract metadata and content from Org document in a single pass.
    ///
    /// Combines metadata extraction from directives and full document parsing
    /// into one efficient operation. Looks for:
    /// - #+TITLE: → title
    /// - #+AUTHOR: → author/authors
    /// - #+DATE: → date
    /// - #+KEYWORDS: → keywords
    /// - Other #+DIRECTIVE: entries
    ///
    /// Also extracts document structure and content in parallel.
    fn extract_metadata_and_content(org_text: &str, org: &Org) -> (Metadata, String) {
        let mut metadata = Metadata::default();
        let mut additional: AHashMap<Cow<'static, str>, serde_json::Value> = Default::default();

        for line in org_text.lines().take(100) {
            let trimmed = line.trim();

            if let Some(rest) = trimmed.strip_prefix("#+TITLE:") {
                let value = rest.trim().to_string();
                additional.insert(Cow::Borrowed("title"), serde_json::json!(value));
            } else if let Some(rest) = trimmed.strip_prefix("#+AUTHOR:") {
                let value = rest.trim().to_string();
                additional.insert(Cow::Borrowed("author"), serde_json::json!(&value));
                additional.insert(Cow::Borrowed("authors"), serde_json::json!(vec![value]));
            } else if let Some(rest) = trimmed.strip_prefix("#+DATE:") {
                let value = rest.trim().to_string();
                metadata.created_at = Some(value.clone());
                additional.insert(Cow::Borrowed("date"), serde_json::json!(value));
            } else if let Some(rest) = trimmed.strip_prefix("#+KEYWORDS:") {
                let value = rest.trim();
                let keywords: Vec<&str> = value.split(',').map(|s| s.trim()).collect();
                additional.insert(Cow::Borrowed("keywords"), serde_json::json!(keywords));
            } else if let Some(rest) = trimmed.strip_prefix("#+")
                && let Some((key, val)) = rest.split_once(':')
            {
                let key_lower = key.trim().to_lowercase();
                let value = val.trim();
                if !key_lower.is_empty() && !value.is_empty() {
                    additional.insert(Cow::Owned(format!("directive_{}", key_lower)), serde_json::json!(value));
                }
            }
        }

        metadata.title = additional
            .remove(&Cow::Borrowed("title"))
            .and_then(|v| v.as_str().map(|s| s.to_string()));
        metadata.authors = additional.remove(&Cow::Borrowed("authors")).and_then(|v| {
            v.as_array()
                .map(|arr| arr.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect())
        });
        additional.remove(&Cow::Borrowed("author"));
        metadata.keywords = additional.remove(&Cow::Borrowed("keywords")).and_then(|v| {
            v.as_array()
                .map(|arr| arr.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect())
        });

        metadata.additional = additional;

        let content = Self::extract_content(org);

        (metadata, content)
    }

    /// Extract all content from an Org document using tree-based parsing.
    ///
    /// Uses org's tree-based API to recursively traverse the document structure:
    /// - Headings with proper hierarchy
    /// - Paragraphs
    /// - Lists (both ordered and unordered)
    /// - Code blocks with language info
    /// - Tables as structured data
    /// - Inline formatting markers
    fn extract_content(org: &Org) -> String {
        let mut content = String::new();
        Self::extract_org_tree(org, &mut content);
        content.trim().to_string()
    }

    /// Recursively walk the Org tree and extract content.
    ///
    /// Processes:
    /// - Heading text from `org.heading()`
    /// - Content lines from `org.content_as_ref()`
    /// - Subtrees from `org.subtrees_as_ref()`
    fn extract_org_tree(org: &Org, content: &mut String) {
        let heading = org.heading();
        if !heading.is_empty() {
            let (stripped, _) = Self::parse_inline_markup(heading);
            content.push_str("# ");
            content.push_str(&stripped);
            content.push('\n');
        }

        let lines = org.content_as_ref();
        if !lines.is_empty() {
            let mut paragraph_lines: Vec<&str> = Vec::new();
            for line in lines {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    if !paragraph_lines.is_empty() {
                        let joined = paragraph_lines.join(" ");
                        let (stripped, _) = Self::parse_inline_markup(&joined);
                        content.push_str(&stripped);
                        content.push('\n');
                        paragraph_lines.clear();
                    }
                } else {
                    paragraph_lines.push(trimmed);
                }
            }
            if !paragraph_lines.is_empty() {
                let joined = paragraph_lines.join(" ");
                let (stripped, _) = Self::parse_inline_markup(&joined);
                content.push_str(&stripped);
                content.push('\n');
            }
            content.push('\n');
        }

        let subtrees = org.subtrees_as_ref();
        for subtree in subtrees {
            Self::extract_org_tree(subtree, content);
        }
    }

    /// Strip OrgMode inline markup from text and produce annotations with byte offsets.
    ///
    /// Handles: `*bold*`, `/italic/`, `_underline_`, `=verbatim=`, `~code~`,
    /// `+strikethrough+`, and `[[url][desc]]` links.
    fn parse_inline_markup(raw: &str) -> (String, Vec<TextAnnotation>) {
        let mut out = String::with_capacity(raw.len());
        let mut annotations = Vec::new();
        let bytes = raw.as_bytes();
        let len = bytes.len();
        let mut i = 0;

        while i < len {
            if i + 1 < len
                && bytes[i] == b'['
                && bytes[i + 1] == b'['
                && let Some((url, display, consumed_to)) = Self::parse_org_link(raw, i)
            {
                let start = out.len() as u32;
                out.push_str(&display);
                let end = out.len() as u32;
                if start < end {
                    annotations.push(TextAnnotation {
                        start,
                        end,
                        kind: AnnotationKind::Link { url, title: None },
                    });
                }
                i = consumed_to;
                continue;
            }

            if bytes[i].is_ascii() && Self::is_org_markup_char(bytes[i]) {
                let marker = bytes[i];
                let preceded_ok =
                    i == 0 || bytes[i - 1].is_ascii_whitespace() || bytes[i - 1] == b'(' || bytes[i - 1] == b'"';
                if preceded_ok
                    && i + 1 < len
                    && !bytes[i + 1].is_ascii_whitespace()
                    && let Some(close) = Self::find_org_markup_close(bytes, i + 1, marker)
                {
                    let inner = &raw[i + 1..close];
                    let start = out.len() as u32;
                    out.push_str(inner);
                    let end_off = out.len() as u32;
                    let kind = match marker {
                        b'*' => AnnotationKind::Bold,
                        b'/' => AnnotationKind::Italic,
                        b'_' => AnnotationKind::Underline,
                        b'=' | b'~' => AnnotationKind::Code,
                        b'+' => AnnotationKind::Strikethrough,
                        _ => unreachable!("byte not in is_org_markup_char set"),
                    };
                    if start < end_off {
                        annotations.push(TextAnnotation {
                            start,
                            end: end_off,
                            kind,
                        });
                    }
                    i = close + 1;
                    continue;
                }
            }

            let ch = &raw[i..];
            let c = ch.chars().next().unwrap();
            out.push(c);
            i += c.len_utf8();
        }

        (out, annotations)
    }

    fn is_org_markup_char(b: u8) -> bool {
        matches!(b, b'*' | b'/' | b'_' | b'=' | b'~' | b'+')
    }

    /// Find the closing position of an Org markup character.
    /// The closing marker must not be preceded by whitespace.
    fn find_org_markup_close(bytes: &[u8], from: usize, marker: u8) -> Option<usize> {
        let mut j = from;
        while j < bytes.len() {
            if bytes[j] == marker
                && j > from
                && !bytes[j - 1].is_ascii_whitespace()
                && (j + 1 >= bytes.len()
                    || bytes[j + 1].is_ascii_whitespace()
                    || bytes[j + 1] == b'.'
                    || bytes[j + 1] == b','
                    || bytes[j + 1] == b';'
                    || bytes[j + 1] == b':'
                    || bytes[j + 1] == b')'
                    || bytes[j + 1] == b']'
                    || bytes[j + 1] == b'"')
            {
                return Some(j);
            }
            j += 1;
        }
        None
    }

    /// Parse `[[url][desc]]` or `[[url]]` starting at position `start` (the first `[`).
    /// Returns `(url, display_text, end_position)`.
    fn parse_org_link(text: &str, start: usize) -> Option<(String, String, usize)> {
        if !text[start..].starts_with("[[") {
            return None;
        }
        let after_open = start + 2;
        let rest = &text[after_open..];
        if let Some(desc_start) = rest.find("][") {
            let url = &rest[..desc_start];
            let desc_begin = after_open + desc_start + 2;
            if let Some(close) = text[desc_begin..].find("]]") {
                let description = &text[desc_begin..desc_begin + close];
                return Some((url.to_string(), description.to_string(), desc_begin + close + 2));
            }
        } else if let Some(close) = rest.find("]]") {
            let url = &rest[..close];
            return Some((url.to_string(), url.to_string(), after_open + close + 2));
        }
        None
    }

    /// Parse `[fn:name]` footnote references from text, returning label names.
    fn find_footnote_references(line: &str) -> Vec<String> {
        let mut refs = Vec::new();
        let mut search_from = 0;
        while let Some(pos) = line[search_from..].find("[fn:") {
            let abs_pos = search_from + pos;
            if let Some(close) = line[abs_pos..].find(']') {
                let label = &line[abs_pos + 4..abs_pos + close];
                if !label.is_empty() {
                    refs.push(label.to_string());
                }
                search_from = abs_pos + close + 1;
            } else {
                break;
            }
        }
        refs
    }

    /// Build an `InternalDocument` from Org Mode source text.
    ///
    /// Handles headings, paragraphs, lists, code blocks, tables, inline links,
    /// and footnote references.
    pub(crate) fn build_internal_document(org_text: &str) -> InternalDocument {
        let mut b = InternalDocumentBuilder::new("orgmode");
        let lines: Vec<&str> = org_text.lines().collect();
        let mut i = 0;

        let mut metadata_entries: Vec<(String, String)> = Vec::new();
        while i < lines.len() {
            let trimmed = lines[i].trim();
            if let Some(rest) = trimmed.strip_prefix("#+") {
                let rest_upper = rest.to_ascii_uppercase();
                if rest_upper.starts_with("BEGIN") || rest_upper.starts_with("END") {
                    break;
                }
                if let Some((key, val)) = rest.split_once(':') {
                    let key_upper = key.trim().to_uppercase();
                    // `#+CAPTION:`/`#+NAME:` are body-level affiliated keywords that attach to
                    // the immediately-following element (image link, table, ...), not
                    // document-level preamble metadata — hand them to the body loop below
                    // instead of swallowing them into `metadata_entries`.
                    if key_upper == "CAPTION" || key_upper == "NAME" {
                        break;
                    }
                    let value = val.trim().to_string();
                    if !value.is_empty() {
                        if key_upper == "INCLUDE" {
                            // Recorded as preamble metadata below, but the referenced file's
                            // content itself is never read by this single-file parser (#171).
                            b.add_warning(crate::core::diagnostics::warning(
                                ORGMODE_WARNING_SOURCE,
                                format!(
                                    "'#+INCLUDE: {value}' references an external file that was not read; \
                                     its content is missing from the extracted text"
                                ),
                            ));
                        }
                        metadata_entries.push((key_upper, value));
                    }
                }
                i += 1;
                continue;
            }
            if !trimmed.is_empty() {
                break;
            }
            i += 1;
        }
        if !metadata_entries.is_empty() {
            b.push_metadata_block(&metadata_entries, None);
        }

        let mut pending_caption: Option<String> = None;
        let mut pending_name: Option<String> = None;

        while i < lines.len() {
            let trimmed = lines[i].trim();

            if !trimmed.is_empty()
                && (pending_caption.is_some() || pending_name.is_some())
                && !Self::is_caption_keyword_line(trimmed)
                && !Self::is_caption_target_line(trimmed)
            {
                // The buffered `#+CAPTION:`/`#+NAME:` line was not immediately followed by an
                // element that can carry it (image link or table); drop it rather than
                // mis-attaching it to an unrelated, later element.
                pending_caption = None;
                pending_name = None;
            }

            if trimmed.starts_with("#+")
                && !trimmed.starts_with("#+BEGIN")
                && !trimmed.starts_with("#+begin")
                && !trimmed.starts_with("#+END")
                && !trimmed.starts_with("#+end")
            {
                if let Some((key, val)) = trimmed[2..].split_once(':') {
                    let key_upper = key.trim().to_ascii_uppercase();
                    let value = val.trim();
                    if key_upper == "CAPTION" && !value.is_empty() {
                        pending_caption = Some(match pending_caption.take() {
                            Some(existing) => format!("{existing} {value}"),
                            None => value.to_string(),
                        });
                    } else if key_upper == "NAME" && !value.is_empty() {
                        pending_name = Some(value.to_string());
                    } else if key_upper == "INCLUDE" && !value.is_empty() {
                        // `#+INCLUDE: "file.org"` inlines another file's rendered content at
                        // this point. This parser works on a single in-memory document and
                        // never resolves the reference, so the referenced file's content is
                        // always missing from the extracted text (#171).
                        b.add_warning(crate::core::diagnostics::warning(
                            ORGMODE_WARNING_SOURCE,
                            format!(
                                "'#+INCLUDE: {value}' references an external file that was not read; \
                                 its content is missing from the extracted text"
                            ),
                        ));
                    }
                }
                i += 1;
                continue;
            }

            if trimmed == ":PROPERTIES:" {
                let mut props: Vec<(String, String)> = Vec::new();
                i += 1;
                while i < lines.len() {
                    let pt = lines[i].trim();
                    if pt == ":END:" {
                        i += 1;
                        break;
                    }
                    if pt.starts_with(':')
                        && pt.len() > 1
                        && let Some(colon2) = pt[1..].find(':')
                    {
                        let key = pt[1..1 + colon2].to_string();
                        let value = pt[2 + colon2..].trim().to_string();
                        if !key.is_empty() {
                            props.push((key, value));
                        }
                    }
                    i += 1;
                }
                if !props.is_empty() {
                    b.push_metadata_block(&props, None);
                }
                continue;
            }

            if trimmed.starts_with('*') {
                let mut level: u8 = 0;
                for ch in trimmed.chars() {
                    if ch == '*' {
                        level += 1;
                    } else {
                        break;
                    }
                }
                if level > 0 && trimmed.len() > level as usize && trimmed.as_bytes()[level as usize] == b' ' {
                    let raw_heading = trimmed[level as usize + 1..].trim();
                    if !raw_heading.is_empty() {
                        let todo_keywords = ["TODO", "DONE", "NEXT", "WAITING", "CANCELLED", "CANCELED"];
                        let mut heading_text = raw_heading;
                        for kw in &todo_keywords {
                            if heading_text.starts_with(kw) {
                                let after = &heading_text[kw.len()..];
                                if after.is_empty() || after.starts_with(' ') {
                                    heading_text = after.trim_start();
                                    break;
                                }
                            }
                        }
                        if let Some(tag_start) = heading_text.rfind(" :") {
                            let potential_tags = &heading_text[tag_start + 1..];
                            if potential_tags.ends_with(':') && potential_tags.len() > 2 {
                                heading_text = heading_text[..tag_start].trim_end();
                            }
                        }
                        b.push_heading(level, heading_text, None, None);
                    }
                    i += 1;
                    continue;
                }
            }

            if trimmed.starts_with("#+BEGIN_SRC") || trimmed.starts_with("#+begin_src") {
                let language: Option<&str> = trimmed.split_whitespace().nth(1);
                i += 1;
                let mut code_content = String::new();
                while i < lines.len() {
                    let t = lines[i].trim();
                    if t.starts_with("#+END_SRC") || t.starts_with("#+end_src") {
                        i += 1;
                        break;
                    }
                    if !code_content.is_empty() {
                        code_content.push('\n');
                    }
                    code_content.push_str(lines[i]);
                    i += 1;
                }
                b.push_code(code_content.trim_end(), language, None, None);
                continue;
            }

            if trimmed.starts_with("#+BEGIN_QUOTE") || trimmed.starts_with("#+begin_quote") {
                b.push_quote_start();
                i += 1;
                while i < lines.len() {
                    let t = lines[i].trim();
                    if t.starts_with("#+END_QUOTE") || t.starts_with("#+end_quote") {
                        i += 1;
                        break;
                    }
                    if !t.is_empty() {
                        b.push_paragraph(t, vec![], None, None);
                    }
                    i += 1;
                }
                b.push_quote_end();
                continue;
            }

            if trimmed.starts_with("#+BEGIN_EXAMPLE") || trimmed.starts_with("#+begin_example") {
                i += 1;
                let mut block_content = String::new();
                while i < lines.len() {
                    let t = lines[i].trim();
                    if t.starts_with("#+END_EXAMPLE") || t.starts_with("#+end_example") {
                        i += 1;
                        break;
                    }
                    if !block_content.is_empty() {
                        block_content.push('\n');
                    }
                    block_content.push_str(lines[i]);
                    i += 1;
                }
                b.push_code(block_content.trim_end(), None, None, None);
                continue;
            }

            if trimmed.starts_with("#+BEGIN_") || trimmed.starts_with("#+begin_") {
                let block_type = trimmed
                    .split_whitespace()
                    .next()
                    .unwrap_or("")
                    .strip_prefix("#+BEGIN_")
                    .or_else(|| trimmed.split_whitespace().next().unwrap_or("").strip_prefix("#+begin_"))
                    .unwrap_or("UNKNOWN")
                    .to_string();
                let end_marker_upper = format!("#+END_{}", block_type);
                let end_marker_lower = end_marker_upper.to_lowercase();
                i += 1;
                let mut block_content = String::new();
                while i < lines.len() {
                    let t = lines[i].trim();
                    if t.starts_with(&end_marker_upper) || t.starts_with(&end_marker_lower) {
                        i += 1;
                        break;
                    }
                    if !block_content.is_empty() {
                        block_content.push('\n');
                    }
                    block_content.push_str(lines[i]);
                    i += 1;
                }
                b.push_raw_block("orgmode", block_content.trim_end(), None);
                continue;
            }

            if trimmed.starts_with('|') && trimmed.ends_with('|') {
                let mut table_cells: Vec<Vec<String>> = Vec::new();
                let mut has_header_separator = false;
                while i < lines.len() {
                    let t = lines[i].trim();
                    if !t.starts_with('|') || !t.ends_with('|') {
                        break;
                    }
                    if Self::is_org_table_horizontal_line(t) {
                        has_header_separator |= !table_cells.is_empty();
                        i += 1;
                        continue;
                    }
                    let cells: Vec<String> = t
                        .split('|')
                        .map(|cell| cell.trim().to_string())
                        .filter(|cell| !cell.is_empty())
                        .collect();
                    if !cells.is_empty() {
                        table_cells.push(cells);
                    }
                    i += 1;
                }
                if !table_cells.is_empty() {
                    let element_idx = Self::push_org_table(&mut b, table_cells, has_header_separator);
                    Self::attach_pending_caption_and_name(&mut b, element_idx, &mut pending_caption, &mut pending_name);
                }
                continue;
            }

            if !trimmed.is_empty() && Self::is_org_list_item(trimmed) {
                let is_ordered = Self::is_org_ordered_item(trimmed);
                b.push_list(is_ordered);
                while i < lines.len() {
                    let t = lines[i].trim();
                    if t.is_empty() {
                        break;
                    }
                    if Self::is_org_list_item(t) {
                        let item_text = Self::strip_list_prefix(t);
                        let mut item_parts: Vec<&str> = vec![item_text];
                        i += 1;
                        while i < lines.len() {
                            let raw_next = lines[i];
                            let next_t = raw_next.trim();
                            if next_t.is_empty() || Self::is_org_list_item(next_t) || Self::is_structural_start(next_t)
                            {
                                break;
                            }
                            if raw_next.starts_with(' ') || raw_next.starts_with('\t') {
                                item_parts.push(next_t);
                                i += 1;
                            } else {
                                break;
                            }
                        }
                        let joined_item = item_parts.join(" ");
                        b.push_list_item(&joined_item, is_ordered, vec![], None, None);
                    } else {
                        break;
                    }
                }
                b.end_list();
                continue;
            }

            if trimmed.starts_with("[fn:") {
                if let Some(close) = trimmed.find(']') {
                    let name = &trimmed[4..close];
                    if !name.is_empty() {
                        let def_text = trimmed[close + 1..].trim();
                        if !def_text.is_empty() {
                            b.push_footnote_definition(def_text, name, None);
                        }
                    }
                }
                i += 1;
                continue;
            }

            if !trimmed.is_empty() {
                if let Some((url, display, consumed_to)) = Self::parse_org_link(trimmed, 0)
                    && consumed_to == trimmed.len()
                    && Self::is_image_url(&url)
                {
                    use crate::types::document_structure::ContentLayer;
                    use crate::types::internal::{ElementKind, InternalElement, InternalElementId};
                    let alt = if display == url { String::new() } else { display.clone() };
                    let kind = ElementKind::Image { image_index: u32::MAX };
                    let id = InternalElementId::generate(kind.discriminant(), &alt, None, 0);
                    let element_idx = b.push_element(InternalElement {
                        id,
                        kind,
                        text: alt,
                        depth: 0,
                        page: None,
                        bbox: None,
                        layer: ContentLayer::Body,
                        annotations: Vec::new(),
                        attributes: None,
                        anchor: None,
                        ocr_geometry: None,
                        ocr_confidence: None,
                        ocr_rotation: None,
                    });
                    Self::attach_pending_caption_and_name(&mut b, element_idx, &mut pending_caption, &mut pending_name);
                    let label = if display == url { None } else { Some(display) };
                    b.push_uri(ExtractedUri::image(&url, label));
                    i += 1;
                    continue;
                }

                let mut para_raw_lines: Vec<&str> = vec![trimmed];
                let mut next = i + 1;
                while next < lines.len() {
                    let next_trimmed = lines[next].trim();
                    if next_trimmed.is_empty() || Self::is_structural_start(next_trimmed) {
                        break;
                    }
                    if let Some((url, _, consumed_to)) = Self::parse_org_link(next_trimmed, 0)
                        && consumed_to == next_trimmed.len()
                        && Self::is_image_url(&url)
                    {
                        break;
                    }
                    para_raw_lines.push(next_trimmed);
                    next += 1;
                }

                let joined_raw = para_raw_lines.join(" ");

                let footnote_refs = Self::find_footnote_references(&joined_raw);
                let (stripped, annotations) = Self::parse_inline_markup(&joined_raw);

                for ann in &annotations {
                    if let AnnotationKind::Link { url, .. } = &ann.kind
                        && !url.is_empty()
                    {
                        let label = stripped
                            .get(ann.start as usize..ann.end as usize)
                            .map(|s| s.to_string());
                        let is_image = url.ends_with(".png")
                            || url.ends_with(".jpg")
                            || url.ends_with(".jpeg")
                            || url.ends_with(".gif")
                            || url.ends_with(".svg")
                            || (url.starts_with("file:")
                                && label.as_deref().is_some_and(|l| {
                                    l.ends_with(".png") || l.ends_with(".jpg") || l.ends_with(".jpeg")
                                }));
                        if is_image {
                            b.push_uri(ExtractedUri::image(url, label));
                        } else {
                            b.push_uri(ExtractedUri::hyperlink(url, label));
                        }
                    }
                }

                let idx = b.push_paragraph(&stripped, annotations, None, None);

                for fref in &footnote_refs {
                    let ref_idx = b.push_footnote_ref(&format!("[fn:{}]", fref), fref, None);
                    let _ = ref_idx;
                }

                Self::extract_internal_links(&joined_raw, idx, &mut b);

                i = next;
                continue;
            }

            i += 1;
        }

        b.build()
    }

    /// Push an Org table while preserving whether the source declared a header separator.
    ///
    /// Returns the index of the created `Table` element so callers can attach a preceding
    /// `#+CAPTION:`/`#+NAME:` affiliated keyword to it (see `attach_pending_caption_and_name`).
    fn push_org_table(b: &mut InternalDocumentBuilder, cells: Vec<Vec<String>>, has_header: bool) -> u32 {
        let columns = has_header.then(|| cells[0].clone());
        let mut markdown_cells = cells.clone();
        if !has_header {
            let column_count = cells.iter().map(Vec::len).max().unwrap_or(0);
            markdown_cells.insert(0, vec![String::new(); column_count]);
        }
        let table = Table {
            cells,
            markdown: Self::cells_to_markdown(&markdown_cells),
            columns,
            ..Default::default()
        };
        b.push_table(table, None, None)
    }

    /// Check whether a trimmed line is a `#+CAPTION:` or `#+NAME:` affiliated keyword line.
    /// Used to decide whether a still-pending caption/name should keep being carried forward
    /// to a subsequent line (multi-line captions, or a `#+NAME:` line following `#+CAPTION:`).
    fn is_caption_keyword_line(trimmed: &str) -> bool {
        let Some(rest) = trimmed.strip_prefix("#+") else {
            return false;
        };
        let Some((key, _)) = rest.split_once(':') else {
            return false;
        };
        matches!(key.trim().to_ascii_uppercase().as_str(), "CAPTION" | "NAME")
    }

    /// Check whether a trimmed line starts an element that Org "affiliated keywords"
    /// (`#+CAPTION:`, `#+NAME:`) can attach to: an image link or a table row.
    fn is_caption_target_line(trimmed: &str) -> bool {
        if trimmed.starts_with('|') && trimmed.ends_with('|') {
            return true;
        }
        if let Some((_, _, consumed_to)) = Self::parse_org_link(trimmed, 0)
            && consumed_to == trimmed.len()
        {
            return true;
        }
        false
    }

    /// Attach any buffered `#+CAPTION:` / `#+NAME:` affiliated keywords to the element at
    /// `element_idx`, consuming (`take`-ing) them so they are not applied a second time.
    fn attach_pending_caption_and_name(
        b: &mut InternalDocumentBuilder,
        element_idx: u32,
        pending_caption: &mut Option<String>,
        pending_name: &mut Option<String>,
    ) {
        if let Some(caption) = pending_caption.take() {
            let mut attributes: AHashMap<String, String> = AHashMap::default();
            attributes.insert("caption".to_string(), caption);
            b.set_attributes(element_idx, attributes);
        }
        if let Some(name) = pending_name.take() {
            b.set_anchor(element_idx, name);
        }
    }

    fn is_org_table_horizontal_line(line: &str) -> bool {
        let Some(inner) = line.trim().strip_prefix('|').and_then(|line| line.strip_suffix('|')) else {
            return false;
        };

        inner
            .split('+')
            .all(|segment| !segment.is_empty() && segment.bytes().all(|byte| byte == b'-'))
    }

    /// Extract internal org links from a line and add relationships.
    fn extract_internal_links(line: &str, source_idx: u32, b: &mut InternalDocumentBuilder) {
        let mut search_from = 0;
        while let Some(pos) = line[search_from..].find("[[") {
            let abs_pos = search_from + pos;
            let after = &line[abs_pos + 2..];
            let close = if let Some(desc_start) = after.find("][") {
                after[desc_start + 2..]
                    .find("]]")
                    .map(|close| desc_start + 2 + close + 2)
            } else {
                after.find("]]").map(|p| p + 2)
            };

            if let Some(consumed) = close {
                let link_content = &after[..consumed - 2];
                let url_part = if let Some(sep) = link_content.find("][") {
                    &link_content[..sep]
                } else {
                    link_content
                };

                if let Some(anchor) = url_part.strip_prefix('#') {
                    b.push_relationship(
                        source_idx,
                        RelationshipTarget::Key(anchor.to_string()),
                        RelationshipKind::InternalLink,
                    );
                } else if let Some(heading) = url_part.strip_prefix('*') {
                    b.push_relationship(
                        source_idx,
                        RelationshipTarget::Key(heading.to_string()),
                        RelationshipKind::InternalLink,
                    );
                }

                search_from = abs_pos + 2 + consumed;
            } else {
                break;
            }
        }
    }

    /// Check if a trimmed line starts a new structural element (heading, block, table, list, etc.).
    /// Used to determine paragraph continuation boundaries.
    fn is_structural_start(trimmed: &str) -> bool {
        if trimmed.starts_with('*') {
            let mut level: u8 = 0;
            for ch in trimmed.chars() {
                if ch == '*' {
                    level += 1;
                } else {
                    break;
                }
            }
            if level > 0 && trimmed.len() > level as usize && trimmed.as_bytes()[level as usize] == b' ' {
                return true;
            }
        }
        if trimmed.starts_with("#+BEGIN")
            || trimmed.starts_with("#+begin")
            || trimmed.starts_with("#+END")
            || trimmed.starts_with("#+end")
        {
            return true;
        }
        if trimmed.starts_with("#+") {
            return true;
        }
        if trimmed == ":PROPERTIES:" || trimmed == ":END:" {
            return true;
        }
        if trimmed.starts_with('|') && trimmed.ends_with('|') {
            return true;
        }
        if Self::is_org_list_item(trimmed) {
            return true;
        }
        if trimmed.starts_with("[fn:") {
            return true;
        }
        false
    }

    /// Check if a line is an Org list item.
    fn is_org_list_item(line: &str) -> bool {
        let t = line.trim_start();
        if t.starts_with("- ") || t.starts_with("+ ") {
            return true;
        }
        if let Some(space_pos) = t.find(' ')
            && space_pos > 0
            && space_pos < 5
        {
            let prefix = &t[..space_pos];
            if (prefix.ends_with('.') || prefix.ends_with(')'))
                && prefix[..prefix.len() - 1].chars().all(|c| c.is_numeric())
            {
                return true;
            }
        }
        false
    }

    /// Check if a list item is ordered.
    fn is_org_ordered_item(line: &str) -> bool {
        let t = line.trim_start();
        if let Some(space_pos) = t.find(' ')
            && space_pos > 0
            && space_pos < 5
        {
            let prefix = &t[..space_pos];
            return (prefix.ends_with('.') || prefix.ends_with(')'))
                && prefix[..prefix.len() - 1].chars().all(|c| c.is_numeric());
        }
        false
    }

    /// Strip list prefix (-, +, 1., 1)) from a list item line.
    fn strip_list_prefix(line: &str) -> &str {
        let t = line.trim_start();
        if let Some(rest) = t.strip_prefix("- ").or_else(|| t.strip_prefix("+ ")) {
            return rest;
        }
        if let Some(space_pos) = t.find(' ') {
            return &t[space_pos + 1..];
        }
        t
    }

    /// Check if a URL points to an image based on its file extension.
    fn is_image_url(url: &str) -> bool {
        let path = url
            .strip_prefix("file:")
            .unwrap_or(url)
            .split(['?', '#'])
            .next()
            .unwrap_or(url);
        let lower = path.to_ascii_lowercase();
        lower.ends_with(".png")
            || lower.ends_with(".jpg")
            || lower.ends_with(".jpeg")
            || lower.ends_with(".gif")
            || lower.ends_with(".svg")
            || lower.ends_with(".webp")
            || lower.ends_with(".bmp")
            || lower.ends_with(".tiff")
            || lower.ends_with(".tif")
            || lower.ends_with(".avif")
    }

    /// Convert table cells to markdown format.
    ///
    /// Delegates to the crate's single table renderer
    /// ([`crate::rendering::common::render_table_markdown`]) so an Org table
    /// serialises identically to the same table from any other source
    /// (xberg-io/xberg#220). Org cells may legitimately contain `|` (escaped as
    /// `\vert` in the source, but literal after unescaping), which this copy
    /// emitted raw and thereby split into extra columns (xberg-io/xberg#163).
    fn cells_to_markdown(cells: &[Vec<String>]) -> String {
        crate::rendering::common::render_table_markdown(cells)
    }
}

#[cfg(feature = "office")]
impl Default for OrgModeExtractor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(feature = "office")]
impl Plugin for OrgModeExtractor {
    fn name(&self) -> &str {
        "orgmode-extractor"
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
        "Native Rust extractor for Org Mode documents with comprehensive metadata extraction"
    }

    fn author(&self) -> &str {
        "Xberg Team"
    }
}

#[cfg(feature = "office")]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
impl InternalDocumentExtractor for OrgModeExtractor {
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
        tracing::debug!(format = "orgmode", size_bytes = content.len(), "extraction starting");
        let mut budget = SecurityBudget::from_config(config);
        budget.account_text(content.len())?;
        let org_text = String::from_utf8_lossy(content).into_owned();

        let lines: Vec<String> = org_text.lines().map(|s| s.to_string()).collect();
        let org = Org::from_vec(&lines)?;

        let (metadata, _extracted_content) = Self::extract_metadata_and_content(&org_text, &org);

        // Tables are parsed in place inside `build_internal_document` (see `push_org_table`),
        // which produces correctly-positioned table elements. A second `extract_tables` tree walk
        // used to raw-push the same tables again, doubling `counts.tables` in structured output
        // without contributing anything to rendered output.
        let mut doc = Self::build_internal_document(&org_text);
        doc.mime_type = mime_type.to_string();
        doc.metadata = metadata;

        tracing::debug!(
            element_count = doc.elements.len(),
            format = "orgmode",
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
        &["text/x-org", "text/org", "application/x-org"]
    }

    fn priority(&self) -> i32 {
        50
    }
}

#[cfg(all(test, feature = "office"))]
mod tests {
    use super::*;

    #[test]
    fn test_orgmode_extractor_plugin_interface() {
        let extractor = OrgModeExtractor::new();
        assert_eq!(extractor.name(), "orgmode-extractor");
        assert_eq!(extractor.version(), env!("CARGO_PKG_VERSION"));
        assert_eq!(extractor.priority(), 50);
        assert!(!extractor.supported_mime_types().is_empty());
    }

    #[test]
    fn test_orgmode_extractor_supports_text_x_org() {
        let extractor = OrgModeExtractor::new();
        assert!(extractor.supported_mime_types().contains(&"text/x-org"));
    }

    #[test]
    fn test_orgmode_extractor_default() {
        let extractor = OrgModeExtractor;
        assert_eq!(extractor.name(), "orgmode-extractor");
    }

    #[test]
    fn test_orgmode_extractor_initialize_shutdown() {
        let extractor = OrgModeExtractor::new();
        assert!(extractor.initialize().is_ok());
        assert!(extractor.shutdown().is_ok());
    }

    #[test]
    fn test_extract_metadata_with_title() {
        let org_text = "#+TITLE: Test Document\n\nContent here.";
        let lines: Vec<String> = org_text.lines().map(|s| s.to_string()).collect();
        let org = Org::from_vec(&lines).expect("Failed to parse org");
        let (metadata, _) = OrgModeExtractor::extract_metadata_and_content(org_text, &org);

        assert!(metadata.title.is_some());
    }

    #[test]
    fn test_extract_metadata_with_author() {
        let org_text = "#+AUTHOR: John Doe\n\nContent here.";
        let lines: Vec<String> = org_text.lines().map(|s| s.to_string()).collect();
        let org = Org::from_vec(&lines).expect("Failed to parse org");
        let (metadata, _) = OrgModeExtractor::extract_metadata_and_content(org_text, &org);

        assert!(metadata.authors.is_some());
    }

    #[test]
    fn test_extract_metadata_with_date() {
        let org_text = "#+DATE: 2024-01-15\n\nContent here.";
        let lines: Vec<String> = org_text.lines().map(|s| s.to_string()).collect();
        let org = Org::from_vec(&lines).expect("Failed to parse org");
        let (metadata, _) = OrgModeExtractor::extract_metadata_and_content(org_text, &org);

        assert_eq!(metadata.created_at, Some("2024-01-15".to_string()));
    }

    #[test]
    fn test_extract_metadata_with_keywords() {
        let org_text = "#+KEYWORDS: rust, org-mode, parsing\n\nContent here.";
        let lines: Vec<String> = org_text.lines().map(|s| s.to_string()).collect();
        let org = Org::from_vec(&lines).expect("Failed to parse org");
        let (metadata, _) = OrgModeExtractor::extract_metadata_and_content(org_text, &org);

        assert!(metadata.keywords.is_some());
    }

    #[test]
    fn test_extract_content_with_headings() {
        let org_text = "* Heading 1\n\nSome content.\n\n** Heading 2\n\nMore content.";
        let lines: Vec<String> = org_text.lines().map(|s| s.to_string()).collect();
        let org = Org::from_vec(&lines).expect("Failed to parse org");
        let content = OrgModeExtractor::extract_content(&org);

        assert!(content.contains("Heading 1"));
        assert!(content.contains("Heading 2"));
        assert!(content.contains("Some content"));
        assert!(content.contains("More content"));
    }

    #[test]
    fn test_extract_content_with_paragraphs() {
        let org_text = "First paragraph.\n\nSecond paragraph.";
        let lines: Vec<String> = org_text.lines().map(|s| s.to_string()).collect();
        let org = Org::from_vec(&lines).expect("Failed to parse org");
        let content = OrgModeExtractor::extract_content(&org);

        assert!(content.contains("First paragraph"));
        assert!(content.contains("Second paragraph"));
    }

    /// Regression test: the trait-level `extract_content` used to additionally re-push every
    /// table via the raw, element-less `InternalDocument::push_table`, on top of the correctly
    /// created table element from `build_internal_document`'s `push_org_table`. That created a
    /// duplicate, unreferenced entry in `doc.tables` for every table without changing rendered
    /// output. Assert there is exactly one table, not two.
    #[tokio::test]
    async fn test_orgmode_table_is_not_duplicated_in_structured_output() {
        let org_text = b"| Name | Age |\n| Alice | 30 |\n";
        let extractor = OrgModeExtractor::new();
        let config = ExtractionConfig::default();

        let doc = extractor
            .extract_content(org_text, "text/x-org", &config)
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
    fn test_extract_content_with_lists() {
        let org_text = "- Item 1\n- Item 2\n- Item 3";
        let lines: Vec<String> = org_text.lines().map(|s| s.to_string()).collect();
        let org = Org::from_vec(&lines).expect("Failed to parse org");
        let content = OrgModeExtractor::extract_content(&org);

        assert!(content.contains("Item 1"));
        assert!(content.contains("Item 2"));
        assert!(content.contains("Item 3"));
    }

    #[test]
    fn test_cells_to_markdown_format() {
        let cells = vec![
            vec!["Name".to_string(), "Age".to_string()],
            vec!["Alice".to_string(), "30".to_string()],
            vec!["Bob".to_string(), "25".to_string()],
        ];

        let markdown = OrgModeExtractor::cells_to_markdown(&cells);
        assert!(markdown.contains("Name"));
        assert!(markdown.contains("Age"));
        assert!(markdown.contains("Alice"));
        assert!(markdown.contains("Bob"));
        assert!(markdown.contains("---"));
    }

    #[test]
    fn should_preserve_separator_header_in_structured_and_rendered_org_table() {
        let document = OrgModeExtractor::build_internal_document("| Name | Age |\n|------+-----|\n| Alice | 30 |");
        assert_eq!(document.tables.len(), 1);
        let table = &document.tables[0];

        assert_eq!(
            table.cells,
            vec![
                vec!["Name".to_string(), "Age".to_string()],
                vec!["Alice".to_string(), "30".to_string()],
            ]
        );
        assert_eq!(table.columns, Some(vec!["Name".to_string(), "Age".to_string()]));
        assert_eq!(table.markdown, "| Name | Age |\n| --- | --- |\n| Alice | 30 |\n");
        assert_eq!(
            crate::rendering::render_markdown(&document),
            "| Name | Age |\n| --- | --- |\n| Alice | 30 |\n"
        );
    }

    #[test]
    fn should_preserve_data_containing_horizontal_line_substrings() {
        let document = OrgModeExtractor::build_internal_document(
            "| Version | Status |\n|---------+--------|\n| release---candidate | stable |",
        );
        let table = &document.tables[0];

        assert_eq!(
            table.cells,
            vec![
                vec!["Version".to_string(), "Status".to_string()],
                vec!["release---candidate".to_string(), "stable".to_string()],
            ]
        );
        assert_eq!(
            crate::rendering::render_markdown(&document),
            "| Version | Status |\n| --- | --- |\n| release---candidate | stable |\n"
        );
    }

    #[test]
    fn should_preserve_headerless_rows_in_structured_and_rendered_org_table() {
        let document = OrgModeExtractor::build_internal_document("| Alice | 30 |\n| Bob | 40 |");
        assert_eq!(document.tables.len(), 1);
        let table = &document.tables[0];

        assert_eq!(
            table.cells,
            vec![
                vec!["Alice".to_string(), "30".to_string()],
                vec!["Bob".to_string(), "40".to_string()],
            ]
        );
        assert_eq!(table.columns, None);
        assert_eq!(table.markdown, "|  |  |\n| --- | --- |\n| Alice | 30 |\n| Bob | 40 |\n");
        assert_eq!(
            crate::rendering::render_markdown(&document),
            "|  |  |\n| --- | --- |\n| Alice | 30 |\n| Bob | 40 |\n"
        );
    }

    #[test]
    fn test_orgmode_extractor_supported_mime_types() {
        let extractor = OrgModeExtractor::new();
        let supported = extractor.supported_mime_types();
        assert!(supported.contains(&"text/x-org"));
    }

    #[test]
    fn test_link_with_description() {
        let org_text = r#"* Links Test

[[http://att.com/][AT&T]]
"#;
        let lines: Vec<String> = org_text.lines().map(|s| s.to_string()).collect();
        let org = Org::from_vec(&lines).expect("Failed to parse org");
        let content = OrgModeExtractor::extract_content(&org);

        assert!(content.contains("AT&T"), "Should contain link description 'AT&T'");
    }

    #[test]
    fn test_link_without_description() {
        let org_text = r#"* Links Test

[[https://example.com]]
"#;
        let lines: Vec<String> = org_text.lines().map(|s| s.to_string()).collect();
        let org = Org::from_vec(&lines).expect("Failed to parse org");
        let content = OrgModeExtractor::extract_content(&org);

        assert!(
            content.contains("example.com"),
            "Should contain link path when no description provided"
        );
    }

    #[test]
    fn test_link_with_ampersand_in_description() {
        let org_text = r#"* Company Links

[[http://att.com/][AT&T Company]]
"#;
        let lines: Vec<String> = org_text.lines().map(|s| s.to_string()).collect();
        let org = Org::from_vec(&lines).expect("Failed to parse org");
        let content = OrgModeExtractor::extract_content(&org);

        assert!(
            content.contains("AT&T"),
            "Should preserve ampersand in link description"
        );
    }

    #[test]
    fn test_multiple_links_with_mixed_descriptions() {
        let org_text = r#"* Multiple Links

[[https://example.com][Example Link]]

[[https://example.org]]

[[mailto:test@example.com][Contact]]
"#;
        let lines: Vec<String> = org_text.lines().map(|s| s.to_string()).collect();
        let org = Org::from_vec(&lines).expect("Failed to parse org");
        let content = OrgModeExtractor::extract_content(&org);

        assert!(content.contains("Example Link"));
        assert!(content.contains("example.org"));
        assert!(content.contains("Contact"));
    }

    #[test]
    fn test_link_description_priority_over_url() {
        let org_text = r#"[[http://att.com/][AT&T]]"#;
        let lines: Vec<String> = org_text.lines().map(|s| s.to_string()).collect();
        let org = Org::from_vec(&lines).expect("Failed to parse org");
        let content = OrgModeExtractor::extract_content(&org);

        assert!(content.contains("AT&T"), "Description should be prioritized over URL");
        assert!(
            !content.contains("[["),
            "Raw org link syntax should be stripped by inline markup processing"
        );
    }

    #[test]
    fn test_emoji_and_cjk_with_inline_markup() {
        let (text, annotations) = OrgModeExtractor::parse_inline_markup("🎉 *太字* テスト");
        assert!(text.contains("🎉"), "Emoji preserved");
        assert!(text.contains("太字"), "Bold content present");
        assert!(text.contains("テスト"), "Trailing CJK preserved");
        assert!(!annotations.is_empty(), "Should have bold annotation");
    }

    #[test]
    fn test_cjk_heading_with_markup() {
        let org_text = "* 見出し\n\n🎉 *太字* テスト";
        let lines: Vec<String> = org_text.lines().map(|s| s.to_string()).collect();
        let org = Org::from_vec(&lines).expect("Failed to parse org");
        let content = OrgModeExtractor::extract_content(&org);
        assert!(content.contains("見出し"), "CJK heading preserved");
        assert!(content.contains("太字"), "Bold CJK text present");
    }

    #[test]
    fn test_src_block_lowercase_produces_code_element() {
        use crate::types::internal::ElementKind;

        let org_text = "#+begin_src python\ndef hello():\n    print(\"Hello, World!\")\n#+end_src\n";
        let doc = OrgModeExtractor::build_internal_document(org_text);

        let code_elements: Vec<_> = doc
            .elements
            .iter()
            .filter(|e| matches!(e.kind, ElementKind::Code))
            .collect();
        assert!(
            !code_elements.is_empty(),
            "Should produce Code element for lowercase #+begin_src block"
        );
        let code = &code_elements[0];
        assert!(
            code.text.contains("def hello():"),
            "Code element should contain the function definition"
        );
        let lang = code
            .attributes
            .as_ref()
            .and_then(|a| a.get("language"))
            .map(|s| s.as_str());
        assert_eq!(lang, Some("python"), "Language should be python");
    }

    #[test]
    fn test_src_block_uppercase_produces_code_element() {
        use crate::types::internal::ElementKind;

        let org_text = "#+BEGIN_SRC bash\necho \"hello\"\n#+END_SRC\n";
        let doc = OrgModeExtractor::build_internal_document(org_text);

        let code_elements: Vec<_> = doc
            .elements
            .iter()
            .filter(|e| matches!(e.kind, ElementKind::Code))
            .collect();
        assert!(
            !code_elements.is_empty(),
            "Should produce Code element for uppercase #+BEGIN_SRC block"
        );
        assert!(code_elements[0].text.contains("echo"));
    }

    #[test]
    fn test_example_block_produces_code_element() {
        use crate::types::internal::ElementKind;

        let org_text = "#+BEGIN_EXAMPLE\nSome example text\nSecond line\n#+END_EXAMPLE\n";
        let doc = OrgModeExtractor::build_internal_document(org_text);

        let code_elements: Vec<_> = doc
            .elements
            .iter()
            .filter(|e| matches!(e.kind, ElementKind::Code))
            .collect();
        assert!(
            !code_elements.is_empty(),
            "Should produce Code element for #+BEGIN_EXAMPLE block"
        );
        assert!(
            code_elements[0].text.contains("Some example text"),
            "Code element should contain example content"
        );
        let lang = code_elements[0].attributes.as_ref().and_then(|a| a.get("language"));
        assert!(lang.is_none(), "EXAMPLE blocks should not have a language attribute");
    }

    #[test]
    fn test_lowercase_example_block_produces_code_element() {
        use crate::types::internal::ElementKind;

        let org_text = "#+begin_example\nExample content\n#+end_example\n";
        let doc = OrgModeExtractor::build_internal_document(org_text);

        let code_elements: Vec<_> = doc
            .elements
            .iter()
            .filter(|e| matches!(e.kind, ElementKind::Code))
            .collect();
        assert!(
            !code_elements.is_empty(),
            "Should produce Code element for lowercase #+begin_example block"
        );
    }

    fn orgmode_warnings(doc: &crate::types::internal::InternalDocument) -> Vec<String> {
        doc.processing_warnings
            .iter()
            .filter(|w| w.source == ORGMODE_WARNING_SOURCE)
            .map(|w| w.message.to_string())
            .collect()
    }

    /// #171: `#+INCLUDE: "file.org"` inlines another file's rendered content,
    /// which this single-file parser has no way to read, whether the keyword
    /// appears in the document preamble or later in the body.
    #[test]
    fn should_warn_when_orgmode_include_keyword_in_preamble_is_skipped() {
        let org_text = "#+TITLE: Doc\n#+INCLUDE: \"chapter1.org\"\n\n* Heading\nBody text\n";
        let doc = OrgModeExtractor::build_internal_document(org_text);

        let warnings = orgmode_warnings(&doc);
        assert_eq!(
            warnings.len(),
            1,
            "expected exactly one orgmode warning, got {warnings:?}"
        );
        assert!(
            warnings[0].contains("chapter1.org") && warnings[0].contains("was not read"),
            "warning must name the skipped #+INCLUDE target, got {warnings:?}"
        );
    }

    #[test]
    fn should_warn_when_orgmode_include_keyword_in_body_is_skipped() {
        let org_text = "* Heading\nBody text\n#+INCLUDE: \"chapter2.org\"\nMore text\n";
        let doc = OrgModeExtractor::build_internal_document(org_text);

        let warnings = orgmode_warnings(&doc);
        assert_eq!(
            warnings.len(),
            1,
            "expected exactly one orgmode warning, got {warnings:?}"
        );
        assert!(
            warnings[0].contains("chapter2.org"),
            "warning must name the skipped #+INCLUDE target, got {warnings:?}"
        );
    }

    /// A document with no `#+INCLUDE` keyword must not warn.
    #[test]
    fn should_not_warn_for_orgmode_document_without_include() {
        let org_text = "#+TITLE: Doc\n\n* Heading\nBody text\n";
        let doc = OrgModeExtractor::build_internal_document(org_text);

        assert!(
            orgmode_warnings(&doc).is_empty(),
            "a document without #+INCLUDE must not warn, got {:?}",
            orgmode_warnings(&doc)
        );
    }
}
