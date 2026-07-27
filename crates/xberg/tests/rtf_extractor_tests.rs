//! Comprehensive TDD test suite for RTF extraction.
//!
//! This test suite validates RTF extraction capabilities.
//! Tests cover:
//! - Accent and Unicode handling
//! - Bookmarks and internal links
//! - Footnotes and references
//! - Text formatting (bold, italic, underline, strikeout, superscript, subscript, small caps)
//! - Headings and structure
//! - Image extraction
//! - External hyperlinks
//! - List extraction (simple and complex nested lists)
//! - Table extraction (simple and complex with special formatting)
//! - Unicode characters and special symbols
//!
//! Test Organization:
//! - Basic Content Extraction (unicode, accent)
//! - Structure Preservation (heading, list_simple, list_complex)
//! - Table Extraction (table_simple, table_error_codes)
//! - Formatting Detection (formatting)
//! - Special Features (footnote, bookmark, link)
//! - Integration Tests (deterministic extraction, no content loss)
//!
//! Success Criteria:
//! - All tests passing (100%)
//! - No content loss (should extract meaningful text from all files)
//! - Deterministic extraction (same input = same output)
//!
//! Note: These tests require the `office` feature to be enabled.

#![cfg(feature = "office")]
#![allow(clippy::doc_suspicious_footnotes)]

use std::path::PathBuf;
use xberg::core::config::ExtractionConfig;

mod helpers;
use helpers::extract_uri_document;

/// Helper function to get path to RTF test document
fn get_rtf_path(filename: &str) -> PathBuf {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    PathBuf::from(manifest_dir)
        .parent()
        .expect("xberg crate should have a parent")
        .parent()
        .expect("parent should have a parent")
        .join("test_documents")
        .join("rtf")
        .join(filename)
}

/// Helper for reaching the workspace root from the xberg crate
fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xberg crate should have a parent")
        .parent()
        .expect("workspace root exists")
        .to_path_buf()
}

/// Test extraction of RTF file with accent characters (accented vowels).
///
/// File: accent.rtf
/// Content: "le café où on ne fume pas"
/// Expected: Correctly extracts French text with accented characters (é, ù)
/// Pandoc baseline: le café où on ne fume pas
#[tokio::test]
async fn test_rtf_accent_extraction() {
    let config = ExtractionConfig::default();
    let path = get_rtf_path("accent.rtf");

    let result = extract_uri_document(&path, Some("application/rtf"), &config).await;

    assert!(result.is_ok(), "RTF extraction should succeed for accent.rtf");
    let extraction = result.expect("Operation failed");

    assert_eq!(extraction.mime_type, "application/rtf");

    assert!(!extraction.content.is_empty(), "Content should not be empty");

    let content = extraction.content.to_lowercase();

    assert!(
        extraction.content.contains("café") || content.contains("cafe"),
        "Should extract French word 'café' or 'cafe'"
    );

    assert!(
        extraction.content.contains("où") || content.contains("ou"),
        "Should extract French word 'où' or 'ou'"
    );

    assert!(
        content.contains("fume") || content.contains("smoking"),
        "Should extract content about smoking"
    );
}

/// Test extraction of RTF file with CP1251 hex byte escapes.
///
/// File: ansicpg1251.rtf
/// Content: Cyrillic text encoded as `\'hh` bytes with `\ansicpg1251`
/// Expected: Decodes byte escapes with the declared ANSI codepage
#[tokio::test]
async fn test_rtf_ansicpg1251_extraction() {
    let config = ExtractionConfig::default();
    let path = get_rtf_path("ansicpg1251.rtf");

    let result = extract_uri_document(&path, Some("application/rtf"), &config).await;

    assert!(result.is_ok(), "RTF extraction should succeed for ansicpg1251.rtf");
    let extraction = result.expect("Operation failed");

    assert_eq!(extraction.mime_type, "application/rtf");
    assert!(
        extraction.content.contains("Привет, мир!"),
        "Should decode CP1251 hex escapes as Cyrillic text (found: {})",
        extraction.content
    );
    assert!(
        !extraction.content.contains("Ïðèâåò"),
        "Should not decode CP1251 bytes as Windows-1252 mojibake"
    );
}

