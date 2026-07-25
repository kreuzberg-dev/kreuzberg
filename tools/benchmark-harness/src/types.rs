//! Core types for benchmark results and metrics

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::str::FromStr;
use std::time::Duration;

/// Output format for document extraction
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OutputFormat {
    /// Markdown output format with structure preservation
    #[default]
    Markdown,
    /// Plain text output format
    Plaintext,
}

impl std::fmt::Display for OutputFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OutputFormat::Markdown => write!(f, "markdown"),
            OutputFormat::Plaintext => write!(f, "plaintext"),
        }
    }
}

impl FromStr for OutputFormat {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "markdown" | "md" => Ok(OutputFormat::Markdown),
            "plaintext" | "text" | "txt" => Ok(OutputFormat::Plaintext),
            _ => Err(format!(
                "unknown output format: {}. Valid: markdown, md, plaintext, text, txt",
                s
            )),
        }
    }
}

/// Default output format for backward compatibility with old results
fn default_output_format() -> OutputFormat {
    OutputFormat::Markdown
}

/// Per-stage cold-start timing breakdown parsed from an xberg CLI JSON envelope's
/// `stage_timings` field (see `crates/xberg-cli/src/output.rs::StageTimings`).
///
/// Field names and semantics mirror the CLI struct exactly; this is a plain-data mirror rather
/// than a shared type because the benchmark harness does not depend on the `xberg-cli` crate.
/// Only populated when the harness invokes the CLI with `XBERG_EMIT_STAGE_TIMING` set.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct StageTimings {
    /// Time from CLI process start to the point extraction begins (arg parsing, logging setup,
    /// config load/merge), in milliseconds.
    pub process_init_ms: f64,
    /// Wall-clock time for the core library's extraction call to return, in milliseconds.
    pub first_parse_ms: f64,
    /// Coarse approximation of ONNX Runtime session-creation-plus-first-inference cost, present
    /// only when a layout/OCR configuration that uses ORT was active. See the CLI-side
    /// `StageTimings` doc comment for why this is not an independently measured sub-stage.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub ort_session_and_inference_ms: Option<f64>,
}

/// Xberg extraction pipeline variant
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum XbergPipeline {
    /// Baseline: text extraction without layout or OCR
    Baseline,
    /// Layout: layout detection and structure preservation
    Layout,
    /// PaddleOCR: OCR with PaddleOCR backend
    #[serde(rename = "paddle-ocr")]
    PaddleOcr,
    /// Candle TrOCR: OCR with candle-based TrOCR backend
    #[serde(rename = "candle-trocr")]
    CandleTrocr,
    /// Candle PaddleOCR-VL: OCR with candle-based PaddleOCR-VL backend (end-to-end markdown)
    #[serde(rename = "candle-paddleocr-vl")]
    CandlePaddleocrVl,
    /// Candle GLM-OCR: OCR with candle-based GLM-OCR vision-language backend
    #[serde(rename = "candle-glm-ocr")]
    CandleGlmOcr,
    /// Candle DeepSeek-OCR: OCR with candle-based DeepSeek-OCR vision-language backend
    #[serde(rename = "candle-deepseek-ocr")]
    CandleDeepseekOcr,
    /// Candle PaddleOCR-VL 1.5: OCR with candle-based PaddleOCR-VL 1.5 vision-language backend
    #[serde(rename = "candle-paddleocr-vl-15")]
    CandlePaddleocrVl15,
}

impl XbergPipeline {
    /// Get the string representation of the pipeline
    pub fn as_str(self) -> &'static str {
        match self {
            XbergPipeline::Baseline => "baseline",
            XbergPipeline::Layout => "layout",
            XbergPipeline::PaddleOcr => "paddle-ocr",
            XbergPipeline::CandleTrocr => "candle-trocr",
            XbergPipeline::CandlePaddleocrVl => "candle-paddleocr-vl",
            XbergPipeline::CandleGlmOcr => "candle-glm-ocr",
            XbergPipeline::CandleDeepseekOcr => "candle-deepseek-ocr",
            XbergPipeline::CandlePaddleocrVl15 => "candle-paddleocr-vl-15",
        }
    }
}

