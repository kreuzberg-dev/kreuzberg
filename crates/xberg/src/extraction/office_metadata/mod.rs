//! Office Open XML metadata extraction
//!
//! This module provides functionality to extract comprehensive metadata from Office Open XML
//! documents (DOCX, XLSX, PPTX) by parsing the `docProps/core.xml`, `docProps/app.xml`,
//! and `docProps/custom.xml` files within the ZIP container.
//!
//! # Overview
//!
//! Office documents store metadata in three XML files:
//! - `docProps/core.xml` - Dublin Core metadata (title, creator, dates, keywords, etc.)
//! - `docProps/app.xml` - Application-specific properties (page count, word count, etc.)
//! - `docProps/custom.xml` - Custom properties defined by users or applications
//!
//! # Example
//!
//! ```ignore
//! use xberg::extraction::office_metadata::{extract_core_properties, extract_docx_app_properties};
//! use std::fs::File;
//! use zip::ZipArchive;
//!
//! let file = File::open("document.docx")?;
//! let mut archive = ZipArchive::new(file)?;
//!
//! // Extract core properties
//! let core = extract_core_properties(&mut archive)?;
//! println!("Title: {:?}", core.title);
//! println!("Created: {:?}", core.created);
//!
//! // Extract DOCX app properties
//! let app = extract_docx_app_properties(&mut archive)?;
//! println!("Word count: {:?}", app.words);
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

pub mod app_properties;
pub mod core_properties;
pub mod custom_properties;
pub mod odt_properties;

pub use app_properties::DocxAppProperties;
pub use core_properties::CoreProperties;
pub use custom_properties::CustomProperties;
pub use odt_properties::OdtProperties;

#[cfg(any(feature = "excel", feature = "excel-wasm"))]
pub(crate) use app_properties::extract_xlsx_app_properties;
pub(crate) use app_properties::{extract_docx_app_properties, extract_pptx_app_properties};
pub(crate) use core_properties::extract_core_properties;
pub(crate) use custom_properties::extract_custom_properties;
pub(crate) use odt_properties::extract_odt_properties;

use crate::error::{Result, XbergError};
use roxmltree::Node;
use std::io::Read;
use zip::ZipArchive;

/// Read a ZIP archive entry to a `String`.
///
/// Returns `Ok(Some(content))` if the entry exists and was read successfully,
/// `Ok(None)` if the entry does not exist in the archive, or an error if the
/// entry exists but cannot be read.
///
/// # Arguments
///
/// * `archive` - ZIP archive to read from
/// * `entry_path` - Path of the entry within the archive (e.g. `"docProps/core.xml"`)
/// * `display_name` - Short name used in error messages (e.g. `"core.xml"`)
pub(crate) fn read_zip_entry_to_string<R: Read + std::io::Seek>(
    archive: &mut ZipArchive<R>,
    entry_path: &str,
    display_name: &str,
) -> Result<Option<String>> {
    match archive.by_name(entry_path) {
        Ok(mut file) => {
            let mut content = String::new();
            file.read_to_string(&mut content)
                .map_err(|e| XbergError::parsing(format!("Failed to read {display_name}: {e}")))?;
            Ok(Some(content))
        }
        Err(_) => Ok(None),
    }
}

/// Parse text content from an XML element by tag name
///
/// Returns the text content if the element exists and has non-empty text.
pub(crate) fn parse_xml_text(node: Node, name: &str) -> Option<String> {
    node.descendants()
        .find(|n| n.has_tag_name(name))
        .and_then(|n| n.text())
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(String::from)
}

/// Parse integer content from an XML element by tag name
///
/// Returns the parsed integer if the element exists and contains valid integer text.
pub(crate) fn parse_xml_int(node: Node, name: &str) -> Option<i32> {
    node.descendants()
        .find(|n| n.has_tag_name(name))
        .and_then(|n| n.text())
        .and_then(|s| s.trim().parse::<i32>().ok())
}

/// Parse text content from every XML element matching a tag name, joined with `separator`.
///
/// Unlike [`parse_xml_text`] (which returns only the first match via `.find()`), this
/// collects every matching descendant's non-empty trimmed text. Used for fields such as
/// `dc:creator` that Dublin Core / OOXML permit to repeat (e.g. co-authored documents),
/// where keeping only the first entry would silently drop the rest.
pub(crate) fn parse_xml_text_joined(node: Node, name: &str, separator: &str) -> Option<String> {
    let values: Vec<String> = node
        .descendants()
        .filter(|n| n.has_tag_name(name))
        .filter_map(|n| n.text())
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect();

    if values.is_empty() {
        None
    } else {
        Some(values.join(separator))
    }
}

/// Parse boolean content from an XML element by tag name
///
/// Handles "true"/"false" string values and converts to boolean.
pub(crate) fn parse_xml_bool(node: Node, name: &str) -> Option<bool> {
    node.descendants()
        .find(|n| n.has_tag_name(name))
        .and_then(|n| n.text())
        .map(|s| s.trim())
        .and_then(|s| match s.to_lowercase().as_str() {
            "true" => Some(true),
            "false" => Some(false),
            _ => None,
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_xml_text() {
        let xml = r#"<root><title>Test Document</title></root>"#;
        let doc = roxmltree::Document::parse(xml).unwrap();
        let root = doc.root_element();

        assert_eq!(parse_xml_text(root, "title"), Some("Test Document".to_string()));
        assert_eq!(parse_xml_text(root, "missing"), None);
    }

    #[test]
    fn test_parse_xml_text_empty() {
        let xml = r#"<root><title></title></root>"#;
        let doc = roxmltree::Document::parse(xml).unwrap();
        let root = doc.root_element();

        assert_eq!(parse_xml_text(root, "title"), None);
    }

    #[test]
    fn test_parse_xml_text_joined_multiple() {
        let xml = r#"<root><creator>Alice</creator><creator>Bob</creator></root>"#;
        let doc = roxmltree::Document::parse(xml).unwrap();
        let root = doc.root_element();

        assert_eq!(
            parse_xml_text_joined(root, "creator", "; "),
            Some("Alice; Bob".to_string())
        );
    }

    #[test]
    fn test_parse_xml_text_joined_missing() {
        let xml = r#"<root></root>"#;
        let doc = roxmltree::Document::parse(xml).unwrap();
        let root = doc.root_element();

        assert_eq!(parse_xml_text_joined(root, "creator", "; "), None);
    }

    #[test]
    fn test_parse_xml_int() {
        let xml = r#"<root><count>42</count></root>"#;
        let doc = roxmltree::Document::parse(xml).unwrap();
        let root = doc.root_element();

        assert_eq!(parse_xml_int(root, "count"), Some(42));
        assert_eq!(parse_xml_int(root, "missing"), None);
    }

    #[test]
    fn test_parse_xml_bool() {
        let xml = r#"<root><flag>true</flag><other>false</other></root>"#;
        let doc = roxmltree::Document::parse(xml).unwrap();
        let root = doc.root_element();

        assert_eq!(parse_xml_bool(root, "flag"), Some(true));
        assert_eq!(parse_xml_bool(root, "other"), Some(false));
        assert_eq!(parse_xml_bool(root, "missing"), None);
    }
}
