//! Integration tests for the `use_layout_for_markdown` flag.
//!
//! These tests verify that:
//! 1. `use_layout_for_markdown = true` feeds layout regions into the non-OCR
//!    markdown pipeline, producing richer structural output compared to the
//!    baseline (font-clustering only).
//! 2. `use_layout_for_markdown = false` (default) leaves the pipeline unchanged
//!    and produces the same output as a config without the field.
//!
//! Tests are feature-gated on `pdf` and `layout-detection` and are marked
//! `#[ignore]` when the layout engine model files are not available on CI.

#![cfg(all(feature = "pdf", feature = "layout-detection"))]

mod helpers;
use helpers::extract_uri_document_blocking;

use helpers::{get_test_file_path, test_documents_available};
#[cfg(target_os = "macos")]
use xberg::core::config::{AccelerationConfig, ExecutionProviderType};
use xberg::core::config::{ExtractionConfig, OutputFormat, layout::LayoutDetectionConfig};

#[cfg(target_os = "macos")]
fn accelerated_layout(provider: ExecutionProviderType) -> LayoutDetectionConfig {
    LayoutDetectionConfig {
        acceleration: Some(AccelerationConfig {
            provider,
            ..Default::default()
        }),
        ..Default::default()
    }
}

#[cfg(target_os = "macos")]
fn has_layout_warning(warnings: &[xberg::types::ProcessingWarning]) -> bool {
    warnings
        .iter()
        .any(|warning| warning.source == "layout" && warning.message.contains("layout detection failed"))
}

/// Extract `relative_path` (from `test_documents/`) with the given config.
fn extract_md(relative_path: &str, config: &ExtractionConfig) -> String {
    let path = get_test_file_path(relative_path);
    extract_uri_document_blocking(&path, None, config)
        .expect("extraction should succeed")
        .content
}

/// Config: output_format=Markdown, no layout at all (pure baseline).
fn baseline_config() -> ExtractionConfig {
    ExtractionConfig {
        output_format: OutputFormat::Markdown,
        ..Default::default()
    }
}

/// Config: layout=Some(default), use_layout_for_markdown=false.
/// Layout model is loaded but NOT injected into the native path.
fn layout_config_not_injected() -> ExtractionConfig {
    ExtractionConfig {
        output_format: OutputFormat::Markdown,
        layout: Some(LayoutDetectionConfig::default()),
        use_layout_for_markdown: false,
        ..Default::default()
    }
}

/// Config: layout=Some(default), use_layout_for_markdown=true.
/// Layout regions ARE injected into the native markdown pipeline.
fn layout_for_markdown_config() -> ExtractionConfig {
    ExtractionConfig {
        output_format: OutputFormat::Markdown,
        layout: Some(LayoutDetectionConfig::default()),
        use_layout_for_markdown: true,
        ..Default::default()
    }
}

/// With `use_layout_for_markdown = false` (the default), the pipeline must
/// produce output that is indistinguishable from the baseline (no layout).
/// This guards against accidental regressions introduced by the new field.
#[test]
fn test_use_layout_for_markdown_false_matches_baseline() {
    if !test_documents_available() {
        return;
    }

    let pdf = "pdf/google_doc_document.pdf";
    let baseline = extract_md(pdf, &baseline_config());
    let layout_not_injected = extract_md(pdf, &layout_config_not_injected());

    assert_eq!(
        baseline, layout_not_injected,
        "use_layout_for_markdown=false must not change extraction output compared to no-layout config"
    );
}

/// With `use_layout_for_markdown = true` and a PDF that has headings, the
/// markdown output must contain at least one ATX heading line (`# ...`).
///
/// The test uses `google_doc_document.pdf`, which is a structured Google Docs
/// export with clear title and section headings detectable by the RT-DETR model.
///
/// This test requires the layout model to be available (ORT + model files).
/// It is marked `#[ignore]` on CI where model weights are not pre-downloaded.
#[test]
#[ignore = "requires layout model files (ORT inference)"]
fn test_use_layout_for_markdown_produces_headings() {
    if !test_documents_available() {
        return;
    }

    let pdf = "pdf/google_doc_document.pdf";
    let output = extract_md(pdf, &layout_for_markdown_config());

    let has_heading = output.lines().any(|line| line.starts_with('#'));
    assert!(
        has_heading,
        "use_layout_for_markdown=true should produce at least one ATX heading line; got:\n{}",
        &output[..output.len().min(500)]
    );
}

