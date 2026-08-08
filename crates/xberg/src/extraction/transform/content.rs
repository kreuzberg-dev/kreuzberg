//! Content processing utilities for transformation.
//!
//! This module handles processing of page content, tables, and images
//! during the transformation to semantic elements.

use crate::types::{BoundingBox, Element, ElementMetadata, ElementType};
use std::collections::HashMap;

use super::elements::{add_paragraphs, detect_list_items, generate_element_id, normalize_line_endings};

/// Detect a markdown ATX heading and return its level (1-6) when matched.
fn detect_markdown_heading(line: &str) -> Option<u8> {
    let trimmed = line.trim_start();
    let mut hashes = 0u8;
    for ch in trimmed.chars() {
        if ch == '#' {
            hashes += 1;
            if hashes > 6 {
                return None;
            }
        } else if ch == ' ' || ch == '\t' {
            return if hashes >= 1 { Some(hashes) } else { None };
        } else {
            return None;
        }
    }
    None
}

/// Detect an `[Image: ...]` placeholder line and return the description text.
fn detect_image_placeholder(line: &str) -> Option<&str> {
    let trimmed = line.trim();
    let inner = trimmed.strip_prefix("[Image: ")?.strip_suffix(']')?;
    Some(inner)
}

/// True when `para_text` is a single-line paragraph matching `\d{1,2}\. [A-Z…]`.
///
/// Untagged PDF chapter headings ("1. Introduction", "6. Campagne Performance")
/// arrive here after `detect_list_items` filters them out via the lone-numbered-line
/// guard.  Promote them to Heading rather than falling through to NarrativeText.
fn detect_isolated_numbered_heading(para_text: &str) -> bool {
    if para_text.contains('\n') {
        return false;
    }
    let trimmed = para_text.trim_start();
    let Some(pos) = trimmed.find('.') else {
        return false;
    };
    let prefix = &trimmed[..pos];
    if !prefix.chars().all(|c| c.is_ascii_digit()) || pos == 0 || pos >= 3 {
        return false;
    }
    let after_dot = &trimmed[pos + 1..];
    if !after_dot.starts_with(' ') {
        return false;
    }
    after_dot.chars().nth(1).is_some_and(|c| c.is_uppercase())
}

/// Map a markdown heading level to the appropriate ElementType.
fn heading_level_to_element_type(level: u8) -> ElementType {
    if level == 1 {
        ElementType::Title
    } else {
        ElementType::Heading
    }
}

/// Add paragraphs to `elements`, but first attempt to classify each paragraph as
/// a markdown heading or an `[Image: ...]` placeholder. Falls back to
/// NarrativeText (via `add_paragraphs`) when neither pattern matches.
fn add_paragraphs_with_classification(
    elements: &mut Vec<Element>,
    text: &str,
    page_number: u32,
    title: &Option<String>,
) {
    if text.is_empty() {
        return;
    }

    let normalized = normalize_line_endings(text);
    let mut leftover = String::new();
    for paragraph in normalized.split("\n\n").filter(|p| !p.trim().is_empty()) {
        let para_text = paragraph.trim();
        if para_text.is_empty() {
            continue;
        }

        let is_single_line = !para_text.contains('\n');

        if is_single_line && let Some(level) = detect_markdown_heading(para_text) {
            if !leftover.is_empty() {
                add_paragraphs(elements, leftover.trim(), page_number, title);
                leftover.clear();
            }
            let element_type = heading_level_to_element_type(level);
            let heading_text = para_text.trim_start_matches('#').trim();
            let element_id = generate_element_id(heading_text, element_type, Some(page_number));
            elements.push(Element {
                element_id,
                element_type,
                text: heading_text.to_string(),
                metadata: ElementMetadata {
                    page_number: Some(page_number),
                    filename: title.clone(),
                    coordinates: None,
                    element_index: Some(elements.len()),
                    additional: {
                        let mut m = HashMap::new();
                        m.insert("heading_level".to_string(), level.to_string());
                        m
                    },
                },
            });
            continue;
        }

        if is_single_line && let Some(description) = detect_image_placeholder(para_text) {
            if !leftover.is_empty() {
                add_paragraphs(elements, leftover.trim(), page_number, title);
                leftover.clear();
            }
            let element_id = generate_element_id(para_text, ElementType::Image, Some(page_number));
            elements.push(Element {
                element_id,
                element_type: ElementType::Image,
                text: para_text.to_string(),
                metadata: ElementMetadata {
                    page_number: Some(page_number),
                    filename: title.clone(),
                    coordinates: None,
                    element_index: Some(elements.len()),
                    additional: {
                        let mut m = HashMap::new();
                        m.insert("image_description".to_string(), description.to_string());
                        m
                    },
                },
            });
            continue;
        }

        if is_single_line && detect_isolated_numbered_heading(para_text) {
            if !leftover.is_empty() {
                add_paragraphs(elements, leftover.trim(), page_number, title);
                leftover.clear();
            }
            let element_id = generate_element_id(para_text, ElementType::Heading, Some(page_number));
            elements.push(Element {
                element_id,
                element_type: ElementType::Heading,
                text: para_text.to_string(),
                metadata: ElementMetadata {
                    page_number: Some(page_number),
                    filename: title.clone(),
                    coordinates: None,
                    element_index: Some(elements.len()),
                    additional: {
                        let mut m = HashMap::new();
                        m.insert("heading_level".to_string(), "1".to_string());
                        m
                    },
                },
            });
            continue;
        }

        if !leftover.is_empty() {
            leftover.push_str("\n\n");
        }
        leftover.push_str(para_text);
    }

    if !leftover.is_empty() {
        add_paragraphs(elements, leftover.trim(), page_number, title);
    }
}

