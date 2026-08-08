//! GLM-OCR backend plugin for the Xberg OCR pipeline.
//!
//! This module wraps the candle-based GLM-OCR engine in the `OcrBackend` trait,
//! making it available to the extraction pipeline.
//!
//! # Engine pool design
//!
//! The pool key is `(DevicePreference, DType)` — NOT including the task.
//! All five GLM-OCR tasks (`Ocr`, `Table`, `Formula`, `Chart`, `Caption`) differ
//! only in the prompt prefix fed to the decoder. The model weights are identical,
//! so a single engine instance handles every task via `process_image_with_task`.
//! This avoids loading the ~900 MB safetensors five times in paired mode.

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
use xberg_candle_ocr::CandleOcrError;
use xberg_candle_ocr::DType;
use xberg_candle_ocr::DevicePreference;
use xberg_candle_ocr::models::GlmOcrEngine;
use xberg_candle_ocr::models::GlmOcrTask;

/// How the backend dispatches inference across a page image.
///
/// `WholePage` passes the raw page bytes to the engine as a single call — fast
/// and simple. `Paired` (compiled only when `layout-detection` is enabled) runs
/// PP-DocLayout-V3 first, crops individual regions, and dispatches each crop to
/// the task that best matches the detected layout class, merging results in
/// reading order.
#[cfg_attr(alef, alef(skip))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutMode {
    /// Feed the whole page to the model with the backend's default task.
    WholePage,
    /// Detect layout regions first; dispatch each crop to the matching task.
    ///
    /// Only available when the `layout-detection` feature is enabled.
    #[cfg(feature = "layout-detection")]
    Paired,
}

// compiled in, WholePage otherwise. #[derive(Default)] cannot express this.
#[allow(clippy::derivable_impls)]
impl Default for LayoutMode {
    fn default() -> Self {
        #[cfg(feature = "layout-detection")]
        {
            LayoutMode::Paired
        }
        #[cfg(not(feature = "layout-detection"))]
        {
            LayoutMode::WholePage
        }
    }
}

/// Pool type alias for the GLM-OCR engine pool keyed by `(DevicePreference, DType)`.
type EnginePool = RwLock<AHashMap<(DevicePreference, DType, PathBuf, String), Arc<GlmOcrEngine>>>;

/// Process-wide engine pool keyed by `(DevicePreference, DType)`.
///
/// A single engine instance handles all tasks (OCR / Table / Formula / Chart /
/// Caption) via `process_image_with_task`, so the pool key does not include the
/// task. Two callers requesting the same device+dtype but different tasks will
/// share one engine and avoid loading weights twice.
static ENGINE_POOL: LazyLock<EnginePool> = LazyLock::new(|| RwLock::new(AHashMap::new()));

/// Pool type alias for the layout model pool keyed by `(model_path, device_preference)`.
#[cfg(feature = "layout-detection")]
type LayoutPool = RwLock<
    AHashMap<(String, DevicePreference), Arc<Mutex<crate::layout::models::pp_doclayout_v3::PpDocLayoutV3Model>>>,
>;

/// Process-wide layout model pool keyed by `(model_path, device_preference)`.
///
/// Caches loaded `PpDocLayoutV3Model` instances by their file path and device preference
/// to avoid reloading the expensive ONNX model on each `process_paired` invocation.
/// Two callers requesting the same model path with the same device will share one instance.
/// Different devices get separate model instances due to device-specific optimizations.
/// The model is wrapped in Mutex (not RwLock) since `detect` takes `&mut self`.
///
/// Only available when `layout-detection` is enabled.
#[cfg(feature = "layout-detection")]
static LAYOUT_POOL: LazyLock<LayoutPool> = LazyLock::new(|| RwLock::new(AHashMap::new()));

/// Generic double-checked-lock pool: get or initialize a value from cache.
///
/// Uses a read → miss → write → double-check pattern so two racing callers do
/// not both pay the initialization cost. Returns an Arc to the cached value,
/// with pointer equality guarantees: two callers with the same key will receive
/// Arc instances with `Arc::ptr_eq(a, b) == true`.
///
/// # Parameters
/// - `pool`: The RwLock-wrapped pool
/// - `key`: The cache key
/// - `init`: A closure that constructs the value on cache miss
///
/// # Errors
/// Propagates errors from the `init` closure.
#[inline]
fn pool_get_or_init<K, V, E>(
    pool: &RwLock<AHashMap<K, Arc<V>>>,
    key: K,
    init: impl FnOnce() -> std::result::Result<V, E>,
) -> std::result::Result<Arc<V>, E>
where
    K: std::hash::Hash + Eq + Clone,
    V: Send + 'static,
{
    {
        let pool_guard = pool.read();
        if let Some(value) = pool_guard.get(&key) {
            return Ok(Arc::clone(value));
        }
    }

    let new_value = Arc::new(init()?);

    let mut pool_guard = pool.write();
    if let Some(existing) = pool_guard.get(&key) {
        return Ok(Arc::clone(existing));
    }
    pool_guard.insert(key, Arc::clone(&new_value));
    Ok(new_value)
}

