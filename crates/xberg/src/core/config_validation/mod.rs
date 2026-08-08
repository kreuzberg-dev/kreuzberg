//! Configuration validation module.
//!
//! Provides centralized validation for configuration values across all bindings.
//! This eliminates duplication of validation logic in Python, TypeScript, Java, Go, and other language bindings.
//!
//! All validation functions return `Result<()>` and produce detailed error messages
//! suitable for user-facing error handling.
//!
//! # Examples
//!
//! ```ignore
//! use xberg::core::config_validation::{
//!     validate_binarization_method,
//!     validate_token_reduction_level,
//!     validate_language_code,
//! };
//!
//! // Valid values
//! assert!(validate_binarization_method("otsu").is_ok());
//! assert!(validate_token_reduction_level("moderate").is_ok());
//! assert!(validate_language_code("en").is_ok());
//!
//! // Invalid values
//! assert!(validate_binarization_method("invalid").is_err());
//! assert!(validate_token_reduction_level("extreme").is_err());
//! ```

mod dependencies;
mod sections;

pub(crate) use sections::{
    validate_chunking_params, validate_confidence, validate_csv_delimiter, validate_dpi, validate_language_code,
    validate_ocr_backend, validate_token_reduction_level, validate_vlm_backend_config,
};

// The remaining validators are exported under `#[cfg(test)]` only: every `ExtractionConfig`
// field they would apply to lives outside `crates/xberg/src/core/config*/**`, so wiring them
// into production requires editing those out-of-tree files (see issue #270 follow-up):
//
// - `validate_binarization_method`, `validate_tesseract_psm`, `validate_tesseract_oem`: their
//   target fields (`ImagePreprocessingConfig::binarization_method`, `TesseractConfig::{psm,oem}`)
//   live in `crate::types::formats`.
// - `validate_output_format`: both `ExtractionConfig::output_format` and
//   `OcrConfig::output_format` are already the strongly-typed `OutputFormat` enum, which
//   rejects invalid values at deserialization time — there is no raw-string field left to
//   validate with this function.
// - `validate_cors_origin`, `validate_host`, `validate_port`, `validate_upload_size`: their
//   target fields live on `ServerConfig` in `crate::core::server_config`, a separate module
//   tree from `crate::core::config`.
#[cfg(test)]
pub(crate) use dependencies::{validate_cors_origin, validate_host, validate_port, validate_upload_size};
#[cfg(test)]
pub(crate) use sections::{
    validate_binarization_method, validate_output_format, validate_tesseract_oem, validate_tesseract_psm,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_binarization_method_valid() {
        assert!(validate_binarization_method("otsu").is_ok());
        assert!(validate_binarization_method("adaptive").is_ok());
        assert!(validate_binarization_method("sauvola").is_ok());
    }

    #[test]
    fn test_validate_binarization_method_case_insensitive() {
        assert!(validate_binarization_method("OTSU").is_ok());
        assert!(validate_binarization_method("Adaptive").is_ok());
        assert!(validate_binarization_method("SAUVOLA").is_ok());
    }

    #[test]
    fn test_validate_binarization_method_invalid() {
        let result = validate_binarization_method("invalid");
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("Invalid binarization method"));
        assert!(msg.contains("otsu"));
    }

    #[test]
    fn test_validate_token_reduction_level_valid() {
        assert!(validate_token_reduction_level("off").is_ok());
        assert!(validate_token_reduction_level("light").is_ok());
        assert!(validate_token_reduction_level("moderate").is_ok());
        assert!(validate_token_reduction_level("aggressive").is_ok());
        assert!(validate_token_reduction_level("maximum").is_ok());
    }

    #[test]
    fn test_validate_token_reduction_level_case_insensitive() {
        assert!(validate_token_reduction_level("OFF").is_ok());
        assert!(validate_token_reduction_level("Moderate").is_ok());
        assert!(validate_token_reduction_level("MAXIMUM").is_ok());
    }

    #[test]
    fn test_validate_token_reduction_level_invalid() {
        let result = validate_token_reduction_level("extreme");
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("Invalid token reduction level"));
    }

    #[test]
    fn test_validate_ocr_backend_valid() {
        assert!(validate_ocr_backend("tesseract").is_ok());
        assert!(validate_ocr_backend("paddleocr").is_ok());
        assert!(validate_ocr_backend("sceptre").is_ok());
    }

    #[test]
    fn test_validate_ocr_backend_case_insensitive() {
        assert!(validate_ocr_backend("TESSERACT").is_ok());
        assert!(validate_ocr_backend("PADDLEOCR").is_ok());
        assert!(validate_ocr_backend("SCEPTRE").is_ok());
    }

    #[test]
    fn test_validate_ocr_backend_rejects_unknown_backend() {
        let result = validate_ocr_backend("unsupported-ocr");
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("Invalid OCR backend"));
    }

    #[test]
    fn test_validate_ocr_backend_invalid() {
        let result = validate_ocr_backend("invalid_backend");
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("Invalid OCR backend"));
    }

    #[test]
    fn test_validate_language_code_valid_iso639_1() {
        assert!(validate_language_code("en").is_ok());
        assert!(validate_language_code("de").is_ok());
        assert!(validate_language_code("fr").is_ok());
        assert!(validate_language_code("es").is_ok());
        assert!(validate_language_code("zh").is_ok());
        assert!(validate_language_code("ja").is_ok());
        assert!(validate_language_code("ko").is_ok());
    }

    #[test]
    fn test_validate_language_code_valid_iso639_3() {
        assert!(validate_language_code("eng").is_ok());
        assert!(validate_language_code("deu").is_ok());
        assert!(validate_language_code("fra").is_ok());
        assert!(validate_language_code("spa").is_ok());
        assert!(validate_language_code("zho").is_ok());
        assert!(validate_language_code("jpn").is_ok());
        assert!(validate_language_code("jpn_vert").is_ok());
        assert!(validate_language_code("JPN_VERT").is_ok());
        assert!(validate_language_code("kor").is_ok());
        for language in ["afr", "aze", "bos", "bel", "kaz", "kir", "srp", "tgk"] {
            assert!(
                validate_language_code(language).is_ok(),
                "Sceptre language {language} should pass shared validation"
            );
        }
        for language in ["ch_sim", "rs_latin", "rs-cyrillic", "tel", "kan", "abq", "tjk"] {
            assert!(
                validate_language_code(language).is_ok(),
                "EasyOCR Gen2 language {language} should pass shared validation"
            );
        }
    }

    #[test]
    fn test_validate_language_code_case_insensitive() {
        assert!(validate_language_code("EN").is_ok());
        assert!(validate_language_code("ENG").is_ok());
        assert!(validate_language_code("De").is_ok());
        assert!(validate_language_code("DEU").is_ok());
    }

    #[test]
    fn test_validate_language_code_all_keyword() {
        assert!(validate_language_code("all").is_ok());
        assert!(validate_language_code("ALL").is_ok());
        assert!(validate_language_code("All").is_ok());
        assert!(validate_language_code("*").is_ok());
    }

    #[test]
    fn test_validate_language_code_invalid() {
        let result = validate_language_code("invalid");
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("Invalid language code"));
        assert!(msg.contains("ISO 639"));
    }

    #[test]
    fn test_validate_tesseract_psm_valid() {
        for psm in 0..=13 {
            assert!(validate_tesseract_psm(psm).is_ok(), "PSM {} should be valid", psm);
        }
    }

    #[test]
    fn test_validate_tesseract_psm_invalid() {
        assert!(validate_tesseract_psm(-1).is_err());
        assert!(validate_tesseract_psm(14).is_err());
        assert!(validate_tesseract_psm(100).is_err());
    }

    #[test]
    fn test_validate_tesseract_oem_valid() {
        for oem in 0..=3 {
            assert!(validate_tesseract_oem(oem).is_ok(), "OEM {} should be valid", oem);
        }
    }

    #[test]
    fn test_validate_tesseract_oem_invalid() {
        assert!(validate_tesseract_oem(-1).is_err());
        assert!(validate_tesseract_oem(4).is_err());
        assert!(validate_tesseract_oem(10).is_err());
    }

    #[test]
    fn test_validate_output_format_valid() {
        assert!(validate_output_format("text").is_ok());
        assert!(validate_output_format("markdown").is_ok());
    }

    #[test]
    fn test_validate_output_format_case_insensitive() {
        assert!(validate_output_format("TEXT").is_ok());
        assert!(validate_output_format("Markdown").is_ok());
    }

    #[test]
    fn test_validate_output_format_invalid() {
        let result = validate_output_format("xml");
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("Invalid output format"));
    }

    #[test]
    fn test_validate_confidence_valid() {
        assert!(validate_confidence(0.0).is_ok());
        assert!(validate_confidence(0.5).is_ok());
        assert!(validate_confidence(1.0).is_ok());
        assert!(validate_confidence(0.75).is_ok());
    }

    #[test]
    fn test_validate_confidence_invalid() {
        assert!(validate_confidence(-0.1).is_err());
        assert!(validate_confidence(1.1).is_err());
        assert!(validate_confidence(2.0).is_err());
    }

    #[test]
    fn test_validate_dpi_valid() {
        assert!(validate_dpi(72).is_ok());
        assert!(validate_dpi(96).is_ok());
        assert!(validate_dpi(300).is_ok());
        assert!(validate_dpi(600).is_ok());
        assert!(validate_dpi(1).is_ok());
    }

    #[test]
    fn test_validate_dpi_invalid() {
        assert!(validate_dpi(0).is_err());
        assert!(validate_dpi(-1).is_err());
        assert!(validate_dpi(2401).is_err());
    }

    #[test]
    fn test_validate_chunking_params_valid() {
        assert!(validate_chunking_params(1000, 200).is_ok());
        assert!(validate_chunking_params(500, 50).is_ok());
        assert!(validate_chunking_params(1, 0).is_ok());
    }

    #[test]
    fn test_validate_chunking_params_zero_chars() {
        let result = validate_chunking_params(0, 100);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("max_chars"));
    }

    #[test]
    fn test_validate_chunking_params_overlap_too_large() {
        let result = validate_chunking_params(100, 100);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("overlap"));

        let result = validate_chunking_params(100, 150);
        assert!(result.is_err());
    }

    #[test]
    fn test_error_messages_are_helpful() {
        let err = validate_binarization_method("bad").unwrap_err().to_string();
        assert!(err.contains("otsu"));
        assert!(err.contains("adaptive"));
        assert!(err.contains("sauvola"));

        let err = validate_token_reduction_level("bad").unwrap_err().to_string();
        assert!(err.contains("off"));
        assert!(err.contains("moderate"));

        let err = validate_language_code("bad").unwrap_err().to_string();
        assert!(err.contains("ISO 639"));
        assert!(err.contains("en"));
    }

    #[test]
    fn test_validate_port_valid() {
        assert!(validate_port(1).is_ok());
        assert!(validate_port(80).is_ok());
        assert!(validate_port(443).is_ok());
        assert!(validate_port(8000).is_ok());
        assert!(validate_port(65535).is_ok());
    }

    #[test]
    fn test_validate_port_invalid() {
        let result = validate_port(0);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("Port must be 1-65535"));
        assert!(msg.contains("0"));
    }

    #[test]
    fn test_validate_host_ipv4() {
        assert!(validate_host("127.0.0.1").is_ok());
        assert!(validate_host("0.0.0.0").is_ok());
        assert!(validate_host("192.168.1.1").is_ok());
        assert!(validate_host("10.0.0.1").is_ok());
        assert!(validate_host("255.255.255.255").is_ok());
    }

    #[test]
    fn test_validate_host_ipv6() {
        assert!(validate_host("::1").is_ok());
        assert!(validate_host("::").is_ok());
        assert!(validate_host("2001:db8::1").is_ok());
        assert!(validate_host("fe80::1").is_ok());
    }

    #[test]
    fn test_validate_host_hostname() {
        assert!(validate_host("localhost").is_ok());
        assert!(validate_host("example.com").is_ok());
        assert!(validate_host("sub.example.com").is_ok());
        assert!(validate_host("api-server").is_ok());
        assert!(validate_host("app123").is_ok());
    }

    #[test]
    fn test_validate_host_invalid() {
        let result = validate_host("");
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("Invalid host"));

        let result = validate_host("not a valid host");
        assert!(result.is_err());

        let result = validate_host("256.256.256.256");
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_cors_origin_https() {
        assert!(validate_cors_origin("https://example.com").is_ok());
        assert!(validate_cors_origin("https://localhost:3000").is_ok());
        assert!(validate_cors_origin("https://sub.example.com").is_ok());
        assert!(validate_cors_origin("https://192.168.1.1").is_ok());
        assert!(validate_cors_origin("https://example.com/path").is_ok());
    }

    #[test]
    fn test_validate_cors_origin_http() {
        assert!(validate_cors_origin("http://example.com").is_ok());
        assert!(validate_cors_origin("http://localhost:3000").is_ok());
        assert!(validate_cors_origin("http://127.0.0.1:8000").is_ok());
    }

    #[test]
    fn test_validate_cors_origin_wildcard() {
        assert!(validate_cors_origin("*").is_ok());
    }

    #[test]
    fn test_validate_cors_origin_invalid() {
        let result = validate_cors_origin("not-a-url");
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("Invalid CORS origin"));

        let result = validate_cors_origin("ftp://example.com");
        assert!(result.is_err());

        let result = validate_cors_origin("example.com");
        assert!(result.is_err());

        let result = validate_cors_origin("http://");
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_upload_size_valid() {
        assert!(validate_upload_size(1).is_ok());
        assert!(validate_upload_size(1024).is_ok());
        assert!(validate_upload_size(1_000_000).is_ok());
        assert!(validate_upload_size(1_000_000_000).is_ok());
        assert!(validate_upload_size(usize::MAX).is_ok());
    }

    #[test]
    fn test_validate_upload_size_invalid() {
        let result = validate_upload_size(0);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("Upload size must be greater than 0"));
        assert!(msg.contains("0"));
    }
}