/// Adjust a byte offset to the nearest valid UTF-8 char boundary, searching forward.
fn snap_to_char_boundary(s: &str, offset: usize) -> usize {
    let clamped = offset.min(s.len());
    let mut pos = clamped;
    while pos < s.len() && !s.is_char_boundary(pos) {
        pos += 1;
    }
    pos
}

/// Process page content to extract paragraphs and list items.
pub(super) fn process_content(elements: &mut Vec<Element>, content: &str, page_number: u32, title: &Option<String>) {
    let list_items = detect_list_items(content);
    let mut current_byte_offset = 0;

    for list_item in list_items {
        let safe_start = snap_to_char_boundary(content, list_item.byte_start);
        let safe_end = snap_to_char_boundary(content, list_item.byte_end);
        let safe_current = snap_to_char_boundary(content, current_byte_offset);

        if safe_current < safe_start {
            let text_slice = content[safe_current..safe_start].trim();
            add_paragraphs_with_classification(elements, text_slice, page_number, title);
        }

        let item_text = content[safe_start..safe_end].trim();
        if !item_text.is_empty() {
            let element_id = generate_element_id(item_text, ElementType::ListItem, Some(page_number));
            elements.push(Element {
                element_id,
                element_type: ElementType::ListItem,
                text: item_text.to_string(),
                metadata: ElementMetadata {
                    page_number: Some(page_number),
                    filename: title.clone(),
                    coordinates: None,
                    element_index: Some(elements.len()),
                    additional: {
                        let mut m = HashMap::new();
                        m.insert("indent_level".to_string(), list_item.indent_level.to_string());
                        m.insert("list_type".to_string(), format!("{:?}", list_item.list_type));
                        m
                    },
                },
            });
        }

        current_byte_offset = safe_end;
    }

    if current_byte_offset < content.len() {
        let safe_current = snap_to_char_boundary(content, current_byte_offset);
        let text_slice = content[safe_current..].trim();
        add_paragraphs_with_classification(elements, text_slice, page_number, title);
    }
}

/// Format a table as plain text for element representation.
pub(super) fn format_table_as_text(table: &crate::types::Table) -> String {
    let mut output = String::new();

    for row in &table.cells {
        for (i, cell) in row.iter().enumerate() {
            if i > 0 {
                output.push('\t');
            }
            output.push_str(cell);
        }
        output.push('\n');
    }

    output.trim().to_string()
}

