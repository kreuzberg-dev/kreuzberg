//! hOCR → `InternalDocument` parser.
//!
//! Adapted from `html_to_markdown_rs::hocr` property and element parsing.
//!
//! This module parses hOCR HTML produced by Tesseract (and compatible engines)
//! into xberg's `InternalDocument` representation, preserving bounding boxes,
//! confidence scores, and page structure.
//!
//! ## hOCR hierarchy handled
//!
//! ```text
//! ocr_page  →  PageBreak between pages
//!   ocr_carea / ocrx_block
//!     ocr_par  →  InternalElement (OcrText::Block)
//!       ocr_line / ocrx_line  →  line break within paragraph
//!         ocrx_word  →  word text with bbox and confidence
//! ```

use crate::types::extraction::BoundingBox;
use crate::types::internal::{ElementKind, InternalDocument, InternalElement};
use crate::types::ocr_elements::{OcrBoundingGeometry, OcrConfidence, OcrElementLevel};

/// Attribute used to retain the logical hOCR block enclosing an `ocr_par`.
pub(crate) const HOCR_BLOCK_ID_ATTRIBUTE: &str = "hocr_block_id";

#[derive(Debug)]
struct HocrBlockExtent {
    end: usize,
    id: String,
}

/// Parse hOCR HTML into an `InternalDocument` with full spatial and confidence metadata.
///
/// This is the primary entry point. It replaces the older `convert_hocr_to_markdown` path
/// by producing structured [`InternalElement`]s directly, preserving OCR geometry and
/// confidence that the markdown conversion discards.
///
/// # Arguments
///
/// * `hocr_html` — raw hOCR output from Tesseract (or compatible engine).
///
/// # Output mapping
///
/// | hOCR element   | xberg element                             |
/// |---------------|-----------------------------------------------|
/// | `ocr_page`    | `PageBreak` between consecutive pages         |
/// | `ocr_par`     | `OcrText { level: Block }` with union bbox    |
/// | `ocr_line`    | newline separator within a paragraph          |
/// | `ocrx_word`   | word text, bbox, `x_wconf` → `OcrConfidence` |
///
/// Page numbers come from the `ppageno` title property (converted to 1-indexed).
pub(crate) fn parse_hocr_to_internal_document(hocr_html: &str) -> InternalDocument {
    let mut doc = InternalDocument::new("ocr");
    doc.mime_type = "application/x-hocr".to_string();

    let mut element_index: u32 = 0;
    let mut last_page: Option<u32> = None;

    let bytes = hocr_html.as_bytes();
    let mut pos = 0;
    let mut block_extents = Vec::<HocrBlockExtent>::new();

    while pos < bytes.len() {
        let Some(tag_start) = memchr(b'<', &bytes[pos..]).map(|i| pos + i) else {
            break;
        };
        let Some(tag_end) = memchr(b'>', &bytes[tag_start..]).map(|i| tag_start + i) else {
            break;
        };
        let tag_content = &hocr_html[tag_start + 1..tag_end];
        pos = tag_end + 1;
        block_extents.retain(|extent| tag_start < extent.end);

        if tag_content.starts_with('/') || tag_content.ends_with('/') {
            continue;
        }

        if has_class(tag_content, "ocr_page") {
            let title = extract_title_attr(tag_content);
            let props = parse_title_properties(&title);
            let page_number = props.ppageno.map(|p| p + 1);

            if let Some(prev) = last_page
                && page_number != Some(prev)
            {
                let pb = InternalElement::text(ElementKind::PageBreak, "", 0).with_index(element_index);
                element_index += 1;
                doc.push_element(pb);
            }
            last_page = page_number;
            continue;
        }

        if has_class(tag_content, "ocr_carea") || has_class(tag_content, "ocrx_block") {
            let tag_name = tag_content
                .split_whitespace()
                .next()
                .unwrap_or("div")
                .to_ascii_lowercase();
            let end = skip_to_matching_close(hocr_html, pos, &tag_name);
            let id = extract_attribute(tag_content, "id")
                .filter(|id| !id.is_empty())
                .unwrap_or_else(|| format!("hocr-block-{tag_start}-{end}"));
            block_extents.push(HocrBlockExtent { end, id });
            continue;
        }

        if is_paragraph_tag(tag_content) {
            let par_tag_name = tag_content
                .split_whitespace()
                .next()
                .unwrap_or("p")
                .to_ascii_lowercase();
            let (paragraph, end_pos) =
                parse_paragraph(hocr_html, pos, last_page.unwrap_or(1), element_index, &par_tag_name);
            pos = end_pos;

            if let Some(mut elem) = paragraph {
                if let Some(block) = block_extents.last() {
                    elem.attributes
                        .get_or_insert_with(Default::default)
                        .insert(HOCR_BLOCK_ID_ATTRIBUTE.to_string(), block.id.clone());
                }
                element_index += 1;
                doc.push_element(elem);
            }
        }
    }

    tracing::debug!(
        input_bytes = hocr_html.len(),
        elements = doc.elements.len(),
        total_text_chars = doc.elements.iter().map(|e| e.text.len()).sum::<usize>(),
        "hOCR parse complete"
    );

    doc
}

/// Parsed properties from an hOCR `title` attribute.
#[derive(Debug, Default)]
struct HocrProperties {
    /// Bounding box: (x1, y1, x2, y2).
    bbox: Option<(u32, u32, u32, u32)>,
    /// Word confidence 0–100.
    x_wconf: Option<f64>,
    /// Physical page number (0-indexed from Tesseract).
    ppageno: Option<u32>,
    /// Text rotation angle.
    textangle: Option<f64>,
    /// Baseline (slope, constant).
    baseline: Option<(f64, i32)>,
    /// Font name.
    x_font: Option<String>,
    /// Font size in points.
    x_fsize: Option<u32>,
}

