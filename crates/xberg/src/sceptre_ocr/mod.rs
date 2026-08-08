//! Sceptre OCR backend.
//!
//! Sceptre runs CRAFT text detection followed by CRNN recognition through ONNX
//! Runtime or the pure-Rust tract engine. Readers are initialized lazily and
//! cached by their effective model and inference configuration.

use std::borrow::Cow;
use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::sync::{Arc, Mutex, Weak};

use async_trait::async_trait;
use once_cell::sync::OnceCell;
use sceptre::{Backend, Image, Language, OcrResult, ReadOptions, Reader, TextLine};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::core::config::{ExecutionProviderType, OcrConfig};
use crate::plugins::{OcrBackend, OcrBackendType, Plugin};
use crate::sceptre_languages::{language_group, language_group_name, supported_language_aliases};
use crate::types::internal::{ElementKind, InternalDocument, InternalElement};
use crate::types::{
    BoundingBox, ExtractedDocument, FormatMetadata, Metadata, OcrBoundingGeometry, OcrConfidence, OcrElement,
    OcrElementLevel, OcrMetadata,
};
use crate::{Result, XbergError};

const BACKEND_NAME: &str = "sceptre";
const DEFAULT_LANGUAGE: &str = "eng";
// The metadata key names come from the ungated `crate::ocr_metadata_keys` rather than
// `crate::ocr`, which is gated on `feature = "ocr"` / `"ocr-wasm"` — neither of which a
// Sceptre-only build implies. ~keep
use crate::ocr_metadata_keys::OCR_PROCESSED_IMAGE_HEIGHT_METADATA_KEY as PROCESSED_HEIGHT_KEY;
use crate::ocr_metadata_keys::OCR_PROCESSED_IMAGE_WIDTH_METADATA_KEY as PROCESSED_WIDTH_KEY;
#[cfg(auto_rotate)]
use crate::ocr_metadata_keys::{
    OCR_AUTO_ROTATED_METADATA_KEY as AUTO_ROTATED_KEY,
    OCR_ORIENTATION_CONFIDENCE_METADATA_KEY as ORIENTATION_CONFIDENCE_KEY,
    OCR_ORIENTATION_DEGREES_METADATA_KEY as ORIENTATION_DEGREES_KEY,
};
// Each reader retains multiple ONNX sessions and a worker pool. Four entries
// cover common language/configuration reuse while bounding retained resources. ~keep
const READER_CACHE_CAPACITY: usize = 4;
// A transient overflow slot prevents requests from blocking when every cached
// reader is active while still bounding heavyweight reader initialization. ~keep
const READER_OVERFLOW_CAPACITY: usize = 1;
// A Sceptre reader already parallelizes detection and recognition internally.
// Admit one operation at a time so cached readers cannot multiply the process
// thread budget during concurrent extraction. ~keep
const EXECUTION_CAPACITY: usize = 1;

type ReaderCell = Arc<ReaderSlot>;

struct ReaderSlot {
    reader: OnceCell<Reader>,
    _overflow_permit: Option<OwnedSemaphorePermit>,
}

impl ReaderSlot {
    fn cached() -> Self {
        Self {
            reader: OnceCell::new(),
            _overflow_permit: None,
        }
    }

    fn overflow(permit: OwnedSemaphorePermit) -> Self {
        Self {
            reader: OnceCell::new(),
            _overflow_permit: Some(permit),
        }
    }
}

struct ReaderCache {
    entries: HashMap<String, ReaderCell>,
    overflow: HashMap<String, Weak<ReaderSlot>>,
    recency: VecDeque<String>,
}

impl ReaderCache {
    fn new() -> Self {
        Self {
            entries: HashMap::with_capacity(READER_CACHE_CAPACITY),
            overflow: HashMap::with_capacity(READER_OVERFLOW_CAPACITY),
            recency: VecDeque::with_capacity(READER_CACHE_CAPACITY),
        }
    }

    fn reader_cell(&mut self, key: &str) -> Option<ReaderCell> {
        if let Some(cell) = self.entries.get(key).cloned() {
            self.mark_recent(key);
            return Some(cell);
        }

        self.overflow.retain(|_, cell| cell.strong_count() > 0);
        if let Some(cell) = self.overflow.get(key).and_then(Weak::upgrade) {
            return Some(cell);
        }

        if self.entries.len() == READER_CACHE_CAPACITY && !self.evict_oldest_inactive() {
            return None;
        }

        let cell = Arc::new(ReaderSlot::cached());
        self.entries.insert(key.to_string(), Arc::clone(&cell));
        self.recency.push_back(key.to_string());
        Some(cell)
    }

    fn insert_overflow(&mut self, key: &str, permit: OwnedSemaphorePermit) -> ReaderCell {
        let cell = Arc::new(ReaderSlot::overflow(permit));
        self.overflow.insert(key.to_string(), Arc::downgrade(&cell));
        cell
    }

    fn mark_recent(&mut self, key: &str) {
        if let Some(index) = self.recency.iter().position(|cached_key| cached_key == key) {
            self.recency.remove(index);
        }
        self.recency.push_back(key.to_string());
    }

    fn evict_oldest_inactive(&mut self) -> bool {
        let Some(index) = self
            .recency
            .iter()
            .position(|key| self.entries.get(key).is_some_and(|cell| Arc::strong_count(cell) == 1))
        else {
            return false;
        };
        let Some(key) = self.recency.remove(index) else {
            return false;
        };
        self.entries.remove(&key);
        true
    }
}

