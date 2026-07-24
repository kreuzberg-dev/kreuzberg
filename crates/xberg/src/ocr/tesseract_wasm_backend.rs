//! WASM-compatible Tesseract OCR backend.
//!
//! Drives the Tesseract+Leptonica WASI build (provided by `xberg-tesseract`'s
//! `build-tesseract-wasm` feature) via in-memory tessdata, so OCR works on
//! `wasm32-unknown-unknown` with no filesystem and no JavaScript dependencies.
//!
//! Tessdata bytes can come from two sources, in priority order:
//! 1. `OcrConfig::tessdata_bytes` — caller-supplied per-language map.
//! 2. The `bundle-tessdata-eng` feature on `xberg-tesseract`, which embeds
//!    the English `eng.traineddata` (~4 MB, tessdata_fast) into the WASM
//!    binary at compile time.
//!
//! Without either, this backend returns a `MissingDependency` error explaining
//! how to provide tessdata.

use crate::Result;
use crate::core::config::OcrConfig;
use crate::plugins::{OcrBackend, OcrBackendType, Plugin};
use crate::types::{ExtractedDocument, FormatMetadata, Metadata, OcrMetadata};
use async_trait::async_trait;
use std::borrow::Cow;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;
use xberg_tesseract::{Pix, TessMonitor, TessPageSegMode, TesseractAPI};

/// Default OCR engine mode: LSTM only (mode 1). Matches the `OEM_LSTM_ONLY`
/// constant from Tesseract's `tesseract/publictypes.h`. LSTM is the only
/// recognition engine compiled into our WASI Tesseract build.
const OEM_LSTM_ONLY: i32 = 1;

/// Bounded default page segmentation mode used when `OcrConfig::tesseract_config`
/// carries no explicit (or an out-of-range) PSM.
///
/// `PSM_AUTO` (3) — Tesseract's own library default — is known to hang or
/// abort inside the WASI-compiled Tesseract build (issue #855: "Tesseract
/// PSM_AUTO hangs 60-90s in WASM build"), surfacing to callers as an
/// uncatchable wasm `unreachable` trap. `PSM_SINGLE_BLOCK` treats the whole
/// image as one block of text, which is safe in the WASM build and matches
/// `TesseractConfig::default()`'s wasm32-specific `psm: 6`.
const DEFAULT_WASM_PSM: TessPageSegMode = TessPageSegMode::PSM_SINGLE_BLOCK;

/// Recognition deadline, in milliseconds, enforced via `TessMonitor`.
///
/// Bounds worst-case recognition time for a pathological image so a stuck
/// recognition run fails gracefully instead of hanging indefinitely. Kept
/// well under typical caller-side timeouts (e.g. the 30s WASM smoke-test
/// limit that exposed issue #855).
const RECOGNITION_DEADLINE_MS: i32 = 15_000;

/// WASM-compatible Tesseract OCR backend.
#[cfg_attr(alef, alef(skip))]
pub struct TesseractWasmBackend {
    /// Process-local tessdata cache, keyed by language code.
    tessdata_cache: Mutex<HashMap<String, Vec<u8>>>,
}

impl TesseractWasmBackend {
    /// Create a new Tesseract WASM backend.
    pub(crate) fn new() -> Result<Self> {
        Ok(Self {
            tessdata_cache: Mutex::new(HashMap::new()),
        })
    }

    /// Resolve tessdata bytes for a language, consulting the cache, the
    /// supplied OcrConfig, and the optional bundled-eng compile-time blob.
    fn resolve_tessdata(&self, language: &str, config: &OcrConfig) -> Result<Vec<u8>> {
        if let Ok(cache) = self.tessdata_cache.lock()
            && let Some(cached) = cache.get(language)
        {
            return Ok(cached.clone());
        }

        if let Some(ref user_supplied) = config.tessdata_bytes
            && let Some(bytes) = user_supplied.get(language)
        {
            self.cache_tessdata(language, bytes.clone());
            return Ok(bytes.clone());
        }

        if language == "eng"
            && let Some(bundled) = bundled_eng_traineddata()
        {
            self.cache_tessdata(language, bundled.to_vec());
            return Ok(bundled.to_vec());
        }

        Err(crate::XbergError::MissingDependency(format!(
            "Tesseract tessdata for language '{language}' not available on WASM. \
             Provide bytes via OcrConfig::tessdata_bytes, or build with the \
             'bundle-tessdata-eng' feature for English."
        )))
    }