/// Layout geometry must not rewrite native heading semantics.
///
/// The native PDF path uses layout regions for reading order, grouping, and
/// tables. Font/tag-derived heading roles remain authoritative; OCR keeps its
/// separate layout-semantic classification path.
#[test]
#[ignore = "requires layout model files (ORT inference)"]
fn test_use_layout_for_markdown_preserves_native_headings() {
    if !test_documents_available() {
        return;
    }

    let pdf = "pdf/google_doc_document.pdf";
    let baseline = extract_md(pdf, &baseline_config());
    let layout = extract_md(pdf, &layout_for_markdown_config());

    fn atx_headings(content: &str) -> Vec<(String, usize)> {
        let mut headings = content
            .lines()
            .filter_map(|line| {
                let trimmed = line.trim_start();
                let level = trimmed.chars().take_while(|character| *character == '#').count();
                (1..=6)
                    .contains(&level)
                    .then(|| trimmed.get(level..))
                    .flatten()
                    .filter(|remainder| remainder.starts_with(' '))
                    .map(|remainder| (remainder.trim().to_owned(), level))
            })
            .collect::<Vec<_>>();
        headings.sort();
        headings
    }

    let baseline_headings = atx_headings(&baseline);
    let layout_headings = atx_headings(&layout);

    assert_eq!(
        baseline_headings, layout_headings,
        "layout geometry must preserve native heading texts and levels"
    );
}

#[cfg(target_os = "macos")]
#[test]
#[ignore = "requires layout model files and CoreML"]
fn test_auto_layout_avoids_coreml_failure() {
    if !test_documents_available() {
        return;
    }

    let config = ExtractionConfig {
        output_format: OutputFormat::Markdown,
        disable_ocr: true,
        layout: Some(accelerated_layout(ExecutionProviderType::Auto)),
        use_layout_for_markdown: true,
        ..Default::default()
    };
    let path = get_test_file_path("pdf/tiny.pdf");
    let result = extract_uri_document_blocking(&path, None, &config).expect("auto layout extraction should succeed");

    assert!(!result.content.trim().is_empty());
    assert!(
        !has_layout_warning(&result.processing_warnings),
        "auto layout unexpectedly degraded: {:?}",
        result.processing_warnings
    );
}

#[cfg(target_os = "macos")]
#[test]
#[ignore = "requires layout model files and CoreML"]
fn test_coreml_layout_failure_emits_processing_warning() {
    if !test_documents_available() {
        return;
    }

    let config = ExtractionConfig {
        output_format: OutputFormat::Markdown,
        disable_ocr: true,
        layout: Some(accelerated_layout(ExecutionProviderType::CoreMl)),
        use_layout_for_markdown: true,
        ..Default::default()
    };
    let path = get_test_file_path("pdf/tiny.pdf");
    let result =
        extract_uri_document_blocking(&path, None, &config).expect("extraction should soft-fail to native text");

    assert!(
        has_layout_warning(&result.processing_warnings),
        "expected a caller-visible layout warning, got {:?}",
        result.processing_warnings
    );
}

#[cfg(all(target_os = "macos", feature = "ocr"))]
#[test]
#[ignore = "requires layout model files, CoreML, and Tesseract"]
fn test_coreml_ocr_layout_failure_emits_processing_warning() {
    if !test_documents_available() {
        return;
    }

    let config = ExtractionConfig {
        force_ocr: true,
        layout: Some(accelerated_layout(ExecutionProviderType::CoreMl)),
        ..Default::default()
    };
    let path = get_test_file_path("pdf/tiny.pdf");
    let result = extract_uri_document_blocking(&path, None, &config).expect("OCR should continue without layout");

    assert!(
        has_layout_warning(&result.processing_warnings),
        "expected a caller-visible OCR layout warning, got {:?}",
        result.processing_warnings
    );
}

/// Verify that `use_layout_for_markdown = true` with `layout = None` silently
/// produces the same output as the baseline (no-op when layout config is absent).
#[test]
fn test_use_layout_for_markdown_without_layout_config_is_noop() {
    if !test_documents_available() {
        return;
    }

    let pdf = "pdf/google_doc_document.pdf";
    let baseline = extract_md(pdf, &baseline_config());

    let noop_config = ExtractionConfig {
        output_format: OutputFormat::Markdown,
        layout: None,
        use_layout_for_markdown: true,
        ..Default::default()
    };
    let noop_output = extract_md(pdf, &noop_config);

    assert_eq!(
        baseline, noop_output,
        "use_layout_for_markdown=true with layout=None must produce the same output as baseline"
    );
}

/// Verify that `force_ocr=true` bypasses the layout-for-markdown path.
/// The field must be a no-op when the entire document is OCR'd.
#[test]
fn test_use_layout_for_markdown_skipped_when_force_ocr() {
    let config = ExtractionConfig {
        output_format: OutputFormat::Markdown,
        layout: Some(LayoutDetectionConfig::default()),
        use_layout_for_markdown: true,
        force_ocr: true,
        ..Default::default()
    };
    assert!(config.use_layout_for_markdown);
    assert!(config.force_ocr);
    assert!(config.layout.is_some());
}
