//! Regression tests for issues #273, #123, #142.
//!
//! #273: `MdxExtractor::build_internal_document` was a ~470-line copy-paste fork of
//! `MarkdownExtractor::build_internal_document` that had drifted and handled zero
//! occurrences of `Event::InlineMath`, `Event::DisplayMath`, `Event::InlineHtml`,
//! `Event::Superscript`, `Event::Subscript`, and `Event::DefinitionListTitle`. Both
//! extractors now share a single builder (`MarkdownExtractor::build_internal_document`),
//! parameterised by an extra `raw_jsx_blocks` slice for MDX-specific JSX recording.
//!
//! #123: the MDX parser now uses `markdown_options()` (the same twelve-flag pulldown-cmark
//! option set as the Markdown extractor) instead of a three-flag subset.
//!
//! #142: MDX inline JSX expressions (`{expr}`) and inline component props
//! (`<Chart data={data} />` embedded in prose) are now recorded as raw JSX blocks instead
//! of being silently discarded.
//!
//! Requires `--features mdx` (Markdown itself is always compiled).

// Every test below is `#[cfg(feature = "mdx")]`, but the imports and fixture below were not,
// so a build without `mdx` left them unused — three warnings that `-D warnings` turns into
// errors. Gate the whole file instead of each item.
#![cfg(feature = "mdx")]

use xberg::core::config::ExtractionConfig;

mod helpers;
use helpers::extract_bytes_document;

/// Markdown source exercising every event kind the #273 audit found missing from MDX:
/// inline math, display math, inline raw HTML, superscript, subscript, and a definition
/// list title/description pair.
const SHARED_SOURCE: &str = "\
# Doc Title

Inline math $x^{SUPI}$ stays inline.

$$y = mc^{FORMULAMARK}$$

Raw inline HTML: <mark>HTMLMARK</mark> stays.

SUPBASE^SUPMARK^ and SUBBASE~SUBMARK~ preserved.

TERMWORD
: DEFINITIONTEXT for TERMWORD.
";

/// #273: feeds identical source through both the `.md` and `.mdx` extraction paths and
/// asserts BOTH recover every construct — proving the drift between the two builders is
/// fixed, not just that the code was refactored.
#[tokio::test]
#[cfg(feature = "mdx")]
async fn issue_273_md_and_mdx_paths_preserve_math_html_super_sub_deflist() {
    let config = ExtractionConfig::default();

    let md_result = extract_bytes_document(SHARED_SOURCE.as_bytes(), "text/markdown", &config)
        .await
        .expect("markdown extraction should succeed");
    let mdx_result = extract_bytes_document(SHARED_SOURCE.as_bytes(), "text/mdx", &config)
        .await
        .expect("mdx extraction should succeed");

    for (label, content) in [("markdown", &md_result.content), ("mdx", &mdx_result.content)] {
        assert!(
            content.contains("SUPI"),
            "{label}: inline math content dropped: {content}"
        );
        assert!(
            content.contains("FORMULAMARK"),
            "{label}: display math content dropped: {content}"
        );
        assert!(
            content.contains("HTMLMARK"),
            "{label}: inline raw HTML content dropped: {content}"
        );
        assert!(
            content.contains("SUPMARK"),
            "{label}: superscript content dropped: {content}"
        );
        assert!(
            content.contains("SUBMARK"),
            "{label}: subscript content dropped: {content}"
        );
        assert!(
            content.contains("TERMWORD"),
            "{label}: definition list term dropped: {content}"
        );
        assert!(
            content.contains("DEFINITIONTEXT"),
            "{label}: definition list description dropped: {content}"
        );
    }
}

/// #123: MDX now parses with the full `markdown_options()` set. GFM alerts
/// (`> [!WARNING]`) require `ENABLE_GFM`, which the old three-flag MDX option set did not
/// enable — the `[!WARNING]` marker used to leak into the body text verbatim instead of
/// being consumed as an admonition marker.
#[tokio::test]
#[cfg(feature = "mdx")]
async fn issue_123_mdx_uses_full_markdown_options_gfm_alert() {
    let content = b"> [!WARNING]\n> Be careful ALERTMARK here.\n";
    let result = extract_bytes_document(content, "text/mdx", &ExtractionConfig::default())
        .await
        .expect("mdx extraction should succeed");

    assert!(
        result.content.contains("ALERTMARK"),
        "alert body text missing: {}",
        result.content
    );
    assert!(
        !result.content.contains("[!WARNING]"),
        "GFM alert marker should be consumed, not leaked verbatim: {}",
        result.content
    );
}

/// #142: an inline JSX expression embedded in prose (not a standalone `{expr}` line) used
/// to be silently deleted. It's now recorded as a raw JSX block, which renders back into
/// content instead of vanishing.
#[tokio::test]
#[cfg(feature = "mdx")]
async fn issue_142_inline_jsx_expression_in_prose_recorded() {
    let content = b"The count is {countValue} today.\n";
    let result = extract_bytes_document(content, "text/mdx", &ExtractionConfig::default())
        .await
        .expect("mdx extraction should succeed");

    assert!(
        result.content.contains("countValue"),
        "inline JSX expression should be recorded as a raw block, not dropped: {}",
        result.content
    );
}

/// #142: an inline self-closing JSX component's props embedded in prose used to be
/// dropped entirely (only captured when the *whole line* went empty after tag stripping).
#[tokio::test]
#[cfg(feature = "mdx")]
async fn issue_142_inline_component_props_in_prose_recorded() {
    let content = b"See below: <Chart dataProp=\"lineChartValue\" /> for details.\n";
    let result = extract_bytes_document(content, "text/mdx", &ExtractionConfig::default())
        .await
        .expect("mdx extraction should succeed");

    assert!(
        result.content.contains("lineChartValue"),
        "inline component props should be recorded as a raw block, not dropped: {}",
        result.content
    );
    assert!(
        result.content.contains("See below"),
        "surrounding prose must survive: {}",
        result.content
    );
    assert!(
        result.content.contains("for details"),
        "surrounding prose must survive: {}",
        result.content
    );
}
