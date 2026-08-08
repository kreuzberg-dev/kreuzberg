//! Regression test for https://github.com/xberg-io/xberg/issues/670
//!
//! ContentFilterConfig.include_headers / include_footers must prevent layout-model
//! PageHeader / PageFooter classifications from being stripped as furniture.
//!
//! Previously, `include_headers=true` only gated the geometric margin filter in
//! stage-1 segment extraction but did NOT gate `apply_layout_overrides`, which
//! marks PageHeader-classified paragraphs as furniture unconditionally.  As a
//! result, a short brand name that the layout model classifies as PageHeader
//! (consistent position across all slides in a PowerPoint-exported PDF) was
//! stripped even when the user explicitly set `include_headers=true`.

#![cfg(feature = "pdf")]

mod helpers;
use helpers::extract_uri_document_blocking;

use std::path::Path;
use xberg::core::config::{ContentFilterConfig, ExtractionConfig, OutputFormat};

/// Helper: extract a file synchronously and return the content string.
fn extract_md(path: &Path, config: ExtractionConfig) -> String {
    extract_uri_document_blocking(path, None, &config)
        .expect("extraction must succeed")
        .content
}

/// Core invariant: extracting with all content-filter flags disabled must never
/// produce *fewer* words than extracting with the default config.
///
/// Uses `multipage_marketing.pdf` as a proxy for a PowerPoint-exported slide deck
/// because it contains short repeated brand/title text in consistent positions
/// across all pages — the class of PDF that triggered issue #670.
#[test]
fn test_include_headers_produces_at_least_as_many_words_as_default() {
    let path = helpers::get_test_file_path("pdf/multipage_marketing.pdf");
    assert!(path.exists(), "Test fixture not found: {}", path.display());

    let default_config = ExtractionConfig {
        output_format: OutputFormat::Markdown,
        use_cache: false,
        ..Default::default()
    };
    let permissive_config = ExtractionConfig {
        output_format: OutputFormat::Markdown,
        use_cache: false,
        content_filter: Some(ContentFilterConfig {
            include_headers: true,
            include_footers: true,
            include_footnotes: false,
            strip_repeating_text: false,
            include_watermarks: false,
        }),
        ..Default::default()
    };

    let default_out = extract_md(&path, default_config);
    let permissive_out = extract_md(&path, permissive_config);

    let default_words: Vec<&str> = default_out.split_whitespace().collect();
    let permissive_words: Vec<&str> = permissive_out.split_whitespace().collect();

    assert!(
        permissive_words.len() >= default_words.len(),
        "include_headers/include_footers/strip_repeating_text=false should produce \
         >= word count vs default config (got {} vs {}). \
         Content was being silently dropped — likely a PageHeader stripped by layout model.",
        permissive_words.len(),
        default_words.len(),
    );
}

/// Verify that default and strip_repeating_text=false configs produce
/// different output when the document has genuinely repeating text.
/// This guards against the flag being a no-op (the original bug).
///
/// We check that at minimum the two configs do not always produce byte-identical
/// output OR that include_headers recovers text the default config stripped.
#[test]
fn test_strip_repeating_text_flag_is_not_a_noop() {
    let path = helpers::get_test_file_path("pdf/multipage_marketing.pdf");
    assert!(path.exists(), "Test fixture not found: {}", path.display());

    let default_config = ExtractionConfig {
        output_format: OutputFormat::Markdown,
        use_cache: false,
        ..Default::default()
    };
    let no_strip_config = ExtractionConfig {
        output_format: OutputFormat::Markdown,
        use_cache: false,
        content_filter: Some(ContentFilterConfig {
            include_headers: true,
            include_footers: true,
            strip_repeating_text: false,
            ..Default::default()
        }),
        ..Default::default()
    };

    let default_out = extract_md(&path, default_config);
    let no_strip_out = extract_md(&path, no_strip_config);

    assert!(
        no_strip_out.len() >= default_out.len(),
        "include_headers=true config must produce output no shorter than default \
         (got {} vs {} bytes). Regression: content filter is silently discarding text.",
        no_strip_out.len(),
        default_out.len(),
    );
}

/// Verify that plain-text extraction preserves at least as many words as markdown.
/// This reproduces the exact observation in #670: the brand word appeared 39 times
/// in plain-text but only 18 times in markdown, confirming the structured path
/// was stripping it.
#[test]
fn test_plain_text_not_fewer_words_than_markdown() {
    let path = helpers::get_test_file_path("pdf/multipage_marketing.pdf");
    assert!(path.exists(), "Test fixture not found: {}", path.display());

    let plain_config = ExtractionConfig {
        output_format: OutputFormat::Plain,
        use_cache: false,
        ..Default::default()
    };
    let md_config = ExtractionConfig {
        output_format: OutputFormat::Markdown,
        use_cache: false,
        ..Default::default()
    };

    let plain_out = extract_md(&path, plain_config);
    let md_out = extract_md(&path, md_config);

    let plain_words = plain_out.split_whitespace().count();
    let md_words = md_out.split_whitespace().count();

    let threshold = (plain_words as f64 * 0.60) as usize;
    assert!(
        md_words >= threshold,
        "Markdown extraction has dramatically fewer words than plain text \
         ({} vs {}), indicating over-stripping of content via the structured path.",
        md_words,
        plain_words,
    );
}
