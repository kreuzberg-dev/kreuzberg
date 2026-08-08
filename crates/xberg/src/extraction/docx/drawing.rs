//! DOCX drawing object parsing.
//!
//! This module handles extraction and parsing of drawing objects (`<w:drawing>`)
//! from DOCX documents. Drawing objects can be inline or anchored and may contain
//! images or shapes.

use crate::extractors::security::{SecurityBudget, SecurityError};
use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};
use serde::{Deserialize, Serialize};

/// A drawing object extracted from `<w:drawing>`.
#[cfg_attr(alef, alef(skip))]
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Drawing {
    /// Whether the drawing is inline (in text flow) or anchored (floating).
    pub drawing_type: DrawingType,
    /// Physical dimensions in EMUs (English Metric Units).
    pub extent: Option<Extent>,
    /// Document properties such as ID, name, and alt-text description.
    pub doc_properties: Option<DocProperties>,
    /// Relationship ID (`r:embed`) referencing the image part in the DOCX package.
    pub image_ref: Option<String>,
    /// Text extracted from a text box hosted by this drawing (#81): either the
    /// DrawingML `wps:txbx/w:txbxContent` path, or the VML `v:textbox/w:txbxContent`
    /// fallback path parsed via [`parse_vml_pict`].
    pub text_box_content: Option<String>,
}

/// Whether the drawing is inline or anchored.
#[cfg_attr(alef, alef(skip))]
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub enum DrawingType {
    /// Drawing is inline: placed within the text flow at its insertion point.
    #[default]
    Inline,
    /// Drawing is anchored: floats at a fixed position relative to the page or paragraph.
    Anchored(AnchorProperties),
}

/// Size in EMUs (English Metric Units, 1 inch = 914400 EMU).
#[cfg_attr(alef, alef(skip))]
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Extent {
    /// Width in EMU.
    pub cx: i64,
    /// Height in EMU.
    pub cy: i64,
}

impl Extent {
    /// Convert width to inches.
    pub(crate) fn width_inches(&self) -> f64 {
        self.cx as f64 / super::EMUS_PER_INCH as f64
    }

    /// Convert height to inches.
    pub(crate) fn height_inches(&self) -> f64 {
        self.cy as f64 / super::EMUS_PER_INCH as f64
    }
}

/// Document properties from `<wp:docPr>`.
#[cfg_attr(alef, alef(skip))]
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct DocProperties {
    /// Unique numeric identifier for this drawing within the document.
    pub id: Option<String>,
    /// Human-readable name for this drawing object.
    pub name: Option<String>,
    /// Alt-text description for accessibility.
    pub description: Option<String>,
}

/// Properties for anchored drawings.
#[cfg_attr(alef, alef(skip))]
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct AnchorProperties {
    /// Whether the drawing is placed behind the document text.
    pub behind_doc: bool,
    /// Whether the drawing is laid out inside a table cell.
    pub layout_in_cell: bool,
    /// Z-order relative height used for stacking overlapping objects.
    pub relative_height: Option<i64>,
    /// Horizontal position specification.
    pub position_h: Option<Position>,
    /// Vertical position specification.
    pub position_v: Option<Position>,
    /// Text-wrapping mode around this drawing.
    pub wrap_type: WrapType,
}

/// Horizontal or vertical position.
#[cfg_attr(alef, alef(skip))]
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Position {
    /// Reference object for this position: `"page"`, `"margin"`, `"column"`, `"paragraph"`, or `"character"`.
    pub relative_from: String,
    /// Offset from the reference object in EMUs.
    pub offset: Option<i64>,
}

/// Text wrapping type around an anchored drawing.
#[cfg_attr(alef, alef(skip))]
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub enum WrapType {
    /// No text wrapping; drawing floats above or below text.
    #[default]
    None,
    /// Text wraps in a square around the drawing bounding box.
    Square,
    /// Text wraps tightly to the drawing outline.
    Tight,
    /// Text appears above and below but not to the sides.
    TopAndBottom,
    /// Text flows through the drawing's transparent areas.
    Through,
}

