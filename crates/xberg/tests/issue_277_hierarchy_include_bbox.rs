#![allow(clippy::print_stdout, clippy::print_stderr, clippy::dbg_macro)] // ~keep: test/bench binaries print by design; org logging policy exempts tests
//! Regression test for issue #277: `HierarchyConfig::include_bbox` had zero readers.
//!
//! `HierarchyConfig::k_clusters` (its sibling) was already threaded into
//! `extract_all_from_oxide_document` (crates/xberg/src/extractors/pdf/extraction.rs), so the
//! struct was reached during extraction — only `include_bbox` was ignored, meaning bounding-box
//! coordinates were always attached to hierarchy blocks regardless of the configured value.

#![cfg(feature = "pdf")]

mod helpers;
use helpers::extract_bytes_document;

use std::path::Path;
use xberg::core::config::{ExtractionConfig, HierarchyConfig, PageConfig, PdfConfig};

/// With `include_bbox: false`, every hierarchy block on every page must have `bbox = None`.
///
/// This is the direct counterpart to `pdf_hierarchy_detection.rs::test_full_hierarchy_extraction`,
/// which uses the same fixture with `include_bbox: true` and asserts blocks are allowed to carry
/// bbox data. Before the fix, `include_bbox: false` had no effect at all: bbox was always
/// populated because nothing ever read the field.
#[tokio::test]
async fn hierarchy_include_bbox_false_strips_bbox_from_all_blocks() {
    let pdf_path = "../../test_documents/pdf/embedded_images_tables.pdf";

    if !Path::new(pdf_path).exists() {
        eprintln!("Test PDF not found at: {}", pdf_path);
        return;
    }

    let pdf_bytes = std::fs::read(pdf_path).expect("Failed to read PDF file");

    let config = ExtractionConfig {
        pages: Some(PageConfig {
            extract_pages: true,
            ..Default::default()
        }),
        pdf_options: Some(PdfConfig {
            extract_images: false,
            extract_tables: true,
            passwords: None,
            extract_metadata: true,
            hierarchy: Some(HierarchyConfig {
                enabled: true,
                k_clusters: 6,
                include_bbox: false,
            }),
            extract_annotations: false,
            ..Default::default()
        }),
        ..Default::default()
    };

    let result = extract_bytes_document(&pdf_bytes, "application/pdf", &config)
        .await
        .expect("PDF extraction failed");

    let pages = result.pages.as_ref().expect("pages should be extracted");
    assert!(!pages.is_empty(), "at least one page should be extracted");

    let mut total_blocks = 0usize;
    for page in pages {
        let Some(hierarchy) = page.hierarchy.as_ref() else {
            continue;
        };
        for block in &hierarchy.blocks {
            total_blocks += 1;
            assert_eq!(
                block.bbox,
                None,
                "block '{}' must have bbox = None when include_bbox = false",
                block.text.chars().take(40).collect::<String>()
            );
        }
    }

    assert!(
        total_blocks > 0,
        "fixture must produce at least one hierarchy block to make this assertion meaningful"
    );
}
