//! DeepSeek-OCR backend plugin for the Xberg OCR pipeline.
//!
//! This module wraps the candle-based DeepSeek-OCR engine in the `OcrBackend`
//! trait, making it available to the extraction pipeline.
//!
//! # Engine pool design
//!
//! The pool key is `(DevicePreference, DType)` with a cached instance per unique
//! device+dtype pair. All calls to `process_image` share the same engine instance
//! for efficiency, avoiding redundant weight loading.

use async_trait::async_trait;
use std::borrow::Cow;
use std::path::Path;
use std::sync::{Arc, LazyLock};

use ahash::AHashMap;
use parking_lot::RwLock;

use crate::Result;
use crate::core::config::OcrConfig;
use crate::plugins::{OcrBackend, OcrBackendType, Plugin};
use crate::types::ExtractedDocument;
use xberg_candle_ocr::DType;
use xberg_candle_ocr::DevicePreference;
use xberg_candle_ocr::models::DeepseekOCREngine;

/// Pool key: device preference + data type pair.
type EnginePoolKey = (DevicePreference, DType);

/// Pooled engine value: shared reference with interior mutability for the engine.
type PooledEngine = Arc<parking_lot::Mutex<DeepseekOCREngine>>;

/// Process-wide engine pool keyed by `(DevicePreference, DType)`.
///
/// A single engine instance handles image inference, so the pool key does not
/// include task or other per-call variations. Two callers requesting the same
/// device+dtype will share one engine and avoid loading weights twice.
#[allow(clippy::type_complexity)]
static ENGINE_POOL: LazyLock<RwLock<AHashMap<EnginePoolKey, PooledEngine>>> =
    LazyLock::new(|| RwLock::new(AHashMap::new()));

/// Return a cached engine for `(preference, dtype)`, initialising one on first use.
///
/// Uses a read → miss → write → double-check pattern so two racing callers do
/// not both pay the initialisation cost. The returned engine is wrapped in a
/// parking_lot Mutex for interior mutability during inference.
fn get_or_init_engine(
    preference: DevicePreference,
    dtype: DType,
    model_path: &str,
    version: usize,
) -> crate::Result<PooledEngine> {
    let key = (preference, dtype);

    {
        let pool = ENGINE_POOL.read();
        if let Some(engine) = pool.get(&key) {
            return Ok(Arc::clone(engine));
        }
    }

    let device = preference.select().map_err(|e| crate::XbergError::Ocr {
        message: format!("Failed to select compute device: {e}"),
        source: Some(Box::new(e)),
    })?;

    tracing::info!(
        preference = ?preference,
        ?dtype,
        model_path = %model_path,
        "Initialising DeepSeek-OCR engine (cold start)"
    );

    let new_engine =
        DeepseekOCREngine::init(model_path, device, dtype, version).map_err(|e| crate::XbergError::Ocr {
            message: format!("DeepSeek-OCR engine initialisation failed: {e}"),
            source: Some(Box::new(e)),
        })?;
    let new_engine = Arc::new(parking_lot::Mutex::new(new_engine));

    let mut pool = ENGINE_POOL.write();
    if let Some(existing) = pool.get(&key) {
        return Ok(Arc::clone(existing));
    }
    pool.insert(key, Arc::clone(&new_engine));
    Ok(new_engine)
}

/// DeepSeek-OCR backend using candle transformers.
///
/// A vision-language model combining SAM vision encoder, ViT/Qwen2 vision
/// transformer, CLIP projection, and language decoder for multimodal OCR.
///
/// # Configuration
///
/// DeepSeek-OCR accepts backend options for device, model path, and version:
/// ```json
/// {
///   "device": "auto",
///   "model_path": "/path/to/deepseek-ocr-model",
///   "version": 2
/// }
/// ```
///
/// - `device` (string): `"auto"` (default), `"cpu"`, `"cuda"`, `"metal"`
/// - `model_path` (string): path to the local model directory (required)
/// - `version` (integer): model version (default: `2`)
#[cfg_attr(alef, alef(skip))]
pub struct DeepseekOcrBackend {
    dtype: DType,
}

impl DeepseekOcrBackend {
    /// Create a new DeepSeek-OCR backend.
    ///
    /// The data type defaults to `F32`. Use [`DeepseekOcrBackend::with_dtype`] to override.
    pub fn new() -> Self {
        Self { dtype: DType::F32 }
    }

    /// Override the floating-point precision used by the candle engine.
    pub fn with_dtype(mut self, dtype: DType) -> Self {
        self.dtype = dtype;
        self
    }

    /// Parse backend options to extract DeepSeek-OCR-specific configuration.
    ///
    /// Device selection is delegated to [`crate::candle_ocr::resolve_device_preference`]
    /// so the central `AccelerationConfig` is honoured.
    ///
    /// Returns `(model_path, device_preference, version)`.
    fn parse_options(config: &OcrConfig) -> (Option<String>, DevicePreference, usize) {
        let mut model_path: Option<String> = None;
        let mut version: usize = 2;

        if let Some(opts) = &config.backend_options {
            if let Some(p) = opts.get("model_path").and_then(|v| v.as_str()) {
                model_path = Some(p.to_string());
            }
            if let Some(v) = opts.get("version").and_then(|v| v.as_u64()) {
                version = v as usize;
            }
        }

        let device = super::resolve_device_preference(config);
        (model_path, device, version)
    }
}

impl Default for DeepseekOcrBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl Plugin for DeepseekOcrBackend {
    fn name(&self) -> &str {
        "candle-deepseek-ocr"
    }