/// Parse a drawing object starting after the `<w:drawing>` Start event.
///
/// This function reads events until it encounters the closing `</w:drawing>` tag,
/// parsing the drawing type (inline or anchored), extent, properties, and image references.
///
/// Threads `budget` through every event so nesting inside `w:drawing` is measured
/// against the caller's depth cap instead of passing through unaccounted (GH#384).
/// The local `depth` counter below is a separate, pre-existing mechanism: it tracks
/// same-named nesting so this function can find its *own* matching `</w:drawing>`
/// end tag, and is unrelated to `budget`'s document-wide depth accounting.
pub(crate) fn parse_drawing(reader: &mut Reader<&[u8]>, budget: &mut SecurityBudget) -> Result<Drawing, SecurityError> {
    let mut drawing = Drawing {
        drawing_type: DrawingType::Inline,
        extent: None,
        doc_properties: None,
        image_ref: None,
        text_box_content: None,
    };

    let mut depth = 1;
    let mut buf = Vec::new();

    loop {
        budget.step()?;
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                budget.enter()?;
                let local = e.local_name();
                let local_name = local.as_ref();

                match local_name {
                    b"inline" => {
                        drawing.drawing_type = DrawingType::Inline;
                        depth += 1;
                    }
                    b"anchor" => {
                        let anchor = AnchorProperties {
                            behind_doc: get_attr_bool(e, b"behindDoc"),
                            layout_in_cell: get_attr_bool(e, b"layoutInCell"),
                            relative_height: get_attr_i64(e, b"relativeHeight"),
                            ..Default::default()
                        };
                        drawing.drawing_type = DrawingType::Anchored(anchor);
                        depth += 1;
                    }
                    b"positionH" => {
                        let relative_from = get_attr(e, b"relativeFrom").unwrap_or_else(|| "page".to_string());
                        let position = parse_position(reader, "positionH");
                        // `parse_position` reads through its own `</wp:positionH>`
                        // without touching `budget`; refund the enter above.
                        budget.leave();
                        if let DrawingType::Anchored(ref mut anchor) = drawing.drawing_type {
                            anchor.position_h = Some(Position {
                                relative_from,
                                offset: position,
                            });
                        }
                    }
                    b"positionV" => {
                        let relative_from = get_attr(e, b"relativeFrom").unwrap_or_else(|| "paragraph".to_string());
                        let position = parse_position(reader, "positionV");
                        // Same as `positionH`: consumes its own end tag.
                        budget.leave();
                        if let DrawingType::Anchored(ref mut anchor) = drawing.drawing_type {
                            anchor.position_v = Some(Position {
                                relative_from,
                                offset: position,
                            });
                        }
                    }
                    b"blip" => {
                        if drawing.image_ref.is_none() {
                            drawing.image_ref = get_attr(e, b"embed").or_else(|| get_attr(e, b"link"));
                        }
                        depth += 1;
                    }
                    b"txbxContent" => {
                        // Consumes through its own `</w:txbxContent>` end tag, so it
                        // must not also increment `depth` (#81). `collect_txbx_content_text`
                        // now threads `budget` through and balances the `enter()` above
                        // internally, so no manual `budget.leave()` is needed here.
                        let text = collect_txbx_content_text(reader, budget)?;
                        if !text.is_empty() {
                            drawing.text_box_content = Some(text);
                        }
                    }
                    b"wrapSquare" | b"wrapTight" | b"wrapTopAndBottom" | b"wrapThrough" => {
                        if let DrawingType::Anchored(ref mut anchor) = drawing.drawing_type {
                            match local_name {
                                b"wrapSquare" => anchor.wrap_type = WrapType::Square,
                                b"wrapTight" => anchor.wrap_type = WrapType::Tight,
                                b"wrapTopAndBottom" => anchor.wrap_type = WrapType::TopAndBottom,
                                b"wrapThrough" => anchor.wrap_type = WrapType::Through,
                                _ => {}
                            }
                        }
                        depth += 1;
                    }
                    _ => {
                        depth += 1;
                    }
                }
            }
            Ok(Event::Empty(ref e)) => {
                let local = e.local_name();
                let local_name = local.as_ref();

                match local_name {
                    b"extent" => {
                        if let (Some(cx), Some(cy)) = (get_attr_i64(e, b"cx"), get_attr_i64(e, b"cy")) {
                            drawing.extent = Some(Extent { cx, cy });
                        }
                    }
                    b"docPr" => {
                        drawing.doc_properties = Some(DocProperties {
                            id: get_attr(e, b"id"),
                            name: get_attr(e, b"name"),
                            description: get_attr(e, b"descr"),
                        });
                    }
                    b"blip" if drawing.image_ref.is_none() => {
                        drawing.image_ref = get_attr(e, b"embed").or_else(|| get_attr(e, b"link"));
                    }
                    b"wrapNone" => {
                        if let DrawingType::Anchored(ref mut anchor) = drawing.drawing_type {
                            anchor.wrap_type = WrapType::None;
                        }
                    }
                    b"wrapSquare" => {
                        if let DrawingType::Anchored(ref mut anchor) = drawing.drawing_type {
                            anchor.wrap_type = WrapType::Square;
                        }
                    }
                    b"wrapTight" => {
                        if let DrawingType::Anchored(ref mut anchor) = drawing.drawing_type {
                            anchor.wrap_type = WrapType::Tight;
                        }
                    }
                    b"wrapTopAndBottom" => {
                        if let DrawingType::Anchored(ref mut anchor) = drawing.drawing_type {
                            anchor.wrap_type = WrapType::TopAndBottom;
                        }
                    }
                    b"wrapThrough" => {
                        if let DrawingType::Anchored(ref mut anchor) = drawing.drawing_type {
                            anchor.wrap_type = WrapType::Through;
                        }
                    }
                    _ => {}
                }
            }
            Ok(Event::End(e)) => {
                budget.leave();
                depth -= 1;
                if e.local_name().as_ref() as &[u8] == b"drawing" && depth == 0 {
                    break;
                }
            }
            Ok(Event::Eof) => {
                break;
            }
            Err(_) => {
                break;
            }
            _ => {}
        }
        buf.clear();
    }

    Ok(drawing)
}

