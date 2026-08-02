//! PaddleOCR backend using ONNX Runtime.
//!
//! This module provides a PaddleOCR implementation that uses ONNX Runtime
//! for inference, enabling high-quality OCR without Python dependencies.
//!
//! # Features
//!
//! - PP-OCRv5 model support (server detection, per-family recognition)
//! - Excellent CJK (Chinese, Japanese, Korean) recognition
//! - Pure Rust implementation via `paddle-ocr-rs`
//! - Shared ONNX Runtime with embeddings feature
//!
//! # Model Files
//!
//! PaddleOCR requires three model files:
//! - Detection model (`*_det_*.onnx`)
//! - Classification model (`*_cls_*.onnx`)
//! - Recognition model (`*_rec_*.onnx`)
//!
//! Models are auto-downloaded on first use through the standard Hugging Face
//! cache (`HF_HUB_CACHE`, `HUGGINGFACE_HUB_CACHE`, or `$HF_HOME/hub`).
//!
//! # Example
//!
//! ```rust,ignore
//! use xberg::ocr::paddle::PaddleOcrBackend;
//! use xberg::plugins::OcrBackend;
//! use xberg::OcrConfig;
//!
//! let backend = PaddleOcrBackend::new()?;
//! let config = OcrConfig {
//!     language: "ch".to_string(),
//!     ..Default::default()
//! };
//!
//! let result = backend.process_image(&image_bytes, &config).await?;
//! println!("Extracted: {}", result.content);
//! ```

#[cfg(feature = "paddle-ocr")]
mod backend;
mod config;
pub(crate) mod model_manager;

#[cfg(feature = "paddle-ocr")]
pub use backend::PaddleOcrBackend;
pub use config::{PaddleLanguage, PaddleOcrConfig};
pub use model_manager::{ModelPaths, RecModelPaths, ResolvedRecModel, SharedModelPaths};

#[cfg(feature = "paddle-ocr")]
pub use model_manager::{ModelCacheStats, ModelManager, ModelManifestEntry};

/// Supported languages for PaddleOCR.
///
/// PaddleOCR supports 15+ optimized language models covering 80+ languages
/// via 11 script-family recognition models (all PP-OCRv5).
pub const SUPPORTED_LANGUAGES: &[&str] = &[
    "ch",
    "en",
    "french",
    "german",
    "korean",
    "japan",
    "chinese_cht",
    "latin",
    "cyrillic",
    "thai",
    "greek",
    "arabic",
    "devanagari",
    "tamil",
    "telugu",
];

/// Check if a language code is supported by PaddleOCR.
#[allow(dead_code)]
pub(crate) fn is_language_supported(lang: &str) -> bool {
    SUPPORTED_LANGUAGES.contains(&lang)
}

/// Map a PaddleOCR language code to its script family.
///
/// Script families group languages that share a single recognition model.
/// For example, French, German, and Spanish all use the `latin` rec model.
/// Chinese simplified, traditional, and Japanese share the `chinese` rec model.
///
/// # Script Families (11, all PP-OCRv5)
///
/// | Family | Languages |
/// |---|---|
/// | `english` | English |
/// | `chinese` | Chinese (simplified+traditional), Japanese |
/// | `latin` | French, German, Spanish, Italian, 40+ more |
/// | `korean` | Korean |
/// | `eslav` | Russian, Ukrainian, Belarusian |
/// | `thai` | Thai |
/// | `greek` | Greek |
/// | `arabic` | Arabic, Persian, Urdu |
/// | `devanagari` | Hindi, Marathi, Sanskrit, Nepali |
/// | `tamil` | Tamil |
/// | `telugu` | Telugu |
#[allow(dead_code)]
pub(crate) fn language_to_script_family(paddle_lang: &str) -> &'static str {
    match paddle_lang {
        "en" => "english",
        "ch" | "japan" | "chinese_cht" => "chinese",
        "korean" => "korean",
        "french" | "german" | "latin" => "latin",
        "cyrillic" => "eslav",
        "thai" => "thai",
        "greek" => "greek",
        "arabic" => "arabic",
        "devanagari" => "devanagari",
        "tamil" => "tamil",
        "telugu" => "telugu",
        _ => "english",
    }
}

