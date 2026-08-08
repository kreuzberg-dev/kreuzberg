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
#[cfg(feature = "paddle-ocr-ort")]
use std::cell::RefCell;
use std::panic::catch_unwind;
use std::path::Path;
use std::sync::{Arc, Mutex};

// Acceleration/execution-provider hook is ORT-only: the tract backend is CPU-only and
// never reads this thread-local (see `init_engine_tract`, which ignores acceleration
// rather than erroring).
#[cfg(feature = "paddle-ocr-ort")]
thread_local! {
    static PADDLE_TL_ACCEL: RefCell<Option<crate::core::config::acceleration::AccelerationConfig>> = const { RefCell::new(None) };
}

#[cfg(feature = "paddle-ocr-ort")]
fn paddle_accel_builder_fn(
    builder: ort::session::builder::SessionBuilder,
) -> std::result::Result<ort::session::builder::SessionBuilder, ort::Error> {
    let accel = PADDLE_TL_ACCEL.with(|cell| cell.borrow().clone());
    crate::ort_discovery::apply_execution_providers(builder, accel.as_ref())
}

use crate::Result;
use crate::core::config::OcrConfig;
use crate::ocr::conversion::{detailed_text_block_to_elements, elements_to_hocr_words};
use crate::plugins::{OcrBackend, OcrBackendType, Plugin};
use crate::table_core::{reconstruct_table, table_to_markdown};
use crate::types::{
    ExtractedDocument, FormatMetadata, Metadata, OcrElement, OcrElementConfig, OcrElementLevel, OcrMetadata, Table,
};

#[cfg(test)]
use super::config::DEFAULT_RECOGNITION_BATCH_SIZE;
use super::config::{MAX_RECOGNITION_BATCH_SIZE, MIN_RECOGNITION_BATCH_SIZE, PaddleInferenceBackend, PaddleOcrConfig};
use super::model_manager::{ModelManager, ResolvedRecModel, SharedModelPaths};
use super::{is_language_supported, language_to_script_family, map_language_code};

use xberg_paddle_ocr::PaddleOcrEngine;

type InitCell<T> = Arc<once_cell::sync::OnceCell<T>>;
type InitPool<T> = Mutex<AHashMap<String, InitCell<T>>>;

#[cfg(feature = "paddle-ocr-ort")]
struct PaddleAccelerationGuard {
    previous: Option<crate::core::config::acceleration::AccelerationConfig>,
}

#[cfg(feature = "paddle-ocr-ort")]
impl PaddleAccelerationGuard {
    fn set(acceleration: Option<crate::core::config::acceleration::AccelerationConfig>) -> Self {
        let previous = PADDLE_TL_ACCEL.with(|cell| cell.replace(acceleration));
        Self { previous }
    }
}

#[cfg(feature = "paddle-ocr-ort")]
impl Drop for PaddleAccelerationGuard {
    fn drop(&mut self) {
        PADDLE_TL_ACCEL.with(|cell| {
            cell.replace(self.previous.take());
        });
    }
}

fn init_cell_for_key<T>(pool: &InitPool<T>, key: &str) -> std::result::Result<InitCell<T>, String> {
    let mut pool = pool.lock().map_err(|error| error.to_string())?;
    if let Some(cell) = pool.get(key) {
        return Ok(Arc::clone(cell));
    }

    let cell = Arc::new(once_cell::sync::OnceCell::new());
    pool.insert(key.to_string(), Arc::clone(&cell));
    Ok(cell)
}

fn engine_pool_key(
    version: &str,
    tier: &str,
    model_key: &str,
    accel: Option<&crate::core::config::acceleration::AccelerationConfig>,
    backend: PaddleInferenceBackend,
) -> String {
    use crate::core::config::acceleration::ExecutionProviderType;

    let accel_key = match accel.map(|config| (&config.provider, config.device_id)) {
        Some((ExecutionProviderType::Cuda, device_id)) => format!("cuda:{device_id}"),
        Some((ExecutionProviderType::TensorRt, device_id)) => format!("tensorrt:{device_id}"),
        Some((ExecutionProviderType::CoreMl, _)) => "coreml".to_string(),
        Some((ExecutionProviderType::Auto, _)) => "auto".to_string(),
        Some((ExecutionProviderType::Cpu, _)) | None => "cpu".to_string(),
    };
    // A build can compile both `paddle-ocr-ort` and `paddle-ocr-tract` together (native
    // parity benchmarks); fold the resolved backend into the key so the pool never hands
    // back an engine constructed on the other engine.
    let backend_key = match backend {
        PaddleInferenceBackend::Ort => "ort",
        PaddleInferenceBackend::Tract => "tract",
    };
    format!("{version}/{tier}/{model_key}/{accel_key}/{backend_key}")
}

const INFERENCE_THREAD_COUNT: usize = 1;
use crate::ocr_metadata_keys::OCR_ORIENTATION_CONFIDENCE_METADATA_KEY as ORIENTATION_CONFIDENCE_METADATA_KEY;
const VERTICAL_TEXT_MIN_ASPECT_RATIO: f32 = 1.5;
const VERTICAL_COLUMN_MIN_OVERLAP_RATIO: f32 = 0.5;

#[derive(Debug)]
struct RotationOutcome {
    rotated_bytes: Option<Vec<u8>>,
    processed_width: u32,
    processed_height: u32,
    orientation: Option<crate::doc_orientation::OrientationResult>,
}

struct PaddlePageOcr {
    text: String,
    line_elements: Vec<OcrElement>,
    word_elements: Vec<OcrElement>,
    processed_width: u32,
    processed_height: u32,
}

impl RotationOutcome {
    fn unrotated(width: u32, height: u32) -> Self {
        Self {
            rotated_bytes: None,
            processed_width: width,
            processed_height: height,
            orientation: None,
        }
    }

    fn auto_rotated(&self) -> bool {
        self.rotated_bytes.is_some()
    }
}