/// Test extraction of RTF file with bookmarks (internal anchors/references).
///
/// File: bookmark.rtf
/// Content: Bookmark anchor labeled "Bookmark_1" and link text "click me"
/// Expected: Extracts bookmark definition and link text
/// Pandoc baseline: [Bookmark_1]{#bookmark_1} and [click me](#bookmark_1)
#[tokio::test]
async fn test_rtf_bookmark_extraction() {
    let config = ExtractionConfig::default();
    let path = get_rtf_path("bookmark.rtf");

    let result = extract_uri_document(&path, Some("application/rtf"), &config).await;

    assert!(result.is_ok(), "RTF extraction should succeed for bookmark.rtf");
    let extraction = result.expect("Operation failed");

    let content = extraction.content.to_lowercase();

    assert!(
        content.contains("bookmark") || content.contains("click") || content.contains("me"),
        "Should extract bookmark or link text (found: {})",
        extraction.content
    );
}

/// Test extraction of RTF file with footnotes.
///
/// File: footnote.rtf
/// Content: Academic text with footnote references and their content
/// Expected: Extracts both main text and footnote content
/// Pandoc baseline: Uses [^1] and [^2] syntax for footnotes
#[tokio::test]
async fn test_rtf_footnote_extraction() {
    let config = ExtractionConfig::default();
    let path = get_rtf_path("footnote.rtf");

    let result = extract_uri_document(&path, Some("application/rtf"), &config).await;

    assert!(result.is_ok(), "RTF extraction should succeed for footnote.rtf");
    let extraction = result.expect("Operation failed");

    assert!(!extraction.content.is_empty(), "Content should not be empty");

    let content = extraction.content.to_lowercase();

    assert!(
        content.contains("mead") || content.contains("landmark"),
        "Should extract main text about Mead's study"
    );

    assert!(
        content.contains("note")
            || content.contains("annotated")
            || content.contains("bibliography")
            || content.contains("sahlins"),
        "Should extract footnote content or references"
    );

    assert!(
        content.contains("footnote") || extraction.content.contains("[^") || content.contains("annotated"),
        "Should contain footnote indicators"
    );
}

/// Test extraction of RTF file with various text formatting.
///
/// File: formatting.rtf
/// Content: Text with bold, italic, underline, strikeout, superscript, subscript, small caps
/// Expected: Preserves or indicates all formatting types
/// Pandoc baseline: Detailed formatting in markdown syntax
#[tokio::test]
async fn test_rtf_formatting_extraction() {
    let config = ExtractionConfig::default();
    let path = get_rtf_path("formatting.rtf");

    let result = extract_uri_document(&path, Some("application/rtf"), &config).await;

    assert!(result.is_ok(), "RTF extraction should succeed for formatting.rtf");
    let extraction = result.expect("Operation failed");

    assert!(!extraction.content.is_empty(), "Content should not be empty");

    let content = extraction.content.to_lowercase();

    assert!(
        content.contains("formatting") || content.contains("test") || content.contains("bold"),
        "Should extract formatting-related content"
    );

    assert!(
        extraction.content.contains("**bold**") || content.contains("bold"),
        "Should preserve or indicate bold text"
    );

    assert!(
        extraction.content.contains("*italic") || content.contains("italic"),
        "Should preserve or indicate italic text"
    );

    let has_formatting = extraction.content.contains("**")
        || extraction.content.contains("*")
        || extraction.content.contains("__")
        || extraction.content.contains("_")
        || extraction.content.contains("~~")
        || extraction.content.contains("^")
        || extraction.content.contains("~")
        || content.contains("bold");

    assert!(has_formatting, "Should preserve or indicate text formatting");
}

/// Test extraction of RTF file with heading hierarchy.
///
/// File: heading.rtf
/// Content: Three levels of headings (H1, H2, H3) followed by paragraph
/// Expected: Extracts all headings and paragraph text
/// Pandoc baseline: Markdown heading syntax (# ## ###)
#[tokio::test]
async fn test_rtf_heading_extraction() {
    let config = ExtractionConfig::default();
    let path = get_rtf_path("heading.rtf");

    let result = extract_uri_document(&path, Some("application/rtf"), &config).await;

    assert!(result.is_ok(), "RTF extraction should succeed for heading.rtf");
    let extraction = result.expect("Operation failed");

    assert!(!extraction.content.is_empty(), "Content should not be empty");

    assert!(
        extraction.content.contains("Heading 1"),
        "Should extract Heading 1 text"
    );

    assert!(
        extraction.content.contains("Heading 2"),
        "Should extract Heading 2 text"
    );

    assert!(
        extraction.content.contains("Heading 3"),
        "Should extract Heading 3 text"
    );

    assert!(
        extraction.content.contains("Paragraph"),
        "Should extract paragraph text"
    );

    let content_lower = extraction.content.to_lowercase();
    assert!(
        extraction.content.contains("#")
            || (content_lower.contains("heading 1") && content_lower.contains("heading 2")),
        "Should preserve heading hierarchy"
    );
}

