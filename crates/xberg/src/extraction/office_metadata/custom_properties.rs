//! Custom properties extraction from docProps/custom.xml
//!
//! Extracts user-defined custom metadata from Office Open XML documents.

use crate::error::{Result, XbergError};
use serde_json::Value;
use std::collections::HashMap;
use std::io::Read;
use zip::ZipArchive;

/// Custom properties from docProps/custom.xml
///
/// Maps property names to their values. Values are converted to JSON types
/// based on the VT (Variant Type) specified in the XML.
#[cfg_attr(alef, alef(skip))]
pub type CustomProperties = HashMap<String, Value>;

/// Extract custom properties from an Office Open XML document
///
/// Parses `docProps/custom.xml` and extracts user-defined or application-defined
/// custom metadata.
///
/// # Arguments
///
/// * `archive` - ZIP archive containing the Office document
///
/// # Returns
///
/// Returns a `HashMap` of property names to JSON values. Supported VT types:
/// - `vt:lpwstr` / `vt:lpstr` → String
/// - `vt:i4` → Number (integer)
/// - `vt:r8` → Number (float)
/// - `vt:bool` → Boolean
/// - `vt:filetime` → String (ISO 8601 timestamp)
///
/// # Example
///
/// ```ignore
/// use xberg::extraction::office_metadata::extract_custom_properties;
/// use std::fs::File;
/// use zip::ZipArchive;
///
/// let file = File::open("document.docx")?;
/// let mut archive = ZipArchive::new(file)?;
/// let custom = extract_custom_properties(&mut archive)?;
///
/// for (name, value) in custom {
///     println!("{}: {:?}", name, value);
/// }
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[cfg_attr(alef, alef(skip))]
pub fn extract_custom_properties<R: Read + std::io::Seek>(archive: &mut ZipArchive<R>) -> Result<CustomProperties> {
    let xml_content = match super::read_zip_entry_to_string(archive, "docProps/custom.xml", "custom.xml")? {
        Some(content) => content,
        None => return Ok(HashMap::new()),
    };

    let doc = roxmltree::Document::parse(&xml_content)
        .map_err(|e| XbergError::parsing(format!("Failed to parse custom.xml: {}", e)))?;

    let root = doc.root_element();
    let mut properties = HashMap::new();

    for property_node in root.descendants().filter(|n| n.has_tag_name("property")) {
        if let Some(name) = property_node.attribute("name")
            && let Some(value) = extract_vt_value(property_node)
        {
            properties.insert(name.to_string(), value);
        }
    }

    Ok(properties)
}

/// Extract value from a VT (Variant Type) element
///
/// Handles various VT types and converts them to appropriate JSON values.
fn extract_vt_value(node: roxmltree::Node) -> Option<Value> {
    for child in node.children().filter(|n| n.is_element()) {
        let tag = child.tag_name().name();

        if tag == "vector" {
            return Some(extract_vt_vector_value(child));
        }

        if let Some(value) = convert_vt_scalar(tag, child) {
            return Some(value);
        }
    }

    None
}

/// Convert a single scalar VT element (`tag` is its local name, e.g. `"i4"`) to a JSON value.
///
/// Returns `None` for unrecognized tags or unparsable text, letting the caller decide how
/// to treat that (fall through to the next child, or skip the vector element).
fn convert_vt_scalar(tag: &str, child: roxmltree::Node) -> Option<Value> {
    match tag {
        "lpwstr" | "lpstr" => child.text().map(|s| Value::String(s.to_string())),
        "i4" | "i8" | "i1" | "i2" | "int" => child
            .text()
            .and_then(|s| s.trim().parse::<i64>().ok())
            .map(|n| Value::Number(n.into())),
        "ui1" | "ui2" | "ui4" | "ui8" | "uint" => child
            .text()
            .and_then(|s| s.trim().parse::<u64>().ok())
            .map(|n| Value::Number(n.into())),
        "r4" | "r8" | "decimal" | "cy" => child.text().and_then(|s| {
            s.trim()
                .parse::<f64>()
                .ok()
                .and_then(|f| serde_json::Number::from_f64(f).map(Value::Number))
        }),
        "bool" => child.text().and_then(|s| match s.trim().to_lowercase().as_str() {
            "true" | "1" => Some(Value::Bool(true)),
            "false" | "0" => Some(Value::Bool(false)),
            _ => None,
        }),
        // filetime/date/clsid/error/bstr are represented as opaque strings: filetime and
        // date are ISO 8601 timestamps, clsid is a GUID, error is a numeric HRESULT string.
        "filetime" | "date" | "clsid" | "error" | "bstr" => child.text().map(|s| Value::String(s.to_string())),
        _ => None,
    }
}