impl std::fmt::Display for XbergPipeline {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl FromStr for XbergPipeline {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "baseline" => Ok(XbergPipeline::Baseline),
            "layout" => Ok(XbergPipeline::Layout),
            "paddle-ocr" | "paddle_ocr" | "paddleocr" => Ok(XbergPipeline::PaddleOcr),
            "candle-trocr" | "candle_trocr" | "trocr" => Ok(XbergPipeline::CandleTrocr),
            "candle-paddleocr-vl" | "candle_paddleocr_vl" | "paddleocr-vl" => Ok(XbergPipeline::CandlePaddleocrVl),
            "candle-glm-ocr" | "candle_glm_ocr" | "glm-ocr" => Ok(XbergPipeline::CandleGlmOcr),
            "candle-deepseek-ocr" | "candle_deepseek_ocr" | "deepseek-ocr" => Ok(XbergPipeline::CandleDeepseekOcr),
            "candle-paddleocr-vl-15" | "candle_paddleocr_vl_15" | "paddleocr-vl-15" => {
                Ok(XbergPipeline::CandlePaddleocrVl15)
            }
            _ => Err(format!(
                "unknown Xberg pipeline: {}. Valid: baseline, layout, paddle-ocr, candle-trocr, candle-paddleocr-vl, candle-glm-ocr, candle-deepseek-ocr, candle-paddleocr-vl-15",
                s
            )),
        }
    }
}

/// OCR usage status for a benchmark extraction
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum OcrStatus {
    /// OCR was used for this extraction
    Used,
    /// OCR was not used for this extraction
    NotUsed,
    /// Unknown whether OCR was used
    #[default]
    Unknown,
}

/// Categorizes the source of a benchmark error.
///
/// This distinction is critical:
/// - **FrameworkError**: the framework itself reported an extraction error (returned `{"error": "..."}`)
/// - **HarnessError**: harness infrastructure problem (process crash, invalid JSON output, etc.)
/// - **ConfigSetupError**: environment/dependency misconfiguration (missing models, torch module not available, etc.)
/// - **Timeout**: extraction exceeded configured timeout
/// - **EmptyContent**: framework ran but produced no content
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    /// The framework itself reported an extraction error (returned `{"error": "..."}`)
    /// This is NOT our fault - the framework couldn't handle this file.
    FrameworkError,
    /// A harness-level error: process crash, invalid JSON output, subprocess failure, etc.
    /// This IS our fault or an infrastructure issue.
    HarnessError,
    /// Configuration or setup error: missing dependencies, environment misconfiguration
    /// (e.g., torch.PP-OCRv6 not available, partition_X not available, missing tessdata, etc.)
    ConfigSetupError,
    /// Extraction timed out (exceeded the configured timeout duration).
    Timeout,
    /// Framework returned empty or missing content (ran but produced nothing).
    EmptyContent,
    /// No error occurred
    #[default]
    None,
}

