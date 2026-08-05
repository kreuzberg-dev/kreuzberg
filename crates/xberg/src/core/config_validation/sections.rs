//! Per-section validation functions.
//!
//! This module contains validation functions for individual configuration sections
//! and their specific parameters. Each function validates a specific aspect of
//! the configuration and returns detailed error messages when validation fails.

use crate::{Result, XbergError};

/// Valid binarization methods for image preprocessing.
#[cfg(test)]
const VALID_BINARIZATION_METHODS: &[&str] = &["otsu", "adaptive", "sauvola"];

/// Valid token reduction levels.
const VALID_TOKEN_REDUCTION_LEVELS: &[&str] = &["off", "light", "moderate", "aggressive", "maximum"];

/// Valid OCR backends.
const VALID_OCR_BACKENDS: &[&str] = &["tesseract", "paddleocr", "paddle-ocr", "sceptre", "vlm"];

/// Common ISO 639-1 language codes (extended list).
/// Covers most major languages and variants used in document processing.
const VALID_LANGUAGE_CODES: &[&str] = &[
    "en",
    "de",
    "fr",
    "es",
    "it",
    "pt",
    "nl",
    "pl",
    "ru",
    "zh",
    "ja",
    "ko",
    "bg",
    "cs",
    "da",
    "el",
    "et",
    "fi",
    "hu",
    "lt",
    "lv",
    "ro",
    "sk",
    "sl",
    "sv",
    "uk",
    "ar",
    "hi",
    "th",
    "tr",
    "vi",
    "eng",
    "deu",
    "fra",
    "spa",
    "ita",
    "por",
    "nld",
    "pol",
    "rus",
    "zho",
    "jpn",
    "jpn_vert",
    "kor",
    "ces",
    "dan",
    "ell",
    "est",
    "fin",
    "hun",
    "lit",
    "lav",
    "ron",
    "slk",
    "slv",
    "swe",
    "tur",
    "ch",
    "chinese_cht",
    "latin",
    "cyrillic",
    "english",
    "japanese",
    "korean",
    "telugu",
    "kannada",
    "chinese-simplified",
    "simplified-chinese",
    "devanagari",
    "arabic",
    // ISO codes and established aliases accepted by the Sceptre backend that are
    // not already covered above. Keep this feature-independent for config parsing. ~keep
    "af",
    "afr",
    "az",
    "aze",
    "bel",
    "be",
    "bul",
    "bs",
    "bos",
    "ca",
    "cat",
    "cy",
    "cym",
    "cze",
    "wel",
    "ger",
    "fre",
    "ga",
    "fil",
    "gle",
    "hr",
    "hrv",
    "id",
    "ind",
    "is",
    "isl",
    "ice",
    "kk",
    "kaz",
    "ky",
    "kir",
    "mk",
    "mkd",
    "mac",
    "mn",
    "mon",
    "mi",
    "mri",
    "mao",
    "ms",
    "msa",
    "may",
    "mt",
    "mlt",
    "dut",
    "no",
    "nor",
    "rum",
    "slo",
    "sq",
    "sqi",
    "alb",
    "sr",
    "srp",
    "sw",
    "swa",
    "tl",
    "tg",
    "tgk",
    "ukr",
    "uz",
    "uzb",
    "vie",
    "ku",
    "kur",
    "la",
    "lat",
    "oc",
    "oci",
    "pi",
    "pli",
    "te",
    "tel",
    "kn",
    "kan",
    "abq",
    "ady",
    "kbd",
    "ava",
    "dar",
    "inh",
    "che",
    "lbe",
    "lez",
    "tab",
    "tjk",
    "ch_sim",
    "ch-sim",
    "rs_latin",
    "rs-latin",
    "rs_cyrillic",
    "rs-cyrillic",
    "jpn-vert",
    "sr-latn",
    "srp-latn",
    "sr-cyrl",
    "srp-cyrl",
    "zh-cn",
    "zh-hans",
    "chi",
    "chs",
];

