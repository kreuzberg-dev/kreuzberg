#![cfg(feature = "tree-sitter")]
//! Regression test for #259.
//!
//! `ExtractedDocument.code_intelligence` is documented (`types/extraction.rs`) as
//! carrying the full `tree_sitter_language_pack::ProcessResult` — metrics, structural
//! analysis, imports/exports, comments, docstrings, symbols and diagnostics — for
//! source-code extractions. `extraction::derive::derive_extraction_result` used to
//! hardcode it to `None` under `cfg(feature = "tree-sitter")`, so none of that ever
//! reached callers regardless of what tree-sitter actually produced.
//!
//! This extracts a small Python fixture end-to-end through the public API and pins the
//! exact populated shape: `language`, `metrics` (hand-computed from the fixture source
//! below), `structure` (the one top-level function), and `imports` (the one top-level
//! import). `exports`/`comments`/`docstrings`/`symbols`/`diagnostics`/`chunks`/`data` are
//! all left at their tree-sitter-language-pack defaults (disabled, or empty for this
//! fixture) and must therefore be entirely absent from the JSON rather than present as
//! empty placeholders — the upstream `ProcessResult` type skips empty/`None` fields when
//! serializing.

mod helpers;
use helpers::extract_bytes_document;
use xberg::core::config::ExtractionConfig;

/// Fixture source. The byte/line offsets asserted below were computed by hand against
/// this exact literal — keep it in sync with any change to the numbers in the test.
///
/// Line-by-line (0-indexed, matching `Span::start_line`):
/// 0. `#!/usr/bin/env python3`  (shebang; counts as a comment line, `#`-prefixed)
/// 1. `import os`               (code line)
/// 2. `` (blank)
/// 3. `` (blank)
/// 4. `def greet(name):`        (code line; byte offset 35)
/// 5. `    return f"hello {name}"` (code line)
const SOURCE: &str = "#!/usr/bin/env python3\nimport os\n\n\ndef greet(name):\n    return f\"hello {name}\"\n";

#[tokio::test]
async fn code_intelligence_surfaces_the_full_tree_sitter_process_result() {
    let config = ExtractionConfig::default();

    let extraction = extract_bytes_document(SOURCE.as_bytes(), "text/x-source-code", &config)
        .await
        .expect("python source extraction should succeed");

    let code_intelligence = extraction
        .code_intelligence
        .expect("code_intelligence must be populated for a source-code extraction");

    assert_eq!(code_intelligence["language"], serde_json::json!("python"));

    // `metrics` are always computed, and hand-verified against `SOURCE` above: 6 lines
    // total, only the shebang is a comment line, the two separator lines are blank, and
    // the remaining 3 lines (import + def + return) are code.
    let metrics = &code_intelligence["metrics"];
    assert_eq!(metrics["total_lines"], serde_json::json!(6));
    assert_eq!(metrics["code_lines"], serde_json::json!(3));
    assert_eq!(metrics["comment_lines"], serde_json::json!(1));
    assert_eq!(metrics["blank_lines"], serde_json::json!(2));
    assert_eq!(metrics["total_bytes"], serde_json::json!(SOURCE.len()));
    assert_eq!(
        metrics["error_count"],
        serde_json::json!(0),
        "valid python source must not report parse errors"
    );
    // `node_count`/`max_depth` reflect the tree-sitter-python grammar's internal parse
    // tree shape, not something mechanically derivable from the source text by hand;
    // only assert they were actually computed (never zero for a non-empty parse).
    assert!(metrics["node_count"].as_u64().expect("node_count must be a number") > 0);
    assert!(metrics["max_depth"].as_u64().expect("max_depth must be a number") > 0);

    // `structure` (enabled by default): exactly the one top-level `def greet` function.
    let structure = code_intelligence["structure"]
        .as_array()
        .expect("structure must be an array");
    assert_eq!(
        structure.len(),
        1,
        "expected exactly one top-level structural item: {structure:?}"
    );
    assert_eq!(structure[0]["kind"], serde_json::json!("Function"));
    assert_eq!(structure[0]["name"], serde_json::json!("greet"));
    // The function definition starts at the literal `def` keyword: line 4 (0-indexed),
    // column 0, byte 35 — computed by hand against `SOURCE`.
    assert_eq!(structure[0]["span"]["start_line"], serde_json::json!(4));
    assert_eq!(structure[0]["span"]["start_column"], serde_json::json!(0));
    assert_eq!(structure[0]["span"]["start_byte"], serde_json::json!(35));

    // `imports` (enabled by default): exactly the one top-level `import os`.
    let imports = code_intelligence["imports"]
        .as_array()
        .expect("imports must be an array");
    assert_eq!(imports.len(), 1, "expected exactly one import: {imports:?}");
    assert_eq!(imports[0]["source"], serde_json::json!("import os"));
    assert_eq!(imports[0]["is_wildcard"], serde_json::json!(false));
    assert_eq!(imports[0]["span"]["start_byte"], serde_json::json!(23));
    assert_eq!(imports[0]["span"]["end_byte"], serde_json::json!(32));

    // Python has no `export` construct, and this fixture/config leaves comments,
    // docstrings, symbols, diagnostics and chunking all at their default-disabled
    // state, and does not enable data-format extraction. All of these must therefore
    // be entirely absent from the JSON rather than present as empty arrays/objects.
    for absent_field in [
        "exports",
        "comments",
        "docstrings",
        "symbols",
        "diagnostics",
        "chunks",
        "data",
    ] {
        assert!(
            code_intelligence.get(absent_field).is_none(),
            "expected `{absent_field}` to be entirely absent from code_intelligence, got {:?}",
            code_intelligence.get(absent_field)
        );
    }
}

/// Companion to the existing derive-layer unit tests in
/// `crates/xberg/src/extraction/derive.rs` (`test_derive_extraction_result_*`): confirms
/// the fix end-to-end through the public extraction API, and that a document with no
/// source-code format metadata still reports `code_intelligence: None` — no accidental
/// leak of the internal `metadata.additional` scratch key or a stray empty object.
#[tokio::test]
async fn code_intelligence_is_none_for_non_code_documents() {
    let config = ExtractionConfig::default();

    let extraction = extract_bytes_document(b"Hello, this is plain text.", "text/plain", &config)
        .await
        .expect("plain text extraction should succeed");

    assert!(extraction.code_intelligence.is_none());
}