/// Process hierarchy blocks into Title and NarrativeText elements.
///
/// Returns `true` when any body-level block was emitted, indicating
/// the caller should skip the plain-text `process_content` pass to avoid
/// producing duplicate elements. Body blocks without bounding boxes are still
/// emitted (without coordinates); the flag is set regardless of bbox presence.
pub(super) fn process_hierarchy(
    elements: &mut Vec<Element>,
    hierarchy: &crate::types::PageHierarchy,
    page_number: u32,
    title: &Option<String>,
) -> bool {
    let mut has_any_body_blocks = false;

    for block in &hierarchy.blocks {
        let coords = block.bbox.as_ref().map(|(left, top, right, bottom)| BoundingBox {
            x0: *left as f64,
            y0: *top as f64,
            x1: *right as f64,
            y1: *bottom as f64,
        });

        let element_type = match block.level.as_str() {
            "h1" => ElementType::Title,
            "h2" | "h3" | "h4" | "h5" | "h6" => ElementType::Heading,
            _ => {
                if block.text.trim().is_empty() {
                    continue;
                }
                has_any_body_blocks = true;
                let element_id = generate_element_id(&block.text, ElementType::NarrativeText, Some(page_number));
                elements.push(Element {
                    element_id,
                    element_type: ElementType::NarrativeText,
                    text: block.text.clone(),
                    metadata: ElementMetadata {
                        page_number: Some(page_number),
                        filename: title.clone(),
                        coordinates: coords,
                        element_index: Some(elements.len()),
                        additional: {
                            let mut m = HashMap::new();
                            m.insert("font_size".to_string(), block.font_size.to_string());
                            m
                        },
                    },
                });
                continue;
            }
        };

        let element_id = generate_element_id(&block.text, element_type, Some(page_number));
        elements.push(Element {
            element_id,
            element_type,
            text: block.text.clone(),
            metadata: ElementMetadata {
                page_number: Some(page_number),
                filename: title.clone(),
                coordinates: coords,
                element_index: Some(elements.len()),
                additional: {
                    let mut m = HashMap::new();
                    m.insert("level".to_string(), block.level.clone());
                    m.insert("font_size".to_string(), block.font_size.to_string());
                    if let Some(level_digit) = block.level.strip_prefix('h').and_then(|s| s.parse::<u8>().ok()) {
                        m.insert("heading_level".to_string(), level_digit.to_string());
                    }
                    m
                },
            },
        });
    }

    has_any_body_blocks
}