/// Parse all properties from an hOCR title attribute string.
///
/// Handles the semicolon-separated `key value ...` format produced by Tesseract:
///
/// ```text
/// bbox 100 50 200 150; x_wconf 95; ppageno 0
/// ```
fn parse_title_properties(title: &str) -> HocrProperties {
    let mut props = HocrProperties::default();

    for part in title.split(';') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }

        let mut tokens = part.split_whitespace();
        let Some(key) = tokens.next() else {
            continue;
        };

        match key {
            "bbox" => {
                let coords: Vec<u32> = tokens.filter_map(|s| s.parse().ok()).collect();
                if coords.len() == 4 {
                    props.bbox = Some((coords[0], coords[1], coords[2], coords[3]));
                }
            }
            "x_wconf" => {
                if let Some(val) = tokens.next().and_then(|s| s.parse::<f64>().ok()) {
                    props.x_wconf = Some(val);
                }
            }
            "ppageno" => {
                if let Some(val) = tokens.next().and_then(|s| s.parse::<u32>().ok()) {
                    props.ppageno = Some(val);
                }
            }
            "textangle" => {
                if let Some(val) = tokens.next().and_then(|s| s.parse::<f64>().ok()) {
                    props.textangle = Some(val);
                }
            }
            "baseline" => {
                let slope = tokens.next().and_then(|s| s.parse::<f64>().ok());
                let constant = tokens.next().and_then(|s| s.parse::<i32>().ok());
                if let (Some(s), Some(c)) = (slope, constant) {
                    props.baseline = Some((s, c));
                }
            }
            "x_font" => {
                props.x_font = parse_quoted_value(part);
            }
            "x_fsize" => {
                if let Some(val) = tokens.next().and_then(|s| s.parse::<u32>().ok()) {
                    props.x_fsize = Some(val);
                }
            }
            _ => {}
        }
    }

    props
}

/// Extract a quoted string value from a property part like `x_font "Arial"`.
fn parse_quoted_value(part: &str) -> Option<String> {
    let start = part.find('"')?;
    let end = part[start + 1..].find('"')?;
    Some(part[start + 1..start + 1 + end].to_string())
}

/// A word extracted from hOCR with its metadata.
struct HocrWordInfo {
    text: String,
    x0: u32,
    y0: u32,
    x1: u32,
    y1: u32,
    confidence: Option<f64>,
    /// Font size in points, from the word's `x_fsize` hOCR property.
    font_size: Option<u32>,
    /// Text rotation angle in degrees, from the word's `textangle` hOCR property.
    text_angle: Option<f64>,
}

/// Attribute key holding the paragraph's average word font size (points, as a
/// decimal string). Consumed by markdown assembly to promote large-font
/// paragraphs to headings (#185).
pub(crate) const HOCR_FONT_SIZE_ATTRIBUTE: &str = "x_fsize";

/// Attribute key holding the paragraph's average word text-rotation angle in
/// degrees (as a decimal string), when any word reported a non-zero angle.
pub(crate) const HOCR_TEXT_ANGLE_ATTRIBUTE: &str = "textangle";

