//! DOCX section properties parsing from `w:sectPr` elements.
//!
//! Parses page size, margins, orientation, columns, and other
//! section-level properties from OOXML documents.

#[cfg(test)]
use crate::extraction::ooxml_constants::WORDPROCESSINGML_NAMESPACE;
use crate::extractors::security::{SecurityBudget, SecurityError};
use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};

/// Page orientation.
#[cfg_attr(alef, alef(skip))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Orientation {
    /// Page height is greater than page width.
    Portrait,
    /// Page width is greater than page height.
    Landscape,
}

/// Page margins in twips (twentieths of a point).
#[cfg_attr(alef, alef(skip))]
#[derive(Debug, Clone, Default)]
pub struct PageMargins {
    /// Top margin in twips.
    pub top: Option<i32>,
    /// Right margin in twips.
    pub right: Option<i32>,
    /// Bottom margin in twips.
    pub bottom: Option<i32>,
    /// Left margin in twips.
    pub left: Option<i32>,
    /// Header offset in twips.
    pub header: Option<i32>,
    /// Footer offset in twips.
    pub footer: Option<i32>,
    /// Gutter margin in twips.
    pub gutter: Option<i32>,
}

/// Page margins converted to points (1/72 inch).
#[cfg_attr(alef, alef(skip))]
#[derive(Debug, Clone, Default)]
pub struct PageMarginsPoints {
    /// Top margin in points.
    pub top: Option<f64>,
    /// Right margin in points.
    pub right: Option<f64>,
    /// Bottom margin in points.
    pub bottom: Option<f64>,
    /// Left margin in points.
    pub left: Option<f64>,
    /// Header offset in points.
    pub header: Option<f64>,
    /// Footer offset in points.
    pub footer: Option<f64>,
    /// Gutter margin in points.
    pub gutter: Option<f64>,
}

/// Column layout configuration.
#[cfg_attr(alef, alef(skip))]
#[derive(Debug, Clone, Default)]
pub struct ColumnLayout {
    /// Number of columns.
    pub count: Option<i32>,
    /// Space between columns in twips.
    pub space_twips: Option<i32>,
    /// Whether columns have equal width; if `false`, individual widths may vary.
    pub equal_width: Option<bool>,
}

/// DOCX section properties parsed from `w:sectPr` element.
#[cfg_attr(alef, alef(skip))]
#[derive(Debug, Clone, Default)]
pub struct SectionProperties {
    /// Page width in twips (from `w:pgSz w:w`).
    pub page_width_twips: Option<i32>,
    /// Page height in twips (from `w:pgSz w:h`).
    pub page_height_twips: Option<i32>,
    /// Page orientation (from `w:pgSz w:orient`).
    pub orientation: Option<Orientation>,
    /// Page margins (from `w:pgMar`).
    pub margins: PageMargins,
    /// Column layout (from `w:cols`).
    pub columns: ColumnLayout,
    /// Document grid line pitch in twips (from `w:docGrid w:linePitch`).
    pub doc_grid_line_pitch: Option<i32>,
}

/// Get a namespaced integer attribute.
fn get_w_attr_i32(element: &BytesStart, local_name: &str) -> Option<i32> {
    let w_prefixed = format!("w:{}", local_name);
    for attr in element.attributes().flatten() {
        let key = attr.key.as_ref();
        if (key == w_prefixed.as_bytes() || key == local_name.as_bytes())
            && let Ok(val) = std::str::from_utf8(&attr.value)
            && let Ok(num) = val.parse::<i32>()
        {
            return Some(num);
        }
    }
    None
}

/// Get a namespaced string attribute.
fn get_w_attr_string(element: &BytesStart, local_name: &str) -> Option<String> {
    for attr in element.attributes().flatten() {
        let key_str = std::str::from_utf8(attr.key.as_ref()).ok()?;
        if (key_str == local_name || key_str.ends_with(&format!(":{}", local_name)))
            && let Ok(val) = std::str::from_utf8(&attr.value)
        {
            return Some(val.to_string());
        }
    }
    None
}

