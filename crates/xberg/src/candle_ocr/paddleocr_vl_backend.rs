//! PaddleOCR-VL backend plugin for the Xberg OCR pipeline.
//!
//! This module wraps the candle-based PaddleOCR-VL engine in the `OcrBackend`
//! trait, making it available to the extraction pipeline. The engine itself
//! (`xberg_candle_ocr::models::PaddleOcrVlEngine`) implements the PaddleOCR-VL 1.5
//! architecture (SigLIP vision encoder + ERNIE 4.5 text decoder); PaddleOCR-VL 1.6 is
//! a weights-only upgrade that keeps this architecture unchanged, so the same engine
//! loads either checkpoint — only the default `model_id` changed.
//!
//! # Engine pool design
//!
//! The pool key is `(task, DevicePreference)`. Engines are expensive to initialise
//! (~900 MB – 2 GB of safetensors weights depending on the checkpoint). The pool
//! ensures each `(task, device)` combination is loaded at most once per process.
//!
//! `PaddleOcrVlEngine::process_image` takes `&mut self` (the model maintains KV
//! cache state), so the pool stores engines wrapped in `parking_lot::Mutex` for
//! interior mutability.
//!
//! # Weight resolution
//!
//! `backend_options.model_path`, when present, always wins (offline / custom weights).
//! Otherwise the backend auto-downloads `backend_options.model_id` (default
//! `xberg-io/paddleocr-vl-1.6`, a checksum-pinned mirror of
//! `PaddlePaddle/PaddleOCR-VL-1.6`) through [`super::model_stager`].

use async_trait::async_trait;
use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};

use ahash::AHashMap;
use parking_lot::{Mutex, RwLock};

use crate::Result;
use crate::core::config::OcrConfig;
use crate::plugins::{OcrBackend, OcrBackendType, Plugin};
use crate::types::ExtractedDocument;
use xberg_candle_ocr::DType;
use xberg_candle_ocr::DevicePreference;
use xberg_candle_ocr::models::PaddleOcrVlEngine;
use xberg_candle_ocr::models::PaddleOcrVlTask;

/// Engine pool key: `(resolved_model_path, task, device_preference)`.
type PoolKey = (String, PaddleOcrVlTask, DevicePreference);
/// Pooled engine value: mutex-wrapped engine for interior mutability.
type PooledEngine = Arc<Mutex<PaddleOcrVlEngine>>;

/// Process-wide engine pool keyed by model path, task, and device preference.
///
/// `DevicePreference::Auto` keeps its own slot because it resolves to whatever
/// is available at runtime — collapsing it onto a concrete device would be wrong.
///
/// Engines are wrapped in `Mutex` because `PaddleOcrVlEngine::process_image`
/// takes `&mut self` (it manages an internal KV cache).
static ENGINE_POOL: LazyLock<RwLock<AHashMap<PoolKey, PooledEngine>>> = LazyLock::new(|| RwLock::new(AHashMap::new()));

/// Return a cached engine for `(task, preference)`, initialising one on first use.
///
/// Uses a read → miss → write → double-check pattern so that two racing callers
/// do not both pay the initialisation cost.
///
/// # Errors
///
/// Returns [`crate::XbergError::Ocr`] if device selection fails or the
/// engine cannot be initialised from the model directory.
fn get_or_init_engine(
    model_path: &str,
    task: PaddleOcrVlTask,
    preference: DevicePreference,
) -> crate::Result<PooledEngine> {
    let key: PoolKey = (model_path.to_string(), task, preference);

    {
        let pool = ENGINE_POOL.read();
        if let Some(engine) = pool.get(&key) {
            return Ok(Arc::clone(engine));
        }
    }

    let candle_device = preference.select().map_err(|e| crate::XbergError::Ocr {
        message: format!("Failed to select compute device: {e}"),
        source: Some(Box::new(e)),
    })?;

    tracing::info!(
        task = ?task,
        preference = ?preference,
        "Initialising PaddleOCR-VL engine (cold start)"
    );
    let new_engine =
        PaddleOcrVlEngine::new(model_path, task, candle_device, DType::F32).map_err(|e| crate::XbergError::Ocr {
            message: format!("PaddleOCR-VL engine initialisation failed: {e}"),
            source: Some(Box::new(e)),
        })?;
    let new_engine = Arc::new(Mutex::new(new_engine));

    let mut pool = ENGINE_POOL.write();
    if let Some(existing) = pool.get(&key) {
        return Ok(Arc::clone(existing));
    }
    pool.insert(key, Arc::clone(&new_engine));
    Ok(new_engine)
}