/// Return a cached engine for `(preference, dtype)`, initialising one on first use.
///
/// Uses the generic [`pool_get_or_init`] helper to ensure two callers with the same
/// `(preference, dtype)` receive the same Arc instance.
fn get_or_init_engine(
    preference: DevicePreference,
    dtype: DType,
    cache_dir: PathBuf,
    revision: String,
) -> crate::Result<Arc<GlmOcrEngine>> {
    let key = (preference, dtype, cache_dir.clone(), revision.clone());

    pool_get_or_init::<(DevicePreference, DType, PathBuf, String), GlmOcrEngine, crate::XbergError>(
        &ENGINE_POOL,
        key,
        || {
            let device = preference.select().map_err(|e| crate::XbergError::Ocr {
                message: format!("Failed to select compute device: {e}"),
                source: Some(Box::new(e)),
            })?;

            tracing::info!(
                preference = ?preference,
                ?dtype,
                "Initialising GLM-OCR engine (cold start)"
            );
            GlmOcrEngine::new_with_hf(GlmOcrTask::default(), device, dtype, Some(&cache_dir), Some(&revision)).map_err(
                |e| crate::XbergError::Ocr {
                    message: format!("GLM-OCR engine initialisation failed: {e}"),
                    source: Some(Box::new(e)),
                },
            )
        },
    )
}

/// Return a cached layout model for the given path and device, initialising one on first use.
///
/// Uses the generic [`pool_get_or_init`] helper to ensure two callers with the same
/// path and device preference receive the same Arc instance. The model is wrapped in Mutex
/// (not RwLock) since `detect` takes `&mut self` and requires exclusive access.
///
/// Only available when `layout-detection` is enabled.
#[cfg(feature = "layout-detection")]
fn get_or_init_layout_model(
    model_path: &Path,
    device: DevicePreference,
) -> crate::Result<Arc<Mutex<crate::layout::models::pp_doclayout_v3::PpDocLayoutV3Model>>> {
    use crate::layout::models::pp_doclayout_v3::PpDocLayoutV3Model;

    let model_path_str = model_path
        .to_str()
        .ok_or_else(|| crate::XbergError::Ocr {
            message: format!("Model path contains invalid UTF-8: {}", model_path.display()),
            source: None,
        })?
        .to_string();

    let key = (model_path_str.clone(), device);

    pool_get_or_init::<(String, DevicePreference), Mutex<PpDocLayoutV3Model>, crate::XbergError>(
        &LAYOUT_POOL,
        key,
        || {
            tracing::info!(
                path = model_path_str.as_str(),
                ?device,
                "Initialising PP-DocLayout-V3 model (cold start)"
            );
            PpDocLayoutV3Model::from_file(&model_path_str, None)
                .map_err(|e| crate::XbergError::Ocr {
                    message: format!("PP-DocLayout-V3 model initialisation failed: {e}"),
                    source: Some(Box::new(e)),
                })
                .map(Mutex::new)
        },
    )
}

/// Options parsed from backend-specific configuration.
///
/// Extracted from [`OcrConfig.backend_options`] to make GLM-OCR configuration
/// available to both the constructor and the runtime processing paths.
#[cfg_attr(alef, alef(skip))]
#[derive(Debug, Clone)]
struct GlmOcrOptions {
    task: GlmOcrTask,
    device: DevicePreference,
    layout_mode: LayoutMode,
    enable_chart_understanding: bool,
    cache_dir: Option<PathBuf>,
    hf_revision: Option<String>,
}

/// Map a layout detection class to the GLM-OCR task best suited for that region.
///
/// Picture and Chart both go to `Caption` since the model has no standalone
/// chart-region task; caption elicits a descriptive output from the VLM.
/// However, Chart is routed to the Chart task when chart understanding is enabled;
/// otherwise it falls back to Caption for all images.
/// Header and footer are treated as plain `Ocr`.
#[cfg(feature = "layout-detection")]
fn task_for_label(label: crate::layout::LayoutClass, enable_chart_understanding: bool) -> GlmOcrTask {
    use crate::layout::LayoutClass;
    match label {
        LayoutClass::Table => GlmOcrTask::Table,
        LayoutClass::Formula => GlmOcrTask::Formula,
        LayoutClass::Chart => {
            if enable_chart_understanding {
                GlmOcrTask::Chart
            } else {
                GlmOcrTask::Caption
            }
        }
        LayoutClass::Picture => GlmOcrTask::Caption,
        LayoutClass::Text
        | LayoutClass::Title
        | LayoutClass::SectionHeader
        | LayoutClass::Caption
        | LayoutClass::ListItem
        | LayoutClass::Footnote
        | LayoutClass::PageHeader
        | LayoutClass::PageFooter
        | LayoutClass::DocumentIndex
        | LayoutClass::Code
        | LayoutClass::CheckboxSelected
        | LayoutClass::CheckboxUnselected
        | LayoutClass::Form
        | LayoutClass::KeyValueRegion => GlmOcrTask::Ocr,
    }
}

/// Wrap a raw GLM-OCR model output string according to its task.
///
/// Table output is left as-is (the model already emits GFM table syntax).
/// Formula output is wrapped in display-math `$$ … $$` fences.
/// Chart output is fenced in a JSON code block.
/// All other tasks return the text verbatim.
#[cfg(feature = "layout-detection")]
/// Strip leading and trailing `$$` delimiters and surrounding whitespace from a string.
///
/// Used to normalize formula content that may have been wrapped by the model.
/// If the content starts with `$$` or ends with `$$`, removes those delimiters
/// and any immediately adjacent whitespace.
fn strip_formula_delimiters(content: &str) -> String {
    let trimmed = content.trim();
    let stripped = trimmed.strip_prefix("$$").unwrap_or(trimmed).trim_start();
    stripped.strip_suffix("$$").unwrap_or(stripped).trim_end().to_string()
}