/// Complete benchmark result for a single file extraction
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchmarkResult {
    /// Framework that performed the extraction
    pub framework: String,

    /// Output format used for extraction (markdown or plaintext)
    #[serde(default = "default_output_format")]
    pub output_format: OutputFormat,

    /// Path to the test document
    pub file_path: PathBuf,

    /// File size in bytes
    pub file_size: u64,

    /// Whether extraction succeeded
    pub success: bool,

    /// Error message if extraction failed
    pub error_message: Option<String>,

    /// Categorizes the error source (framework vs harness)
    #[serde(default)]
    pub error_kind: ErrorKind,

    /// Total wall-clock duration (process spawn + extraction)
    /// For single iteration: the actual duration
    /// For multiple iterations: mean duration across all iterations
    pub duration: Duration,

    /// Pure extraction time (reported by subprocess via _extraction_time_ms)
    /// Only available for external frameworks with internal timing
    pub extraction_duration: Option<Duration>,

    /// Subprocess overhead outside framework-reported extraction work.
    ///
    /// For native batches this is process-wall time minus the reported batch
    /// total. It is never inferred from per-item extraction timings.
    pub subprocess_overhead: Option<Duration>,

    /// Performance metrics (averaged across iterations if multiple)
    pub metrics: PerformanceMetrics,

    /// Quality metrics (if ground truth available)
    pub quality: Option<QualityMetrics>,

    /// Individual iteration results (empty for single iteration)
    pub iterations: Vec<IterationResult>,

    /// Statistical analysis of durations across iterations
    /// Only present when multiple iterations were run
    pub statistics: Option<DurationStatistics>,

    /// Cold start duration: Time from framework not loaded to ready and warm state
    /// This is measured during the first warmup extraction and represents the
    /// initial framework load time (imports, initializations, etc.)
    pub cold_start_duration: Option<Duration>,

    /// File extension without dot (e.g., "pdf", "docx")
    /// Extracted from file_path for per-extension analysis
    pub file_extension: String,

    /// Framework capability metadata at time of extraction
    /// Contains OCR support, batch support, async support flags
    pub framework_capabilities: FrameworkCapabilities,

    /// PDF-specific metadata (only present for PDF files)
    /// Includes text layer detection results and OCR strategy
    pub pdf_metadata: Option<PdfMetadata>,

    /// OCR usage status for this extraction
    #[serde(default)]
    pub ocr_status: OcrStatus,

    /// Extracted text content (for quality assessment)
    /// Not serialized to output JSON to save space
    #[serde(skip)]
    pub extracted_text: Option<String>,

    /// System load captured at measurement time.
    ///
    /// Recorded so local timing comparisons can be qualified: throughput and
    /// cold-start numbers taken under heavy background load are not comparable
    /// to those taken on an idle machine. `None` for results that predate this
    /// field or were constructed outside a measurement path.
    #[serde(default)]
    pub system_load: Option<crate::system_load::SystemLoad>,
}

impl BenchmarkResult {
    /// Create a framework key combining framework name, output format, and execution mode
    /// Format: "{framework}:{output_format}:{execution_mode}"
    /// Example: "xberg-rust:markdown:batch"
    pub fn framework_key(&self, execution_mode: &str) -> String {
        format!("{}:{}:{}", self.framework, self.output_format, execution_mode)
    }

    /// Whether this row owns the aggregate process metrics for its native batch.
    ///
    /// Native batch adapters mark exactly one process-metric sample. Single-file
    /// results omit the marker and always own their metrics.
    pub(crate) fn is_performance_sample(&self) -> bool {
        self.framework_capabilities.batch_performance_sample.unwrap_or_else(|| {
            self.framework_capabilities.batch_capability.is_none() || self.metrics.throughput_bytes_per_sec > 0.0
        })
    }
}

pub(crate) fn successful_performance_samples<'a>(
    results: impl IntoIterator<Item = &'a BenchmarkResult>,
) -> Vec<&'a BenchmarkResult> {
    let mut batch_samples = std::collections::HashSet::new();
    let mut samples = Vec::new();
    for result in results {
        if !result.success {
            continue;
        }
        if let Some(sample_id) = result.framework_capabilities.batch_sample_id.as_deref() {
            if batch_samples.insert(sample_id) {
                samples.push(result);
            }
        } else if result.is_performance_sample() {
            samples.push(result);
        }
    }
    samples
}

