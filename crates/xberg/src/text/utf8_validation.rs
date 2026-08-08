//! SIMD-accelerated UTF-8 validation.
//!
//! This module provides high-performance UTF-8 validation using SIMD instructions
//! when available. On platforms without SIMD support, it falls back to standard
//! validation.
//!
//! # Performance
//!
//! SIMD validation can process 16-32 bytes per cycle, providing 15-20% improvement
//! over standard byte-by-byte validation on text-heavy operations.
//!
//! # Example
//!
//! Not run as a doctest: `from_utf8` is `pub(crate)`, an internal drop-in for
//! [`std::str::from_utf8`] used on the extraction hot paths.
//!
//! ```ignore
//! use xberg::text::utf8_validation::from_utf8;
//!
//! let bytes = b"Hello, UTF-8 world!";
//! let result = from_utf8(bytes).expect("valid UTF-8");
//! assert_eq!(result, "Hello, UTF-8 world!");
//! ```

/// Validates and converts bytes to string using SIMD when available.
///
/// This function attempts to use SIMD UTF-8 validation if the `simd-utf8` feature
/// is enabled and the platform supports it. Otherwise, it falls back to the standard
/// `std::str::from_utf8()` validation.
///
/// # Arguments
///
/// * `bytes` - The byte slice to validate and convert
///
/// # Returns
///
/// `Ok(&str)` if the bytes are valid UTF-8, `Err(std::str::Utf8Error)` otherwise.
///
/// # Safety
///
/// This function is safe and does not use any unsafe code directly. The underlying
/// SIMD validation (when enabled) is contained within the simdutf8 crate and is safe.
#[inline]
pub(crate) fn from_utf8(bytes: &[u8]) -> Result<&str, std::str::Utf8Error> {
    #[cfg(feature = "simd-utf8")]
    {
        simdutf8::basic::from_utf8(bytes).map_err(|_| {
            #[allow(invalid_from_utf8)]
            let err = std::str::from_utf8(&[0xFF, 0xFF, 0xFF, 0xFF]).unwrap_err();
            err
        })
    }

    #[cfg(not(feature = "simd-utf8"))]
    {
        std::str::from_utf8(bytes)
    }
}

/// Validates and converts owned bytes to String using SIMD when available.
///
/// This function converts bytes to an owned String, validating UTF-8 using SIMD
/// when available. The caller's bytes are consumed to create the String.
///
/// # Arguments
///
/// * `bytes` - The byte vector to validate and convert
///
/// # Returns
///
/// `Ok(String)` if the bytes are valid UTF-8, `Err(std::string::FromUtf8Error)` otherwise.
///
/// # Performance
///
/// When enabled, SIMD validation significantly reduces the time spent on validation,
/// especially for large text documents.
#[cfg(feature = "quality")]
#[inline]
pub(crate) fn string_from_utf8(bytes: Vec<u8>) -> Result<String, std::string::FromUtf8Error> {
    #[cfg(feature = "simd-utf8")]
    {
        #[allow(clippy::collapsible_if)]
        if simdutf8::basic::from_utf8(&bytes).is_ok() {
            #[allow(unsafe_code)]
            Ok(unsafe { String::from_utf8_unchecked(bytes) })
        } else {
            String::from_utf8(bytes)
        }
    }

    #[cfg(not(feature = "simd-utf8"))]
    {
        String::from_utf8(bytes)
    }
}

/// Validates bytes as UTF-8 without conversion to string slice.
///
/// Returns `true` if the bytes represent valid UTF-8, `false` otherwise.
/// This is useful when you only need to check validity without constructing a string.
///
/// # Arguments
///
/// * `bytes` - The byte slice to validate
///
/// # Returns
///
/// `true` if valid UTF-8, `false` otherwise.
///
/// # Performance
///
/// This function is optimized for early exit on invalid sequences.
#[cfg(test)]
#[inline]
pub(crate) fn is_valid_utf8(bytes: &[u8]) -> bool {
    #[cfg(feature = "simd-utf8")]
    {
        simdutf8::basic::from_utf8(bytes).is_ok()
    }

    #[cfg(not(feature = "simd-utf8"))]
    {
        std::str::from_utf8(bytes).is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_ascii() {
        let bytes = b"Hello, world!";
        let result = from_utf8(bytes).unwrap();
        assert_eq!(result, "Hello, world!");
    }

    #[test]
    fn test_valid_utf8_multibyte() {
        let bytes = "Hello, 世界! 🌍".as_bytes();
        let result = from_utf8(bytes).unwrap();
        assert_eq!(result, "Hello, 世界! 🌍");
    }

    #[test]
    fn test_empty_bytes() {
        let bytes = b"";
        let result = from_utf8(bytes).unwrap();
        assert_eq!(result, "");
    }

    #[test]
    fn test_invalid_utf8() {
        let bytes: &[u8] = &[0xFF, 0xFE];
        assert!(from_utf8(bytes).is_err());
    }

    #[test]
    fn test_is_valid_utf8_true() {
        let bytes = "Valid UTF-8 text".as_bytes();
        assert!(is_valid_utf8(bytes));
    }

    #[test]
    fn test_is_valid_utf8_false() {
        let bytes: &[u8] = &[0xC0, 0x80];
        assert!(!is_valid_utf8(bytes));
    }

    #[cfg(feature = "quality")]
    #[test]
    fn test_string_from_utf8_valid() {
        let bytes = b"Test string".to_vec();
        let result = string_from_utf8(bytes).unwrap();
        assert_eq!(result, "Test string");
    }

    #[cfg(feature = "quality")]
    #[test]
    fn test_string_from_utf8_invalid() {
        let bytes: Vec<u8> = vec![0xFF, 0xFE];
        assert!(string_from_utf8(bytes).is_err());
    }

    #[test]
    fn test_large_valid_text() {
        let text = "a".repeat(100_000);
        let bytes = text.as_bytes();
        let result = from_utf8(bytes).unwrap();
        assert_eq!(result.len(), 100_000);
    }

    #[test]
    fn test_mixed_unicode() {
        let text = "Latin αλφα 中文 العربية עברית Кириллица";
        let bytes = text.as_bytes();
        let result = from_utf8(bytes).unwrap();
        assert_eq!(result, text);
    }
}