/// Map Xberg language codes to PaddleOCR language codes.
#[allow(dead_code)]
pub(crate) fn map_language_code(xberg_code: &str) -> Option<&'static str> {
    match xberg_code {
        "ch" | "chi_sim" | "zho" | "zh" | "chinese" => Some("ch"),
        "en" | "eng" | "english" => Some("en"),
        "fr" | "fra" | "french" => Some("french"),
        "de" | "deu" | "german" => Some("german"),
        "ko" | "kor" | "korean" => Some("korean"),
        "ja" | "jpn" | "jpn_vert" | "japanese" | "japan" => Some("japan"),
        "chi_tra" | "zh_tw" | "zh_hant" | "chinese_cht" => Some("chinese_cht"),
        "ru" | "rus" | "russian" | "uk" | "ukr" | "ukrainian" | "be" | "bel" | "belarusian" | "cyrillic" => {
            Some("cyrillic")
        }
        "th" | "tha" | "thai" => Some("thai"),
        "el" | "ell" | "greek" => Some("greek"),
        "ar" | "ara" | "arabic" | "fa" | "fas" | "persian" | "ur" | "urd" | "urdu" => Some("arabic"),
        "hi" | "hin" | "hindi" | "mr" | "mar" | "marathi" | "sa" | "san" | "sanskrit" | "ne" | "nep" | "nepali"
        | "devanagari" => Some("devanagari"),
        "ta" | "tam" | "tamil" => Some("tamil"),
        "te" | "tel" | "telugu" => Some("telugu"),
        "latin" | "es" | "spa" | "spanish" | "it" | "ita" | "italian" | "pt" | "por" | "portuguese" | "nl" | "nld"
        | "dutch" | "pl" | "pol" | "polish" | "sv" | "swe" | "swedish" | "da" | "dan" | "danish" | "no" | "nor"
        | "norwegian" | "fi" | "fin" | "finnish" | "cs" | "ces" | "czech" | "sk" | "slk" | "slovak" | "hr" | "hrv"
        | "croatian" | "hu" | "hun" | "hungarian" | "ro" | "ron" | "romanian" | "tr" | "tur" | "turkish" | "id"
        | "ind" | "indonesian" | "ms" | "msa" | "malay" | "vi" | "vie" | "vietnamese" => Some("latin"),
        _ => None,
    }
}

/// Select the PaddleOCR recognition language for a request and report what the
/// selection cannot cover.
///
/// PaddleOCR loads one recognition model per call. The first requested language
/// selects it, except that an English-first request prefers a later Korean or
/// Japanese model because those models also cover Latin text. Any language the
/// selected model does not cover emits a `ProcessingWarning` instead of being
/// silently dropped (#1346). An unmapped first language falls back to English.
#[cfg(feature = "paddle-ocr")]
pub(crate) fn select_paddle_language(languages: &[String]) -> (&'static str, Vec<crate::types::ProcessingWarning>) {
    use std::borrow::Cow;

    let mut warnings = Vec::new();
    let primary = languages.first().map(String::as_str).unwrap_or("eng");
    let primary_code = map_language_code(primary);
    let paddle_lang = match primary_code {
        Some("en") => languages
            .iter()
            .filter_map(|language| map_language_code(language))
            // Korean and Japanese recognition models also cover Latin text. ~keep
            .find(|code| matches!(*code, "korean" | "japan"))
            .unwrap_or("en"),
        Some(code) => code,
        None => {
            tracing::warn!(
                requested = %primary,
                "paddle-ocr: requested language is not supported; falling back to the 'en' recognition model"
            );
            warnings.push(crate::types::ProcessingWarning {
                source: Cow::Borrowed("paddle-ocr"),
                message: Cow::Owned(format!(
                    "requested language '{primary}' is not supported by paddle-ocr; falling back to the 'en' recognition model"
                )),
            });
            "en"
        }
    };

    let uncovered: Vec<&str> = languages
        .iter()
        .skip(usize::from(primary_code.is_none()))
        .map(String::as_str)
        .filter(|lang| map_language_code(lang).is_none_or(|code| !paddle_model_covers(paddle_lang, code)))
        .collect();

    if !uncovered.is_empty() {
        tracing::warn!(
            selected = %paddle_lang,
            uncovered = %uncovered.join(", "),
            "paddle-ocr: single recognition model per run; requested languages are not covered by the selected model and their text may be dropped"
        );
        warnings.push(crate::types::ProcessingWarning {
            source: Cow::Borrowed("paddle-ocr"),
            message: Cow::Owned(format!(
                "paddle-ocr uses a single recognition model per run; requested languages [{}] are not covered by the selected '{paddle_lang}' model and their text may be dropped",
                uncovered.join(", ")
            )),
        });
    }

    (paddle_lang, warnings)
}

