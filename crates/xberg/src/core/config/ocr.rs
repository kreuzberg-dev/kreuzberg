//! OCR configuration.
//!
//! Defines OCR-specific configuration including backend selection, language settings,
//! Tesseract-specific parameters, quality thresholds, and multi-backend pipeline config.

use serde::{Deserialize, Deserializer, Serialize};
use std::path::PathBuf;

use super::formats::OutputFormat;
#[cfg(test)]
use crate::core::config_validation::validate_ocr_backend;
#[cfg(test)]
use crate::error::XbergError;
use crate::types::OcrElementConfig;

/// Deserialize a language field that accepts either a string or a list of strings.
///
/// This helper enables backward compatibility: old configs with `language: "eng"`
/// deserialize to `Vec<String>` via coercion, while new configs with
/// `language: ["eng", "deu"]` deserialize directly.
fn deserialize_languages<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: Deserializer<'de>,
{
    use serde::de::Error;

    let value: serde_json::Value = serde_json::Value::deserialize(deserializer)?;

    match value {
        serde_json::Value::String(s) => {
            if s.contains('+') {
                Ok(s.split('+').map(|l| l.to_string()).collect())
            } else {
                Ok(vec![s])
            }
        }
        serde_json::Value::Array(arr) => arr
            .into_iter()
            .map(|v| {
                v.as_str()
                    .map(String::from)
                    .ok_or_else(|| Error::custom("each language must be a string"))
            })
            .collect(),
        _ => Err(Error::custom(
            "language must be a string (e.g., \"eng\") or an array of strings (e.g., [\"eng\", \"deu\"])",
        )),
    }
}

/// Deserialize an optional language field (for pipeline stages).
fn deserialize_optional_languages<'de, D>(deserializer: D) -> Result<Option<Vec<String>>, D::Error>
where
    D: Deserializer<'de>,
{
    use serde::de::Error;

    let value: Option<serde_json::Value> = Option::deserialize(deserializer)?;

    match value {
        None => Ok(None),
        Some(serde_json::Value::String(s)) => {
            if s.contains('+') {
                Ok(Some(s.split('+').map(|l| l.to_string()).collect()))
            } else {
                Ok(Some(vec![s]))
            }
        }
        Some(serde_json::Value::Array(arr)) => {
            let langs: Result<Vec<String>, D::Error> = arr
                .into_iter()
                .map(|v| {
                    v.as_str()
                        .map(String::from)
                        .ok_or_else(|| Error::custom("each language must be a string"))
                })
                .collect();
            langs.map(Some)
        }
        Some(_) => Err(Error::custom("language must be a string or an array of strings")),
    }
}

/// Quality thresholds for OCR fallback decisions and pipeline quality gating.
///
/// All fields default to the values that match the previous hardcoded behavior,
/// so `OcrQualityThresholds::default()` preserves existing semantics exactly.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OcrQualityThresholds {
    /// Minimum total non-whitespace characters to consider text substantive.
    #[serde(default = "default_min_total_non_whitespace")]
    pub min_total_non_whitespace: usize,

    /// Minimum non-whitespace characters per page on average.
    #[serde(default = "default_min_non_whitespace_per_page")]
    pub min_non_whitespace_per_page: f64,

    /// Minimum character count for a word to be "meaningful".
    #[serde(default = "default_min_meaningful_word_len")]
    pub min_meaningful_word_len: usize,

    /// Minimum count of meaningful words before text is accepted.
    #[serde(default = "default_min_meaningful_words")]
    pub min_meaningful_words: usize,

    /// Minimum alphanumeric ratio (non-whitespace chars that are alphanumeric).
    #[serde(default = "default_min_alnum_ratio")]
    pub min_alnum_ratio: f64,

    /// Minimum Unicode replacement characters (U+FFFD) to trigger OCR fallback.
    #[serde(default = "default_min_garbage_chars")]
    pub min_garbage_chars: usize,

    /// Maximum fraction of short (1-2 char) words before text is considered fragmented.
    #[serde(default = "default_max_fragmented_word_ratio")]
    pub max_fragmented_word_ratio: f64,

    /// Critical fragmentation threshold — triggers OCR regardless of meaningful words.
    /// Normal English text has ~20-30% short words. 80%+ is definitive garbage.
    #[serde(default = "default_critical_fragmented_word_ratio")]
    pub critical_fragmented_word_ratio: f64,

    /// Minimum average word length. Below this with enough words indicates garbled extraction.
    #[serde(default = "default_min_avg_word_length")]
    pub min_avg_word_length: f64,

    /// Minimum word count before average word length check applies.
    #[serde(default = "default_min_words_for_avg_length_check")]
    pub min_words_for_avg_length_check: usize,

    /// Minimum consecutive word repetition ratio to detect column scrambling.
    #[serde(default = "default_min_consecutive_repeat_ratio")]
    pub min_consecutive_repeat_ratio: f64,

    /// Minimum word count before consecutive repetition check is applied.
    #[serde(default = "default_min_words_for_repeat_check")]
    pub min_words_for_repeat_check: usize,

    /// Minimum character count for "substantive markdown" OCR skip gate.
    #[serde(default = "default_substantive_min_chars")]
    pub substantive_min_chars: usize,

    /// Minimum character count for "non-text content" OCR skip gate.
    #[serde(default = "default_non_text_min_chars")]
    pub non_text_min_chars: usize,

    /// Alphanumeric+whitespace ratio threshold for skip decisions.
    #[serde(default = "default_alnum_ws_ratio_threshold")]
    pub alnum_ws_ratio_threshold: f64,

    /// Minimum quality score (0.0-1.0) for a pipeline stage result to be accepted.
    /// If the result from a backend scores below this, try the next backend.
    #[serde(default = "default_pipeline_min_quality")]
    pub pipeline_min_quality: f64,

    /// Minimum fraction of non-whitespace characters that are undecodable
    /// (Unicode Private Use Area, replacement characters, or non-whitespace
    /// control characters) before a page's text layer is treated as
    /// unreadable and routed to OCR (issue #1254). Gated by
    /// `min_total_non_whitespace` so short snippets with a stray symbol or
    /// two do not trip this check.
    #[serde(default = "default_min_undecodable_ratio")]
    pub min_undecodable_ratio: f64,

    /// Whether to route a page to OCR when pdf_oxide reports that a high
    /// fraction of its text was fabricated rather than read from the file
    /// (`MappingProvenance::Fallback`, pdf_oxide 0.3.75+, issue #1254). This is
    /// a direct fact from the extractor's ISO 32000-1 §9.10.2 mapping cascade,
    /// distinct from the character-heuristic proxy behind `min_undecodable_ratio`.
    /// Defaults to `true`.
    #[serde(default = "default_enable_provenance_ocr_routing")]
    pub enable_provenance_ocr_routing: bool,

    /// Minimum fraction of a page's non-whitespace characters with
    /// `MappingProvenance::Fallback` provenance before the page is treated as
    /// having a fabricated text layer and routed to OCR (issue #1254). Gated by
    /// `min_total_non_whitespace` so a short page with a stray fallback
    /// character cannot trip it. Only used when `enable_provenance_ocr_routing`
    /// is `true`.
    #[serde(default = "default_min_provenance_fallback_ratio")]
    pub min_provenance_fallback_ratio: f64,
}

impl Default for OcrQualityThresholds {
    fn default() -> Self {
        Self {
            min_total_non_whitespace: 64,
            min_non_whitespace_per_page: 32.0,
            min_meaningful_word_len: 4,
            min_meaningful_words: 3,
            min_alnum_ratio: 0.3,
            min_garbage_chars: 5,
            max_fragmented_word_ratio: 0.6,
            critical_fragmented_word_ratio: 0.80,
            min_avg_word_length: 2.0,
            min_words_for_avg_length_check: 50,
            min_consecutive_repeat_ratio: 0.08,
            min_words_for_repeat_check: 50,
            substantive_min_chars: 100,
            non_text_min_chars: 20,
            alnum_ws_ratio_threshold: 0.4,
            pipeline_min_quality: 0.5,
            min_undecodable_ratio: default_min_undecodable_ratio(),
            enable_provenance_ocr_routing: default_enable_provenance_ocr_routing(),
            min_provenance_fallback_ratio: default_min_provenance_fallback_ratio(),
        }
    }
}