/// Sceptre-backed OCR implementation.
///
/// Reader construction resolves model files and creates inference sessions, so
/// it is deferred until the first call for an effective configuration. The
/// initialized reader is reused without holding the cache lock during inference;
/// backend-wide admission prevents cached readers from oversubscribing the host.
#[cfg_attr(alef, alef(skip))]
pub struct SceptreOcrBackend {
    readers: Arc<Mutex<ReaderCache>>,
    overflow_slots: Arc<Semaphore>,
    execution_slots: Arc<Semaphore>,
    #[cfg(auto_rotate)]
    orientation_detector: Arc<OnceCell<crate::doc_orientation::DocOrientationDetector>>,
}

impl SceptreOcrBackend {
    /// Create an empty, lazily initialized Sceptre backend.
    pub fn new() -> Result<Self> {
        Ok(Self {
            readers: Arc::new(Mutex::new(ReaderCache::new())),
            overflow_slots: Arc::new(Semaphore::new(READER_OVERFLOW_CAPACITY)),
            execution_slots: Arc::new(Semaphore::new(EXECUTION_CAPACITY)),
            #[cfg(auto_rotate)]
            orientation_detector: Arc::new(OnceCell::new()),
        })
    }

    async fn reader_cell(&self, key: &str) -> Result<ReaderCell> {
        if let Some(cell) = self.lock_reader_cache()?.reader_cell(key) {
            return Ok(cell);
        }

        let permit = Arc::clone(&self.overflow_slots)
            .acquire_owned()
            .await
            .map_err(|error| ocr_error(format!("Failed to acquire Sceptre reader admission: {error}")))?;
        let mut readers = self.lock_reader_cache()?;
        if let Some(cell) = readers.reader_cell(key) {
            return Ok(cell);
        }
        Ok(readers.insert_overflow(key, permit))
    }

    fn lock_reader_cache(&self) -> Result<std::sync::MutexGuard<'_, ReaderCache>> {
        self.readers.lock().map_err(|error| XbergError::Plugin {
            message: format!("Failed to acquire Sceptre reader cache: {error}"),
            plugin_name: BACKEND_NAME.to_string(),
        })
    }

    fn effective_config(config: &OcrConfig) -> Result<(sceptre::OcrConfig, Vec<String>)> {
        validate_auto_rotate_support(config)?;
        validate_acceleration(config)?;
        let languages = effective_languages(config);
        let group = resolve_language_group(&languages)?;
        let mut sceptre_config = parse_sceptre_options(config)?;
        if !has_explicit_model_backend(config) {
            #[cfg(all(feature = "sceptre-ocr-tract", not(feature = "sceptre-ocr-ort")))]
            {
                sceptre_config.model.backend = Backend::Tract;
            }
        }
        validate_inference_backend(sceptre_config.model.backend)?;
        let thread_budget = crate::core::config::concurrency::active_thread_budget();
        sceptre_config.concurrency.max_threads = Some(
            sceptre_config
                .concurrency
                .max_threads
                .unwrap_or(thread_budget)
                .clamp(1, thread_budget),
        );
        sceptre_config.model.languages = vec![group];
        Ok((sceptre_config, languages))
    }

    async fn process_owned(&self, image_bytes: Arc<Vec<u8>>, config: &OcrConfig) -> Result<ExtractedDocument> {
        if image_bytes.is_empty() {
            return Err(XbergError::validation("Cannot run Sceptre OCR on empty image data"));
        }

        let (sceptre_config, languages) = Self::effective_config(config)?;
        let cache_key = reader_cache_key(&sceptre_config)?;
        let reader_cell = self.reader_cell(&cache_key).await?;
        let execution_permit = Arc::clone(&self.execution_slots)
            .acquire_owned()
            .await
            .map_err(|error| ocr_error(format!("Failed to acquire Sceptre execution admission: {error}")))?;
        let rotation = RotationContext {
            #[cfg(auto_rotate)]
            detector: Arc::clone(&self.orientation_detector),
            #[cfg(auto_rotate)]
            enabled: config.auto_rotate,
            #[cfg(auto_rotate)]
            acceleration: config.acceleration.clone(),
        };

        let output = tokio::task::spawn_blocking(move || {
            let _execution_permit = execution_permit;
            run_blocking(image_bytes.as_slice(), sceptre_config, reader_cell, rotation)
        })
        .await
        .map_err(|error| XbergError::Plugin {
            message: format!("Sceptre OCR blocking task failed: {error}"),
            plugin_name: BACKEND_NAME.to_string(),
        })??;

        Ok(build_document(output, languages, config))
    }
}

impl Default for SceptreOcrBackend {
    fn default() -> Self {
        Self::new().expect("constructing an empty Sceptre backend is infallible")
    }
}

impl Plugin for SceptreOcrBackend {
    fn name(&self) -> &str {
        BACKEND_NAME
    }

    fn version(&self) -> String {
        sceptre::VERSION.to_string()
    }
}

#[async_trait]
impl OcrBackend for SceptreOcrBackend {
    async fn process_image(&self, image_bytes: &[u8], config: &OcrConfig) -> Result<ExtractedDocument> {
        self.process_owned(Arc::new(image_bytes.to_vec()), config).await
    }

    async fn process_image_owned(&self, image_bytes: Arc<Vec<u8>>, config: &OcrConfig) -> Result<ExtractedDocument> {
        self.process_owned(image_bytes, config).await
    }

    async fn process_image_file(&self, path: &Path, config: &OcrConfig) -> Result<ExtractedDocument> {
        let bytes = crate::core::io::read_file_async(path).await?;
        self.process_owned(Arc::new(bytes), config).await
    }

    fn supports_language(&self, language: &str) -> bool {
        language_group(language).is_some()
    }

