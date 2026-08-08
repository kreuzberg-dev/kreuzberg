//! Regression tests for issues #135, #145, #225 (Markdown extractor).
//!
//! #135: block-level raw HTML (a bare `<div>...</div>` between blank lines) used to be
//! dropped silently — now recorded as a raw block.
//! #145: inline SVG data URIs (`data:image/svg+xml;base64,...`) used to be dropped by
//! `decode_data_uri_image` (only png/jpeg/gif/webp were recognized) — now decoded.
//! #225: TOML frontmatter (`+++...+++`, Hugo/Zola style) used to never be recognized —
//! `extract_frontmatter` only handled `---` YAML delimiters.
//!
//! Markdown is always compiled (no feature flag required).

use xberg::core::config::ExtractionConfig;

mod helpers;
use helpers::extract_bytes_document;

/// #135: a block-level `<div>` between blank lines (not inside a paragraph) is recorded and
/// survives to rendered output instead of vanishing.
#[tokio::test]
async fn issue_135_block_level_raw_html_captured() {
    let content = b"Intro paragraph.\n\n<div class=\"callout\">\nBLOCKHTMLMARKER\n</div>\n\nOutro paragraph.\n";
    let result = extract_bytes_document(content, "text/markdown", &ExtractionConfig::default())
        .await
        .expect("markdown extraction should succeed");

    assert!(
        result.content.contains("BLOCKHTMLMARKER"),
        "block-level raw HTML should be preserved as a raw block, not dropped: {}",
        result.content
    );
    assert!(
        result.content.contains("Intro paragraph"),
        "surrounding content must survive: {}",
        result.content
    );
    assert!(
        result.content.contains("Outro paragraph"),
        "surrounding content must survive: {}",
        result.content
    );
}

/// #145: an inline SVG data URI is decoded into an extracted image instead of being
/// silently dropped.
#[tokio::test]
async fn issue_145_inline_svg_data_uri_decoded() {
    let svg = r#"<svg xmlns="http://www.w3.org/2000/svg"><rect width="1" height="1"/></svg>"#;
    let svg_b64 = base64_encode(svg.as_bytes());
    let content = format!("Intro.\n\n![a diagram](data:image/svg+xml;base64,{svg_b64})\n\nOutro.\n");

    let result = extract_bytes_document(content.as_bytes(), "text/markdown", &ExtractionConfig::default())
        .await
        .expect("markdown extraction should succeed");

    let images = result
        .images
        .expect("SVG data URI should decode into an extracted image");
    assert_eq!(images.len(), 1, "expected exactly one decoded image: {images:?}");
    assert_eq!(images[0].format.as_ref(), "svg");
}

/// #225: TOML frontmatter (`+++`) is recognized and its `title` field reaches document
/// metadata, mirroring how `---` YAML frontmatter already works.
#[tokio::test]
async fn issue_225_toml_frontmatter_recognized() {
    let content = b"+++\ntitle = \"My Zola Post\"\nauthor = \"Jane Doe\"\n+++\n\n# Body\n\nBODYMARKER\n";
    let result = extract_bytes_document(content, "text/markdown", &ExtractionConfig::default())
        .await
        .expect("markdown extraction should succeed");

    assert_eq!(result.metadata.title.as_deref(), Some("My Zola Post"));
    assert_eq!(result.metadata.created_by.as_deref(), Some("Jane Doe"));
    assert!(
        !result.content.contains("title = \"My Zola Post\""),
        "raw TOML frontmatter must not leak into the body: {}",
        result.content
    );
    assert!(
        result.content.contains("BODYMARKER"),
        "body content must survive: {}",
        result.content
    );
}

/// Minimal base64 encoder (avoids adding a new dependency just for this test file).
fn base64_encode(data: &[u8]) -> String {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0];
        let b1 = *chunk.get(1).unwrap_or(&0);
        let b2 = *chunk.get(2).unwrap_or(&0);
        let n = ((b0 as u32) << 16) | ((b1 as u32) << 8) | (b2 as u32);
        out.push(CHARS[((n >> 18) & 0x3F) as usize] as char);
        out.push(CHARS[((n >> 12) & 0x3F) as usize] as char);
        out.push(if chunk.len() > 1 {
            CHARS[((n >> 6) & 0x3F) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            CHARS[(n & 0x3F) as usize] as char
        } else {
            '='
        });
    }
    out
}