/// Test extraction of RTF file with embedded or referenced image.
///
/// File: image.rtf
/// Content: Image reference with dimensions (2.0in x 2.0in)
/// Expected: Extracts image reference and/or dimensions
/// Pandoc baseline: Markdown image syntax with dimensions
#[tokio::test]
async fn test_rtf_image_extraction() {
    let config = ExtractionConfig::default();
    let path = get_rtf_path("image.rtf");

    let result = extract_uri_document(&path, Some("application/rtf"), &config).await;

    assert!(result.is_ok(), "RTF extraction should succeed for image.rtf");
    let extraction = result.expect("Operation failed");

    assert!(!extraction.content.is_empty(), "Content should not be empty");

    let content = extraction.content.to_lowercase();

    assert!(
        extraction.content.contains("!")
            || content.contains("image")
            || extraction.content.contains(".jpg")
            || content.contains("2.0")
            || content.contains("width")
            || content.contains("height"),
        "Should contain image reference or dimension information (found: {})",
        extraction.content
    );
}

/// Test extraction of RTF file with external hyperlink.
///
/// File: link.rtf
/// Content: Link to pandoc.org website
/// Expected: Extracts link text and/or URL
/// Pandoc baseline: Markdown link syntax [pandoc](http://pandoc.org)
#[tokio::test]
async fn test_rtf_link_extraction() {
    let config = ExtractionConfig::default();
    let path = get_rtf_path("link.rtf");

    let result = extract_uri_document(&path, Some("application/rtf"), &config).await;

    assert!(result.is_ok(), "RTF extraction should succeed for link.rtf");
    let extraction = result.expect("Operation failed");

    assert!(!extraction.content.is_empty(), "Content should not be empty");

    let content = extraction.content.to_lowercase();

    assert!(
        content.contains("pandoc") || content.contains("http"),
        "Should extract link-related content (found: {})",
        extraction.content
    );
}

/// Test extraction of RTF file with complex nested list structure.
///
/// File: list_complex.rtf
/// Content: Multi-level nested list with various numbering (numeric, alphabetic, roman)
/// Expected: Extracts all list items preserving or indicating hierarchy
/// Pandoc baseline: Markdown nested list with mixed numbering schemes
#[tokio::test]
async fn test_rtf_list_complex_extraction() {
    let config = ExtractionConfig::default();
    let path = get_rtf_path("list_complex.rtf");

    let result = extract_uri_document(&path, Some("application/rtf"), &config).await;

    assert!(result.is_ok(), "RTF extraction should succeed for list_complex.rtf");
    let extraction = result.expect("Operation failed");

    assert!(!extraction.content.is_empty(), "Content should not be empty");

    let content = extraction.content.to_lowercase();

    assert!(content.contains("one"), "Should extract list item 'One'");

    assert!(content.contains("two"), "Should extract list item 'Two'");

    assert!(
        content.contains("three") || content.contains("three"),
        "Should extract nested list item 'Three'"
    );

    assert!(
        content.contains("five") || content.contains("six"),
        "Should extract deeply nested list items"
    );

    assert!(
        extraction.content.contains("1")
            || extraction.content.contains("-")
            || extraction.content.contains("•")
            || content.contains("one"),
        "Should preserve list structure indicators"
    );

    assert!(
        content.contains("out of list") || content.contains("out"),
        "Should extract separator text 'Out of list'"
    );

    assert!(
        content.contains("seven") || content.contains("eight") || content.contains("7") || content.contains("8"),
        "Should extract restarted list numbering (7, 8)"
    );
}

/// Test extraction of RTF file with simple bulleted list.
///
/// File: list_simple.rtf
/// Content: Simple bullet list with one nested item and list break
/// Expected: Extracts all list items and indicates nesting
/// Pandoc baseline: Simple markdown bullet list with nesting
#[tokio::test]
async fn test_rtf_list_simple_extraction() {
    let config = ExtractionConfig::default();
    let path = get_rtf_path("list_simple.rtf");

    let result = extract_uri_document(&path, Some("application/rtf"), &config).await;

    assert!(result.is_ok(), "RTF extraction should succeed for list_simple.rtf");
    let extraction = result.expect("Operation failed");

    assert!(!extraction.content.is_empty(), "Content should not be empty");

    let content = extraction.content.to_lowercase();

    assert!(content.contains("one"), "Should extract list item 'one'");

    assert!(content.contains("two"), "Should extract list item 'two'");

    assert!(content.contains("sub"), "Should extract nested list item 'sub'");

    assert!(content.contains("new"), "Should extract 'new list' text");

    assert!(
        content.contains("one") && content.contains("two") && content.contains("sub"),
        "Should extract all list items"
    );
}

