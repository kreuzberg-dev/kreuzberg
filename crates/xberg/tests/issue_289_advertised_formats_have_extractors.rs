//! Regression test for internal issue #289 (GitHub xberg-io/xberg#1387).
//!
//! # The defect class
//!
//! `crates/xberg/src/core/mime.rs` holds `static FORMATS` — the ungated catalogue of every
//! document format the codebase knows how to *describe*. Every extractor that turns one of
//! those formats into text, by contrast, is `#[cfg(feature = "…")]`-gated in
//! `crates/xberg/src/extractors/mod.rs::register_default_extractors`. Nothing structurally ties
//! the two together, so a build whose Cargo feature set omits an extractor still advertises the
//! format and then rejects it with `XbergError::UnsupportedFormat`.
//!
//! That is exactly what shipped in 1.0.13/1.0.14: `crates/xberg-ffi/Cargo.toml`'s
//! `cfg(not(any(android, ios, windows, macos-x86_64)))` dependency — the block that builds the
//! native libraries for linux-x64, linux-aarch64 and macos-arm64 — lost its baked `"full"`
//! feature and was left with a hand-enumerated list that omitted `excel`, `hwp`, `hwpx`,
//! `iwork`, `wordperfect`, `mdx`, `xml` and `qr-codes`. Reporters saw `.xlsx` listed by
//! `list_supported_formats()` and rejected by `extract()` in the same process.
//!
//! # Why this test is gated on `full` rather than made feature-aware
//!
//! The constraint is that the catalogue and the registry must agree *in whatever build is
//! running*. Two shapes satisfy that:
//!
//! 1. Gate the whole test on `feature = "full"` and assert the complete catalogue resolves.
//! 2. Run in every build and skip a format when its gating Cargo feature is genuinely off.
//!
//! Shape 2 is rejected deliberately. It needs a hand-maintained map from each of the ~90
//! catalogue entries to the `cfg!(feature = "…")` that gates its extractor — a second,
//! independent copy of the same truth, kept in sync by hand. That copy can drift out of step
//! with `register_default_extractors`, and when it does the test would *skip* the format it was
//! supposed to guard, passing while the defect is live. Encoding the bug's own failure mode
//! (two hand-maintained lists silently diverging) into its regression test is not acceptable.
//!
//! Shape 1 has no such copy: under `full` every format extractor is compiled in by definition,
//! so any advertised format that does not resolve to a registered extractor is unambiguously a
//! catalogue lie — either a `FORMATS` entry added without an extractor, or an aggregate feature
//! that stopped implying one. The cost is that narrow builds are not covered here; those are
//! covered from the other side by `list_supported_formats()`'s registry intersection (issue
//! #308) and, for the FFI manifest specifically, by
//! `crates/xberg/tests/issue_1387_xlsx_ffi_feature_regression.rs`.
//!
//! # Relationship to the in-crate alias guard
//!
//! `core::mime`'s test module has `every_declared_alias_resolves_to_the_same_extractor_as_its_
//! canonical_mime`, which checks alias → canonical routing and deliberately *skips* any format
//! whose canonical MIME has no extractor. This test is the complementary half: canonical →
//! registered. Neither subsumes the other.
//!
//! The catalogue is read out of the `mime.rs` source because `FORMATS` is a private `static`
//! with no public enumeration API, and `list_supported_formats()` cannot stand in for it — that
//! function already filters itself by the very registry lookup under test, so asserting against
//! its output would be tautological.
//!
//! This test only reads the process-global extractor registry; it never registers, clears or
//! otherwise mutates it, so it is safe alongside tests that exercise registry lifecycle.
#![cfg(feature = "full")]

use std::collections::HashSet;
use std::fs;
use std::path::Path;

use xberg::extractors::ensure_initialized;
use xberg::plugins::registry::get_document_extractor_registry;

/// Opening line of the catalogue table in `core/mime.rs`.
const FORMATS_TABLE_HEADER: &str = "static FORMATS: &[FormatEntry] = &[";

/// Closing line of the catalogue table in `core/mime.rs`.
const FORMATS_TABLE_FOOTER: &str = "\n];";

/// Lower bound on how many catalogue entries carry at least one file extension. Guards against a
/// parser regression silently reducing the table to a handful of entries (or none), which would
/// let `every_catalogued_format_resolves_to_a_registered_extractor` pass vacuously. Chosen well
/// below the current count (93 at the time of writing) so adding or removing a format never
/// trips it.
const MIN_ENTRIES_WITH_EXTENSIONS: usize = 80;

/// The formats whose extractors were compiled out of the shipped 1.0.13/1.0.14 native libraries.
/// Pinned by extension so the parser is proven to reach the specific rows this defect was about,
/// spread across the whole table rather than clustered at its start.
const REGRESSION_EXTENSIONS: &[&str] = &[
    "xlsx", "xls", "ods", "hwp", "hwpx", "pages", "numbers", "key", "mdx", "xml", "wpd",
];

/// One row of `core/mime.rs`'s `FORMATS` table.
struct CatalogueEntry {
    extensions: Vec<String>,
    mime_type: String,
}

/// Reads the `core/mime.rs` source that owns the format catalogue.
fn mime_source() -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/core/mime.rs");
    fs::read_to_string(&path).unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()))
}

/// Collects every double-quoted string literal in `line`, in order.
fn string_literals(line: &str) -> Vec<String> {
    let mut literals = Vec::new();
    let mut rest = line;
    while let Some(open) = rest.find('"') {
        let after_open = &rest[open + 1..];
        let Some(close) = after_open.find('"') else {
            break;
        };
        literals.push(after_open[..close].to_owned());
        rest = &after_open[close + 1..];
    }
    literals
}

