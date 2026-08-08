//! Regression test for #206 — the extraction cache key carried no version
//! fingerprint.
//!
//! The cache key was `blake3(file bytes) + blake3(config)`. Neither term moves
//! when *extraction behaviour* changes, so an entry written by an older build
//! stayed valid forever and kept serving the output of a bug after the bug was
//! fixed. Every key is now prefixed with an 8-hex build fingerprint.
//!
//! This test drives the public extraction API, inspects the on-disk entry, and
//! then relabels it with a foreign fingerprint to prove it is no longer served.

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::tempdir;
use xberg::core::config::{ExtractInput, ExtractionConfig};

/// `std::env::set_var` is `unsafe` in edition 2024; this binary contains a
/// single test, so no other thread is reading the environment concurrently.
#[allow(unsafe_code)]
fn set_env(key: &str, value: &str) {
    unsafe {
        std::env::set_var(key, value);
    }
}

const DOCUMENT_TEXT: &str = "issue 206 fingerprint payload";
/// A fingerprint no real build can produce for a non-empty version string.
const FOREIGN_TAG: &str = "00000000";
/// Width of the build fingerprint that prefixes every cache key.
const TAG_LEN: usize = 8;

/// Cache blobs (`*.msgpack`) directly inside `dir`, sorted for determinism.
fn blobs_in(dir: &Path) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = fs::read_dir(dir)
        .expect("read cache dir")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension() == Some(OsStr::new("msgpack")))
        .collect();
    paths.sort();
    paths
}

/// The `<tag>` part of a `<tag>-<key>.msgpack` filename.
fn tag_of(path: &Path) -> String {
    let stem = path
        .file_stem()
        .and_then(OsStr::to_str)
        .unwrap_or_else(|| panic!("cache blob {} has no stem", path.display()));
    let (tag, key) = stem
        .split_once('-')
        .unwrap_or_else(|| panic!("cache blob stem {stem:?} is not <tag>-<key>; #206 is not fixed"));
    assert_eq!(
        tag.len(),
        TAG_LEN,
        "the build fingerprint must be {TAG_LEN} characters, got {tag:?}"
    );
    assert!(
        tag.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
        "the build fingerprint must be lowercase hex, got {tag:?}"
    );
    assert!(!key.is_empty(), "the content/config key must survive the prefix");
    tag.to_string()
}

/// Rename `<tag>-<key>.<ext>` to `<FOREIGN_TAG>-<key>.<ext>`, preserving bytes.
fn relabel(path: &Path) {
    let stem = path.file_stem().and_then(OsStr::to_str).expect("stem");
    let extension = path.extension().and_then(OsStr::to_str).expect("extension");
    let (_, key) = stem.split_once('-').expect("<tag>-<key>");
    let renamed = path.with_file_name(format!("{FOREIGN_TAG}-{key}.{extension}"));
    fs::rename(path, &renamed).expect("relabel cache entry");
}

#[tokio::test]
async fn an_entry_written_by_a_different_build_must_not_be_served() {
    let temp = tempdir().expect("temp dir");
    let cache_root = temp.path().join("cache-root");
    set_env("XBERG_CACHE_DIR", cache_root.to_str().expect("utf-8 cache root"));

    let source = temp.path().join("document.txt");
    fs::write(&source, DOCUMENT_TEXT).expect("write source document");

    let config = ExtractionConfig {
        use_cache: true,
        ..Default::default()
    };

    // 1. Populate the cache.
    let first = xberg::extract(ExtractInput::from_uri(source.to_string_lossy()), &config)
        .await
        .expect("first extraction");
    assert_eq!(first.results.len(), 1);
    assert_eq!(first.results[0].content.trim(), DOCUMENT_TEXT);

    let cache_dir = cache_root.join("extraction");
    let blobs = blobs_in(&cache_dir);
    assert_eq!(
        blobs.len(),
        1,
        "expected exactly one cache blob in {}, found {blobs:?}",
        cache_dir.display()
    );

    // 2. The on-disk key must carry the build fingerprint.
    let real_tag = tag_of(&blobs[0]);
    assert_ne!(real_tag, FOREIGN_TAG, "the real fingerprint must not be all zeroes");

    // 3. Relabel the (still perfectly valid) entry as another build's. Bytes are
    //    untouched, so anything that ignores the fingerprint still reads it.
    relabel(&blobs[0]);
    for meta in fs::read_dir(&cache_dir)
        .expect("read cache dir")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension() == Some(OsStr::new("meta")))
    {
        relabel(&meta);
    }
    assert_eq!(
        blobs_in(&cache_dir).iter().map(|path| tag_of(path)).collect::<Vec<_>>(),
        vec![FOREIGN_TAG.to_string()],
        "precondition: only the foreign-tagged entry may remain"
    );

    // 4. Extracting again must miss, re-extract, and write a fresh entry under
    //    the real fingerprint.
    let second = xberg::extract(ExtractInput::from_uri(source.to_string_lossy()), &config)
        .await
        .expect("second extraction");
    assert_eq!(second.results.len(), 1);
    assert_eq!(
        second.results[0].content.trim(),
        DOCUMENT_TEXT,
        "the re-extracted content must match the original"
    );

    let mut tags: Vec<String> = blobs_in(&cache_dir).iter().map(|path| tag_of(path)).collect();
    tags.sort();
    let mut expected = vec![FOREIGN_TAG.to_string(), real_tag];
    expected.sort();
    assert_eq!(
        tags, expected,
        "a foreign-tagged entry must be ignored and a fresh entry written under this build's fingerprint"
    );
}