/// Parse position offset from positionH or positionV element.
/// Consumes all events through the closing element_name end tag.
fn parse_position(reader: &mut Reader<&[u8]>, element_name: &str) -> Option<i64> {
    let mut buf = Vec::new();
    let element_bytes = element_name.as_bytes();
    let mut result = None;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) if e.local_name().as_ref() as &[u8] == b"posOffset" => {
                let mut text_buf = Vec::new();
                if let Ok(Event::Text(t)) = reader.read_event_into(&mut text_buf)
                    && let Ok(text) = t.decode()
                {
                    result = text.parse::<i64>().ok();
                }
                let mut end_buf = Vec::new();
                let _ = reader.read_event_into(&mut end_buf);
            }
            Ok(Event::End(e)) if e.local_name().as_ref() as &[u8] == element_bytes => {
                return result;
            }
            Ok(Event::Eof) => {
                return result;
            }
            Err(_) => {
                return result;
            }
            _ => {}
        }
        buf.clear();
    }
}

/// Extract a string attribute by local name.
fn get_attr(e: &BytesStart, key: &[u8]) -> Option<String> {
    e.attributes()
        .flatten()
        .find(|attr| attr.key.local_name().as_ref() as &[u8] == key)
        .and_then(|attr| {
            let raw = std::str::from_utf8(&attr.value).ok()?;
            quick_xml::escape::unescape(raw).ok().map(|s| s.into_owned())
        })
}

/// Extract an i64 attribute by local name.
fn get_attr_i64(e: &BytesStart, key: &[u8]) -> Option<i64> {
    get_attr(e, key).and_then(|s| s.parse().ok())
}

/// Extract a boolean attribute by local name (value "1" = true).
fn get_attr_bool(e: &BytesStart, key: &[u8]) -> bool {
    get_attr(e, key).as_deref() == Some("1")
}

