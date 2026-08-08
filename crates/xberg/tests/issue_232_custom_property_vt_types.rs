//! Regression test for #232 — `extract_vt_value` in `custom_properties.rs` only handles
//! `lpwstr`/`lpstr`, `i4`, `r8`, `bool`, and `filetime`; every other VT type (`vt:vector`,
//! `vt:date`, `vt:i8`, `vt:ui4`, `vt:decimal`, `vt:cy`, `vt:clsid`) falls through to
//! `_ => continue` and the whole property is dropped — no key, no placeholder.
//!
//! Custom properties are where classification labels, matter numbers and retention
//! codes live, so silently dropping unrecognized-but-common VT types loses data.

#![cfg(feature = "office")]

use std::io::{Cursor, Write};
use xberg::extraction::office_metadata::custom_properties::extract_custom_properties;
use zip::ZipArchive;

fn zip_with_custom_xml(custom_xml: &str) -> ZipArchive<Cursor<Vec<u8>>> {
    let cursor = Cursor::new(Vec::new());
    let mut zip = zip::ZipWriter::new(cursor);
    let options = zip::write::FileOptions::<()>::default().compression_method(zip::CompressionMethod::Stored);
    zip.start_file("docProps/custom.xml", options).unwrap();
    zip.write_all(custom_xml.as_bytes()).unwrap();
    ZipArchive::new(zip.finish().unwrap()).unwrap()
}

const PROPS_HEADER: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Properties xmlns="http://schemas.openxmlformats.org/officeDocument/2006/custom-properties"
            xmlns:vt="http://schemas.openxmlformats.org/officeDocument/2006/docPropsVTypes">"#;

#[test]
fn should_extract_i8_property() {
    let xml = format!(
        r#"{PROPS_HEADER}
    <property fmtid="{{D5CDD505-2E9C-101B-9397-08002B2CF9AE}}" pid="2" name="BigInt">
        <vt:i8>9223372036854775807</vt:i8>
    </property>
</Properties>"#
    );

    let mut archive = zip_with_custom_xml(&xml);
    let props = extract_custom_properties(&mut archive).unwrap();

    assert_eq!(props.get("BigInt"), Some(&serde_json::json!(9223372036854775807i64)));
}

#[test]
fn should_extract_ui4_property() {
    let xml = format!(
        r#"{PROPS_HEADER}
    <property fmtid="{{D5CDD505-2E9C-101B-9397-08002B2CF9AE}}" pid="2" name="UnsignedCount">
        <vt:ui4>4000000000</vt:ui4>
    </property>
</Properties>"#
    );

    let mut archive = zip_with_custom_xml(&xml);
    let props = extract_custom_properties(&mut archive).unwrap();

    assert_eq!(props.get("UnsignedCount"), Some(&serde_json::json!(4000000000u64)));
}

#[test]
fn should_extract_decimal_property() {
    let xml = format!(
        r#"{PROPS_HEADER}
    <property fmtid="{{D5CDD505-2E9C-101B-9397-08002B2CF9AE}}" pid="2" name="RetentionScore">
        <vt:decimal>123.45</vt:decimal>
    </property>
</Properties>"#
    );

    let mut archive = zip_with_custom_xml(&xml);
    let props = extract_custom_properties(&mut archive).unwrap();

    assert_eq!(props.get("RetentionScore"), Some(&serde_json::json!(123.45)));
}

#[test]
fn should_extract_cy_property() {
    let xml = format!(
        r#"{PROPS_HEADER}
    <property fmtid="{{D5CDD505-2E9C-101B-9397-08002B2CF9AE}}" pid="2" name="Budget">
        <vt:cy>1999.99</vt:cy>
    </property>
</Properties>"#
    );

    let mut archive = zip_with_custom_xml(&xml);
    let props = extract_custom_properties(&mut archive).unwrap();

    assert_eq!(props.get("Budget"), Some(&serde_json::json!(1999.99)));
}

#[test]
fn should_extract_clsid_property_as_string() {
    let xml = format!(
        r#"{PROPS_HEADER}
    <property fmtid="{{D5CDD505-2E9C-101B-9397-08002B2CF9AE}}" pid="2" name="ClassId">
        <vt:clsid>{{46180C4D-C7A9-404E-8F88-C118F1BAA112}}</vt:clsid>
    </property>
</Properties>"#
    );

    let mut archive = zip_with_custom_xml(&xml);
    let props = extract_custom_properties(&mut archive).unwrap();

    assert_eq!(
        props.get("ClassId"),
        Some(&serde_json::json!("{46180C4D-C7A9-404E-8F88-C118F1BAA112}"))
    );
}

#[test]
fn should_extract_date_property_as_string() {
    let xml = format!(
        r#"{PROPS_HEADER}
    <property fmtid="{{D5CDD505-2E9C-101B-9397-08002B2CF9AE}}" pid="2" name="ReviewDate">
        <vt:date>2024-03-15T00:00:00Z</vt:date>
    </property>
</Properties>"#
    );

    let mut archive = zip_with_custom_xml(&xml);
    let props = extract_custom_properties(&mut archive).unwrap();

    assert_eq!(
        props.get("ReviewDate"),
        Some(&serde_json::json!("2024-03-15T00:00:00Z"))
    );
}

#[test]
fn should_extract_vector_property_as_array() {
    let xml = format!(
        r#"{PROPS_HEADER}
    <property fmtid="{{D5CDD505-2E9C-101B-9397-08002B2CF9AE}}" pid="2" name="Tags">
        <vt:vector size="3" baseType="lpwstr">
            <vt:lpwstr>Confidential</vt:lpwstr>
            <vt:lpwstr>Matter-4471</vt:lpwstr>
            <vt:lpwstr>Retention-7yr</vt:lpwstr>
        </vt:vector>
    </property>
</Properties>"#
    );

    let mut archive = zip_with_custom_xml(&xml);
    let props = extract_custom_properties(&mut archive).unwrap();

    assert_eq!(
        props.get("Tags"),
        Some(&serde_json::json!(["Confidential", "Matter-4471", "Retention-7yr"]))
    );
}

#[test]
fn should_still_extract_previously_supported_types() {
    let xml = format!(
        r#"{PROPS_HEADER}
    <property fmtid="{{D5CDD505-2E9C-101B-9397-08002B2CF9AE}}" pid="2" name="StringProp">
        <vt:lpwstr>Test String</vt:lpwstr>
    </property>
    <property fmtid="{{D5CDD505-2E9C-101B-9397-08002B2CF9AE}}" pid="3" name="IntProp">
        <vt:i4>42</vt:i4>
    </property>
</Properties>"#
    );

    let mut archive = zip_with_custom_xml(&xml);
    let props = extract_custom_properties(&mut archive).unwrap();

    assert_eq!(props.get("StringProp"), Some(&serde_json::json!("Test String")));
    assert_eq!(props.get("IntProp"), Some(&serde_json::json!(42)));
}
