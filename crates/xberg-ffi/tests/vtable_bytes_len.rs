use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};
/// Regression tests: vtable Bytes params carry companion length
///
/// The alef vtable generator previously emitted only `*const u8` for `&[u8]`
/// trait-method parameters without a companion `{name}_len: usize`. Binary
/// payloads contain embedded NUL bytes; read-until-NUL semantics silently
/// truncated every real image or document buffer at the first `0x00`.
///
/// Fix shipped in alef ≥ v0.19.21 and is present in the generated FFI shim.
/// These tests construct a vtable bridge directly, pass a buffer with an
/// embedded NUL at a known offset, and assert the full buffer is received.
///
/// Per-test state is passed via `user_data` — no global statics — so tests
/// are independent and can run in parallel without interfering.
use xberg_ffi::{
    XbergDocumentExtractorBridge, XbergDocumentExtractorVTable, XbergOcrBackendBridge, XbergOcrBackendVTable,
};

struct CallbackState {
    received_len: AtomicUsize,
    received_last_byte: AtomicU8,
}

impl CallbackState {
    fn new() -> Self {
        Self {
            received_len: AtomicUsize::new(0),
            received_last_byte: AtomicU8::new(0),
        }
    }
}

unsafe extern "C" fn ocr_process_image(
    user_data: *const std::ffi::c_void,
    image_bytes: *const u8,
    image_bytes_len: usize,
    _config: *const std::ffi::c_char,
    out_result: *mut *mut std::ffi::c_char,
    out_error: *mut *mut std::ffi::c_char,
) -> i32 {
    // SAFETY: user_data points to a CallbackState that the calling test keeps alive.
    let state = unsafe { &*(user_data as *const CallbackState) };
    state.received_len.store(image_bytes_len, Ordering::SeqCst);
    if image_bytes_len > 0 {
        // SAFETY: caller guarantees image_bytes[0..image_bytes_len] is valid.
        let last = unsafe { *image_bytes.add(image_bytes_len - 1) };
        state.received_last_byte.store(last, Ordering::SeqCst);
    }
    unsafe { *out_result = std::ptr::null_mut() };
    let msg = std::ffi::CString::new("stub").unwrap();
    // SAFETY: caller owns out_error and will free it via xberg_free_string.
    unsafe { *out_error = msg.into_raw() };
    1
}

unsafe extern "C" fn extractor_extract(
    user_data: *const std::ffi::c_void,
    input: *const std::ffi::c_char,
    _config: *const std::ffi::c_char,
    out_result: *mut *mut std::ffi::c_char,
    out_error: *mut *mut std::ffi::c_char,
) -> i32 {
    // SAFETY: user_data points to a CallbackState that the calling test keeps alive.
    let state = unsafe { &*(user_data as *const CallbackState) };
    let input_json = if input.is_null() {
        String::new()
    } else {
        // SAFETY: caller passes a valid NUL-terminated JSON string.
        unsafe { std::ffi::CStr::from_ptr(input) }
            .to_string_lossy()
            .into_owned()
    };
    if let Ok(input_value) = serde_json::from_str::<serde_json::Value>(&input_json) {
        if let Some(bytes) = input_value.get("bytes").and_then(serde_json::Value::as_array) {
            state.received_len.store(bytes.len(), Ordering::SeqCst);
            if let Some(last) = bytes.last().and_then(serde_json::Value::as_u64) {
                state.received_last_byte.store(last as u8, Ordering::SeqCst);
            }
        }
    }
    unsafe { *out_result = std::ptr::null_mut() };
    let msg = std::ffi::CString::new("stub").unwrap();
    unsafe { *out_error = msg.into_raw() };
    1
}

