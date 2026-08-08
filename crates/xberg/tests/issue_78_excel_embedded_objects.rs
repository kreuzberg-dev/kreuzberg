#![allow(clippy::print_stdout, clippy::print_stderr, clippy::dbg_macro)] // ~keep: test/bench binaries print by design; org logging policy exempts tests
//! xberg-io/xberg#78: Excel workbooks may carry embedded objects under `xl/embeddings/`
//! (e.g. an embedded text file, spreadsheet, or OLE object pasted into a cell). Those
//! bytes must not be silently dropped — the extractor reuses the shared
//! `extraction::ooxml_embedded` OOXML-embedded-object pipeline (already exercised for
//! DOCX/PPTX) with the `xl/embeddings/` prefix, attaching results to
//! `ExtractedDocument.children`.

#![cfg(feature = "office")]

use std::io::Write as _;
use zip::write::{SimpleFileOptions, ZipWriter};

mod helpers;
use helpers::extract_bytes_document;

const XLSX_MIME: &str = "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet";

/// Build a minimal in-memory `.xlsx` with one sheet and one embedded `.txt` file under
/// `xl/embeddings/`.
fn make_xlsx_with_embedded_text_file(embedded_contents: &[u8]) -> Vec<u8> {
    let mut buffer = Vec::new();
    {
        let mut zip = ZipWriter::new(std::io::Cursor::new(&mut buffer));
        let options = SimpleFileOptions::default();

        zip.start_file("[Content_Types].xml", options).unwrap();
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
        .unwrap();

        zip.start_file("_rels/.rels", options).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1"
    Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument"
    Target="xl/workbook.xml"/>
</Relationships>"#,
        )
        .unwrap();

        zip.start_file("xl/workbook.xml", options).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"
          xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
  <sheets><sheet name="Sheet1" sheetId="1" r:id="rId1"/></sheets>
</workbook>"#,
        )
        .unwrap();

        zip.start_file("xl/_rels/workbook.xml.rels", options).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1"
    Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet"
    Target="worksheets/sheet1.xml"/>
</Relationships>"#,
        )
        .unwrap();

        zip.start_file("xl/worksheets/sheet1.xml", options).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">
  <dimension ref="A1"/>
  <sheetData><row r="1"><c r="A1" t="inlineStr"><is><t>Hi</t></is></c></row></sheetData>
</worksheet>"#,
        )
        .unwrap();

        zip.start_file("xl/embeddings/oleObject1.txt", options).unwrap();
        zip.write_all(embedded_contents).unwrap();

        zip.finish().unwrap();
    }
    buffer
}

#[tokio::test]
async fn should_attach_embedded_object_as_child_document() {
    let embedded_text = b"embedded note contents";
    let bytes = make_xlsx_with_embedded_text_file(embedded_text);

    let result = extract_bytes_document(&bytes, XLSX_MIME, &xberg::ExtractionConfig::default())
        .await
        .expect("workbook with an embedded object must still extract successfully");

    let children = result
        .children
        .as_ref()
        .expect("embedded object under xl/embeddings/ must be attached as a child document");
    assert_eq!(children.len(), 1, "exactly one embedded object expected");
    assert_eq!(children[0].path, "oleObject1.txt");
    assert!(
        children[0].result.content.contains("embedded note contents"),
        "child document content must contain the embedded file's text: {:?}",
        children[0].result.content
    );
}

#[tokio::test]
async fn should_extract_normally_when_no_embedded_objects_present() {
    // Reuse the same builder shape but with an empty embeddings directory marker
    // removed entirely by not writing any xl/embeddings/ entry at all.
    let mut buffer = Vec::new();
    {
        let mut zip = ZipWriter::new(std::io::Cursor::new(&mut buffer));
        let options = SimpleFileOptions::default();

        zip.start_file("[Content_Types].xml", options).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
  <Default Extension="xml" ContentType="application/xml"/>
  <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
  <Override PartName="/xl/workbook.xml"
    ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/>
</Types>"#,
        )
        .unwrap();
        zip.start_file("_rels/.rels", options).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1"
    Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument"
    Target="xl/workbook.xml"/>
</Relationships>"#,
        )
        .unwrap();
        zip.start_file("xl/workbook.xml", options).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"
          xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
  <sheets><sheet name="Sheet1" sheetId="1" r:id="rId1"/></sheets>
</workbook>"#,
        )
        .unwrap();
        zip.start_file("xl/_rels/workbook.xml.rels", options).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1"
    Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet"
    Target="worksheets/sheet1.xml"/>
</Relationships>"#,
        )
        .unwrap();
        zip.start_file("xl/worksheets/sheet1.xml", options).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">
  <dimension ref="A1"/>
  <sheetData><row r="1"><c r="A1" t="inlineStr"><is><t>Plain</t></is></c></row></sheetData>
</worksheet>"#,
        )
        .unwrap();
        zip.finish().unwrap();
    }

    let result = extract_bytes_document(&buffer, XLSX_MIME, &xberg::ExtractionConfig::default())
        .await
        .expect("workbook without embedded objects must extract normally");

    assert!(
        result.children.is_none(),
        "no children expected when there are no embedded objects"
    );
    assert!(result.content.contains("Plain"));
}
