//! Regression test for GitHub #1387.
//!
//! On 1.0.9-1.0.14, `crates/xberg-ffi/Cargo.toml` dropped the baked `"full"` feature from
//! the `xberg` dependency (commit cf7fa0533d, "stop forcing heic/candle into no-heic
//! consumers") and replaced it with an explicit, hand-maintained feature list for the
//! `not(android/ios/windows/macos-x86_64)` target — the block that builds the native
//! libraries shipped for linux-x64, linux-aarch64, and macos-arm64 (osx-arm64) across every
//! `xberg-ffi` consumer (C, C#, Go, Java). That list omitted `"excel"`, so the sole extractor
//! for xlsx/xlsm/xlsb/xltx/ods (`crates/xberg/src/extractors/excel.rs`, gated by
//! `#[cfg(feature = "excel")]`, see `crates/xberg/Cargo.toml:55`) was compiled out of those
//! native libraries while the static format catalogue in `crates/xberg/src/core/mime.rs`
//! kept advertising xlsx/xlsm/xlsb/xltx/ods unconditionally — producing
//! `UnsupportedFormatException` for a format `ListSupportedFormats()` still reports as
//! supported.
//!
//! `crates/xberg/Cargo.toml` already carries a near-identical historical fix for
//! `windows-target` (see the comment above its own `"excel"` entry, referencing "the 1.0.4
//! Windows XLSX bug") — this test locks in the analogous fix for the desktop/server target
//! and prevents a future edit of the hand-maintained feature list from silently dropping
//! `"excel"` again.
#![allow(clippy::print_stdout, clippy::print_stderr, clippy::dbg_macro)] // ~keep: test binaries print by design

use std::fs;
use std::path::Path;

/// Locates the `crates/xberg-ffi/Cargo.toml` manifest relative to this crate.
fn ffi_manifest_source() -> String {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let ffi_manifest = manifest_dir
        .parent()
        .expect("crates/xberg has a parent crates/ directory")
        .join("xberg-ffi")
        .join("Cargo.toml");
    fs::read_to_string(&ffi_manifest)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", ffi_manifest.display()))
}

/// Extracts the single-line `features = [...]` array that follows `needle` in `source`,
/// returning the comma-separated feature name list.
fn features_array_after(source: &str, needle: &str) -> Vec<String> {
    let needle_index = source
        .find(needle)
        .unwrap_or_else(|| panic!("expected to find target block `{needle}` in xberg-ffi/Cargo.toml"));
    let after_needle = &source[needle_index..];
    let features_start = after_needle
        .find("features = [")
        .expect("expected a `features = [...]` dependency declaration after the target block");
    let array_start = features_start + "features = [".len();
    let array_end = after_needle[array_start..]
        .find(']')
        .expect("expected a closing `]` for the features array");
    after_needle[array_start..array_start + array_end]
        .split(',')
        .map(|entry| entry.trim().trim_matches('"').to_owned())
        .filter(|entry| !entry.is_empty())
        .collect()
}

/// The default target — used for linux-x64/linux-arm64/macos-arm64 native builds (Go, C,
/// Java, and C# runtime packs) — must carry `"excel"`, or xlsx/xlsm/xlsb/xltx/ods extraction
/// silently regresses to `UnsupportedFormat` on those platforms while `list_supported_formats`
/// keeps advertising the formats (GH#1387).
#[test]
fn ffi_default_target_dependency_must_enable_excel_feature() {
    let source = ffi_manifest_source();
    let target_cfg = r#"[target.'cfg(not(any(target_os = "android", target_os = "ios", target_os = "windows", all(target_os = "macos", target_arch = "x86_64"))))'.dependencies]"#;
    let features = features_array_after(&source, target_cfg);

    assert!(
        features.iter().any(|feature| feature == "excel"),
        "the default (linux-x64/linux-arm64/macos-arm64) xberg-ffi target dependency must \
         request the \"excel\" feature so xlsx/xlsm/xlsb/xltx/ods extraction is compiled into \
         the shipped native library (GH#1387); got features = {features:?}"
    );
}

/// `xberg-ffi` must expose its own `excel` feature (mapping to `xberg/excel`) so binding
/// authors and CI can request it explicitly, and must enable it in its `default` feature set
/// so a plain `cargo build --release -p xberg-ffi` restores spreadsheet extraction.
#[test]
fn ffi_crate_declares_and_defaults_to_excel_feature() {
    let source = ffi_manifest_source();

    assert!(
        source.contains(r#"excel = ["xberg/excel"]"#),
        "xberg-ffi must declare `excel = [\"xberg/excel\"]` in its [features] table"
    );

    let default_features = features_array_multiline(&source, "default = [");
    assert!(
        default_features.iter().any(|feature| feature == "excel"),
        "xberg-ffi's default feature set must include \"excel\"; got {default_features:?}"
    );
}

/// Extracts a multi-line, one-feature-per-line `key = [...]` array (as used by the
/// `default` feature list, which is formatted one entry per line rather than inline).
fn features_array_multiline(source: &str, needle: &str) -> Vec<String> {
    let needle_index = source
        .find(needle)
        .unwrap_or_else(|| panic!("expected to find `{needle}` in xberg-ffi/Cargo.toml"));
    let array_start = needle_index + needle.len();
    let array_end = source[array_start..]
        .find(']')
        .expect("expected a closing `]` for the default features array");
    source[array_start..array_start + array_end]
        .lines()
        .map(|line| line.trim().trim_end_matches(',').trim_matches('"').to_owned())
        .filter(|entry| !entry.is_empty())
        .collect()
}
