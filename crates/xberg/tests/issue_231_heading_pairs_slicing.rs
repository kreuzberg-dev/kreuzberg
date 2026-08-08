//! Regression test for #231 — `extract_titles_of_parts` takes every `vt:lpstr` under the
//! first `TitlesOfParts` `vt:vector` without consulting the sibling `HeadingPairs` element,
//! which defines the group boundaries within that flat vector.
//!
//! For XLSX, `TitlesOfParts` is `[worksheet names..., named ranges...]`; for PPTX it is
//! `[theme names..., slide titles...]`. Without slicing by `HeadingPairs`, `worksheet_names`
//! silently includes named ranges, and `slide_titles` begins with the theme name(s) instead
//! of the first slide's title.

#![cfg(any(feature = "excel", feature = "office"))]

use std::io::{Cursor, Write};
#[cfg(feature = "office")]
use xberg::extraction::office_metadata::app_properties::extract_pptx_app_properties;
#[cfg(feature = "excel")]
use xberg::extraction::office_metadata::app_properties::extract_xlsx_app_properties;
use zip::ZipArchive;

fn zip_with_app_xml(app_xml: &str) -> ZipArchive<Cursor<Vec<u8>>> {
    let cursor = Cursor::new(Vec::new());
    let mut zip = zip::ZipWriter::new(cursor);
    let options = zip::write::FileOptions::<()>::default().compression_method(zip::CompressionMethod::Stored);
    zip.start_file("docProps/app.xml", options).unwrap();
    zip.write_all(app_xml.as_bytes()).unwrap();
    ZipArchive::new(zip.finish().unwrap()).unwrap()
}

// The two XLSX tests need the same gate as the import they use: the file compiles under
// `office` alone, but `extract_xlsx_app_properties` only exists with `excel`.
#[test]
#[cfg(feature = "excel")]
fn should_exclude_named_ranges_from_worksheet_names_using_heading_pairs() {
    // Real-world shape: 3 worksheets followed by 2 named ranges, with HeadingPairs
    // declaring the ("Worksheets", 3) then ("Named Ranges", 2) group boundaries.
    let app_xml = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Properties xmlns="http://schemas.openxmlformats.org/officeDocument/2006/extended-properties"
            xmlns:vt="http://schemas.openxmlformats.org/officeDocument/2006/docPropsVTypes">
    <Application>Microsoft Excel</Application>
    <HeadingPairs>
        <vt:vector size="4" baseType="variant">
            <vt:variant><vt:lpstr>Worksheets</vt:lpstr></vt:variant>
            <vt:variant><vt:i4>3</vt:i4></vt:variant>
            <vt:variant><vt:lpstr>Named Ranges</vt:lpstr></vt:variant>
            <vt:variant><vt:i4>2</vt:i4></vt:variant>
        </vt:vector>
    </HeadingPairs>
    <TitlesOfParts>
        <vt:vector size="5" baseType="lpstr">
            <vt:lpstr>Sheet1</vt:lpstr>
            <vt:lpstr>Sheet2</vt:lpstr>
            <vt:lpstr>Sheet3</vt:lpstr>
            <vt:lpstr>Print_Area</vt:lpstr>
            <vt:lpstr>Company_Name</vt:lpstr>
        </vt:vector>
    </TitlesOfParts>
</Properties>"#;

    let mut archive = zip_with_app_xml(app_xml);
    let props = extract_xlsx_app_properties(&mut archive).unwrap();

    assert_eq!(
        props.worksheet_names,
        vec!["Sheet1".to_string(), "Sheet2".to_string(), "Sheet3".to_string()],
        "worksheet_names must not include named ranges from the second HeadingPairs group"
    );
}

#[test]
#[cfg(feature = "excel")]
fn should_return_all_titles_when_heading_pairs_absent_for_backward_compat() {
    let app_xml = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Properties xmlns="http://schemas.openxmlformats.org/officeDocument/2006/extended-properties"
            xmlns:vt="http://schemas.openxmlformats.org/officeDocument/2006/docPropsVTypes">
    <Application>Microsoft Excel</Application>
    <TitlesOfParts>
        <vt:vector size="3" baseType="lpstr">
            <vt:lpstr>Sheet1</vt:lpstr>
            <vt:lpstr>Sheet2</vt:lpstr>
            <vt:lpstr>Sheet3</vt:lpstr>
        </vt:vector>
    </TitlesOfParts>
</Properties>"#;

    let mut archive = zip_with_app_xml(app_xml);
    let props = extract_xlsx_app_properties(&mut archive).unwrap();

    assert_eq!(
        props.worksheet_names,
        vec!["Sheet1".to_string(), "Sheet2".to_string(), "Sheet3".to_string()]
    );
}

#[test]
#[cfg(feature = "office")]
fn should_exclude_theme_name_from_slide_titles_using_heading_pairs() {
    // Real-world shape: 1 theme name followed by 2 slide titles, with HeadingPairs
    // declaring ("Fonts Used", 1), ("Theme", 1), ("Slide Titles", 2).
    let app_xml = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Properties xmlns="http://schemas.openxmlformats.org/officeDocument/2006/extended-properties"
            xmlns:vt="http://schemas.openxmlformats.org/officeDocument/2006/docPropsVTypes">
    <Application>Microsoft Office PowerPoint</Application>
    <HeadingPairs>
        <vt:vector size="4" baseType="variant">
            <vt:variant><vt:lpstr>Theme</vt:lpstr></vt:variant>
            <vt:variant><vt:i4>1</vt:i4></vt:variant>
            <vt:variant><vt:lpstr>Slide Titles</vt:lpstr></vt:variant>
            <vt:variant><vt:i4>2</vt:i4></vt:variant>
        </vt:vector>
    </HeadingPairs>
    <TitlesOfParts>
        <vt:vector size="3" baseType="lpstr">
            <vt:lpstr>Office Theme</vt:lpstr>
            <vt:lpstr>Title Slide</vt:lpstr>
            <vt:lpstr>Agenda</vt:lpstr>
        </vt:vector>
    </TitlesOfParts>
</Properties>"#;

    let mut archive = zip_with_app_xml(app_xml);
    let props = extract_pptx_app_properties(&mut archive).unwrap();

    assert_eq!(
        props.slide_titles,
        vec!["Title Slide".to_string(), "Agenda".to_string()],
        "slide_titles must not begin with the theme name from the first HeadingPairs group"
    );
}
