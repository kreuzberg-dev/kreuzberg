//! Core properties extraction from docProps/core.xml
//!
//! Extracts Dublin Core metadata from Office Open XML documents.

use crate::error::{Result, XbergError};
use std::io::Read;
use zip::ZipArchive;

/// Dublin Core metadata from docProps/core.xml
///
/// Contains standard metadata fields defined by the Dublin Core standard
/// and Office-specific extensions.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CoreProperties {
    /// Document title
    pub title: Option<String>,
    /// Document subject/topic
    pub subject: Option<String>,
    /// Document creator/author
    pub creator: Option<String>,
    /// Keywords or tags
    pub keywords: Option<String>,
    /// Document description/abstract
    pub description: Option<String>,
    /// User who last modified the document
    pub last_modified_by: Option<String>,
    /// Revision number
    pub revision: Option<String>,
    /// Creation timestamp (ISO 8601)
    pub created: Option<String>,
    /// Last modification timestamp (ISO 8601)
    pub modified: Option<String>,
    /// Document category
    pub category: Option<String>,
    /// Content status (Draft, Final, etc.)
    pub content_status: Option<String>,
    /// Document language
    pub language: Option<String>,
    /// Unique identifier
    pub identifier: Option<String>,
    /// Document version
    pub version: Option<String>,
    /// Last print timestamp (ISO 8601)
    pub last_printed: Option<String>,
}

