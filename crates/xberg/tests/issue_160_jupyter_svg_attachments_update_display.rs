//! Regression tests for issue #160.
//!
//! `JupyterExtractor` ignored three pieces of Jupyter notebook content:
//!
//! - `image/svg+xml` output data (present but never turned into an
//!   `ExtractedImage`, only ever a discarded placeholder string).
//! - Cell/markdown `attachments` (base64-embedded resources referenced from
//!   markdown source via `attachment:` URIs).
//! - `update_display_data` output messages (silently matched by the catch-all
//!   `_ => {}` arm alongside genuinely unknown output types).

#![cfg(feature = "notebook")]

use xberg::core::config::ExtractionConfig;

mod helpers;
use helpers::extract_bytes_document_blocking;

/// PNG magic bytes: `\x89PNG\r\n\x1a\n`.
const PNG_MAGIC: &[u8] = b"\x89PNG\r\n\x1a\n";

/// A valid 1x1 transparent PNG, base64-encoded (reused from the existing
/// jupyter output-image fixtures in `jupyter_extractor_tests.rs`).
const PNG_1X1_BASE64: &str =
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAAC0lEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==";

const SVG_MARKUP: &str = "<svg xmlns=\"http://www.w3.org/2000/svg\"><rect width=\"1\" height=\"1\"/></svg>";

/// A `display_data` output whose only representation is `image/svg+xml` must
/// be turned into an `ExtractedImage` with `format == "svg"` carrying the raw
/// SVG markup as its bytes, not silently dropped.
#[test]
fn should_extract_svg_output_as_image() {
    let config = ExtractionConfig::default();

    let notebook = format!(
        r#"{{
        "cells": [
            {{
                "cell_type": "code",
                "source": ["render_svg()"],
                "execution_count": 1,
                "outputs": [
                    {{
                        "output_type": "display_data",
                        "data": {{"image/svg+xml": ["{svg}"]}},
                        "metadata": {{}}
                    }}
                ],
                "metadata": {{}}
            }}
        ],
        "metadata": {{}},
        "nbformat": 4,
        "nbformat_minor": 5
    }}"#,
        svg = SVG_MARKUP.replace('"', "\\\"")
    );

    let result = extract_bytes_document_blocking(notebook.as_bytes(), "application/x-ipynb+json", &config)
        .expect("notebook extraction should succeed");

    let images = result.images.as_ref().expect("images must be populated");
    let svg_images: Vec<_> = images.iter().filter(|img| img.format == "svg").collect();

    assert_eq!(
        svg_images.len(),
        1,
        "expected exactly one svg image, got images: {:?}",
        images
    );
    assert_eq!(
        svg_images[0].data.as_ref(),
        SVG_MARKUP.as_bytes(),
        "svg image bytes must match the raw markup exactly"
    );
}

/// A markdown cell's `attachments` map must be extracted into
/// `ExtractedImage` entries, not silently ignored.
#[test]
fn should_extract_markdown_cell_attachment_as_image() {
    let config = ExtractionConfig::default();

    let notebook = format!(
        r#"{{
        "cells": [
            {{
                "cell_type": "markdown",
                "source": ["![pic](attachment:pic.png)"],
                "attachments": {{
                    "pic.png": {{"image/png": "{png}"}}
                }},
                "metadata": {{}}
            }}
        ],
        "metadata": {{}},
        "nbformat": 4,
        "nbformat_minor": 5
    }}"#,
        png = PNG_1X1_BASE64
    );

    let result = extract_bytes_document_blocking(notebook.as_bytes(), "application/x-ipynb+json", &config)
        .expect("notebook extraction should succeed");

    let images = result.images.as_ref().expect("images must be populated");
    let png_images: Vec<_> = images.iter().filter(|img| img.format == "png").collect();

    assert_eq!(
        png_images.len(),
        1,
        "expected exactly one attachment image, got images: {:?}",
        images
    );
    assert!(
        png_images[0].data.starts_with(PNG_MAGIC),
        "attachment image bytes must be the decoded PNG"
    );
    assert!(
        png_images[0]
            .description
            .as_deref()
            .is_some_and(|d| d.contains("pic.png")),
        "attachment image description should name the attachment file, got: {:?}",
        png_images[0].description
    );
}

/// An `update_display_data` output message must contribute its text content
/// like `display_data`/`execute_result`, not be silently matched by a
/// catch-all "unknown output type" arm.
#[test]
fn should_extract_update_display_data_output() {
    let config = ExtractionConfig::default();

    let notebook = br#"{
        "cells": [
            {
                "cell_type": "code",
                "source": ["update_widget()"],
                "execution_count": 1,
                "outputs": [
                    {
                        "output_type": "update_display_data",
                        "data": {"text/plain": ["updated widget text"]},
                        "metadata": {},
                        "transient": {"display_id": "widget-1"}
                    }
                ],
                "metadata": {}
            }
        ],
        "metadata": {},
        "nbformat": 4,
        "nbformat_minor": 5
    }"#;

    let result = extract_bytes_document_blocking(notebook, "application/x-ipynb+json", &config)
        .expect("notebook extraction should succeed");

    assert_eq!(
        result.content.matches("updated widget text").count(),
        1,
        "update_display_data output content must be extracted exactly once, got: {:?}",
        result.content
    );
}

/// An attachment with an unsupported MIME type cannot be turned into an
/// image; it must be skipped with a `ProcessingWarning` naming the
/// attachment file, not silently dropped without a trace.
#[test]
fn should_warn_when_attachment_mime_type_is_unsupported() {
    let config = ExtractionConfig::default();

    let notebook = br#"{
        "cells": [
            {
                "cell_type": "markdown",
                "source": ["See [doc](attachment:report.pdf)"],
                "attachments": {
                    "report.pdf": {"application/pdf": "JVBERi0xLjQK"}
                },
                "metadata": {}
            }
        ],
        "metadata": {},
        "nbformat": 4,
        "nbformat_minor": 5
    }"#;

    let result = extract_bytes_document_blocking(notebook, "application/x-ipynb+json", &config)
        .expect("notebook extraction should succeed even with an unsupported attachment");

    assert!(
        result.images.as_ref().map(|images| images.is_empty()).unwrap_or(true),
        "an unsupported-MIME attachment must not produce an image"
    );
    assert!(
        result
            .processing_warnings
            .iter()
            .any(|w| w.message.contains("report.pdf")),
        "expected a processing warning naming the unsupported attachment, got: {:?}",
        result.processing_warnings
    );
}