/// Process tables on a page into Table elements.
pub(super) fn process_tables(
    elements: &mut Vec<Element>,
    tables: &[std::sync::Arc<crate::types::Table>],
    page_number: u32,
    title: &Option<String>,
) {
    for table_arc in tables {
        let table = table_arc.as_ref();
        let table_text = format_table_as_text(table);

        let element_id = generate_element_id(&table_text, ElementType::Table, Some(page_number));
        elements.push(Element {
            element_id,
            element_type: ElementType::Table,
            text: table_text,
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

/// Process images on a page into Image elements.
pub(super) fn process_images(
    elements: &mut Vec<Element>,
    image_indices: &[u32],
    all_images: &[crate::types::ExtractedImage],
    page_number: u32,
    title: &Option<String>,
) {
    for &idx in image_indices {
        let Some(image) = all_images.get(idx as usize) else {
            continue;
        };
        let image_text = format!(
            "Image: {} ({}x{})",
            image.format,
            image.width.unwrap_or(0),
            image.height.unwrap_or(0)
        );

        let element_id = generate_element_id(&image_text, ElementType::Image, Some(page_number));
        elements.push(Element {
            element_id,
            element_type: ElementType::Image,
            text: image_text,
            metadata: ElementMetadata {
                page_number: Some(page_number),
                filename: title.clone(),
                coordinates: None,
                element_index: Some(elements.len()),
                additional: {
                    let mut m = HashMap::new();
                    m.insert("image_index".to_string(), idx.to_string());
                    m.insert("format".to_string(), image.format.to_string());
                    if let Some(width) = image.width {
                        m.insert("width".to_string(), width.to_string());
                    }
                    if let Some(height) = image.height {
                        m.insert("height".to_string(), height.to_string());
                    }
                    m
                },
            },
        });
    }
}

/// Add a PageBreak element between pages.
pub(super) fn add_page_break(elements: &mut Vec<Element>, current_page: u32, next_page: u32, title: &Option<String>) {
    let page_break_text = format!("--- PAGE BREAK (page {} → {}) ---", current_page, next_page);
    let element_id = generate_element_id(&page_break_text, ElementType::PageBreak, Some(current_page));
    elements.push(Element {
        element_id,
        element_type: ElementType::PageBreak,
        text: page_break_text,
        metadata: ElementMetadata {
            page_number: Some(current_page),
            filename: title.clone(),
            coordinates: None,
            element_index: Some(elements.len()),
            additional: HashMap::new(),
        },
    });
}

#[cfg(test)]
mod tests_issue_782 {
    use super::*;
    use crate::types::{HierarchicalBlock, PageHierarchy};

    fn block(level: &str, text: &str) -> HierarchicalBlock {
        HierarchicalBlock {
            level: level.to_string(),
            text: text.to_string(),
            font_size: 12.0,
            bbox: None,
        }
    }

    #[test]
    fn test_process_hierarchy_h1_is_title_h2_h6_is_heading() {
        let mut elements = Vec::new();
        let blocks = vec![
            block("h1", "Document Title"),
            block("h2", "Section A"),
            block("h3", "Subsection"),
            block("h4", "Sub-sub"),
            block("h5", "Deeper"),
            block("h6", "Deepest"),
        ];
        let hierarchy = PageHierarchy {
            block_count: blocks.len() as u32,
            blocks,
        };
        process_hierarchy(&mut elements, &hierarchy, 1, &None);

        assert_eq!(elements.len(), 6);
        assert_eq!(elements[0].element_type, ElementType::Title);
        assert_eq!(
            elements[0].metadata.additional.get("heading_level").map(String::as_str),
            Some("1")
        );
        for (i, expected_level) in (2u8..=6u8).enumerate() {
            assert_eq!(elements[i + 1].element_type, ElementType::Heading);
            assert_eq!(
                elements[i + 1]
                    .metadata
                    .additional
                    .get("heading_level")
                    .map(String::as_str),
                Some(expected_level.to_string().as_str())
            );
        }
    }

    #[test]
    fn test_detect_markdown_heading() {
        assert_eq!(detect_markdown_heading("# Title"), Some(1));
        assert_eq!(detect_markdown_heading("## H2"), Some(2));
        assert_eq!(detect_markdown_heading("###### H6"), Some(6));
        assert_eq!(detect_markdown_heading("####### too many"), None);
        assert_eq!(detect_markdown_heading("#no space"), None);
        assert_eq!(detect_markdown_heading("not a heading"), None);
        assert_eq!(detect_markdown_heading("  ## indented"), Some(2));
    }

    #[test]
    fn test_detect_image_placeholder() {
        assert_eq!(detect_image_placeholder("[Image: Cover]"), Some("Cover"));
        assert_eq!(
            detect_image_placeholder("[Image: jpeg (640x480)]"),
            Some("jpeg (640x480)")
        );
        assert_eq!(detect_image_placeholder("[Image:no space]"), None);
        assert_eq!(detect_image_placeholder("not a placeholder"), None);
        assert_eq!(detect_image_placeholder("[Image: foo] trailing"), None);
    }

    #[test]
    fn test_process_content_classifies_markdown_headings() {
        let mut elements = Vec::new();
        let content = "# Title\n\n## Section\n\n### Sub\n\nbody paragraph here.";
        process_content(&mut elements, content, 1, &None);

        let kinds: Vec<_> = elements.iter().map(|e| (e.element_type, e.text.as_str())).collect();
        assert_eq!(kinds[0], (ElementType::Title, "Title"));
        assert_eq!(kinds[1], (ElementType::Heading, "Section"));
        assert_eq!(
            elements[1].metadata.additional.get("heading_level").map(String::as_str),
            Some("2")
        );
        assert_eq!(kinds[2], (ElementType::Heading, "Sub"));
        assert_eq!(
            elements[2].metadata.additional.get("heading_level").map(String::as_str),
            Some("3")
        );
        assert_eq!(kinds[3].0, ElementType::NarrativeText);
        assert_eq!(kinds[3].1, "body paragraph here.");
    }

    #[test]
    fn test_process_content_emits_image_placeholder_as_image_element() {
        let mut elements = Vec::new();
        let content = "Intro text.\n\n[Image: Cover]\n\nMore text.";
        process_content(&mut elements, content, 1, &None);

        let image_idx = elements
            .iter()
            .position(|e| e.element_type == ElementType::Image)
            .expect("image placeholder should produce an Image element");
        assert_eq!(elements[image_idx].text, "[Image: Cover]");
        assert_eq!(
            elements[image_idx]
                .metadata
                .additional
                .get("image_description")
                .map(String::as_str),
            Some("Cover")
        );
    }
}

#[cfg(test)]
mod tests_issue_961 {
    use super::*;

    /// Isolated "N. Title" paragraphs must produce Heading elements, not ListItem.
    #[test]
    fn isolated_numbered_heading_becomes_heading_not_list_item() {
        let mut elements = Vec::new();
        let content = "1. Managementsamenvatting\n\nDit rapport geeft een overzicht van de prestaties.";
        process_content(&mut elements, content, 1, &None);

        let headings: Vec<_> = elements
            .iter()
            .filter(|e| e.element_type == ElementType::Heading)
            .collect();
        let list_items: Vec<_> = elements
            .iter()
            .filter(|e| e.element_type == ElementType::ListItem)
            .collect();

        assert_eq!(headings.len(), 1, "should produce one Heading element");
        assert_eq!(headings[0].text, "1. Managementsamenvatting");
        assert_eq!(
            headings[0].metadata.additional.get("heading_level").map(String::as_str),
            Some("1")
        );
        assert_eq!(list_items.len(), 0, "should not produce any ListItem");
    }

    /// Multiple chapter headings across simulated pages — each isolated, each becomes Heading.
    #[test]
    fn multiple_chapter_headings_on_separate_pages() {
        let content = "1. Managementsamenvatting\n\nIntro body.\n\n\
                       2. Lead Generation per Channel\n\nBody two.\n\n\
                       3. Key Performance Indicators\n\nBody three.";
        let mut elements = Vec::new();
        process_content(&mut elements, content, 1, &None);

        let headings: Vec<_> = elements
            .iter()
            .filter(|e| e.element_type == ElementType::Heading)
            .collect();
        assert_eq!(headings.len(), 3, "each chapter heading should be a Heading");
        assert_eq!(headings[0].text, "1. Managementsamenvatting");
        assert_eq!(headings[1].text, "2. Lead Generation per Channel");
        assert_eq!(headings[2].text, "3. Key Performance Indicators");

        let list_items: Vec<_> = elements
            .iter()
            .filter(|e| e.element_type == ElementType::ListItem)
            .collect();
        assert_eq!(list_items.len(), 0);
    }

    /// Real numbered lists (sibling items on consecutive lines) still produce ListItems.
    #[test]
    fn real_numbered_list_still_produces_list_items() {
        let content = "Introduction.\n\n1. First step\n2. Second step\n3. Third step\n\nConclusion.";
        let mut elements = Vec::new();
        process_content(&mut elements, content, 1, &None);

        let list_items: Vec<_> = elements
            .iter()
            .filter(|e| e.element_type == ElementType::ListItem)
            .collect();
        assert_eq!(list_items.len(), 3, "real numbered list must remain as ListItems");
        assert!(list_items.iter().all(|e| e.element_type == ElementType::ListItem));

        let headings: Vec<_> = elements
            .iter()
            .filter(|e| e.element_type == ElementType::Heading)
            .collect();
        assert_eq!(headings.len(), 0, "real list items must not be promoted to Heading");
    }

    /// Numbered item following a bullet in the same block stays a ListItem.
    #[test]
    fn numbered_item_with_bullet_sibling_stays_list_item() {
        let content = "Context.\n\n- Bullet point\n1. Numbered item\n- Another bullet\n\nRest.";
        let mut elements = Vec::new();
        process_content(&mut elements, content, 1, &None);

        let list_items: Vec<_> = elements
            .iter()
            .filter(|e| e.element_type == ElementType::ListItem)
            .collect();
        assert!(
            list_items.iter().any(|e| e.text.contains("Numbered item")),
            "numbered item with a bullet sibling must remain a ListItem"
        );
    }

    /// Lowercase-start numbered line stays NarrativeText, not promoted to Heading.
    #[test]
    fn lowercase_numbered_line_not_promoted() {
        let content = "1. lowercase start\n\nBody text.";
        let mut elements = Vec::new();
        process_content(&mut elements, content, 1, &None);

        let headings: Vec<_> = elements
            .iter()
            .filter(|e| e.element_type == ElementType::Heading)
            .collect();
        assert_eq!(headings.len(), 0, "lowercase numbered line must not become Heading");
    }

    /// A lone lowercase-numbered line must NOT be promoted to Heading — it is a
    /// real list item that happens to have no siblings.  Regression guard for the
    /// is_lone_numbered_line_in_paragraph suppression scope.
    #[test]
    fn lone_lowercase_numbered_line_stays_list_item() {
        let content = "Context.\n\n1. lowercase alone\n\nTrailing.";
        let mut elements = Vec::new();
        process_content(&mut elements, content, 1, &None);

        let list_items: Vec<_> = elements
            .iter()
            .filter(|e| e.element_type == ElementType::ListItem)
            .collect();
        assert_eq!(list_items.len(), 1, "lone lowercase numbered line must remain ListItem");
        assert!(list_items[0].text.contains("lowercase alone"));
    }

    /// Real list block + isolated chapter headings in the same document must be
    /// classified independently: list items stay ListItem, lone uppercase-initial
    /// numbered lines become Heading.
    #[test]
    fn mixed_real_list_and_isolated_chapters() {
        let content = "1. First list item\n2. Second list item\n\n5. Chapter Five\n\n6. Chapter Six\n\nBody.";
        let mut elements = Vec::new();
        process_content(&mut elements, content, 1, &None);

        let list_items: Vec<_> = elements
            .iter()
            .filter(|e| e.element_type == ElementType::ListItem)
            .collect();
        let headings: Vec<_> = elements
            .iter()
            .filter(|e| e.element_type == ElementType::Heading)
            .collect();

        assert_eq!(
            list_items.len(),
            2,
            "items 1. and 2. share a block — both stay ListItem"
        );
        assert!(list_items.iter().any(|e| e.text.contains("First list item")));
        assert!(list_items.iter().any(|e| e.text.contains("Second list item")));

        assert_eq!(
            headings.len(),
            2,
            "items 5. and 6. are each isolated — both become Heading"
        );
        assert!(headings.iter().any(|e| e.text.contains("Chapter Five")));
        assert!(headings.iter().any(|e| e.text.contains("Chapter Six")));
    }

    /// Sub-section notation like "2.1 Title" must not be promoted (not a list pattern).
    #[test]
    fn subsection_notation_not_promoted() {
        let content = "2.1 Tabeloverzicht\n\nBody text.";
        let mut elements = Vec::new();
        process_content(&mut elements, content, 1, &None);

        let headings: Vec<_> = elements
            .iter()
            .filter(|e| e.element_type == ElementType::Heading)
            .collect();
        assert_eq!(
            headings.len(),
            0,
            "sub-section notation must not match the numbered-heading pattern"
        );
    }

    /// Detect helper rejects multi-line paragraphs.
    #[test]
    fn detect_isolated_numbered_heading_rejects_multi_line() {
        assert!(!detect_isolated_numbered_heading("1. Title\nSecond line"));
    }

    /// Detect helper accepts valid patterns.
    #[test]
    fn detect_isolated_numbered_heading_accepts_valid_patterns() {
        assert!(detect_isolated_numbered_heading("1. Introduction"));
        assert!(detect_isolated_numbered_heading(
            "6. Campagne Performance per Doelgroep & Kanaal"
        ));
        assert!(detect_isolated_numbered_heading("10. Final Chapter"));
    }

    /// Detect helper rejects edge cases.
    #[test]
    fn detect_isolated_numbered_heading_rejects_edge_cases() {
        assert!(!detect_isolated_numbered_heading("1.  No capital after space"));
        assert!(!detect_isolated_numbered_heading("1.Introduction"));
        assert!(!detect_isolated_numbered_heading("100. Too many digits"));
        assert!(!detect_isolated_numbered_heading(""));
    }
}