fn rotate_for_detected_orientation(
    image: &image::RgbImage,
    orientation: crate::doc_orientation::OrientationResult,
) -> Result<RotationOutcome> {
    if orientation.degrees == 0 || orientation.confidence < crate::doc_orientation::MIN_CONFIDENCE {
        return Ok(RotationOutcome {
            rotated_bytes: None,
            processed_width: image.width(),
            processed_height: image.height(),
            orientation: Some(orientation),
        });
    }

    let rotated = match orientation.degrees {
        90 => image::imageops::rotate270(image),
        180 => image::imageops::rotate180(image),
        270 => image::imageops::rotate90(image),
        _ => {
            return Ok(RotationOutcome {
                rotated_bytes: None,
                processed_width: image.width(),
                processed_height: image.height(),
                orientation: Some(orientation),
            });
        }
    };
    let processed_width = rotated.width();
    let processed_height = rotated.height();
    let mut encoded = std::io::Cursor::new(Vec::new());
    rotated
        .write_to(&mut encoded, image::ImageFormat::Png)
        .map_err(|error| crate::XbergError::Ocr {
            message: format!("Failed to encode rotated PaddleOCR image: {error}"),
            source: None,
        })?;

    Ok(RotationOutcome {
        rotated_bytes: Some(encoded.into_inner()),
        processed_width,
        processed_height,
        orientation: Some(orientation),
    })
}

fn image_metadata(outcome: &RotationOutcome) -> AHashMap<Cow<'static, str>, serde_json::Value> {
    let mut additional = AHashMap::new();
    additional.insert(
        Cow::Borrowed(crate::ocr_metadata_keys::OCR_PROCESSED_IMAGE_WIDTH_METADATA_KEY),
        serde_json::Value::Number(outcome.processed_width.into()),
    );
    additional.insert(
        Cow::Borrowed(crate::ocr_metadata_keys::OCR_PROCESSED_IMAGE_HEIGHT_METADATA_KEY),
        serde_json::Value::Number(outcome.processed_height.into()),
    );
    if let Some(orientation) = outcome.orientation {
        additional.insert(
            Cow::Borrowed(crate::ocr_metadata_keys::OCR_ORIENTATION_DEGREES_METADATA_KEY),
            serde_json::Value::Number(orientation.degrees.into()),
        );
        additional.insert(
            Cow::Borrowed(ORIENTATION_CONFIDENCE_METADATA_KEY),
            serde_json::json!(orientation.confidence),
        );
    }
    if outcome.auto_rotated() {
        additional.insert(
            Cow::Borrowed(crate::ocr_metadata_keys::OCR_AUTO_ROTATED_METADATA_KEY),
            serde_json::Value::Bool(true),
        );
    }
    additional
}

/// PaddleOCR backend using ONNX Runtime.
///
/// Maintains a pool of OCR engines keyed by script family. Each family has its own
/// recognition model and character dictionary, while detection and classification
/// models are shared across all families.
///
/// # Thread Safety
///
/// The backend is `Send + Sync` and can be used across threads safely via `Arc`.
/// Per-key initialization cells serialize each cold start. Initialized engines
/// can run OCR concurrently without holding the pool lock.
#[cfg_attr(alef, alef(skip))]
pub struct PaddleOcrBackend {
    config: Arc<PaddleOcrConfig>,
    model_manager: ModelManager,
    /// Detection + classification model paths, lazily initialized and keyed by
    /// `"{model_version}/{model_tier}"` so a per-request `paddle_ocr_config`
    /// override loads the detection model matching its recognition model instead
    /// of the backend-default version/tier (issue #1279).
    shared_paths: Arc<InitPool<SharedModelPaths>>,
    /// Per-model OCR engines, lazily initialized. Keyed by "{version}/{tier}/{model_key}/{accel}".
    /// Multiple script families may share the same engine (e.g. chinese+japanese use unified_server).
    /// The per-key cell ensures concurrent cold requests initialize each engine only once. ~keep
    /// Paddle inference methods take `&self`, enabling lock-free concurrent page OCR.
    engine_pool: Arc<InitPool<Arc<PaddleOcrEngine>>>,
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
            shared_paths: Arc::new(Mutex::new(AHashMap::new())),
            engine_pool: Arc::new(Mutex::new(AHashMap::new())),
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
    fn get_or_init_shared_paths(
        model_manager: &ModelManager,
        shared_paths: &InitPool<SharedModelPaths>,
        config: &PaddleOcrConfig,
    ) -> Result<SharedModelPaths> {
        let key = format!("{}/{}", config.model_version, config.model_tier);
        let init_cell = init_cell_for_key(shared_paths, &key).map_err(|error| crate::XbergError::Plugin {
            message: format!("Failed to acquire shared paths lock: {error}"),
            plugin_name: "paddle-ocr".to_string(),
        })?;
        init_cell
            .get_or_try_init(|| model_manager.ensure_shared_models_versioned(&config.model_version, &config.model_tier))
            .cloned()
    }

    /// Get or create an OCR engine for the given script family.
    ///
    /// The engine pool is keyed by a composite `"{version}/{tier}/{model_key}/{accel}"` string.
    /// This ensures that:
    /// - Multiple families sharing the same unified model reuse one engine
    /// - Different tiers get different engines (different det model)
    /// - Different acceleration configs get separate engines (CPU vs CUDA)
    async fn get_or_init_engine_for_family(
        &self,
        family: &str,
        config: Arc<PaddleOcrConfig>,
        accel: Option<&crate::core::config::acceleration::AccelerationConfig>,
    ) -> Result<Arc<PaddleOcrEngine>> {
        let model_manager = self.model_manager.clone();
        let shared_paths = Arc::clone(&self.shared_paths);
        let engine_pool = Arc::clone(&self.engine_pool);
        let family = family.to_string();
        let accel = accel.cloned();

        // Model I/O, same-key waits, and ORT session construction must stay off async workers. ~keep
        tokio::task::spawn_blocking(move || {
            Self::get_or_init_engine_for_family_blocking(
                &model_manager,
                &shared_paths,
                &engine_pool,
                &family,
                &config,
                accel.as_ref(),
            )
        })
        .await
        .map_err(|error| crate::XbergError::Plugin {
            message: format!("PaddleOCR initialization task panicked: {error}"),
            plugin_name: "paddle-ocr".to_string(),
        })?
    }