/// Default HuggingFace repo id for PaddleOCR-VL weights: a checksum-pinned mirror of
/// `PaddlePaddle/PaddleOCR-VL-1.6`. Used when `backend_options` provides neither
/// `model_path` nor a custom `model_id`.
const DEFAULT_MODEL_ID: &str = "xberg-io/paddleocr-vl-1.6";

/// PaddleOCR-VL backend using candle transformers.
///
/// A vision-language model for comprehensive document parsing. Supports text
/// recognition, tables, formulas, and charts through a unified interface with
/// markdown output. The engine implements the PaddleOCR-VL 1.5 architecture (SigLIP
/// vision encoder + ERNIE 4.5 text decoder); PaddleOCR-VL 1.6 keeps that architecture
/// unchanged, so it loads through the same engine — only the default weights changed.
///
/// Supports 109+ languages through the PaddlePaddle pretrained models.
///
/// # Configuration
///
/// PaddleOCR-VL accepts backend options for task selection, device, and weight source:
/// ```json
/// {
///   "task": "ocr",
///   "device": "auto",
///   "model_id": "xberg-io/paddleocr-vl-1.6",
///   "model_path": "/path/to/paddleocr-vl-model"
/// }
/// ```
///
/// - `task` (string): `"ocr"` (default), `"table"`, `"formula"`, `"chart"`
/// - `device` (string): `"auto"`, `"cpu"`, `"cuda"`, `"metal"`
/// - `model_id` (string): HuggingFace repo id to auto-download weights from. Defaults to
///   `xberg-io/paddleocr-vl-1.6`, a checksum-pinned mirror of
///   `PaddlePaddle/PaddleOCR-VL-1.6`. Ignored when `model_path` is set.
/// - `model_path` (string, optional): path to a local model directory. When omitted,
///   the weights named by `model_id` are downloaded on first use into the standard
///   Hugging Face cache — no manual staging required.
/// - `hf_revision` (string, optional): immutable commit for a custom `model_id`.
///   The default model is pinned automatically.
/// - `cache_dir` (string, optional): explicit Hugging Face Hub cache root. When
///   omitted, `HF_HUB_CACHE`, `HUGGINGFACE_HUB_CACHE`, and `HF_HOME` are honored.
#[cfg_attr(alef, alef(skip))]
pub struct PaddleOcrVlBackend {
    task: PaddleOcrVlTask,
}

struct PaddleOcrVlOptions {
    task: PaddleOcrVlTask,
    model_path: Option<String>,
    model_id: String,
    hf_revision: Option<String>,
    cache_dir: Option<PathBuf>,
    device: DevicePreference,
}

impl PaddleOcrVlBackend {
    /// Create a new PaddleOCR-VL backend with the specified task.
    pub fn new(task: PaddleOcrVlTask) -> Self {
        Self { task }
    }

    /// Create a PaddleOCR-VL backend with the default task (OCR).
    pub fn default_task() -> Self {
        Self::new(PaddleOcrVlTask::default())
    }