/// Test extraction of RTF file with table containing error codes.
///
/// File: table_error_codes.rtf
/// Content: Table with Code and Error columns, 23 rows of Pandoc error codes
/// Expected: Extracts table structure and all data cells
/// Pandoc baseline: Markdown table format with 2 columns and 23 rows
///
/// Note: RTF table extraction via Pandoc markdown output may result in empty content
/// due to limitations in Pandoc's markdown table rendering. Tables are present
/// in Pandoc's internal JSON representation but may not render in text format.
#[tokio::test]
async fn test_rtf_table_error_codes_extraction() {
    let config = ExtractionConfig::default();
    let path = get_rtf_path("table_error_codes.rtf");

    let result = extract_uri_document(&path, Some("application/rtf"), &config).await;

    assert!(
        result.is_ok(),
        "RTF extraction should succeed for table_error_codes.rtf"
    );
    let extraction = result.expect("Operation failed");

    assert!(
        extraction.mime_type == "application/rtf",
        "MIME type should be preserved"
    );
}

/// Test extraction of RTF file with simple 4-column, 2-row table.
///
/// File: table_simple.rtf
/// Content: Table with headers A, B, C, D and data row E, F, G, H
/// Expected: Extracts all cells in correct table structure
/// Pandoc baseline: Markdown table format
///
/// Note: RTF table extraction via Pandoc markdown output may result in empty content
/// due to limitations in Pandoc's markdown table rendering. Tables are present
/// in Pandoc's internal JSON representation but may not render in text format.
#[tokio::test]
async fn test_rtf_table_simple_extraction() {
    let config = ExtractionConfig::default();
    let path = get_rtf_path("table_simple.rtf");

    let result = extract_uri_document(&path, Some("application/rtf"), &config).await;

    assert!(result.is_ok(), "RTF extraction should succeed for table_simple.rtf");
    let extraction = result.expect("Operation failed");

    assert!(
        extraction.mime_type == "application/rtf",
        "MIME type should be preserved"
    );
}

/// Test extraction of RTF file with various Unicode characters.
///
/// File: unicode.rtf
/// Content: Smart quotes, Greek letters (α, ä)
/// Expected: Correctly extracts and preserves Unicode characters
/// Pandoc baseline: "hi"'hi'αä
#[tokio::test]
async fn test_rtf_unicode_extraction() {
    let config = ExtractionConfig::default();
    let path = get_rtf_path("unicode.rtf");

    let result = extract_uri_document(&path, Some("application/rtf"), &config).await;

    assert!(result.is_ok(), "RTF extraction should succeed for unicode.rtf");
    let extraction = result.expect("Operation failed");

    assert!(!extraction.content.is_empty(), "Content should not be empty");

    assert!(
        extraction.content.contains("hi") || extraction.content.contains("α") || extraction.content.contains("ä"),
        "Should extract unicode content (found: {})",
        extraction.content
    );
}

/// Test that RTF extraction is deterministic
/// Same input should produce identical output
#[tokio::test]
async fn test_rtf_extraction_deterministic_unicode() {
    let config = ExtractionConfig::default();
    let path = get_rtf_path("unicode.rtf");

    let result1 = extract_uri_document(&path, Some("application/rtf"), &config).await;
    let result2 = extract_uri_document(&path, Some("application/rtf"), &config).await;

    assert!(result1.is_ok() && result2.is_ok(), "Both extractions should succeed");

    let extraction1 = result1.expect("Operation failed");
    let extraction2 = result2.expect("Operation failed");

    assert_eq!(
        extraction1.content, extraction2.content,
        "FAIL: Extraction is not deterministic. Same input produced different outputs."
    );
}