/// Maps `const NAME: &str = "value";` declarations to their values.
///
/// Four `FORMATS` rows (`odt`, `odp`, `ods`, `wpd`) name a constant instead of inlining the MIME
/// string, so the parser has to resolve them or it would drop those rows.
fn string_constants(source: &str) -> Vec<(String, String)> {
    source
        .lines()
        .filter_map(|line| {
            let declaration = line.trim();
            let after_const = declaration
                .strip_prefix("pub(crate) const ")
                .or_else(|| declaration.strip_prefix("pub const "))
                .or_else(|| declaration.strip_prefix("const "))?;
            let (name, tail) = after_const.split_once(": &str = ")?;
            let value = string_literals(tail).into_iter().next()?;
            Some((name.to_owned(), value))
        })
        .collect()
}

/// Isolates the body of the `FORMATS` table from the surrounding source.
fn formats_table(source: &str) -> &str {
    let after_header = source
        .split_once(FORMATS_TABLE_HEADER)
        .unwrap_or_else(|| panic!("`{FORMATS_TABLE_HEADER}` not found in core/mime.rs"))
        .1;
    after_header
        .split_once(FORMATS_TABLE_FOOTER)
        .unwrap_or_else(|| panic!("unterminated `FORMATS` table in core/mime.rs"))
        .0
}

/// Parses every `FormatEntry` row of the catalogue.
///
/// Line-oriented rather than regex-based, and cross-checked against the raw `FormatEntry {`
/// count by [`the_parser_reads_every_row_of_the_format_table`], so a formatting change that this
/// parser cannot handle fails loudly instead of quietly shrinking the table under test.
fn parse_catalogue(source: &str) -> Vec<CatalogueEntry> {
    let constants = string_constants(source);
    let mut entries = Vec::new();
    let mut extensions: Vec<String> = Vec::new();
    let mut mime_type: Option<String> = None;
    let mut inside_entry = false;

    for line in formats_table(source).lines() {
        let line = line.trim();
        if line.starts_with("FormatEntry {") {
            inside_entry = true;
            extensions.clear();
            mime_type = None;
        } else if !inside_entry {
            continue;
        } else if let Some(tail) = line.strip_prefix("extensions:") {
            extensions = string_literals(tail);
        } else if let Some(tail) = line.strip_prefix("mime_type:") {
            let value = tail.trim().trim_end_matches(',');
            mime_type = Some(if value.starts_with('"') {
                string_literals(value)
                    .into_iter()
                    .next()
                    .unwrap_or_else(|| panic!("malformed mime_type literal: {line}"))
            } else {
                constants
                    .iter()
                    .find(|(name, _)| name.as_str() == value)
                    .map(|(_, resolved)| resolved.clone())
                    .unwrap_or_else(|| panic!("mime_type constant `{value}` is not declared in core/mime.rs"))
            });
        } else if line == "}," {
            let resolved = mime_type
                .take()
                .unwrap_or_else(|| panic!("FormatEntry row without a mime_type in core/mime.rs"));
            entries.push(CatalogueEntry {
                extensions: std::mem::take(&mut extensions),
                mime_type: resolved,
            });
            inside_entry = false;
        }
    }

    entries
}

/// Every format the catalogue advertises by file extension must resolve to an extractor that is
/// actually registered in this build.
///
/// A failure names every offending format, not just the first, so a feature-list regression that
/// drops several extractors at once is diagnosable in one run.
#[test]
fn every_catalogued_format_resolves_to_a_registered_extractor() {
    ensure_initialized().expect("built-in extractor registration should succeed");

    let source = mime_source();
    let catalogue = parse_catalogue(&source);
    let registry = get_document_extractor_registry();
    let registry_guard = registry.read();

    let mut unextractable: Vec<String> = catalogue
        .iter()
        .filter(|entry| !entry.extensions.is_empty())
        .filter(|entry| registry_guard.get(&entry.mime_type).is_err())
        .map(|entry| format!("{} (.{})", entry.mime_type, entry.extensions.join(", .")))
        .collect();
    unextractable.sort();

    assert!(
        unextractable.is_empty(),
        "core/mime.rs advertises {} format(s) with no registered extractor in this build, so \
         extract() rejects them as UnsupportedFormat while the catalogue claims support \
         (issue #289 / GH#1387):\n  {}",
        unextractable.len(),
        unextractable.join("\n  ")
    );
}

/// The parser must see every row of the table.
///
/// Without this, a `FORMATS` formatting change the parser cannot follow would shrink the table
/// under test and let the guard above pass while advertising unextractable formats — a test that
/// passes when the code is broken.
#[test]
fn the_parser_reads_every_row_of_the_format_table() {
    let source = mime_source();
    let table = formats_table(&source);
    let catalogue = parse_catalogue(&source);

    assert_eq!(
        catalogue.len(),
        table.matches("FormatEntry {").count(),
        "the catalogue parser dropped rows of core/mime.rs's FORMATS table; \
         every guard in this file would be weakened by the difference"
    );

    let with_extensions = catalogue.iter().filter(|entry| !entry.extensions.is_empty()).count();
    assert!(
        with_extensions >= MIN_ENTRIES_WITH_EXTENSIONS,
        "only {with_extensions} catalogue entries carry a file extension, expected at least \
         {MIN_ENTRIES_WITH_EXTENSIONS}; the parser is not reading the real table"
    );

    let extensions: HashSet<&str> = catalogue
        .iter()
        .flat_map(|entry| entry.extensions.iter().map(String::as_str))
        .collect();
    let unseen: Vec<&str> = REGRESSION_EXTENSIONS
        .iter()
        .copied()
        .filter(|extension| !extensions.contains(extension))
        .collect();

    assert!(
        unseen.is_empty(),
        "the parser did not reach the catalogue rows this regression is about: {unseen:?}"
    );
}
