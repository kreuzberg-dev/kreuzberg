#![allow(clippy::print_stdout, clippy::print_stderr)] // ~keep: test binaries print by design; org logging policy exempts tests
//! End-to-end tests for the data the CLI surfaces from an extraction.
//!
//! These cover the defects where the CLI discarded most of the extraction envelope:
//!
//! - text mode printed only `result.content`, so the rest of the envelope (and every
//!   processing warning) was invisible;
//! - `--format toon` serialized the bare `ExtractedDocument`, so TOON consumers lost the
//!   timing/peak-memory fields that JSON consumers already received;
//! - `xberg formats` reported the core's static format catalogue, which is not feature-gated,
//!   so it advertised formats the binary cannot extract.
//!
//! The binary under test is located via `CARGO_BIN_EXE_xberg`, which Cargo builds with the same
//! feature set as this test target. Nothing here shells out to `cargo`.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Path to the `xberg` binary built for this test target.
fn xberg_bin() -> &'static str {
    env!("CARGO_BIN_EXE_xberg")
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates/xberg-cli parent")
        .parent()
        .expect("crates parent")
        .to_path_buf()
}

fn text_fixture() -> PathBuf {
    repo_root().join("test_documents").join("text").join("fake_text.txt")
}

fn docx_fixture() -> PathBuf {
    repo_root().join("test_documents").join("docx").join("word_tables.docx")
}

fn require(path: &Path) {
    assert!(
        path.is_file(),
        "fixture missing at {} — this test must not silently pass",
        path.display()
    );
}

struct Output {
    stdout: String,
    stderr: String,
    success: bool,
}

fn run(args: &[&str]) -> Output {
    let output = Command::new(xberg_bin())
        .args(args)
        .output()
        .unwrap_or_else(|error| panic!("failed to run `xberg {}`: {error}", args.join(" ")));
    Output {
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        success: output.status.success(),
    }
}

/// Text mode must keep `stdout` as the extracted content only, so redirecting it still yields
/// the document.
#[test]
fn extract_text_keeps_stdout_free_of_envelope_output() {
    let fixture = text_fixture();
    require(&fixture);

    let output = run(&["extract", &fixture.to_string_lossy()]);

    assert!(output.success, "extraction failed: {}", output.stderr);
    assert!(
        output.stdout.contains("This is a test document to use for unit tests."),
        "stdout must carry the extracted content; got {:?}",
        output.stdout
    );
    assert!(
        !output.stdout.contains("--- extraction envelope ---"),
        "the envelope summary must not pollute the content stream; got {:?}",
        output.stdout
    );
}

/// The envelope fields that text mode used to discard must be reported on `stderr`.
#[test]
fn extract_text_reports_the_extraction_envelope_on_stderr() {
    let fixture = text_fixture();
    require(&fixture);

    let output = run(&["extract", &fixture.to_string_lossy()]);

    assert!(output.success, "extraction failed: {}", output.stderr);
    assert!(
        output.stderr.contains("--- extraction envelope ---"),
        "text mode must emit the envelope summary; got {:?}",
        output.stderr
    );
    assert!(
        output.stderr.contains("mime type: text/plain"),
        "the envelope summary must report the resolved MIME type; got {:?}",
        output.stderr
    );
    assert!(
        output.stderr.contains("extraction time: "),
        "the envelope summary must report the extraction time; got {:?}",
        output.stderr
    );
}

/// TOON output must carry the same timing envelope the JSON path emits.
#[test]
fn extract_toon_includes_the_timing_envelope() {
    let fixture = text_fixture();
    require(&fixture);

    let output = run(&["extract", &fixture.to_string_lossy(), "--format", "toon"]);

    assert!(output.success, "extraction failed: {}", output.stderr);
    for field in ["extraction_time_ms", "peak_memory_bytes"] {
        assert!(
            output.stdout.contains(field),
            "TOON output must include the `{field}` envelope field; got {:?}",
            output.stdout
        );
    }
    assert!(
        output.stdout.contains("This is a test document to use for unit tests."),
        "TOON output must still carry the extracted document; got {:?}",
        output.stdout
    );
}

/// The JSON envelope already carried timing; this pins it so the TOON fix cannot regress it.
#[test]
fn extract_json_includes_the_timing_envelope() {
    let fixture = text_fixture();
    require(&fixture);

    let output = run(&["extract", &fixture.to_string_lossy(), "--format", "json"]);

    assert!(output.success, "extraction failed: {}", output.stderr);
    let parsed: serde_json::Value =
        serde_json::from_str(&output.stdout).unwrap_or_else(|error| panic!("JSON output was not valid: {error}"));
    assert!(
        parsed.get("extraction_time_ms").is_some(),
        "JSON envelope must include extraction_time_ms; got {parsed}"
    );
    assert!(
        parsed.get("peak_memory_bytes").is_some(),
        "JSON envelope must include peak_memory_bytes; got {parsed}"
    );
}

/// `xberg formats` must not advertise a format the binary cannot extract.
///
/// This is the invariant behind the .NET report in GH#1387: the catalogue said `xlsx` was
/// supported while extraction raised `UnsupportedFormat`. The two sides are checked against each
/// other here rather than against a hardcoded expectation, so the test is meaningful under every
/// feature combination.
#[test]
fn formats_command_agrees_with_actual_extraction_capability() {
    let fixture = docx_fixture();
    require(&fixture);

    let listed = run(&["formats", "--format", "json"]);
    assert!(listed.success, "`xberg formats` failed: {}", listed.stderr);
    let formats: serde_json::Value =
        serde_json::from_str(&listed.stdout).unwrap_or_else(|error| panic!("formats JSON was not valid: {error}"));
    let advertises_docx = formats
        .as_array()
        .expect("formats output must be a JSON array")
        .iter()
        .any(|entry| entry.get("extension").and_then(serde_json::Value::as_str) == Some("docx"));

    let extracted = run(&["extract", &fixture.to_string_lossy()]);

    assert_eq!(
        advertises_docx, extracted.success,
        "`xberg formats` advertises docx = {advertises_docx}, but extracting a docx succeeded = {}. \
         The catalogue must describe the built binary. stderr: {}",
        extracted.success, extracted.stderr
    );
}

/// The advertised list must track the compiled feature set rather than the static catalogue.
#[test]
fn formats_command_reflects_the_compiled_feature_set() {
    let listed = run(&["formats", "--format", "json"]);
    assert!(listed.success, "`xberg formats` failed: {}", listed.stderr);
    let formats: serde_json::Value = serde_json::from_str(&listed.stdout).expect("formats JSON must parse");
    let extensions: Vec<&str> = formats
        .as_array()
        .expect("formats output must be a JSON array")
        .iter()
        .filter_map(|entry| entry.get("extension").and_then(serde_json::Value::as_str))
        .collect();

    for always_present in ["txt", "md", "csv", "json"] {
        assert!(
            extensions.contains(&always_present),
            "'{always_present}' has an ungated extractor and must always be listed; got {extensions:?}"
        );
    }

    if cfg!(feature = "formats-no-heic") {
        assert!(
            extensions.contains(&"docx"),
            "`formats-no-heic` is enabled, so docx must be listed; got {extensions:?}"
        );
    } else {
        assert!(
            !extensions.contains(&"docx"),
            "`formats-no-heic` is disabled, so docx must not be advertised; got {extensions:?}"
        );
    }
}
