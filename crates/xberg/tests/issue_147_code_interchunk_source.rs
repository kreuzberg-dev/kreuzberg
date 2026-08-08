#![cfg(feature = "tree-sitter")]
//! Investigation for #147: does the code extractor's default content mode drop
//! top-level source between tree-sitter chunks (module docstrings, imports)?
//!
//! `tree_sitter_language_pack::split_code` (vendored text-splitter, used by
//! `chunk_source` to build `ProcessResult::chunks`) produces contiguous,
//! non-overlapping byte ranges that tile the *entire* source — see its own
//! `chunks_cover_entire_source` test. Every chunk's `content` field is pushed
//! verbatim via `builder.push_code` in `CodeExtractor::extract_with_language`'s
//! default (non-Raw, non-Structure) content mode, so inter-chunk source such as
//! a leading import statement ends up inside whichever chunk covers that byte
//! range — it is not a separate, droppable segment.
//!
//! This test proves that end-to-end: an import statement preceding a function
//! definition must still appear in the extracted content.

mod helpers;
use helpers::extract_bytes_document;
use xberg::core::config::ExtractionConfig;

#[tokio::test]
async fn default_content_mode_preserves_import_between_chunks() {
    let config = ExtractionConfig::default();

    let source = "#!/usr/bin/env python3\nimport os\n\n\ndef foo():\n    return os.getcwd()\n";

    let extraction = extract_bytes_document(source.as_bytes(), "text/x-source-code", &config)
        .await
        .expect("code extraction should succeed");

    assert!(
        extraction.content.contains("import os"),
        "top-level import between chunks must survive extraction, got: {}",
        extraction.content
    );
    assert!(
        extraction.content.contains("def foo"),
        "function chunk must still be present, got: {}",
        extraction.content
    );
}
