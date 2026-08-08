#![allow(clippy::print_stdout, clippy::print_stderr, clippy::dbg_macro)] // ~keep: test/bench binaries print by design; org logging policy exempts tests
//! Integration tests for the JSON timing envelope added to `xberg extract` and
//! `xberg batch`.
//!
//! Verifies:
//!  - `extract --format json` emits `{ result, extraction_time_ms }` shape
//!  - `batch --format json` emits `{ results, total_ms, per_file_ms }` shape
//!  - `result.metadata.ocr_used` exists as a bool field
//!  - `--pdf-backend xyz` exits non-zero and mentions "pdf-oxide"

use std::path::{Path, PathBuf};
use std::process::Command;

/// Returns path to the compiled `xberg` binary for this test target.
fn xberg_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_xberg"))
}

/// Returns path to the small reference PDF used in these tests.
fn pdf_fixture() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .parent()
        .expect("crates/xberg-cli parent")
        .parent()
        .expect("crates parent")
        .join("test_documents")
        .join("pdf")
        .join("pdfa_001.pdf")
}

/// Returns path to the small plain-text fixture used for batch tests.
fn txt_fixture() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .parent()
        .expect("crates/xberg-cli parent")
        .parent()
        .expect("crates parent")
        .join("test_documents")
        .join("text")
        .join("fake_text.txt")
}

/// Skip-guard: returns `true` when the fixture exists so the test can run.
fn fixture_exists(path: &Path) -> bool {
    path.exists() && path.is_file()
}

