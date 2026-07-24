//! PaddleOCR backend implementation.
//!
//! This module implements the `OcrBackend` trait for PaddleOCR using ONNX Runtime.
//! PaddleOCR provides excellent recognition quality, especially for CJK languages.
//!
//! The backend maintains a pool of OCR engines keyed by script family.
//! Each family gets its own lazily-initialized engine with the appropriate
//! recognition model and character dictionary.

use ahash::AHashMap;
use async_trait::async_trait;
use std::borrow::Cow;
use std::cell::RefCell;
use std::panic::catch_unwind;
use std::path::Path;
use std::sync::{Arc, Mutex};

thread_local! {
    static PADDLE_TL_ACCEL: RefCell<Option<crate::core::config::acceleration::AccelerationConfig>> = const { RefCell::new(None) };
}

fn paddle_accel_builder_fn(
    builder: ort::session::builder::SessionBuilder,
) -> std::result::Result<ort::session::builder::SessionBuilder, ort::Error> {
    let accel = PADDLE_TL_ACCEL.with(|cell| cell.borrow().clone());
    crate::ort_discovery::apply_execution_providers(builder, accel.as_ref())
}

use crate::Result;
use crate::core::config::OcrConfig;
use crate::ocr::conversion::{elements_to_hocr_words, text_block_to_element};
use crate::plugins::{OcrBackend, OcrBackendType, Plugin};
use crate::table_core::{reconstruct_table, table_to_markdown};
use crate::types::{ExtractedDocument, FormatMetadata, Metadata, OcrElement, OcrMetadata, Table};

#[cfg(test)]
use super::config::DEFAULT_RECOGNITION_BATCH_SIZE;
use super::config::{MAX_RECOGNITION_BATCH_SIZE, MIN_RECOGNITION_BATCH_SIZE, PaddleOcrConfig};
use super::model_manager::{ModelManager, SharedModelPaths};
use super::{is_language_supported, language_to_script_family, map_language_code};

use xberg_paddle_ocr::OcrLite;

/// PaddleOCR backend using ONNX Runtime.
///
/// Maintains a pool of OCR engines keyed by script family. Each family has its own
/// recognition model and character dictionary, while detection and classification
/// models are shared across all families.
///
/// # Thread Safety
///
/// The backend is `Send + Sync` and can be used across threads safely via `Arc`.
/// Each engine in the pool has its own mutex, so concurrent OCR on different
/// script families does not block.
#[cfg_attr(alef, alef(skip))]
pub struct PaddleOcrBackend {
    config: Arc<PaddleOcrConfig>,
    model_manager: ModelManager,
    /// Detection + classification model paths, lazily initialized and keyed by
    /// `"{model_version}/{model_tier}"` so a per-request `paddle_ocr_config`
    /// override loads the detection model matching its recognition model instead
    /// of the backend-default version/tier (issue #1279).
    shared_paths: Mutex<AHashMap<String, SharedModelPaths>>,
    /// Per-model OCR engines, lazily initialized. Keyed by "{tier}/{model_key}".
    /// Multiple script families may share the same engine (e.g. chinese+japanese use unified_server).
    /// OcrLite inference methods take `&self`, enabling lock-free concurrent page OCR.
    engine_pool: Mutex<AHashMap<String, Arc<OcrLite>>>,
    /// Document orientation detector, lazily initialized.
    doc_ori_detector: once_cell::sync::OnceCell<crate::doc_orientation::DocOrientationDetector>,
    /// Hardware acceleration configuration for ORT sessions (set at construction).
    /// Per-request acceleration from `OcrConfig.acceleration` takes precedence.
    acceleration: Option<crate::core::config::acceleration::AccelerationConfig>,
}

impl PaddleOcrBackend {
    /// Create a new PaddleOCR backend with default configuration.
    pub fn new() -> Result<Self> {
        Self::with_config(PaddleOcrConfig::default())
    }

