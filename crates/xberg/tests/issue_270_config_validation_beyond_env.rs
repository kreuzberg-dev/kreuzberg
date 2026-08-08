//! Regression test for https://github.com/xberg-io/xberg/issues/270
//!
//! `ExtractionConfig`'s validators were only applied by the environment-variable
//! loader. A config arriving from a TOML/YAML/JSON file — or from a JSON override
//! merge — was deserialized and returned unvalidated, so an invalid OCR backend
//! name or a nonsensical DPI reached the pipeline and failed much later, far from
//! the setting that caused it (or not at all).
//!
//! `ExtractionConfig::validate()` is now called by every file loader and by the
//! JSON merge path. These tests exercise the loaders through their public API.

use std::io::Write;

use xberg::core::config::ExtractionConfig;

fn write_temp(name: &str, contents: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("xberg-issue-270-{}-{}", std::process::id(), name));
    std::fs::create_dir_all(&dir).expect("failed to create temp dir");
    let path = dir.join(name);
    let mut file = std::fs::File::create(&path).expect("failed to create temp config file");
    file.write_all(contents.as_bytes())
        .expect("failed to write temp config file");
    path
}

#[test]
fn should_reject_a_toml_config_with_an_unknown_ocr_backend() {
    let path = write_temp("bad_backend.toml", "[ocr]\nbackend = \"tesserct\"\n");

    let error = ExtractionConfig::from_toml_file(&path).expect_err("an unknown OCR backend must be rejected at load");

    let message = error.to_string();
    assert!(
        message.contains("tesserct"),
        "the error must name the offending value so the user can find it; got: {message}"
    );

    let _ = std::fs::remove_dir_all(path.parent().expect("temp file must have a parent"));
}

#[test]
fn should_reject_a_json_config_with_a_zero_target_dpi() {
    let path = write_temp("bad_dpi.json", r#"{"images": {"target_dpi": 0}}"#);

    let error = ExtractionConfig::from_json_file(&path).expect_err("a zero target DPI must be rejected at load");

    let message = error.to_string();
    assert!(
        message.contains("DPI"),
        "the error must identify DPI as the invalid setting; got: {message}"
    );

    let _ = std::fs::remove_dir_all(path.parent().expect("temp file must have a parent"));
}

#[test]
fn should_reject_chunking_overlap_that_is_not_smaller_than_the_chunk_size() {
    let path = write_temp("bad_chunking.toml", "[chunking]\nmax_chars = 500\nmax_overlap = 500\n");

    let error =
        ExtractionConfig::from_toml_file(&path).expect_err("an overlap equal to the chunk size must be rejected");

    let message = error.to_string();
    assert!(
        message.contains("max_overlap") && message.contains("max_chars"),
        "the error must name both offending fields; got: {message}"
    );

    let _ = std::fs::remove_dir_all(path.parent().expect("temp file must have a parent"));
}

/// A `preset` replaces `max_characters` and `overlap` before anything reads them,
/// so the raw pair must not be validated alongside one — otherwise a perfectly
/// usable preset-driven config is rejected on values that are never used.
#[test]
fn should_accept_a_preset_config_whose_raw_chunking_pair_would_otherwise_be_invalid() {
    let path = write_temp(
        "preset_chunking.toml",
        "[chunking]\npreset = \"balanced\"\nmax_chars = 500\nmax_overlap = 500\n",
    );

    let config = ExtractionConfig::from_toml_file(&path).expect("a preset config must load; the preset wins");
    let chunking = config.chunking.expect("the chunking section must survive loading");
    assert_eq!(chunking.preset.as_deref(), Some("balanced"));

    let _ = std::fs::remove_dir_all(path.parent().expect("temp file must have a parent"));
}

#[test]
fn should_still_accept_a_valid_config() {
    let path = write_temp(
        "good.toml",
        "[chunking]\nmax_chars = 1000\nmax_overlap = 100\n\n[images]\ntarget_dpi = 300\nmin_dpi = 72\nmax_dpi = 600\n",
    );

    let config = ExtractionConfig::from_toml_file(&path).expect("a valid config must still load");
    let chunking = config.chunking.expect("the chunking section must survive loading");
    assert_eq!(chunking.max_characters, 1000);
    assert_eq!(chunking.overlap, 100);

    let _ = std::fs::remove_dir_all(path.parent().expect("temp file must have a parent"));
}