#[test]
fn test_extract_json_has_result_and_timing() {
    let pdf = pdf_fixture();
    if !fixture_exists(&pdf) {
        eprintln!("SKIP: PDF fixture not found at {}", pdf.display());
        return;
    }

    let output = Command::new(xberg_bin())
        .args(["extract", &pdf.to_string_lossy(), "--format", "json"])
        .output()
        .expect("failed to run xberg extract");

    assert!(
        output.status.success(),
        "extract exited non-zero: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let json: serde_json::Value = serde_json::from_slice(&output.stdout).expect("stdout is not valid JSON");

    assert!(json.get("result").is_some(), "missing 'result' key in envelope");
    let extraction_time_ms = json
        .get("extraction_time_ms")
        .and_then(|v| v.as_f64())
        .expect("'extraction_time_ms' must be a number");
    assert!(
        extraction_time_ms > 0.0,
        "extraction_time_ms must be positive, got {extraction_time_ms}"
    );

    let ocr_used = json["result"]["metadata"]
        .get("ocr_used")
        .expect("'result.metadata.ocr_used' must be present")
        .as_bool()
        .expect("'result.metadata.ocr_used' must be a boolean");
    assert!(!ocr_used, "expected ocr_used=false for native PDF extraction");
}

#[test]
fn test_extract_json_includes_plausible_peak_memory_bytes() {
    let pdf = pdf_fixture();
    if !fixture_exists(&pdf) {
        eprintln!("SKIP: PDF fixture not found at {}", pdf.display());
        return;
    }

    let output = Command::new(xberg_bin())
        .args(["extract", &pdf.to_string_lossy(), "--format", "json"])
        .output()
        .expect("failed to run xberg extract");

    assert!(
        output.status.success(),
        "extract exited non-zero: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let json: serde_json::Value = serde_json::from_slice(&output.stdout).expect("stdout is not valid JSON");

    let peak_memory_bytes = json
        .get("peak_memory_bytes")
        .and_then(|v| v.as_u64())
        .expect("'peak_memory_bytes' must be present and an unsigned integer");
    // A process that just loaded and ran the xberg extraction pipeline has allocated at least a
    // few MB of RSS (binary text/data pages alone exceed this); a plausible lower bound catches
    // a field that's present but stubbed to 0.
    const PLAUSIBLE_MIN_PEAK_MEMORY_BYTES: u64 = 1_000_000;
    assert!(
        peak_memory_bytes >= PLAUSIBLE_MIN_PEAK_MEMORY_BYTES,
        "peak_memory_bytes must be plausible (>= {PLAUSIBLE_MIN_PEAK_MEMORY_BYTES}), got {peak_memory_bytes}"
    );
}

#[test]
fn test_batch_json_has_results_and_timing() {
    let pdf = pdf_fixture();
    let txt = txt_fixture();
    if !fixture_exists(&pdf) || !fixture_exists(&txt) {
        eprintln!("SKIP: one or more batch fixtures not found");
        return;
    }

    let output = Command::new(xberg_bin())
        .args([
            "batch",
            &pdf.to_string_lossy(),
            &txt.to_string_lossy(),
            "--format",
            "json",
        ])
        .output()
        .expect("failed to run xberg batch");

    assert!(
        output.status.success(),
        "batch exited non-zero: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let json: serde_json::Value = serde_json::from_slice(&output.stdout).expect("stdout is not valid JSON");

    let results = json
        .get("results")
        .and_then(|v| v.as_array())
        .expect("'results' must be an array");
    assert_eq!(results.len(), 2, "expected 2 results for 2 input files");

    let total_ms = json
        .get("total_ms")
        .and_then(|v| v.as_f64())
        .expect("'total_ms' must be a number");
    assert!(total_ms > 0.0, "total_ms must be positive, got {total_ms}");

    let per_file_ms = json
        .get("per_file_ms")
        .and_then(|v| v.as_array())
        .expect("'per_file_ms' must be an array");
    assert_eq!(per_file_ms.len(), 2, "per_file_ms must have one entry per file");

    for (i, timing) in per_file_ms.iter().enumerate() {
        let ms = timing.as_f64().expect("per_file_ms entry must be a number");
        assert!(ms > 0.0, "per_file_ms[{i}] must be positive, got {ms}");
    }

    for (i, result) in results.iter().enumerate() {
        assert!(
            result["metadata"].get("ocr_used").and_then(|v| v.as_bool()).is_some(),
            "results[{i}].metadata.ocr_used must be a bool"
        );
    }
}

#[test]
fn test_pdf_backend_invalid_value_exits_nonzero() {
    let pdf = pdf_fixture();
    if !fixture_exists(&pdf) {
        eprintln!("SKIP: PDF fixture not found at {}", pdf.display());
        return;
    }

    let output = Command::new(xberg_bin())
        .args(["extract", &pdf.to_string_lossy(), "--pdf-backend", "xyz"])
        .output()
        .expect("failed to run xberg extract");

    assert!(
        !output.status.success(),
        "expected non-zero exit for unknown --pdf-backend"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("pdf-oxide"),
        "error message should mention 'pdf-oxide', got: {stderr}"
    );
}

#[test]
fn test_extract_json_omits_stage_timings_by_default() {
    let pdf = pdf_fixture();
    if !fixture_exists(&pdf) {
        eprintln!("SKIP: PDF fixture not found at {}", pdf.display());
        return;
    }

    let output = Command::new(xberg_bin())
        .args(["extract", &pdf.to_string_lossy(), "--format", "json"])
        .env_remove("XBERG_EMIT_STAGE_TIMING")
        .output()
        .expect("failed to run xberg extract");

    assert!(
        output.status.success(),
        "extract exited non-zero: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let json: serde_json::Value = serde_json::from_slice(&output.stdout).expect("stdout is not valid JSON");
    assert!(
        json.get("stage_timings").is_none(),
        "stage_timings must be absent when XBERG_EMIT_STAGE_TIMING is unset"
    );
}

#[test]
fn test_extract_json_includes_stage_timings_when_requested() {
    let pdf = pdf_fixture();
    if !fixture_exists(&pdf) {
        eprintln!("SKIP: PDF fixture not found at {}", pdf.display());
        return;
    }

    let output = Command::new(xberg_bin())
        .args(["extract", &pdf.to_string_lossy(), "--format", "json"])
        .env("XBERG_EMIT_STAGE_TIMING", "1")
        .output()
        .expect("failed to run xberg extract");

    assert!(
        output.status.success(),
        "extract exited non-zero: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let json: serde_json::Value = serde_json::from_slice(&output.stdout).expect("stdout is not valid JSON");
    let stage_timings = json
        .get("stage_timings")
        .expect("stage_timings must be present when XBERG_EMIT_STAGE_TIMING=1");

    let process_init_ms = stage_timings
        .get("process_init_ms")
        .and_then(|v| v.as_f64())
        .expect("'stage_timings.process_init_ms' must be a number");
    assert!(
        process_init_ms >= 0.0,
        "process_init_ms must be non-negative, got {process_init_ms}"
    );

    let first_parse_ms = stage_timings
        .get("first_parse_ms")
        .and_then(|v| v.as_f64())
        .expect("'stage_timings.first_parse_ms' must be a number");
    assert!(
        first_parse_ms > 0.0,
        "first_parse_ms must be positive, got {first_parse_ms}"
    );

    assert!(
        stage_timings.get("ort_session_and_inference_ms").is_none(),
        "ort_session_and_inference_ms must be absent when layout/OCR are not active"
    );
}

#[test]
fn test_pdf_backend_valid_value_succeeds() {
    let pdf = pdf_fixture();
    if !fixture_exists(&pdf) {
        eprintln!("SKIP: PDF fixture not found at {}", pdf.display());
        return;
    }

    let output = Command::new(xberg_bin())
        .args([
            "extract",
            &pdf.to_string_lossy(),
            "--pdf-backend",
            "pdf-oxide",
            "--format",
            "json",
        ])
        .output()
        .expect("failed to run xberg extract");

    assert!(
        output.status.success(),
        "--pdf-backend pdf-oxide should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let json: serde_json::Value = serde_json::from_slice(&output.stdout).expect("stdout is not valid JSON");
    assert!(json.get("result").is_some(), "missing 'result' key");
    assert!(json.get("extraction_time_ms").is_some(), "missing 'extraction_time_ms'");
}