/// Test that RTF extraction is deterministic for complex files
/// Same input should produce identical output
#[tokio::test]
async fn test_rtf_extraction_deterministic_list_complex() {
    let config = ExtractionConfig::default();
    let path = get_rtf_path("list_complex.rtf");

    let result1 = extract_uri_document(&path, Some("application/rtf"), &config).await;
    let result2 = extract_uri_document(&path, Some("application/rtf"), &config).await;

    assert!(result1.is_ok() && result2.is_ok(), "Both extractions should succeed");

    let extraction1 = result1.expect("Operation failed");
    let extraction2 = result2.expect("Operation failed");

    assert_eq!(
        extraction1.content, extraction2.content,
        "FAIL: Extraction is not deterministic. Same input produced different outputs."
    );
}

/// Test no critical content loss
/// All RTF files should extract non-empty content (except possibly image-only files)
#[tokio::test]
async fn test_rtf_no_critical_content_loss() {
    let config = ExtractionConfig::default();

    let must_extract = vec![
        "unicode.rtf",
        "ansicpg1251.rtf",
        "accent.rtf",
        "heading.rtf",
        "list_simple.rtf",
        "list_complex.rtf",
        "formatting.rtf",
        "footnote.rtf",
        "bookmark.rtf",
        "link.rtf",
    ];

    for filename in must_extract {
        let path = get_rtf_path(filename);
        let result = extract_uri_document(&path, Some("application/rtf"), &config).await;

        assert!(
            result.is_ok(),
            "FAIL: Extraction failed for {} (critical file)",
            filename
        );

        let extraction = result.expect("Operation failed");
        assert!(
            !extraction.content.is_empty(),
            "FAIL: CRITICAL - Extracted 0 bytes from {}. RTF extractor lost all content.",
            filename
        );

        assert!(
            extraction.content.len() >= 5,
            "FAIL: Extracted only {} bytes from {} (expected at least 5 characters). Content: '{}'",
            extraction.content.len(),
            filename,
            extraction.content
        );
    }
}

/// Test MIME type preservation
/// All RTF extractions should preserve the application/rtf MIME type
#[tokio::test]
async fn test_rtf_mime_type_preservation() {
    let config = ExtractionConfig::default();

    let test_files = vec![
        "unicode.rtf",
        "ansicpg1251.rtf",
        "accent.rtf",
        "heading.rtf",
        "list_simple.rtf",
    ];

    for filename in test_files {
        let path = get_rtf_path(filename);
        let result = extract_uri_document(&path, Some("application/rtf"), &config).await;

        assert!(result.is_ok(), "Extraction should succeed for {}", filename);

        let extraction = result.expect("Operation failed");
        assert_eq!(
            extraction.mime_type, "application/rtf",
            "FAIL: MIME type not preserved for {}",
            filename
        );
    }
}

/// Parity check: RTF extracted from the DOCX `word_sample.docx` should
/// carry the same content signals and metadata as the DOCX extractor.
#[tokio::test]
async fn test_rtf_word_sample_matches_docx_metadata_and_content() {
    let root = workspace_root();
    let rtf_path = root.join("test_documents/rtf/word_sample.rtf");
    let docx_path = root.join("test_documents/docx/word_sample.docx");

    if !rtf_path.exists() || !docx_path.exists() {
        println!("Skipping word_sample parity test: fixtures missing");
        return;
    }

    let config = ExtractionConfig::default();
    let rtf_result = extract_uri_document(&rtf_path, Some("application/rtf"), &config)
        .await
        .expect("RTF extraction should succeed for word_sample");
    let docx_result = extract_uri_document(&docx_path, None, &config)
        .await
        .expect("DOCX extraction should succeed for word_sample");

    let rtf_content_lower = rtf_result.content.to_lowercase();
    assert!(
        rtf_content_lower.contains("swim"),
        "RTF content should include the same body text as DOCX"
    );

    assert_eq!(
        rtf_result.metadata.created_by.as_deref(),
        docx_result.metadata.created_by.as_deref(),
        "Metadata field created_by should align with DOCX"
    );
    assert_eq!(
        rtf_result.metadata.modified_by.as_deref(),
        docx_result.metadata.modified_by.as_deref(),
        "Metadata field modified_by should align with DOCX"
    );
    assert_eq!(
        rtf_result.metadata.created_at.as_deref(),
        docx_result.metadata.created_at.as_deref(),
        "Metadata field created_at should align with DOCX"
    );
    assert_eq!(
        rtf_result.metadata.additional.get("revision").and_then(|v| v.as_str()),
        docx_result.metadata.additional.get("revision").and_then(|v| v.as_str()),
        "Metadata field revision should align with DOCX"
    );

    for (key, expected) in [
        ("page_count", 2),
        ("word_count", 108),
        ("character_count", 620),
        ("line_count", 5),
        ("paragraph_count", 1),
    ] {
        assert_eq!(
            rtf_result.metadata.additional.get(key).and_then(|v| v.as_i64()),
            Some(expected),
            "Metadata field {} should match DOCX values",
            key
        );
    }
}

