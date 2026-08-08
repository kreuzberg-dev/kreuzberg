//! Application properties extraction from docProps/app.xml
//!
//! Extracts format-specific metadata from Office Open XML documents.

use crate::error::{Result, XbergError};
use roxmltree::Node;
use std::io::Read;
use zip::ZipArchive;

/// Application properties from docProps/app.xml for DOCX
///
/// Contains Word-specific document statistics and metadata.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DocxAppProperties {
    /// Application name (e.g., "Microsoft Office Word")
    pub application: Option<String>,
    /// Application version
    pub app_version: Option<String>,
    /// Template filename
    pub template: Option<String>,
    /// Total editing time in minutes
    pub total_time: Option<i32>,
    /// Number of pages
    pub pages: Option<i32>,
    /// Number of words
    pub words: Option<i32>,
    /// Number of characters (excluding spaces)
    pub characters: Option<i32>,
    /// Number of characters (including spaces)
    pub characters_with_spaces: Option<i32>,
    /// Number of lines
    pub lines: Option<i32>,
    /// Number of paragraphs
    pub paragraphs: Option<i32>,
    /// Company name
    pub company: Option<String>,
    /// Document security level
    pub doc_security: Option<i32>,
    /// Scale crop flag
    pub scale_crop: Option<bool>,
    /// Links up to date flag
    pub links_up_to_date: Option<bool>,
    /// Shared document flag
    pub shared_doc: Option<bool>,
    /// Hyperlinks changed flag
    pub hyperlinks_changed: Option<bool>,
}

/// Application properties from docProps/app.xml for XLSX
///
/// Contains Excel-specific document metadata.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct XlsxAppProperties {
    /// Application name (e.g., "Microsoft Excel")
    pub application: Option<String>,
    /// Application version
    pub app_version: Option<String>,
    /// Document security level
    pub doc_security: Option<i32>,
    /// Scale crop flag
    pub scale_crop: Option<bool>,
    /// Links up to date flag
    pub links_up_to_date: Option<bool>,
    /// Shared document flag
    pub shared_doc: Option<bool>,
    /// Hyperlinks changed flag
    pub hyperlinks_changed: Option<bool>,
    /// Company name
    pub company: Option<String>,
    /// Worksheet names
    pub worksheet_names: Vec<String>,
}

/// Application properties from docProps/app.xml for PPTX
///
/// Contains PowerPoint-specific document metadata.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PptxAppProperties {
    /// Application name (e.g., "Microsoft Office PowerPoint")
    pub application: Option<String>,
    /// Application version
    pub app_version: Option<String>,
    /// Total editing time in minutes
    pub total_time: Option<i32>,
    /// Company name
    pub company: Option<String>,
    /// Document security level
    pub doc_security: Option<i32>,
    /// Scale crop flag
    pub scale_crop: Option<bool>,
    /// Links up to date flag
    pub links_up_to_date: Option<bool>,
    /// Shared document flag
    pub shared_doc: Option<bool>,
    /// Hyperlinks changed flag
    pub hyperlinks_changed: Option<bool>,
    /// Number of slides
    pub slides: Option<i32>,
    /// Number of notes
    pub notes: Option<i32>,
    /// Number of hidden slides
    pub hidden_slides: Option<i32>,
    /// Number of multimedia clips
    pub multimedia_clips: Option<i32>,
    /// Presentation format (e.g., "Widescreen", "Standard")
    pub presentation_format: Option<String>,
    /// Slide titles
    pub slide_titles: Vec<String>,
}

/// Metadata key carrying the raw, undecoded `DocSecurity` integer.
pub(crate) const DOC_SECURITY_KEY: &str = "doc_security";