fn default_min_total_non_whitespace() -> usize {
    64
}
fn default_min_non_whitespace_per_page() -> f64 {
    32.0
}
fn default_min_meaningful_word_len() -> usize {
    4
}
fn default_min_meaningful_words() -> usize {
    3
}
fn default_min_alnum_ratio() -> f64 {
    0.3
}
fn default_min_garbage_chars() -> usize {
    5
}
fn default_max_fragmented_word_ratio() -> f64 {
    0.6
}
fn default_critical_fragmented_word_ratio() -> f64 {
    0.80
}
fn default_min_avg_word_length() -> f64 {
    2.0
}
fn default_min_words_for_avg_length_check() -> usize {
    50
}
fn default_min_consecutive_repeat_ratio() -> f64 {
    0.08
}
fn default_min_words_for_repeat_check() -> usize {
    50
}
fn default_substantive_min_chars() -> usize {
    100
}
fn default_non_text_min_chars() -> usize {
    20
}
fn default_alnum_ws_ratio_threshold() -> f64 {
    0.4
}
fn default_pipeline_min_quality() -> f64 {
    0.5
}
/// Pages at or above this fraction of undecodable (PUA/replacement/control-garbage)
/// characters are treated as having an unreadable text layer (issue #1254).
fn default_min_undecodable_ratio() -> f64 {
    0.5
}
/// Provenance-based fabricated-text OCR routing is on by default: it is a
/// direct fact from pdf_oxide 0.3.75+, not a heuristic, so false positives are
/// rare (issue #1254).
fn default_enable_provenance_ocr_routing() -> bool {
    true
}
/// Pages at or above this fraction of `MappingProvenance::Fallback` characters
/// are treated as having a fabricated text layer (issue #1254).
fn default_min_provenance_fallback_ratio() -> f64 {
    0.5
}

/// A single backend stage in the OCR pipeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OcrPipelineStage {
    /// Backend name: "tesseract", "paddleocr", "paddle-ocr", "sceptre", "vlm", or a custom registered name.
    /// Sceptre uses ONNX Runtime on desktop/server and tract on Android/iOS; browser WebAssembly has a separate
    /// byte-fed engine because the normal async OCR registry assumes native model storage.
    pub backend: String,

    /// Priority weight (higher = tried first). Stages are sorted by priority descending.
    #[serde(default = "default_priority")]
    pub priority: u32,

    /// Language override for this stage (None = use parent OcrConfig.language).
    /// Accepts either a single language code ("eng") or a list (["eng", "deu"]).
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_optional_languages"
    )]
    pub language: Option<Vec<String>>,

    /// Tesseract-specific config override for this stage.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tesseract_config: Option<crate::types::TesseractConfig>,

    /// PaddleOCR-specific config for this stage.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paddle_ocr_config: Option<serde_json::Value>,

    /// VLM config override for this pipeline stage.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vlm_config: Option<super::llm::LlmConfig>,

    /// Arbitrary per-call options passed through to the backend unchanged.
    ///
    /// Backends that support runtime tuning (mode switching, preprocessing
    /// flags, inference parameters, etc.) read this value and deserialize
    /// the keys they care about. Keys unknown to the backend are silently
    /// ignored, so options from different backends can coexist in the same
    /// config without conflict.
    ///
    /// Example (custom backend):
    /// ```json
    /// { "mode": "fast", "enable_layout": true }
    /// ```
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend_options: Option<serde_json::Value>,
}

fn default_priority() -> u32 {
    100
}

/// Selection policy for a pipeline's best-effort fallback: which stage's result to
/// return when no stage clears `quality_thresholds.pipeline_min_quality`.
///
/// Internal (`pub(crate)`) rather than a field on [`OcrPipelineConfig`]: it is derived
/// from the [`OcrConfig`] that produced the pipeline (see [`OcrConfig::pipeline_selection`]),
/// so keeping it off the binding-facing config avoids a public API break and a full
/// binding regeneration for a behavior-only fix (#1341).
#[cfg(all(feature = "pdf", any(feature = "ocr", feature = "ocr-pipeline")))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum OcrPipelineSelection {
    /// Return the highest-scoring stage result (default).
    ///
    /// Correct for pipelines where every stage is an independent attempt at the
    /// same OCR problem, so the surface quality score is a reasonable proxy for
    /// which result to keep.
    #[default]
    HighestScore,

    /// Return the last stage that produced non-empty text, regardless of score.
    ///
    /// Intended for fallback pipelines (e.g. `vlm_fallback`) where a later stage
    /// only runs because the earlier stage(s) were judged inadequate — so a later
    /// non-empty result is a deliberate override, not noise, even when it scores
    /// lower than an earlier stage's output (#1341). An empty result never
    /// overwrites a prior non-empty one.
    PreferLastNonEmpty,
}

/// Multi-backend OCR pipeline with quality-based fallback.
///
/// Backends are tried in priority order (highest first). After each backend
/// produces output, quality is evaluated. If it meets `quality_thresholds.pipeline_min_quality`,
/// the result is accepted. Otherwise the next backend is tried; if none clears the
/// threshold, an internal selection policy derived from the `OcrConfig` decides which
/// stage's result is returned as the best effort (`vlm_fallback` pipelines prefer their
/// last non-empty stage; explicit and classical pipelines stay score-based).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OcrPipelineConfig {
    /// Ordered list of backends to try. Sorted by priority (descending) at runtime.
    pub stages: Vec<OcrPipelineStage>,

    /// Quality thresholds for deciding whether to accept a result or try the next backend.
    #[serde(default)]
    pub quality_thresholds: OcrQualityThresholds,
}

/// Policy controlling when VLM (Vision Language Model) OCR is used as a fallback.
///
/// This knob is syntactic sugar over the explicit [`OcrPipelineConfig`] stage
/// ordering. When `vlm_fallback` is set and `pipeline` is `None`, an equivalent
/// pipeline is synthesised at extraction time:
///
/// - [`VlmFallbackPolicy::Disabled`] — no synthesis; single-backend mode (default).
/// - [`VlmFallbackPolicy::OnLowQuality`] — tries the classical backend first; if the
///   result scores below `quality_threshold`, tries VLM.
/// - [`VlmFallbackPolicy::Always`] — skips the classical backend and sends every page
///   to the VLM.
///
/// When [`OcrConfig::pipeline`] is explicitly set, `vlm_fallback` is ignored — the
/// explicit pipeline takes precedence.
///
/// # Errors
///
/// Both `OnLowQuality` and `Always` require [`OcrConfig::vlm_config`] to be `Some`.
/// Constructing an [`OcrConfig`] with one of these policies but no `vlm_config` is
/// detected by `OcrConfig::validate` and will surface as a
/// [`crate::XbergError::Validation`] error at extraction time, not a panic.
///
/// # Example
///
/// ```rust
/// use xberg::{OcrConfig, VlmFallbackPolicy, LlmConfig};
///
/// # fn example() -> xberg::Result<()> {
/// let config = OcrConfig {
///     vlm_fallback: VlmFallbackPolicy::OnLowQuality { quality_threshold: 0.6 },
///     vlm_config: Some(LlmConfig {
///         model: "openai/gpt-4o-mini".to_string(),
///         ..Default::default()
///     }),
///     ..Default::default()
/// };
///
/// // Threshold calibration is deferred to the Stage 0 benchmark harness.
/// assert!(matches!(config.vlm_fallback, VlmFallbackPolicy::OnLowQuality { .. }));
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum VlmFallbackPolicy {
    /// No VLM fallback (default). Behaves identically to the pre-policy single-backend mode.
    #[default]
    Disabled,

    /// Try the classical OCR backend first. If the quality score is below
    /// `quality_threshold`, send the page to the VLM.
    ///
    /// `quality_threshold` is in the `[0.0, 1.0]` range produced by
    /// [`crate::text::quality::calculate_quality_score`]. A value of `0.5` is a
    /// reasonable starting point; calibrate with the Stage 0 benchmark harness.
    OnLowQuality {
        /// Minimum acceptable quality score from the classical backend.
        /// Pages scoring below this are retried with VLM.
        quality_threshold: f64,
    },

    /// Skip the classical OCR backend entirely. Every page is sent to the VLM.
    Always,
}

