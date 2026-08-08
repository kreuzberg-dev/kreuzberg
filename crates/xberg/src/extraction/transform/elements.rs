//! Element generation and list detection utilities.
//!
//! This module provides functions for detecting semantic elements in text,
//! including list items, and generating unique element IDs.

use crate::types::{Element, ElementId, ElementMetadata, ElementType};
use std::collections::HashMap;

use super::types::{ListItemMetadata, ListType};

/// Detect list items in text with support for multiple formats.
///
/// Identifies bullet points, numbered items, and indented items.
/// Supports formats like:
/// - `- bullet item`
/// - `* bullet item`
/// - `• bullet item`
/// - `1. numbered item`
/// - `a. lettered item`
/// - Indented items with leading whitespace
///
/// # Arguments
///
/// * `text` - The text to search for list items
///
/// # Returns
///
/// A vector of ListItemMetadata structs describing detected list items
pub(crate) fn detect_list_items(text: &str) -> Vec<ListItemMetadata> {
    let mut items = Vec::new();
    let lines: Vec<&str> = text.lines().collect();

    let mut current_byte_offset = 0;

    for line in lines {
        let line_start_offset = current_byte_offset;
        let trimmed = line.trim_start();
        let indent_level = (line.len() - trimmed.len()) / 2;

        let byte_end = line_start_offset + line.len();

        let next_offset = if byte_end < text.len() {
            let rest = &text.as_bytes()[byte_end..];
            if rest.starts_with(b"\r\n") {
                byte_end + 2
            } else if rest.starts_with(b"\n") || rest.starts_with(b"\r") {
                byte_end + 1
            } else {
                byte_end
            }
        } else {
            byte_end
        };

        if let Some(stripped) = trimmed.strip_prefix('-')
            && (stripped.starts_with(' ') || stripped.is_empty())
        {
            items.push(ListItemMetadata {
                list_type: ListType::Bullet,
                byte_start: line_start_offset,
                byte_end,
                indent_level: indent_level as u32,
            });
            current_byte_offset = next_offset;
            continue;
        }

        if let Some(stripped) = trimmed.strip_prefix('*')
            && (stripped.starts_with(' ') || stripped.is_empty())
        {
            items.push(ListItemMetadata {
                list_type: ListType::Bullet,
                byte_start: line_start_offset,
                byte_end,
                indent_level: indent_level as u32,
            });
            current_byte_offset = next_offset;
            continue;
        }

        if let Some(stripped) = trimmed.strip_prefix('•')
            && (stripped.starts_with(' ') || stripped.is_empty())
        {
            items.push(ListItemMetadata {
                list_type: ListType::Bullet,
                byte_start: line_start_offset,
                byte_end,
                indent_level: indent_level as u32,
            });
            current_byte_offset = next_offset;
            continue;
        }

        if let Some(pos) = trimmed.find('.') {
            let prefix = &trimmed[..pos];
            let uppercase_initial = trimmed[pos + 1..].chars().nth(1).is_some_and(|c| c.is_uppercase());
            if prefix.chars().all(|c| c.is_ascii_digit())
                && pos > 0
                && pos < 3
                && trimmed.len() > pos + 1
                && trimmed[pos + 1..].starts_with(' ')
                && !(uppercase_initial && is_lone_numbered_line_in_paragraph(text, line_start_offset))
            {
                items.push(ListItemMetadata {
                    list_type: ListType::Numbered,
                    byte_start: line_start_offset,
                    byte_end,
                    indent_level: indent_level as u32,
                });
                current_byte_offset = next_offset;
                continue;
            }
        }

        if let Some(pos) = trimmed.find('.') {
            let prefix = &trimmed[..pos];
            if prefix.len() == 1
                && prefix.chars().all(|c| c.is_alphabetic())
                && pos > 0
                && trimmed.len() > pos + 1
                && trimmed[pos + 1..].starts_with(' ')
            {
                items.push(ListItemMetadata {
                    list_type: ListType::Lettered,
                    byte_start: line_start_offset,
                    byte_end,
                    indent_level: indent_level as u32,
                });
                current_byte_offset = next_offset;
                continue;
            }
        }

        if indent_level >= 2 && !trimmed.is_empty() {
            items.push(ListItemMetadata {
                list_type: ListType::Indented,
                byte_start: line_start_offset,
                byte_end,
                indent_level: indent_level as u32,
            });
            current_byte_offset = next_offset;
            continue;
        }

        current_byte_offset = next_offset;
    }

    items
}