    fn version(&self) -> String {
        "0.1.0".to_string()
    }

    fn initialize(&self) -> Result<()> {
        tracing::debug!("Initializing DeepSeek-OCR backend");
        Ok(())
    }

    fn shutdown(&self) -> Result<()> {
        Ok(())
    }
}

#[async_trait]
impl OcrBackend for DeepseekOcrBackend {
    /// Process an image using the DeepSeek-OCR engine.
    ///
    /// # Errors
    ///
    /// Returns an error if the image is empty, model_path is not provided,
    /// the model fails to initialize, or inference fails.
    async fn process_image(&self, image_bytes: &[u8], config: &OcrConfig) -> Result<ExtractedDocument> {
        if image_bytes.is_empty() {
            return Err(crate::XbergError::Validation {
                message: "Empty image data provided to DeepSeek-OCR".to_string(),
                source: None,
            });
        }

        let (model_path, device, version) = Self::parse_options(config);

        let model_path = model_path.ok_or_else(|| crate::XbergError::Validation {
            message: "DeepSeek-OCR requires `model_path` in backend_options".to_string(),
            source: None,
        })?;

        let image_bytes_owned = image_bytes.to_vec();
        let dtype = self.dtype;

        let content = tokio::task::spawn_blocking(move || {
            let engine = get_or_init_engine(device, dtype, &model_path, version)?;
            let mut engine_guard = engine.lock();
            let output = engine_guard
                .process_image(&image_bytes_owned, None)
                .map_err(|e| crate::XbergError::Ocr {
                    message: format!("DeepSeek-OCR inference failed: {e}"),
                    source: Some(Box::new(e)),
                })?;
            Ok::<String, crate::XbergError>(output)
        })
        .await
        .map_err(|e| crate::XbergError::Ocr {
            message: format!("DeepSeek-OCR task execution failed: {e}"),
            source: None,
        })??;

        Ok(super::ocr_result::build_ocr_document(
            content,
            Vec::new(),
            Cow::Borrowed("text/markdown"),
            image_bytes,
            config,
        ))
    }

    /// Process an image file using the DeepSeek-OCR engine.
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be read or if inference fails.
    async fn process_image_file(&self, path: &Path, config: &OcrConfig) -> Result<ExtractedDocument> {
        let bytes = crate::core::io::read_file_async(path).await?;
        self.process_image(&bytes, config).await
    }

    fn supports_language(&self, _lang: &str) -> bool {
        true
    }

    fn supported_languages(&self) -> Vec<String> {
        vec![
            "eng", "en", "zho", "zh", "jpn", "ja", "kor", "ko", "fra", "fr", "deu", "de", "spa", "es", "ita", "it",
            "por", "pt", "rus", "ru", "ara", "ar", "hin", "hi", "tha", "th", "vie", "vi",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect()
    }

    fn backend_type(&self) -> OcrBackendType {
        OcrBackendType::Candle
    }

    fn emits_structured_markdown(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deepseek_ocr_backend_creation() {
        let backend = DeepseekOcrBackend::new();
        assert_eq!(backend.name(), "candle-deepseek-ocr");
        assert_eq!(backend.backend_type(), OcrBackendType::Candle);
    }

    #[test]
    fn test_deepseek_ocr_emits_structured_markdown() {
        let backend = DeepseekOcrBackend::new();
        assert!(backend.emits_structured_markdown());
    }

    #[test]
    fn test_deepseek_ocr_language_support() {
        let backend = DeepseekOcrBackend::new();
        assert!(backend.supports_language("eng"));
        assert!(backend.supports_language("zho"));
        assert!(backend.supports_language("jpn"));
        assert!(backend.supports_language("unknown"));
    }

    #[test]
    fn test_deepseek_ocr_supported_languages() {
        let backend = DeepseekOcrBackend::new();
        let langs = backend.supported_languages();
        assert!(langs.contains(&"eng".to_string()));
        assert!(langs.contains(&"zho".to_string()));
        assert!(langs.contains(&"fra".to_string()));
    }

    #[test]
    fn test_parse_options_defaults() {
        let config = OcrConfig::default();
        let (model_path, device, version) = DeepseekOcrBackend::parse_options(&config);
        assert!(model_path.is_none());
        assert_eq!(device, DevicePreference::Auto);
        assert_eq!(version, 2);
    }

    #[test]
    fn test_parse_options_model_path() {
        let config = OcrConfig {
            backend_options: Some(serde_json::json!({"model_path": "/models/deepseek"})),
            ..Default::default()
        };
        let (model_path, _device, _version) = DeepseekOcrBackend::parse_options(&config);
        assert_eq!(model_path.as_deref(), Some("/models/deepseek"));
    }

    #[test]
    fn test_parse_options_custom_device() {
        let config = OcrConfig {
            backend_options: Some(serde_json::json!({"device": "cpu"})),
            ..Default::default()
        };
        let (_model_path, device, _version) = DeepseekOcrBackend::parse_options(&config);
        assert_eq!(device, DevicePreference::Cpu);
    }

    #[test]
    fn test_parse_options_version() {
        let config = OcrConfig {
            backend_options: Some(serde_json::json!({"version": 3})),
            ..Default::default()
        };
        let (_model_path, _device, version) = DeepseekOcrBackend::parse_options(&config);
        assert_eq!(version, 3);
    }

    #[test]
    fn test_initialize_and_shutdown() {
        let backend = DeepseekOcrBackend::new();
        assert!(backend.initialize().is_ok());
        assert!(backend.shutdown().is_ok());
    }
}
