/// Regression test for GitHub #1059.
///
/// `xberg_email_attachment_data` was the only byte-buffer accessor on a public
/// FFI-exposed DTO that did not follow the established `*_data(ptr, out_len: *mut usize)`
/// protocol used by `xberg_extracted_image_data`, `xberg_embedded_file_data`,
/// and `xberg_batch_bytes_item_content`.
///
/// Because `EmailAttachment.data` is `Option<Bytes>` (the only optional byte buffer among
/// public types), alef's heuristic for emitting the two-parameter form did not trigger.
/// Callers had no way to know the valid length of the returned pointer, making any read
/// past the first byte undefined behaviour (especially for payloads containing 0x00).
///
/// The alef fix shipped with the 2-parameter form (`ptr`, `out_len`). These tests
/// lock in the correct 2-param ABI and verify the full-length contract for payloads
/// that contain embedded NUL bytes.
///
/// Per project rules: every unsafe block has a SAFETY comment.
use std::ffi::{CString, c_char};
use std::fs;
use std::path::Path;

use xberg_ffi::{xberg_email_attachment_free, xberg_email_attachment_from_json, xberg_last_error_code};

/// Construct a minimal EmailAttachment JSON with a data payload that contains
/// an embedded NUL and a trailing high byte (0xEF). This defeats any strlen-based
/// or "read first byte only" implementations.
fn attachment_json_with_nuls() -> CString {
    // 8 bytes: JPEG-ish magic + NUL in the middle + high byte at the end. ~keep
    let data: Vec<u8> = vec![0xFF, 0xD8, 0xFF, 0x00, 0xDE, 0xAD, 0xBE, 0xEF];
    let json = format!(
        r#"{{
            "name": "test.bin",
            "filename": "test.bin",
            "mime_type": "application/octet-stream",
            "size": {},
            "is_image": false,
            "data": {}
        }}"#,
        data.len(),
        serde_json::to_string(&data).unwrap()
    );
    CString::new(json).expect("valid UTF-8 JSON for test attachment")
}

/// The committed C header must declare the 2-parameter form for
/// `xberg_email_attachment_data` (with `out_len`). This locks in the fix
/// for GitHub #1059 so a future regeneration cannot silently revert to the
/// 1-parameter form.
#[test]
fn email_attachment_data_accessor_must_provide_out_len_in_header() {
    let header_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("include/xberg.h");
    let header = fs::read_to_string(&header_path).expect("committed xberg.h must be readable by the test");

    let has_out_len = header.contains("xberg_email_attachment_data") && header.contains("out_len");

    assert!(
        has_out_len,
        "GitHub #1059 regression: the declaration of xberg_email_attachment_data \
         in crates/xberg-ffi/include/xberg.h does not contain the required \
         `out_len` parameter.\n\n\
         Expected something like:\n    uint8_t *xberg_email_attachment_data(..., uintptr_t *out_len);\n\n\
         Found the old 1-parameter form. Fix requires `task alef:generate` with an \
         updated alef that handles Option<Bytes> fields for the FFI byte accessor heuristic.\n\n\
         This is the lock-in test for #1059."
    );
}

/// When an attachment has no data payload the accessor must return a null pointer
/// and write 0 to out_len.
#[test]
fn email_attachment_data_none_returns_null_pointer() {
    let json = CString::new(
        r#"{"name":"empty","filename":"empty","mime_type":null,"size":null,"is_image":false,"data":null}"#,
    )
    .unwrap();

    // SAFETY: json is valid null-terminated UTF-8.
    let handle = unsafe { xberg_email_attachment_from_json(json.as_ptr() as *const c_char) };
    assert!(
        handle != 0,
        "from_json should succeed (last_error_code={})",
        // SAFETY: no precondition; reads a thread-local.
        unsafe { xberg_last_error_code() }
    );

    let mut out_len: usize = usize::MAX;
    // SAFETY: handle is a valid non-null pointer returned by from_json;
    // out_len is a valid stack-allocated usize. ~keep
    let data_ptr = unsafe { xberg_ffi::xberg_email_attachment_data(handle, &mut out_len) };

    assert!(
        data_ptr.is_null(),
        "data must be null when the attachment has no payload"
    );
    assert_eq!(out_len, 0, "out_len must be 0 when data is None");

    // SAFETY: handle came from from_json; we are the sole owner.
    unsafe { xberg_email_attachment_free(handle) };
}

/// When an attachment carries a binary payload the accessor must return a non-null
/// pointer and write the exact byte count — including bytes past any embedded NUL —
/// to out_len. This is the core contract broken by the 1-parameter bug (#1059).
#[test]
fn email_attachment_data_with_out_len_returns_full_buffer_including_embedded_nuls() {
    let json = attachment_json_with_nuls();
    // SAFETY: json is a valid null-terminated CString we just created.
    let handle = unsafe { xberg_email_attachment_from_json(json.as_ptr() as *const c_char) };
    assert!(
        handle != 0,
        "from_json should succeed for our well-formed test attachment (last_error_code={})",
        // SAFETY: no precondition; reads a thread-local.
        unsafe { xberg_last_error_code() }
    );

    let mut out_len: usize = 0;

    // SAFETY: handle is non-null and freshly allocated by from_json;
    // out_len is a valid stack-allocated usize. The returned pointer must not
    // be freed by us — it borrows the internal Bytes of the handle. ~keep
    let data_ptr = unsafe { xberg_ffi::xberg_email_attachment_data(handle, &mut out_len) };

    assert!(
        !data_ptr.is_null(),
        "data pointer must be non-null for an attachment we created with a Some(data) payload"
    );
    assert_eq!(
        out_len, 8,
        "out_len must report the exact length of the Bytes payload (not 0, not guessed, not truncated at NUL)"
    );

    // SAFETY: data_ptr is valid for [0..out_len] because:
    // - it came from the handle's internal Bytes (which we control),
    // - out_len was written by the accessor,
    // - the handle is still alive (we have not called free yet). ~keep
    let slice = unsafe { std::slice::from_raw_parts(data_ptr, out_len) };

    assert_eq!(slice.len(), 8);
    assert_eq!(slice[0], 0xFF);
    assert_eq!(slice[3], 0x00, "must be able to read the embedded NUL");
    assert_eq!(
        slice[7], 0xEF,
        "must be able to read bytes after the NUL (no truncation)"
    );

    // SAFETY: handle came from from_json; we are the owner.
    unsafe { xberg_email_attachment_free(handle) };
}

/// Verify that passing a null out_len pointer is safe: the accessor must not
/// segfault, and the data pointer itself must still be returned.
#[test]
fn email_attachment_data_null_out_len_is_safe() {
    let json = CString::new(
        r#"{"name":"hasdata.bin","filename":"hasdata.bin","mime_type":"application/octet-stream","size":4,"is_image":false,"data":[65,0,66,67]}"#,
    )
    .unwrap();

    // SAFETY: json is valid.
    let handle = unsafe { xberg_email_attachment_from_json(json.as_ptr() as *const c_char) };
    assert!(handle != 0);

    // SAFETY: handle is valid; passing null for out_len is a defined contract
    // (the accessor null-checks before writing). ~keep
    let data_ptr = unsafe { xberg_ffi::xberg_email_attachment_data(handle, std::ptr::null_mut()) };

    assert!(
        !data_ptr.is_null(),
        "data pointer should be non-null when the attachment carries a payload"
    );

    // SAFETY: handle from from_json; we are the owner.
    unsafe { xberg_email_attachment_free(handle) };
}