/// Default confidence a page must reach before [`OcrStrategy::ScannedPages`] OCRs it.
///
/// A slide with a full-bleed background image scores `0.50`, so a threshold of
/// `0.50` or lower also sends such slides to OCR.
pub const DEFAULT_SCANNED_MIN_CONFIDENCE: f64 = 0.70;

/// Which pages of a PDF get OCR'd when neither `force_ocr` nor `force_ocr_pages` applies.
///
/// # Examples
///
/// ```
/// use xberg::{ExtractionConfig, OcrStrategy};
///
/// // OCR pages that look like scans; keep native text everywhere else.
/// let config = ExtractionConfig {
///     ocr_strategy: OcrStrategy::ScannedPages { min_confidence: 0.7 },
///     ..Default::default()
/// };
/// assert!(matches!(config.ocr_strategy, OcrStrategy::ScannedPages { .. }));
/// ```
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum OcrStrategy {
    /// OCR only when the native text layer fails a quality check (default).
    ///
    /// A scanner's invisible OCR sidecar passes that check, so scanned pages
    /// carrying one are extracted natively. Use [`OcrStrategy::ScannedPages`]
    /// to OCR them instead.
    #[default]
    Auto,

    /// Additionally OCR every page that looks like a scan.
    ///
    /// Pages are graded on raster coverage, whether the text layer is invisible
    /// or absent, the image codec, and the producer. Pages at or above
    /// `min_confidence` are OCR'd; the rest keep native text and still go through
    /// the `Auto` quality check.
    ///
    /// Detects that a text layer came from a scanner, not whether it is accurate,
    /// so a page carrying a good sidecar is OCR'd too.
    ScannedPages {
        /// Minimum scan confidence, in `[0.0, 1.0]`. Values outside the range are
        /// clamped. See [`DEFAULT_SCANNED_MIN_CONFIDENCE`] for how to pick one.
        min_confidence: f64,
    },
}

impl OcrStrategy {
    /// Confidence a page must reach to count as a scan, clamped to `[0.0, 1.0]`.
    ///
    /// [`OcrStrategy::Auto`] uses the default: it does not select pages by scan
    /// confidence, but `scanned_pages` metadata still needs a threshold.
    #[must_use]
    pub fn effective_min_confidence(&self) -> f64 {
        match self {
            Self::Auto => DEFAULT_SCANNED_MIN_CONFIDENCE,
            Self::ScannedPages { min_confidence } => min_confidence.clamp(0.0, 1.0),
        }
    }
}

/// OCR configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OcrConfig {
    /// Whether OCR is enabled.
    ///
    /// Setting `enabled: false` is a shorthand for `disable_ocr: true` on the parent
    /// [`ExtractionConfig`](crate::core::config::ExtractionConfig). Images return
    /// metadata only; PDFs use native text extraction without OCR fallback.
    ///
    /// Defaults to `true`. When `false`, all other OCR settings are ignored.
    #[serde(default = "default_ocr_enabled")]
    pub enabled: bool,

    /// OCR backend: tesseract, paddleocr, paddle-ocr, sceptre, or vlm.
    /// Sceptre uses ONNX Runtime on desktop/server and tract on supported mobile builds.
    /// Browser WebAssembly uses the separate byte-fed Sceptre worker API.
    #[serde(default = "default_tesseract_backend")]
    pub backend: String,

    /// Language code(s) for OCR recognition.
    /// Accepts either a single language code ("eng") or a list (["eng", "deu"]).
    /// Defaults to ["eng"]. For Tesseract, languages are joined with "+".
    #[serde(default = "default_eng", deserialize_with = "deserialize_languages")]
    pub language: Vec<String>,

    /// Tesseract-specific configuration (optional)
    #[serde(default)]
    pub tesseract_config: Option<crate::types::TesseractConfig>,

    /// Output format for OCR results (optional, for format conversion)
    #[serde(default)]
    pub output_format: Option<OutputFormat>,

    /// PaddleOCR-specific configuration (optional, JSON passthrough).
    ///
    /// Deserialized into a [`PaddleOcrConfig`](crate::PaddleOcrConfig), so any of its fields can be
    /// overridden here — most notably `model_version` (`"pp-ocrv6"` default / `"pp-ocrv5"`) and
    /// `model_tier`. In TOML:
    ///
    /// ```toml
    /// [ocr.paddle_ocr_config]
    /// model_version = "pp-ocrv5"
    /// model_tier = "server"
    /// ```
    ///
    /// The `XBERG_OCR_MODEL_VERSION` / `XBERG_OCR_MODEL_TIER` environment variables set the same two
    /// keys for env-configured servers (issue #1279).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paddle_ocr_config: Option<serde_json::Value>,

    /// Arbitrary per-call options passed through to the backend unchanged.
    ///
    /// Custom OCR backends and built-in backends that support runtime tuning
    /// can read this value and deserialize the keys they care about. Keys
    /// unknown to the backend are silently ignored.
    ///
    /// This is the recommended extension point for per-call parameters that
    /// are not covered by the typed fields above (e.g. mode switching,
    /// preprocessing flags, inference batch size).
    ///
    /// **Scope:** when `pipeline` is `None`, this value is propagated to the
    /// primary stage of the auto-constructed pipeline. When `pipeline` is
    /// explicitly set, this field has **no effect** — the caller must set
    /// `OcrPipelineStage.backend_options` directly on the relevant stage(s)
    /// instead.
    ///
    /// Example:
    /// ```json
    /// { "mode": "fast", "enable_layout": true, "timeout_ms": 5000 }
    /// ```
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend_options: Option<serde_json::Value>,

    /// OCR element extraction configuration
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub element_config: Option<OcrElementConfig>,

    /// Quality thresholds for the native-text-to-OCR fallback decision.
    /// When None, uses compiled defaults (matching previous hardcoded behavior).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quality_thresholds: Option<OcrQualityThresholds>,

    /// Multi-backend OCR pipeline configuration. When set, enables weighted
    /// fallback across multiple OCR backends based on output quality.
    /// When None, uses the single `backend` field (same as today).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pipeline: Option<OcrPipelineConfig>,

    /// Enable automatic page rotation based on orientation detection.
    ///
    /// When enabled, uses Tesseract's `DetectOrientationScript()` to detect
    /// page orientation (0/90/180/270 degrees) before OCR. If the page is
    /// rotated with high confidence, the image is corrected before recognition.
    /// This is critical for handling rotated scanned documents.
    #[serde(default)]
    pub auto_rotate: bool,

    /// Ergonomic VLM fallback policy.
    ///
    /// When set to anything other than [`VlmFallbackPolicy::Disabled`] and
    /// [`OcrConfig::pipeline`] is `None`, a multi-stage pipeline is synthesised
    /// automatically:
    ///
    /// - [`VlmFallbackPolicy::OnLowQuality`] → `[classical_stage, vlm_stage]` with the
    ///   `quality_threshold` mapped onto [`OcrQualityThresholds::pipeline_min_quality`].
    /// - [`VlmFallbackPolicy::Always`] → `[vlm_stage]` only.
    ///
    /// Requires [`OcrConfig::vlm_config`] to be `Some` when not `Disabled`.
    /// When [`OcrConfig::pipeline`] is explicitly set, this field is ignored.
    #[serde(default)]
    pub vlm_fallback: VlmFallbackPolicy,

    /// VLM (Vision Language Model) OCR configuration.
    ///
    /// Required when `backend` is `"vlm"` or when `vlm_fallback` is not
    /// [`VlmFallbackPolicy::Disabled`]. Uses liter-llm to send page images to a
    /// vision model for text extraction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vlm_config: Option<super::llm::LlmConfig>,

    /// Custom Jinja2 prompt template for VLM OCR.
    ///
    /// When `None`, uses the default template. Available variables:
    /// - `{{ language }}` — The document language code (e.g., "eng", "deu").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vlm_prompt: Option<String>,

    /// Hardware acceleration for ONNX Runtime models (e.g. PaddleOCR, layout detection).
    ///
    /// Not user-configurable via config files — injected at runtime from
    /// `ExtractionConfig::acceleration` before each `process_image` call.
    #[serde(skip)]
    pub acceleration: Option<super::acceleration::AccelerationConfig>,

    /// Caller-supplied Tesseract `traineddata` bytes per language code.
    ///
    /// Primary use case is the WASM build, which has no filesystem and cannot
    /// download tessdata at runtime. Native builds typically rely on
    /// `TessdataManager` and ignore this field. When present, the WASM
    /// Tesseract backend prefers these bytes over its compile-time-bundled
    /// English data.
    ///
    /// Skipped by serde to keep config files small — supply via the typed API
    /// at runtime.
    #[serde(skip)]
    pub tessdata_bytes: Option<std::collections::HashMap<String, Vec<u8>>>,

    /// Runtime override for tessdata directory path.
    ///
    /// When set, uses this path as the highest-priority tessdata location,
    /// bypassing environment variables and cache directories. Useful for
    /// embedding pre-installed tessdata in applications. When `None`, uses
    /// the standard resolution chain: TESSDATA_PREFIX env, cache dir, system paths.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tessdata_path: Option<PathBuf>,
}