#[cfg(feature = "paddle-ocr")]
fn paddle_model_covers(selected: &str, requested: &str) -> bool {
    language_to_script_family(selected) == language_to_script_family(requested)
        || (matches!(selected, "korean" | "japan") && requested == "en")
}

#[cfg(all(test, feature = "paddle-ocr"))]
mod language_selection_tests {
    use super::select_paddle_language;

    fn langs(codes: &[&str]) -> Vec<String> {
        codes.iter().map(|c| c.to_string()).collect()
    }

    #[test]
    fn test_single_language_no_warnings() {
        let (lang, warnings) = select_paddle_language(&langs(&["rus"]));
        assert_eq!(lang, "cyrillic");
        assert!(warnings.is_empty());
    }

    #[test]
    fn test_same_family_languages_no_warnings() {
        let (lang, warnings) = select_paddle_language(&langs(&["fra", "deu", "spa"]));
        assert_eq!(lang, "french");
        assert!(warnings.is_empty());
    }

    #[test]
    fn test_cross_family_language_warns() {
        let (lang, warnings) = select_paddle_language(&langs(&["eng", "rus"]));
        assert_eq!(lang, "en");
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].message.contains("rus"));
        assert!(warnings[0].message.contains("'en'"));
    }

    #[test]
    fn test_unmapped_primary_falls_back_with_warning() {
        let (lang, warnings) = select_paddle_language(&langs(&["xyz"]));
        assert_eq!(lang, "en");
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].message.contains("xyz"));
    }

    #[test]
    fn test_unmapped_secondary_warns() {
        let (lang, warnings) = select_paddle_language(&langs(&["eng", "xyz"]));
        assert_eq!(lang, "en");
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].message.contains("xyz"));
    }

    #[test]
    fn test_empty_list_defaults_to_english() {
        let (lang, warnings) = select_paddle_language(&[]);
        assert_eq!(lang, "en");
        assert!(warnings.is_empty());
    }

    #[test]
    fn should_map_vertical_japanese_to_japanese_model() {
        let (lang, warnings) = select_paddle_language(&langs(&["jpn_vert"]));
        assert_eq!(lang, "japan");
        assert!(warnings.is_empty());
    }

    #[test]
    fn should_prioritize_korean_model_for_mixed_english_and_korean() {
        let (lang, warnings) = select_paddle_language(&langs(&["eng", "kor"]));
        assert_eq!(lang, "korean");
        assert!(warnings.is_empty());
    }

    #[test]
    fn should_prioritize_japanese_model_for_mixed_english_and_vertical_japanese() {
        let (lang, warnings) = select_paddle_language(&langs(&["eng", "jpn_vert"]));
        assert_eq!(lang, "japan");
        assert!(warnings.is_empty());
    }
}