    /// Parse backend options to extract PaddleOCR-VL-specific configuration.
    ///
    /// Device selection is delegated to [`crate::candle_ocr::resolve_device_preference`]
    /// so the central `AccelerationConfig` is honoured.
    ///
    /// `model_id` defaults to [`DEFAULT_MODEL_ID`] and is only consulted when
    /// `model_path` is absent.
    fn parse_options(config: &OcrConfig) -> PaddleOcrVlOptions {
        let mut task = PaddleOcrVlTask::default();
        let mut model_path: Option<String> = None;
        let mut model_id = DEFAULT_MODEL_ID.to_string();
        let mut hf_revision = None;
        let mut cache_dir = None;

        if let Some(opts) = &config.backend_options {
            if let Some(t) = opts.get("task").and_then(|v| v.as_str()) {
                task = match t {
                    "table" => PaddleOcrVlTask::Table,
                    "formula" => PaddleOcrVlTask::Formula,
                    "chart" => PaddleOcrVlTask::Chart,
                    _ => PaddleOcrVlTask::Ocr,
                };
            }
            if let Some(p) = opts.get("model_path").and_then(|v| v.as_str()) {
                model_path = Some(p.to_string());
            }
            if let Some(id) = opts.get("model_id").and_then(|v| v.as_str()) {
                model_id = id.to_string();
            }
            hf_revision = opts
                .get("hf_revision")
                .or_else(|| opts.get("revision"))
                .and_then(|value| value.as_str())
                .map(str::to_string);
            cache_dir = opts
                .get("cache_dir")
                .and_then(|value| value.as_str())
                .map(PathBuf::from);
        }

        PaddleOcrVlOptions {
            task,
            model_path,
            model_id,
            hf_revision,
            cache_dir,
            device: super::resolve_device_preference(config),
        }
    }
}

impl Plugin for PaddleOcrVlBackend {
    fn name(&self) -> &str {
        "candle-paddleocr-vl"
    }

    fn version(&self) -> String {
        "0.1.0".to_string()
    }

    fn initialize(&self) -> Result<()> {
        tracing::debug!("Initializing PaddleOCR-VL backend: {} task", self.task);
        Ok(())
    }

    fn shutdown(&self) -> Result<()> {
        Ok(())
    }
}