/// Parse a single `<p class="ocr_par">` (or `<span class="ocr_par">`) and all nested
/// content up to the matching closing tag.
///
/// `par_tag` is the lowercase tag name of the paragraph element (e.g. "p", "span", "div").
/// Depth tracking uses ONLY matching tag names to find the paragraph's closing tag.
/// This prevents inner elements (lines, words, formatting) from interfering with
/// the paragraph boundary detection — even if their subtrees are malformed.
///
/// Returns the constructed element (if any words were found) and the byte position
/// after the closing tag.
fn parse_paragraph(
    html: &str,
    start: usize,
    page: u32,
    element_index: u32,
    par_tag: &str,
) -> (Option<InternalElement>, usize) {
    let bytes = html.as_bytes();
    let mut pos = start;

    let mut lines: Vec<Vec<HocrWordInfo>> = Vec::new();
    let mut current_line: Vec<HocrWordInfo> = Vec::new();
    let mut in_line = false;

    let mut depth: u32 = 1;

    while pos < bytes.len() {
        let Some(tag_start) = memchr(b'<', &bytes[pos..]).map(|i| pos + i) else {
            break;
        };
        let Some(tag_end) = memchr(b'>', &bytes[tag_start..]).map(|i| tag_start + i) else {
            break;
        };
        let tag_content = &html[tag_start + 1..tag_end];
        pos = tag_end + 1;

        if let Some(stripped) = tag_content.strip_prefix('/') {
            let closing_name = stripped.trim().to_ascii_lowercase();
            if closing_name == par_tag {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    if !current_line.is_empty() {
                        lines.push(std::mem::take(&mut current_line));
                    }
                    break;
                }
            }
            continue;
        }

        if tag_content.ends_with('/') {
            continue;
        }

        let tag_name = tag_content.split_whitespace().next().unwrap_or("").to_ascii_lowercase();

        if has_class(tag_content, "ocr_line") || has_class(tag_content, "ocrx_line") {
            if in_line && !current_line.is_empty() {
                lines.push(std::mem::take(&mut current_line));
            }
            in_line = true;
            if tag_name == par_tag {
                depth += 1;
            }
            continue;
        }

        if has_class(tag_content, "ocrx_word") {
            let title = extract_title_attr(tag_content);
            let props = parse_title_properties(&title);

            let word_text = extract_inner_text(html, pos);
            let trimmed = decode_html_entities(&word_text);
            let trimmed = trimmed.trim();

            pos = skip_to_matching_close(html, pos, &tag_name);

            if !trimmed.is_empty() {
                let (x0, y0, x1, y1) = props.bbox.unwrap_or((0, 0, 0, 0));
                current_line.push(HocrWordInfo {
                    text: trimmed.to_string(),
                    x0,
                    y0,
                    x1,
                    y1,
                    confidence: props.x_wconf,
                    font_size: props.x_fsize,
                    text_angle: props.textangle,
                });
            }
            continue;
        }

        if tag_name == par_tag {
            depth += 1;
        }
    }

    let all_words: Vec<&HocrWordInfo> = lines.iter().flat_map(|l| l.iter()).collect();
    if all_words.is_empty() {
        return (None, pos);
    }

    let text: String = lines
        .iter()
        .map(|line| line.iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join(" "))
        .collect::<Vec<_>>()
        .join("\n");

    let mut min_x0 = u32::MAX;
    let mut min_y0 = u32::MAX;
    let mut max_x1 = 0u32;
    let mut max_y1 = 0u32;
    let mut conf_sum = 0.0f64;
    let mut conf_count = 0u32;
    let mut font_size_sum = 0u32;
    let mut font_size_count = 0u32;
    let mut angle_sum = 0.0f64;
    let mut angle_count = 0u32;

    for word in &all_words {
        if word.x1 > 0 || word.y1 > 0 {
            min_x0 = min_x0.min(word.x0);
            min_y0 = min_y0.min(word.y0);
            max_x1 = max_x1.max(word.x1);
            max_y1 = max_y1.max(word.y1);
        }
        if let Some(c) = word.confidence {
            conf_sum += c;
            conf_count += 1;
        }
        if let Some(fs) = word.font_size {
            font_size_sum += fs;
            font_size_count += 1;
        }
        if let Some(angle) = word.text_angle {
            angle_sum += angle;
            angle_count += 1;
        }
    }

    let has_valid_bbox = max_x1 > 0 || max_y1 > 0;

    let bbox = if has_valid_bbox {
        Some(BoundingBox {
            x0: min_x0 as f64,
            y0: min_y0 as f64,
            x1: max_x1 as f64,
            y1: max_y1 as f64,
        })
    } else {
        None
    };

    let ocr_geometry = if has_valid_bbox {
        Some(OcrBoundingGeometry::Rectangle {
            left: min_x0,
            top: min_y0,
            width: max_x1.saturating_sub(min_x0),
            height: max_y1.saturating_sub(min_y0),
        })
    } else {
        None
    };

    let ocr_confidence = if conf_count > 0 {
        #[cfg(feature = "ocr")]
        {
            Some(OcrConfidence::from_tesseract(conf_sum / conf_count as f64))
        }
        #[cfg(not(feature = "ocr"))]
        {
            Some(OcrConfidence {
                recognition: (conf_sum / conf_count as f64) / 100.0,
                detection: None,
            })
        }
    } else {
        None
    };

    let kind = ElementKind::OcrText {
        level: OcrElementLevel::Block,
    };

    let mut elem = InternalElement::text(kind, text, 0)
        .with_page(page)
        .with_index(element_index);

    elem.bbox = bbox;
    elem.ocr_geometry = ocr_geometry;
    elem.ocr_confidence = ocr_confidence;

    if font_size_count > 0 {
        let avg_font_size = font_size_sum as f64 / font_size_count as f64;
        elem.attributes
            .get_or_insert_with(Default::default)
            .insert(HOCR_FONT_SIZE_ATTRIBUTE.to_string(), avg_font_size.to_string());
    }
    if angle_count > 0 {
        let avg_angle = angle_sum / angle_count as f64;
        if avg_angle.abs() > 0.0 {
            elem.attributes
                .get_or_insert_with(Default::default)
                .insert(HOCR_TEXT_ANGLE_ATTRIBUTE.to_string(), avg_angle.to_string());
        }
    }

    (Some(elem), pos)
}

/// Fast single-byte search (equivalent to `memchr::memchr` but without the dependency).
#[inline]
fn memchr(needle: u8, haystack: &[u8]) -> Option<usize> {
    haystack.iter().position(|&b| b == needle)
}

/// Check if a tag's class attribute contains the given class name.
fn has_class(tag_content: &str, cls: &str) -> bool {
    if let Some(class_start) = tag_content.find("class=") {
        let rest = &tag_content[class_start + 6..];
        let quote = rest.as_bytes().first().copied().unwrap_or(b'"');
        if quote == b'"' || quote == b'\'' {
            let inner = &rest[1..];
            if let Some(end) = inner.find(quote as char) {
                let class_value = &inner[..end];
                return class_value.split_whitespace().any(|c| c == cls);
            }
        }
    }
    false
}

/// Check if tag content opens a paragraph element (`<p class="ocr_par">` or
/// `<span class="ocr_par">` etc.).
fn is_paragraph_tag(tag_content: &str) -> bool {
    has_class(tag_content, "ocr_par")
}

/// Extract the `title="..."` attribute value from raw tag content.
fn extract_title_attr(tag_content: &str) -> String {
    extract_attribute(tag_content, "title").unwrap_or_default()
}

/// Extract a quoted attribute value from raw tag content.
fn extract_attribute(tag_content: &str, attribute: &str) -> Option<String> {
    let marker = format!("{attribute}=");
    if let Some(attribute_start) = tag_content.find(&marker) {
        let rest = &tag_content[attribute_start + marker.len()..];
        let quote = rest.as_bytes().first().copied().unwrap_or(b'"');
        if quote == b'"' || quote == b'\'' {
            let inner = &rest[1..];
            if let Some(end) = inner.find(quote as char) {
                return Some(inner[..end].to_string());
            }
        }
    }
    None
}