    fn get_or_init_engine_for_family_blocking(
        model_manager: &ModelManager,
        shared_paths: &InitPool<SharedModelPaths>,
        engine_pool: &InitPool<Arc<PaddleOcrEngine>>,
        family: &str,
        config: &PaddleOcrConfig,
        accel: Option<&crate::core::config::acceleration::AccelerationConfig>,
    ) -> Result<Arc<PaddleOcrEngine>> {
        let tier = &config.model_tier;
        let version = &config.model_version;
        let backend = Self::effective_backend(config)?;
        let resolved = model_manager.resolve_rec_model_versioned(version, family, tier)?;
        let pool_key = engine_pool_key(version, tier, &resolved.model_key, accel, backend);

        let init_cell = init_cell_for_key(engine_pool, &pool_key).map_err(|error| crate::XbergError::Plugin {
            message: format!("Failed to acquire engine pool lock: {error}"),
            plugin_name: "paddle-ocr".to_string(),
        })?;
        let engine = init_cell.get_or_try_init(|| -> Result<Arc<PaddleOcrEngine>> {
            let shared = Self::get_or_init_shared_paths(model_manager, shared_paths, config)?;
            Self::initialize_engine(family, tier, &resolved, &shared, accel.cloned(), backend)
        })?;

        Ok(Arc::clone(engine))
    }

    /// Resolve which inference engine to use, validating an explicit
    /// `config.inference_backend` request against the compiled features.
    ///
    /// Mirrors `sceptre_ocr`'s `validate_inference_backend` free function in
    /// `crates/xberg/src/sceptre_ocr/mod.rs`: an unset request resolves to the
    /// compiled default (`ort` when `paddle-ocr-ort` is compiled in, else `tract`,
    /// preserving today's behavior exactly); an explicit choice whose feature is not
    /// compiled in is a clear configuration error rather than a silent fallback.
    fn effective_backend(config: &PaddleOcrConfig) -> Result<PaddleInferenceBackend> {
        let requested = config.inference_backend.unwrap_or_else(Self::default_inference_backend);
        match requested {
            PaddleInferenceBackend::Ort if cfg!(feature = "paddle-ocr-ort") => Ok(requested),
            PaddleInferenceBackend::Tract if cfg!(feature = "paddle-ocr-tract") => Ok(requested),
            PaddleInferenceBackend::Ort => Err(crate::XbergError::Ocr {
                message: "PaddleOCR ORT inference is unavailable in this build; enable `paddle-ocr-ort`".to_string(),
                source: None,
            }),
            PaddleInferenceBackend::Tract => Err(crate::XbergError::Ocr {
                message: "PaddleOCR tract inference is unavailable in this build; enable `paddle-ocr-tract`"
                    .to_string(),
                source: None,
            }),
        }
    }

    /// The compile-time default backend when `config.inference_backend` is unset:
    /// `ort` when `paddle-ocr-ort` is compiled in (preserving today's behavior
    /// exactly), otherwise `tract`.
    fn default_inference_backend() -> PaddleInferenceBackend {
        #[cfg(feature = "paddle-ocr-ort")]
        {
            PaddleInferenceBackend::Ort
        }
        #[cfg(not(feature = "paddle-ocr-ort"))]
        {
            PaddleInferenceBackend::Tract
        }
    }

    fn initialize_engine(
        family: &str,
        tier: &str,
        resolved: &ResolvedRecModel,
        shared: &SharedModelPaths,
        accel: Option<crate::core::config::acceleration::AccelerationConfig>,
        backend: PaddleInferenceBackend,
    ) -> Result<Arc<PaddleOcrEngine>> {
        tracing::info!(family, model_key = %resolved.model_key, tier, ?backend, "Initializing PaddleOCR engine");

        let mut ocr_engine = PaddleOcrEngine::new();
        let det_model_path = Self::find_onnx_model(&shared.det_model)?;
        let cls_model_path = Self::find_onnx_model(&shared.cls_model)?;
        let rec_model_path = Self::find_onnx_model(&resolved.model_dir)?;
        let det_model_path = det_model_path.to_str().ok_or_else(|| crate::XbergError::Ocr {
            message: "Invalid detection model path".to_string(),
            source: None,
        })?;
        let cls_model_path = cls_model_path.to_str().ok_or_else(|| crate::XbergError::Ocr {
            message: "Invalid classification model path".to_string(),
            source: None,
        })?;
        let rec_model_path = rec_model_path.to_str().ok_or_else(|| crate::XbergError::Ocr {
            message: "Invalid recognition model path".to_string(),
            source: None,
        })?;
        let dict_path = resolved.dict_file.to_str().ok_or_else(|| crate::XbergError::Ocr {
            message: "Invalid dictionary file path".to_string(),
            source: None,
        })?;

        match backend {
            #[cfg(feature = "paddle-ocr-ort")]
            PaddleInferenceBackend::Ort => Self::init_engine_ort(
                &mut ocr_engine,
                det_model_path,
                cls_model_path,
                rec_model_path,
                dict_path,
                accel,
            )
            .map_err(|error| crate::XbergError::Ocr {
                message: format!(
                    "Failed to initialize PaddleOCR models for {family} ({}) on the ort backend: {error}",
                    resolved.model_key
                ),
                source: None,
            })?,
            #[cfg(feature = "paddle-ocr-tract")]
            PaddleInferenceBackend::Tract => Self::init_engine_tract(
                &mut ocr_engine,
                det_model_path,
                cls_model_path,
                rec_model_path,
                dict_path,
                accel.as_ref(),
            )
            .map_err(|error| crate::XbergError::Ocr {
                message: format!(
                    "Failed to initialize PaddleOCR models for {family} ({}) on the tract backend: {error}",
                    resolved.model_key
                ),
                source: None,
            })?,
            // Unreachable in practice: `effective_backend` already rejects a backend whose
            // feature is not compiled in before `initialize_engine` is ever called. This arm
            // only exists so the match stays exhaustive in a single-engine build, mirroring
            // `xberg_paddle_ocr::inference::load_backend`'s catch-all. ~keep
            #[allow(unreachable_patterns)]
            other => {
                return Err(crate::XbergError::Ocr {
                    message: format!(
                        "PaddleOCR backend {other:?} is not compiled in (enable the matching cargo feature)"
                    ),
                    source: None,
                });
            }
        }

        tracing::info!(family, model_key = %resolved.model_key, "PaddleOCR engine initialized successfully");
        Ok(Arc::new(ocr_engine))
    }