/// OcrBackend.process_image must pass the full buffer length even when
/// the payload contains embedded NUL bytes.
#[tokio::test]
async fn ocr_backend_vtable_process_image_passes_full_length_with_embedded_nuls() {
    // 8-byte buffer; NUL at index 3. strlen-style reads would stop at 3. ~keep
    let image_bytes: Vec<u8> = vec![0xFF, 0xD8, 0xFF, 0x00, 0xDE, 0xAD, 0xBE, 0xEF];

    let state = Box::new(CallbackState::new());
    let state_ptr = state.as_ref() as *const CallbackState as *const std::ffi::c_void;

    let vtable = XbergOcrBackendVTable {
        process_image: Some(ocr_process_image),
        process_image_file: None,
        name_fn: None,
        version_fn: None,
        initialize_fn: None,
        shutdown_fn: None,
        supports_language: None,
        backend_type: None,
        supported_languages: None,
        supports_table_detection: None,
        supports_document_processing: None,
        emits_structured_markdown: None,
        process_document: None,
        free_string: None,
        free_user_data: None,
    };

    // SAFETY: state lives for the duration of this test and outlives the bridge.
    let bridge = unsafe { XbergOcrBackendBridge::new("test-ocr-stub".to_string(), vtable, state_ptr) };

    use xberg::OcrBackend;
    let _ = bridge.process_image(&image_bytes, &xberg::OcrConfig::default()).await;

    assert_eq!(
        state.received_len.load(Ordering::SeqCst),
        8,
        "process_image vtable received wrong length (truncated at embedded NUL?)"
    );
    assert_eq!(
        state.received_last_byte.load(Ordering::SeqCst),
        0xEF,
        "process_image vtable could not read past the embedded NUL"
    );
}

/// DocumentExtractor.extract must pass the full bytes input even when the
/// document bytes contain embedded NUL bytes.
#[tokio::test]
async fn document_extractor_vtable_extract_passes_full_length_with_embedded_nuls() {
    let content: Vec<u8> = vec![0x50, 0x4B, 0x00, 0x03, 0x14, 0x00, 0x00, 0x02];

    let state = Box::new(CallbackState::new());
    let state_ptr = state.as_ref() as *const CallbackState as *const std::ffi::c_void;

    let vtable = XbergDocumentExtractorVTable {
        name_fn: None,
        version_fn: None,
        initialize_fn: None,
        shutdown_fn: None,
        extract: Some(extractor_extract),
        supported_mime_types: None,
        priority: None,
        can_handle: None,
        free_string: None,
        free_user_data: None,
    };

    // SAFETY: state lives for the duration of this test and outlives the bridge.
    let bridge = unsafe { XbergDocumentExtractorBridge::new("test-extractor-stub".to_string(), vtable, state_ptr) };

    use xberg::DocumentExtractor;
    let input = xberg::ExtractInput::from_bytes(content, "application/octet-stream", Some("embedded.bin".to_string()));
    let _ = bridge.extract(input, &xberg::ExtractionConfig::default()).await;

    assert_eq!(
        state.received_len.load(Ordering::SeqCst),
        8,
        "extract vtable received wrong length (truncated at embedded NUL?)"
    );
    assert_eq!(
        state.received_last_byte.load(Ordering::SeqCst),
        0x02,
        "extract vtable could not read past the embedded NUL"
    );
}

/// ImageKind numeric values: PageRaster must be 10 and Unknown must be 11.
///
/// alef ≥ v0.19.21 added PageRaster between Mask (9) and Unknown, bumping
/// Unknown from 10 → 11. Any C/Go/Java/C# code that hardcoded Unknown = 10
/// must be updated; this test pins the new ordinals so the renumbering is
/// visible to CI.
#[test]
fn image_kind_page_raster_is_10_and_unknown_is_11() {
    // SAFETY: pure integer dispatch, no pointers.
    assert_eq!(
        unsafe { xberg_ffi::xberg_image_kind_from_i32(10) },
        10,
        "PageRaster == 10"
    );
    assert_eq!(unsafe { xberg_ffi::xberg_image_kind_from_i32(11) }, 11, "Unknown == 11");
    assert_ne!(
        unsafe { xberg_ffi::xberg_image_kind_from_i32(10) },
        -1,
        "10 must be valid"
    );
}