/// Extract a `vt:vector` element into a JSON array.
///
/// Each child of the vector carries its own VT type tag (per the OOXML docPropsVTypes
/// schema); unrecognized or unparsable entries are omitted rather than aborting the
/// whole vector.
fn extract_vt_vector_value(vector_node: roxmltree::Node) -> Value {
    let values: Vec<Value> = vector_node
        .children()
        .filter(|n| n.is_element())
        .filter_map(|item| convert_vt_scalar(item.tag_name().name(), item))
        .collect();

    Value::Array(values)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Write};

    fn create_test_zip_with_custom_xml(custom_xml: &str) -> ZipArchive<Cursor<Vec<u8>>> {
        let buffer = Vec::new();
        let cursor = Cursor::new(buffer);
        let mut zip = zip::ZipWriter::new(cursor);

        let options = zip::write::FileOptions::<()>::default().compression_method(zip::CompressionMethod::Stored);

        zip.start_file("docProps/custom.xml", options).unwrap();
        zip.write_all(custom_xml.as_bytes()).unwrap();

        let cursor = zip.finish().unwrap();
        ZipArchive::new(cursor).unwrap()
    }

    #[test]
    fn test_extract_custom_properties_string() {
        let custom_xml = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Properties xmlns="http://schemas.openxmlformats.org/officeDocument/2006/custom-properties"
            xmlns:vt="http://schemas.openxmlformats.org/officeDocument/2006/docPropsVTypes">
    <property fmtid="{D5CDD505-2E9C-101B-9397-08002B2CF9AE}" pid="2" name="ProjectName">
        <vt:lpwstr>Alpha Project</vt:lpwstr>
    </property>
</Properties>"#;

        let mut archive = create_test_zip_with_custom_xml(custom_xml);
        let props = extract_custom_properties(&mut archive).unwrap();

        assert_eq!(
            props.get("ProjectName"),
            Some(&Value::String("Alpha Project".to_string()))
        );
    }

    #[test]
    fn test_extract_custom_properties_multiple_types() {
        let custom_xml = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Properties xmlns="http://schemas.openxmlformats.org/officeDocument/2006/custom-properties"
            xmlns:vt="http://schemas.openxmlformats.org/officeDocument/2006/docPropsVTypes">
    <property fmtid="{D5CDD505-2E9C-101B-9397-08002B2CF9AE}" pid="2" name="StringProp">
        <vt:lpwstr>Test String</vt:lpwstr>
    </property>
    <property fmtid="{D5CDD505-2E9C-101B-9397-08002B2CF9AE}" pid="3" name="IntProp">
        <vt:i4>42</vt:i4>
    </property>
    <property fmtid="{D5CDD505-2E9C-101B-9397-08002B2CF9AE}" pid="4" name="BoolProp">
        <vt:bool>true</vt:bool>
    </property>
    <property fmtid="{D5CDD505-2E9C-101B-9397-08002B2CF9AE}" pid="5" name="FloatProp">
        <vt:r8>3.14159</vt:r8>
    </property>
</Properties>"#;

        let mut archive = create_test_zip_with_custom_xml(custom_xml);
        let props = extract_custom_properties(&mut archive).unwrap();

        assert_eq!(props.get("StringProp"), Some(&Value::String("Test String".to_string())));
        assert_eq!(props.get("IntProp"), Some(&Value::Number(42.into())));
        assert_eq!(props.get("BoolProp"), Some(&Value::Bool(true)));
        assert!(matches!(props.get("FloatProp"), Some(Value::Number(_))));
    }

    #[test]
    fn test_extract_custom_properties_missing_file() {
        let buffer = Vec::new();
        let cursor = Cursor::new(buffer);
        let zip = zip::ZipWriter::new(cursor);
        let cursor = zip.finish().unwrap();
        let mut archive = ZipArchive::new(cursor).unwrap();

        let props = extract_custom_properties(&mut archive).unwrap();
        assert_eq!(props, HashMap::new());
    }

    #[test]
    fn test_extract_vt_value_lpstr() {
        let xml = r#"<property xmlns:vt="http://schemas.openxmlformats.org/officeDocument/2006/docPropsVTypes">
            <vt:lpstr>Test</vt:lpstr>
        </property>"#;

        let doc = roxmltree::Document::parse(xml).unwrap();
        let value = extract_vt_value(doc.root_element()).unwrap();
        assert_eq!(value, Value::String("Test".to_string()));
    }

    #[test]
    fn test_extract_vt_value_bool_variants() {
        let xml_true1 = r#"<property xmlns:vt="http://schemas.openxmlformats.org/officeDocument/2006/docPropsVTypes">
            <vt:bool>true</vt:bool>
        </property>"#;
        let xml_true2 = r#"<property xmlns:vt="http://schemas.openxmlformats.org/officeDocument/2006/docPropsVTypes">
            <vt:bool>1</vt:bool>
        </property>"#;
        let xml_false1 = r#"<property xmlns:vt="http://schemas.openxmlformats.org/officeDocument/2006/docPropsVTypes">
            <vt:bool>false</vt:bool>
        </property>"#;
        let xml_false2 = r#"<property xmlns:vt="http://schemas.openxmlformats.org/officeDocument/2006/docPropsVTypes">
            <vt:bool>0</vt:bool>
        </property>"#;

        let doc1 = roxmltree::Document::parse(xml_true1).unwrap();
        assert_eq!(extract_vt_value(doc1.root_element()), Some(Value::Bool(true)));

        let doc2 = roxmltree::Document::parse(xml_true2).unwrap();
        assert_eq!(extract_vt_value(doc2.root_element()), Some(Value::Bool(true)));

        let doc3 = roxmltree::Document::parse(xml_false1).unwrap();
        assert_eq!(extract_vt_value(doc3.root_element()), Some(Value::Bool(false)));

        let doc4 = roxmltree::Document::parse(xml_false2).unwrap();
        assert_eq!(extract_vt_value(doc4.root_element()), Some(Value::Bool(false)));
    }
}
