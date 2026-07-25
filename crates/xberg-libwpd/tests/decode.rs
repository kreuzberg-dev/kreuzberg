//! Synthetic error-path and concurrency tests for the libwpd bindings: they
//! need no corpus, so they run everywhere and exercise the shim's exception
//! safety (malformed input must never crash across the FFI boundary) and
//! thread-safety (no shared mutable state across calls).
//!
//! Decoding of real documents is covered in `ground_truth.rs` against the
//! `test_documents/` submodule. No WordPerfect binaries live in this crate.

#[test]
fn empty_input_is_not_supported() {
    assert!(!xberg_libwpd::is_supported(&[]));
    assert!(xberg_libwpd::extract_text(&[]).is_err());
}

#[test]
fn random_bytes_do_not_crash() {
    let junk: Vec<u8> = (0..4096u32).map(|i| (i.wrapping_mul(2654435761) >> 24) as u8).collect();
    assert!(!xberg_libwpd::is_supported(&junk));
    assert!(xberg_libwpd::extract_text(&junk).is_err());
}

#[test]
fn wordperfect_magic_but_truncated_body_fails_gracefully() {
    let mut buf = vec![0xff, b'W', b'P', b'C'];
    buf.extend_from_slice(&[0x95, 0x06, 0x00, 0x00]);
    buf.resize(64, 0);
    let _ = xberg_libwpd::is_supported(&buf);
    assert!(xberg_libwpd::extract_text(&buf).is_err());
}

/// `extract_text`/`extract_markdown`/`is_supported` construct a fresh
/// `DocumentBuilder` per call and share no mutable state across the FFI
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