/// Extract core properties from an Office Open XML document
///
/// Parses `docProps/core.xml` from the ZIP archive and extracts Dublin Core metadata.
///
/// # Arguments
///
/// * `archive` - ZIP archive containing the Office document
///
/// # Returns
///
/// Returns `CoreProperties` with extracted metadata. Fields that are not present
/// in the document will be `None`.
///
/// # Errors
///
/// Returns an error if:
/// - The ZIP archive cannot be read
/// - The core.xml file is malformed
/// - XML parsing fails
///
/// # Example
///
/// ```ignore
/// use xberg::extraction::office_metadata::extract_core_properties;
/// use std::fs::File;
/// use zip::ZipArchive;
///
/// let file = File::open("document.docx")?;
/// let mut archive = ZipArchive::new(file)?;
/// let core = extract_core_properties(&mut archive)?;
///
/// println!("Title: {:?}", core.title);
/// println!("Creator: {:?}", core.creator);
/// println!("Created: {:?}", core.created);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[cfg_attr(alef, alef(skip))]
pub fn extract_core_properties<R: Read + std::io::Seek>(archive: &mut ZipArchive<R>) -> Result<CoreProperties> {
    let xml_content = match super::read_zip_entry_to_string(archive, "docProps/core.xml", "core.xml")? {
        Some(content) => content,
        None => return Ok(CoreProperties::default()),
    };

    let doc = roxmltree::Document::parse(&xml_content)
        .map_err(|e| XbergError::parsing(format!("Failed to parse core.xml: {}", e)))?;

    let root = doc.root_element();

    let title = super::parse_xml_text(root, "title");
    let subject = super::parse_xml_text(root, "subject");
    // dc:creator may legally repeat for co-authored documents (Dublin Core); keep them all
    // instead of silently dropping every author but the first (#237).
    let creator = super::parse_xml_text_joined(root, "creator", "; ");
    let description = super::parse_xml_text(root, "description");
    let language = super::parse_xml_text(root, "language");
    let identifier = super::parse_xml_text(root, "identifier");

    let keywords = super::parse_xml_text(root, "keywords");
    let last_modified_by = super::parse_xml_text(root, "lastModifiedBy");
    let revision = super::parse_xml_text(root, "revision");
    let category = super::parse_xml_text(root, "category");
    let content_status = super::parse_xml_text(root, "contentStatus");
    let version = super::parse_xml_text(root, "version");

    let created = super::parse_xml_text(root, "created");
    let modified = super::parse_xml_text(root, "modified");
    let last_printed = super::parse_xml_text(root, "lastPrinted");

    Ok(CoreProperties {
        title,
        subject,
        creator,
        keywords,
        description,
        last_modified_by,
        revision,
        created,
        modified,
        category,
        content_status,
        language,
        identifier,
        version,
        last_printed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Write};

    fn create_test_zip_with_core_xml(core_xml: &str) -> ZipArchive<Cursor<Vec<u8>>> {
        let buffer = Vec::new();
        let cursor = Cursor::new(buffer);
        let mut zip = zip::ZipWriter::new(cursor);

        let options = zip::write::FileOptions::<()>::default().compression_method(zip::CompressionMethod::Stored);

        zip.start_file("docProps/core.xml", options).unwrap();
        zip.write_all(core_xml.as_bytes()).unwrap();

        let cursor = zip.finish().unwrap();
        ZipArchive::new(cursor).unwrap()
    }

    #[test]
    fn test_extract_core_properties_full() {
        let core_xml = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties"
                   xmlns:dc="http://purl.org/dc/elements/1.1/"
                   xmlns:dcterms="http://purl.org/dc/terms/"
                   xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance">
    <dc:title>Test Document</dc:title>
    <dc:subject>Testing</dc:subject>
    <dc:creator>John Doe</dc:creator>
    <cp:keywords>test, metadata</cp:keywords>
    <dc:description>A test document</dc:description>
    <cp:lastModifiedBy>Jane Doe</cp:lastModifiedBy>
    <cp:revision>5</cp:revision>
    <dcterms:created xsi:type="dcterms:W3CDTF">2024-01-01T10:00:00Z</dcterms:created>
    <dcterms:modified xsi:type="dcterms:W3CDTF">2024-01-02T15:30:00Z</dcterms:modified>
    <cp:category>Documents</cp:category>
    <cp:contentStatus>Final</cp:contentStatus>
    <dc:language>en-US</dc:language>
</cp:coreProperties>"#;

        let mut archive = create_test_zip_with_core_xml(core_xml);
        let props = extract_core_properties(&mut archive).unwrap();

        assert_eq!(props.title, Some("Test Document".to_string()));
        assert_eq!(props.subject, Some("Testing".to_string()));
        assert_eq!(props.creator, Some("John Doe".to_string()));
        assert_eq!(props.keywords, Some("test, metadata".to_string()));
        assert_eq!(props.description, Some("A test document".to_string()));
        assert_eq!(props.last_modified_by, Some("Jane Doe".to_string()));
        assert_eq!(props.revision, Some("5".to_string()));
        assert_eq!(props.created, Some("2024-01-01T10:00:00Z".to_string()));
        assert_eq!(props.modified, Some("2024-01-02T15:30:00Z".to_string()));
        assert_eq!(props.category, Some("Documents".to_string()));
        assert_eq!(props.content_status, Some("Final".to_string()));
        assert_eq!(props.language, Some("en-US".to_string()));
    }

    #[test]
    fn test_extract_core_properties_minimal() {
        let core_xml = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties"
                   xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:creator>Alice</dc:creator>
</cp:coreProperties>"#;

        let mut archive = create_test_zip_with_core_xml(core_xml);
        let props = extract_core_properties(&mut archive).unwrap();

        assert_eq!(props.creator, Some("Alice".to_string()));
        assert_eq!(props.title, None);
        assert_eq!(props.keywords, None);
    }

    #[test]
    fn test_extract_core_properties_empty_elements() {
        let core_xml = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties"
                   xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:title></dc:title>
    <dc:creator>Bob</dc:creator>
</cp:coreProperties>"#;

        let mut archive = create_test_zip_with_core_xml(core_xml);
        let props = extract_core_properties(&mut archive).unwrap();

        assert_eq!(props.title, None);
        assert_eq!(props.creator, Some("Bob".to_string()));
    }

    #[test]
    fn test_extract_core_properties_missing_file() {
        let buffer = Vec::new();
        let cursor = Cursor::new(buffer);
        let zip = zip::ZipWriter::new(cursor);
        let cursor = zip.finish().unwrap();
        let mut archive = ZipArchive::new(cursor).unwrap();

        let props = extract_core_properties(&mut archive).unwrap();
        assert_eq!(props, CoreProperties::default());
    }

    #[test]
    fn test_extract_core_properties_malformed_xml() {
        let core_xml = "not valid xml <";
        let mut archive = create_test_zip_with_core_xml(core_xml);

        let result = extract_core_properties(&mut archive);
        assert!(result.is_err());
    }
}