/// Generate a unique element ID for semantic content.
///
/// Creates a deterministic hash-based ID from the element type, text content,
/// and page number. Uses a simple wrapping multiplication algorithm for
/// consistent ID generation without external dependencies.
///
/// # Arguments
///
/// * `text` - The element text content
/// * `element_type` - The semantic element type
/// * `page_number` - Optional page number for multi-page documents
///
/// # Returns
///
/// An ElementId suitable for referencing this semantic element
pub(crate) fn generate_element_id(text: &str, element_type: ElementType, page_number: Option<u32>) -> ElementId {
    let type_hash = format!("{:?}", element_type)
        .bytes()
        .fold(0u64, |acc, b| acc.wrapping_mul(31).wrapping_add(b as u64));

    let text_hash = text
        .bytes()
        .fold(0u64, |acc, b| acc.wrapping_mul(31).wrapping_add(b as u64));

    let page_hash = page_number
        .unwrap_or(1)
        .to_string()
        .bytes()
        .fold(0u64, |acc, b| acc.wrapping_mul(31).wrapping_add(b as u64));

    let combined = type_hash
        .wrapping_mul(65599)
        .wrapping_add(text_hash)
        .wrapping_mul(65599)
        .wrapping_add(page_hash);

    ElementId::new(format!("elem-{:x}", combined)).expect("ElementId creation failed")
}

/// True when `trimmed` matches the numbered-list pattern `\d{1,2}\. `.
fn is_numbered_list_line(trimmed: &str) -> bool {
    if let Some(pos) = trimmed.find('.') {
        let prefix = &trimmed[..pos];
        prefix.chars().all(|c| c.is_ascii_digit())
            && pos > 0
            && pos < 3
            && trimmed.len() > pos + 1
            && trimmed[pos + 1..].starts_with(' ')
    } else {
        false
    }
}

/// True when the `\n\n`-bounded paragraph containing `line_offset` holds exactly
/// one list-like line (numbered or bulleted).  Chapter headings like
/// "1. Introduction" are isolated this way; real numbered lists have siblings.
fn is_lone_numbered_line_in_paragraph(text: &str, line_offset: usize) -> bool {
    let para_start = text[..line_offset].rfind("\n\n").map_or(0, |p| p + 2);
    let para_end = text[line_offset..].find("\n\n").map_or(text.len(), |p| line_offset + p);
    let paragraph = &text[para_start..para_end];
    let list_line_count = paragraph
        .lines()
        .filter(|l| {
            let t = l.trim_start();
            is_numbered_list_line(t) || t.starts_with("- ") || t.starts_with("* ") || t.starts_with("• ")
        })
        .count();
    list_line_count == 1
}

/// Normalize `\r\n` and lone `\r` line endings to `\n`.
///
/// `str::split("\n\n")` (used throughout this module to find paragraph
/// boundaries) only recognizes bare LF blank lines. Windows-authored CRLF
/// text has blank lines encoded as `\r\n\r\n`, and classic Mac-style text
/// uses a lone `\r` with no `\n` at all — neither is a `"\n\n"` match, so a
/// CRLF or lone-CR document collapsed into a single paragraph (xberg-io/xberg#227).
/// Normalizing every line ending to `\n` up front makes the existing
/// `"\n\n"` paragraph-boundary logic (and downstream `str::lines()` calls)
/// work identically regardless of source line-ending style, including
/// documents that mix styles.
pub(crate) fn normalize_line_endings(text: &str) -> std::borrow::Cow<'_, str> {
    if !text.contains('\r') {
        return std::borrow::Cow::Borrowed(text);
    }

    let mut normalized = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\r' {
            if chars.peek() == Some(&'\n') {
                chars.next();
            }
            normalized.push('\n');
        } else {
            normalized.push(c);
        }
    }
    std::borrow::Cow::Owned(normalized)
}

/// Add paragraphs as NarrativeText elements, splitting on double newlines.
pub(super) fn add_paragraphs(elements: &mut Vec<Element>, text: &str, page_number: u32, title: &Option<String>) {
    if text.is_empty() {
        return;
    }

    let normalized = normalize_line_endings(text);
    for paragraph in normalized.split("\n\n").filter(|p| !p.trim().is_empty()) {
        let para_text = paragraph.trim();
        if para_text.is_empty() {
            continue;
        }

        let element_id = generate_element_id(para_text, ElementType::NarrativeText, Some(page_number));
        elements.push(Element {
            element_id,
            element_type: ElementType::NarrativeText,
            text: para_text.to_string(),
            metadata: ElementMetadata {
                page_number: Some(page_number),
                filename: title.clone(),
                coordinates: None,
                element_index: Some(elements.len()),
                additional: HashMap::new(),
            },
        });
    }
}
