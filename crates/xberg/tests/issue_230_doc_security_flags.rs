//! Regression test for #230: the OOXML `DocSecurity` document-protection flag was parsed
//! into `*AppProperties::doc_security` and then discarded — for XLSX and PPTX entirely
//! (neither `FormatMetadata::Excel` nor `FormatMetadata::Pptx` carries app properties), and
//! for DOCX only as an opaque integer a consumer would have to decode by hand.
//!
//! These tests extract synthesized documents end to end and assert that both the raw value
//! and the decoded ECMA-376 flags reach `ExtractedDocument::metadata.additional`. The bit
//! semantics themselves are unit-tested next to the decoder in
//! `extraction::office_metadata::app_properties`.
//!
//! Value shapes differ by format because the metadata carriers do: the DOCX extractor
//! populates a `serde_json::Value` map directly (numbers and booleans stay typed), while
//! the XLSX and PPTX extractors funnel their office metadata through a
//! `HashMap<String, String>` before it is JSON-ified, so every entry — pre-existing ones
//! included — arrives as a JSON string.

#![cfg(feature = "office")]

mod helpers;

const PASSWORD_PROTECTED: &str = "doc_security_password_protected";
const READ_ONLY_RECOMMENDED: &str = "doc_security_read_only_recommended";
const READ_ONLY_ENFORCED: &str = "doc_security_read_only_enforced";
const LOCKED_FOR_ANNOTATIONS: &str = "doc_security_locked_for_annotations";

/// Build a `docProps/app.xml` body, optionally carrying a `DocSecurity` element.
fn app_xml(application: &str, doc_security: Option<i32>) -> Vec<u8> {
    let security = doc_security.map_or_else(String::new, |raw| format!("<DocSecurity>{raw}</DocSecurity>"));
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Properties xmlns="http://schemas.openxmlformats.org/officeDocument/2006/extended-properties"
            xmlns:vt="http://schemas.openxmlformats.org/officeDocument/2006/docPropsVTypes">
  <Application>{application}</Application>{security}
</Properties>"#
    )
    .into_bytes()
}

mod docx {
    use std::io::{Cursor, Write};
    use zip::CompressionMethod;
    use zip::write::{FileOptions, ZipWriter};

    use crate::helpers::extract_bytes_document;
    use crate::{LOCKED_FOR_ANNOTATIONS, PASSWORD_PROTECTED, READ_ONLY_ENFORCED, READ_ONLY_RECOMMENDED, app_xml};
    use xberg::ExtractionConfig;

    const DOCX_MIME: &str = "application/vnd.openxmlformats-officedocument.wordprocessingml.document";