/// Collect visible text from a `<w:txbxContent>` subtree (#81): the paragraphs of a
/// text box, reached either via the DrawingML `wps:txbx` path (from [`parse_drawing`])
/// or the VML `v:textbox` fallback path (from [`parse_vml_pict`]).
///
/// Called with the reader positioned right after the `<w:txbxContent>` start tag;
/// consumes events through the matching `</w:txbxContent>` end tag. Paragraphs are
/// joined with newlines; `w:tab`/`w:br` become `\t`/`\n` within a paragraph, matching
/// how the main body loop renders inline breaks.
///
/// Threads `budget` through every event so nesting and iteration count inside
/// `w:txbxContent` are measured against the caller's caps instead of passing through
/// unaccounted (GH#1395/#384). The caller already performed `budget.enter()` for the
/// opening `<w:txbxContent>` tag; this function balances that when it reaches its own
/// matching `</w:txbxContent>` (depth 0).
fn collect_txbx_content_text(reader: &mut Reader<&[u8]>, budget: &mut SecurityBudget) -> Result<String, SecurityError> {
    let mut buf = Vec::new();
    let mut depth = 1u32;
    let mut paragraphs: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut in_text = false;

    loop {
        budget.step()?;
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                budget.enter()?;
                match e.local_name().as_ref() {
                    b"txbxContent" => depth += 1,
                    b"t" => in_text = true,
                    _ => {}
                }
            }
            Ok(Event::Empty(ref e)) => match e.local_name().as_ref() {
                b"tab" => current.push('\t'),
                b"br" => current.push('\n'),
                _ => {}
            },
            Ok(Event::Text(e)) => {
                if in_text && let Ok(text) = e.decode() {
                    current.push_str(&text);
                }
            }
            Ok(Event::End(ref e)) => {
                budget.leave();
                match e.local_name().as_ref() {
                    b"t" => in_text = false,
                    b"p" => paragraphs.push(std::mem::take(&mut current)),
                    b"txbxContent" => {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    _ => {}
                }
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
        buf.clear();
    }

    if !current.is_empty() {
        paragraphs.push(current);
    }

    Ok(paragraphs
        .into_iter()
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("\n"))
}

/// Parse a `<v:textbox>` element (already open), looking for a nested
/// `<w:txbxContent>` (#81). Consumes events through the matching `</v:textbox>` end
/// tag regardless of whether a `w:txbxContent` was found.
///
/// Threads `budget` through every event so nesting and iteration count inside
/// `v:textbox` are measured against the caller's caps instead of passing through
/// unaccounted (GH#1395/#384). The caller already performed `budget.enter()` for the
/// opening `<v:textbox>` tag; this function balances that when it reaches its own
/// matching `</v:textbox>` (depth 0).
fn parse_vml_textbox(reader: &mut Reader<&[u8]>, budget: &mut SecurityBudget) -> Result<Option<String>, SecurityError> {
    let mut buf = Vec::new();
    let mut depth = 1u32;
    let mut text: Option<String> = None;

    loop {
        budget.step()?;
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                budget.enter()?;
                if e.local_name().as_ref() == b"txbxContent" {
                    // `collect_txbx_content_text` consumes its own end tag and
                    // balances the `enter()` above internally, so no manual
                    // `budget.leave()` is needed here.
                    let collected = collect_txbx_content_text(reader, budget)?;
                    if !collected.is_empty() {
                        text = Some(collected);
                    }
                } else {
                    depth += 1;
                }
            }
            Ok(Event::End(_)) => {
                budget.leave();
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
        buf.clear();
    }

    Ok(text)
}

/// Parse a `<w:pict>` VML fallback wrapper, extracting text-box content from a
/// nested `<v:textbox><w:txbxContent>` if present (#81, #224).
///
/// Consumes events through the matching `</w:pict>` end tag regardless of whether a
/// text box was found, so the caller's own event loop never sees `w:pict`'s inner
/// `v:shape`/`w:p`/`w:r`/`w:t` events leak out as if they were ordinary body content.
/// Returns `Ok(None)` when no text box was found (nothing to attach to the document).
///
/// Threads `budget` through every event so nesting and iteration count inside
/// `w:pict` are measured against the caller's caps instead of passing through
/// unaccounted (a25335db0a left this delegate unthreaded, unlike every other
/// budget-aware delegate in this module; see GH#1395/#384). The caller already
/// performed `budget.enter()` for the opening `<w:pict>` tag; this function balances
/// that when it reaches its own matching `</w:pict>` (depth 0).
pub(crate) fn parse_vml_pict(
    reader: &mut Reader<&[u8]>,
    budget: &mut SecurityBudget,
) -> Result<Option<Drawing>, SecurityError> {
    let mut buf = Vec::new();
    let mut depth = 1u32;
    let mut text_box_content: Option<String> = None;

    loop {
        budget.step()?;
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                budget.enter()?;
                if e.local_name().as_ref() == b"textbox" {
                    // `parse_vml_textbox` consumes its own end tag and balances
                    // the `enter()` above internally, so no manual
                    // `budget.leave()` is needed here.
                    text_box_content = parse_vml_textbox(reader, budget)?;
                } else {
                    depth += 1;
                }
            }
            Ok(Event::End(_)) => {
                budget.leave();
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
        buf.clear();
    }

    Ok(text_box_content.map(|text| Drawing {
        text_box_content: Some(text),
        ..Default::default()
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper to parse drawing XML and return the Drawing object.
    fn parse_drawing_from_xml(xml: &[u8]) -> Drawing {
        let mut reader = Reader::from_reader(xml);
        let mut buf = Vec::new();

        loop {
            match reader.read_event_into(&mut buf) {
                Ok(Event::Start(e)) if e.local_name().as_ref() as &[u8] == b"drawing" => {
                    break;
                }
                Ok(Event::Eof) => {
                    return Drawing {
                        drawing_type: DrawingType::Inline,
                        extent: None,
                        doc_properties: None,
                        image_ref: None,
                        text_box_content: None,
                    };
                }
                Err(_) => {
                    return Drawing {
                        drawing_type: DrawingType::Inline,
                        extent: None,
                        doc_properties: None,
                        image_ref: None,
                        text_box_content: None,
                    };
                }
                _ => {}
            }
            buf.clear();
        }

        let mut budget = SecurityBudget::with_defaults();
        parse_drawing(&mut reader, &mut budget).expect("parse_drawing should not exceed the default budget")
    }

    #[test]
    fn test_parse_inline_drawing() {
        let xml = br#"<w:drawing xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"
                        xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing"
                        xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"
                        xmlns:pic="http://schemas.openxmlformats.org/drawingml/2006/picture"
                        xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
          <wp:inline>
            <wp:extent cx="914400" cy="457200"/>
            <wp:docPr id="1" name="Picture 1" descr="A test image"/>
            <a:graphic>
              <a:graphicData>
                <pic:pic>
                  <pic:blipFill>
                    <a:blip r:embed="rId5"/>
                  </pic:blipFill>
                </pic:pic>
              </a:graphicData>
            </a:graphic>
          </wp:inline>
        </w:drawing>"#;

        let drawing = parse_drawing_from_xml(xml);

        assert_eq!(drawing.drawing_type, DrawingType::Inline);
        assert_eq!(drawing.extent, Some(Extent { cx: 914400, cy: 457200 }));
        assert_eq!(
            drawing.doc_properties,
            Some(DocProperties {
                id: Some("1".to_string()),
                name: Some("Picture 1".to_string()),
                description: Some("A test image".to_string()),
            })
        );
        assert_eq!(drawing.image_ref, Some("rId5".to_string()));
    }

    #[test]
    fn test_parse_anchored_drawing() {
        let xml = br#"<w:drawing xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"
                        xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing"
                        xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"
                        xmlns:pic="http://schemas.openxmlformats.org/drawingml/2006/picture"
                        xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
          <wp:anchor behindDoc="0" layoutInCell="1" relativeHeight="251573248">
            <wp:positionH relativeFrom="page">
              <wp:posOffset>621792</wp:posOffset>
            </wp:positionH>
            <wp:positionV relativeFrom="paragraph">
              <wp:posOffset>274320</wp:posOffset>
            </wp:positionV>
            <wp:extent cx="209550" cy="209550"/>
            <wp:wrapSquare/>
            <wp:docPr id="2" name="Picture 2"/>
            <a:graphic>
              <a:graphicData>
                <pic:pic>
                  <pic:blipFill>
                    <a:blip r:embed="rId6"/>
                  </pic:blipFill>
                </pic:pic>
              </a:graphicData>
            </a:graphic>
          </wp:anchor>
        </w:drawing>"#;

        let drawing = parse_drawing_from_xml(xml);

        match drawing.drawing_type {
            DrawingType::Anchored(anchor) => {
                assert!(!anchor.behind_doc);
                assert!(anchor.layout_in_cell);
                assert_eq!(anchor.relative_height, Some(251573248));
                assert_eq!(anchor.wrap_type, WrapType::Square);

                assert!(anchor.position_h.is_some());
                if let Some(pos_h) = anchor.position_h {
                    assert_eq!(pos_h.relative_from, "page");
                    assert_eq!(pos_h.offset, Some(621792));
                }

                assert!(anchor.position_v.is_some());
                if let Some(pos_v) = anchor.position_v {
                    assert_eq!(pos_v.relative_from, "paragraph");
                    assert_eq!(pos_v.offset, Some(274320));
                }
            }
            _ => panic!("Expected DrawingType::Anchored"),
        }

        assert_eq!(drawing.extent, Some(Extent { cx: 209550, cy: 209550 }));
        assert_eq!(drawing.image_ref, Some("rId6".to_string()));
    }

    #[test]
    fn test_parse_drawing_wrap_none() {
        let xml = br#"<w:drawing xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"
                        xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing"
                        xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"
                        xmlns:pic="http://schemas.openxmlformats.org/drawingml/2006/picture"
                        xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
          <wp:anchor behindDoc="0" layoutInCell="0" relativeHeight="0">
            <wp:wrapNone/>
            <wp:extent cx="100000" cy="100000"/>
            <wp:docPr id="3" name="Picture 3"/>
            <a:graphic>
              <a:graphicData>
                <pic:pic>
                  <pic:blipFill>
                    <a:blip r:embed="rId7"/>
                  </pic:blipFill>
                </pic:pic>
              </a:graphicData>
            </a:graphic>
          </wp:anchor>
        </w:drawing>"#;

        let drawing = parse_drawing_from_xml(xml);

        match drawing.drawing_type {
            DrawingType::Anchored(anchor) => {
                assert_eq!(anchor.wrap_type, WrapType::None);
            }
            _ => panic!("Expected DrawingType::Anchored"),
        }
    }

    #[test]
    fn test_parse_drawing_wrap_top_and_bottom() {
        let xml = br#"<w:drawing xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"
                        xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing"
                        xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"
                        xmlns:pic="http://schemas.openxmlformats.org/drawingml/2006/picture"
                        xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
          <wp:anchor behindDoc="0" layoutInCell="0" relativeHeight="0">
            <wp:wrapTopAndBottom/>
            <wp:extent cx="100000" cy="100000"/>
            <wp:docPr id="4" name="Picture 4"/>
            <a:graphic>
              <a:graphicData>
                <pic:pic>
                  <pic:blipFill>
                    <a:blip r:embed="rId8"/>
                  </pic:blipFill>
                </pic:pic>
              </a:graphicData>
            </a:graphic>
          </wp:anchor>
        </w:drawing>"#;

        let drawing = parse_drawing_from_xml(xml);

        match drawing.drawing_type {
            DrawingType::Anchored(anchor) => {
                assert_eq!(anchor.wrap_type, WrapType::TopAndBottom);
            }
            _ => panic!("Expected DrawingType::Anchored"),
        }
    }

    #[test]
    fn test_extent_conversion() {
        let extent = Extent { cx: 914400, cy: 914400 };

        assert_eq!(extent.width_inches(), 1.0);
        assert_eq!(extent.height_inches(), 1.0);

        let extent2 = Extent {
            cx: 1828800,
            cy: 914400,
        };

        assert_eq!(extent2.width_inches(), 2.0);
        assert_eq!(extent2.height_inches(), 1.0);
    }

    #[test]
    fn test_parse_drawing_no_image() {
        let xml = br#"<w:drawing xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"
                        xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing"
                        xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main">
          <wp:inline>
            <wp:extent cx="100000" cy="100000"/>
            <wp:docPr id="5" name="Shape 5"/>
            <a:graphic>
              <a:graphicData>
                <!-- No blip element, just a shape -->
              </a:graphicData>
            </a:graphic>
          </wp:inline>
        </w:drawing>"#;

        let drawing = parse_drawing_from_xml(xml);

        assert_eq!(drawing.drawing_type, DrawingType::Inline);
        assert_eq!(drawing.extent, Some(Extent { cx: 100000, cy: 100000 }));
        assert_eq!(drawing.image_ref, None);
    }

    #[test]
    fn test_parse_drawing_empty_extent() {
        let xml = br#"<w:drawing xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"
                        xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing"
                        xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"
                        xmlns:pic="http://schemas.openxmlformats.org/drawingml/2006/picture"
                        xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
          <wp:inline>
            <wp:docPr id="6" name="Picture 6"/>
            <a:graphic>
              <a:graphicData>
                <pic:pic>
                  <pic:blipFill>
                    <a:blip r:embed="rId9"/>
                  </pic:blipFill>
                </pic:pic>
              </a:graphicData>
            </a:graphic>
          </wp:inline>
        </w:drawing>"#;

        let drawing = parse_drawing_from_xml(xml);

        assert_eq!(drawing.drawing_type, DrawingType::Inline);
        assert_eq!(drawing.extent, None);
        assert_eq!(drawing.image_ref, Some("rId9".to_string()));
    }

    /// Regression test for issue #590: <a:blip> with children (e.g. <a:extLst>) is parsed
    /// as Event::Start, not Event::Empty — the image reference must still be extracted.
    #[test]
    fn test_parse_blip_with_extlst_children() {
        let xml = br#"<w:drawing xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"
                        xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing"
                        xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"
                        xmlns:a14="http://schemas.microsoft.com/office/drawing/2010/main"
                        xmlns:pic="http://schemas.openxmlformats.org/drawingml/2006/picture"
                        xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
          <wp:inline distT="0" distB="0" distL="0" distR="0">
            <wp:extent cx="6480175" cy="9064625"/>
            <wp:docPr id="1" name="Picture 1"/>
            <a:graphic>
              <a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/picture">
                <pic:pic>
                  <pic:blipFill>
                    <a:blip r:embed="rId4" cstate="print">
                      <a:extLst>
                        <a:ext uri="{28A0092B-C50C-407E-A947-70E740481C1C}">
                          <a14:useLocalDpi val="0"/>
                        </a:ext>
                      </a:extLst>
                    </a:blip>
                  </pic:blipFill>
                </pic:pic>
              </a:graphicData>
            </a:graphic>
          </wp:inline>
        </w:drawing>"#;

        let drawing = parse_drawing_from_xml(xml);

        assert_eq!(drawing.drawing_type, DrawingType::Inline);
        assert_eq!(
            drawing.image_ref,
            Some("rId4".to_string()),
            "image_ref must be extracted even when <a:blip> has child elements"
        );
    }

    #[test]
    fn test_drawing_serialization() {
        let drawing = Drawing {
            drawing_type: DrawingType::Inline,
            extent: Some(Extent { cx: 914400, cy: 457200 }),
            doc_properties: Some(DocProperties {
                id: Some("1".to_string()),
                name: Some("Test".to_string()),
                description: Some("Test description".to_string()),
            }),
            image_ref: Some("rId5".to_string()),
            text_box_content: None,
        };

        let json = serde_json::to_string(&drawing).expect("Failed to serialize");

        let deserialized: Drawing = serde_json::from_str(&json).expect("Failed to deserialize");

        assert_eq!(drawing, deserialized);
    }
}