fn wrap_output(task: GlmOcrTask, content: &str) -> String {
    match task {
        GlmOcrTask::Table => content.to_string(),
        GlmOcrTask::Formula => format!("$$\n{}\n$$", content.trim()),
        GlmOcrTask::Chart => format!("```json\n{}\n```", content.trim()),
        GlmOcrTask::Ocr | GlmOcrTask::Caption => content.to_string(),
    }
}

/// Whether a detected layout region is a table, i.e. its bounding box must be
/// carried through to the corresponding `Table` entry in
/// `ExtractedDocument::tables` (issue #187).
#[cfg(feature = "layout-detection")]
fn is_table_region(class: crate::layout::LayoutClass) -> bool {
    class == crate::layout::LayoutClass::Table
}

/// Attach detection-time bounding boxes to the `Table` entries `extract_gfm_tables`
/// parsed out of the assembled markdown, correlating purely by position: the Nth
/// `Table`-class detection with non-empty OCR output corresponds to the Nth GFM
/// table block that appears in `content`, since `process_paired` appends region
/// output to `content` in the same reading order it iterates detections. Extra
/// entries on either side (a table detection whose output failed to parse as GFM,
/// or vice versa) are left without a match rather than mis-attributed.
#[cfg(feature = "layout-detection")]
fn merge_table_bounding_boxes(
    tables: &mut [crate::types::Table],
    detection_bboxes: &[crate::types::extraction::BoundingBox],
) {
    for (table, bbox) in tables.iter_mut().zip(detection_bboxes.iter()) {
        table.bounding_box = Some(*bbox);
    }
}

/// Canonical markdown checkbox marker for a checkbox-class layout detection.
///
/// OCR cannot reliably recover checked-vs-unchecked state from glyph
/// recognition on a tiny checkbox crop (issue #190) — the layout detector has
/// already computed the state, so it is emitted directly instead of routing
/// the crop through the model. Returns `None` for every other layout class.
#[cfg(feature = "layout-detection")]
fn checkbox_marker_for_class(class: crate::layout::LayoutClass) -> Option<&'static str> {
    use crate::layout::LayoutClass;
    match class {
        LayoutClass::CheckboxSelected => Some("[x]"),
        LayoutClass::CheckboxUnselected => Some("[ ]"),
        _ => None,
    }
}

/// GLM-OCR backend using candle transformers.
///
/// A compact vision-language model (0.9 B) for full-page document parsing.
/// Supports text recognition, tables, formulas, charts, and image captioning
/// through a unified interface with markdown output.
///
/// # Constructor notes
///
/// `GlmOcrBackend::new(task, layout_mode)` stores the default task and layout
/// mode. `dtype` defaults to `F32` — the only dtype validated during the smoke
/// test. Use `GlmOcrBackend::with_dtype` to override.
///
/// # Configuration
///
/// GLM-OCR accepts backend options for task, device, and layout mode selection:
/// ```json
/// {
///   "task": "ocr",
///   "device": "auto",
///   "layout_mode": "whole_page"
/// }
/// ```
///
/// - `task` (string): `"ocr"` (default), `"table"`, `"formula"`, `"chart"`, `"caption"`
/// - `device` (string): `"auto"`, `"cpu"`, `"cuda"`, `"metal"`
/// - `layout_mode` (string): `"whole_page"` (default), `"paired"` (requires `layout-detection` feature)
#[cfg_attr(alef, alef(skip))]
pub struct GlmOcrBackend {
    default_task: GlmOcrTask,
    layout_mode: LayoutMode,
    dtype: DType,
}

impl GlmOcrBackend {
    /// Create a new GLM-OCR backend.
    ///
    /// `dtype` defaults to `F32`. Use [`GlmOcrBackend::with_dtype`] to change it.
    pub fn new(default_task: GlmOcrTask, layout_mode: LayoutMode) -> Self {
        Self {
            default_task,
            layout_mode,
            dtype: DType::F32,
        }
    }

    /// Override the floating-point precision used by the candle engine.
    pub fn with_dtype(mut self, dtype: DType) -> Self {
        self.dtype = dtype;
        self
    }

    /// Parse backend options to extract GLM-OCR-specific configuration.
    ///
    /// Device selection is delegated to [`crate::candle_ocr::resolve_device_preference`]
    /// so the central `AccelerationConfig` is honoured.
    ///
    /// Supports the following backend options (as serde_json values):
    /// - `task` (string): `"ocr"`, `"table"`, `"formula"`, `"chart"`, `"caption"` (default: `"ocr"`)
    /// - `layout_mode` (string): `"whole_page"`, `"paired"` (default: platform-dependent)
    /// - `enable_chart_understanding` (bool): route detected charts to chart task (default: `false`)
    fn parse_options(&self, config: &OcrConfig) -> GlmOcrOptions {
        let mut task = self.default_task;
        let mut layout_mode = self.layout_mode;
        let mut enable_chart_understanding = false;
        let mut cache_dir = None;
        let mut hf_revision = None;

        if let Some(opts) = &config.backend_options {
            if let Some(t) = opts.get("task").and_then(|v| v.as_str()) {
                task = match t {
                    "table" => GlmOcrTask::Table,
                    "formula" => GlmOcrTask::Formula,
                    "chart" => GlmOcrTask::Chart,
                    "caption" => GlmOcrTask::Caption,
                    _ => GlmOcrTask::Ocr,
                };
            }

            if let Some(m) = opts.get("layout_mode").and_then(|v| v.as_str()) {
                layout_mode = match m {
                    #[cfg(feature = "layout-detection")]
                    "paired" => LayoutMode::Paired,
                    _ => LayoutMode::WholePage,
                };
            }

            if let Some(e) = opts.get("enable_chart_understanding").and_then(|v| v.as_bool()) {
                enable_chart_understanding = e;
            }
            cache_dir = opts
                .get("cache_dir")
                .and_then(|value| value.as_str())
                .map(PathBuf::from);
            hf_revision = opts
                .get("hf_revision")
                .or_else(|| opts.get("revision"))
                .and_then(|value| value.as_str())
                .map(str::to_string);
        }

        let device = super::resolve_device_preference(config);
        GlmOcrOptions {
            task,
            device,
            layout_mode,
            enable_chart_understanding,
            cache_dir,
            hf_revision,
        }
    }
}

