//! Guards that `[patch.crates-io] ttf-parser` is actually engaged.
//!
//! `pdf_oxide` and `fontdb` are third-party crates that request `ttf-parser = "0.25"`
//! by name. We cannot edit their manifests, so the only way to make them use the
//! patched parser is the `[patch.crates-io]` entry in the root `Cargo.toml`, which
//! points at this name-compat shim.
//!
//! Cargo applies a patch only when the patch crate's version is semver-compatible with
//! what the dependents request — and when it is not, cargo says **nothing**. It just
//! resolves the real crates.io `ttf-parser` and carries on. That is what happened when a
//! blanket `alef sync-versions` rewrote the shim's `version = "0.25.1"` to the workspace
//! version `1.1.0`: every rasterized page silently lost its dotted and accented glyphs,
//! because upstream's parser aborts a charstring containing the `dotsection` operator.
//!
//! The end-to-end symptom (`pdf::render::tests::test_render_paints_cff_glyphs_that_use_dotsection`)
//! is a blank glyph, which reads like a font bug rather than a dependency-resolution bug.
//! These tests fail on the actual cause instead.

use std::path::PathBuf;

/// Walk up from this crate to the workspace root and read the lockfile.
fn workspace_lock() -> String {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    loop {
        let candidate = path.join("Cargo.lock");
        if candidate.exists() {
            return std::fs::read_to_string(candidate).expect("workspace Cargo.lock must be readable");
        }
        assert!(
            path.pop(),
            "no Cargo.lock found walking up from {}",
            env!("CARGO_MANIFEST_DIR")
        );
    }
}

/// Split a lockfile into its `[[package]]` blocks.
fn packages(lock: &str) -> Vec<&str> {
    lock.split("[[package]]").skip(1).collect()
}

fn block_field<'a>(block: &'a str, key: &str) -> Option<&'a str> {
    block
        .lines()
        .find_map(|line| {
            line.strip_prefix(key)?
                .trim()
                .strip_prefix('=')?
                .trim()
                .strip_prefix('"')
        })
        .and_then(|rest| rest.split('"').next())
}

fn package_block<'a>(lock: &'a str, name: &str) -> Option<&'a str> {
    packages(lock)
        .into_iter()
        .find(|block| block_field(block, "name") == Some(name))
}

#[test]
fn should_resolve_exactly_one_ttf_parser_and_it_must_be_ours() {
    let lock = workspace_lock();
    let ttf_blocks: Vec<&str> = packages(&lock)
        .into_iter()
        .filter(|block| block_field(block, "name") == Some("ttf-parser"))
        .collect();

    assert_eq!(
        ttf_blocks.len(),
        1,
        "expected exactly one `ttf-parser` in the lockfile; {} means [patch.crates-io] is not \
         engaged and the real crates.io parser is being compiled alongside ours. Check that \
         crates/ttf-parser-compat/Cargo.toml still declares a version inside `^0.25`.",
        ttf_blocks.len()
    );

    let block = ttf_blocks[0];
    assert!(
        block_field(block, "source").is_none(),
        "the resolved `ttf-parser` carries a `source` field, so it came from the registry \
         rather than from crates/ttf-parser-compat"
    );
    assert!(
        block.contains("xberg-ttf-parser"),
        "the resolved `ttf-parser` does not depend on `xberg-ttf-parser`, so it is not our \
         compat shim: {block}"
    );
}

#[test]
fn should_point_every_third_party_consumer_at_the_patched_parser() {
    let lock = workspace_lock();

    // A qualified entry (`"ttf-parser 0.25.1"`) means the lockfile had to disambiguate
    // between two packages of that name — i.e. the patch did not take. When the patch is
    // engaged there is only one, and cargo writes it bare.
    for consumer in ["pdf_oxide", "fontdb"] {
        let Some(block) = package_block(&lock, consumer) else {
            continue; // consumer not in this feature resolution
        };

        for line in block.lines().map(str::trim) {
            if line.starts_with("\"ttf-parser") {
                assert_eq!(
                    line.trim_end_matches(','),
                    "\"ttf-parser\"",
                    "{consumer} depends on a version-qualified ttf-parser, which means two \
                     packages of that name exist and {consumer} resolved to the unpatched one"
                );
            }
        }
    }
}

/// The parser is consumed from crates.io, not vendored into this workspace.
///
/// It used to live at `crates/xberg-ttf-parser/` — 27k lines of vendored upstream. Nothing
/// here depends on `ttf-parser` directly, so carrying that source bought only the nine
/// upstream fixes it cherry-picked, and those now ship in the published crate. If this
/// assertion fails, someone has re-vendored the parser behind a path dependency and the
/// workspace has silently grown the maintenance burden back.
#[test]
fn should_consume_the_parser_from_the_registry_rather_than_a_path() {
    let lock = workspace_lock();
    let block = package_block(&lock, "xberg-ttf-parser")
        .expect("`xberg-ttf-parser` must be in the lockfile; the compat shim depends on it");

    let source = block_field(block, "source").unwrap_or_else(|| {
        panic!(
            "`xberg-ttf-parser` has no `source`, so it resolved to a path inside this \
             workspace rather than to the published crate: {block}"
        )
    });
    assert!(
        source.starts_with("registry+"),
        "`xberg-ttf-parser` resolved from `{source}` rather than a registry"
    );
    assert!(
        block_field(block, "checksum").is_some(),
        "`xberg-ttf-parser` carries no checksum, so it is not a verified registry release: {block}"
    );
}
