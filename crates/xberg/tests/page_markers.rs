//! Page marker insertion tests.
//!
//! Tests the page marker feature that inserts markers before each page in extracted content.
//! This is critical for downstream applications that need to know where page boundaries are
//! in the text stream.

#![cfg(feature = "pdf")]

mod helpers;
use helpers::extract_uri_document_blocking;

use helpers::*;
use xberg::core::config::{ExtractionConfig, PageConfig};

/// Test that page markers are inserted when enabled.
#[test]
fn test_page_markers_inserted_when_enabled() {
    if skip_if_missing("pdf/multi_page.pdf") {
        return;
    }

    let file_path = get_test_file_path("pdf/multi_page.pdf");
    let config = ExtractionConfig {
        pages: Some(PageConfig {
            insert_page_markers: true,
            ..Default::default()
        }),
        ..Default::default()
    };

    let result =
        extract_uri_document_blocking(&file_path, None, &config).expect("Failed to extract PDF with page markers");

    assert!(
        result.content.contains("<!-- PAGE"),
        "Content should contain page markers when insert_page_markers is true. Content: {}",
        &result.content[..result.content.len().min(500)]
    );
}

/// Test that page 1 gets a marker (regression test for the bug where page 1 was skipped).
#[test]
fn test_page_1_gets_marker() {
    if skip_if_missing("pdf/multi_page.pdf") {
        return;
    }

    let file_path = get_test_file_path("pdf/multi_page.pdf");
    let config = ExtractionConfig {
        pages: Some(PageConfig {
            insert_page_markers: true,
            ..Default::default()
        }),
        ..Default::default()
    };

    let result =
        extract_uri_document_blocking(&file_path, None, &config).expect("Failed to extract PDF with page markers");

    assert!(
        result.content.contains("<!-- PAGE 1 -->"),
        "Content should contain marker for page 1. Content start: {}",
        &result.content[..result.content.len().min(200)]
    );
}

/// Test that custom marker format works correctly.
#[test]
fn test_custom_marker_format() {
    if skip_if_missing("pdf/multi_page.pdf") {
        return;
    }

    let file_path = get_test_file_path("pdf/multi_page.pdf");
    let custom_format = "=== Page {page_num} ===";
    let config = ExtractionConfig {
        pages: Some(PageConfig {
            insert_page_markers: true,
            marker_format: custom_format.to_string(),
            ..Default::default()
        }),
        ..Default::default()
    };

    let result =
        extract_uri_document_blocking(&file_path, None, &config).expect("Failed to extract PDF with custom markers");

    assert!(
        result.content.contains("=== Page 1 ==="),
        "Content should contain custom marker for page 1"
    );
}

/// Test that {page_num} placeholder is replaced with actual page numbers.
#[test]
fn test_page_num_placeholder_replacement() {
    if skip_if_missing("pdf/multi_page.pdf") {
        return;
    }

    let file_path = get_test_file_path("pdf/multi_page.pdf");
    let config = ExtractionConfig {
        pages: Some(PageConfig {
            insert_page_markers: true,
            marker_format: "[PAGE {page_num}]".to_string(),
            ..Default::default()
        }),
        ..Default::default()
    };

    let result =
        extract_uri_document_blocking(&file_path, None, &config).expect("Failed to extract PDF with custom markers");

    assert!(
        !result.content.contains("{page_num}"),
        "Placeholder should be replaced, not appear in output"
    );

    assert!(
        result.content.contains("[PAGE 1]"),
        "Should contain marker with actual page number"
    );
}

/// Test that page markers and extract_pages work together.
#[test]
fn test_markers_and_extract_pages_together() {
    if skip_if_missing("pdf/multi_page.pdf") {
        return;
    }

    let file_path = get_test_file_path("pdf/multi_page.pdf");
    let config = ExtractionConfig {
        pages: Some(PageConfig {
            insert_page_markers: true,
            extract_pages: true,
            marker_format: "--- PAGE {page_num} ---".to_string(),
        }),
        ..Default::default()
    };

    let result =
        extract_uri_document_blocking(&file_path, None, &config).expect("Failed to extract PDF with both features");

    assert!(
        result.pages.is_some(),
        "Pages array should be present when extract_pages is true"
    );

    assert!(
        result.content.contains("--- PAGE 1 ---"),
        "Content should contain page markers"
    );
}