/// Extract all text content inside an element, stripping nested tags.
///
/// Walks from `pos` collecting text nodes and descending into nested tags
/// until the matching close tag for the current element is reached.
fn extract_inner_text(html: &str, start: usize) -> String {
    let bytes = html.as_bytes();
    let mut result = String::new();
    let mut pos = start;
    let mut depth: u32 = 1;

    while pos < bytes.len() && depth > 0 {
        if let Some(lt) = memchr(b'<', &bytes[pos..]).map(|i| pos + i) {
            result.push_str(&html[pos..lt]);

            if let Some(gt) = memchr(b'>', &bytes[lt..]).map(|i| lt + i) {
                let tag = &html[lt + 1..gt];
                if tag.starts_with('/') {
                    depth -= 1;
                } else if !tag.ends_with('/') {
                    depth += 1;
                }
                pos = gt + 1;
            } else {
                break;
            }
        } else {
            result.push_str(&html[pos..]);
            break;
        }
    }

    result
}

/// Skip past the matching closing tag for a tag that was just opened.
///
/// `tag_name` is the lowercase name of the opening tag (e.g. "span").
/// Returns the byte position after the closing `>`.
fn skip_to_matching_close(html: &str, start: usize, tag_name: &str) -> usize {
    let bytes = html.as_bytes();
    let mut pos = start;
    let mut depth: u32 = 1;

    while pos < bytes.len() && depth > 0 {
        let Some(lt) = memchr(b'<', &bytes[pos..]).map(|i| pos + i) else {
            break;
        };
        let Some(gt) = memchr(b'>', &bytes[lt..]).map(|i| lt + i) else {
            break;
        };
        let tag = &html[lt + 1..gt];

        if let Some(stripped) = tag.strip_prefix('/') {
            let name = stripped.split_whitespace().next().unwrap_or("");
            if name.eq_ignore_ascii_case(tag_name) {
                depth -= 1;
            }
        } else if !tag.ends_with('/') {
            let name = tag.split_whitespace().next().unwrap_or("");
            if name.eq_ignore_ascii_case(tag_name) {
                depth += 1;
            }
        }

        pos = gt + 1;
    }

    pos
}