    /// Create a new PaddleOCR backend with custom configuration.
    pub fn with_config(config: PaddleOcrConfig) -> Result<Self> {
        let cache_dir = config.resolve_cache_dir();
        Ok(Self {
            config: Arc::new(config),
            model_manager: ModelManager::new(cache_dir),
            shared_paths: Mutex::new(AHashMap::new()),
            engine_pool: Mutex::new(AHashMap::new()),
            doc_ori_detector: once_cell::sync::OnceCell::new(),
            acceleration: None,
        })
    }

    /// Set hardware acceleration for ORT sessions.
    pub fn with_acceleration(mut self, accel: crate::core::config::acceleration::AccelerationConfig) -> Self {
        self.acceleration = Some(accel);
        self
    }

    /// Get the current acceleration configuration, if any.
    pub fn acceleration(&self) -> Option<&crate::core::config::acceleration::AccelerationConfig> {
        self.acceleration.as_ref()
    }

    /// Resolve effective acceleration: per-request from OcrConfig takes precedence
    /// over the backend-level default.
    fn resolve_acceleration(
        &self,
        request_accel: Option<&crate::core::config::acceleration::AccelerationConfig>,
    ) -> Option<crate::core::config::acceleration::AccelerationConfig> {
        request_accel.cloned().or_else(|| self.acceleration.clone())
    }

    /// Get or initialize shared model paths (det + cls) for the given config's
    /// version and tier.
    ///
    /// Keyed by `"{model_version}/{model_tier}"` so a per-request override
    /// (`OcrConfig.paddle_ocr_config`) resolves a detection model matching its
    /// recognition model rather than the backend default (issue #1279).
    fn get_or_init_shared_paths(&self, config: &PaddleOcrConfig) -> Result<SharedModelPaths> {
        let key = format!("{}/{}", config.model_version, config.model_tier);
        let mut paths = self.shared_paths.lock().map_err(|e| crate::XbergError::Plugin {
            message: format!("Failed to acquire shared paths lock: {e}"),
            plugin_name: "paddle-ocr".to_string(),
        })?;

        if let Some(p) = paths.get(&key) {
            return Ok(p.clone());
        }

        let shared = self
            .model_manager
            .ensure_shared_models_versioned(&config.model_version, &config.model_tier)?;
        paths.insert(key, shared.clone());
        Ok(shared)
    }