    fn cache_tessdata(&self, language: &str, bytes: Vec<u8>) {
        if let Ok(mut cache) = self.tessdata_cache.lock() {
            cache.insert(language.to_string(), bytes);
        }
    }
}

impl Plugin for TesseractWasmBackend {
    fn name(&self) -> &str {
        "tesseract"
    }

    fn version(&self) -> String {
        TesseractAPI::version()
    }

    fn initialize(&self) -> Result<()> {
        Ok(())
    }

    fn shutdown(&self) -> Result<()> {
        Ok(())
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl OcrBackend for TesseractWasmBackend {
    async fn process_image(&self, image_bytes: &[u8], config: &OcrConfig) -> Result<ExtractedDocument> {
        if image_bytes.is_empty() {
            return Err(crate::XbergError::Validation {
                message: "OCR input image is empty".to_string(),
                source: None,
            });
        }

        let languages = config.effective_languages();
        let language = languages[0].clone();
        if languages.len() > 1 {
            tracing::warn!(
                requested = ?languages,
                used = %language,
                "WASM Tesseract backend recognizes a single language per call; using the primary language"
            );
        }
        let tessdata = self.resolve_tessdata(&language, config)?;

        let img = image::load_from_memory(image_bytes).map_err(|e| crate::XbergError::Ocr {
            message: format!("Failed to decode image for OCR: {e}"),
            source: Some(Box::new(e)),
        })?;
        let rgb = img.to_rgb8();
        let (width, height) = rgb.dimensions();
        let pix = Pix::from_raw_rgb(rgb.as_raw(), width, height).map_err(|e| crate::XbergError::Ocr {
            message: format!("Failed to create Leptonica Pix from image: {e}"),
            source: Some(Box::new(e)),
        })?;

        let api = TesseractAPI::new().map_err(|e| crate::XbergError::Ocr {
            message: format!("Failed to create Tesseract API handle: {e}"),
            source: Some(Box::new(e)),
        })?;

        api.init_5(&tessdata, tessdata.len() as i32, &language, OEM_LSTM_ONLY, &[])
            .map_err(|e| crate::XbergError::Ocr {
                message: format!("Failed to init Tesseract with bundled tessdata: {e}"),
                source: Some(Box::new(e)),
            })?;

        let psm_mode = resolve_psm(config);
        api.set_page_seg_mode(psm_mode).map_err(|e| crate::XbergError::Ocr {
            message: format!("Failed to set Tesseract page segmentation mode: {e}"),
            source: Some(Box::new(e)),
        })?;

        api.set_image_2(pix.as_ptr()).map_err(|e| crate::XbergError::Ocr {
            message: format!("Failed to set image on Tesseract API: {e}"),
            source: Some(Box::new(e)),
        })?;

        let monitor = TessMonitor::new();
        monitor
            .set_deadline(RECOGNITION_DEADLINE_MS)
            .map_err(|e| crate::XbergError::Ocr {
                message: format!("Failed to configure Tesseract recognition deadline: {e}"),
                source: Some(Box::new(e)),
            })?;
        api.recognize_with_monitor(&monitor)
            .map_err(|e| crate::XbergError::Ocr {
                message: format!("Tesseract recognition failed or exceeded its deadline: {e}"),
                source: Some(Box::new(e)),
            })?;
        let text = api.get_utf8_text().map_err(|e| crate::XbergError::Ocr {
            message: format!("Failed to read Tesseract text output: {e}"),
            source: Some(Box::new(e)),
        })?;

        let metadata = Metadata {
            format: Some(FormatMetadata::Ocr(OcrMetadata {
                language: language.clone(),
                psm: psm_mode as i32,
                output_format: "text".to_string(),
                table_count: 0,
                table_rows: None,
                table_cols: None,
            })),
            ..Default::default()
        };

        Ok(ExtractedDocument {
            content: text,
            mime_type: Cow::Borrowed("text/plain"),
            metadata,
            ..Default::default()
        })
    }

    async fn process_image_file(&self, path: &Path, config: &OcrConfig) -> Result<ExtractedDocument> {
        let bytes = std::fs::read(path).map_err(crate::XbergError::from)?;
        self.process_image(&bytes, config).await
    }

    fn supports_language(&self, _lang: &str) -> bool {
        true
    }

    fn backend_type(&self) -> OcrBackendType {
        OcrBackendType::Tesseract
    }
}

/// Returns the compile-time-bundled English tessdata when the
/// `xberg-tesseract/bundle-tessdata-eng` feature is on, otherwise `None`.
fn bundled_eng_traineddata() -> Option<&'static [u8]> {
    xberg_tesseract::bundled_eng_traineddata()
}

/// Resolves the page segmentation mode to use for a recognition call.
///
/// Respects `config.tesseract_config.psm` when it is present and maps to a
/// valid `TessPageSegMode`. Falls back to [`DEFAULT_WASM_PSM`] — never to
/// Tesseract's own `PSM_AUTO` default — when the config is unset or carries
/// an out-of-range value, so callers can never end up hitting the PSM_AUTO
/// hang described in issue #855 by omission.
fn resolve_psm(config: &OcrConfig) -> TessPageSegMode {
    config
        .tesseract_config
        .as_ref()
        .and_then(|c| TessPageSegMode::try_from_int(c.psm as i32))
        .unwrap_or(DEFAULT_WASM_PSM)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// This is a native-compilable test of the PSM-selection logic only.
    /// `TesseractWasmBackend::process_image` itself drives the WASI-compiled
    /// Tesseract engine, which requires a wasm32 build with the
    /// `build-tesseract-wasm` toolchain and is not exercised by `cargo test`
    /// on this target; the deadline/monitor wiring around `recognize()` is
    /// therefore not covered by an automated test here.
    #[test]
    fn should_use_default_wasm_psm_when_config_has_no_tesseract_config() {
        let config = OcrConfig {
            tesseract_config: None,
            ..Default::default()
        };

        assert_eq!(resolve_psm(&config), DEFAULT_WASM_PSM);
        assert_eq!(DEFAULT_WASM_PSM, TessPageSegMode::PSM_SINGLE_BLOCK);
    }

    #[test]
    fn should_never_resolve_to_psm_auto_when_config_has_no_tesseract_config() {
        let config = OcrConfig {
            tesseract_config: None,
            ..Default::default()
        };

        assert_ne!(resolve_psm(&config), TessPageSegMode::PSM_AUTO);
    }

    #[test]
    fn should_respect_explicit_psm_from_tesseract_config() {
        let config = OcrConfig {
            tesseract_config: Some(crate::types::TesseractConfig {
                psm: 7, 
                ..Default::default()
            }),
            ..Default::default()
        };

        assert_eq!(resolve_psm(&config), TessPageSegMode::PSM_SINGLE_LINE);
    }

    #[test]
    fn should_respect_explicit_psm_auto_when_caller_opts_in() {
        let config = OcrConfig {
            tesseract_config: Some(crate::types::TesseractConfig {
                psm: 3, 
                ..Default::default()
            }),
            ..Default::default()
        };

        assert_eq!(resolve_psm(&config), TessPageSegMode::PSM_AUTO);
    }

    #[test]
    fn should_fall_back_to_default_wasm_psm_for_out_of_range_psm_value() {
        let config = OcrConfig {
            tesseract_config: Some(crate::types::TesseractConfig {
                psm: 255,
                ..Default::default()
            }),
            ..Default::default()
        };

        assert_eq!(resolve_psm(&config), DEFAULT_WASM_PSM);
    }
}