impl Default for OcrConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            backend: default_tesseract_backend(),
            language: vec!["eng".to_string()],
            tesseract_config: None,
            output_format: None,
            paddle_ocr_config: None,
            backend_options: None,
            element_config: None,
            quality_thresholds: None,
            pipeline: None,
            auto_rotate: false,
            vlm_fallback: VlmFallbackPolicy::Disabled,
            vlm_config: None,
            vlm_prompt: None,
            acceleration: None,
            tessdata_bytes: None,
            tessdata_path: None,
        }
    }
}

impl OcrConfig {
    /// Validates that the configured backend is supported.
    ///
    /// This method checks that the backend name is one of the supported OCR backends:
    /// - tesseract
    /// - paddleocr
    /// - paddle-ocr
    /// - sceptre
    /// - vlm
    ///
    /// Typos in backend names are caught at configuration validation time, not at runtime.
    /// Also validates pipeline stage backends when a pipeline is configured.
    ///
    /// When `vlm_fallback` is not `Disabled` and no explicit `pipeline` is set,
    /// `vlm_config` must be `Some`. A missing `vlm_config` in that case is a
    /// configuration error detected here, not at runtime.
    #[cfg(test)]
    pub(crate) fn validate(&self) -> Result<(), XbergError> {
        validate_ocr_backend(&self.backend)?;
        crate::core::config_validation::validate_vlm_backend_config(&self.backend, self.vlm_config.as_ref())?;
        if let Some(ref pipeline) = self.pipeline {
            for stage in &pipeline.stages {
                validate_ocr_backend(&stage.backend)?;
                crate::core::config_validation::validate_vlm_backend_config(&stage.backend, stage.vlm_config.as_ref())?;
            }
        } else if self.vlm_fallback != VlmFallbackPolicy::Disabled && self.vlm_config.is_none() {
            return Err(XbergError::validation(
                "vlm_fallback is set but vlm_config is missing; \
                 provide an LlmConfig with the model and API key"
                    .to_string(),
            ));
        }
        Ok(())
    }

    /// Effective OCR languages, with the documented default applied.
    ///
    /// Blank entries are dropped and an empty list falls back to
    /// [`DEFAULT_OCR_LANGUAGE`]. OCR backends call this instead of reading
    /// [`language`](Self::language) directly so an unset language never reaches
    /// an engine as an empty language string — which Tesseract would otherwise
    /// try to load as a language pack named `""`, surfacing as a confusing
    /// "Failed to download language pack ''" error.
    #[cfg(any(
        feature = "ocr",
        feature = "ocr-wasm",
        feature = "paddle-ocr",
        all(feature = "liter-llm", not(target_arch = "wasm32")),
    ))]
    pub(crate) fn effective_languages(&self) -> Vec<String> {
        let langs: Vec<String> = self
            .language
            .iter()
            .map(|lang| lang.trim())
            .filter(|lang| !lang.is_empty())
            .map(str::to_string)
            .collect();
        if langs.is_empty() {
            vec![DEFAULT_OCR_LANGUAGE.to_string()]
        } else {
            langs
        }
    }

    /// Returns the effective quality thresholds, using configured values or defaults.
    #[cfg(all(any(feature = "ocr", feature = "ocr-pipeline"), feature = "pdf"))]
    pub(crate) fn effective_thresholds(&self) -> OcrQualityThresholds {
        self.quality_thresholds.clone().unwrap_or_default()
    }

    /// Returns the effective pipeline config.
    ///
    /// Priority order:
    /// 1. If `pipeline` is explicitly set, return it unchanged.
    /// 2. If `vlm_fallback` is `OnLowQuality` or `Always` (and `pipeline` is
    ///    `None`), synthesise a pipeline from the policy:
    ///    - `OnLowQuality { quality_threshold }` → `[classical_stage @ 100, vlm_stage @ 50]`
    ///      with `quality_thresholds.pipeline_min_quality = quality_threshold`.
    ///    - `Always` → `[vlm_stage @ 100]` only (no classical stage).
    ///      Returns `None` if `vlm_config` is not set (misconfiguration; surfaces at
    ///      call-time as a logged warning rather than a panic — [`validate`] catches it
    ///      at config-load time).
    /// 3. If `paddle-ocr` is compiled in and the backend is the default (tesseract),
    ///    auto-constructs `[tesseract @ 100, paddleocr @ 50]`.
    /// 4. Otherwise returns `None` (single-backend mode).
    ///
    /// Explicit non-default backend selections are honored as-is — a silent
    /// paddleocr fallback would mask errors from the chosen backend.
    #[cfg(all(any(feature = "ocr", feature = "ocr-pipeline"), feature = "pdf"))]
    pub(crate) fn effective_pipeline(&self) -> Option<OcrPipelineConfig> {
        if self.pipeline.is_some() {
            return self.pipeline.clone();
        }

        match &self.vlm_fallback {
            VlmFallbackPolicy::OnLowQuality { quality_threshold } => {
                let Some(vlm_cfg) = self.vlm_config.clone() else {
                    tracing::warn!(
                        "vlm_fallback=OnLowQuality is set but vlm_config is missing; \
                         falling through to single-backend mode"
                    );
                    return self.effective_pipeline_classical();
                };
                let mut thresholds = self.effective_thresholds();
                thresholds.pipeline_min_quality = *quality_threshold;
                let classical_stage = OcrPipelineStage {
                    backend: self.backend.clone(),
                    priority: 100,
                    language: if self.language.len() == 1 && self.language[0] == "eng" {
                        None
                    } else {
                        Some(self.language.clone())
                    },
                    tesseract_config: self.tesseract_config.clone(),
                    paddle_ocr_config: None,
                    vlm_config: None,
                    backend_options: self.backend_options.clone(),
                };
                let vlm_stage = OcrPipelineStage {
                    backend: "vlm".to_string(),
                    priority: 50,
                    language: None,
                    tesseract_config: None,
                    paddle_ocr_config: None,
                    vlm_config: Some(vlm_cfg),
                    backend_options: None,
                };
                return Some(OcrPipelineConfig {
                    stages: vec![classical_stage, vlm_stage],
                    quality_thresholds: thresholds,
                });
            }
            VlmFallbackPolicy::Always => {
                let Some(vlm_cfg) = self.vlm_config.clone() else {
                    tracing::warn!(
                        "vlm_fallback=Always is set but vlm_config is missing; \
                         falling through to single-backend mode"
                    );
                    return self.effective_pipeline_classical();
                };
                let vlm_stage = OcrPipelineStage {
                    backend: "vlm".to_string(),
                    priority: 100,
                    language: None,
                    tesseract_config: None,
                    paddle_ocr_config: None,
                    vlm_config: Some(vlm_cfg),
                    backend_options: None,
                };
                return Some(OcrPipelineConfig {
                    stages: vec![vlm_stage],
                    quality_thresholds: self.effective_thresholds(),
                });
            }
            VlmFallbackPolicy::Disabled => {}
        }

        self.effective_pipeline_classical()
    }

    /// Classical pipeline synthesis: paddle-ocr auto-fallback or `None`.
    ///
    /// Extracted so the vlm_fallback paths can fall through cleanly without
    /// duplicating the paddle-ocr conditional compilation block.
    #[cfg(all(any(feature = "ocr", feature = "ocr-pipeline"), feature = "pdf"))]
    fn effective_pipeline_classical(&self) -> Option<OcrPipelineConfig> {
        #[cfg(feature = "paddle-ocr")]
        {
            if self.backend != default_tesseract_backend() {
                return None;
            }

            let stages = vec![
                OcrPipelineStage {
                    backend: self.backend.clone(),
                    priority: 100,
                    language: if self.language.len() == 1 && self.language[0] == "eng" {
                        None
                    } else {
                        Some(self.language.clone())
                    },
                    tesseract_config: self.tesseract_config.clone(),
                    paddle_ocr_config: None,
                    vlm_config: self.vlm_config.clone(),
                    backend_options: self.backend_options.clone(),
                },
                OcrPipelineStage {
                    backend: "paddleocr".to_string(),
                    priority: 50,
                    language: None,
                    tesseract_config: None,
                    paddle_ocr_config: self.paddle_ocr_config.clone(),
                    vlm_config: None,
                    backend_options: None,
                },
            ];
            Some(OcrPipelineConfig {
                stages,
                quality_thresholds: self.effective_thresholds(),
            })
        }

        #[cfg(not(feature = "paddle-ocr"))]
        {
            None
        }
    }

    /// Best-effort selection policy for the pipeline this config produces.
    ///
    /// A `vlm_fallback` policy synthesises a fallback pipeline whose later VLM stage
    /// is a deliberate override of an inadequate earlier stage, so its best-effort
    /// result should be the last non-empty stage rather than the highest-scoring one
    /// (#1341). Explicit pipelines and the classical paddle auto-fallback keep the
    /// score-based selection. Mirrors the branch structure of [`Self::effective_pipeline`].
    #[cfg(all(feature = "pdf", any(feature = "ocr", feature = "ocr-pipeline")))]
    pub(crate) fn pipeline_selection(&self) -> OcrPipelineSelection {
        // Explicit pipelines keep score-based selection regardless of vlm_fallback
        // (they take precedence in `effective_pipeline`).
        if self.pipeline.is_some() {
            return OcrPipelineSelection::HighestScore;
        }
        // A synthesised vlm_fallback pipeline only adds its VLM stage when `vlm_config`
        // is present; without it, `effective_pipeline` falls through to the classical
        // (score-based) pipeline, so match that outcome here.
        if self.vlm_fallback != VlmFallbackPolicy::Disabled && self.vlm_config.is_some() {
            OcrPipelineSelection::PreferLastNonEmpty
        } else {
            OcrPipelineSelection::HighestScore
        }
    }
}