    fn backend_type(&self) -> OcrBackendType {
        // Built-in OCR stages are selected by registry name; keep the exhaustive
        // v1 enum stable while adding Sceptre in a minor release. ~keep
        OcrBackendType::Custom
    }

    fn supported_languages(&self) -> Vec<String> {
        supported_language_aliases().into_iter().map(str::to_string).collect()
    }
}

struct BlockingOutput {
    result: OcrResult,
    width: u32,
    height: u32,
    #[cfg(auto_rotate)]
    orientation: Option<crate::doc_orientation::OrientationResult>,
    #[cfg(auto_rotate)]
    auto_rotated: bool,
}

struct RotationContext {
    #[cfg(auto_rotate)]
    detector: Arc<OnceCell<crate::doc_orientation::DocOrientationDetector>>,
    #[cfg(auto_rotate)]
    enabled: bool,
    #[cfg(auto_rotate)]
    acceleration: Option<crate::core::config::AccelerationConfig>,
}

fn run_blocking(
    image_bytes: &[u8],
    config: sceptre::OcrConfig,
    reader_cell: ReaderCell,
    rotation: RotationContext,
) -> Result<BlockingOutput> {
    #[cfg(not(auto_rotate))]
    let _ = rotation;
    #[cfg(auto_rotate)]
    let (image, orientation, was_rotated) = decode_and_rotate(
        image_bytes,
        rotation.enabled,
        rotation.detector.as_ref(),
        rotation.acceleration,
    )?;
    #[cfg(not(auto_rotate))]
    let image = Image::from_bytes(image_bytes).map_err(map_sceptre_error)?;
    let width = image.width();
    let height = image.height();
    let reader = reader_cell
        .reader
        .get_or_try_init(|| Reader::builder().config(config).build().map_err(map_sceptre_error))?;
    let result = reader
        .recognize(&image, &ReadOptions::default())
        .map_err(map_sceptre_error)?;
    Ok(BlockingOutput {
        result,
        width,
        height,
        #[cfg(auto_rotate)]
        orientation,
        #[cfg(auto_rotate)]
        auto_rotated: was_rotated,
    })
}

#[cfg(auto_rotate)]
fn decode_and_rotate(
    image_bytes: &[u8],
    auto_rotate: bool,
    detector_cell: &OnceCell<crate::doc_orientation::DocOrientationDetector>,
    acceleration: Option<crate::core::config::AccelerationConfig>,
) -> Result<(Image, Option<crate::doc_orientation::OrientationResult>, bool)> {
    if !auto_rotate {
        return Image::from_bytes(image_bytes)
            .map(|image| (image, None, false))
            .map_err(map_sceptre_error);
    }

    let decoded = image::load_from_memory(image_bytes)
        .map_err(|error| ocr_error(format!("Failed to decode image for Sceptre auto-rotation: {error}")))?
        .to_rgb8();
    let detector = detector_cell.get_or_init(|| {
        crate::doc_orientation::DocOrientationDetector::with_acceleration(
            crate::doc_orientation::resolve_cache_dir(),
            acceleration,
        )
    });
    let orientation = match detector.detect(&decoded) {
        Ok(orientation) => orientation,
        Err(error) => {
            tracing::warn!(error = %error, "Sceptre orientation detection failed; using the original image");
            return Image::from_rgb8(decoded.width(), decoded.height(), decoded.into_raw())
                .map(|image| (image, None, false))
                .map_err(map_sceptre_error);
        }
    };
    let should_rotate = orientation.degrees != 0 && orientation.confidence >= crate::doc_orientation::MIN_CONFIDENCE;
    let rotated = if should_rotate {
        rotate_to_upright(&decoded, orientation.degrees)
    } else {
        decoded
    };
    Image::from_rgb8(rotated.width(), rotated.height(), rotated.into_raw())
        .map(|image| (image, Some(orientation), should_rotate))
        .map_err(map_sceptre_error)
}

#[cfg(auto_rotate)]
fn rotate_to_upright(image: &image::RgbImage, degrees: u32) -> image::RgbImage {
    match degrees {
        90 => image::imageops::rotate270(image),
        180 => image::imageops::rotate180(image),
        270 => image::imageops::rotate90(image),
        _ => image.clone(),
    }
}