    /// Load models onto the native ONNX Runtime backend, applying the acceleration/EP
    /// hook (see `paddle_accel_builder_fn`) when one is configured.
    #[cfg(feature = "paddle-ocr-ort")]
    fn init_engine_ort(
        ocr_engine: &mut PaddleOcrEngine,
        det_model_path: &str,
        cls_model_path: &str,
        rec_model_path: &str,
        dict_path: &str,
        accel: Option<crate::core::config::acceleration::AccelerationConfig>,
    ) -> std::result::Result<(), xberg_paddle_ocr::OcrError> {
        let _acceleration_guard = PaddleAccelerationGuard::set(accel);
        crate::ort_discovery::ensure_ort_available();

        let builder_fn: Option<
            fn(
                ort::session::builder::SessionBuilder,
            ) -> std::result::Result<ort::session::builder::SessionBuilder, ort::Error>,
        > = if PADDLE_TL_ACCEL.with(|cell| cell.borrow().is_some()) {
            Some(paddle_accel_builder_fn)
        } else {
            None
        };

        ocr_engine.init_models_with_dict_custom(
            det_model_path,
            cls_model_path,
            rec_model_path,
            dict_path,
            INFERENCE_THREAD_COUNT,
            builder_fn,
        )
    }

    /// Load models onto the pure-Rust tract backend. Tract is CPU-only, so an EP
    /// acceleration request is logged and ignored rather than treated as an error —
    /// mirroring `sceptre_ocr::validate_acceleration`'s CPU-only stance for tract targets.
    ///
    /// Detection needs no shape configuration here. tract cannot shape-infer DBNet with a
    /// symbolic input H/W (the `Resize`-upsampled extent fails to unify against the FPN skip
    /// connection at `Concat`; see the Phase 0 spike note in
    /// `docs-site/src/content/docs/concepts/tract-inference.md`), so `xberg-paddle-ocr` builds
    /// a DBNet plan per page extent and caches it. Pinning to the page's own extent — rather
    /// than padding every page into one fixed canvas — is what keeps tract detection numerically
    /// equal to ORT: DBNet's squeeze-and-excitation blocks average over the whole input, so a
    /// padded canvas shifts the probability map across the entire page. `AngleNet`/`CrnnNet`
    /// stay unpinned; their graphs carry no dimension tract cannot resolve.
    #[cfg(feature = "paddle-ocr-tract")]
    fn init_engine_tract(
        ocr_engine: &mut PaddleOcrEngine,
        det_model_path: &str,
        cls_model_path: &str,
        rec_model_path: &str,
        dict_path: &str,
        accel: Option<&crate::core::config::acceleration::AccelerationConfig>,
    ) -> std::result::Result<(), xberg_paddle_ocr::OcrError> {
        if accel.is_some() {
            tracing::debug!(
                "PaddleOCR tract backend is CPU-only; ignoring the requested hardware acceleration provider"
            );
        }

        // Selected explicitly rather than via `init_models_with_dict`: that entry point takes
        // `xberg_paddle_ocr`'s compile-time default engine, which prefers `ort` whenever
        // `paddle-ocr-ort` is also compiled in. In a dual-engine build (native cross-engine
        // parity) the default would hand this function an `ort` engine, so a caller asking for
        // tract would silently get ORT.
        ocr_engine.init_models_with_dict_on(
            xberg_paddle_ocr::InferenceBackend::Tract,
            det_model_path,
            cls_model_path,
            rec_model_path,
            dict_path,
            INFERENCE_THREAD_COUNT,
        )
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
    fn detect_and_rotate(&self, image: &image::RgbImage) -> Result<RotationOutcome> {
        let detector = self.doc_ori_detector.get_or_try_init(|| {
            let cache_dir = self.config.resolve_cache_dir();
            Ok::<_, crate::XbergError>(crate::doc_orientation::DocOrientationDetector::with_acceleration(
                cache_dir,
                self.acceleration.clone(),
            ))
        })?;

        let orientation = detector.detect(image)?;
        tracing::debug!(
            degrees = orientation.degrees,
            confidence = orientation.confidence,
            "Document orientation detected for PaddleOCR"
        );
        rotate_for_detected_orientation(image, orientation)
    }

    /// Perform OCR on image bytes using the appropriate script family engine.
    async fn do_ocr(
        &self,
        image_bytes: &[u8],
        language: &str,
        effective_config: Arc<PaddleOcrConfig>,
        accel: Option<&crate::core::config::acceleration::AccelerationConfig>,
    ) -> Result<PaddlePageOcr> {
        let family = language_to_script_family(language);
        let engine = self
            .get_or_init_engine_for_family(family, Arc::clone(&effective_config), accel)
            .await?;

        let image_bytes_owned = image_bytes.to_vec();
        let config = effective_config;

        let (mut text_blocks, processed_width, processed_height) = tokio::task::spawn_blocking(move || {
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

        let vertical_cjk = Self::sort_vertical_cjk_blocks(&mut text_blocks, language);

        let mut line_elements = Vec::with_capacity(text_blocks.len());
        let mut word_elements = Vec::new();
        for block in &text_blocks {
            if let Some(group) = detailed_text_block_to_elements(block, 1)? {
                line_elements.push(group.line);
                word_elements.extend(group.words);
            }
        }

        let text = Self::assemble_block_text(&text_blocks, vertical_cjk);

        Ok(PaddlePageOcr {
            text,
            line_elements,
            word_elements,
            processed_width,
            processed_height,
        })
    }

    fn sort_vertical_cjk_blocks(blocks: &mut [xberg_paddle_ocr::DetailedTextBlock], language: &str) -> bool {
        if !matches!(language, "ch" | "chinese_cht" | "japan" | "korean") || blocks.is_empty() {
            return false;
        }

        let vertical_count = blocks
            .iter()
            .filter(|block| Self::is_vertical_text_block(block))
            .count();
        // Mixed pages need layout-region ordering; never move or fuse their horizontal content. ~keep
        if vertical_count != blocks.len() {
            return false;
        }

        let mut bounds = blocks.iter().filter_map(Self::block_bounds).collect::<Vec<_>>();
        bounds.sort_by_key(|(min_x, _, max_x, _)| std::cmp::Reverse(u64::from(*min_x) + u64::from(*max_x)));

        let mut columns: Vec<(u32, u32)> = Vec::new();
        for (min_x, _, max_x, _) in bounds {
            if let Some((column_min, column_max)) = columns.iter_mut().find(|(column_min, column_max)| {
                Self::ranges_share_vertical_column(min_x, max_x, *column_min, *column_max)
            }) {
                *column_min = (*column_min).min(min_x);
                *column_max = (*column_max).max(max_x);
            } else {
                columns.push((min_x, max_x));
            }
        }

        // Traditional CJK columns read right-to-left; fragments within a column read top-to-bottom. ~keep
        blocks.sort_by_key(|block| {
            let Some((min_x, min_y, max_x, _)) = Self::block_bounds(block) else {
                return (usize::MAX, u32::MAX);
            };
            let column = columns
                .iter()
                .position(|(column_min, column_max)| {
                    Self::ranges_share_vertical_column(min_x, max_x, *column_min, *column_max)
                })
                .unwrap_or(usize::MAX);
            (column, min_y)
        });
        true
    }

    fn assemble_block_text(blocks: &[xberg_paddle_ocr::DetailedTextBlock], compact_vertical: bool) -> String {
        // Detector blocks are visual lines, not paragraphs; Markdown keeps single newlines inside a paragraph. ~keep
        blocks
            .iter()
            .map(|block| block.block.text.as_str())
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join(if compact_vertical { "" } else { "\n" })
    }

    fn ranges_share_vertical_column(left_min: u32, left_max: u32, right_min: u32, right_max: u32) -> bool {
        let overlap = left_max.min(right_max).saturating_sub(left_min.max(right_min));
        let narrower_width = left_max
            .saturating_sub(left_min)
            .min(right_max.saturating_sub(right_min));
        narrower_width > 0 && overlap as f32 / narrower_width as f32 >= VERTICAL_COLUMN_MIN_OVERLAP_RATIO
    }

    fn is_vertical_text_block(block: &xberg_paddle_ocr::DetailedTextBlock) -> bool {
        let Some((min_x, min_y, max_x, max_y)) = Self::block_bounds(block) else {
            return false;
        };
        let width = max_x.saturating_sub(min_x);
        let height = max_y.saturating_sub(min_y);
        height as f32 >= width as f32 * VERTICAL_TEXT_MIN_ASPECT_RATIO
    }

    fn block_bounds(block: &xberg_paddle_ocr::DetailedTextBlock) -> Option<(u32, u32, u32, u32)> {
        let first = block.block.box_points.first()?;
        Some(block.block.box_points.iter().fold(
            (first.x, first.y, first.x, first.y),
            |(min_x, min_y, max_x, max_y), point| {
                (
                    min_x.min(point.x),
                    min_y.min(point.y),
                    max_x.max(point.x),
                    max_y.max(point.y),
                )
            },
        ))
    }

    /// Perform actual OCR inference (runs in blocking context).
    /// PaddleOcrEngine::detect takes `&self`; no mutex is needed for parallel page OCR.
    fn effective_rec_batch_size(config: &PaddleOcrConfig) -> u32 {
        config
            .rec_batch_num
            .clamp(MIN_RECOGNITION_BATCH_SIZE, MAX_RECOGNITION_BATCH_SIZE)
    }

    fn perform_ocr(
        image_bytes: &[u8],
        ocr_engine: &Arc<PaddleOcrEngine>,
        config: &PaddleOcrConfig,
    ) -> Result<(Vec<xberg_paddle_ocr::DetailedTextBlock>, u32, u32)> {
        let img = crate::extraction::image::load_image_for_ocr(image_bytes)
            .map_err(|e| crate::XbergError::Ocr {
                message: e.to_string(),
                source: None,
            })?
            .to_rgb8();
        let processed_width = img.width();
        let processed_height = img.height();

        let padding = config.padding;
        let max_side_len = config.det_limit_side_len;
        let box_score_thresh = config.det_db_box_thresh;
        let box_thresh = config.det_db_thresh;
        let un_clip_ratio = config.det_db_unclip_ratio;
        let do_angle = config.use_angle_cls;
        let most_angle = false;
        let rec_batch_size = Self::effective_rec_batch_size(config);

        let result = ocr_engine
            .detect_detailed_with_rec_batch_size(
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
            .filter(|block| block.block.text_score >= drop_score && !block.block.text_score.is_nan())
            .collect();

        tracing::debug!(text_block_count = text_blocks.len(), "PaddleOCR detection completed");

        Ok((text_blocks, processed_width, processed_height))
    }

    fn select_output_elements(
        lines: &[OcrElement],
        words: &[OcrElement],
        config: Option<&OcrElementConfig>,
    ) -> Vec<OcrElement> {
        let Some(config) = config.filter(|config| config.include_elements) else {
            return Vec::new();
        };
        let mut elements = match config.min_level {
            OcrElementLevel::Word => lines.iter().chain(words).cloned().collect::<Vec<_>>(),
            OcrElementLevel::Line => lines.to_vec(),
            OcrElementLevel::Block | OcrElementLevel::Page => Vec::new(),
        };
        elements.retain(|element| element.confidence.recognition >= config.min_confidence);
        elements
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
        let (paddle_lang, language_warnings) = super::select_paddle_language(&languages);

        let mut rotation_outcome = None;
        let ocr_image_bytes: Cow<'_, [u8]> = if config.auto_rotate {
            let decoded_image = crate::extraction::image::load_image_for_ocr(image_bytes)
                .map_err(|error| crate::XbergError::Ocr {
                    message: format!("Failed to decode PaddleOCR image for orientation detection: {error}"),
                    source: None,
                })?
                .to_rgb8();
            match self.detect_and_rotate(&decoded_image) {
                Ok(outcome) => {
                    rotation_outcome = Some(outcome);
                }
                Err(e) => {
                    tracing::warn!("Doc orientation detection failed, proceeding without rotation: {e}");
                    rotation_outcome = Some(RotationOutcome::unrotated(
                        decoded_image.width(),
                        decoded_image.height(),
                    ));
                }
            }
            match rotation_outcome
                .as_ref()
                .and_then(|outcome| outcome.rotated_bytes.as_deref())
            {
                Some(rotated) => Cow::Borrowed(rotated),
                None => Cow::Borrowed(image_bytes),
            }
        } else {
            Cow::Borrowed(image_bytes)
        };

        let effective_accel = self.resolve_acceleration(config.acceleration.as_ref());

        let PaddlePageOcr {
            text,
            line_elements,
            word_elements,
            processed_width,
            processed_height,
        } = self
            .do_ocr(
                &ocr_image_bytes,
                paddle_lang,
                Arc::clone(&effective_config),
                effective_accel.as_ref(),
            )
            .await?;
        let rotation_outcome =
            rotation_outcome.unwrap_or_else(|| RotationOutcome::unrotated(processed_width, processed_height));

        let text_blocks_count = line_elements.len();

        let ocr_doc = {
            use crate::types::extraction::BoundingBox;
            use crate::types::internal::{ElementKind, InternalDocument, InternalElement};
            use crate::types::ocr_elements::OcrElementLevel;

            let mut doc = InternalDocument::new("pdf");
            for elem in &line_elements {
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
            line_elements = line_elements.len(),
            word_elements = word_elements.len(),
            internal_doc_elements = ocr_doc.elements.len(),
            "PaddleOCR InternalDocument built"
        );

        let mut tables: Vec<Table> = vec![];
        let mut table_count = 0;
        let mut table_rows: Option<u32> = None;
        let mut table_cols: Option<u32> = None;

        if effective_config.enable_table_detection && !line_elements.is_empty() {
            let table_elements = line_elements.iter().chain(&word_elements).cloned().collect::<Vec<_>>();
            let words = elements_to_hocr_words(&table_elements, 0.3);

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
                language: paddle_lang.to_string(),
                psm: 3,
                output_format: "text".to_string(),
                table_count,
                table_rows,
                table_cols,
            })),
            additional: image_metadata(&rotation_outcome),
            ..Default::default()
        };

        let output_elements =
            Self::select_output_elements(&line_elements, &word_elements, config.element_config.as_ref());
        let ocr_elements_opt = if output_elements.is_empty() {
            None
        } else {
            Some(output_elements)
        };

        Ok(ExtractedDocument {
            content: text,
            mime_type: Cow::Borrowed("text/plain"),
            metadata,
            tables,
            detected_languages: Some(languages),
            ocr_elements: ocr_elements_opt,
            ocr_internal_document: Some(ocr_doc),
            processing_warnings: language_warnings,
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

    #[cfg_attr(alef, alef(skip))]
    fn probe(&self, config: &OcrConfig) -> crate::doctor::DoctorCheck {
        use crate::doctor::DoctorCheck;

        let effective_config: PaddleOcrConfig = match &config.paddle_ocr_config {
            Some(paddle_json) => match serde_json::from_value(paddle_json.clone()) {
                Ok(overridden) => overridden,
                Err(e) => {
                    return DoctorCheck::fail("ocr.paddle-ocr", format!("invalid paddle_ocr_config: {e}"));
                }
            },
            None => (*self.config).clone(),
        };

        let languages = config.effective_languages();
        let (paddle_lang, _warnings) = super::select_paddle_language(&languages);
        let family = language_to_script_family(paddle_lang);

        let manager = ModelManager::new(effective_config.resolve_cache_dir());
        match manager.check_models_cached(
            &effective_config.model_version,
            family,
            &effective_config.model_tier,
            config.auto_rotate,
        ) {
            Ok(artifacts) => {
                let missing: Vec<&str> = artifacts
                    .iter()
                    .filter(|(_, cached)| !cached)
                    .map(|(label, _)| label.as_str())
                    .collect();
                if missing.is_empty() {
                    DoctorCheck::pass(
                        "ocr.paddle-ocr",
                        format!("all models cached and verified ({family} recognition)"),
                    )
                } else {
                    DoctorCheck::skip(
                        "ocr.paddle-ocr",
                        format!(
                            "models not cached locally: {} (will download on first use)",
                            missing.join(", ")
                        ),
                    )
                }
            }
            Err(e) => DoctorCheck::fail("ocr.paddle-ocr", format!("{e}")),
        }
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
    use std::sync::Barrier;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::thread;

    const CONCURRENT_INITIALIZER_COUNT: usize = 8;

    #[test]
    fn engine_init_cell_initializes_a_key_once_across_threads() {
        let pool = Arc::new(Mutex::new(AHashMap::new()));
        let start = Arc::new(Barrier::new(CONCURRENT_INITIALIZER_COUNT));
        let initialization_count = Arc::new(AtomicUsize::new(0));

        let workers = (0..CONCURRENT_INITIALIZER_COUNT)
            .map(|_| {
                let pool = Arc::clone(&pool);
                let start = Arc::clone(&start);
                let initialization_count = Arc::clone(&initialization_count);
                thread::spawn(move || {
                    start.wait();
                    let cell = init_cell_for_key(&pool, "shared").expect("pool lock should be available");
                    *cell.get_or_init(|| {
                        initialization_count.fetch_add(1, Ordering::SeqCst);
                        42
                    })
                })
            })
            .collect::<Vec<_>>();

        let results = workers
            .into_iter()
            .map(|worker| worker.join().expect("initializer worker should not panic"))
            .collect::<Vec<_>>();

        assert_eq!(results, vec![42; CONCURRENT_INITIALIZER_COUNT]);
        assert_eq!(initialization_count.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn engine_init_cell_does_not_hold_pool_lock_during_initialization() {
        let pool = Mutex::new(AHashMap::new());
        let first = init_cell_for_key(&pool, "first").expect("pool lock should be available");

        let value = first.get_or_init(|| {
            let second = init_cell_for_key(&pool, "second").expect("another key should remain accessible");
            assert_eq!(second.set(2), Ok(()));
            1
        });

        assert_eq!(*value, 1);
        assert_eq!(pool.lock().expect("pool lock should be available").len(), 2);
    }

    #[test]
    fn engine_init_cell_retries_after_initialization_failure() {
        let pool = Mutex::new(AHashMap::new());
        let cell = init_cell_for_key(&pool, "retryable").expect("pool lock should be available");
        let initialization_count = AtomicUsize::new(0);

        let first: std::result::Result<&usize, &str> = cell.get_or_try_init(|| {
            initialization_count.fetch_add(1, Ordering::SeqCst);
            Err("initialization failed")
        });
        let second = cell.get_or_try_init(|| {
            initialization_count.fetch_add(1, Ordering::SeqCst);
            Ok::<usize, &str>(42)
        });

        assert_eq!(first, Err("initialization failed"));
        assert_eq!(second, Ok(&42));
        assert_eq!(initialization_count.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn engine_pool_key_distinguishes_gpu_devices() {
        use crate::core::config::acceleration::{AccelerationConfig, ExecutionProviderType};

        let first_gpu = AccelerationConfig {
            provider: ExecutionProviderType::Cuda,
            device_id: 0,
        };
        let second_gpu = AccelerationConfig {
            provider: ExecutionProviderType::Cuda,
            device_id: 1,
        };

        assert_eq!(
            engine_pool_key("v6", "small", "latin", None, PaddleInferenceBackend::Ort),
            "v6/small/latin/cpu/ort"
        );
        assert_ne!(
            engine_pool_key("v6", "small", "latin", Some(&first_gpu), PaddleInferenceBackend::Ort),
            engine_pool_key("v6", "small", "latin", Some(&second_gpu), PaddleInferenceBackend::Ort)
        );
    }

    #[test]
    fn engine_pool_key_distinguishes_inference_backends() {
        assert_ne!(
            engine_pool_key("v6", "small", "latin", None, PaddleInferenceBackend::Ort),
            engine_pool_key("v6", "small", "latin", None, PaddleInferenceBackend::Tract)
        );
    }

    fn detailed_block(text: &str, left: u32, top: u32, width: u32, height: u32) -> xberg_paddle_ocr::DetailedTextBlock {
        xberg_paddle_ocr::DetailedTextBlock {
            block: xberg_paddle_ocr::TextBlock {
                box_points: vec![
                    xberg_paddle_ocr::Point { x: left, y: top },
                    xberg_paddle_ocr::Point {
                        x: left + width,
                        y: top,
                    },
                    xberg_paddle_ocr::Point {
                        x: left + width,
                        y: top + height,
                    },
                    xberg_paddle_ocr::Point {
                        x: left,
                        y: top + height,
                    },
                ],
                box_score: 0.9,
                angle_index: 0,
                angle_score: 1.0,
                text: text.to_string(),
                text_score: 0.9,
            },
            words: Vec::new(),
            line_column_count: 0.0,
            rotation_retained: false,
        }
    }

    fn output_element(text: &str, level: OcrElementLevel, confidence: f64) -> OcrElement {
        OcrElement::new(
            text,
            crate::types::OcrBoundingGeometry::Rectangle {
                left: 0,
                top: 0,
                width: 10,
                height: 10,
            },
            crate::types::OcrConfidence::from_tesseract(confidence * 100.0),
        )
        .with_level(level)
    }

    #[test]
    fn default_paddle_element_granularity_remains_line_only() {
        let lines = [output_element("line", OcrElementLevel::Line, 0.9)];
        let words = [output_element("word", OcrElementLevel::Word, 0.9)];
        let config = OcrElementConfig {
            include_elements: true,
            ..Default::default()
        };

        let selected = PaddleOcrBackend::select_output_elements(&lines, &words, Some(&config));

        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].text, "line");
    }

    #[test]
    fn word_granularity_exposes_hierarchy_and_filters_confidence() {
        let lines = [output_element("line", OcrElementLevel::Line, 0.9)];
        let words = [
            output_element("kept", OcrElementLevel::Word, 0.8),
            output_element("dropped", OcrElementLevel::Word, 0.4),
        ];
        let config = OcrElementConfig {
            include_elements: true,
            min_level: OcrElementLevel::Word,
            min_confidence: 0.5,
            build_hierarchy: false,
        };

        let selected = PaddleOcrBackend::select_output_elements(&lines, &words, Some(&config));

        assert_eq!(
            selected.iter().map(|element| element.text.as_str()).collect::<Vec<_>>(),
            ["line", "kept"]
        );
    }

    #[test]
    fn vertical_japanese_columns_are_ordered_right_to_left() {
        let mut blocks = vec![
            detailed_block("left", 10, 0, 10, 100),
            detailed_block("right", 50, 0, 10, 100),
            detailed_block("middle", 30, 0, 10, 100),
        ];

        let vertical = PaddleOcrBackend::sort_vertical_cjk_blocks(&mut blocks, "japan");

        assert!(vertical);
        assert_eq!(
            blocks.iter().map(|block| block.block.text.as_str()).collect::<Vec<_>>(),
            ["right", "middle", "left"]
        );
        assert_eq!(
            PaddleOcrBackend::assemble_block_text(&blocks, vertical),
            "rightmiddleleft"
        );
    }

    #[test]
    fn vertical_column_fragments_are_ordered_top_to_bottom_despite_x_jitter() {
        let mut blocks = vec![
            detailed_block("left", 10, 0, 10, 100),
            detailed_block("right-bottom", 51, 60, 10, 50),
            detailed_block("right-top", 50, 0, 10, 50),
        ];

        let vertical = PaddleOcrBackend::sort_vertical_cjk_blocks(&mut blocks, "japan");

        assert!(vertical);
        assert_eq!(
            blocks.iter().map(|block| block.block.text.as_str()).collect::<Vec<_>>(),
            ["right-top", "right-bottom", "left"]
        );
    }

    #[test]
    fn horizontal_japanese_lines_keep_detector_order() {
        let mut blocks = vec![
            detailed_block("first", 50, 0, 100, 10),
            detailed_block("second", 10, 20, 100, 10),
        ];

        let vertical = PaddleOcrBackend::sort_vertical_cjk_blocks(&mut blocks, "japan");

        assert!(!vertical);
        assert_eq!(
            blocks.iter().map(|block| block.block.text.as_str()).collect::<Vec<_>>(),
            ["first", "second"]
        );
    }

    #[test]
    fn horizontal_lines_remain_within_one_markdown_paragraph() {
        let blocks = vec![
            detailed_block("first visual line", 0, 0, 100, 10),
            detailed_block("second visual line", 0, 20, 100, 10),
        ];

        assert_eq!(
            PaddleOcrBackend::assemble_block_text(&blocks, false),
            "first visual line\nsecond visual line"
        );
    }

    #[test]
    fn mixed_japanese_layout_keeps_detector_order_and_separators() {
        let mut blocks = vec![
            detailed_block("title", 0, 0, 100, 10),
            detailed_block("right", 50, 20, 10, 100),
            detailed_block("left", 10, 20, 10, 100),
        ];

        let vertical = PaddleOcrBackend::sort_vertical_cjk_blocks(&mut blocks, "japan");

        assert!(!vertical);
        assert_eq!(
            blocks.iter().map(|block| block.block.text.as_str()).collect::<Vec<_>>(),
            ["title", "right", "left"]
        );
        assert_eq!(
            PaddleOcrBackend::assemble_block_text(&blocks, vertical),
            "title\nright\nleft"
        );
    }

    #[test]
    fn non_cjk_vertical_lines_keep_detector_order() {
        let mut blocks = vec![
            detailed_block("left", 10, 0, 10, 100),
            detailed_block("right", 50, 0, 10, 100),
        ];

        let vertical = PaddleOcrBackend::sort_vertical_cjk_blocks(&mut blocks, "en");

        assert!(!vertical);
        assert_eq!(
            blocks.iter().map(|block| block.block.text.as_str()).collect::<Vec<_>>(),
            ["left", "right"]
        );
    }

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
    fn test_unrotated_image_metadata_uses_original_dimensions() {
        let image = image::RgbImage::new(3, 2);
        let outcome = rotate_for_detected_orientation(
            &image,
            crate::doc_orientation::OrientationResult {
                degrees: 0,
                confidence: 1.0,
            },
        )
        .expect("zero-degree orientation should not require a model or fail");

        assert!(outcome.rotated_bytes.is_none());
        assert_eq!((outcome.processed_width, outcome.processed_height), (3, 2));

        let metadata = image_metadata(&outcome);
        assert_eq!(
            metadata.get(crate::ocr_metadata_keys::OCR_PROCESSED_IMAGE_WIDTH_METADATA_KEY),
            Some(&serde_json::json!(3))
        );
        assert_eq!(
            metadata.get(crate::ocr_metadata_keys::OCR_PROCESSED_IMAGE_HEIGHT_METADATA_KEY),
            Some(&serde_json::json!(2))
        );
        assert_eq!(
            metadata.get(crate::ocr_metadata_keys::OCR_ORIENTATION_DEGREES_METADATA_KEY),
            Some(&serde_json::json!(0))
        );
        assert!(!metadata.contains_key(crate::ocr_metadata_keys::OCR_AUTO_ROTATED_METADATA_KEY));
    }

    #[test]
    fn test_rotated_image_metadata_and_geometry_use_corrected_space() {
        let mut image = image::RgbImage::new(3, 2);
        let marker = image::Rgb([17, 31, 47]);
        image.put_pixel(0, 0, marker);

        let outcome = rotate_for_detected_orientation(
            &image,
            crate::doc_orientation::OrientationResult {
                degrees: 90,
                confidence: 1.0,
            },
        )
        .expect("in-memory rotation should succeed");

        assert_eq!((outcome.processed_width, outcome.processed_height), (2, 3));
        let rotated = image::load_from_memory(outcome.rotated_bytes.as_deref().expect("rotation should produce bytes"))
            .expect("rotated PNG should decode")
            .to_rgb8();
        assert_eq!(rotated.dimensions(), (2, 3));
        assert_eq!(*rotated.get_pixel(0, 2), marker);

        let metadata = image_metadata(&outcome);
        assert_eq!(
            metadata.get(crate::ocr_metadata_keys::OCR_PROCESSED_IMAGE_WIDTH_METADATA_KEY),
            Some(&serde_json::json!(2))
        );
        assert_eq!(
            metadata.get(crate::ocr_metadata_keys::OCR_PROCESSED_IMAGE_HEIGHT_METADATA_KEY),
            Some(&serde_json::json!(3))
        );
        assert_eq!(
            metadata.get(crate::ocr_metadata_keys::OCR_ORIENTATION_DEGREES_METADATA_KEY),
            Some(&serde_json::json!(90))
        );
        assert_eq!(
            metadata.get(crate::ocr_metadata_keys::OCR_AUTO_ROTATED_METADATA_KEY),
            Some(&serde_json::json!(true))
        );
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

    #[cfg(feature = "paddle-ocr-ort")]
    #[test]
    fn paddle_acceleration_guard_restores_worker_state() {
        use crate::core::config::AccelerationConfig;

        let cpu_accel = AccelerationConfig {
            provider: crate::core::config::acceleration::ExecutionProviderType::Cpu,
            device_id: 0,
        };
        let cuda_accel = AccelerationConfig {
            provider: crate::core::config::acceleration::ExecutionProviderType::Cuda,
            device_id: 1,
        };
        PADDLE_TL_ACCEL.with(|cell| {
            cell.replace(Some(cpu_accel.clone()));
        });

        {
            let _guard = PaddleAccelerationGuard::set(Some(cuda_accel));
            let provider = PADDLE_TL_ACCEL.with(|cell| cell.borrow().as_ref().map(|config| config.provider.clone()));
            assert_eq!(
                provider,
                Some(crate::core::config::acceleration::ExecutionProviderType::Cuda)
            );
        }

        let restored = PADDLE_TL_ACCEL.with(|cell| cell.borrow().clone());
        assert_eq!(
            restored,
            Some(cpu_accel),
            "blocking-pool threads must not retain another request's acceleration"
        );

        PADDLE_TL_ACCEL.with(|cell| {
            cell.replace(None);
        });
    }
}