/// Valid tesseract PSM (Page Segmentation Mode) values.
#[cfg(test)]
const VALID_TESSERACT_PSM: &[i32] = &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13];

/// Valid tesseract OEM (OCR Engine Mode) values.
#[cfg(test)]
const VALID_TESSERACT_OEM: &[i32] = &[0, 1, 2, 3];

/// Valid output formats for document extraction.
/// Supports plain text, markdown, djot, HTML, and structured (JSON) output formats.
/// Also accepts aliases: "text" for "plain", "md" for "markdown", "json" for "structured".
#[cfg(test)]
const VALID_OUTPUT_FORMATS: &[&str] = &["plain", "text", "markdown", "md", "djot", "html", "structured", "json"];

/// Validate a binarization method string.
///
/// # Arguments
///
/// * `method` - The binarization method to validate (e.g., "otsu", "adaptive", "sauvola")
///
/// # Returns
///
/// `Ok(())` if the method is valid, or a `ValidationError` with details about valid options.
///
/// # Examples
///
/// ```rust
/// use xberg::core::config_validation::validate_binarization_method;
///
/// assert!(validate_binarization_method("otsu").is_ok());
/// assert!(validate_binarization_method("adaptive").is_ok());
/// assert!(validate_binarization_method("invalid").is_err());
/// ```
#[cfg(test)]
pub(crate) fn validate_binarization_method(method: &str) -> Result<()> {
    let method = method.to_lowercase();
    if VALID_BINARIZATION_METHODS.contains(&method.as_str()) {
        Ok(())
    } else {
        Err(XbergError::Validation {
            message: format!(
                "Invalid binarization method '{}'. Valid options are: {}",
                method,
                VALID_BINARIZATION_METHODS.join(", ")
            ),
            source: None,
        })
    }
}

/// Validate a token reduction level string.
///
/// # Arguments
///
/// * `level` - The token reduction level to validate (e.g., "off", "light", "moderate")
///
/// # Returns
///
/// `Ok(())` if the level is valid, or a `ValidationError` with details about valid options.
///
/// # Examples
///
/// ```ignore
/// use xberg::core::config_validation::validate_token_reduction_level;
///
/// assert!(validate_token_reduction_level("off").is_ok());
/// assert!(validate_token_reduction_level("moderate").is_ok());
/// assert!(validate_token_reduction_level("extreme").is_err());
/// ```
pub(crate) fn validate_token_reduction_level(level: &str) -> Result<()> {
    let level = level.to_lowercase();
    if VALID_TOKEN_REDUCTION_LEVELS.contains(&level.as_str()) {
        Ok(())
    } else {
        Err(XbergError::Validation {
            message: format!(
                "Invalid token reduction level '{}'. Valid options are: {}",
                level,
                VALID_TOKEN_REDUCTION_LEVELS.join(", ")
            ),
            source: None,
        })
    }
}

/// Validate an OCR backend string.
///
/// # Arguments
///
/// * `backend` - The OCR backend to validate (e.g., "tesseract", "paddleocr")
///
/// # Returns
///
/// `Ok(())` if the backend is valid, or a `ValidationError` with details about valid options.
///
/// # Examples
///
/// ```ignore
/// use xberg::core::config_validation::validate_ocr_backend;
///
/// assert!(validate_ocr_backend("tesseract").is_ok());
/// assert!(validate_ocr_backend("paddleocr").is_ok());
/// assert!(validate_ocr_backend("invalid").is_err());
/// ```
pub(crate) fn validate_ocr_backend(backend: &str) -> Result<()> {
    let backend = backend.to_lowercase();
    if VALID_OCR_BACKENDS.contains(&backend.as_str()) {
        Ok(())
    } else {
        Err(XbergError::Validation {
            message: format!(
                "Invalid OCR backend '{}'. Valid options are: {}",
                backend,
                VALID_OCR_BACKENDS.join(", ")
            ),
            source: None,
        })
    }
}