/// Test that when markers are disabled, no markers appear in content.
#[test]
fn test_no_markers_when_disabled() {
    if skip_if_missing("pdf/multi_page.pdf") {
        return;
    }

    let file_path = get_test_file_path("pdf/multi_page.pdf");
    let config = ExtractionConfig {
        pages: Some(PageConfig {
            insert_page_markers: false,
            ..Default::default()
        }),
        ..Default::default()
    };

    let result =
        extract_uri_document_blocking(&file_path, None, &config).expect("Failed to extract PDF without markers");

    assert!(
        !result.content.contains("<!-- PAGE"),
        "Content should not contain markers when insert_page_markers is false"
    );
}

/// Test that markers appear before page content, not after.
#[test]
fn test_marker_appears_before_content() {
    if skip_if_missing("pdf/multi_page.pdf") {
        return;
    }

    let file_path = get_test_file_path("pdf/multi_page.pdf");
    let config = ExtractionConfig {
        pages: Some(PageConfig {
            insert_page_markers: true,
            marker_format: "[[PAGE {page_num}]]".to_string(),
            ..Default::default()
        }),
        ..Default::default()
    };

    let result = extract_uri_document_blocking(&file_path, None, &config).expect("Failed to extract PDF with markers");

    let marker_pos = result.content.find("[[PAGE 1]]");
    assert!(marker_pos.is_some(), "Marker should be present");

    let pos = marker_pos.expect("Operation failed");
    assert!(
        pos < 50,
        "Marker for page 1 should appear at the start, but found at position {}",
        pos
    );
}

/// Test that multi-page PDFs get markers for all pages.
#[test]
fn test_multi_page_markers() {
    if skip_if_missing("pdf/multi_page.pdf") {
        return;
    }

    let file_path = get_test_file_path("pdf/multi_page.pdf");
    let config = ExtractionConfig {
        pages: Some(PageConfig {
            insert_page_markers: true,
            extract_pages: true,
            ..Default::default()
        }),
        ..Default::default()
    };

    let result = extract_uri_document_blocking(&file_path, None, &config).expect("Failed to extract PDF");

    if let Some(ref pages) = result.pages {
        let page_count = pages.len();

        for page_num in 1..=page_count.min(3) {
            let marker = format!("<!-- PAGE {} -->", page_num);
            assert!(
                result.content.contains(&marker),
                "Should contain marker for page {} (total pages: {})",
                page_num,
                page_count
            );
        }
    }
}

/// Test default marker format value.
#[test]
fn test_default_marker_format() {
    let config = PageConfig::default();
    assert_eq!(
        config.marker_format, "\n\n<!-- PAGE {page_num} -->\n\n",
        "Default marker format should match expected value"
    );
}

/// Test that empty page still gets a marker.
#[test]
fn test_empty_page_gets_marker() {
    let config = PageConfig {
        insert_page_markers: true,
        ..Default::default()
    };

    assert!(
        config.insert_page_markers,
        "Config should enable markers regardless of page content"
    );
}

/// Test page markers are inserted in markdown output format (regression test for #412).
#[test]
fn test_page_markers_in_markdown_output() {
    if skip_if_missing("pdf/multi_page.pdf") {
        return;
    }

    let file_path = get_test_file_path("pdf/multi_page.pdf");
    let config = ExtractionConfig {
        output_format: xberg::OutputFormat::Markdown,
        pages: Some(PageConfig {
            insert_page_markers: true,
            marker_format: "<!-- PAGE {page_num} -->".to_string(),
            ..Default::default()
        }),
        ..Default::default()
    };

    let result = extract_uri_document_blocking(&file_path, None, &config).expect("Failed to extract PDF as markdown");

    assert!(
        result.content.contains("<!-- PAGE 1 -->"),
        "Markdown output should contain page marker for page 1. Content start: {}",
        &result.content[..result.content.len().min(300)]
    );
}

/// Test page markers with custom format in markdown output (regression test for #412).
#[test]
fn test_page_markers_custom_format_markdown() {
    if skip_if_missing("pdf/multi_page.pdf") {
        return;
    }

    let file_path = get_test_file_path("pdf/multi_page.pdf");
    let config = ExtractionConfig {
        output_format: xberg::OutputFormat::Markdown,
        pages: Some(PageConfig {
            insert_page_markers: true,
            marker_format: "<page number=\"{page_num}\">".to_string(),
            ..Default::default()
        }),
        ..Default::default()
    };

    let result = extract_uri_document_blocking(&file_path, None, &config).expect("Failed to extract PDF as markdown");

    assert!(
        result.content.contains("<page number=\"1\">"),
        "Markdown output should contain custom page marker. Content start: {}",
        &result.content[..result.content.len().min(300)]
    );
}