fn build_document(output: BlockingOutput, languages: Vec<String>, config: &OcrConfig) -> ExtractedDocument {
    let line_elements: Vec<OcrElement> = output.result.lines.iter().map(line_to_element).collect();
    let content = output
        .result
        .lines
        .iter()
        .map(|line| line.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let internal_document = build_internal_document(&line_elements);
    let ocr_elements = select_output_elements(&line_elements, config);
    let metadata = build_metadata(&languages, &output);

    ExtractedDocument {
        content,
        mime_type: Cow::Borrowed("text/plain"),
        metadata,
        detected_languages: Some(languages),
        ocr_elements,
        ocr_internal_document: Some(internal_document),
        ..Default::default()
    }
}

fn line_to_element(line: &TextLine) -> OcrElement {
    OcrElement {
        text: line.text.clone(),
        geometry: OcrBoundingGeometry::Quadrilateral {
            points: line
                .quad
                .points
                .map(|point| (pixel_coordinate(point.x), pixel_coordinate(point.y))),
        },
        confidence: OcrConfidence {
            detection: None,
            recognition: f64::from(line.confidence).clamp(0.0, 1.0),
        },
        level: OcrElementLevel::Line,
        rotation: None,
        page_number: 1,
        parent_id: None,
        backend_metadata: HashMap::new(),
    }
}

fn pixel_coordinate(value: f32) -> u32 {
    if !value.is_finite() || value <= 0.0 {
        return 0;
    }
    value.round().min(u32::MAX as f32) as u32
}

fn build_internal_document(elements: &[OcrElement]) -> InternalDocument {
    let mut document = InternalDocument::new("ocr");
    document.mime_type = "text/plain".to_string();
    document.prebuilt_ocr_elements = Some(elements.to_vec());
    for element in elements {
        let mut internal = InternalElement::text(
            ElementKind::OcrText {
                level: OcrElementLevel::Line,
            },
            &element.text,
            0,
        );
        internal.page = Some(element.page_number);
        internal.bbox = geometry_bounds(&element.geometry);
        internal.ocr_geometry = Some(element.geometry.clone());
        internal.ocr_confidence = Some(element.confidence.clone());
        document.push_element(internal);
    }
    document
}

fn geometry_bounds(geometry: &OcrBoundingGeometry) -> Option<BoundingBox> {
    let OcrBoundingGeometry::Quadrilateral { points } = geometry else {
        return None;
    };
    let x0 = points.iter().map(|(x, _)| *x).min()?;
    let y0 = points.iter().map(|(_, y)| *y).min()?;
    let x1 = points.iter().map(|(x, _)| *x).max()?;
    let y1 = points.iter().map(|(_, y)| *y).max()?;
    Some(BoundingBox {
        x0: f64::from(x0),
        y0: f64::from(y0),
        x1: f64::from(x1),
        y1: f64::from(y1),
    })
}

fn select_output_elements(elements: &[OcrElement], config: &OcrConfig) -> Option<Vec<OcrElement>> {
    let options = config.element_config.as_ref()?;
    if !options.include_elements || !matches!(options.min_level, OcrElementLevel::Word | OcrElementLevel::Line) {
        return None;
    }
    let selected: Vec<OcrElement> = elements
        .iter()
        .filter(|element| element.confidence.recognition >= options.min_confidence)
        .cloned()
        .collect();
    (!selected.is_empty()).then_some(selected)
}

// Matches the Tesseract backend's low-word-confidence cutoff
// (ocr/processor/execution.rs) so downstream quality gating applies the same
// threshold regardless of which backend produced the metadata. ~keep
const LOW_CONFIDENCE_THRESHOLD_PCT: i64 = 50;

/// Per-line confidence statistics on Tesseract's 0-100 integer percentage
/// scale, so consumers that already gate on Tesseract's `mean_text_conf` /
/// `median_word_conf` / `p10_word_conf` keys (e.g. `extractors/pdf/ocr.rs`,
/// which divides `mean_text_conf` by 100.0) work unchanged against Sceptre.
struct ConfidenceStats {
    mean_pct: i64,
    median_pct: i64,
    p10_pct: i64,
    line_count: usize,
    low_confidence_count: usize,
}

/// Convert a Sceptre line confidence (`0.0..=1.0`) to Tesseract's `0..=100`
/// integer percentage scale.
fn confidence_to_percent(confidence: f32) -> i64 {
    (f64::from(confidence).clamp(0.0, 1.0) * 100.0).round() as i64
}

/// Compute mean/median/p10 confidence and low-confidence-line count from
/// per-line confidence values. Returns `None` when there are no lines, mirroring
/// the Tesseract backend leaving these keys out of `metadata.additional` when it
/// has nothing to report.
fn compute_confidence_stats(lines: &[TextLine]) -> Option<ConfidenceStats> {
    if lines.is_empty() {
        return None;
    }

    let percentages: Vec<i64> = lines
        .iter()
        .map(|line| confidence_to_percent(line.confidence))
        .collect();
    let line_count = percentages.len();
    let low_confidence_count = percentages
        .iter()
        .filter(|&&pct| pct < LOW_CONFIDENCE_THRESHOLD_PCT)
        .count();

    let mean_pct = percentages.iter().sum::<i64>() / line_count as i64;

    let mut sorted = percentages.clone();
    sorted.sort_unstable();
    let median_pct = if line_count.is_multiple_of(2) {
        (sorted[line_count / 2 - 1] + sorted[line_count / 2]) / 2
    } else {
        sorted[line_count / 2]
    };
    let p10_idx = ((line_count as f64 - 1.0) * 0.1).floor() as usize;
    let p10_pct = sorted[p10_idx.min(line_count - 1)];

    Some(ConfidenceStats {
        mean_pct,
        median_pct,
        p10_pct,
        line_count,
        low_confidence_count,
    })
}

/// Publish confidence statistics into `metadata.additional` using the same key
/// names the Tesseract backend uses, so quality-gating code that reads
/// `mean_text_conf` / `median_word_conf` / `p10_word_conf` / `word_count` /
/// `low_conf_word_count` works against Sceptre output too (issue #186). Sceptre
/// has no word-level detections, so line confidence is the finest granularity
/// available and is reported under the same keys.
fn insert_confidence_stats(metadata: &mut Metadata, lines: &[TextLine]) {
    let Some(stats) = compute_confidence_stats(lines) else {
        return;
    };
    metadata
        .additional
        .insert(Cow::Borrowed("mean_text_conf"), serde_json::json!(stats.mean_pct));
    metadata
        .additional
        .insert(Cow::Borrowed("median_word_conf"), serde_json::json!(stats.median_pct));
    metadata
        .additional
        .insert(Cow::Borrowed("p10_word_conf"), serde_json::json!(stats.p10_pct));
    metadata
        .additional
        .insert(Cow::Borrowed("word_count"), serde_json::json!(stats.line_count));
    metadata.additional.insert(
        Cow::Borrowed("low_conf_word_count"),
        serde_json::json!(stats.low_confidence_count),
    );
}

fn build_metadata(languages: &[String], output: &BlockingOutput) -> Metadata {
    let mut metadata = Metadata {
        format: Some(FormatMetadata::Ocr(OcrMetadata {
            language: languages.join("+"),
            // Sceptre has no Tesseract-style Page Segmentation Mode; `psm: 3`
            // (Tesseract's "fully automatic" default) would misrepresent a
            // concept this backend does not have. Leave it at the zero default. ~keep
            output_format: "text".to_string(),
            ..Default::default()
        })),
        ocr_used: true,
        ..Default::default()
    };
    metadata
        .additional
        .insert(Cow::Borrowed(PROCESSED_WIDTH_KEY), serde_json::json!(output.width));
    metadata
        .additional
        .insert(Cow::Borrowed(PROCESSED_HEIGHT_KEY), serde_json::json!(output.height));
    insert_confidence_stats(&mut metadata, &output.result.lines);
    #[cfg(auto_rotate)]
    if let Some(orientation) = output.orientation {
        metadata.additional.insert(
            Cow::Borrowed(ORIENTATION_DEGREES_KEY),
            serde_json::json!(orientation.degrees),
        );
        metadata.additional.insert(
            Cow::Borrowed(ORIENTATION_CONFIDENCE_KEY),
            serde_json::json!(orientation.confidence),
        );
    }
    #[cfg(auto_rotate)]
    if output.auto_rotated {
        metadata
            .additional
            .insert(Cow::Borrowed(AUTO_ROTATED_KEY), serde_json::Value::Bool(true));
    }
    metadata
}

fn parse_sceptre_options(config: &OcrConfig) -> Result<sceptre::OcrConfig> {
    let Some(options) = config.backend_options.as_ref() else {
        return Ok(sceptre::OcrConfig::default());
    };
    let object = options
        .as_object()
        .ok_or_else(|| ocr_error("Invalid `backend_options` configuration: expected an object"))?;
    let mut parsed = sceptre::OcrConfig::default();
    if let Some(value) = object.get("detection") {
        parsed.detection = parse_sceptre_section("detection", value)?;
    }
    if let Some(value) = object.get("recognition") {
        parsed.recognition = parse_sceptre_section("recognition", value)?;
    }
    if let Some(value) = object.get("concurrency") {
        parsed.concurrency = parse_sceptre_section("concurrency", value)?;
    }
    if let Some(value) = object.get("model") {
        parsed.model = parse_sceptre_section("model", value)?;
    }
    Ok(parsed)
}

fn has_explicit_model_backend(config: &OcrConfig) -> bool {
    config
        .backend_options
        .as_ref()
        .and_then(serde_json::Value::as_object)
        .and_then(|options| options.get("model"))
        .and_then(serde_json::Value::as_object)
        .is_some_and(|model| model.contains_key("backend"))
}

fn validate_inference_backend(backend: Backend) -> Result<()> {
    match backend {
        Backend::Ort if cfg!(feature = "sceptre-ocr-ort") => Ok(()),
        Backend::Tract if cfg!(feature = "sceptre-ocr-tract") => Ok(()),
        Backend::Ort => Err(ocr_error(
            "Sceptre ORT inference is unavailable in this build; enable `sceptre-ocr-ort`",
        )),
        Backend::Tract => Err(ocr_error(
            "Sceptre tract inference is unavailable in this build; enable `sceptre-ocr-tract`",
        )),
        _ => Err(ocr_error(
            "The selected Sceptre inference backend is not supported by Xberg",
        )),
    }
}

fn parse_sceptre_section<T: serde::de::DeserializeOwned>(name: &str, value: &serde_json::Value) -> Result<T> {
    serde_json::from_value(value.clone()).map_err(|error| {
        ocr_error(format!(
            "Invalid Sceptre `backend_options.{name}` configuration: {error}"
        ))
    })
}

fn reader_cache_key(config: &sceptre::OcrConfig) -> Result<String> {
    serde_json::to_string(config)
        .map_err(|error| ocr_error(format!("Failed to serialize effective Sceptre configuration: {error}")))
}

fn effective_languages(config: &OcrConfig) -> Vec<String> {
    let languages: Vec<String> = config
        .language
        .iter()
        .map(|language| language.trim().to_ascii_lowercase())
        .filter(|language| !language.is_empty())
        .collect();
    if languages.is_empty() {
        vec![DEFAULT_LANGUAGE.to_string()]
    } else {
        languages
    }
}

fn resolve_language_group(languages: &[String]) -> Result<Language> {
    let mut selected = Language::English;
    for language in languages {
        let group = language_group(language).ok_or_else(|| {
            ocr_error(format!(
                concat!(
                    "Sceptre does not support OCR language `{}`; choose an English, Latin, ",
                    "simplified Chinese, Japanese, Korean, Telugu, Kannada, or Cyrillic language"
                ),
                language
            ))
        })?;
        if group == Language::English {
            continue;
        }
        if selected != Language::English && selected != group {
            return Err(ocr_error(format!(
                concat!(
                    "Sceptre cannot combine language groups `{}` and `{}` in one reader; ",
                    "use languages from one script family (English may be combined with any one family)"
                ),
                language_group_name(selected),
                language_group_name(group),
            )));
        }
        selected = group;
    }
    Ok(selected)
}

fn validate_acceleration(config: &OcrConfig) -> Result<()> {
    let Some(acceleration) = config.acceleration.as_ref() else {
        return Ok(());
    };
    if acceleration.provider == ExecutionProviderType::Cpu {
        return Ok(());
    }
    Err(ocr_error(format!(
        concat!(
            "Sceptre in Xberg 1.1 is CPU-only; acceleration provider `{:?}` is not supported. ",
            "Set acceleration.provider to `cpu` or omit acceleration"
        ),
        acceleration.provider
    )))
}

fn validate_auto_rotate_support(_config: &OcrConfig) -> Result<()> {
    #[cfg(not(auto_rotate))]
    if _config.auto_rotate {
        return Err(ocr_error(
            "Sceptre auto-rotation is unavailable in this build; enable the `auto-rotate` or `auto-rotate-tract` feature",
        ));
    }
    Ok(())
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
    use sceptre::{Point, Quad};

    fn line(text: &str, confidence: f32) -> TextLine {
        TextLine {
            quad: Quad {
                points: [
                    Point::new(10.4, 20.2),
                    Point::new(110.6, 20.0),
                    Point::new(111.0, 42.8),
                    Point::new(10.0, 43.1),
                ],
            },
            text: text.to_string(),
            confidence,
        }
    }

    #[test]
    fn should_map_iso_codes_to_sceptre_groups() {
        assert_eq!(language_group("eng"), Some(Language::English));
        assert_eq!(language_group("de"), Some(Language::Latin));
        assert_eq!(language_group("zh_Hans"), Some(Language::ChineseSimplified));
        assert_eq!(language_group("jpn"), Some(Language::Japanese));
        assert_eq!(language_group("jpn_vert"), Some(Language::Japanese));
        assert_eq!(language_group("kor"), Some(Language::Korean));
        assert_eq!(language_group("tel"), Some(Language::Telugu));
        assert_eq!(language_group("kan"), Some(Language::Kannada));
        assert_eq!(language_group("ukr"), Some(Language::Cyrillic));
        assert_eq!(language_group("ara"), None);
        assert_eq!(language_group("cat"), None);
        assert_eq!(language_group("kaz"), None);
    }

    #[test]
    fn shared_validation_should_accept_every_sceptre_language_alias() {
        for language in supported_language_aliases() {
            assert!(
                crate::core::config_validation::validate_language_code(language).is_ok(),
                "Sceptre language alias {language} should pass shared validation"
            );
        }
    }

    #[test]
    fn should_allow_english_with_one_non_english_group() {
        let languages = vec!["eng".to_string(), "deu".to_string(), "fra".to_string()];
        assert_eq!(
            resolve_language_group(&languages).expect("compatible languages"),
            Language::Latin
        );
    }

    #[test]
    fn should_normalize_easyocr_script_tokens() {
        assert_eq!(language_group("CH_SIM"), Some(Language::ChineseSimplified));
        assert_eq!(language_group("rs_latin"), Some(Language::Latin));
        assert_eq!(language_group("rs-cyrillic"), Some(Language::Cyrillic));
    }

    #[test]
    fn should_reject_incompatible_language_groups() {
        let languages = vec!["jpn".to_string(), "kor".to_string()];
        let error = resolve_language_group(&languages).expect_err("groups must be incompatible");
        assert!(error.to_string().contains("cannot combine language groups"));
    }

    #[test]
    fn should_parse_direct_sceptre_options_and_override_languages() {
        let config = OcrConfig {
            language: vec!["rus".to_string()],
            backend_options: Some(serde_json::json!({
                "recognition": { "batch_size": 4 },
                "concurrency": { "max_threads": 2 }
            })),
            ..Default::default()
        };
        let (effective, _) = SceptreOcrBackend::effective_config(&config).expect("valid config");
        assert_eq!(effective.recognition.batch_size, 4);
        assert_eq!(effective.concurrency.max_threads, Some(2));
        assert_eq!(effective.model.languages, vec![Language::Cyrillic]);
    }

    #[test]
    fn should_propagate_xberg_thread_budget_when_unspecified() {
        let (effective, _) = SceptreOcrBackend::effective_config(&OcrConfig::default()).expect("valid config");
        assert_eq!(
            effective.concurrency.max_threads,
            Some(crate::core::config::concurrency::active_thread_budget())
        );
    }

    #[test]
    fn should_clamp_sceptre_threads_to_xberg_budget() {
        let config = OcrConfig {
            backend_options: Some(serde_json::json!({ "concurrency": { "max_threads": usize::MAX } })),
            ..Default::default()
        };
        let (effective, _) = SceptreOcrBackend::effective_config(&config).expect("valid config");
        assert_eq!(
            effective.concurrency.max_threads,
            Some(crate::core::config::concurrency::active_thread_budget())
        );
    }

    #[cfg(all(feature = "sceptre-ocr-tract", not(feature = "sceptre-ocr-ort")))]
    #[test]
    fn tract_only_build_should_select_tract_by_default() {
        let (effective, _) = SceptreOcrBackend::effective_config(&OcrConfig::default()).expect("valid config");
        assert_eq!(effective.model.backend, Backend::Tract);
    }

    #[cfg(not(feature = "sceptre-ocr-tract"))]
    #[test]
    fn ort_only_build_should_reject_explicit_tract() {
        let config = OcrConfig {
            backend_options: Some(serde_json::json!({ "model": { "backend": "tract" } })),
            ..Default::default()
        };
        let error = SceptreOcrBackend::effective_config(&config).expect_err("tract must require its feature");
        assert!(error.to_string().contains("sceptre-ocr-tract"));
    }

    #[test]
    fn should_ignore_options_owned_by_other_backends() {
        let config = OcrConfig {
            backend_options: Some(serde_json::json!({ "custom_backend_option": true })),
            ..Default::default()
        };
        let parsed = parse_sceptre_options(&config).expect("unrelated options must be ignored");
        assert_eq!(
            parsed.recognition.batch_size,
            sceptre::OcrConfig::default().recognition.batch_size
        );
    }

    #[test]
    fn should_reject_unknown_sceptre_section_option() {
        let config = OcrConfig {
            backend_options: Some(serde_json::json!({ "recognition": { "typo": true } })),
            ..Default::default()
        };
        let error = parse_sceptre_options(&config).expect_err("unknown Sceptre fields must fail");
        assert!(error.to_string().contains("backend_options.recognition"));
    }

    #[test]
    fn should_reject_non_cpu_acceleration() {
        let config = OcrConfig {
            acceleration: Some(crate::core::config::AccelerationConfig {
                provider: ExecutionProviderType::CoreMl,
                ..Default::default()
            }),
            ..Default::default()
        };

        let error = SceptreOcrBackend::effective_config(&config).expect_err("CoreML must be rejected");
        assert!(error.to_string().contains("CPU-only"));
    }

    #[cfg(not(auto_rotate))]
    #[test]
    fn should_reject_auto_rotate_without_capability() {
        let config = OcrConfig {
            auto_rotate: true,
            ..Default::default()
        };

        let error = SceptreOcrBackend::effective_config(&config).expect_err("auto-rotation must require its feature");
        assert!(error.to_string().contains("auto-rotation is unavailable"));
    }

    #[test]
    fn should_build_line_geometry_and_clamp_confidence() {
        let element = line_to_element(&line("hello", 1.4));
        assert_eq!(element.text, "hello");
        assert_eq!(element.level, OcrElementLevel::Line);
        assert_eq!(element.confidence.recognition, 1.0);
        assert_eq!(
            element.geometry,
            OcrBoundingGeometry::Quadrilateral {
                points: [(10, 20), (111, 20), (111, 43), (10, 43)]
            }
        );
    }

    #[test]
    fn should_join_lines_and_build_layout_document() {
        let output = BlockingOutput {
            result: OcrResult {
                lines: vec![line("first", 0.9), line("second", 0.8)],
            },
            width: 640,
            height: 480,
            #[cfg(auto_rotate)]
            orientation: None,
            #[cfg(auto_rotate)]
            auto_rotated: false,
        };
        let result = build_document(output, vec!["eng".to_string()], &OcrConfig::default());
        assert_eq!(result.content, "first\nsecond");
        let internal = result.ocr_internal_document.expect("internal OCR document");
        assert_eq!(internal.elements.len(), 2);
        let first = &internal.elements[0];
        assert_eq!(
            first.ocr_confidence.as_ref().expect("confidence").recognition,
            0.9_f32 as f64
        );
        assert_eq!(
            first.bbox,
            Some(BoundingBox {
                x0: 10.0,
                y0: 20.0,
                x1: 111.0,
                y1: 43.0,
            })
        );
        assert!(result.ocr_elements.is_none());
    }

    #[test]
    fn should_filter_requested_line_elements_by_confidence() {
        let config = OcrConfig {
            element_config: Some(crate::types::OcrElementConfig {
                include_elements: true,
                min_level: OcrElementLevel::Line,
                min_confidence: 0.85,
                build_hierarchy: false,
            }),
            ..Default::default()
        };
        let elements = vec![line_to_element(&line("keep", 0.9)), line_to_element(&line("drop", 0.8))];
        let selected = select_output_elements(&elements, &config).expect("one selected line");
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].text, "keep");
    }

    /// Regression test for issue #186: Sceptre must publish confidence
    /// statistics under the same keys Tesseract uses, so quality gating that
    /// reads `mean_text_conf` / `median_word_conf` / `p10_word_conf` /
    /// `word_count` / `low_conf_word_count` works against Sceptre output too.
    #[test]
    fn should_publish_confidence_statistics_in_metadata() {
        let output = BlockingOutput {
            result: OcrResult {
                // percentages: 90, 80, 40, 100 -> mean 77, sorted [40,80,90,100]
                lines: vec![line("a", 0.9), line("b", 0.8), line("c", 0.4), line("d", 1.0)],
            },
            width: 100,
            height: 100,
            #[cfg(auto_rotate)]
            orientation: None,
            #[cfg(auto_rotate)]
            auto_rotated: false,
        };

        let metadata = build_metadata(&["eng".to_string()], &output);

        assert_eq!(metadata.additional.get("mean_text_conf"), Some(&serde_json::json!(77)));
        assert_eq!(
            metadata.additional.get("median_word_conf"),
            Some(&serde_json::json!(85))
        );
        assert_eq!(metadata.additional.get("p10_word_conf"), Some(&serde_json::json!(40)));
        assert_eq!(metadata.additional.get("word_count"), Some(&serde_json::json!(4)));
        assert_eq!(
            metadata.additional.get("low_conf_word_count"),
            Some(&serde_json::json!(1))
        );
    }

    /// With no recognized lines there is nothing to report; the confidence keys
    /// must be absent rather than fabricated zeros (matching Tesseract's
    /// behavior of only inserting `mean_text_conf` when `>= 0`).
    #[test]
    fn should_omit_confidence_statistics_when_no_lines_recognized() {
        let output = BlockingOutput {
            result: OcrResult { lines: vec![] },
            width: 100,
            height: 100,
            #[cfg(auto_rotate)]
            orientation: None,
            #[cfg(auto_rotate)]
            auto_rotated: false,
        };

        let metadata = build_metadata(&["eng".to_string()], &output);

        assert!(metadata.additional.get("mean_text_conf").is_none());
        assert!(metadata.additional.get("word_count").is_none());
    }

    /// Regression test for issue #186: `psm` must not carry Tesseract's PSM-3
    /// default, since Sceptre has no Page Segmentation Mode concept.
    #[test]
    fn should_not_report_tesseract_psm_for_sceptre_metadata() {
        let output = BlockingOutput {
            result: OcrResult {
                lines: vec![line("hello", 0.9)],
            },
            width: 100,
            height: 100,
            #[cfg(auto_rotate)]
            orientation: None,
            #[cfg(auto_rotate)]
            auto_rotated: false,
        };

        let metadata = build_metadata(&["eng".to_string()], &output);
        let Some(FormatMetadata::Ocr(ocr_metadata)) = metadata.format else {
            panic!("expected FormatMetadata::Ocr");
        };
        assert_eq!(
            ocr_metadata.psm, 0,
            "Sceptre has no PSM; must not claim Tesseract's PSM 3"
        );
    }

    #[test]
    fn cache_key_should_change_with_reader_configuration() {
        let first = sceptre::OcrConfig::default();
        let mut second = first.clone();
        second.recognition.batch_size = 8;
        assert_ne!(
            reader_cache_key(&first).expect("key"),
            reader_cache_key(&second).expect("key")
        );
    }

    #[tokio::test]
    async fn reader_cache_should_reuse_cell_for_same_key() {
        let backend = SceptreOcrBackend::new().expect("backend should construct");
        let first = backend.reader_cell("english").await.expect("cache lookup");
        let second = backend.reader_cell("english").await.expect("cache lookup");

        assert!(Arc::ptr_eq(&first, &second));
        let cache = backend.readers.lock().expect("cache lock");
        assert_eq!(cache.entries.len(), 1);
        assert_eq!(cache.recency, VecDeque::from(["english".to_string()]));
    }

    #[tokio::test]
    async fn reader_cache_should_evict_least_recently_used_inactive_cell() {
        let backend = SceptreOcrBackend::new().expect("backend should construct");
        for index in 0..READER_CACHE_CAPACITY {
            drop(
                backend
                    .reader_cell(&format!("reader-{index}"))
                    .await
                    .expect("cache lookup"),
            );
        }

        drop(backend.reader_cell("reader-0").await.expect("cache lookup"));
        drop(backend.reader_cell("replacement").await.expect("cache lookup"));

        let cache = backend.readers.lock().expect("cache lock");
        assert_eq!(cache.entries.len(), READER_CACHE_CAPACITY);
        assert!(cache.entries.contains_key("reader-0"));
        assert!(!cache.entries.contains_key("reader-1"));
        assert!(cache.entries.contains_key("replacement"));
        assert_eq!(
            cache.recency,
            VecDeque::from([
                "reader-2".to_string(),
                "reader-3".to_string(),
                "reader-0".to_string(),
                "replacement".to_string(),
            ])
        );
    }

    #[tokio::test]
    async fn reader_cache_should_bound_distinct_overflow_admission() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        const OVERFLOW_REQUESTS: usize = 8;

        let backend = Arc::new(SceptreOcrBackend::new().expect("backend should construct"));
        let mut cached = Vec::with_capacity(READER_CACHE_CAPACITY);
        for index in 0..READER_CACHE_CAPACITY {
            cached.push(
                backend
                    .reader_cell(&format!("reader-{index}"))
                    .await
                    .expect("cache lookup"),
            );
        }

        let first_overflow = backend.reader_cell("overflow-0").await.expect("overflow admission");
        let same_overflow = backend.reader_cell("overflow-0").await.expect("overflow reuse");
        assert!(Arc::ptr_eq(&first_overflow, &same_overflow));

        let active_overflow = Arc::new(AtomicUsize::new(0));
        let maximum_overflow = Arc::new(AtomicUsize::new(0));
        let mut tasks = Vec::with_capacity(OVERFLOW_REQUESTS);
        for index in 1..=OVERFLOW_REQUESTS {
            let backend = Arc::clone(&backend);
            let active_overflow = Arc::clone(&active_overflow);
            let maximum_overflow = Arc::clone(&maximum_overflow);
            tasks.push(tokio::spawn(async move {
                let cell = backend
                    .reader_cell(&format!("overflow-{index}"))
                    .await
                    .expect("overflow admission");
                let active = active_overflow.fetch_add(1, Ordering::SeqCst) + 1;
                maximum_overflow.fetch_max(active, Ordering::SeqCst);
                tokio::task::yield_now().await;
                active_overflow.fetch_sub(1, Ordering::SeqCst);
                drop(cell);
            }));
        }

        tokio::task::yield_now().await;
        assert_eq!(maximum_overflow.load(Ordering::SeqCst), 0);
        drop(same_overflow);
        drop(first_overflow);
        for task in tasks {
            task.await.expect("overflow task");
        }

        assert_eq!(maximum_overflow.load(Ordering::SeqCst), READER_OVERFLOW_CAPACITY);
        assert_eq!(backend.overflow_slots.available_permits(), READER_OVERFLOW_CAPACITY);
        let cache = backend.readers.lock().expect("cache lock");
        assert_eq!(cache.entries.len(), READER_CACHE_CAPACITY);
        assert!(cache.overflow.len() <= READER_OVERFLOW_CAPACITY);
        drop(cache);
        drop(cached);
    }

    #[tokio::test]
    async fn should_reject_invalid_image_without_loading_models() {
        let backend = SceptreOcrBackend::new().expect("backend should construct");
        let error = backend
            .process_image(b"not an image", &OcrConfig::default())
            .await
            .expect_err("invalid image data must fail");

        assert_eq!(error.to_string(), "OCR error: Sceptre OCR operation failed");
        let XbergError::Ocr { source, .. } = error else {
            panic!("invalid Sceptre image data must return an OCR error");
        };
        assert!(
            source.is_some(),
            "the Sceptre decode error must remain in the source chain"
        );
    }

    #[test]
    fn plugin_version_matches_sceptre_crate() {
        let backend = SceptreOcrBackend::new().expect("backend should construct");
        assert_eq!(backend.version(), sceptre::VERSION);
    }
}