/// Check if an attribute has a specific value (for roxmltree).
#[cfg(test)]
fn roxmltree_get_attr(node: &roxmltree::Node, local_name: &str) -> Option<String> {
    node.attribute((WORDPROCESSINGML_NAMESPACE, local_name))
        .or_else(|| node.attribute(local_name))
        .map(String::from)
}

/// Get a namespaced integer attribute (for roxmltree).
#[cfg(test)]
fn roxmltree_get_i32_attr(node: &roxmltree::Node, local_name: &str) -> Option<i32> {
    node.attribute((WORDPROCESSINGML_NAMESPACE, local_name))
        .or_else(|| node.attribute(local_name))
        .and_then(|v| v.parse::<i32>().ok())
}

/// Parse a `w:sectPr` XML element (roxmltree node) into `SectionProperties`.
#[cfg(test)]
pub(crate) fn parse_section_properties(node: &roxmltree::Node) -> SectionProperties {
    let mut props = SectionProperties::default();

    for child in node.children() {
        if !child.is_element() {
            continue;
        }

        match child.tag_name().name() {
            "pgSz" => {
                props.page_width_twips = roxmltree_get_i32_attr(&child, "w");
                props.page_height_twips = roxmltree_get_i32_attr(&child, "h");

                props.orientation = match roxmltree_get_attr(&child, "orient").as_deref() {
                    Some("landscape") => Some(Orientation::Landscape),
                    _ => Some(Orientation::Portrait),
                };
            }
            "pgMar" => {
                props.margins.top = roxmltree_get_i32_attr(&child, "top");
                props.margins.right = roxmltree_get_i32_attr(&child, "right");
                props.margins.bottom = roxmltree_get_i32_attr(&child, "bottom");
                props.margins.left = roxmltree_get_i32_attr(&child, "left");
                props.margins.header = roxmltree_get_i32_attr(&child, "header");
                props.margins.footer = roxmltree_get_i32_attr(&child, "footer");
                props.margins.gutter = roxmltree_get_i32_attr(&child, "gutter");
            }
            "cols" => {
                props.columns.count = roxmltree_get_i32_attr(&child, "num");
                props.columns.space_twips = roxmltree_get_i32_attr(&child, "space");

                if let Some(eq_width_str) = roxmltree_get_attr(&child, "equalWidth") {
                    props.columns.equal_width = Some(eq_width_str == "1" || eq_width_str == "true");
                }
            }
            "docGrid" => {
                props.doc_grid_line_pitch = roxmltree_get_i32_attr(&child, "linePitch");
            }
            _ => {}
        }
    }

    props
}