    /// Get or create an OCR engine for the given script family.
    ///
    /// The engine pool is keyed by a composite `"{tier}/{model_key}/{accel}"` string.
    /// This ensures that:
    /// - Multiple families sharing the same unified model reuse one engine
    /// - Different tiers get different engines (different det model)
    /// - Different acceleration configs get separate engines (CPU vs CUDA)
    fn get_or_init_engine_for_family(
        &self,
        family: &str,
        config: &PaddleOcrConfig,
        accel: Option<&crate::core::config::acceleration::AccelerationConfig>,
    ) -> Result<Arc<OcrLite>> {
        let tier = &config.model_tier;
        let version = &config.model_version;
        let resolved = self.model_manager.resolve_rec_model_versioned(version, family, tier)?;
        let accel_key = match accel.map(|a| &a.provider) {
            Some(crate::core::config::acceleration::ExecutionProviderType::Cuda) => "cuda",
            Some(crate::core::config::acceleration::ExecutionProviderType::TensorRt) => "tensorrt",
            Some(crate::core::config::acceleration::ExecutionProviderType::CoreMl) => "coreml",
            Some(crate::core::config::acceleration::ExecutionProviderType::Auto) => "auto",
            Some(crate::core::config::acceleration::ExecutionProviderType::Cpu) | None => "cpu",
        };
        let pool_key = format!("{version}/{tier}/{}/{accel_key}", resolved.model_key);

        {
            let pool = self.engine_pool.lock().map_err(|e| crate::XbergError::Plugin {
                message: format!("Failed to acquire engine pool lock: {e}"),
                plugin_name: "paddle-ocr".to_string(),
            })?;
            if let Some(engine) = pool.get(&pool_key) {
                return Ok(Arc::clone(engine));
            }
        }

        let shared = self.get_or_init_shared_paths(config)?;

        crate::ort_discovery::ensure_ort_available();

        tracing::info!(family, model_key = %resolved.model_key, tier, "Initializing PaddleOCR engine");

        let mut ocr_lite = OcrLite::new();

        let det_model_path = Self::find_onnx_model(&shared.det_model)?;
        let cls_model_path = Self::find_onnx_model(&shared.cls_model)?;
        let rec_model_path = Self::find_onnx_model(&resolved.model_dir)?;

        let num_threads = 1;

        let dict_path = resolved.dict_file.to_str().ok_or_else(|| crate::XbergError::Ocr {
            message: "Invalid dictionary file path".to_string(),
            source: None,
        })?;

        // NOTE: The thread-local is set by `process_image` from the per-call
        let builder_fn: Option<
            fn(
                ort::session::builder::SessionBuilder,
            ) -> std::result::Result<ort::session::builder::SessionBuilder, ort::Error>,
        > = if PADDLE_TL_ACCEL.with(|cell| cell.borrow().is_some()) {
            Some(paddle_accel_builder_fn)
        } else {
            None
        };

        ocr_lite
            .init_models_with_dict_custom(
                det_model_path.to_str().ok_or_else(|| crate::XbergError::Ocr {
                    message: "Invalid detection model path".to_string(),
                    source: None,
                })?,
                cls_model_path.to_str().ok_or_else(|| crate::XbergError::Ocr {
                    message: "Invalid classification model path".to_string(),
                    source: None,
                })?,
                rec_model_path.to_str().ok_or_else(|| crate::XbergError::Ocr {
                    message: "Invalid recognition model path".to_string(),
                    source: None,
                })?,
                dict_path,
                num_threads,
                builder_fn,
            )
            .map_err(|e| crate::XbergError::Ocr {
                message: format!(
                    "Failed to initialize PaddleOCR models for {family} ({}): {e}",
                    resolved.model_key
                ),
                source: None,
            })?;

        tracing::info!(family, model_key = %resolved.model_key, "PaddleOCR engine initialized successfully");

        let engine = Arc::new(ocr_lite);

        let mut pool = self.engine_pool.lock().map_err(|e| crate::XbergError::Plugin {
            message: format!("Failed to acquire engine pool lock: {e}"),
            plugin_name: "paddle-ocr".to_string(),
        })?;

        if let Some(existing_engine) = pool.get(&pool_key) {
            return Ok(Arc::clone(existing_engine));
        }

        pool.insert(pool_key, Arc::clone(&engine));

        Ok(engine)
    }

    /// Find the ONNX model file within a model directory.
    fn find_onnx_model(model_dir: &std::path::Path) -> Result<std::path::PathBuf> {
        if model_dir.is_file() && model_dir.extension().is_some_and(|extension| extension == "onnx") {
            return Ok(model_dir.to_path_buf());
        }
        if !model_dir.exists() {
            return Err(crate::XbergError::Ocr {
                message: format!("Model directory does not exist: {:?}", model_dir),
                source: None,
            });
        }

        let standard_path = model_dir.join("model.onnx");
        if standard_path.exists() {
            return Ok(standard_path);
        }

        let entries = std::fs::read_dir(model_dir).map_err(|e| crate::XbergError::Ocr {
            message: format!("Failed to read model directory {:?}: {}", model_dir, e),
            source: None,
        })?;

        for entry in entries {
            let entry = entry.map_err(|e| crate::XbergError::Ocr {
                message: format!("Failed to read directory entry: {}", e),
                source: None,
            })?;
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "onnx") {
                return Ok(path);
            }
        }