    /// Minimal in-memory DOCX whose `docProps/app.xml` declares `doc_security`.
    fn build_docx(doc_security: Option<i32>) -> Vec<u8> {
        let mut cursor = Cursor::new(Vec::new());
        {
            let mut zip = ZipWriter::new(&mut cursor);
            let options: FileOptions<()> = FileOptions::default().compression_method(CompressionMethod::Stored);

            zip.start_file("[Content_Types].xml", options).expect("zip write");
            zip.write_all(br#"<?xml version="1.0" encoding="UTF-8"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
  <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
  <Default Extension="xml" ContentType="application/xml"/>
  <Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>
  <Override PartName="/docProps/app.xml" ContentType="application/vnd.openxmlformats-officedocument.extended-properties+xml"/>
</Types>"#).expect("zip write");

            zip.start_file("_rels/.rels", options).expect("zip write");
            zip.write_all(br#"<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/>
  <Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/extended-properties" Target="docProps/app.xml"/>
</Relationships>"#).expect("zip write");

            zip.start_file("word/_rels/document.xml.rels", options)
                .expect("zip write");
            zip.write_all(
                br#"<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"/>"#,
            )
            .expect("zip write");

            zip.start_file("docProps/app.xml", options).expect("zip write");
            zip.write_all(&app_xml("Microsoft Office Word", doc_security))
                .expect("zip write");

            zip.start_file("word/document.xml", options).expect("zip write");
            zip.write_all(
                br#"<?xml version="1.0" encoding="UTF-8"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:body><w:p><w:r><w:t>Protected report body</w:t></w:r></w:p></w:body>
</w:document>"#,
            )
            .expect("zip write");

            zip.finish().expect("zip finish");
        }
        cursor.into_inner()
    }

    #[tokio::test]
    async fn should_surface_read_only_enforced_doc_security_in_docx_metadata() {
        let bytes = build_docx(Some(4));
        let result = extract_bytes_document(&bytes, DOCX_MIME, &ExtractionConfig::default())
            .await
            .expect("extraction should succeed");
        let additional = &result.metadata.additional;

        assert_eq!(
            additional.get("doc_security"),
            Some(&serde_json::json!(4)),
            "raw DocSecurity value must be surfaced; additional = {additional:?}"
        );
        assert_eq!(additional.get(PASSWORD_PROTECTED), Some(&serde_json::json!(false)));
        assert_eq!(additional.get(READ_ONLY_RECOMMENDED), Some(&serde_json::json!(false)));
        assert_eq!(additional.get(READ_ONLY_ENFORCED), Some(&serde_json::json!(true)));
        assert_eq!(additional.get(LOCKED_FOR_ANNOTATIONS), Some(&serde_json::json!(false)));
    }

    #[tokio::test]
    async fn should_surface_combined_password_and_read_only_recommended_bits_in_docx_metadata() {
        // 3 = 1 (password protected) + 2 (read-only recommended). Pins the two adjacent,
        // easily transposed low bits against a real extraction, not just the decoder.
        let bytes = build_docx(Some(3));
        let result = extract_bytes_document(&bytes, DOCX_MIME, &ExtractionConfig::default())
            .await
            .expect("extraction should succeed");
        let additional = &result.metadata.additional;

        assert_eq!(additional.get("doc_security"), Some(&serde_json::json!(3)));
        assert_eq!(additional.get(PASSWORD_PROTECTED), Some(&serde_json::json!(true)));
        assert_eq!(additional.get(READ_ONLY_RECOMMENDED), Some(&serde_json::json!(true)));
        assert_eq!(additional.get(READ_ONLY_ENFORCED), Some(&serde_json::json!(false)));
        assert_eq!(additional.get(LOCKED_FOR_ANNOTATIONS), Some(&serde_json::json!(false)));
    }

    #[tokio::test]
    async fn should_surface_all_false_flags_when_docx_declares_zero_doc_security() {
        // 0 is not the same as absent: the document explicitly declares no restrictions.
        let bytes = build_docx(Some(0));
        let result = extract_bytes_document(&bytes, DOCX_MIME, &ExtractionConfig::default())
            .await
            .expect("extraction should succeed");
        let additional = &result.metadata.additional;

        assert_eq!(additional.get("doc_security"), Some(&serde_json::json!(0)));
        assert_eq!(additional.get(PASSWORD_PROTECTED), Some(&serde_json::json!(false)));
        assert_eq!(additional.get(READ_ONLY_RECOMMENDED), Some(&serde_json::json!(false)));
        assert_eq!(additional.get(READ_ONLY_ENFORCED), Some(&serde_json::json!(false)));
        assert_eq!(additional.get(LOCKED_FOR_ANNOTATIONS), Some(&serde_json::json!(false)));
    }

    #[tokio::test]
    async fn should_omit_doc_security_keys_when_docx_declares_no_doc_security_element() {
        let bytes = build_docx(None);
        let result = extract_bytes_document(&bytes, DOCX_MIME, &ExtractionConfig::default())
            .await
            .expect("extraction should succeed");
        let additional = &result.metadata.additional;

        assert_eq!(additional.get("doc_security"), None);
        assert_eq!(additional.get(PASSWORD_PROTECTED), None);
        assert_eq!(additional.get(READ_ONLY_RECOMMENDED), None);
        assert_eq!(additional.get(READ_ONLY_ENFORCED), None);
        assert_eq!(additional.get(LOCKED_FOR_ANNOTATIONS), None);
    }
}

#[cfg(feature = "excel")]
mod xlsx {
    use std::io::Write;
    use zip::write::{SimpleFileOptions, ZipWriter};

    use crate::helpers::extract_bytes_document;
    use crate::{LOCKED_FOR_ANNOTATIONS, PASSWORD_PROTECTED, READ_ONLY_ENFORCED, READ_ONLY_RECOMMENDED, app_xml};
    use xberg::ExtractionConfig;

    const XLSX_MIME: &str = "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet";

    /// Minimal in-memory single-sheet XLSX whose `docProps/app.xml` declares `doc_security`.
    fn build_xlsx(doc_security: Option<i32>) -> Vec<u8> {
        let mut buffer = Vec::new();
        {
            let mut zip = ZipWriter::new(std::io::Cursor::new(&mut buffer));
            let options = SimpleFileOptions::default();

            zip.start_file("[Content_Types].xml", options).expect("zip write");
            zip.write_all(
                br#"<?xml version="1.0" encoding="UTF-8"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
  <Default Extension="xml" ContentType="application/xml"/>
  <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
  <Override PartName="/xl/workbook.xml"
    ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/>
  <Override PartName="/xl/worksheets/sheet1.xml"
    ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>
</Types>"#,
            )
            .expect("zip write");

            zip.start_file("_rels/.rels", options).expect("zip write");
            zip.write_all(
                br#"<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1"
    Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument"
    Target="xl/workbook.xml"/>
</Relationships>"#,
            )
            .expect("zip write");

            zip.start_file("docProps/app.xml", options).expect("zip write");
            zip.write_all(&app_xml("Microsoft Excel", doc_security))
                .expect("zip write");

            zip.start_file("xl/workbook.xml", options).expect("zip write");
            zip.write_all(
                br#"<?xml version="1.0" encoding="UTF-8"?>
<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"
          xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
  <sheets><sheet name="Sheet1" sheetId="1" r:id="rId1"/></sheets>
</workbook>"#,
            )
            .expect("zip write");

            zip.start_file("xl/_rels/workbook.xml.rels", options)
                .expect("zip write");
            zip.write_all(
                br#"<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1"
    Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet"
    Target="worksheets/sheet1.xml"/>
</Relationships>"#,
            )
            .expect("zip write");

            zip.start_file("xl/worksheets/sheet1.xml", options).expect("zip write");
            zip.write_all(
                br#"<?xml version="1.0" encoding="UTF-8"?>
<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">
  <dimension ref="A1"/>
  <sheetData><row r="1"><c r="A1" t="inlineStr"><is><t>Hi</t></is></c></row></sheetData>
</worksheet>"#,
            )
            .expect("zip write");

            zip.finish().expect("zip finish");
        }
        buffer
    }

    #[tokio::test]
    async fn should_surface_read_only_recommended_doc_security_in_xlsx_metadata() {
        let bytes = build_xlsx(Some(2));
        let result = extract_bytes_document(&bytes, XLSX_MIME, &ExtractionConfig::default())
            .await
            .expect("extraction should succeed");
        let additional = &result.metadata.additional;

        assert_eq!(
            additional.get("doc_security"),
            Some(&serde_json::json!("2")),
            "raw DocSecurity value must be surfaced; additional = {additional:?}"
        );
        assert_eq!(additional.get(PASSWORD_PROTECTED), Some(&serde_json::json!("false")));
        assert_eq!(additional.get(READ_ONLY_RECOMMENDED), Some(&serde_json::json!("true")));
        assert_eq!(additional.get(READ_ONLY_ENFORCED), Some(&serde_json::json!("false")));
        assert_eq!(
            additional.get(LOCKED_FOR_ANNOTATIONS),
            Some(&serde_json::json!("false"))
        );
    }

    #[tokio::test]
    async fn should_omit_doc_security_keys_when_xlsx_declares_no_doc_security_element() {
        let bytes = build_xlsx(None);
        let result = extract_bytes_document(&bytes, XLSX_MIME, &ExtractionConfig::default())
            .await
            .expect("extraction should succeed");
        let additional = &result.metadata.additional;

        assert_eq!(additional.get("doc_security"), None);
        assert_eq!(additional.get(READ_ONLY_RECOMMENDED), None);
    }
}

mod pptx {
    use std::io::Write;
    use zip::write::{SimpleFileOptions, ZipWriter};

    use crate::helpers::extract_bytes_document;
    use crate::{LOCKED_FOR_ANNOTATIONS, PASSWORD_PROTECTED, READ_ONLY_ENFORCED, READ_ONLY_RECOMMENDED, app_xml};
    use xberg::ExtractionConfig;

    const PPTX_MIME: &str = "application/vnd.openxmlformats-officedocument.presentationml.presentation";

    /// Minimal in-memory single-slide PPTX whose `docProps/app.xml` declares `doc_security`.
    fn build_pptx(doc_security: Option<i32>) -> Vec<u8> {
        let mut buffer = Vec::new();
        {
            let mut zip = ZipWriter::new(std::io::Cursor::new(&mut buffer));
            let options = SimpleFileOptions::default();

            zip.start_file("[Content_Types].xml", options).expect("zip write");
            zip.write_all(br#"<?xml version="1.0" encoding="UTF-8"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
  <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
  <Default Extension="xml" ContentType="application/xml"/>
  <Override PartName="/ppt/presentation.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml"/>
  <Override PartName="/ppt/slides/slide1.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slide+xml"/>
</Types>"#).expect("zip write");

            zip.start_file("_rels/.rels", options).expect("zip write");
            zip.write_all(br#"<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="ppt/presentation.xml"/>
</Relationships>"#).expect("zip write");

            zip.start_file("docProps/app.xml", options).expect("zip write");
            zip.write_all(&app_xml("Microsoft Office PowerPoint", doc_security))
                .expect("zip write");

            zip.start_file("ppt/presentation.xml", options).expect("zip write");
            zip.write_all(
                br#"<?xml version="1.0" encoding="UTF-8"?>
<p:presentation xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"
                xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
  <p:sldIdLst><p:sldId id="256" r:id="rId2"/></p:sldIdLst>
</p:presentation>"#,
            )
            .expect("zip write");

            zip.start_file("ppt/_rels/presentation.xml.rels", options)
                .expect("zip write");
            zip.write_all(br#"<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide" Target="slides/slide1.xml"/>
</Relationships>"#).expect("zip write");

            zip.start_file("ppt/slides/slide1.xml", options).expect("zip write");
            zip.write_all(
                br#"<?xml version="1.0" encoding="UTF-8"?>
<p:sld xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"
       xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main">
  <p:cSld><p:spTree>
    <p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr>
    <p:grpSpPr/>
    <p:sp>
      <p:nvSpPr><p:cNvPr id="2" name="Title"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr>
      <p:spPr/>
      <p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:t>Confidential deck</a:t></a:r></a:p></p:txBody>
    </p:sp>
  </p:spTree></p:cSld>
</p:sld>"#,
            )
            .expect("zip write");

            zip.start_file("ppt/slides/_rels/slide1.xml.rels", options)
                .expect("zip write");
            zip.write_all(
                br#"<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"/>"#,
            )
            .expect("zip write");

            zip.finish().expect("zip finish");
        }
        buffer
    }

    #[tokio::test]
    async fn should_surface_locked_for_annotations_doc_security_in_pptx_metadata() {
        let bytes = build_pptx(Some(8));
        let result = extract_bytes_document(&bytes, PPTX_MIME, &ExtractionConfig::default())
            .await
            .expect("extraction should succeed");
        let additional = &result.metadata.additional;

        assert_eq!(
            additional.get("doc_security"),
            Some(&serde_json::json!("8")),
            "raw DocSecurity value must be surfaced; additional = {additional:?}"
        );
        assert_eq!(additional.get(PASSWORD_PROTECTED), Some(&serde_json::json!("false")));
        assert_eq!(additional.get(READ_ONLY_RECOMMENDED), Some(&serde_json::json!("false")));
        assert_eq!(additional.get(READ_ONLY_ENFORCED), Some(&serde_json::json!("false")));
        assert_eq!(additional.get(LOCKED_FOR_ANNOTATIONS), Some(&serde_json::json!("true")));
    }

    #[tokio::test]
    async fn should_omit_doc_security_keys_when_pptx_declares_no_doc_security_element() {
        let bytes = build_pptx(None);
        let result = extract_bytes_document(&bytes, PPTX_MIME, &ExtractionConfig::default())
            .await
            .expect("extraction should succeed");
        let additional = &result.metadata.additional;

        assert_eq!(additional.get("doc_security"), None);
        assert_eq!(additional.get(LOCKED_FOR_ANNOTATIONS), None);
    }
}