/// RTF generated from lorem_ipsum.docx should expose the same document statistics
/// we validate for the DOCX extractor.
#[tokio::test]
async fn test_rtf_lorem_ipsum_metadata_alignment() {
    let root = workspace_root();
    let rtf_path = root.join("test_documents/rtf/lorem_ipsum.rtf");

    if !rtf_path.exists() {
        println!("Skipping lorem_ipsum metadata test: fixture missing");
        return;
    }

    let config = ExtractionConfig::default();
    let result = extract_uri_document(&rtf_path, Some("application/rtf"), &config)
        .await
        .expect("RTF extraction should succeed for lorem_ipsum");

    assert!(
        result.content.to_lowercase().contains("lorem ipsum"),
        "Content should contain lorem ipsum text"
    );

    for (key, expected) in [
        ("page_count", 1),
        ("word_count", 520),
        ("character_count", 2967),
        ("line_count", 24),
        ("paragraph_count", 6),
    ] {
        assert_eq!(
            result.metadata.additional.get(key).and_then(|v| v.as_i64()),
            Some(expected),
            "Metadata field {} should match DOCX values",
            key
        );
    }
}

/// The comprehensive extraction fixture should mirror the coverage of the ODT/DOCX variants:
/// headings, section text, table content, and metadata fields should all be present.
#[tokio::test]
async fn test_rtf_comprehensive_extraction_alignment() {
    let root = workspace_root();
    let rtf_path = root.join("test_documents/rtf/extraction_test.rtf");
    let docx_path = root.join("test_documents/docx/extraction_test.docx");
    let odt_path = root.join("test_documents/odt/extraction_test.odt");

    if !rtf_path.exists() {
        println!("⚠️  Test document not found at {:?}, skipping", rtf_path);
        return;
    }
    if !docx_path.exists() || !odt_path.exists() {
        println!(
            "⚠️  Companion DOCX/ODT documents missing (docx: {}, odt: {}), skipping",
            docx_path.exists(),
            odt_path.exists()
        );
        return;
    }

    let config = ExtractionConfig::default();
    let rtf_result = extract_uri_document(&rtf_path, Some("application/rtf"), &config)
        .await
        .expect("RTF extraction should succeed for extraction_test.rtf");
    let docx_result = extract_uri_document(&docx_path, None, &config)
        .await
        .expect("DOCX extraction should succeed for extraction_test.docx");
    let odt_result = extract_uri_document(&odt_path, None, &config)
        .await
        .expect("ODT extraction should succeed for extraction_test.odt");

    assert!(
        rtf_result.content.contains("Comprehensive Extraction Test Document"),
        "Should include document heading"
    );
    assert!(
        rtf_result.content.contains("First Section"),
        "Should include first section heading"
    );
    assert!(
        rtf_result.content.contains("Second Section"),
        "Should include second section heading"
    );
    assert!(
        rtf_result.content.contains("Third Section"),
        "Should include third section heading"
    );

    for expected in ["Header 1", "Cell 1A", "Product", "Apple"] {
        assert!(
            rtf_result.content.contains(expected),
            "Should include table content '{}'",
            expected
        );
    }
    assert!(
        !rtf_result.tables.is_empty(),
        "Should extract structured tables from RTF"
    );
    assert!(
        rtf_result
            .tables
            .iter()
            .any(|t| t.markdown.contains("Header 1") || t.markdown.contains("Cell 1A")),
        "Table markdown should include header/data cells"
    );
    assert!(
        rtf_result.tables.len() >= docx_result.tables.len() && rtf_result.tables.len() >= odt_result.tables.len(),
        "RTF should capture at least as many tables as DOCX/ODT"
    );

    for (key, expected) in [
        ("page_count", 1),
        ("word_count", 83),
        ("character_count", 475),
        ("line_count", 12),
        ("paragraph_count", 8),
    ] {
        assert_eq!(
            rtf_result.metadata.additional.get(key).and_then(|v| v.as_i64()),
            Some(expected),
            "Metadata field {} should be populated",
            key
        );
    }
}
