//! Regression tests for issues #129 and #146 (HTML extractor).
//!
//! #129: `<math>` (MathML) elements are converted to LaTeX `Formula` elements instead of
//! leaking a raw `<!-- MathML: ... -->` comment plus flattened token-soup text.
//! #146: `<table><caption>` text now reaches the element stream instead of being dropped
//! (verified: `html-to-markdown-rs`'s `TableGrid` has no caption field at all).
//!
//! Requires `--features html,office` (MathML conversion is gated behind `office`, which
//! provides the `roxmltree` dependency the shared MathML-to-LaTeX converter needs).

use xberg::core::config::ExtractionConfig;

mod helpers;
use helpers::extract_bytes_document;

/// #129: a `<math>` element (the same `1/2` fraction fixture already covered by
/// `extraction::html::structure`'s own unit test) converts to `\frac{1}{2}` LaTeX and the
/// raw `<!-- MathML: ... -->` comment does not leak into rendered content.
#[tokio::test]
#[cfg(feature = "office")]
async fn issue_129_mathml_converts_to_latex_formula() {
    let html = r#"<html><body><p>Before</p><math xmlns="http://www.w3.org/1998/Math/MathML"><mfrac><mn>1</mn><mn>2</mn></mfrac></math><p>After</p></body></html>"#;

    let result = extract_bytes_document(html.as_bytes(), "text/html", &ExtractionConfig::default())
        .await
        .expect("html extraction should succeed");

    assert!(
        result.content.contains("\\frac{1}{2}"),
        "expected LaTeX fraction in content, got: {}",
        result.content
    );
    assert!(
        !result.content.contains("MathML"),
        "raw MathML comment should not leak into rendered content: {}",
        result.content
    );
    assert!(result.content.contains("Before"));
    assert!(result.content.contains("After"));
}

/// #146: a `<table><caption>` reaches the rendered element stream instead of vanishing.
#[tokio::test]
async fn issue_146_table_caption_reaches_element_stream() {
    let html = r#"<html><body>
<table>
<caption>CAPTIONMARKER Quarterly Results</caption>
<tr><th>Q1</th><th>Q2</th></tr>
<tr><td>10</td><td>20</td></tr>
</table>
</body></html>"#;

    let result = extract_bytes_document(html.as_bytes(), "text/html", &ExtractionConfig::default())
        .await
        .expect("html extraction should succeed");

    assert!(
        result.content.contains("CAPTIONMARKER"),
        "table caption text should reach the element stream, not be dropped: {}",
        result.content
    );
    assert!(!result.tables.is_empty(), "table itself should still be extracted");
}