fn default_ocr_enabled() -> bool {
    true
}

fn default_tesseract_backend() -> String {
    "tesseract".to_string()
}

/// Default OCR language (Tesseract/ISO 639 naming): English.
///
/// Single source of truth for the empty-language fallback shared by every OCR
/// backend. Use [`OcrConfig::effective_languages`] rather than this constant
/// directly when a defaulted language list is needed.
pub(crate) const DEFAULT_OCR_LANGUAGE: &str = "eng";

fn default_eng() -> Vec<String> {
    vec![DEFAULT_OCR_LANGUAGE.to_string()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ocr_config_default() {
        let config = OcrConfig::default();
        assert_eq!(config.backend, "tesseract");
        assert_eq!(config.language, vec!["eng".to_string()]);
        assert!(config.tesseract_config.is_none());
        assert!(config.output_format.is_none());
    }

    #[test]
    fn test_ocr_config_with_tesseract() {
        let config = OcrConfig {
            backend: "tesseract".to_string(),
            language: vec!["fra".to_string()],
            ..Default::default()
        };
        assert_eq!(config.backend, "tesseract");
        assert_eq!(config.language, vec!["fra".to_string()]);
    }

    #[test]
    fn test_ocr_config_with_multiple_languages() {
        let config = OcrConfig {
            backend: "tesseract".to_string(),
            language: vec!["eng".to_string(), "deu".to_string()],
            ..Default::default()
        };
        assert_eq!(config.backend, "tesseract");
        assert_eq!(config.language, vec!["eng".to_string(), "deu".to_string()]);
    }

    #[test]
    fn test_language_deserialization_single_string() {
        let json = r#"{"language": "eng"}"#;
        let config: OcrConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.language, vec!["eng".to_string()]);
    }

    #[test]
    fn test_language_deserialization_array() {
        let json = r#"{"language": ["eng", "deu", "fra"]}"#;
        let config: OcrConfig = serde_json::from_str(json).unwrap();
        assert_eq!(
            config.language,
            vec!["eng".to_string(), "deu".to_string(), "fra".to_string()]
        );
    }

    #[test]
    fn test_language_deserialization_tesseract_format() {
        let json = r#"{"language": "eng+deu"}"#;
        let config: OcrConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.language, vec!["eng".to_string(), "deu".to_string()]);
    }

    #[test]
    fn test_validate_tesseract_backend() {
        let config = OcrConfig {
            backend: "tesseract".to_string(),
            ..Default::default()
        };
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_validate_unknown_backend_rejected() {
        let config = OcrConfig {
            backend: "unsupported-ocr".to_string(),
            ..Default::default()
        };
        let result = config.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Invalid OCR backend"));
    }

    #[test]
    fn test_validate_paddleocr_backend() {
        let config = OcrConfig {
            backend: "paddleocr".to_string(),
            ..Default::default()
        };
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_validate_invalid_backend_typo() {
        let config = OcrConfig {
            backend: "tesseract_typo".to_string(),
            ..Default::default()
        };
        let result = config.validate();
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("Invalid OCR backend"));
    }

    #[test]
    fn test_validate_invalid_backend_completely_wrong() {
        let config = OcrConfig {
            backend: "ocr_lib".to_string(),
            ..Default::default()
        };
        let result = config.validate();
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("Invalid OCR backend") || err_msg.contains("Valid options are"));
    }

    #[test]
    fn test_validate_default_backend() {
        let config = OcrConfig::default();
        assert!(config.validate().is_ok());
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_effective_languages_defaults_and_filters() {
        let empty = OcrConfig {
            language: vec![],
            ..Default::default()
        };
        assert_eq!(empty.effective_languages(), vec!["eng".to_string()]);

        let blank = OcrConfig {
            language: vec![String::new(), "   ".to_string()],
            ..Default::default()
        };
        assert_eq!(blank.effective_languages(), vec!["eng".to_string()]);

        let mixed = OcrConfig {
            language: vec!["eng".to_string(), " ".to_string(), " deu".to_string()],
            ..Default::default()
        };
        assert_eq!(mixed.effective_languages(), vec!["eng".to_string(), "deu".to_string()]);
    }

    #[cfg(all(feature = "ocr", feature = "pdf"))]
    #[test]
    fn test_effective_pipeline_explicit_pipeline_returned_unchanged() {
        let explicit_pipeline = OcrPipelineConfig {
            stages: vec![OcrPipelineStage {
                backend: "paddleocr".to_string(),
                priority: 200,
                language: Some(vec!["fra".to_string()]),
                tesseract_config: None,
                paddle_ocr_config: None,
                vlm_config: None,
                backend_options: None,
            }],
            quality_thresholds: OcrQualityThresholds::default(),
        };
        let config = OcrConfig {
            pipeline: Some(explicit_pipeline.clone()),
            ..Default::default()
        };
        let result = config.effective_pipeline().unwrap();
        assert_eq!(result.stages.len(), 1);
        assert_eq!(result.stages[0].backend, "paddleocr");
        assert_eq!(result.stages[0].priority, 200);
        assert_eq!(result.stages[0].language, Some(vec!["fra".to_string()]));
        assert_eq!(
            config.pipeline_selection(),
            OcrPipelineSelection::HighestScore,
            "an explicit pipeline must keep score-based selection"
        );
    }

    #[cfg(all(feature = "ocr", feature = "pdf"))]
    #[test]
    fn test_effective_pipeline_explicit_paddleocr_no_autofallback() {
        let config = OcrConfig {
            backend: "paddleocr".to_string(),
            ..Default::default()
        };
        assert!(config.effective_pipeline().is_none());
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_effective_pipeline_unknown_backend_rejected_by_validation() {
        let config = OcrConfig {
            backend: "unsupported-ocr".to_string(),
            ..Default::default()
        };
        let result = config.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Invalid OCR backend"));
    }

    #[cfg(all(feature = "ocr", feature = "pdf"))]
    #[test]
    fn test_effective_pipeline_default_tesseract_backend() {
        let config = OcrConfig::default();
        let result = config.effective_pipeline();
        #[cfg(feature = "paddle-ocr")]
        {
            let pipeline = result.unwrap();
            assert_eq!(pipeline.stages.len(), 2);
            assert_eq!(pipeline.stages[0].backend, "tesseract");
            assert_eq!(pipeline.stages[0].priority, 100);
            assert_eq!(pipeline.stages[1].backend, "paddleocr");
            assert_eq!(pipeline.stages[1].priority, 50);
        }
        #[cfg(not(feature = "paddle-ocr"))]
        {
            assert!(result.is_none());
        }
    }

    #[cfg(all(feature = "ocr", feature = "pdf"))]
    #[test]
    fn test_effective_thresholds_custom_vs_default() {
        let custom = OcrQualityThresholds {
            min_total_non_whitespace: 128,
            min_meaningful_words: 10,
            ..Default::default()
        };
        let config_custom = OcrConfig {
            quality_thresholds: Some(custom.clone()),
            ..Default::default()
        };
        let eff = config_custom.effective_thresholds();
        assert_eq!(eff.min_total_non_whitespace, 128);
        assert_eq!(eff.min_meaningful_words, 10);

        let config_default = OcrConfig::default();
        let eff_default = config_default.effective_thresholds();
        assert_eq!(eff_default.min_total_non_whitespace, 64);
        assert_eq!(eff_default.min_meaningful_words, 3);
    }

    #[test]
    fn test_vlm_fallback_policy_default_is_disabled() {
        let config = OcrConfig::default();
        assert_eq!(config.vlm_fallback, VlmFallbackPolicy::Disabled);
    }

    #[test]
    fn auto_strategy_reports_the_default_scan_threshold() {
        assert_eq!(
            OcrStrategy::Auto.effective_min_confidence(),
            DEFAULT_SCANNED_MIN_CONFIDENCE
        );
    }

    #[test]
    fn scanned_pages_strategy_clamps_its_threshold_into_the_unit_interval() {
        assert_eq!(
            OcrStrategy::ScannedPages { min_confidence: -0.5 }.effective_min_confidence(),
            0.0
        );
        assert_eq!(
            OcrStrategy::ScannedPages { min_confidence: 1.5 }.effective_min_confidence(),
            1.0
        );
        assert_eq!(
            OcrStrategy::ScannedPages { min_confidence: 0.42 }.effective_min_confidence(),
            0.42
        );
    }

    #[test]
    fn test_vlm_fallback_disabled_validate_ok_without_vlm_config() {
        let config = OcrConfig {
            vlm_fallback: VlmFallbackPolicy::Disabled,
            vlm_config: None,
            ..Default::default()
        };
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_vlm_fallback_on_low_quality_missing_vlm_config_validate_err() {
        let config = OcrConfig {
            vlm_fallback: VlmFallbackPolicy::OnLowQuality { quality_threshold: 0.6 },
            vlm_config: None,
            ..Default::default()
        };
        let result = config.validate();
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("vlm_config"),
            "error must mention vlm_config; got: {err_msg}"
        );
    }

    #[test]
    fn test_vlm_fallback_always_missing_vlm_config_validate_err() {
        let config = OcrConfig {
            vlm_fallback: VlmFallbackPolicy::Always,
            vlm_config: None,
            ..Default::default()
        };
        let result = config.validate();
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("vlm_config"),
            "error must mention vlm_config; got: {err_msg}"
        );
    }

    /// `OnLowQuality` synthesises a two-stage pipeline equivalent to what a caller
    /// would write by hand with an explicit `OcrPipelineConfig`.
    #[cfg(all(feature = "ocr", feature = "pdf"))]
    #[test]
    fn test_vlm_fallback_on_low_quality_synthesises_two_stage_pipeline() {
        use super::super::llm::LlmConfig;

        let vlm_cfg = LlmConfig {
            model: "openai/gpt-4o-mini".to_string(),
            base_url: Some("http://localhost:9999".to_string()),
            ..Default::default()
        };

        let config = OcrConfig {
            backend: "tesseract".to_string(),
            vlm_fallback: VlmFallbackPolicy::OnLowQuality { quality_threshold: 0.6 },
            vlm_config: Some(vlm_cfg.clone()),
            ..Default::default()
        };

        let explicit = OcrConfig {
            pipeline: Some(OcrPipelineConfig {
                stages: vec![
                    OcrPipelineStage {
                        backend: "tesseract".to_string(),
                        priority: 100,
                        language: None,
                        tesseract_config: None,
                        paddle_ocr_config: None,
                        vlm_config: None,
                        backend_options: None,
                    },
                    OcrPipelineStage {
                        backend: "vlm".to_string(),
                        priority: 50,
                        language: None,
                        tesseract_config: None,
                        paddle_ocr_config: None,
                        vlm_config: Some(vlm_cfg),
                        backend_options: None,
                    },
                ],
                quality_thresholds: OcrQualityThresholds {
                    pipeline_min_quality: 0.6,
                    ..Default::default()
                },
            }),
            ..Default::default()
        };

        let synthesised = config.effective_pipeline().expect("must synthesise a pipeline");
        let hand_written = explicit
            .effective_pipeline()
            .expect("explicit pipeline must be returned");

        assert_eq!(
            synthesised.stages.len(),
            hand_written.stages.len(),
            "stage count mismatch"
        );

        assert_eq!(synthesised.stages[0].backend, hand_written.stages[0].backend);
        assert_eq!(synthesised.stages[0].priority, hand_written.stages[0].priority);

        assert_eq!(synthesised.stages[1].backend, hand_written.stages[1].backend);
        assert_eq!(synthesised.stages[1].priority, hand_written.stages[1].priority);
        let s_vlm = synthesised.stages[1]
            .vlm_config
            .as_ref()
            .expect("synthesised stage 1 must have vlm_config");
        let h_vlm = hand_written.stages[1]
            .vlm_config
            .as_ref()
            .expect("hand-written stage 1 must have vlm_config");
        assert_eq!(s_vlm.model, h_vlm.model);

        assert!(
            (synthesised.quality_thresholds.pipeline_min_quality - 0.6).abs() < f64::EPSILON,
            "threshold must be 0.6, got {}",
            synthesised.quality_thresholds.pipeline_min_quality
        );
        assert!((hand_written.quality_thresholds.pipeline_min_quality - 0.6).abs() < f64::EPSILON,);
        assert_eq!(
            config.pipeline_selection(),
            OcrPipelineSelection::PreferLastNonEmpty,
            "OnLowQuality synthesis must prefer the last non-empty stage as its best-effort fallback"
        );
    }

    /// `Always` synthesises a single-stage VLM-only pipeline.
    #[cfg(all(feature = "ocr", feature = "pdf"))]
    #[test]
    fn test_vlm_fallback_always_synthesises_single_stage_vlm_pipeline() {
        use super::super::llm::LlmConfig;

        let vlm_cfg = LlmConfig {
            model: "anthropic/claude-sonnet-4-20250514".to_string(),
            ..Default::default()
        };
        let config = OcrConfig {
            vlm_fallback: VlmFallbackPolicy::Always,
            vlm_config: Some(vlm_cfg),
            ..Default::default()
        };

        let pipeline = config.effective_pipeline().expect("Always must produce a pipeline");
        assert_eq!(pipeline.stages.len(), 1, "Always must produce exactly one stage");
        assert_eq!(pipeline.stages[0].backend, "vlm");
        assert_eq!(pipeline.stages[0].priority, 100);
        assert!(pipeline.stages[0].vlm_config.is_some());
        assert_eq!(
            config.pipeline_selection(),
            OcrPipelineSelection::PreferLastNonEmpty,
            "Always synthesis must prefer the last non-empty stage as its best-effort fallback"
        );
    }

    /// `Disabled` with no explicit pipeline produces no synthesised pipeline
    /// (consistent with pre-policy single-backend mode).
    #[cfg(all(feature = "ocr", feature = "pdf"))]
    #[test]
    fn test_vlm_fallback_disabled_no_synthesis() {
        let config = OcrConfig {
            backend: "paddleocr".to_string(),
            vlm_fallback: VlmFallbackPolicy::Disabled,
            vlm_config: None,
            ..Default::default()
        };
        assert!(config.effective_pipeline().is_none());
    }

    /// Explicit pipeline always wins over vlm_fallback (regression guard).
    #[cfg(all(feature = "ocr", feature = "pdf"))]
    #[test]
    fn test_explicit_pipeline_wins_over_vlm_fallback() {
        use super::super::llm::LlmConfig;

        let explicit = OcrPipelineConfig {
            stages: vec![OcrPipelineStage {
                backend: "paddleocr".to_string(),
                priority: 99,
                language: None,
                tesseract_config: None,
                paddle_ocr_config: None,
                vlm_config: None,
                backend_options: None,
            }],
            quality_thresholds: OcrQualityThresholds::default(),
        };
        let config = OcrConfig {
            pipeline: Some(explicit),
            vlm_fallback: VlmFallbackPolicy::Always,
            vlm_config: Some(LlmConfig {
                model: "openai/gpt-4o".to_string(),
                ..Default::default()
            }),
            ..Default::default()
        };

        let pipeline = config.effective_pipeline().expect("explicit pipeline must be returned");
        assert_eq!(pipeline.stages.len(), 1, "explicit pipeline must win");
        assert_eq!(pipeline.stages[0].backend, "paddleocr", "explicit pipeline must win");
    }

    #[test]
    fn test_vlm_fallback_policy_serde_roundtrip_disabled() {
        let policy = VlmFallbackPolicy::Disabled;
        let json = serde_json::to_string(&policy).unwrap();
        let deserialized: VlmFallbackPolicy = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, VlmFallbackPolicy::Disabled);
    }

    #[test]
    fn test_vlm_fallback_policy_serde_roundtrip_on_low_quality() {
        let policy = VlmFallbackPolicy::OnLowQuality {
            quality_threshold: 0.42,
        };
        let json = serde_json::to_string(&policy).unwrap();
        let deserialized: VlmFallbackPolicy = serde_json::from_str(&json).unwrap();
        match deserialized {
            VlmFallbackPolicy::OnLowQuality { quality_threshold } => {
                assert!((quality_threshold - 0.42).abs() < f64::EPSILON);
            }
            other => panic!("expected OnLowQuality, got {other:?}"),
        }
    }

    #[test]
    fn test_vlm_fallback_policy_serde_roundtrip_always() {
        let policy = VlmFallbackPolicy::Always;
        let json = serde_json::to_string(&policy).unwrap();
        let deserialized: VlmFallbackPolicy = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, VlmFallbackPolicy::Always);
    }

    #[test]
    fn test_ocr_config_vlm_fallback_omitted_when_disabled_default() {
        let config = OcrConfig::default();
        let json = serde_json::to_string(&config).unwrap();
        let deserialized: OcrConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.vlm_fallback, VlmFallbackPolicy::Disabled);
    }

    #[test]
    fn test_pipeline_config_serde_roundtrip() {
        let pipeline = OcrPipelineConfig {
            stages: vec![
                OcrPipelineStage {
                    backend: "tesseract".to_string(),
                    priority: 100,
                    language: Some(vec!["eng".to_string()]),
                    tesseract_config: None,
                    paddle_ocr_config: None,
                    vlm_config: None,
                    backend_options: None,
                },
                OcrPipelineStage {
                    backend: "paddleocr".to_string(),
                    priority: 50,
                    language: None,
                    tesseract_config: None,
                    paddle_ocr_config: Some(serde_json::json!({"use_gpu": false})),
                    vlm_config: None,
                    backend_options: None,
                },
            ],
            quality_thresholds: OcrQualityThresholds::default(),
        };
        let json = serde_json::to_string(&pipeline).unwrap();
        let deserialized: OcrPipelineConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.stages.len(), 2);
        assert_eq!(deserialized.stages[0].backend, "tesseract");
        assert_eq!(deserialized.stages[0].priority, 100);
        assert_eq!(deserialized.stages[1].backend, "paddleocr");
        assert_eq!(deserialized.stages[1].priority, 50);
        assert!(deserialized.stages[1].paddle_ocr_config.is_some());
    }

    /// `pipeline_selection` derives the best-effort policy from the `OcrConfig` rather
    /// than a serialized pipeline field (#1341): score-based by default and for explicit
    /// pipelines, last-non-empty only for a genuinely-synthesised vlm_fallback pipeline.
    #[cfg(all(feature = "ocr", feature = "pdf"))]
    #[test]
    fn test_pipeline_selection_derivation() {
        use super::super::llm::LlmConfig;

        // Default (classical / no fallback): score-based.
        assert_eq!(
            OcrConfig::default().pipeline_selection(),
            OcrPipelineSelection::HighestScore
        );

        // vlm_fallback requested but no vlm_config: falls through to classical, score-based.
        let missing_vlm = OcrConfig {
            vlm_fallback: VlmFallbackPolicy::Always,
            vlm_config: None,
            ..Default::default()
        };
        assert_eq!(missing_vlm.pipeline_selection(), OcrPipelineSelection::HighestScore);

        // vlm_fallback with vlm_config present: synthesised fallback prefers last non-empty.
        let with_vlm = OcrConfig {
            vlm_fallback: VlmFallbackPolicy::Always,
            vlm_config: Some(LlmConfig {
                model: "openai/gpt-4o-mini".to_string(),
                ..Default::default()
            }),
            ..Default::default()
        };
        assert_eq!(with_vlm.pipeline_selection(), OcrPipelineSelection::PreferLastNonEmpty);
    }

    #[test]
    fn test_pipeline_stage_deserialization_missing_optional_fields() {
        let json = r#"{"backend": "tesseract"}"#;
        let stage: OcrPipelineStage = serde_json::from_str(json).unwrap();
        assert_eq!(stage.backend, "tesseract");
        assert_eq!(stage.priority, 100);
        assert!(stage.language.is_none());
        assert!(stage.tesseract_config.is_none());
        assert!(stage.paddle_ocr_config.is_none());
    }

    #[test]
    fn test_pipeline_stage_language_deserialization_single() {
        let json = r#"{"backend": "tesseract", "language": "eng"}"#;
        let stage: OcrPipelineStage = serde_json::from_str(json).unwrap();
        assert_eq!(stage.language, Some(vec!["eng".to_string()]));
    }

    #[test]
    fn test_pipeline_stage_language_deserialization_array() {
        let json = r#"{"backend": "tesseract", "language": ["eng", "deu"]}"#;
        let stage: OcrPipelineStage = serde_json::from_str(json).unwrap();
        assert_eq!(stage.language, Some(vec!["eng".to_string(), "deu".to_string()]));
    }

    #[test]
    fn test_pipeline_stage_default_priority_is_100() {
        let json = r#"{"backend": "paddleocr"}"#;
        let stage: OcrPipelineStage = serde_json::from_str(json).unwrap();
        assert_eq!(stage.priority, 100);
    }

    #[test]
    fn test_ocr_config_deserialization_missing_optional_fields() {
        let json = r#"{}"#;
        let config: OcrConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.backend, "tesseract");
        assert_eq!(config.language, vec!["eng".to_string()]);
        assert!(config.pipeline.is_none());
        assert!(config.quality_thresholds.is_none());
        assert!(config.element_config.is_none());
    }

    #[test]
    fn test_quality_thresholds_deserialization_partial() {
        let json = r#"{"min_total_non_whitespace": 256}"#;
        let thresholds: OcrQualityThresholds = serde_json::from_str(json).unwrap();
        assert_eq!(thresholds.min_total_non_whitespace, 256);
        assert_eq!(thresholds.min_meaningful_words, 3);
        assert_eq!(thresholds.min_garbage_chars, 5);
        assert!((thresholds.pipeline_min_quality - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn test_provenance_ocr_routing_defaults_enabled_with_half_ratio() {
        let thresholds = OcrQualityThresholds::default();
        assert!(thresholds.enable_provenance_ocr_routing);
        assert!((thresholds.min_provenance_fallback_ratio - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn test_provenance_ocr_routing_deserializes_overrides() {
        let json = r#"{"enable_provenance_ocr_routing": false, "min_provenance_fallback_ratio": 0.75}"#;
        let thresholds: OcrQualityThresholds = serde_json::from_str(json).unwrap();
        assert!(!thresholds.enable_provenance_ocr_routing);
        assert!((thresholds.min_provenance_fallback_ratio - 0.75).abs() < f64::EPSILON);
    }

    #[test]
    fn test_validate_catches_invalid_pipeline_stage_backend() {
        let config = OcrConfig {
            pipeline: Some(OcrPipelineConfig {
                stages: vec![
                    OcrPipelineStage {
                        backend: "tesseract".to_string(),
                        priority: 100,
                        language: None,
                        tesseract_config: None,
                        paddle_ocr_config: None,
                        vlm_config: None,
                        backend_options: None,
                    },
                    OcrPipelineStage {
                        backend: "invalid_backend".to_string(),
                        priority: 50,
                        language: None,
                        tesseract_config: None,
                        paddle_ocr_config: None,
                        vlm_config: None,
                        backend_options: None,
                    },
                ],
                quality_thresholds: OcrQualityThresholds::default(),
            }),
            ..Default::default()
        };
        let result = config.validate();
        assert!(result.is_err(), "Should catch invalid backend in pipeline stages");
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("Invalid OCR backend") || err_msg.contains("invalid_backend"));
    }

    #[test]
    fn test_validate_passes_with_valid_pipeline_stages() {
        let config = OcrConfig {
            pipeline: Some(OcrPipelineConfig {
                stages: vec![
                    OcrPipelineStage {
                        backend: "tesseract".to_string(),
                        priority: 100,
                        language: None,
                        tesseract_config: None,
                        paddle_ocr_config: None,
                        vlm_config: None,
                        backend_options: None,
                    },
                    OcrPipelineStage {
                        backend: "paddleocr".to_string(),
                        priority: 50,
                        language: None,
                        tesseract_config: None,
                        paddle_ocr_config: None,
                        vlm_config: None,
                        backend_options: None,
                    },
                ],
                quality_thresholds: OcrQualityThresholds::default(),
            }),
            ..Default::default()
        };
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_ocr_config_backend_options_default_is_none() {
        let config = OcrConfig::default();
        assert!(config.backend_options.is_none());
    }

    #[test]
    fn test_ocr_config_backend_options_serde_roundtrip() {
        let config = OcrConfig {
            backend_options: Some(serde_json::json!({"mode": "fast", "threshold": 0.8, "enable_layout": true})),
            ..Default::default()
        };
        let json = serde_json::to_string(&config).unwrap();
        let deserialized: OcrConfig = serde_json::from_str(&json).unwrap();
        let opts = deserialized.backend_options.unwrap();
        assert_eq!(opts["mode"], "fast");
        assert!((opts["threshold"].as_f64().unwrap() - 0.8).abs() < f64::EPSILON);
        assert_eq!(opts["enable_layout"], true);
    }

    #[test]
    fn test_ocr_config_backend_options_omitted_when_none() {
        let config = OcrConfig::default();
        let json = serde_json::to_string(&config).unwrap();
        assert!(
            !json.contains("backend_options"),
            "backend_options must be omitted when None"
        );
    }

    #[test]
    fn test_pipeline_stage_backend_options_serde_roundtrip() {
        let stage = OcrPipelineStage {
            backend: "custom".to_string(),
            priority: 80,
            language: None,
            tesseract_config: None,
            paddle_ocr_config: None,
            vlm_config: None,
            backend_options: Some(serde_json::json!({"batch_size": 4, "device": "cpu"})),
        };
        let json = serde_json::to_string(&stage).unwrap();
        let deserialized: OcrPipelineStage = serde_json::from_str(&json).unwrap();
        let opts = deserialized.backend_options.unwrap();
        assert_eq!(opts["batch_size"], 4);
        assert_eq!(opts["device"], "cpu");
    }

    #[test]
    fn test_pipeline_stage_backend_options_omitted_when_none() {
        let stage = OcrPipelineStage {
            backend: "tesseract".to_string(),
            priority: 100,
            language: None,
            tesseract_config: None,
            paddle_ocr_config: None,
            vlm_config: None,
            backend_options: None,
        };
        let json = serde_json::to_string(&stage).unwrap();
        assert!(
            !json.contains("backend_options"),
            "backend_options must be omitted when None"
        );
    }

    #[cfg(all(feature = "ocr", feature = "paddle-ocr", feature = "pdf"))]
    #[test]
    fn test_effective_pipeline_propagates_backend_options_to_primary_stage() {
        let config = OcrConfig {
            backend_options: Some(serde_json::json!({"mode": "fast"})),
            ..Default::default()
        };
        let pipeline = config
            .effective_pipeline()
            .expect("paddle-ocr feature must produce a pipeline");
        assert_eq!(pipeline.stages.len(), 2);

        let primary = &pipeline.stages[0];
        assert_eq!(primary.backend, "tesseract");
        let opts = primary
            .backend_options
            .as_ref()
            .expect("primary stage must carry backend_options");
        assert_eq!(opts["mode"], "fast");

        let fallback = &pipeline.stages[1];
        assert_eq!(fallback.backend, "paddleocr");
        assert!(
            fallback.backend_options.is_none(),
            "paddleocr stage must not inherit backend_options"
        );
    }

    #[cfg(all(feature = "ocr", feature = "pdf"))]
    #[test]
    fn test_explicit_pipeline_ignores_top_level_backend_options() {
        let config = OcrConfig {
            backend_options: Some(serde_json::json!({"mode": "fast"})),
            pipeline: Some(OcrPipelineConfig {
                stages: vec![OcrPipelineStage {
                    backend: "tesseract".to_string(),
                    priority: 100,
                    language: None,
                    tesseract_config: None,
                    paddle_ocr_config: None,
                    vlm_config: None,
                    backend_options: None,
                }],
                quality_thresholds: OcrQualityThresholds::default(),
            }),
            ..Default::default()
        };
        let pipeline = config
            .effective_pipeline()
            .expect("explicit pipeline must be returned as-is");
        assert_eq!(pipeline.stages.len(), 1);
        assert!(
            pipeline.stages[0].backend_options.is_none(),
            "top-level backend_options must not be injected into an explicit pipeline stage"
        );
    }

    #[cfg(all(feature = "ocr", feature = "pdf"))]
    #[test]
    fn test_stage_level_backend_options_preserved_in_explicit_pipeline() {
        let stage_opts = serde_json::json!({"device": "gpu", "batch": 8});
        let config = OcrConfig {
            backend_options: Some(serde_json::json!({"mode": "fast"})),
            pipeline: Some(OcrPipelineConfig {
                stages: vec![OcrPipelineStage {
                    backend: "custom".to_string(),
                    priority: 100,
                    language: None,
                    tesseract_config: None,
                    paddle_ocr_config: None,
                    vlm_config: None,
                    backend_options: Some(stage_opts.clone()),
                }],
                quality_thresholds: OcrQualityThresholds::default(),
            }),
            ..Default::default()
        };
        let pipeline = config
            .effective_pipeline()
            .expect("explicit pipeline must be returned as-is");
        let returned_opts = pipeline.stages[0]
            .backend_options
            .as_ref()
            .expect("stage-level backend_options must be preserved");
        assert_eq!(returned_opts["device"], "gpu");
        assert_eq!(returned_opts["batch"], 8);
    }
}
