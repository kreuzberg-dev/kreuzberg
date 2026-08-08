//! Regression tests for issue #148.
//!
//! `JupyterExtractor` had two independent code paths for rendering a cell
//! output's text: the rich `extract_output`/`extract_data_output`/
//! `extract_error_output` family (handling `text/markdown`, `text/html`, and
//! full error tracebacks), and a second, much narrower `collect_output_text`
//! that only ever looked at `text/plain` and dropped the traceback on
//! errors. Only the narrow path fed the `InternalDocument` actually returned
//! to callers — the richer path's result was discarded — so any output whose
//! only representation was `text/html`/`text/markdown`, or any error's
//! traceback, silently vanished from extracted content.

#![cfg(feature = "notebook")]

use xberg::core::config::{ExtractionConfig, OutputFormat};

mod helpers;
use helpers::extract_bytes_document_blocking;

/// A code cell's `execute_result` whose only representation is `text/html`
/// (no `text/plain`) must still surface in the extracted content when the
/// output format is not plain-text-only.
#[test]
fn should_extract_html_only_execute_result_output() {
    let config = ExtractionConfig {
        output_format: OutputFormat::Markdown,
        ..Default::default()
    };

    let notebook = br#"{
        "cells": [
            {
                "cell_type": "code",
                "source": ["render_html()"],
                "execution_count": 1,
                "outputs": [
                    {
                        "output_type": "execute_result",
                        "execution_count": 1,
                        "data": {"text/html": ["<b>Bold HTML</b>"]},
                        "metadata": {}
                    }
                ],
                "metadata": {}
            }
        ],
        "metadata": {"kernelspec": {"name": "python3", "language": "python"}},
        "nbformat": 4,
        "nbformat_minor": 5
    }"#;

    let result = extract_bytes_document_blocking(notebook, "application/x-ipynb+json", &config)
        .expect("notebook extraction should succeed");

    assert!(
        result.content.contains("<b>Bold HTML</b>"),
        "text/html-only output must be extracted through the richer output path; got content: {:?}",
        result.content
    );
}

/// An `error` output's traceback must be preserved in extracted content, not
/// just the exception name and value.
#[test]
fn should_extract_full_traceback_for_error_output() {
    let config = ExtractionConfig::default();

    let notebook = br#"{
        "cells": [
            {
                "cell_type": "code",
                "source": ["1/0"],
                "execution_count": 1,
                "outputs": [
                    {
                        "output_type": "error",
                        "ename": "ZeroDivisionError",
                        "evalue": "division by zero",
                        "traceback": ["Traceback (most recent call last)", "line 1, in <module>"]
                    }
                ],
                "metadata": {}
            }
        ],
        "metadata": {},
        "nbformat": 4,
        "nbformat_minor": 0
    }"#;

    let result = extract_bytes_document_blocking(notebook, "application/x-ipynb+json", &config)
        .expect("notebook extraction should succeed");

    assert!(
        result.content.contains("Error (ZeroDivisionError): division by zero"),
        "expected the error name and value in content, got: {:?}",
        result.content
    );
    assert!(
        result.content.contains("Traceback (most recent call last)") && result.content.contains("line 1, in <module>"),
        "expected the full traceback (both lines) in content through the richer output path, got: {:?}",
        result.content
    );
}