/// Decode a `DocSecurity` bit field into named boolean flags.
///
/// `DocSecurity` (ECMA-376 Part 1 §22.2.2.7, as clarified by MS-OI29500) packs four
/// independent restrictions into one integer: `1` = password protected, `2` = read-only
/// recommended, `4` = read-only enforced, `8` = locked for annotation. `doc_security` (see
/// [`DocxAppProperties::doc_security`] and its XLSX/PPTX equivalents) is parsed and stored
/// as that raw integer; extractors decode it here so consumers see named flags in
/// `Metadata::additional` rather than an opaque bit field (#230).
///
/// The pairs are returned in ascending bit order. Higher-order bits beyond `8` are not part
/// of the schema and are ignored.
///
/// All four flags are always returned, including when `raw` is `0`: an explicit `false`
/// records that the document *declares* no security restrictions, as opposed to the
/// `doc_security: None` case (no `DocSecurity` element at all), where nothing should be
/// decoded because there is no data to decode. Security-relevant booleans are exactly the
/// kind of value where "absent" and "false" must not be conflated by convention.
///
/// Returns `(key, value)` pairs; the keys are stable strings suitable for use in a
/// `Metadata::additional` map.
#[cfg_attr(alef, alef(skip))]
pub fn decode_doc_security_flags(raw: i32) -> [(&'static str, bool); 4] {
    const PASSWORD_PROTECTED_BIT: i32 = 1;
    const READ_ONLY_RECOMMENDED_BIT: i32 = 2;
    const READ_ONLY_ENFORCED_BIT: i32 = 4;
    const LOCKED_FOR_ANNOTATIONS_BIT: i32 = 8;

    [
        ("doc_security_password_protected", raw & PASSWORD_PROTECTED_BIT != 0),
        (
            "doc_security_read_only_recommended",
            raw & READ_ONLY_RECOMMENDED_BIT != 0,
        ),
        ("doc_security_read_only_enforced", raw & READ_ONLY_ENFORCED_BIT != 0),
        (
            "doc_security_locked_for_annotations",
            raw & LOCKED_FOR_ANNOTATIONS_BIT != 0,
        ),
    ]
}

/// Extract DOCX application properties from an Office Open XML document
///
/// Parses `docProps/app.xml` and extracts Word-specific metadata.
#[cfg_attr(alef, alef(skip))]
pub fn extract_docx_app_properties<R: Read + std::io::Seek>(archive: &mut ZipArchive<R>) -> Result<DocxAppProperties> {
    let xml_content = match super::read_zip_entry_to_string(archive, "docProps/app.xml", "app.xml")? {
        Some(content) => content,
        None => return Ok(DocxAppProperties::default()),
    };

    let doc = roxmltree::Document::parse(&xml_content)
        .map_err(|e| XbergError::parsing(format!("Failed to parse app.xml: {}", e)))?;

    let root = doc.root_element();

    Ok(DocxAppProperties {
        application: super::parse_xml_text(root, "Application"),
        app_version: super::parse_xml_text(root, "AppVersion"),
        template: super::parse_xml_text(root, "Template"),
        total_time: super::parse_xml_int(root, "TotalTime"),
        pages: super::parse_xml_int(root, "Pages"),
        words: super::parse_xml_int(root, "Words"),
        characters: super::parse_xml_int(root, "Characters"),
        characters_with_spaces: super::parse_xml_int(root, "CharactersWithSpaces"),
        lines: super::parse_xml_int(root, "Lines"),
        paragraphs: super::parse_xml_int(root, "Paragraphs"),
        company: super::parse_xml_text(root, "Company"),
        doc_security: super::parse_xml_int(root, "DocSecurity"),
        scale_crop: super::parse_xml_bool(root, "ScaleCrop"),
        links_up_to_date: super::parse_xml_bool(root, "LinksUpToDate"),
        shared_doc: super::parse_xml_bool(root, "SharedDoc"),
        hyperlinks_changed: super::parse_xml_bool(root, "HyperlinksChanged"),
    })
}

/// Extract XLSX application properties from an Office Open XML document
///
/// Parses `docProps/app.xml` and extracts Excel-specific metadata including worksheet names.
#[cfg_attr(alef, alef(skip))]
pub fn extract_xlsx_app_properties<R: Read + std::io::Seek>(archive: &mut ZipArchive<R>) -> Result<XlsxAppProperties> {
    let xml_content = match super::read_zip_entry_to_string(archive, "docProps/app.xml", "app.xml")? {
        Some(content) => content,
        None => return Ok(XlsxAppProperties::default()),
    };

    let doc = roxmltree::Document::parse(&xml_content)
        .map_err(|e| XbergError::parsing(format!("Failed to parse app.xml: {}", e)))?;

    let root = doc.root_element();

    let worksheet_names = titles_for_heading(root, "worksheet");

    Ok(XlsxAppProperties {
        application: super::parse_xml_text(root, "Application"),
        app_version: super::parse_xml_text(root, "AppVersion"),
        doc_security: super::parse_xml_int(root, "DocSecurity"),
        scale_crop: super::parse_xml_bool(root, "ScaleCrop"),
        links_up_to_date: super::parse_xml_bool(root, "LinksUpToDate"),
        shared_doc: super::parse_xml_bool(root, "SharedDoc"),
        hyperlinks_changed: super::parse_xml_bool(root, "HyperlinksChanged"),
        company: super::parse_xml_text(root, "Company"),
        worksheet_names,
    })
}

/// Extract PPTX application properties from an Office Open XML document
///
/// Parses `docProps/app.xml` and extracts PowerPoint-specific metadata including slide information.
#[cfg_attr(alef, alef(skip))]
pub fn extract_pptx_app_properties<R: Read + std::io::Seek>(archive: &mut ZipArchive<R>) -> Result<PptxAppProperties> {
    let xml_content = match super::read_zip_entry_to_string(archive, "docProps/app.xml", "app.xml")? {
        Some(content) => content,
        None => return Ok(PptxAppProperties::default()),
    };

    let doc = roxmltree::Document::parse(&xml_content)
        .map_err(|e| XbergError::parsing(format!("Failed to parse app.xml: {}", e)))?;

    let root = doc.root_element();

    let slide_titles = titles_for_heading(root, "slide");

    let presentation_format = super::parse_xml_text(root, "PresentationFormat");

    Ok(PptxAppProperties {
        application: super::parse_xml_text(root, "Application"),
        app_version: super::parse_xml_text(root, "AppVersion"),
        total_time: super::parse_xml_int(root, "TotalTime"),
        company: super::parse_xml_text(root, "Company"),
        doc_security: super::parse_xml_int(root, "DocSecurity"),
        scale_crop: super::parse_xml_bool(root, "ScaleCrop"),
        links_up_to_date: super::parse_xml_bool(root, "LinksUpToDate"),
        shared_doc: super::parse_xml_bool(root, "SharedDoc"),
        hyperlinks_changed: super::parse_xml_bool(root, "HyperlinksChanged"),
        slides: super::parse_xml_int(root, "Slides"),
        notes: super::parse_xml_int(root, "Notes"),
        hidden_slides: super::parse_xml_int(root, "HiddenSlides"),
        multimedia_clips: super::parse_xml_int(root, "MMClips"),
        presentation_format,
        slide_titles,
    })
}

/// Parse every `vt:lpstr` under `TitlesOfParts`'s `vt:vector`, in document order.
///
/// `TitlesOfParts` is a single flat vector that concatenates *several* logical groups
/// (e.g. worksheet names followed by named ranges, or theme names followed by slide
/// titles); the group boundaries are declared separately in the sibling `HeadingPairs`
/// element (see [`parse_heading_pairs`]). Entries are kept in raw form (including empty
/// strings) here so that slicing by `HeadingPairs` counts stays correctly aligned;
/// filtering happens after slicing in [`titles_for_heading`].
fn parse_titles_of_parts_raw(root: Node) -> Vec<String> {
    let mut titles = Vec::new();

    if let Some(titles_node) = root.descendants().find(|n| n.has_tag_name("TitlesOfParts"))
        && let Some(vector_node) = titles_node.descendants().find(|n| n.has_tag_name("vector"))
    {
        for lpstr_node in vector_node.children().filter(|n| n.has_tag_name("lpstr")) {
            titles.push(lpstr_node.text().unwrap_or("").trim().to_string());
        }
    }

    titles
}

/// Parse `HeadingPairs` into an ordered list of `(group name, entry count)` pairs.
///
/// `HeadingPairs` is a `vt:vector` of `vt:variant` pairs: a name (`vt:lpstr`, e.g.
/// `"Worksheets"` or `"Named Ranges"`) immediately followed by a count (`vt:i4`) of how
/// many consecutive entries in `TitlesOfParts` belong to that group. Malformed or
/// incomplete pairs are skipped rather than aborting the whole parse.
fn parse_heading_pairs(root: Node) -> Vec<(String, usize)> {
    let Some(heading_node) = root.descendants().find(|n| n.has_tag_name("HeadingPairs")) else {
        return Vec::new();
    };
    let Some(vector_node) = heading_node.descendants().find(|n| n.has_tag_name("vector")) else {
        return Vec::new();
    };

    let variants: Vec<Node> = vector_node.children().filter(|n| n.has_tag_name("variant")).collect();

    let mut pairs = Vec::new();
    let mut iter = variants.into_iter();
    while let (Some(name_variant), Some(count_variant)) = (iter.next(), iter.next()) {
        let name = name_variant
            .children()
            .find(|n| n.has_tag_name("lpstr"))
            .and_then(|n| n.text())
            .map(|s| s.trim().to_string());
        let count = count_variant
            .children()
            .find(|n| n.has_tag_name("i4"))
            .and_then(|n| n.text())
            .and_then(|s| s.trim().parse::<usize>().ok());

        if let (Some(name), Some(count)) = (name, count) {
            pairs.push((name, count));
        }
    }

    pairs
}

/// Return the `TitlesOfParts` entries belonging to the `HeadingPairs` group whose name
/// contains `needle` (case-insensitive), e.g. `"worksheet"` or `"slide"`.
///
/// Falls back to returning all non-empty titles when `HeadingPairs` is absent, matching
/// the pre-#231 behavior for documents that omit it. When `HeadingPairs` is present but no
/// group name matches `needle`, returns an empty list rather than guessing.
fn titles_for_heading(root: Node, needle: &str) -> Vec<String> {
    let titles = parse_titles_of_parts_raw(root);
    let pairs = parse_heading_pairs(root);

    if pairs.is_empty() {
        return titles.into_iter().filter(|t| !t.is_empty()).collect();
    }

    let mut offset = 0usize;
    for (name, count) in &pairs {
        let start = offset.min(titles.len());
        let end = (offset + count).min(titles.len());
        if name.to_lowercase().contains(needle) {
            return titles[start..end].iter().filter(|t| !t.is_empty()).cloned().collect();
        }
        offset = end;
    }

    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Write};

    fn create_test_zip_with_app_xml(app_xml: &str) -> ZipArchive<Cursor<Vec<u8>>> {
        let buffer = Vec::new();
        let cursor = Cursor::new(buffer);
        let mut zip = zip::ZipWriter::new(cursor);

        let options = zip::write::FileOptions::<()>::default().compression_method(zip::CompressionMethod::Stored);

        zip.start_file("docProps/app.xml", options).unwrap();
        zip.write_all(app_xml.as_bytes()).unwrap();

        let cursor = zip.finish().unwrap();
        ZipArchive::new(cursor).unwrap()
    }

    #[test]
    fn test_extract_docx_app_properties() {
        let app_xml = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Properties xmlns="http://schemas.openxmlformats.org/officeDocument/2006/extended-properties">
    <Application>Microsoft Office Word</Application>
    <AppVersion>16.0000</AppVersion>
    <TotalTime>120</TotalTime>
    <Pages>5</Pages>
    <Words>1000</Words>
    <Characters>5500</Characters>
    <CharactersWithSpaces>6500</CharactersWithSpaces>
    <Lines>100</Lines>
    <Paragraphs>50</Paragraphs>
    <Company>Acme Corp</Company>
    <DocSecurity>0</DocSecurity>
    <ScaleCrop>false</ScaleCrop>
</Properties>"#;

        let mut archive = create_test_zip_with_app_xml(app_xml);
        let props = extract_docx_app_properties(&mut archive).unwrap();

        assert_eq!(props.application, Some("Microsoft Office Word".to_string()));
        assert_eq!(props.app_version, Some("16.0000".to_string()));
        assert_eq!(props.total_time, Some(120));
        assert_eq!(props.pages, Some(5));
        assert_eq!(props.words, Some(1000));
        assert_eq!(props.characters, Some(5500));
        assert_eq!(props.characters_with_spaces, Some(6500));
        assert_eq!(props.lines, Some(100));
        assert_eq!(props.paragraphs, Some(50));
        assert_eq!(props.company, Some("Acme Corp".to_string()));
        assert_eq!(props.doc_security, Some(0));
        assert_eq!(props.scale_crop, Some(false));
    }

    #[test]
    fn test_extract_xlsx_app_properties_with_worksheets() {
        let app_xml = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Properties xmlns="http://schemas.openxmlformats.org/officeDocument/2006/extended-properties"
            xmlns:vt="http://schemas.openxmlformats.org/officeDocument/2006/docPropsVTypes">
    <Application>Microsoft Excel</Application>
    <AppVersion>16.0300</AppVersion>
    <Company>Test Company</Company>
    <TitlesOfParts>
        <vt:vector size="3" baseType="lpstr">
            <vt:lpstr>Sheet1</vt:lpstr>
            <vt:lpstr>Sheet2</vt:lpstr>
            <vt:lpstr>Sheet3</vt:lpstr>
        </vt:vector>
    </TitlesOfParts>
</Properties>"#;

        let mut archive = create_test_zip_with_app_xml(app_xml);
        let props = extract_xlsx_app_properties(&mut archive).unwrap();

        assert_eq!(props.application, Some("Microsoft Excel".to_string()));
        assert_eq!(props.app_version, Some("16.0300".to_string()));
        assert_eq!(props.company, Some("Test Company".to_string()));
        assert_eq!(props.worksheet_names, vec!["Sheet1", "Sheet2", "Sheet3"]);
    }

    #[test]
    fn test_extract_pptx_app_properties() {
        let app_xml = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Properties xmlns="http://schemas.openxmlformats.org/officeDocument/2006/extended-properties"
            xmlns:vt="http://schemas.openxmlformats.org/officeDocument/2006/docPropsVTypes">
    <Application>Microsoft Office PowerPoint</Application>
    <AppVersion>16.0000</AppVersion>
    <TotalTime>45</TotalTime>
    <Slides>10</Slides>
    <Notes>5</Notes>
    <HiddenSlides>2</HiddenSlides>
    <MMClips>3</MMClips>
    <PresentationFormat>Widescreen</PresentationFormat>
    <TitlesOfParts>
        <vt:vector size="2" baseType="lpstr">
            <vt:lpstr>Title Slide</vt:lpstr>
            <vt:lpstr>Agenda</vt:lpstr>
        </vt:vector>
    </TitlesOfParts>
</Properties>"#;

        let mut archive = create_test_zip_with_app_xml(app_xml);
        let props = extract_pptx_app_properties(&mut archive).unwrap();

        assert_eq!(props.application, Some("Microsoft Office PowerPoint".to_string()));
        assert_eq!(props.slides, Some(10));
        assert_eq!(props.notes, Some(5));
        assert_eq!(props.hidden_slides, Some(2));
        assert_eq!(props.multimedia_clips, Some(3));
        assert_eq!(props.presentation_format, Some("Widescreen".to_string()));
        assert_eq!(props.slide_titles, vec!["Title Slide", "Agenda"]);
    }

    #[test]
    fn test_extract_app_properties_missing_file() {
        let buffer = Vec::new();
        let cursor = Cursor::new(buffer);
        let zip = zip::ZipWriter::new(cursor);
        let cursor = zip.finish().unwrap();
        let mut archive = ZipArchive::new(cursor).unwrap();

        let docx = extract_docx_app_properties(&mut archive).unwrap();
        assert_eq!(docx, DocxAppProperties::default());

        let buffer = Vec::new();
        let cursor = Cursor::new(buffer);
        let zip = zip::ZipWriter::new(cursor);
        let cursor = zip.finish().unwrap();
        let mut archive = ZipArchive::new(cursor).unwrap();

        let xlsx = extract_xlsx_app_properties(&mut archive).unwrap();
        assert_eq!(xlsx, XlsxAppProperties::default());
    }

    #[test]
    fn test_extract_titles_of_parts_empty() {
        let xml = r#"<Properties xmlns:vt="http://schemas.openxmlformats.org/officeDocument/2006/docPropsVTypes">
            <TitlesOfParts><vt:vector size="0" baseType="lpstr"></vt:vector></TitlesOfParts>
        </Properties>"#;

        let doc = roxmltree::Document::parse(xml).unwrap();
        let titles = titles_for_heading(doc.root_element(), "worksheet");
        assert_eq!(titles, Vec::<String>::new());
    }

    #[test]
    fn test_titles_for_heading_slices_by_heading_pairs() {
        let xml = r#"<Properties xmlns:vt="http://schemas.openxmlformats.org/officeDocument/2006/docPropsVTypes">
            <HeadingPairs>
                <vt:vector size="4" baseType="variant">
                    <vt:variant><vt:lpstr>Worksheets</vt:lpstr></vt:variant>
                    <vt:variant><vt:i4>2</vt:i4></vt:variant>
                    <vt:variant><vt:lpstr>Named Ranges</vt:lpstr></vt:variant>
                    <vt:variant><vt:i4>1</vt:i4></vt:variant>
                </vt:vector>
            </HeadingPairs>
            <TitlesOfParts>
                <vt:vector size="3" baseType="lpstr">
                    <vt:lpstr>Sheet1</vt:lpstr>
                    <vt:lpstr>Sheet2</vt:lpstr>
                    <vt:lpstr>Print_Area</vt:lpstr>
                </vt:vector>
            </TitlesOfParts>
        </Properties>"#;

        let doc = roxmltree::Document::parse(xml).unwrap();
        assert_eq!(
            titles_for_heading(doc.root_element(), "worksheet"),
            vec!["Sheet1".to_string(), "Sheet2".to_string()]
        );
        assert_eq!(
            titles_for_heading(doc.root_element(), "named range"),
            vec!["Print_Area".to_string()]
        );
    }

    #[test]
    fn should_decode_zero_doc_security_as_all_flags_false() {
        assert_eq!(
            decode_doc_security_flags(0),
            [
                ("doc_security_password_protected", false),
                ("doc_security_read_only_recommended", false),
                ("doc_security_read_only_enforced", false),
                ("doc_security_locked_for_annotations", false),
            ]
        );
    }

    #[test]
    fn should_map_each_doc_security_bit_to_its_ecma376_meaning() {
        // ECMA-376 §22.2.2.7: 1 = password protected, 2 = read-only recommended,
        // 4 = read-only enforced, 8 = locked for annotation. Bits 1 and 2 are
        // adjacent and easy to transpose, so each is pinned individually.
        assert_eq!(
            decode_doc_security_flags(1),
            [
                ("doc_security_password_protected", true),
                ("doc_security_read_only_recommended", false),
                ("doc_security_read_only_enforced", false),
                ("doc_security_locked_for_annotations", false),
            ]
        );
        assert_eq!(
            decode_doc_security_flags(2),
            [
                ("doc_security_password_protected", false),
                ("doc_security_read_only_recommended", true),
                ("doc_security_read_only_enforced", false),
                ("doc_security_locked_for_annotations", false),
            ]
        );
        assert_eq!(
            decode_doc_security_flags(4),
            [
                ("doc_security_password_protected", false),
                ("doc_security_read_only_recommended", false),
                ("doc_security_read_only_enforced", true),
                ("doc_security_locked_for_annotations", false),
            ]
        );
        assert_eq!(
            decode_doc_security_flags(8),
            [
                ("doc_security_password_protected", false),
                ("doc_security_read_only_recommended", false),
                ("doc_security_read_only_enforced", false),
                ("doc_security_locked_for_annotations", true),
            ]
        );
    }

    #[test]
    fn should_decode_combined_read_only_recommended_and_enforced_bits() {
        // 6 = 2 (read-only recommended) + 4 (read-only enforced)
        assert_eq!(
            decode_doc_security_flags(6),
            [
                ("doc_security_password_protected", false),
                ("doc_security_read_only_recommended", true),
                ("doc_security_read_only_enforced", true),
                ("doc_security_locked_for_annotations", false),
            ]
        );
    }

    #[test]
    fn should_decode_all_doc_security_bits_when_all_set() {
        // 15 = 1 + 2 + 4 + 8, all four restrictions active
        assert_eq!(
            decode_doc_security_flags(15),
            [
                ("doc_security_password_protected", true),
                ("doc_security_read_only_recommended", true),
                ("doc_security_read_only_enforced", true),
                ("doc_security_locked_for_annotations", true),
            ]
        );
    }

    #[test]
    fn should_ignore_doc_security_bits_outside_the_ecma376_schema() {
        // Bit 16 (0x10) is outside the DocSecurity schema and must not be
        // surfaced as, or conflated with, any of the four named flags.
        assert_eq!(
            decode_doc_security_flags(16),
            [
                ("doc_security_password_protected", false),
                ("doc_security_read_only_recommended", false),
                ("doc_security_read_only_enforced", false),
                ("doc_security_locked_for_annotations", false),
            ]
        );
    }

    #[test]
    fn test_titles_for_heading_no_match_returns_empty() {
        let xml = r#"<Properties xmlns:vt="http://schemas.openxmlformats.org/officeDocument/2006/docPropsVTypes">
            <HeadingPairs>
                <vt:vector size="2" baseType="variant">
                    <vt:variant><vt:lpstr>Fonts Used</vt:lpstr></vt:variant>
                    <vt:variant><vt:i4>1</vt:i4></vt:variant>
                </vt:vector>
            </HeadingPairs>
            <TitlesOfParts>
                <vt:vector size="1" baseType="lpstr">
                    <vt:lpstr>Arial</vt:lpstr>
                </vt:vector>
            </TitlesOfParts>
        </Properties>"#;

        let doc = roxmltree::Document::parse(xml).unwrap();
        assert_eq!(titles_for_heading(doc.root_element(), "slide"), Vec::<String>::new());
    }
}