/// Decode common HTML entities in text content.
fn decode_html_entities(text: &str) -> String {
    if !text.contains('&') {
        return text.to_string();
    }
    text.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&#x27;", "'")
        .replace("&nbsp;", " ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_hocr() {
        let doc = parse_hocr_to_internal_document("");
        assert!(doc.elements.is_empty());
    }

    #[test]
    fn test_single_page_single_paragraph() {
        let hocr = r#"<div class="ocr_page" title="bbox 0 0 1000 1500; ppageno 0">
            <p class="ocr_par" title="bbox 100 100 900 200">
                <span class="ocr_line" title="bbox 100 100 900 150">
                    <span class="ocrx_word" title="bbox 100 100 200 140; x_wconf 95">Hello</span>
                    <span class="ocrx_word" title="bbox 210 100 350 140; x_wconf 90">World</span>
                </span>
            </p>
        </div>"#;

        let doc = parse_hocr_to_internal_document(hocr);
        let elements = doc.elements;

        assert_eq!(elements.len(), 1);

        let elem = &elements[0];
        assert_eq!(elem.text, "Hello World");
        assert_eq!(elem.page, Some(1));

        let bbox = elem.bbox.as_ref().unwrap();
        assert_eq!(bbox.x0, 100.0);
        assert_eq!(bbox.y0, 100.0);
        assert_eq!(bbox.x1, 350.0);
        assert_eq!(bbox.y1, 140.0);

        let conf = elem.ocr_confidence.as_ref().unwrap();
        assert!((conf.recognition - 0.925).abs() < 0.01);
    }

    #[test]
    fn test_multi_line_paragraph() {
        let hocr = r#"<div class="ocr_page" title="ppageno 0">
            <p class="ocr_par">
                <span class="ocr_line" title="bbox 10 10 200 30">
                    <span class="ocrx_word" title="bbox 10 10 50 30">Line</span>
                    <span class="ocrx_word" title="bbox 60 10 100 30">one</span>
                </span>
                <span class="ocr_line" title="bbox 10 40 200 60">
                    <span class="ocrx_word" title="bbox 10 40 50 60">Line</span>
                    <span class="ocrx_word" title="bbox 60 40 100 60">two</span>
                </span>
            </p>
        </div>"#;

        let doc = parse_hocr_to_internal_document(hocr);
        let elements = doc.elements;
        assert_eq!(elements.len(), 1);
        assert_eq!(elements[0].text, "Line one\nLine two");
    }

    #[test]
    fn test_multi_page_inserts_page_breaks() {
        let hocr = r#"
        <div class="ocr_page" title="ppageno 0">
            <p class="ocr_par">
                <span class="ocrx_word" title="bbox 10 10 50 30">Page1</span>
            </p>
        </div>
        <div class="ocr_page" title="ppageno 1">
            <p class="ocr_par">
                <span class="ocrx_word" title="bbox 10 10 50 30">Page2</span>
            </p>
        </div>"#;

        let doc = parse_hocr_to_internal_document(hocr);
        let elements = doc.elements;

        assert_eq!(elements.len(), 3);
        assert!(matches!(elements[0].kind, ElementKind::OcrText { .. }));
        assert!(matches!(elements[1].kind, ElementKind::PageBreak));
        assert!(matches!(elements[2].kind, ElementKind::OcrText { .. }));
        assert_eq!(elements[0].page, Some(1));
        assert_eq!(elements[2].page, Some(2));
    }

    #[test]
    fn test_html_entity_decoding() {
        let hocr = r#"<div class="ocr_page" title="ppageno 0">
            <p class="ocr_par">
                <span class="ocrx_word" title="bbox 10 10 50 30">&amp;foo&lt;bar&gt;</span>
            </p>
        </div>"#;

        let doc = parse_hocr_to_internal_document(hocr);
        assert_eq!(doc.elements[0].text, "&foo<bar>");
    }

    #[test]
    fn test_words_without_bbox_still_included() {
        let hocr = r#"<div class="ocr_page" title="ppageno 0">
            <p class="ocr_par">
                <span class="ocrx_word">NoBbox</span>
            </p>
        </div>"#;

        let doc = parse_hocr_to_internal_document(hocr);
        assert_eq!(doc.elements.len(), 1);
        assert_eq!(doc.elements[0].text, "NoBbox");
        assert!(doc.elements[0].bbox.is_none());
    }

    #[test]
    fn test_nested_formatting_tags() {
        let hocr = r#"<div class="ocr_page" title="ppageno 0">
            <p class="ocr_par">
                <span class="ocrx_word" title="bbox 10 10 50 30"><strong>Bold</strong></span>
                <span class="ocrx_word" title="bbox 60 10 100 30"><em>Italic</em></span>
            </p>
        </div>"#;

        let doc = parse_hocr_to_internal_document(hocr);
        assert_eq!(doc.elements[0].text, "Bold Italic");
    }

    #[test]
    fn test_property_parsing() {
        let props = parse_title_properties("bbox 100 50 200 150; x_wconf 95.5; ppageno 3; textangle 7.2");
        assert_eq!(props.bbox, Some((100, 50, 200, 150)));
        assert_eq!(props.x_wconf, Some(95.5));
        assert_eq!(props.ppageno, Some(3));
        assert_eq!(props.textangle, Some(7.2));
    }

    #[test]
    fn test_baseline_parsing() {
        let props = parse_title_properties("baseline 0.015 -18");
        assert_eq!(props.baseline, Some((0.015, -18)));
    }

    #[test]
    fn test_font_parsing() {
        let props = parse_title_properties("x_font \"Comic Sans MS\"; x_fsize 12");
        assert_eq!(props.x_font, Some("Comic Sans MS".to_string()));
        assert_eq!(props.x_fsize, Some(12));
    }

    #[test]
    fn test_has_class() {
        assert!(has_class(
            r#"div class="ocr_page" title="bbox 0 0 100 100""#,
            "ocr_page"
        ));
        assert!(!has_class(r#"div class="ocr_page""#, "ocr_par"));
        assert!(has_class(r#"span class="ocrx_word ocr_line""#, "ocrx_word"));
        assert!(has_class(r#"span class="ocrx_word ocr_line""#, "ocr_line"));
    }

    #[test]
    fn test_extract_title_attr() {
        let title = extract_title_attr(r#"div class="ocr_page" title="bbox 0 0 100 200; ppageno 0""#);
        assert_eq!(title, "bbox 0 0 100 200; ppageno 0");
    }

    #[test]
    fn test_paragraph_stores_average_font_size_attribute() {
        let hocr = r#"<div class="ocr_page" title="ppageno 0">
            <p class="ocr_par">
                <span class="ocr_line">
                    <span class="ocrx_word" title="bbox 10 10 50 30; x_wconf 90; x_fsize 24">BIG</span>
                    <span class="ocrx_word" title="bbox 60 10 100 30; x_wconf 90; x_fsize 20">Title</span>
                </span>
            </p>
        </div>"#;

        let doc = parse_hocr_to_internal_document(hocr);
        let attrs = doc.elements[0].attributes.as_ref().expect("attributes present");
        assert_eq!(attrs.get(HOCR_FONT_SIZE_ATTRIBUTE), Some(&"22".to_string()));
    }

    #[test]
    fn test_paragraph_without_font_size_has_no_attribute() {
        let hocr = r#"<div class="ocr_page" title="ppageno 0">
            <p class="ocr_par">
                <span class="ocrx_word" title="bbox 10 10 50 30; x_wconf 90">NoFontSize</span>
            </p>
        </div>"#;

        let doc = parse_hocr_to_internal_document(hocr);
        let has_font_size = doc.elements[0]
            .attributes
            .as_ref()
            .is_some_and(|attrs| attrs.contains_key(HOCR_FONT_SIZE_ATTRIBUTE));
        assert!(
            !has_font_size,
            "should not synthesize a font size when hOCR provides none"
        );
    }

    #[test]
    fn test_paragraph_stores_average_text_angle_attribute() {
        let hocr = r#"<div class="ocr_page" title="ppageno 0">
            <p class="ocr_par">
                <span class="ocr_line">
                    <span class="ocrx_word" title="bbox 10 10 50 30; x_wconf 90; textangle 90">Rotated</span>
                </span>
            </p>
        </div>"#;

        let doc = parse_hocr_to_internal_document(hocr);
        let attrs = doc.elements[0].attributes.as_ref().expect("attributes present");
        assert_eq!(attrs.get(HOCR_TEXT_ANGLE_ATTRIBUTE), Some(&"90".to_string()));
    }

    #[test]
    fn test_ocr_geometry_set() {
        let hocr = r#"<div class="ocr_page" title="ppageno 0">
            <p class="ocr_par">
                <span class="ocrx_word" title="bbox 50 60 150 100; x_wconf 88">test</span>
            </p>
        </div>"#;

        let doc = parse_hocr_to_internal_document(hocr);
        let elem = &doc.elements[0];
        let geom = elem.ocr_geometry.as_ref().unwrap();
        match geom {
            OcrBoundingGeometry::Rectangle {
                left,
                top,
                width,
                height,
            } => {
                assert_eq!(left, &50);
                assert_eq!(top, &60);
                assert_eq!(width, &100);
                assert_eq!(height, &40);
            }
            _ => panic!("Expected Rectangle geometry"),
        }
    }

    #[test]
    fn test_english_pdf_real_data() {
        let hocr = include_str!("../../test_data/hocr/english_pdf_default.hocr");
        let doc = parse_hocr_to_internal_document(hocr);
        assert!(
            !doc.elements.is_empty(),
            "Should extract elements from English PDF hOCR"
        );
        let total_text: String = doc
            .elements
            .iter()
            .map(|e| e.text.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(!total_text.trim().is_empty(), "Should have non-empty text");
        let has_pages = doc.elements.iter().any(|e| e.page.is_some());
        assert!(has_pages, "Should have page numbers");
    }

    #[test]
    fn test_german_pdf_real_data() {
        let hocr = include_str!("../../test_data/hocr/german_pdf_default.hocr");
        let doc = parse_hocr_to_internal_document(hocr);
        assert!(!doc.elements.is_empty(), "Should extract elements from German PDF hOCR");
        let total_text: String = doc
            .elements
            .iter()
            .map(|e| e.text.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(!total_text.trim().is_empty(), "Should have non-empty German text");
    }

    #[test]
    fn test_invoice_image_real_data() {
        let hocr = include_str!("../../test_data/hocr/invoice_image_default.hocr");
        let doc = parse_hocr_to_internal_document(hocr);
        assert!(!doc.elements.is_empty(), "Should extract elements from invoice hOCR");
        let total_text: String = doc
            .elements
            .iter()
            .map(|e| e.text.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(
            total_text.chars().any(|c| c.is_ascii_digit()),
            "Invoice should contain numbers"
        );
    }

    #[test]
    fn test_word_confidence_real_data() {
        let hocr = include_str!("../../test_data/hocr/word_confidence.hocr");
        let doc = parse_hocr_to_internal_document(hocr);
        assert!(
            doc.elements.is_empty(),
            "Non-hOCR-classed elements should not be extracted"
        );
    }

    #[test]
    fn test_utf8_encoding_real_data() {
        let hocr = include_str!("../../test_data/hocr/utf8_encoding.hocr");
        let doc = parse_hocr_to_internal_document(hocr);
        assert!(
            doc.elements.is_empty(),
            "Non-hOCR-classed UTF-8 content should not be extracted"
        );
    }

    #[test]
    fn test_v4_with_tables_and_code() {
        let hocr = include_str!("../../test_data/hocr/v4_code_formula.hocr");
        let doc = parse_hocr_to_internal_document(hocr);
        assert!(
            !doc.elements.is_empty(),
            "Should extract from v4 hOCR with code/formula"
        );
    }

    #[test]
    fn test_v4_embedded_tables() {
        let hocr = include_str!("../../test_data/hocr/v4_embedded_tables.hocr");
        let doc = parse_hocr_to_internal_document(hocr);
        assert!(
            !doc.elements.is_empty(),
            "Should extract from v4 hOCR with embedded tables"
        );
    }

    #[test]
    fn test_many_paragraphs_all_captured() {
        let paragraph_texts: Vec<&str> = vec![
            "First paragraph",
            "Second paragraph",
            "Third paragraph",
            "Fourth paragraph",
            "Fifth paragraph",
            "Sixth paragraph",
            "Seventh paragraph",
            "Eighth paragraph",
            "Ninth paragraph",
            "Tenth paragraph",
            "Eleventh paragraph",
            "Twelfth paragraph",
            "Thirteenth paragraph",
            "Fourteenth paragraph",
            "Fifteenth paragraph",
            "Sixteenth paragraph",
            "Seventeenth paragraph",
            "Eighteenth paragraph",
            "Nineteenth paragraph",
            "Twentieth paragraph",
            "Twenty-first paragraph",
            "Twenty-second paragraph",
            "Twenty-third paragraph",
            "Twenty-fourth paragraph",
            "Twenty-fifth paragraph",
            "Service category alpha",
            "Service category beta",
            "Service category gamma",
            "Service category delta",
            "All other categories",
            "Items provided by client",
            "*** Note this is the last paragraph",
        ];

        let mut hocr = String::from(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE html PUBLIC "-//W3C//DTD XHTML 1.0 Transitional//EN"
    "http://www.w3.org/TR/xhtml1/DTD/xhtml1-transitional.dtd">
<html xmlns="http://www.w3.org/1999/xhtml" xml:lang="en" lang="en">
 <head>
  <title></title>
  <meta http-equiv="Content-Type" content="text/html;charset=utf-8"/>
  <meta name='ocr-system' content='tesseract 5.5.1' />
 </head>
 <body>
  <div class='ocr_page' id='page_1' title='image "test.png"; bbox 0 0 2550 3300; ppageno 0; scan_res 300 300'>
"#,
        );

        let mut y = 100;
        for (i, text) in paragraph_texts.iter().enumerate() {
            let block_id = i + 1;
            let par_id = i + 1;
            let line_id = i + 1;
            let y0 = y;
            let y1 = y + 30;

            hocr.push_str(&format!(
                r#"   <div class='ocr_carea' id='block_1_{block_id}' title="bbox 100 {y0} 2400 {y1}">
    <p class='ocr_par' id='par_1_{par_id}' lang='eng' title="bbox 100 {y0} 2400 {y1}">
     <span class='ocr_line' id='line_1_{line_id}' title="bbox 100 {y0} 2400 {y1}; baseline 0 0; x_size 30; x_descenders 6; x_ascenders 8">
"#
            ));

            let mut wx = 100;
            for (wi, word) in text.split_whitespace().enumerate() {
                let word_id = i * 10 + wi + 1;
                let wx1 = wx + word.len() as u32 * 20;
                hocr.push_str(&format!(
                    "      <span class='ocrx_word' id='word_1_{word_id}' title='bbox {wx} {y0} {wx1} {y1}; x_wconf 90'>{word}</span>\n"
                ));
                wx = wx1 + 10;
            }

            hocr.push_str("     </span>\n    </p>\n   </div>\n");

            y = y1 + 10;
        }

        hocr.push_str("  </div>\n </body>\n</html>\n");

        let doc = parse_hocr_to_internal_document(&hocr);

        let text_elements: Vec<_> = doc
            .elements
            .iter()
            .filter(|e| matches!(e.kind, ElementKind::OcrText { .. }))
            .collect();

        assert_eq!(
            text_elements.len(),
            paragraph_texts.len(),
            "Expected {} paragraphs but got {}. Missing paragraphs from the end.",
            paragraph_texts.len(),
            text_elements.len()
        );

        for (i, (elem, expected)) in text_elements.iter().zip(paragraph_texts.iter()).enumerate() {
            assert_eq!(
                elem.text,
                *expected,
                "Paragraph {} mismatch: expected '{}', got '{}'",
                i + 1,
                expected,
                elem.text
            );
        }

        let last_text = &text_elements.last().unwrap().text;
        assert_eq!(
            last_text, "*** Note this is the last paragraph",
            "Last paragraph should be captured"
        );
    }

    #[test]
    fn test_paragraph_with_nested_span_in_word() {
        let hocr = r#"<div class="ocr_page" title="ppageno 0">
  <div class="ocr_carea">
    <p class="ocr_par">
      <span class="ocr_line">
        <span class="ocrx_word" title="bbox 10 10 50 30; x_wconf 90"><span class="ocrx_font" style="font-size:12px">Hello</span></span>
        <span class="ocrx_word" title="bbox 60 10 100 30; x_wconf 90">World</span>
      </span>
    </p>
  </div>
  <div class="ocr_carea">
    <p class="ocr_par">
      <span class="ocr_line">
        <span class="ocrx_word" title="bbox 10 50 80 70; x_wconf 90">Second</span>
        <span class="ocrx_word" title="bbox 90 50 180 70; x_wconf 90">paragraph</span>
      </span>
    </p>
  </div>
  <div class="ocr_carea">
    <p class="ocr_par">
      <span class="ocr_line">
        <span class="ocrx_word" title="bbox 10 90 80 110; x_wconf 90">Third</span>
        <span class="ocrx_word" title="bbox 90 90 180 110; x_wconf 90">paragraph</span>
      </span>
    </p>
  </div>
</div>"#;

        let doc = parse_hocr_to_internal_document(hocr);
        let text_elements: Vec<_> = doc
            .elements
            .iter()
            .filter(|e| matches!(e.kind, ElementKind::OcrText { .. }))
            .collect();

        assert_eq!(text_elements.len(), 3, "Should capture all 3 paragraphs");
        assert_eq!(text_elements[0].text, "Hello World");
        assert_eq!(text_elements[1].text, "Second paragraph");
        assert_eq!(text_elements[2].text, "Third paragraph");
    }

    #[test]
    fn test_paragraph_with_words_outside_line() {
        let hocr = r#"<div class="ocr_page" title="ppageno 0">
  <div class="ocr_carea">
    <p class="ocr_par">
      <span class="ocrx_word" title="bbox 10 10 50 30; x_wconf 90">Direct</span>
      <span class="ocrx_word" title="bbox 60 10 120 30; x_wconf 90">words</span>
    </p>
  </div>
  <div class="ocr_carea">
    <p class="ocr_par">
      <span class="ocr_line">
        <span class="ocrx_word" title="bbox 10 50 80 70; x_wconf 90">Next</span>
        <span class="ocrx_word" title="bbox 90 50 160 70; x_wconf 90">paragraph</span>
      </span>
    </p>
  </div>
</div>"#;

        let doc = parse_hocr_to_internal_document(hocr);
        let text_elements: Vec<_> = doc
            .elements
            .iter()
            .filter(|e| matches!(e.kind, ElementKind::OcrText { .. }))
            .collect();

        assert_eq!(text_elements.len(), 2, "Should capture both paragraphs");
        assert_eq!(text_elements[0].text, "Direct words");
        assert_eq!(text_elements[1].text, "Next paragraph");
    }

    #[test]
    fn test_paragraph_depth_with_extra_div_nesting() {
        let hocr = r#"<div class="ocr_page" title="ppageno 0">
  <div class="ocr_carea">
    <p class="ocr_par">
      <div class="ocr_column">
        <span class="ocr_line">
          <span class="ocrx_word" title="bbox 10 10 50 30; x_wconf 90">Nested</span>
        </span>
      </div>
    </p>
  </div>
  <div class="ocr_carea">
    <p class="ocr_par">
      <span class="ocr_line">
        <span class="ocrx_word" title="bbox 10 50 80 70; x_wconf 90">After</span>
        <span class="ocrx_word" title="bbox 90 50 160 70; x_wconf 90">nested</span>
      </span>
    </p>
  </div>
</div>"#;

        let doc = parse_hocr_to_internal_document(hocr);
        let text_elements: Vec<_> = doc
            .elements
            .iter()
            .filter(|e| matches!(e.kind, ElementKind::OcrText { .. }))
            .collect();

        assert_eq!(
            text_elements.len(),
            2,
            "Should capture both paragraphs even with extra div nesting"
        );
        assert_eq!(text_elements[0].text, "Nested");
        assert_eq!(text_elements[1].text, "After nested");
    }

    #[test]
    fn test_paragraph_div_swallows_carea_close() {
        let hocr = r#"<div class="ocr_page" title="ppageno 0">
  <div class="ocr_carea">
    <div class="ocr_par">
      <span class="ocr_line">
        <span class="ocrx_word" title="bbox 10 10 50 30; x_wconf 90">First</span>
      </span>
    </div>
  </div>
  <div class="ocr_carea">
    <div class="ocr_par">
      <span class="ocr_line">
        <span class="ocrx_word" title="bbox 10 50 50 70; x_wconf 90">Second</span>
      </span>
    </div>
  </div>
  <div class="ocr_carea">
    <div class="ocr_par">
      <span class="ocr_line">
        <span class="ocrx_word" title="bbox 10 90 50 110; x_wconf 90">Third</span>
      </span>
    </div>
  </div>
</div>"#;

        let doc = parse_hocr_to_internal_document(hocr);
        let text_elements: Vec<_> = doc
            .elements
            .iter()
            .filter(|e| matches!(e.kind, ElementKind::OcrText { .. }))
            .collect();

        assert_eq!(text_elements.len(), 3, "Should capture all 3 div-based paragraphs");
    }

    #[test]
    fn test_paragraph_unclosed_par_div_steals_carea_close() {
        let hocr = r#"<div class="ocr_page" title="ppageno 0">
  <div class="ocr_carea">
    <div class="ocr_par">
      <span class="ocr_line">
        <span class="ocrx_word" title="bbox 10 10 50 30; x_wconf 90">First</span>
      </span>
  </div>
  <div class="ocr_carea">
    <div class="ocr_par">
      <span class="ocr_line">
        <span class="ocrx_word" title="bbox 10 50 50 70; x_wconf 90">Second</span>
      </span>
  </div>
</div>"#;

        let doc = parse_hocr_to_internal_document(hocr);
        let text_elements: Vec<_> = doc
            .elements
            .iter()
            .filter(|e| matches!(e.kind, ElementKind::OcrText { .. }))
            .collect();

        assert_eq!(
            text_elements.len(),
            2,
            "Should find both paragraphs even with unclosed par divs. Got: {:?}",
            text_elements.iter().map(|e| e.text.as_str()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_depth_tracking_uses_paragraph_tag_name() {
        let hocr_separate = r#"<div class="ocr_page" title="ppageno 0">
  <div class="ocr_carea">
    <p class="ocr_par">
      <span class="ocr_line">
        <span class="ocrx_word" title="bbox 10 10 50 30; x_wconf 90"><span>Styled</span></span>
        <span class="ocrx_word" title="bbox 60 10 120 30; x_wconf 90">text</span>
      </span>
    </p>
  </div>
  <div class="ocr_carea">
    <p class="ocr_par">
      <span class="ocr_line">
        <span class="ocrx_word" title="bbox 10 50 80 70; x_wconf 90">After</span>
      </span>
    </p>
  </div>
</div>"#;

        let doc = parse_hocr_to_internal_document(hocr_separate);
        let text_elements: Vec<_> = doc
            .elements
            .iter()
            .filter(|e| matches!(e.kind, ElementKind::OcrText { .. }))
            .collect();
        assert_eq!(text_elements.len(), 2);
        assert_eq!(text_elements[0].text, "Styled text");
        assert_eq!(text_elements[1].text, "After");

        let hocr_same_carea = r#"<div class="ocr_page" title="ppageno 0">
  <div class="ocr_carea">
    <p class="ocr_par">
      <span class="ocr_line">
        <span class="ocrx_word" title="bbox 10 10 50 30; x_wconf 90"><span>Styled</span></span>
      </span>
    </p>
    <p class="ocr_par">
      <span class="ocr_line">
        <span class="ocrx_word" title="bbox 10 50 80 70; x_wconf 90">Should</span>
        <span class="ocrx_word" title="bbox 90 50 180 70; x_wconf 90">be</span>
        <span class="ocrx_word" title="bbox 190 50 280 70; x_wconf 90">separate</span>
      </span>
    </p>
  </div>
</div>"#;

        let doc = parse_hocr_to_internal_document(hocr_same_carea);
        let text_elements: Vec<_> = doc
            .elements
            .iter()
            .filter(|e| matches!(e.kind, ElementKind::OcrText { .. }))
            .collect();
        assert_eq!(
            text_elements.len(),
            2,
            "Should find both paragraphs separately. Got: {:?}",
            text_elements.iter().map(|e| e.text.as_str()).collect::<Vec<_>>()
        );
        assert_eq!(text_elements[0].text, "Styled");
        assert_eq!(text_elements[1].text, "Should be separate");
    }

    #[test]
    fn test_paragraphs_retain_enclosing_hocr_block_id() {
        let hocr = r#"<div class="ocr_page" title="ppageno 0">
  <div class="ocr_carea" id="block_1_1">
    <div class="nested"><p class="ocr_par"><span class="ocrx_word">First</span></p></div>
    <p class="ocr_par"><span class="ocrx_word">Second</span></p>
  </div>
  <div class="ocr_carea" id="block_1_2">
    <p class="ocr_par"><span class="ocrx_word">Third</span></p>
  </div>
</div>"#;

        let doc = parse_hocr_to_internal_document(hocr);
        let block_ids = doc
            .elements
            .iter()
            .filter(|element| matches!(element.kind, ElementKind::OcrText { .. }))
            .map(|element| {
                element
                    .attributes
                    .as_ref()
                    .and_then(|attributes| attributes.get(HOCR_BLOCK_ID_ATTRIBUTE))
                    .map(String::as_str)
            })
            .collect::<Vec<_>>();

        assert_eq!(block_ids, vec![Some("block_1_1"), Some("block_1_1"), Some("block_1_2")]);
    }

    #[test]
    fn test_paragraph_with_ocr_separator_between_paragraphs() {
        let hocr = r#"<div class="ocr_page" title="ppageno 0">
  <div class="ocr_carea">
    <p class="ocr_par">
      <span class="ocr_line">
        <span class="ocrx_word" title="bbox 10 10 50 30; x_wconf 90">Before</span>
      </span>
    </p>
  </div>
  <div class="ocr_separator" title="bbox 10 40 500 42"></div>
  <div class="ocr_carea">
    <p class="ocr_par">
      <span class="ocr_line">
        <span class="ocrx_word" title="bbox 10 50 50 70; x_wconf 90">After</span>
      </span>
    </p>
  </div>
</div>"#;

        let doc = parse_hocr_to_internal_document(hocr);
        let text_elements: Vec<_> = doc
            .elements
            .iter()
            .filter(|e| matches!(e.kind, ElementKind::OcrText { .. }))
            .collect();

        assert_eq!(
            text_elements.len(),
            2,
            "Should capture both paragraphs around separator"
        );
    }
}