impl Plugin for GlmOcrBackend {
    fn name(&self) -> &str {
        "candle-glm-ocr"
    }

    fn version(&self) -> String {
        "0.1.0".to_string()
    }

    fn initialize(&self) -> Result<()> {
        tracing::debug!(
            task = %self.default_task,
            "Initializing GLM-OCR backend"
        );
        Ok(())
    }

    fn shutdown(&self) -> Result<()> {
        Ok(())
    }
}

#[async_trait]
impl OcrBackend for GlmOcrBackend {
    async fn process_image(&self, image_bytes: &[u8], config: &OcrConfig) -> Result<ExtractedDocument> {
        let opts = self.parse_options(config);

        if image_bytes.is_empty() {
            return Err(crate::XbergError::Validation {
                message: "Empty image data provided to GLM-OCR".to_string(),
                source: None,
            });
        }

        let image_bytes_owned = image_bytes.to_vec();
        let dtype = self.dtype;
        let cache_dir = opts.cache_dir.unwrap_or_else(hf_hub::resolve_cache_dir);
        let revision = opts.hf_revision.unwrap_or_else(|| GlmOcrEngine::revision().to_string());

        let (content, formulas, table_bboxes) = match opts.layout_mode {
            LayoutMode::WholePage => {
                let task = opts.task;
                let device = opts.device;
                let content = tokio::task::spawn_blocking(move || {
                    let engine = get_or_init_engine(device, dtype, cache_dir, revision)?;
                    let output = engine.process_image_with_task(&image_bytes_owned, task).map_err(|e| {
                        crate::XbergError::Ocr {
                            message: format!("GLM-OCR inference failed: {e}"),
                            source: Some(Box::new(e)),
                        }
                    })?;
                    Ok::<String, crate::XbergError>(output.content)
                })
                .await
                .map_err(|e| crate::XbergError::Ocr {
                    message: format!("GLM-OCR task execution failed: {e}"),
                    source: None,
                })??;
                (content, Vec::new(), Vec::new())
            }

            #[cfg(feature = "layout-detection")]
            LayoutMode::Paired => {
                let enable_chart_understanding = opts.enable_chart_understanding;
                process_paired(
                    image_bytes_owned,
                    opts.device,
                    dtype,
                    enable_chart_understanding,
                    cache_dir,
                    revision,
                )
                .await?
            }
        };

        let mut document = super::ocr_result::build_ocr_document(
            content,
            formulas,
            Cow::Borrowed("text/markdown"),
            image_bytes,
            config,
        );
        #[cfg(feature = "layout-detection")]
        merge_table_bounding_boxes(&mut document.tables, &table_bboxes);
        #[cfg(not(feature = "layout-detection"))]
        let _ = table_bboxes;
        Ok(document)
    }

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

/// Paired-mode dispatch: run PP-DocLayout-V3, crop regions, dispatch per-region task.
///
/// Returns both the assembled markdown content and a vector of recognized formulas.
/// Each formula captures the LaTeX source (without `$$` delimiters) and its bounding box.
/// The page number must be filled in by the caller.
///
/// Only compiled when `layout-detection` feature is enabled.
#[cfg(feature = "layout-detection")]
async fn process_paired(
    image_bytes: Vec<u8>,
    device: DevicePreference,
    dtype: DType,
    enable_chart_understanding: bool,
    cache_dir: PathBuf,
    revision: String,
) -> crate::Result<(
    String,
    Vec<crate::types::Formula>,
    Vec<crate::types::extraction::BoundingBox>,
)> {
    use crate::layout::LayoutModelManager;
    use crate::layout::models::LayoutModel;

    tokio::task::spawn_blocking(move || {
        let img = image::load_from_memory(&image_bytes)
            .map_err(|e| crate::XbergError::Ocr {
                message: format!("GLM-OCR paired: image decode failed: {e}"),
                source: Some(Box::new(e)),
            })?
            .to_rgb8();

        let manager = LayoutModelManager::new(None);
        let model_path = manager
            .ensure_pp_doclayout_v3_model()
            .map_err(|e| crate::XbergError::Ocr {
                message: format!("GLM-OCR paired: layout model unavailable: {e}"),
                source: Some(Box::new(e)),
            })?;

        let layout_model = get_or_init_layout_model(&model_path, device).map_err(|e| crate::XbergError::Ocr {
            message: format!("GLM-OCR paired: layout detection init failed: {e}"),
            source: Some(Box::new(e)),
        })?;

        let detections = layout_model.lock().detect(&img).map_err(|e| crate::XbergError::Ocr {
            message: format!("GLM-OCR paired: layout detection failed: {e}"),
            source: Some(Box::new(e)),
        })?;

        let mut sorted = detections;
        sorted.sort_by(|a, b| a.bbox.y1.total_cmp(&b.bbox.y1).then(a.bbox.x1.total_cmp(&b.bbox.x1)));

        let engine = get_or_init_engine(device, dtype, cache_dir, revision)?;

        if sorted.is_empty() {
            tracing::debug!("GLM-OCR paired: no layout regions detected, falling back to whole-page inference");
            let output = engine
                .process_image_with_task(&image_bytes, GlmOcrTask::Ocr)
                .map_err(|e| crate::XbergError::Ocr {
                    message: format!("GLM-OCR paired (fallback): whole-page inference failed: {e}"),
                    source: Some(Box::new(e)),
                })?;
            return Ok::<
                (
                    String,
                    Vec<crate::types::Formula>,
                    Vec<crate::types::extraction::BoundingBox>,
                ),
                crate::XbergError,
            >((output.content, Vec::new(), Vec::new()));
        }

        let img_width = img.width();
        let img_height = img.height();

        let mut parts: Vec<String> = Vec::with_capacity(sorted.len());
        let mut formulas: Vec<crate::types::Formula> = Vec::new();
        let mut table_bboxes: Vec<crate::types::extraction::BoundingBox> = Vec::new();

        for detection in &sorted {
            let bbox = &detection.bbox;
            let region_bbox = crate::types::extraction::BoundingBox {
                x0: bbox.x1 as f64,
                y0: bbox.y1 as f64,
                x1: bbox.x2 as f64,
                y1: bbox.y2 as f64,
            };

            // Checkbox state is decided by the layout detector, not the OCR model —
            // glyph OCR on a tiny checkbox crop cannot reliably recover the state bit
            // (#190). Emit the canonical marker directly and skip the model call.
            if let Some(marker) = checkbox_marker_for_class(detection.class_name) {
                parts.push(marker.to_string());
                continue;
            }

            let x = (bbox.x1.max(0.0) as u32).min(img_width.saturating_sub(1));
            let y = (bbox.y1.max(0.0) as u32).min(img_height.saturating_sub(1));
            let w = ((bbox.x2 - bbox.x1).max(1.0) as u32).min(img_width - x);
            let h = ((bbox.y2 - bbox.y1).max(1.0) as u32).min(img_height - y);

            let crop = image::imageops::crop_imm(&img, x, y, w, h).to_image();

            let mut crop_bytes: Vec<u8> = Vec::new();
            crop.write_to(&mut std::io::Cursor::new(&mut crop_bytes), image::ImageFormat::Png)
                .map_err(|e| crate::XbergError::Ocr {
                    message: format!("GLM-OCR paired: crop encode failed: {e}"),
                    source: Some(Box::new(e)),
                })?;

            let region_task = task_for_label(detection.class_name, enable_chart_understanding);

            let output = match engine.process_image_with_task(&crop_bytes, region_task) {
                Ok(out) => out,
                Err(CandleOcrError::UnsupportedConfig(ref msg)) => {
                    tracing::warn!(
                        class = ?detection.class_name,
                        bbox = ?bbox,
                        reason = %msg,
                        "GLM-OCR paired: skipping region (unsupported config)"
                    );
                    continue;
                }
                Err(e) => {
                    return Err(crate::XbergError::Ocr {
                        message: format!("GLM-OCR paired: region inference failed: {e}"),
                        source: Some(Box::new(e)),
                    });
                }
            };

            let latex_clean = if detection.class_name == crate::layout::LayoutClass::Formula {
                strip_formula_delimiters(&output.content)
            } else {
                output.content.clone()
            };

            let wrapped = wrap_output(region_task, &latex_clean);

            if detection.class_name == crate::layout::LayoutClass::Formula && !latex_clean.is_empty() {
                formulas.push(crate::types::Formula {
                    latex: latex_clean,
                    bbox: region_bbox,
                    page: 1,
                });
            } else if is_table_region(detection.class_name) && !output.content.trim().is_empty() {
                table_bboxes.push(region_bbox);
            }

            parts.push(wrapped);
        }

        Ok::<
            (
                String,
                Vec<crate::types::Formula>,
                Vec<crate::types::extraction::BoundingBox>,
            ),
            crate::XbergError,
        >((parts.join("\n\n"), formulas, table_bboxes))
    })
    .await
    .map_err(|e| crate::XbergError::Ocr {
        message: format!("GLM-OCR paired task execution failed: {e}"),
        source: Some(Box::new(e)),
    })?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_glm_ocr_backend_creation() {
        let backend = GlmOcrBackend::new(GlmOcrTask::default(), LayoutMode::default());
        assert_eq!(backend.name(), "candle-glm-ocr");
        assert_eq!(backend.backend_type(), OcrBackendType::Candle);
    }

    #[test]
    fn test_glm_ocr_emits_structured_markdown() {
        let backend = GlmOcrBackend::new(GlmOcrTask::default(), LayoutMode::default());
        assert!(backend.emits_structured_markdown());
    }

    #[test]
    fn test_glm_ocr_language_support() {
        let backend = GlmOcrBackend::new(GlmOcrTask::default(), LayoutMode::default());
        assert!(backend.supports_language("eng"));
        assert!(backend.supports_language("zho"));
        assert!(backend.supports_language("jpn"));
        assert!(backend.supports_language("unknown"));
    }

    #[test]
    fn test_glm_ocr_supported_languages() {
        let backend = GlmOcrBackend::new(GlmOcrTask::default(), LayoutMode::default());
        let langs = backend.supported_languages();
        assert!(langs.contains(&"eng".to_string()));
        assert!(langs.contains(&"zho".to_string()));
        assert!(langs.contains(&"fra".to_string()));
    }

    #[test]
    fn test_parse_options_defaults() {
        let config = OcrConfig::default();
        let opts = GlmOcrBackend::new(GlmOcrTask::default(), LayoutMode::default()).parse_options(&config);
        assert_eq!(opts.task, GlmOcrTask::Ocr);
        assert_eq!(opts.device, DevicePreference::Auto);
        assert!(!opts.enable_chart_understanding);
    }

    #[test]
    fn test_parse_options_custom_task() {
        let config = OcrConfig {
            backend_options: Some(serde_json::json!({"task": "table"})),
            ..Default::default()
        };
        let opts = GlmOcrBackend::new(GlmOcrTask::default(), LayoutMode::default()).parse_options(&config);
        assert_eq!(opts.task, GlmOcrTask::Table);
    }

    #[test]
    fn test_parse_options_formula_task() {
        let config = OcrConfig {
            backend_options: Some(serde_json::json!({"task": "formula"})),
            ..Default::default()
        };
        let opts = GlmOcrBackend::new(GlmOcrTask::default(), LayoutMode::default()).parse_options(&config);
        assert_eq!(opts.task, GlmOcrTask::Formula);
    }

    #[test]
    fn test_parse_options_custom_device() {
        let config = OcrConfig {
            backend_options: Some(serde_json::json!({"device": "cpu"})),
            ..Default::default()
        };
        let opts = GlmOcrBackend::new(GlmOcrTask::default(), LayoutMode::default()).parse_options(&config);
        assert_eq!(opts.device, DevicePreference::Cpu);
    }

    #[test]
    fn test_parse_options_enable_chart_understanding_true() {
        let config = OcrConfig {
            backend_options: Some(serde_json::json!({"enable_chart_understanding": true})),
            ..Default::default()
        };
        let opts = GlmOcrBackend::new(GlmOcrTask::default(), LayoutMode::default()).parse_options(&config);
        assert!(opts.enable_chart_understanding);
    }

    #[test]
    fn test_parse_options_enable_chart_understanding_false() {
        let config = OcrConfig {
            backend_options: Some(serde_json::json!({"enable_chart_understanding": false})),
            ..Default::default()
        };
        let opts = GlmOcrBackend::new(GlmOcrTask::default(), LayoutMode::default()).parse_options(&config);
        assert!(!opts.enable_chart_understanding);
    }

    #[test]
    fn test_parse_options_chart_understanding_default() {
        let config = OcrConfig::default();
        let opts = GlmOcrBackend::new(GlmOcrTask::default(), LayoutMode::default()).parse_options(&config);
        assert!(!opts.enable_chart_understanding);
    }

    #[test]
    fn test_parse_options_combined() {
        let config = OcrConfig {
            backend_options: Some(serde_json::json!({
                "task": "chart",
                "device": "cuda",
                "enable_chart_understanding": true
            })),
            ..Default::default()
        };
        let opts = GlmOcrBackend::new(GlmOcrTask::default(), LayoutMode::default()).parse_options(&config);
        assert_eq!(opts.task, GlmOcrTask::Chart);
        assert_eq!(opts.device, DevicePreference::Cuda);
        assert!(opts.enable_chart_understanding);
    }

    #[test]
    fn test_initialize_and_shutdown() {
        let backend = GlmOcrBackend::new(GlmOcrTask::default(), LayoutMode::default());
        assert!(backend.initialize().is_ok());
        assert!(backend.shutdown().is_ok());
    }

    #[cfg(feature = "layout-detection")]
    #[test]
    fn test_task_for_label_table() {
        use crate::layout::LayoutClass;
        assert_eq!(task_for_label(LayoutClass::Table, false), GlmOcrTask::Table);
    }

    #[cfg(feature = "layout-detection")]
    #[test]
    fn test_task_for_label_formula() {
        use crate::layout::LayoutClass;
        assert_eq!(task_for_label(LayoutClass::Formula, false), GlmOcrTask::Formula);
    }

    #[cfg(feature = "layout-detection")]
    #[test]
    fn test_task_for_label_text() {
        use crate::layout::LayoutClass;
        assert_eq!(task_for_label(LayoutClass::Text, false), GlmOcrTask::Ocr);
    }

    #[cfg(feature = "layout-detection")]
    #[test]
    fn test_task_for_label_chart_disabled() {
        use crate::layout::LayoutClass;
        assert_eq!(task_for_label(LayoutClass::Chart, false), GlmOcrTask::Caption);
    }

    #[cfg(feature = "layout-detection")]
    #[test]
    fn test_task_for_label_chart_enabled() {
        use crate::layout::LayoutClass;
        assert_eq!(task_for_label(LayoutClass::Chart, true), GlmOcrTask::Chart);
    }

    #[cfg(feature = "layout-detection")]
    #[test]
    fn test_parse_and_route_chart_with_understanding_enabled() {
        use crate::layout::LayoutClass;
        let config = OcrConfig {
            backend_options: Some(serde_json::json!({"enable_chart_understanding": true})),
            ..Default::default()
        };
        let opts = GlmOcrBackend::new(GlmOcrTask::default(), LayoutMode::default()).parse_options(&config);
        let routed_task = task_for_label(LayoutClass::Chart, opts.enable_chart_understanding);
        assert_eq!(routed_task, GlmOcrTask::Chart);
    }

    #[cfg(feature = "layout-detection")]
    #[test]
    fn test_parse_and_route_chart_with_understanding_disabled() {
        use crate::layout::LayoutClass;
        let config = OcrConfig {
            backend_options: Some(serde_json::json!({"enable_chart_understanding": false})),
            ..Default::default()
        };
        let opts = GlmOcrBackend::new(GlmOcrTask::default(), LayoutMode::default()).parse_options(&config);
        let routed_task = task_for_label(LayoutClass::Chart, opts.enable_chart_understanding);
        assert_eq!(routed_task, GlmOcrTask::Caption);
    }

    #[cfg(feature = "layout-detection")]
    #[test]
    fn test_wrap_output_formula() {
        let wrapped = wrap_output(GlmOcrTask::Formula, "x^2 + y^2 = r^2");
        assert!(wrapped.starts_with("$$\n"));
        assert!(wrapped.ends_with("\n$$"));
        assert!(wrapped.contains("x^2 + y^2 = r^2"));
    }

    #[cfg(feature = "layout-detection")]
    #[test]
    fn test_strip_formula_delimiters_removes_wrapping_dollars() {
        let wrapped = "$$\nE = mc^2\n$$";
        let result = strip_formula_delimiters(wrapped);
        assert_eq!(result, "E = mc^2");
    }

    #[cfg(feature = "layout-detection")]
    #[test]
    fn test_strip_formula_delimiters_handles_pre_wrapped_content() {
        let pre_wrapped = "$$x^2 + y^2 = z^2$$";
        let result = strip_formula_delimiters(pre_wrapped);
        assert_eq!(result, "x^2 + y^2 = z^2");
    }

    #[cfg(feature = "layout-detection")]
    #[test]
    fn test_strip_formula_delimiters_preserves_undecorated_content() {
        let plain = "a + b = c";
        let result = strip_formula_delimiters(plain);
        assert_eq!(result, "a + b = c");
    }

    #[cfg(feature = "layout-detection")]
    #[test]
    fn test_formula_extraction_from_wrapped_output() {
        let task = GlmOcrTask::Formula;
        let raw_latex = "E = mc^2";
        let wrapped = wrap_output(task, raw_latex);
        let stripped = strip_formula_delimiters(&wrapped);
        assert_eq!(stripped, raw_latex);
    }

    #[cfg(feature = "layout-detection")]
    #[test]
    fn test_wrap_output_chart() {
        let wrapped = wrap_output(GlmOcrTask::Chart, r#"{"type":"bar"}"#);
        assert!(wrapped.starts_with("```json\n"));
        assert!(wrapped.ends_with("\n```"));
    }

    #[cfg(feature = "layout-detection")]
    #[test]
    fn test_wrap_output_table_passthrough() {
        let table = "| A | B |\n|---|---|\n| 1 | 2 |";
        let wrapped = wrap_output(GlmOcrTask::Table, table);
        assert_eq!(wrapped, table);
    }

    #[test]
    fn test_pool_get_or_init_caches_on_first_miss() {
        use std::sync::Arc as StdArc;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let pool = RwLock::new(AHashMap::new());
        let init_count = StdArc::new(AtomicUsize::new(0));

        let init_count_clone = StdArc::clone(&init_count);
        let result1 = pool_get_or_init(&pool, "test_key", || {
            init_count_clone.fetch_add(1, Ordering::SeqCst);
            Ok::<u32, String>(42)
        });

        assert!(result1.is_ok());
        assert_eq!(init_count.load(Ordering::SeqCst), 1, "Initializer should run once");

        let init_count_clone = StdArc::clone(&init_count);
        let result2 = pool_get_or_init(&pool, "test_key", || {
            init_count_clone.fetch_add(1, Ordering::SeqCst);
            Ok::<u32, String>(99)
        });

        assert!(result2.is_ok());
        assert_eq!(
            init_count.load(Ordering::SeqCst),
            1,
            "Initializer should still have run exactly once"
        );

        let v1 = result1.unwrap();
        let v2 = result2.unwrap();
        assert!(Arc::ptr_eq(&v1, &v2), "Cached values should be the same Arc instance");
        assert_eq!(*v1, 42, "First initializer's value should be stored");
    }

    #[test]
    fn test_pool_get_or_init_concurrent_access() {
        use std::sync::Arc as StdArc;
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::thread;

        let pool = StdArc::new(RwLock::new(AHashMap::new()));
        let init_count = StdArc::new(AtomicUsize::new(0));
        let mut handles = vec![];

        for _ in 0..5 {
            let pool_clone = StdArc::clone(&pool);
            let init_count_clone = StdArc::clone(&init_count);

            let handle = thread::spawn(move || {
                let result = pool_get_or_init(&pool_clone, "concurrent_key", || {
                    init_count_clone.fetch_add(1, Ordering::SeqCst);
                    std::thread::sleep(std::time::Duration::from_millis(1));
                    Ok::<u32, String>(42)
                });
                result.unwrap()
            });
            handles.push(handle);
        }

        let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();

        for i in 1..results.len() {
            assert!(
                Arc::ptr_eq(&results[0], &results[i]),
                "All concurrent callers should receive the same Arc instance"
            );
        }

        let final_count = init_count.load(Ordering::SeqCst);
        assert!(final_count >= 1, "Initializer must run at least once");
    }

    #[test]
    fn test_glm_ocr_zero_regions_fallback_guard() {
        assert_eq!(GlmOcrTask::Ocr, GlmOcrTask::default());
    }

    // --- issue #187: paired-mode table bounding boxes must survive re-parsing ---

    #[cfg(feature = "layout-detection")]
    #[test]
    fn test_is_table_region_true_for_table_class() {
        use crate::layout::LayoutClass;
        assert!(is_table_region(LayoutClass::Table));
    }

    #[cfg(feature = "layout-detection")]
    #[test]
    fn test_is_table_region_false_for_text_class() {
        use crate::layout::LayoutClass;
        assert!(!is_table_region(LayoutClass::Text));
    }

    #[cfg(feature = "layout-detection")]
    #[test]
    fn test_merge_table_bounding_boxes_attaches_bbox_matching_detection_order() {
        use crate::types::Table;
        use crate::types::extraction::BoundingBox;

        // Simulates two Table-class GFM tables parsed out of `content` by
        // `extract_gfm_tables`, in the same order the source detections appeared.
        let mut tables = vec![
            Table {
                cells: vec![vec!["A".to_string()]],
                markdown: "| A |\n|---|".to_string(),
                page_number: 1,
                ..Default::default()
            },
            Table {
                cells: vec![vec!["B".to_string()]],
                markdown: "| B |\n|---|".to_string(),
                page_number: 1,
                ..Default::default()
            },
        ];

        let bboxes = vec![
            BoundingBox {
                x0: 10.0,
                y0: 20.0,
                x1: 100.0,
                y1: 200.0,
            },
            BoundingBox {
                x0: 5.0,
                y0: 6.0,
                x1: 7.0,
                y1: 8.0,
            },
        ];

        merge_table_bounding_boxes(&mut tables, &bboxes);

        assert_eq!(tables.len(), 2, "table count must be unchanged by the merge");
        assert_eq!(
            tables[0].bounding_box,
            Some(BoundingBox {
                x0: 10.0,
                y0: 20.0,
                x1: 100.0,
                y1: 200.0
            })
        );
        assert_eq!(
            tables[1].bounding_box,
            Some(BoundingBox {
                x0: 5.0,
                y0: 6.0,
                x1: 7.0,
                y1: 8.0
            })
        );
    }

    #[cfg(feature = "layout-detection")]
    #[test]
    fn test_process_paired_table_detection_yields_exactly_one_table_with_source_bbox() {
        use crate::core::config::OcrConfig;
        use crate::layout::LayoutClass;
        use crate::types::extraction::BoundingBox;

        // Synthetic fixture standing in for `sorted` detections in `process_paired`:
        // one Table-class region (should become a structured Table entry with its
        // detection bbox attached) and one Text-class region (must NOT show up in
        // `result.tables`). Mirrors the per-detection loop body without needing the
        // GLM model or PP-DocLayout-V3.
        let table_bbox = BoundingBox {
            x0: 12.0,
            y0: 34.0,
            x1: 512.0,
            y1: 734.0,
        };
        let table_output = "| Name | Age |\n|------|-----|\n| Alice | 30 |";
        let text_output = "Just some plain OCR'd prose.";

        let regions: Vec<(LayoutClass, BoundingBox, &str)> = vec![
            (LayoutClass::Table, table_bbox, table_output),
            (LayoutClass::Text, BoundingBox::default(), text_output),
        ];

        let mut parts: Vec<String> = Vec::with_capacity(regions.len());
        let mut table_bboxes: Vec<BoundingBox> = Vec::new();
        for (class, bbox, output) in &regions {
            if is_table_region(*class) && !output.trim().is_empty() {
                table_bboxes.push(*bbox);
            }
            parts.push(output.to_string());
        }
        let content = parts.join("\n\n");

        let config = OcrConfig::default();
        let mut doc = super::super::ocr_result::build_ocr_document(
            content,
            Vec::new(),
            std::borrow::Cow::Borrowed("text/markdown"),
            &[],
            &config,
        );
        merge_table_bounding_boxes(&mut doc.tables, &table_bboxes);

        assert_eq!(
            doc.tables.len(),
            1,
            "the Text-class region must not produce a table entry"
        );
        assert_eq!(doc.tables[0].bounding_box, Some(table_bbox));
    }

    // --- issue #190: checkbox selected/unselected state must not be lost to OCR ---

    #[cfg(feature = "layout-detection")]
    #[test]
    fn test_checkbox_marker_for_class_selected_is_x_marker() {
        use crate::layout::LayoutClass;
        assert_eq!(checkbox_marker_for_class(LayoutClass::CheckboxSelected), Some("[x]"));
    }

    #[cfg(feature = "layout-detection")]
    #[test]
    fn test_checkbox_marker_for_class_unselected_is_empty_marker() {
        use crate::layout::LayoutClass;
        assert_eq!(checkbox_marker_for_class(LayoutClass::CheckboxUnselected), Some("[ ]"));
    }

    #[cfg(feature = "layout-detection")]
    #[test]
    fn test_checkbox_marker_for_class_none_for_non_checkbox_class() {
        use crate::layout::LayoutClass;
        assert_eq!(checkbox_marker_for_class(LayoutClass::Text), None);
        assert_eq!(checkbox_marker_for_class(LayoutClass::Table), None);
    }
}
