//! WordPerfect text extraction for Xberg.
//!
//! Thin, safe wrapper over [libwpd](https://libwpd.sourceforge.net/) and its
//! document-model dependency librevenge, both built from source against their
//! MPL-2.0 arm (see `build.rs`). libwpd covers the whole WordPerfect binary
//! family (WP 4.2 through the X-series).
//!
//! libwpd has no `extract()` entry point; it drives a librevenge callback
//! interface. A hand-written C++ shim (`src/shim.cpp`) implements that
//! interface, accumulates a text or (via [`extract_markdown`]) lightly
//! Markdown-marked-up rendering, and exposes a flat C API this crate wraps.
//! Footnotes, endnotes, comments, text boxes, headers and footers are always
//! bracketed apart from body text rather than concatenated into it. WordPerfect
//! support targets Linux, macOS and Windows; on other platforms the functions
//! return [`WpdError::UnsupportedPlatform`].

mod error;

pub use error::WpdError;

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
mod imp {
    use crate::WpdError;
    use std::ffi::CStr;
    use std::os::raw::{c_char, c_int, c_uchar, c_ulong};
    use std::{ptr, slice};

    unsafe extern "C" {
        fn xberg_wpd_is_supported(data: *const c_uchar, len: c_ulong) -> c_int;
        fn xberg_wpd_extract(
            data: *const c_uchar,
            len: c_ulong,
            markdown: c_int,
            out_text: *mut *mut c_char,
            out_len: *mut c_ulong,
            out_err: *mut *mut c_char,
        ) -> c_int;
        fn xberg_wpd_free_string(s: *mut c_char);
        #[cfg(test)]
        fn xberg_wpd_self_test_separation() -> c_int;
    }

    /// Returns true if `data` looks like a WordPerfect document libwpd can parse.
    pub fn is_supported(data: &[u8]) -> bool {
        if data.is_empty() || data.len() > u32::MAX as usize {
            return false;
        }
        // SAFETY: `data` is a valid slice of `len` bytes; the shim only reads it
        // and catches any C++ exception internally. ~keep
        unsafe { xberg_wpd_is_supported(data.as_ptr(), data.len() as c_ulong) != 0 }
    }

    /// Extract the plain text of a WordPerfect document held entirely in memory.
    pub fn extract_text(data: &[u8]) -> Result<String, WpdError> {
        extract(data, false)
    }

    /// Extract a Markdown-marked-up rendering of a WordPerfect document:
    /// heading paragraphs, bold/italic spans and list items are rendered as
    /// Markdown syntax. Tables remain tab/newline-separated in both modes (see
    /// the shim's `render` for why). Footnotes, endnotes, comments, text
    /// boxes, headers and footers are always bracketed apart from body text,
    /// in this mode as in [`extract_text`].
    pub fn extract_markdown(data: &[u8]) -> Result<String, WpdError> {
        extract(data, true)
    }

    fn extract(data: &[u8], markdown: bool) -> Result<String, WpdError> {
        if data.is_empty() || data.len() > u32::MAX as usize {
            return Err(WpdError::InvalidArgs);
        }

        let mut out: *mut c_char = ptr::null_mut();
        let mut out_len: c_ulong = 0;
        let mut out_err: *mut c_char = ptr::null_mut();
        // SAFETY: `data` is a valid slice of `len` bytes; `out`/`out_len`/`out_err`
        // are valid out-pointers. The shim catches any C++ exception and reports
        // it via the return code (plus, optionally, a detail message). On a zero
        // return it hands back a malloc'd buffer of exactly `out_len` bytes whose
        // ownership transfers to us. ~keep
        let code = unsafe {
            xberg_wpd_extract(
                data.as_ptr(),
                data.len() as c_ulong,
                markdown as c_int,
                &mut out,
                &mut out_len,
                &mut out_err,
            )
        };
        if !out_err.is_null() {
            // SAFETY: `out_err` is a malloc'd, NUL-terminated buffer the shim
            // handed us; freed unconditionally right after reading it. ~keep
            let detail = unsafe {
                let msg = CStr::from_ptr(out_err).to_string_lossy().into_owned();
                xberg_wpd_free_string(out_err);
                msg
            };
            tracing::warn!(code, error = %detail, "libwpd raised an exception during extraction");
        }
        if code != 0 {
            return Err(WpdError::from_code(code));
        }
        if out.is_null() {
            return Err(WpdError::Internal);
        }

        // SAFETY: `out` is the non-null buffer the shim allocated, exactly
        // `out_len` bytes long; we copy it out and free it through the matching
        // deallocator before returning. Using the explicit length (rather than
        // scanning for a NUL terminator) means an embedded NUL in the extracted
        // text can't silently truncate the result. ~keep
        let text = unsafe {
            let bytes = slice::from_raw_parts(out as *const u8, out_len as usize).to_vec();
            xberg_wpd_free_string(out);
            String::from_utf8(bytes)
        };
        text.map_err(|_| WpdError::InvalidUtf8)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn collector_separates_asides_from_body() {
            // SAFETY: takes no arguments and only touches its own stack-local state. ~keep
            assert_eq!(unsafe { xberg_wpd_self_test_separation() }, 1);
        }
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
mod imp {
    use crate::WpdError;

    /// WordPerfect extraction is desktop-only; unavailable on this target.
    pub fn is_supported(_data: &[u8]) -> bool {
        false
    }

    /// WordPerfect extraction is desktop-only; unavailable on this target.
    pub fn extract_text(_data: &[u8]) -> Result<String, WpdError> {
        Err(WpdError::UnsupportedPlatform)
    }

    /// WordPerfect extraction is desktop-only; unavailable on this target.
    pub fn extract_markdown(_data: &[u8]) -> Result<String, WpdError> {
        Err(WpdError::UnsupportedPlatform)
    }
}

pub use imp::{extract_markdown, extract_text, is_supported};