/// Validate a language code (ISO 639-1 or 639-3 format).
///
/// Accepts both 2-letter ISO 639-1 codes (e.g., "en", "de") and
/// 3-letter ISO 639-3 codes (e.g., "eng", "deu") for broader compatibility.
///
/// # Arguments
///
/// * `code` - The language code to validate
///
/// # Returns
///
/// `Ok(())` if the code is valid, or a `ValidationError` indicating an invalid language code.
///
/// # Examples
///
/// ```ignore
/// use xberg::core::config_validation::validate_language_code;
///
/// assert!(validate_language_code("en").is_ok());
/// assert!(validate_language_code("eng").is_ok());
/// assert!(validate_language_code("de").is_ok());
/// assert!(validate_language_code("deu").is_ok());
/// assert!(validate_language_code("invalid").is_err());
/// ```
#[cfg_attr(alef, alef(skip))]
pub(crate) fn validate_language_code(code: &str) -> Result<()> {
    let code_lower = code.to_lowercase();
    let code_normalized = code_lower.replace('_', "-");

    if code_lower == "all" || code_lower == "*" {
        return Ok(());
    }

    if VALID_LANGUAGE_CODES.contains(&code_lower.as_str()) || VALID_LANGUAGE_CODES.contains(&code_normalized.as_str()) {
        return Ok(());
    }

    Err(XbergError::Validation {
        message: format!(
            "Invalid language code '{}'. Use ISO 639-1 (2-letter, e.g., 'en', 'de') \
             or ISO 639-3 (3-letter, e.g., 'eng', 'deu') codes. \
             Common codes: en, de, fr, es, it, pt, nl, pl, ru, zh, ja, ko, ar, hi, th.",
            code
        ),
        source: None,
    })
}

/// Validate a tesseract Page Segmentation Mode (PSM).
///
/// # Arguments
///
/// * `psm` - The PSM value to validate (0-13)
///
/// # Returns
///
/// `Ok(())` if the PSM is valid, or a `ValidationError` with details about valid ranges.
///
/// # Examples
///
/// ```rust
/// use xberg::core::config_validation::validate_tesseract_psm;
///
/// assert!(validate_tesseract_psm(3).is_ok());  // Fully automatic
/// assert!(validate_tesseract_psm(6).is_ok());  // Single block of text
/// assert!(validate_tesseract_psm(14).is_err()); // Out of range
/// ```
#[cfg(test)]
pub(crate) fn validate_tesseract_psm(psm: i32) -> Result<()> {
    if VALID_TESSERACT_PSM.contains(&psm) {
        Ok(())
    } else {
        Err(XbergError::Validation {
            message: format!(
                "Invalid tesseract PSM value '{}'. Valid range is 0-13. \
                 Common values: 3 (auto), 6 (single block), 11 (sparse text).",
                psm
            ),
            source: None,
        })
    }
}

/// Validate a tesseract OCR Engine Mode (OEM).
///
/// # Arguments
///
/// * `oem` - The OEM value to validate (0-3)
///
/// # Returns
///
/// `Ok(())` if the OEM is valid, or a `ValidationError` with details about valid options.
///
/// # Examples
///
/// ```rust
/// use xberg::core::config_validation::validate_tesseract_oem;
///
/// assert!(validate_tesseract_oem(1).is_ok());  // Neural nets (LSTM)
/// assert!(validate_tesseract_oem(2).is_ok());  // Legacy + LSTM
/// assert!(validate_tesseract_oem(4).is_err()); // Out of range
/// ```
#[cfg(test)]
pub(crate) fn validate_tesseract_oem(oem: i32) -> Result<()> {
    if VALID_TESSERACT_OEM.contains(&oem) {
        Ok(())
    } else {
        Err(XbergError::Validation {
            message: format!(
                "Invalid tesseract OEM value '{}'. Valid range is 0-3. \
                 0=Legacy, 1=LSTM, 2=Legacy+LSTM, 3=Default",
                oem
            ),
            source: None,
        })
    }
}