/// Performance metrics collected during extraction
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerformanceMetrics {
    /// RSS captured immediately after the monitor attached to the target.
    #[serde(default)]
    pub baseline_memory_bytes: u64,

    /// Absolute peak RSS in bytes.
    pub peak_memory_bytes: u64,

    /// Peak RSS above the captured baseline.
    #[serde(default)]
    pub peak_memory_delta_bytes: u64,

    /// Average CPU usage percentage (0-100)
    pub avg_cpu_percent: f64,

    /// Throughput in bytes per second
    pub throughput_bytes_per_sec: f64,

    /// 50th percentile memory usage in bytes
    pub p50_memory_bytes: u64,

    /// 95th percentile memory usage in bytes
    pub p95_memory_bytes: u64,

    /// 99th percentile memory usage in bytes
    pub p99_memory_bytes: u64,
}

/// Quality metrics comparing extraction output to ground truth
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QualityMetrics {
    /// Text token F1 score (0.0-1.0)
    pub f1_score_text: f64,

    /// Numeric token F1 score (0.0-1.0)
    pub f1_score_numeric: f64,

    /// Layout/structure F1 score (0.0-1.0), optional for plaintext mode
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub f1_score_layout: Option<f64>,

    /// Overall text quality score (0.0-1.0)
    pub quality_score: f64,

    /// Tokens in ground truth but missing/under-represented in extraction (recall misses).
    /// Each entry is (token, deficit_count). Sorted by count descending.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub missing_tokens: Vec<(String, usize)>,

    /// Tokens in extraction but not in ground truth or over-represented (precision misses).
    /// Each entry is (token, surplus_count). Sorted by count descending.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extra_tokens: Vec<(String, usize)>,

    /// Whether the extraction is considered correct (quality_score >= 0.95).
    #[serde(default)]
    pub correct: bool,
}

/// Framework capability metadata
///
/// Records the capabilities of the framework at the time of extraction,
/// enabling proper analysis and comparison of results based on framework features.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FrameworkCapabilities {
    /// Extensions this framework supports (e.g., ["pdf", "docx"])
    #[serde(default)]
    pub supported_extensions: Vec<String>,

    /// Whether framework supports OCR
    #[serde(default)]
    pub ocr_support: bool,

    /// Whether framework supports batch processing
    #[serde(default)]
    pub batch_support: bool,

    /// Verified batch entry point and timing semantics used for this result.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub batch_capability: Option<BatchCapability>,

    /// Whether this row owns its native batch's process metrics.
    ///
    /// `None` denotes a single-file measurement. Native batches set exactly
    /// one row to `Some(true)` and all sibling rows to `Some(false)`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub batch_performance_sample: Option<bool>,

    /// Opaque identity shared by every document row from one batch process.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub batch_sample_id: Option<String>,

    /// Whether framework supports async extraction
    #[serde(default)]
    pub async_support: bool,

    /// Output formats this framework supports
    #[serde(default)]
    pub supported_output_formats: Vec<OutputFormat>,

    /// Framework version
    #[serde(default)]
    pub version: String,

    /// Disk installation size (if known)
    #[serde(default)]
    pub installation_size: Option<DiskSizeInfo>,
}

/// Concrete framework API used for a batch benchmark.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BatchEntryPoint {
    XbergCliExtractBatch,
    DoclingConvertAll,
    LiteparseBatchParse,
    MineruDoParse,
}

/// Scope represented by the measured batch makespan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BatchTimingScope {
    WarmSteadyState,
    ColdEndToEndSubprocess,
}

/// Verified batch capability advertised by an adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BatchCapability {
    pub entry_point: BatchEntryPoint,
    pub timing_scope: BatchTimingScope,
    pub per_item_timing: bool,
}

fn is_zero_u64(v: &u64) -> bool {
    *v == 0
}

/// Disk installation size information for a framework
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiskSizeInfo {
    /// Total size in bytes (package + system deps)
    pub size_bytes: u64,

    /// Package-only size in bytes (before adding system deps)
    #[serde(default)]
    pub package_bytes: u64,

    /// System dependency size in bytes (libreoffice, tesseract, ffmpeg, etc.)
    #[serde(default)]
    pub system_deps_bytes: u64,

    /// ML model size in bytes (auto-downloaded on first use)
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub model_bytes: u64,

    /// Measurement method (e.g., "binary_size", "pip_package", "npm_package")
    pub method: String,

    /// Human-readable description
    pub description: String,

    /// Breakdown of system dependency sizes by package name
    /// Keys are package names (e.g., "poppler-utils"), values are installed sizes in bytes.
    /// Only populated when runtime measurement via dpkg-query succeeds.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub system_deps_detail: HashMap<String, u64>,
}

