//! Regression test for #237 — multiple `dc:creator` entries in `docProps/core.xml`
//! collapse to a single value.
//!
//! Dublin Core (and therefore OOXML core properties) permits `dc:creator` to
//! repeat for co-authored documents. `extract_core_properties` used
//! `parse_xml_text`, which returns only the first matching element via
//! `.find()`, silently dropping every co-author but the first.

#![cfg(feature = "office")]

use std::io::{Cursor, Write};
use xberg::extraction::office_metadata::core_properties::extract_core_properties;
use zip::ZipArchive;

fn zip_with_core_xml(core_xml: &str) -> ZipArchive<Cursor<Vec<u8>>> {
    let cursor = Cursor::new(Vec::new());
    let mut zip = zip::ZipWriter::new(cursor);
    let options = zip::write::FileOptions::<()>::default().compression_method(zip::CompressionMethod::Stored);
    zip.start_file("docProps/core.xml", options).unwrap();
    zip.write_all(core_xml.as_bytes()).unwrap();
    ZipArchive::new(zip.finish().unwrap()).unwrap()
}

#[test]
fn should_keep_all_creators_when_dc_creator_repeats() {
    let core_xml = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties"
                   xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:creator>Alice Author</dc:creator>
    <dc:creator>Bob Coauthor</dc:creator>
    <dc:creator>Carol Contributor</dc:creator>
</cp:coreProperties>"#;

    let mut archive = zip_with_core_xml(core_xml);
    let props = extract_core_properties(&mut archive).unwrap();

    assert_eq!(
        props.creator,
        Some("Alice Author; Bob Coauthor; Carol Contributor".to_string()),
        "all three dc:creator entries must be preserved, not just the first"
    );
}

#[test]
fn should_return_single_creator_unchanged_when_dc_creator_appears_once() {
    let core_xml = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties"
                   xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:creator>Solo Author</dc:creator>
</cp:coreProperties>"#;

    let mut archive = zip_with_core_xml(core_xml);
    let props = extract_core_properties(&mut archive).unwrap();

    assert_eq!(props.creator, Some("Solo Author".to_string()));
}

#[test]
fn should_skip_empty_dc_creator_entries_when_joining() {
    let core_xml = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties"
                   xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:creator>Alice Author</dc:creator>
    <dc:creator></dc:creator>
    <dc:creator>Bob Coauthor</dc:creator>
</cp:coreProperties>"#;

    let mut archive = zip_with_core_xml(core_xml);
    let props = extract_core_properties(&mut archive).unwrap();

    assert_eq!(props.creator, Some("Alice Author; Bob Coauthor".to_string()));
}