/// Validate a document extraction output format.
///
/// Accepts the following formats and aliases:
/// - "plain" or "text" for plain text output
/// - "markdown" or "md" for Markdown output
/// - "djot" for Djot markup format
/// - "html" for HTML output
///
/// # Arguments
///
/// * `format` - The output format to validate
///
/// # Returns
///
/// `Ok(())` if the format is valid, or a `ValidationError` with details about valid options.
///
/// # Examples
///
/// ```rust
/// use xberg::core::config_validation::validate_output_format;
///
/// assert!(validate_output_format("text").is_ok());
/// assert!(validate_output_format("plain").is_ok());
/// assert!(validate_output_format("markdown").is_ok());
/// assert!(validate_output_format("md").is_ok());
/// assert!(validate_output_format("djot").is_ok());
/// assert!(validate_output_format("html").is_ok());
/// assert!(validate_output_format("json").is_ok());
/// ```
#[cfg(test)]
pub(crate) fn validate_output_format(format: &str) -> Result<()> {
    let format = format.to_lowercase();
    if VALID_OUTPUT_FORMATS.contains(&format.as_str()) {
        Ok(())
    } else {
        Err(XbergError::Validation {
            message: format!(
                "Invalid output format '{}'. Valid options are: {}",
                format,
                VALID_OUTPUT_FORMATS.join(", ")
            ),
            source: None,
        })
    }
}

/// Validate a confidence threshold value.
///
/// Confidence thresholds should be between 0.0 and 1.0 inclusive.
///
/// # Arguments
///
/// * `confidence` - The confidence threshold to validate
///
/// # Returns
///
/// `Ok(())` if the confidence is valid, or a `ValidationError` with details about valid ranges.
///
/// # Examples
///
/// ```rust
/// use xberg::core::config_validation::validate_confidence;
///
/// assert!(validate_confidence(0.5).is_ok());
/// assert!(validate_confidence(0.0).is_ok());
/// assert!(validate_confidence(1.0).is_ok());
/// assert!(validate_confidence(1.5).is_err());
/// assert!(validate_confidence(-0.1).is_err());
/// ```
#[cfg(test)]
pub(crate) fn validate_confidence(confidence: f64) -> Result<()> {
    if (0.0..=1.0).contains(&confidence) {
        Ok(())
    } else {
        Err(XbergError::Validation {
            message: format!(
                "Invalid confidence threshold '{}'. Must be between 0.0 and 1.0.",
                confidence
            ),
            source: None,
        })
    }
}

/// Validate a DPI (dots per inch) value.
///
/// DPI should be a positive integer, typically 72-600.
///
/// # Arguments
///
/// * `dpi` - The DPI value to validate
///
/// # Returns
///
/// `Ok(())` if the DPI is valid, or a `ValidationError` with details about valid ranges.
///
/// # Examples
///
/// ```rust
/// use xberg::core::config_validation::validate_dpi;
///
/// assert!(validate_dpi(96).is_ok());
/// assert!(validate_dpi(300).is_ok());
/// assert!(validate_dpi(0).is_err());
/// assert!(validate_dpi(-1).is_err());
/// ```
#[cfg(test)]
pub(crate) fn validate_dpi(dpi: i32) -> Result<()> {
    if dpi > 0 && dpi <= 2400 {
        Ok(())
    } else {
        Err(XbergError::Validation {
            message: format!(
                "Invalid DPI value '{}'. Must be a positive integer, typically 72-600.",
                dpi
            ),
            source: None,
        })
    }
}

