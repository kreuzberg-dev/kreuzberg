//! Tests for the libwpd bindings.
//!
//! The error-path and concurrency tests run everywhere and exercise the
//! shim's exception safety (malformed input must never crash across the FFI
//! boundary) and thread-safety (no shared mutable state across calls). The
//! decode test needs a real WordPerfect sample from the repository-root
//! `test_documents/` submodule and skips when it is not checked out,
//! mirroring the other extractor tests in this workspace. No WordPerfect
//! binaries are stored in the crate.

use std::path::PathBuf;

fn fixture(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../test_documents")
        .join(rel)
}

#[test]
fn empty_input_is_not_supported() {
    assert!(!xberg_libwpd::is_supported(&[]));
    assert!(xberg_libwpd::extract_text(&[]).is_err());
}

#[test]
fn random_bytes_do_not_crash() {
    // Deterministic pseudo-random garbage; must be rejected, never panic.
    let junk: Vec<u8> = (0..4096u32).map(|i| (i.wrapping_mul(2654435761) >> 24) as u8).collect();
    assert!(!xberg_libwpd::is_supported(&junk));
    assert!(xberg_libwpd::extract_text(&junk).is_err());
}

#[test]
fn wordperfect_magic_but_truncated_body_fails_gracefully() {
    // "\xffWPC" is the WordPerfect magic; a header with no real body must fail
    // through libwpd rather than crash.
    let mut buf = vec![0xff, b'W', b'P', b'C'];
    buf.extend_from_slice(&[0x95, 0x06, 0x00, 0x00]);
    buf.resize(64, 0);
    let _ = xberg_libwpd::is_supported(&buf);
    assert!(xberg_libwpd::extract_text(&buf).is_err());
}

#[test]
fn extracts_text_from_sample() {
    let path = fixture("wordperfect/sample.wpd");
    if !path.exists() {
        eprintln!("skipping: {} not present (test_documents submodule)", path.display());
        return;
    }
    let bytes = std::fs::read(&path).expect("read sample.wpd");
    assert!(xberg_libwpd::is_supported(&bytes), "sample should be recognized");
    let text = xberg_libwpd::extract_text(&bytes).expect("extract_text");
    assert!(!text.trim().is_empty(), "expected non-empty extracted text");
    let markdown = xberg_libwpd::extract_markdown(&bytes).expect("extract_markdown");
    assert!(!markdown.trim().is_empty(), "expected non-empty markdown extraction");
}

/// `extract_text`/`extract_markdown`/`is_supported` construct a fresh
/// `TextCollector` per call and share no mutable state across the FFI
/// boundary, so concurrent callers must not crash or corrupt each other's
/// output. Runs on the same deterministic junk buffer from every thread so a
/// data race would show up as a spurious `Ok` or a panic, not just noise.
#[test]
fn concurrent_calls_do_not_crash() {
    let junk: std::sync::Arc<[u8]> = (0..4096u32).map(|i| (i.wrapping_mul(2654435761) >> 24) as u8).collect();

    let handles: Vec<_> = (0..8)
        .map(|_| {
            let junk = std::sync::Arc::clone(&junk);
            std::thread::spawn(move || {
                for _ in 0..50 {
                    assert!(!xberg_libwpd::is_supported(&junk));
                    assert!(xberg_libwpd::extract_text(&junk).is_err());
                    assert!(xberg_libwpd::extract_markdown(&junk).is_err());
                }
            })
        })
        .collect();

    for h in handles {
        h.join().expect("worker thread panicked");
    }
}