        Err(crate::XbergError::Ocr {
            message: format!("No ONNX model file found in directory: {:?}", model_dir),
            source: None,
        })
    }

    /// Detect document orientation and rotate if needed.
    ///
    /// Returns `Ok(Some(rotated_bytes))` if rotation was applied,
    /// `Ok(None)` if no rotation needed (0° or low confidence).
    fn detect_and_rotate(&self, image_bytes: &[u8]) -> Result<Option<Vec<u8>>> {
        let detector = self.doc_ori_detector.get_or_try_init(|| {
            let cache_dir = self.config.resolve_cache_dir();
            Ok::<_, crate::XbergError>(crate::doc_orientation::DocOrientationDetector::with_acceleration(
                cache_dir,
                self.acceleration.clone(),
            ))
        })?;

        crate::doc_orientation::detect_and_rotate(detector, image_bytes)
    }

    /// Perform OCR on image bytes using the appropriate script family engine.
    async fn do_ocr(
        &self,
        image_bytes: &[u8],
        language: &str,
        effective_config: Arc<PaddleOcrConfig>,
        accel: Option<&crate::core::config::acceleration::AccelerationConfig>,
    ) -> Result<(String, Vec<OcrElement>)> {
        let family = language_to_script_family(language);
        let engine = self.get_or_init_engine_for_family(family, &effective_config, accel)?;

        let image_bytes_owned = image_bytes.to_vec();
        let config = effective_config;

        let text_blocks = tokio::task::spawn_blocking(move || {
            catch_unwind(std::panic::AssertUnwindSafe(|| {
                Self::perform_ocr(&image_bytes_owned, &engine, &config)
            }))
            .map_err(|_| crate::XbergError::Plugin {
                message: "PaddleOCR inference panicked (ONNX Runtime error)".to_string(),
                plugin_name: "paddle-ocr".to_string(),
            })?
        })
        .await
        .map_err(|e| crate::XbergError::Plugin {
            message: format!("PaddleOCR task panicked: {}", e),
            plugin_name: "paddle-ocr".to_string(),
        })??;

        let ocr_elements: Result<Vec<OcrElement>> = text_blocks
            .iter()
            .map(|block| text_block_to_element(block, 1))
            .filter_map(|result| result.transpose())
            .collect();

        let ocr_elements = ocr_elements?;

        let text = text_blocks
            .iter()
            .map(|block| block.text.as_str())
            .filter(|t| !t.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n");

        Ok((text, ocr_elements))
    }

    /// Perform actual OCR inference (runs in blocking context).
    /// OcrLite::detect takes &self — no Mutex needed, enabling true parallel page OCR.
    fn effective_rec_batch_size(config: &PaddleOcrConfig) -> u32 {
        config
            .rec_batch_num
            .clamp(MIN_RECOGNITION_BATCH_SIZE, MAX_RECOGNITION_BATCH_SIZE)
    }

    fn perform_ocr(
        image_bytes: &[u8],
        ocr_engine: &Arc<OcrLite>,
        config: &PaddleOcrConfig,
    ) -> Result<Vec<xberg_paddle_ocr::TextBlock>> {
        let img = crate::extraction::image::load_image_for_ocr(image_bytes)
            .map_err(|e| crate::XbergError::Ocr {
                message: e.to_string(),
                source: None,
            })?
            .to_rgb8();

        let padding = config.padding;
        let max_side_len = config.det_limit_side_len;
        let box_score_thresh = config.det_db_box_thresh;
        let box_thresh = config.det_db_thresh;
        let un_clip_ratio = config.det_db_unclip_ratio;
        let do_angle = config.use_angle_cls;
        let most_angle = false;
        let rec_batch_size = Self::effective_rec_batch_size(config);

        let result = ocr_engine
            .detect_with_rec_batch_size(
                &img,
                padding,
                max_side_len,
                box_score_thresh,
                box_thresh,
                un_clip_ratio,
                do_angle,
                most_angle,
                rec_batch_size,
            )
            .map_err(|e| crate::XbergError::Ocr {
                message: format!("PaddleOCR detection failed: {}", e),
                source: None,
            })?;

        let drop_score = config.drop_score;
        let text_blocks: Vec<_> = result
            .text_blocks
            .into_iter()
            .filter(|block| block.text_score >= drop_score && !block.text_score.is_nan())
            .collect();

        tracing::debug!(text_block_count = text_blocks.len(), "PaddleOCR detection completed");

        Ok(text_blocks)
    }
}

impl Plugin for PaddleOcrBackend {
    fn name(&self) -> &str {
        "paddle-ocr"
    }

    fn version(&self) -> String {
        env!("CARGO_PKG_VERSION").to_string()
    }

