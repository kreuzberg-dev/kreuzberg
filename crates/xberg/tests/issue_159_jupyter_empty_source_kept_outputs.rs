//! Regression test for issue #159.
//!
//! `JupyterExtractor::build_internal_document` treated an empty (or
//! whitespace-only) `source` as "nothing in this cell" and `continue`d
//! before ever looking at `outputs`. A code cell that was cleared of its
//! source but still carries a saved `outputs` array (a common pattern for
//! notebooks that strip source for privacy/size while keeping results) was
//! therefore dropped from extraction entirely, including its outputs.

#![cfg(feature = "notebook")]

use xberg::core::config::ExtractionConfig;

mod helpers;
use helpers::extract_bytes_document_blocking;

/// A code cell with an empty `source` array but a non-empty `outputs` array
/// must still contribute its output text to the extracted content.
#[test]
fn should_keep_outputs_of_code_cell_with_empty_source() {
    let config = ExtractionConfig::default();

    let notebook = br#"{
        "cells": [
            {
                "cell_type": "code",
                "source": [],
                "execution_count": null,
                "outputs": [
                    {"output_type": "stream", "name": "stdout", "text": ["kept output line\n"]}
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

    assert_eq!(
        result.content.matches("kept output line").count(),
        1,
        "the output of a cell with empty source must appear exactly once in content, got: {:?}",
        result.content
    );
}

/// The same guarantee holds when the empty source is a single empty string
/// rather than an empty array (both are valid `source` shapes per nbformat).
#[test]
fn should_keep_outputs_of_code_cell_with_blank_string_source() {
    let config = ExtractionConfig::default();

    let notebook = br#"{
        "cells": [
            {
                "cell_type": "code",
                "source": "",
                "execution_count": null,
                "outputs": [
                    {"output_type": "stream", "name": "stdout", "text": ["another kept line\n"]}
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
        result.content.matches("another kept line").count(),
        1,
        "the output of a cell with a blank string source must appear exactly once in content, got: {:?}",
        result.content
    );
}

/// A markdown cell with an empty source and no outputs (outputs are not a
/// valid field on markdown cells) must still be dropped — this is not a
/// regression target, it pins the surrounding behavior so the fix above
/// doesn't accidentally start keeping genuinely empty cells.
#[test]
fn should_still_drop_genuinely_empty_markdown_cell() {
    let config = ExtractionConfig::default();

    let notebook = br#"{
        "cells": [
            {"cell_type": "markdown", "source": [""], "metadata": {}},
            {"cell_type": "markdown", "source": ["Kept markdown paragraph."], "metadata": {}}
        ],
        "metadata": {},
        "nbformat": 4,
        "nbformat_minor": 5
    }"#;

    let result = extract_bytes_document_blocking(notebook, "application/x-ipynb+json", &config)
        .expect("notebook extraction should succeed");

    assert_eq!(
        result.content.matches("Kept markdown paragraph.").count(),
        1,
        "the non-empty markdown cell must still be extracted, got: {:?}",
        result.content
    );
}