/// Validate chunk size parameters.
///
/// Checks that max_chars > 0 and max_overlap < max_chars.
///
/// # Arguments
///
/// * `max_chars` - The maximum characters per chunk
/// * `max_overlap` - The maximum overlap between chunks
///
/// # Returns
///
/// `Ok(())` if the parameters are valid, or a `ValidationError` with details about constraints.
///
/// # Examples
///
/// ```ignore
/// use xberg::core::config_validation::validate_chunking_params;
///
/// assert!(validate_chunking_params(1000, 200).is_ok());
/// assert!(validate_chunking_params(500, 50).is_ok());
/// assert!(validate_chunking_params(0, 100).is_err()); // max_chars must be > 0
/// assert!(validate_chunking_params(100, 150).is_err()); // overlap >= max_chars
/// ```
pub(crate) fn validate_chunking_params(max_chars: usize, max_overlap: usize) -> Result<()> {
    if max_chars == 0 {
        return Err(XbergError::Validation {
            message: "max_chars must be greater than 0".to_string(),
            source: None,
        });
    }

    if max_overlap >= max_chars {
        return Err(XbergError::Validation {
            message: format!(
                "max_overlap ({}) must be less than max_chars ({})",
                max_overlap, max_chars
            ),
            source: None,
        });
    }

    Ok(())
}

/// Validate that an [`LlmConfig`](crate::core::config::LlmConfig) has a non-empty model string.
///
/// # Arguments
///
/// * `model` - The model string to validate
///
/// # Returns
///
/// `Ok(())` if the model is non-empty, or a `ValidationError` otherwise.
///
/// # Examples
///
/// ```rust
/// use xberg::core::config_validation::validate_llm_config_model;
///
/// assert!(validate_llm_config_model("openai/gpt-4o").is_ok());
/// assert!(validate_llm_config_model("").is_err());
/// ```
#[cfg(test)]
pub(crate) fn validate_llm_config_model(model: &str) -> Result<()> {
    if model.trim().is_empty() {
        return Err(XbergError::Validation {
            message: "LLM config 'model' must not be empty. Provide a model identifier (e.g., 'openai/gpt-4o')."
                .to_string(),
            source: None,
        });
    }
    Ok(())
}

/// Validate that a VLM OCR backend has the required `vlm_config`.
///
/// When the OCR backend is set to `"vlm"`, the `vlm_config` field must be present
/// to provide the model endpoint configuration, and the model string must be non-empty.
///
/// # Arguments
///
/// * `backend` - The OCR backend name
/// * `vlm_config` - The optional VLM config to validate
///
/// # Returns
///
/// `Ok(())` if the backend is not `"vlm"` or `vlm_config` is present with a valid model,
/// or a `ValidationError` if `"vlm"` backend is used without `vlm_config` or with an empty model.
///
/// # Examples
///
/// ```rust
/// use xberg::core::config_validation::validate_vlm_backend_config;
/// use xberg::core::config::LlmConfig;
///
/// assert!(validate_vlm_backend_config("tesseract", None).is_ok());
/// let config = LlmConfig {
///     model: "openai/gpt-4o".to_string(),
///     api_key: None,
///     base_url: None,
///     timeout_secs: None,
///     max_retries: None,
///     temperature: None,
///     max_tokens: None,
/// };
/// assert!(validate_vlm_backend_config("vlm", Some(&config)).is_ok());
/// assert!(validate_vlm_backend_config("vlm", None).is_err());
/// ```
#[cfg(test)]
pub(crate) fn validate_vlm_backend_config(
    backend: &str,
    vlm_config: Option<&crate::core::config::LlmConfig>,
) -> Result<()> {
    if backend.to_lowercase() == "vlm" {
        match vlm_config {
            None => {
                return Err(XbergError::Validation {
                    message: "OCR backend 'vlm' requires 'vlm_config' to be set with model endpoint configuration."
                        .to_string(),
                    source: None,
                });
            }
            Some(config) => {
                validate_llm_config_model(&config.model)?;
            }
        }
    }
    Ok(())
}