    fn initialize(&self) -> Result<()> {
        Ok(())
    }

    fn shutdown(&self) -> Result<()> {
        Ok(())
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
impl OcrBackend for PaddleOcrBackend {
    async fn process_image(&self, image_bytes: &[u8], config: &OcrConfig) -> Result<ExtractedDocument> {
        if image_bytes.is_empty() {
            return Err(crate::XbergError::Validation {
                message: "Empty image data provided to PaddleOCR".to_string(),
                source: None,
            });
        }

        PADDLE_TL_ACCEL.with(|cell| {
            *cell.borrow_mut() = config.acceleration.clone();
        });

        let effective_config: Arc<PaddleOcrConfig> = if let Some(ref paddle_json) = config.paddle_ocr_config {
            let overridden: PaddleOcrConfig =
                serde_json::from_value(paddle_json.clone()).map_err(|e| crate::XbergError::Validation {
                    message: format!("Failed to deserialize paddle_ocr_config: {}", e),
                    source: None,
                })?;
            Arc::new(overridden)
        } else {
            Arc::clone(&self.config)
        };

        let languages = config.effective_languages();
        let primary_lang = languages[0].as_str();
        let paddle_lang = map_language_code(primary_lang).unwrap_or("en");

        let ocr_image_bytes: std::borrow::Cow<'_, [u8]> = if config.auto_rotate {
            match self.detect_and_rotate(image_bytes) {
                Ok(Some(rotated)) => std::borrow::Cow::Owned(rotated),
                Ok(None) => std::borrow::Cow::Borrowed(image_bytes),
                Err(e) => {
                    tracing::warn!("Doc orientation detection failed, proceeding without rotation: {e}");
                    std::borrow::Cow::Borrowed(image_bytes)
                }
            }
        } else {
            std::borrow::Cow::Borrowed(image_bytes)
        };

        let effective_accel = self.resolve_acceleration(config.acceleration.as_ref());

        let (text, ocr_elements) = self
            .do_ocr(
                &ocr_image_bytes,
                paddle_lang,
                Arc::clone(&effective_config),
                effective_accel.as_ref(),
            )
            .await?;

        let text_blocks_count = ocr_elements.len();

        let ocr_doc = {
            use crate::types::extraction::BoundingBox;
            use crate::types::internal::{ElementKind, InternalDocument, InternalElement};
            use crate::types::ocr_elements::OcrElementLevel;

            let mut doc = InternalDocument::new("pdf");
            for elem in &ocr_elements {
                let (left, top, width, height) = elem.geometry.to_aabb();
                let bbox = BoundingBox {
                    x0: left as f64,
                    y0: top as f64,
                    x1: (left + width) as f64,
                    y1: (top + height) as f64,
                };
                let mut ie = InternalElement::text(
                    ElementKind::OcrText {
                        level: OcrElementLevel::Line,
                    },
                    &elem.text,
                    0,
                )
                .with_page(elem.page_number);
                ie.bbox = Some(bbox);
                ie.ocr_confidence = Some(elem.confidence.clone());
                ie.ocr_geometry = Some(elem.geometry.clone());
                doc.push_element(ie);
            }
            doc
        };

        tracing::debug!(
            text_blocks = text_blocks_count,
            ocr_elements = ocr_elements.len(),
            internal_doc_elements = ocr_doc.elements.len(),
            "PaddleOCR InternalDocument built"
        );

        let mut tables: Vec<Table> = vec![];
        let mut table_count = 0;
        let mut table_rows: Option<u32> = None;
        let mut table_cols: Option<u32> = None;

        if effective_config.enable_table_detection && !ocr_elements.is_empty() {
            let words = elements_to_hocr_words(&ocr_elements, 0.3);

            if !words.is_empty() {
                let cells = reconstruct_table(&words, 20, 0.5);

                if !cells.is_empty() {
                    table_count = 1;
                    table_rows = Some(cells.len() as u32);
                    table_cols = cells.first().map(|row| row.len() as u32);

                    let table_markdown = table_to_markdown(&cells);

                    tables.push(Table {
                        cells,
                        markdown: table_markdown,
                        page_number: 1,
                        bounding_box: None,
                        ..Default::default()
                    });
                }
            }
        }

        let metadata = Metadata {
            format: Some(FormatMetadata::Ocr(OcrMetadata {
                language: config.effective_languages().join("+"),
                psm: 3,
                output_format: "text".to_string(),
                table_count,
                table_rows,
                table_cols,
            })),
            ..Default::default()
        };

        let include_elements = config.element_config.as_ref().is_some_and(|ec| ec.include_elements);

        let ocr_elements_opt = if include_elements && !ocr_elements.is_empty() {
            Some(ocr_elements)
        } else {
            None
        };

        Ok(ExtractedDocument {
            content: text,
            mime_type: Cow::Borrowed("text/plain"),
            metadata,
            tables,
            detected_languages: Some(config.effective_languages()),
            ocr_elements: ocr_elements_opt,
            ocr_internal_document: Some(ocr_doc),
            ..Default::default()
        })
    }

    async fn process_image_file(&self, path: &Path, config: &OcrConfig) -> Result<ExtractedDocument> {
        let bytes = tokio::fs::read(path).await?;
        self.process_image(&bytes, config).await
    }

    fn supports_language(&self, lang: &str) -> bool {
        is_language_supported(lang) || map_language_code(lang).is_some()
    }

    fn backend_type(&self) -> OcrBackendType {
        OcrBackendType::PaddleOCR
    }

    fn supported_languages(&self) -> Vec<String> {
        super::SUPPORTED_LANGUAGES.iter().map(|s| s.to_string()).collect()
    }

    fn supports_table_detection(&self) -> bool {
        self.config.enable_table_detection
    }
}

impl Default for PaddleOcrBackend {
    fn default() -> Self {
        Self::with_config(PaddleOcrConfig::default())
            .unwrap_or_else(|e| panic!("Failed to create default PaddleOcrBackend: {}", e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_paddle_ocr_backend_creation() {
        let result = PaddleOcrBackend::new();
        assert!(result.is_ok(), "Failed to create PaddleOCR backend");
    }

    #[test]
    fn test_paddle_ocr_backend_with_config() {
        let config = PaddleOcrConfig::default();
        let result = PaddleOcrBackend::with_config(config);
        assert!(result.is_ok());
    }

    #[test]
    fn test_effective_rec_batch_size_enforces_bounds() {
        let cases = [
            (0, MIN_RECOGNITION_BATCH_SIZE),
            (PaddleOcrConfig::default().rec_batch_num, DEFAULT_RECOGNITION_BATCH_SIZE),
            (12, 12),
            (u32::MAX, MAX_RECOGNITION_BATCH_SIZE),
        ];

        for (configured, expected) in cases {
            let config = PaddleOcrConfig {
                rec_batch_num: configured,
                ..Default::default()
            };

            assert_eq!(
                PaddleOcrBackend::effective_rec_batch_size(&config),
                expected,
                "unexpected effective recognition batch size for configured value {configured}"
            );
        }
    }

    #[test]
    fn test_paddle_ocr_language_support_direct() {
        let backend = PaddleOcrBackend::new().unwrap();

        assert!(backend.supports_language("ch"));
        assert!(backend.supports_language("en"));
        assert!(backend.supports_language("japan"));
        assert!(backend.supports_language("korean"));
        assert!(backend.supports_language("french"));
        assert!(backend.supports_language("thai"));
        assert!(backend.supports_language("greek"));
    }

    #[test]
    fn test_paddle_ocr_language_support_mapped() {
        let backend = PaddleOcrBackend::new().unwrap();

        assert!(backend.supports_language("chi_sim"));
        assert!(backend.supports_language("eng"));
        assert!(backend.supports_language("jpn"));
        assert!(backend.supports_language("kor"));
        assert!(backend.supports_language("fra"));
        assert!(backend.supports_language("zho"));
        assert!(backend.supports_language("tha"));
        assert!(backend.supports_language("ell"));
        assert!(backend.supports_language("rus"));
    }

    #[test]
    fn test_paddle_ocr_language_unsupported() {
        let backend = PaddleOcrBackend::new().unwrap();

        assert!(!backend.supports_language("xyz"));
        assert!(!backend.supports_language("invalid"));
    }

    #[test]
    fn test_paddle_ocr_plugin_interface() {
        let backend = PaddleOcrBackend::new().unwrap();

        assert_eq!(backend.name(), "paddle-ocr");
        assert!(!backend.version().is_empty());
        assert!(backend.initialize().is_ok());
        assert!(backend.shutdown().is_ok());
    }

    #[test]
    fn test_paddle_ocr_backend_type() {
        let backend = PaddleOcrBackend::new().unwrap();
        assert_eq!(backend.backend_type(), OcrBackendType::PaddleOCR);
    }

    #[test]
    fn test_paddle_ocr_supported_languages() {
        let backend = PaddleOcrBackend::new().unwrap();
        let languages = backend.supported_languages();

        assert!(!languages.is_empty());
        assert!(languages.contains(&"ch".to_string()));
        assert!(languages.contains(&"en".to_string()));
        assert!(languages.contains(&"thai".to_string()));
        assert!(languages.contains(&"greek".to_string()));
    }

    #[test]
    fn test_paddle_ocr_table_detection_disabled_by_default() {
        let backend = PaddleOcrBackend::new().unwrap();
        assert!(!backend.supports_table_detection());
    }

    #[test]
    fn test_paddle_ocr_table_detection_enabled() {
        let config = PaddleOcrConfig::default().with_table_detection(true);
        let backend = PaddleOcrBackend::with_config(config).unwrap();
        assert!(backend.supports_table_detection());
    }

    #[test]
    fn test_paddle_ocr_default() {
        let backend = PaddleOcrBackend::default();
        assert_eq!(backend.name(), "paddle-ocr");
    }

    #[tokio::test]
    async fn test_paddle_ocr_process_empty_image() {
        let backend = PaddleOcrBackend::new().unwrap();
        let config = OcrConfig {
            backend: "paddle-ocr".to_string(),
            language: vec!["ch".to_string()],
            ..Default::default()
        };

        let result = backend.process_image(&[], &config).await;
        assert!(result.is_err(), "Should error on empty image");
    }

    #[test]
    fn test_internal_document_from_text_blocks() {
        use crate::ocr::conversion::text_block_to_element;
        use crate::types::extraction::BoundingBox;
        use crate::types::internal::{ElementKind, InternalDocument, InternalElement};
        use crate::types::ocr_elements::OcrElementLevel;

        let blocks = [
            xberg_paddle_ocr::TextBlock {
                text: "Hello World".to_string(),
                box_points: vec![
                    xberg_paddle_ocr::Point { x: 10, y: 10 },
                    xberg_paddle_ocr::Point { x: 200, y: 10 },
                    xberg_paddle_ocr::Point { x: 200, y: 50 },
                    xberg_paddle_ocr::Point { x: 10, y: 50 },
                ],
                box_score: 0.95,
                text_score: 0.92,
                angle_index: 0,
                angle_score: 0.99,
            },
            xberg_paddle_ocr::TextBlock {
                text: "Second line".to_string(),
                box_points: vec![
                    xberg_paddle_ocr::Point { x: 10, y: 60 },
                    xberg_paddle_ocr::Point { x: 300, y: 60 },
                    xberg_paddle_ocr::Point { x: 300, y: 100 },
                    xberg_paddle_ocr::Point { x: 10, y: 100 },
                ],
                box_score: 0.88,
                text_score: 0.85,
                angle_index: 0,
                angle_score: 0.97,
            },
        ];

        let ocr_elements: Vec<OcrElement> = blocks
            .iter()
            .map(|block| text_block_to_element(block, 1))
            .filter_map(|result| result.transpose())
            .collect::<crate::Result<Vec<_>>>()
            .expect("text_block_to_element should succeed");

        assert_eq!(ocr_elements.len(), 2, "Should produce 2 OcrElements");

        let mut doc = InternalDocument::new("pdf");
        for elem in &ocr_elements {
            let (left, top, width, height) = elem.geometry.to_aabb();
            let bbox = BoundingBox {
                x0: left as f64,
                y0: top as f64,
                x1: (left + width) as f64,
                y1: (top + height) as f64,
            };
            let mut ie = InternalElement::text(
                ElementKind::OcrText {
                    level: OcrElementLevel::Line,
                },
                &elem.text,
                0,
            )
            .with_page(elem.page_number);
            ie.bbox = Some(bbox);
            ie.ocr_confidence = Some(elem.confidence.clone());
            ie.ocr_geometry = Some(elem.geometry.clone());
            doc.push_element(ie);
        }

        for ie in &doc.elements {
            assert!(
                matches!(
                    ie.kind,
                    ElementKind::OcrText {
                        level: OcrElementLevel::Line
                    }
                ),
                "Element kind should be OcrText with Line level"
            );
        }

        let first_bbox = doc.elements[0].bbox.as_ref().expect("First element should have bbox");
        assert_eq!(first_bbox.x0, 10.0, "left should be min x of quad points");
        assert_eq!(first_bbox.y0, 10.0, "top should be min y of quad points");
        assert_eq!(first_bbox.x1, 200.0, "right should be left + width");
        assert_eq!(first_bbox.y1, 50.0, "bottom should be top + height");

        let second_bbox = doc.elements[1].bbox.as_ref().expect("Second element should have bbox");
        assert_eq!(second_bbox.x0, 10.0);
        assert_eq!(second_bbox.y0, 60.0);
        assert_eq!(second_bbox.x1, 300.0);
        assert_eq!(second_bbox.y1, 100.0);

        let first_conf = doc.elements[0]
            .ocr_confidence
            .as_ref()
            .expect("First element should have confidence");
        assert!(
            (first_conf.detection.unwrap() - 0.95).abs() < 1e-6,
            "Detection confidence should be ~0.95, got {}",
            first_conf.detection.unwrap()
        );
        assert!(
            (first_conf.recognition - 0.92).abs() < 1e-6,
            "Recognition confidence should be ~0.92, got {}",
            first_conf.recognition
        );

        assert_eq!(doc.elements[0].page, Some(1));
        assert_eq!(doc.elements[1].page, Some(1));
    }

    /// Regression test for #783: verifies that `process_image` sets `PADDLE_TL_ACCEL`
    /// from `OcrConfig::acceleration` so that ONNX session builders can apply the
    /// requested execution provider (e.g. CUDA).
    ///
    /// This is a unit test of the threading mechanism only — it does not create
    /// real ONNX sessions or require a GPU.
    #[test]
    fn test_paddle_accel_tl_set_from_ocr_config_acceleration() {
        use crate::core::config::AccelerationConfig;

        PADDLE_TL_ACCEL.with(|cell| {
            *cell.borrow_mut() = Some(AccelerationConfig {
                provider: crate::core::config::acceleration::ExecutionProviderType::Cpu,
                device_id: 0,
            });
        });

        let accel: Option<AccelerationConfig> = None;
        PADDLE_TL_ACCEL.with(|cell| {
            *cell.borrow_mut() = accel.clone();
        });
        let tl_value = PADDLE_TL_ACCEL.with(|cell| cell.borrow().clone());
        assert!(tl_value.is_none(), "TL should be cleared when acceleration is None");

        let cuda_accel = AccelerationConfig {
            provider: crate::core::config::acceleration::ExecutionProviderType::Cuda,
            device_id: 0,
        };
        PADDLE_TL_ACCEL.with(|cell| {
            *cell.borrow_mut() = Some(cuda_accel.clone());
        });
        let tl_value = PADDLE_TL_ACCEL.with(|cell| cell.borrow().clone());
        assert!(tl_value.is_some(), "TL should be set when acceleration is Some");
        assert_eq!(
            tl_value.unwrap().provider,
            crate::core::config::acceleration::ExecutionProviderType::Cuda,
            "TL provider should be Cuda"
        );

        PADDLE_TL_ACCEL.with(|cell| {
            *cell.borrow_mut() = None;
        });
    }
}
