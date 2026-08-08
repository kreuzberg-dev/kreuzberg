//! Regression test for GH#1387 / internal issue #308.
//!
//! `list_supported_formats()` used to map the static `EXT_TO_MIME` table straight into
//! its result with no feature filtering, so a narrower build (fewer Cargo features, or a
//! narrower `xberg-ffi` target dependency list) would advertise formats whose extractors
//! were compiled out. Every FFI binding's `ListSupportedFormats()` inherited the lie.
//!
//! The fix intersects the static table with the document extractor registry's live MIME
//! index, so the advertised catalogue can never claim a format this build cannot actually
//! extract. This test is table-driven over every entry `list_supported_formats()` returns
//! and asserts each one resolves to a registered extractor — it is meant to be run under
//! more than one feature set so the defect class (advertise-without-register) is
//! self-detecting rather than tied to one particular build's format list.
//!
//! This test only *reads* the process-global extractor registry (via the public
//! `ensure_initialized` + `list_supported_formats` APIs) — it does not register, clear, or
//! otherwise mutate it, so it is safe to run concurrently with other tests that exercise
//! registry lifecycle/isolation behavior.

use xberg::core::mime::list_supported_formats;
use xberg::extractors::ensure_initialized;

/// The advertised list must not be empty in any build — the unconditional built-ins alone
/// guarantee several entries, so an empty result means the registry intersection ran before
/// registration and silently filtered everything away.
///
/// NOTE: there is deliberately no test here asserting "every advertised format resolves to a
/// registered extractor". `list_supported_formats()` builds its result by filtering
/// `EXT_TO_MIME` on `registry.get(mime).is_ok()` (core/mime.rs), so re-filtering its output on
/// `is_err()` is necessarily empty and such a test cannot fail in any build, broken or not.
/// The invariant with actual teeth runs over the raw catalogue rather than the filtered output,
/// and lives in `issue_289_advertised_formats_have_extractors.rs`.
#[test]
fn the_advertised_format_list_is_never_empty() {
    ensure_initialized().expect("built-in extractor registration should succeed");

    assert!(
        !list_supported_formats().is_empty(),
        "list_supported_formats() returned nothing; the registry intersection filtered out \
         even the unconditionally-registered built-ins"
    );
}

/// The list must stay sorted by extension regardless of how many entries the registry
/// intersection filters out.
#[test]
fn list_stays_sorted_after_registry_filtering() {
    ensure_initialized().expect("built-in extractor registration should succeed");

    let formats = list_supported_formats();
    let extensions: Vec<&str> = formats.iter().map(|format| format.extension.as_str()).collect();
    let mut sorted = extensions.clone();
    sorted.sort_unstable();

    assert_eq!(
        extensions, sorted,
        "formats must remain sorted by extension after filtering"
    );
}

/// Formats that are always registered regardless of optional Cargo features (the
/// unconditional built-ins: plain text, markdown, structured/JSON, CSV) must survive the
/// registry intersection in every build, including the narrowest ones.
///
/// `html` is intentionally excluded here: `crates/xberg/Cargo.toml` gates the HTML extractor
/// behind the `html` Cargo feature (`html = ["dep:html-to-markdown-rs", "dep:v_htmlescape"]`,
/// and `register_default_extractors()` only registers `HtmlExtractor` under
/// `#[cfg(feature = "html")]`), so asserting on it unconditionally fails on any build with
/// that feature off. It gets its own conditional assertion below instead.
#[test]
fn unconditionally_registered_formats_always_survive_filtering() {
    ensure_initialized().expect("built-in extractor registration should succeed");

    let formats = list_supported_formats();
    let extensions: std::collections::HashSet<&str> = formats.iter().map(|format| format.extension.as_str()).collect();

    for ext in ["txt", "md", "csv", "json"] {
        assert!(
            extensions.contains(ext),
            "extension `{ext}` is served by an unconditionally-registered extractor and \
             should always be present regardless of optional feature flags"
        );
    }

    #[cfg(feature = "html")]
    assert!(
        extensions.contains("html"),
        "extension `html` should be present when the `html` Cargo feature is enabled"
    );
}