#[async_trait]
impl OcrBackend for PaddleOcrVlBackend {
    /// Process an image using the PaddleOCR-VL engine.
    ///
    /// # Errors
    ///
    /// Returns [`crate::XbergError::Validation`] if `image_bytes` is empty.
    /// Returns [`crate::XbergError::Ocr`] if weight download, device selection,
    /// engine initialisation, or inference fails.
    async fn process_image(&self, image_bytes: &[u8], config: &OcrConfig) -> Result<ExtractedDocument> {
        let options = Self::parse_options(config);

        if image_bytes.is_empty() {
            return Err(crate::XbergError::Validation {
                message: "Empty image data provided to PaddleOCR-VL".to_string(),
                source: None,
            });
        }

        let image_bytes_owned = image_bytes.to_vec();

        let content = tokio::task::spawn_blocking(move || {
            let model_path = match options.model_path {
                Some(p) => p,
                None => super::model_stager::ensure_paddleocr_vl_16(
                    &options.model_id,
                    options.hf_revision.as_deref(),
                    options.cache_dir.as_deref(),
                )
                .map(|dir| dir.to_string_lossy().into_owned())
                .map_err(|e| crate::XbergError::Ocr {
                    message: format!("PaddleOCR-VL weight download failed: {e}"),
                    source: None,
                })?,
            };
            let engine = get_or_init_engine(&model_path, options.task, options.device)?;

            let mut engine_guard = engine.lock();
            let output = engine_guard
                .process_image(&image_bytes_owned)
                .map_err(|e| crate::XbergError::Ocr {
                    message: format!("PaddleOCR-VL inference failed: {e}"),
                    source: Some(Box::new(e)),
                })?;

            Ok::<String, crate::XbergError>(output.content)
        })
        .await
        .map_err(|e| crate::XbergError::Ocr {
            message: format!("PaddleOCR-VL task execution failed: {e}"),
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

    /// Process an image file using the PaddleOCR-VL engine.
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
    fn test_paddleocr_vl_backend_creation() {
        let backend = PaddleOcrVlBackend::default_task();
        assert_eq!(backend.name(), "candle-paddleocr-vl");
        assert_eq!(backend.backend_type(), OcrBackendType::Candle);
    }

    #[test]
    fn test_paddleocr_vl_emits_structured_markdown() {
        let backend = PaddleOcrVlBackend::default_task();
        assert!(backend.emits_structured_markdown());
    }

    #[test]
    fn test_paddleocr_vl_language_support() {
        let backend = PaddleOcrVlBackend::default_task();
        assert!(backend.supports_language("eng"));
        assert!(backend.supports_language("zho"));
        assert!(backend.supports_language("jpn"));
        assert!(backend.supports_language("fra"));
        assert!(backend.supports_language("unknown"));
    }

    #[test]
    fn test_paddleocr_vl_supported_languages() {
        let backend = PaddleOcrVlBackend::default_task();
        let langs = backend.supported_languages();
        assert!(langs.contains(&"eng".to_string()));
        assert!(langs.contains(&"zho".to_string()));
        assert!(langs.contains(&"jpn".to_string()));
    }

    #[test]
    fn test_parse_options_defaults() {
        let config = OcrConfig::default();
        let options = PaddleOcrVlBackend::parse_options(&config);
        assert_eq!(options.task, PaddleOcrVlTask::Ocr);
        assert!(options.model_path.is_none());
        assert_eq!(options.model_id, DEFAULT_MODEL_ID);
        assert!(options.hf_revision.is_none());
        assert!(options.cache_dir.is_none());
        assert_eq!(options.device, DevicePreference::Auto);
    }

    #[test]
    fn test_parse_options_custom_task() {
        let config = OcrConfig {
            backend_options: Some(serde_json::json!({
                "task": "table"
            })),
            ..Default::default()
        };
        let options = PaddleOcrVlBackend::parse_options(&config);
        assert_eq!(options.task, PaddleOcrVlTask::Table);
    }

    #[test]
    fn test_parse_options_custom_device() {
        let config = OcrConfig {
            backend_options: Some(serde_json::json!({
                "device": "cpu"
            })),
            ..Default::default()
        };
        let options = PaddleOcrVlBackend::parse_options(&config);
        assert_eq!(options.device, DevicePreference::Cpu);
    }

    #[test]
    fn test_parse_options_model_path() {
        let config = OcrConfig {
            backend_options: Some(serde_json::json!({
                "model_path": "/models/paddleocr-vl"
            })),
            ..Default::default()
        };
        let options = PaddleOcrVlBackend::parse_options(&config);
        assert_eq!(options.model_path.as_deref(), Some("/models/paddleocr-vl"));
    }

    #[test]
    fn test_parse_options_custom_model_id() {
        let config = OcrConfig {
            backend_options: Some(serde_json::json!({
                "model_id": "some-org/custom-paddleocr-vl"
            })),
            ..Default::default()
        };
        let options = PaddleOcrVlBackend::parse_options(&config);
        assert!(options.model_path.is_none());
        assert_eq!(options.model_id, "some-org/custom-paddleocr-vl");
    }

    #[test]
    fn test_parse_options_hf_cache_and_revision() {
        let config = OcrConfig {
            backend_options: Some(serde_json::json!({
                "hf_revision": "0123456789abcdef",
                "cache_dir": "/tmp/hf-hub"
            })),
            ..Default::default()
        };
        let options = PaddleOcrVlBackend::parse_options(&config);
        assert_eq!(options.hf_revision.as_deref(), Some("0123456789abcdef"));
        assert_eq!(options.cache_dir.as_deref(), Some(Path::new("/tmp/hf-hub")));
    }

    #[test]
    fn test_initialize_and_shutdown() {
        let backend = PaddleOcrVlBackend::default_task();
        assert!(backend.initialize().is_ok());
        assert!(backend.shutdown().is_ok());
    }
}
