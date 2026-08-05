//! Byte-fed Sceptre OCR engine for opt-in browser WebAssembly builds.

use std::sync::Arc;

use sceptre::{Backend, ModelRole, ReadOptions, Reader, VerifiedModelProvider};

use crate::sceptre_languages::language_group;
use crate::{Result, XbergError};

/// Reusable, filesystem-free Sceptre reader for a single Gen2 recognizer group.
///
/// Construction verifies and compiles the supplied CRAFT and recognizer models.
/// Browser hosts should create this object inside a Web Worker and reuse it.
#[cfg_attr(alef, alef(skip))]
pub struct SceptreWasmEngine {
    reader: Reader,
}

impl SceptreWasmEngine {
    /// Build and warm a tract reader from checksum-pinned ONNX model bytes.
    pub fn new(
        detector: Arc<[u8]>,
        recognizer: Arc<[u8]>,
        language: &str,
        options: Option<serde_json::Value>,
    ) -> Result<Self> {
        let language = language_group(language).ok_or_else(|| {
            ocr_error(format!(
                "Unsupported Sceptre WebAssembly language `{language}`; select a language covered by an EasyOCR Gen2 model"
            ))
        })?;
        let mut config = match options {
            Some(options) => serde_json::from_value::<sceptre::OcrConfig>(options)
                .map_err(|error| ocr_error(format!("Invalid Sceptre WebAssembly configuration: {error}")))?,
            None => sceptre::OcrConfig::default(),
        };
        config.model.backend = Backend::Tract;
        config.model.languages = vec![language];
        config.concurrency.max_threads = Some(1);

        let descriptors = sceptre::model_descriptors(&config).map_err(map_sceptre_error)?;
        let detector_descriptor = descriptors
            .iter()
            .find(|descriptor| descriptor.role == ModelRole::Detector)
            .cloned()
            .ok_or_else(|| ocr_error("Sceptre did not describe its CRAFT detector model"))?;
        let recognizer_descriptor = descriptors
            .into_iter()
            .find(|descriptor| descriptor.role == ModelRole::Recognizer(language))
            .ok_or_else(|| ocr_error("Sceptre did not describe the selected recognizer model"))?;
        let provider =
            VerifiedModelProvider::new([(detector_descriptor, detector), (recognizer_descriptor, recognizer)])
                .map_err(map_sceptre_error)?;
        let reader = Reader::builder()
            .config(config)
            .model_provider(Arc::new(provider))
            .build_warmed()
            .map_err(map_sceptre_error)?;
        Ok(Self { reader })
    }

    /// Recognize an encoded image while reusing the warmed tract models.
    pub fn recognize(&self, image_bytes: &[u8]) -> Result<sceptre::OcrResult> {
        if image_bytes.is_empty() {
            return Err(XbergError::validation("Cannot run Sceptre OCR on empty image data"));
        }
        let image = sceptre::Image::from_bytes(image_bytes).map_err(map_sceptre_error)?;
        self.reader
            .recognize(&image, &ReadOptions::default())
            .map_err(map_sceptre_error)
    }
}

fn map_sceptre_error(error: sceptre::OcrError) -> XbergError {
    XbergError::Ocr {
        message: "Sceptre OCR operation failed".to_string(),
        source: Some(Box::new(error)),
    }
}

fn ocr_error(message: impl Into<String>) -> XbergError {
    XbergError::Ocr {
        message: message.into(),
        source: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sceptre::Language;

    #[test]
    fn should_map_all_gen2_model_groups() {
        assert_eq!(language_group("english"), Some(Language::English));
        assert_eq!(language_group("deu"), Some(Language::Latin));
        assert_eq!(language_group("ch_sim"), Some(Language::ChineseSimplified));
        assert_eq!(language_group("jpn_vert"), Some(Language::Japanese));
        assert_eq!(language_group("kor"), Some(Language::Korean));
        assert_eq!(language_group("rus"), Some(Language::Cyrillic));
        assert_eq!(language_group("tel"), Some(Language::Telugu));
        assert_eq!(language_group("kan"), Some(Language::Kannada));
    }

    #[test]
    fn should_reject_gen1_model_groups() {
        for group in ["arabic", "bengali", "devanagari", "tamil", "thai", "chinese_tra"] {
            assert!(
                language_group(group).is_none(),
                "{group} must stay outside Gen2 support"
            );
        }
    }
}