/// Parse section properties from a quick_xml event stream.
///
/// Reads events from the reader until `</w:sectPr>` is encountered,
/// extracting the same properties as the roxmltree parser.
///
/// **Important:** This function advances the reader past the closing `</w:sectPr>` tag.
/// The caller must not attempt to process the `w:sectPr` end event again.
///
/// Threads `budget` through every event so nesting inside `w:sectPr` is measured
/// against the caller's depth cap instead of passing through unaccounted (GH#384).
pub(crate) fn parse_section_properties_streaming(
    reader: &mut Reader<&[u8]>,
    budget: &mut SecurityBudget,
) -> Result<SectionProperties, SecurityError> {
    let mut props = SectionProperties::default();
    let mut buf = Vec::new();

    loop {
        budget.step()?;
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                budget.enter()?;
                apply_section_element(e, &mut props);
            }
            Ok(Event::Empty(ref e)) => {
                apply_section_element(e, &mut props);
            }
            Ok(Event::End(ref e)) => {
                budget.leave();
                if e.name().as_ref() as &[u8] == b"w:sectPr" {
                    break;
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }

    if props.page_width_twips.is_some() && props.orientation.is_none() {
        props.orientation = Some(Orientation::Portrait);
    }

    Ok(props)
}

/// Extract section properties from a quick_xml element.
///
/// Shared handler for both `Event::Start` and `Event::Empty` events,
/// since OOXML section child elements (`w:pgSz`, `w:pgMar`, etc.)
/// are typically self-closing.
fn apply_section_element(e: &BytesStart, props: &mut SectionProperties) {
    match e.name().as_ref() as &[u8] {
        b"w:pgSz" => {
            props.page_width_twips = get_w_attr_i32(e, "w");
            props.page_height_twips = get_w_attr_i32(e, "h");
            props.orientation = get_w_attr_string(e, "orient").and_then(|s| match s.as_str() {
                "landscape" => Some(Orientation::Landscape),
                "portrait" => Some(Orientation::Portrait),
                _ => None,
            });
        }
        b"w:pgMar" => {
            props.margins.top = get_w_attr_i32(e, "top");
            props.margins.right = get_w_attr_i32(e, "right");
            props.margins.bottom = get_w_attr_i32(e, "bottom");
            props.margins.left = get_w_attr_i32(e, "left");
            props.margins.header = get_w_attr_i32(e, "header");
            props.margins.footer = get_w_attr_i32(e, "footer");
            props.margins.gutter = get_w_attr_i32(e, "gutter");
        }
        b"w:cols" => {
            props.columns.count = get_w_attr_i32(e, "num");
            props.columns.space_twips = get_w_attr_i32(e, "space");
            if let Some(eq_width_str) = get_w_attr_string(e, "equalWidth") {
                props.columns.equal_width = Some(eq_width_str == "1" || eq_width_str == "true");
            }
        }
        b"w:docGrid" => {
            props.doc_grid_line_pitch = get_w_attr_i32(e, "linePitch");
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_a4_page_size() {
        let xml = r#"<?xml version="1.0"?>
<w:sectPr xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
    <w:pgSz w:w="11906" w:h="16838"/>
</w:sectPr>"#;

        let doc = roxmltree::Document::parse(xml).unwrap();
        let root = doc.root_element();
        let props = parse_section_properties(&root);

        assert_eq!(props.page_width_twips, Some(11906));
        assert_eq!(props.page_height_twips, Some(16838));
    }

    #[test]
    fn test_parse_us_letter_page_size() {
        let xml = r#"<?xml version="1.0"?>
<w:sectPr xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
    <w:pgSz w:w="12240" w:h="15840"/>
</w:sectPr>"#;

        let doc = roxmltree::Document::parse(xml).unwrap();
        let root = doc.root_element();
        let props = parse_section_properties(&root);

        assert_eq!(props.page_width_twips, Some(12240));
        assert_eq!(props.page_height_twips, Some(15840));
    }

    #[test]
    fn test_parse_landscape_orientation() {
        let xml = r#"<?xml version="1.0"?>
<w:sectPr xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
    <w:pgSz w:w="16838" w:h="11906" w:orient="landscape"/>
</w:sectPr>"#;

        let doc = roxmltree::Document::parse(xml).unwrap();
        let root = doc.root_element();
        let props = parse_section_properties(&root);

        assert_eq!(props.orientation, Some(Orientation::Landscape));
        assert_eq!(props.page_width_twips, Some(16838));
        assert_eq!(props.page_height_twips, Some(11906));
    }

    #[test]
    fn test_parse_margins_all_fields() {
        let xml = r#"<?xml version="1.0"?>
<w:sectPr xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
    <w:pgMar w:top="1440" w:right="1440" w:bottom="1440" w:left="1440" w:header="720" w:footer="720" w:gutter="0"/>
</w:sectPr>"#;

        let doc = roxmltree::Document::parse(xml).unwrap();
        let root = doc.root_element();
        let props = parse_section_properties(&root);

        assert_eq!(props.margins.top, Some(1440));
        assert_eq!(props.margins.right, Some(1440));
        assert_eq!(props.margins.bottom, Some(1440));
        assert_eq!(props.margins.left, Some(1440));
        assert_eq!(props.margins.header, Some(720));
        assert_eq!(props.margins.footer, Some(720));
        assert_eq!(props.margins.gutter, Some(0));
    }

    #[test]
    fn test_parse_columns() {
        let xml = r#"<?xml version="1.0"?>
<w:sectPr xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
    <w:cols w:num="2" w:space="720" w:equalWidth="1"/>
</w:sectPr>"#;

        let doc = roxmltree::Document::parse(xml).unwrap();
        let root = doc.root_element();
        let props = parse_section_properties(&root);

        assert_eq!(props.columns.count, Some(2));
        assert_eq!(props.columns.space_twips, Some(720));
        assert_eq!(props.columns.equal_width, Some(true));
    }

    #[test]
    fn test_parse_empty_section_properties() {
        let xml = r#"<?xml version="1.0"?>
<w:sectPr xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
</w:sectPr>"#;

        let doc = roxmltree::Document::parse(xml).unwrap();
        let root = doc.root_element();
        let props = parse_section_properties(&root);

        assert_eq!(props.page_width_twips, None);
        assert_eq!(props.page_height_twips, None);
        assert_eq!(props.orientation, None);
        assert_eq!(props.margins.top, None);
        assert_eq!(props.columns.count, None);
        assert_eq!(props.doc_grid_line_pitch, None);
    }

    #[test]
    fn test_streaming_parse_a4_page_size() {
        let xml = r#"<w:sectPr xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
            <w:pgSz w:w="11906" w:h="16838"/>
            <w:pgMar w:top="1440" w:right="1440" w:bottom="1440" w:left="1440" w:header="708" w:footer="708" w:gutter="0"/>
            <w:cols w:space="708"/>
            <w:docGrid w:linePitch="360"/>
        </w:sectPr>"#;

        let mut reader = Reader::from_str(xml);
        reader.config_mut().trim_text(false);
        let mut buf = Vec::new();

        loop {
            match reader.read_event_into(&mut buf) {
                Ok(Event::Start(ref e)) if e.name().as_ref() as &[u8] == b"w:sectPr" => break,
                Ok(Event::Eof) => panic!("unexpected EOF"),
                Err(e) => panic!("unexpected error: {}", e),
                _ => {}
            }
            buf.clear();
        }

        let mut budget = SecurityBudget::with_defaults();
        let props = parse_section_properties_streaming(&mut reader, &mut budget).unwrap();

        assert_eq!(props.page_width_twips, Some(11906));
        assert_eq!(props.page_height_twips, Some(16838));
        assert_eq!(props.orientation, Some(Orientation::Portrait));
        assert_eq!(props.margins.top, Some(1440));
        assert_eq!(props.margins.right, Some(1440));
        assert_eq!(props.margins.header, Some(708));
        assert_eq!(props.margins.gutter, Some(0));
        assert_eq!(props.columns.space_twips, Some(708));
        assert_eq!(props.doc_grid_line_pitch, Some(360));
    }

    #[test]
    fn test_streaming_parse_landscape() {
        let xml = r#"<w:sectPr xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
            <w:pgSz w:w="16838" w:h="11906" w:orient="landscape"/>
        </w:sectPr>"#;

        let mut reader = Reader::from_str(xml);
        reader.config_mut().trim_text(false);
        let mut buf = Vec::new();
        loop {
            match reader.read_event_into(&mut buf) {
                Ok(Event::Start(ref e)) if e.name().as_ref() as &[u8] == b"w:sectPr" => break,
                Ok(Event::Eof) => panic!("unexpected EOF"),
                _ => {}
            }
            buf.clear();
        }

        let mut budget = SecurityBudget::with_defaults();
        let props = parse_section_properties_streaming(&mut reader, &mut budget).unwrap();
        assert_eq!(props.orientation, Some(Orientation::Landscape));
        assert_eq!(props.page_width_twips, Some(16838));
    }

    #[test]
    fn test_section_properties_with_doc_grid() {
        let xml = r#"<?xml version="1.0"?>
<w:sectPr xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
    <w:docGrid w:linePitch="360"/>
</w:sectPr>"#;

        let doc = roxmltree::Document::parse(xml).unwrap();
        let root = doc.root_element();
        let props = parse_section_properties(&root);

        assert_eq!(props.doc_grid_line_pitch, Some(360));
    }
}