/// PDF-specific metadata
///
/// Contains PDF text layer detection results and OCR strategy used.
/// Only populated for PDF documents.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PdfMetadata {
    /// Whether PDF has a quality text layer
    /// Detected via pdftotext/pdffonts/pypdf
    pub has_text_layer: bool,

    /// Detection method used ("pdftotext", "pdffonts", "pypdf", "fallback")
    pub detection_method: String,

    /// Number of pages in the PDF
    pub page_count: Option<u32>,

    /// Whether OCR was enabled for this extraction
    pub ocr_enabled: bool,

    /// Text extraction quality hint (0.0-1.0)
    /// 0.0 = scanned image, 1.0 = native text
    pub text_quality_score: Option<f64>,
}

/// Result from a single benchmark iteration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IterationResult {
    /// Iteration number (0-indexed)
    pub iteration: usize,

    /// Total wall-clock duration for this iteration
    pub duration: Duration,

    /// Pure extraction time (if available from subprocess)
    pub extraction_duration: Option<Duration>,

    /// Performance metrics for this iteration
    pub metrics: PerformanceMetrics,
}

/// Statistical analysis of durations across multiple iterations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DurationStatistics {
    /// Mean duration
    pub mean: Duration,

    /// Median duration
    pub median: Duration,

    /// Standard deviation (in milliseconds as f64)
    pub std_dev_ms: f64,

    /// Minimum duration
    pub min: Duration,

    /// Maximum duration
    pub max: Duration,

    /// 95th percentile duration
    pub p95: Duration,

    /// 99th percentile duration
    pub p99: Duration,

    /// Number of iterations included in statistics
    pub sample_count: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stage_timings_round_trips_with_ort_field_present() {
        let timings = StageTimings {
            process_init_ms: 12.5,
            first_parse_ms: 1150.25,
            ort_session_and_inference_ms: Some(1150.25),
        };

        let json = serde_json::to_string(&timings).expect("serialize StageTimings");
        let parsed: StageTimings = serde_json::from_str(&json).expect("deserialize StageTimings");

        assert_eq!(parsed.process_init_ms, 12.5);
        assert_eq!(parsed.first_parse_ms, 1150.25);
        assert_eq!(parsed.ort_session_and_inference_ms, Some(1150.25));
    }

    #[test]
    fn stage_timings_omits_ort_field_when_absent() {
        let timings = StageTimings {
            process_init_ms: 8.0,
            first_parse_ms: 235.0,
            ort_session_and_inference_ms: None,
        };

        let json = serde_json::to_string(&timings).expect("serialize StageTimings");

        assert!(
            !json.contains("ort_session_and_inference_ms"),
            "expected ort_session_and_inference_ms to be skipped when None, got: {json}"
        );
    }

    #[test]
    fn stage_timings_parses_from_cli_json_shape() {
        // Mirrors the exact JSON shape emitted by `xberg-cli`'s `output::StageTimings` when
        // XBERG_EMIT_STAGE_TIMING is set and layout/OCR is active. ~keep
        let raw = r#"{
            "process_init_ms": 4.2,
            "first_parse_ms": 1171.0,
            "ort_session_and_inference_ms": 1171.0
        }"#;

        let parsed: StageTimings = serde_json::from_str(raw).expect("parse CLI stage_timings JSON");

        assert_eq!(parsed.process_init_ms, 4.2);
        assert_eq!(parsed.first_parse_ms, 1171.0);
        assert_eq!(parsed.ort_session_and_inference_ms, Some(1171.0));
    }
}