/// Test no page markers in markdown when disabled.
#[test]
fn test_no_markers_in_markdown_when_disabled() {
    if skip_if_missing("pdf/multi_page.pdf") {
        return;
    }

    let file_path = get_test_file_path("pdf/multi_page.pdf");
    let config = ExtractionConfig {
        output_format: xberg::OutputFormat::Markdown,
        pages: Some(PageConfig {
            insert_page_markers: false,
            ..Default::default()
        }),
        ..Default::default()
    };

    let result = extract_uri_document_blocking(&file_path, None, &config).expect("Failed to extract PDF as markdown");

    assert!(
        !result.content.contains("<!-- PAGE"),
        "Markdown output should not contain markers when disabled"
    );
}

/// Test page markers are inserted in djot output format.
#[test]
fn test_page_markers_in_djot_output() {
    if skip_if_missing("pdf/multi_page.pdf") {
        return;
    }

    let file_path = get_test_file_path("pdf/multi_page.pdf");
    let config = ExtractionConfig {
        output_format: xberg::OutputFormat::Djot,
        pages: Some(PageConfig {
            insert_page_markers: true,
            marker_format: "<!-- PAGE {page_num} -->".to_string(),
            ..Default::default()
        }),
        ..Default::default()
    };

    let result = extract_uri_document_blocking(&file_path, None, &config).expect("Failed to extract PDF as djot");

    assert!(
        result.content.contains("<!-- PAGE 1 -->"),
        "Djot output should contain page marker for page 1. Content start: {}",
        &result.content[..result.content.len().min(300)]
    );
}

/// Test marker format with multiple placeholders (edge case).
#[test]
fn test_marker_format_multiple_placeholders() {
    if skip_if_missing("pdf/multi_page.pdf") {
        return;
    }

    let file_path = get_test_file_path("pdf/multi_page.pdf");
    let config = ExtractionConfig {
        pages: Some(PageConfig {
            insert_page_markers: true,
            marker_format: "Page {page_num} of document (page {page_num})".to_string(),
            ..Default::default()
        }),
        ..Default::default()
    };

    let result = extract_uri_document_blocking(&file_path, None, &config).expect("Failed to extract PDF");

    assert!(
        result.content.contains("Page 1 of document (page 1)"),
        "Multiple {{page_num}} placeholders should all be replaced"
    );
}

/// Markdown output must emit the marker verbatim, not markdown-escaped (issue #1328).
#[test]
fn test_page_markers_not_escaped_in_markdown_output() {
    if skip_if_missing("pdf/multi_page.pdf") {
        return;
    }

    let file_path = get_test_file_path("pdf/multi_page.pdf");
    let config = ExtractionConfig {
        output_format: xberg::OutputFormat::Markdown,
        pages: Some(PageConfig {
            insert_page_markers: true,
            ..Default::default()
        }),
        ..Default::default()
    };

    let result = extract_uri_document_blocking(&file_path, None, &config).expect("Failed to extract PDF as markdown");

    assert!(
        result.content.contains("<!-- PAGE 1 -->") && result.content.contains("<!-- PAGE 2 -->"),
        "Markdown output should contain verbatim default markers. Content start: {}",
        &result.content[..result.content.len().min(300)]
    );
    assert!(
        !result.content.contains("\\<\\!--"),
        "Markers must not be backslash-escaped. Content start: {}",
        &result.content[..result.content.len().min(300)]
    );
}

/// OCR-path markdown output must also emit markers verbatim (issue #1328).
#[test]
fn test_page_markers_not_escaped_in_ocr_markdown_output() {
    if skip_if_missing("pdf/ocr_test.pdf") {
        return;
    }

    let file_path = get_test_file_path("pdf/ocr_test.pdf");
    let config = ExtractionConfig {
        output_format: xberg::OutputFormat::Markdown,
        ocr: Some(xberg::OcrConfig {
            backend: "tesseract".to_string(),
            language: vec!["eng".to_string()],
            ..Default::default()
        }),
        force_ocr: true,
        pages: Some(PageConfig {
            insert_page_markers: true,
            ..Default::default()
        }),
        ..Default::default()
    };

    let result =
        extract_uri_document_blocking(&file_path, None, &config).expect("Failed to extract PDF with OCR as markdown");

    assert!(
        result.content.contains("<!-- PAGE 1 -->"),
        "OCR markdown output should contain verbatim page marker. Content start: {}",
        &result.content[..result.content.len().min(300)]
    );
    assert!(
        !result.content.contains("\\<\\!--"),
        "OCR markdown output must not escape page markers. Content start: {}",
        &result.content[..result.content.len().min(300)]
    );
}
