//! Framework comparison: run multiple extraction pipelines on the corpus and
//! compare quality (SF1, TF1) against ground truth with optional guardrails.
//!
//! Replaces the logic previously in `crates/xberg/tests/framework_comparison.rs`.
//! Uses canonical scoring from [`crate::quality`].
//!
//! # Pipeline semantics
//!
//! Each [`Pipeline`] variant represents a distinct extraction configuration:
//!
//! - **Baseline / Layout**: native PDF text extraction; `Layout` adds layout
//!   detection for heading/table identification without OCR.
//! - **Tesseract / TesseractLayout**: Tesseract OCR with `force_ocr`; the
//!   `Layout` suffix adds layout detection on top.
//! - **Paddle***: explicitly pinned PaddleOCR model version and tier, with
//!   optional layout detection; the label-to-model mapping is benchmark identity. ~keep
//! - **Docling / PaddleOcrPython / RapidOcr**: vendored pipelines whose
//!   outputs are pre-computed and read from disk (no live extraction).
//! - **LayoutSlanet***: layout detection with explicit SLANeXT table model
//!   variants (wired, wireless, plus, auto).
//!
//! # Guardrail thresholds
//!
//! When `guardrails` is enabled, the comparison enforces per-document minimum
//! SF1 and TF1 thresholds. Thresholds are set to approximately 90% of observed
//! scores (a floor) to catch regressions without false-positive failures from
//! run-to-run variance. The guardrail table is updated manually after
//! significant quality improvements.

use crate::Result;
use crate::corpus::{self, CorpusDocument, CorpusFilter};
use crate::quality::{compute_f1, compute_token_diff, structural_sidecar, tokenize};
use futures::FutureExt;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use std::time::Instant;

/// Base timeout for extraction in seconds.
///
/// This is the minimum timeout applied to all documents, regardless of size.
const EXTRACTION_TIMEOUT_BASE_SECS: u64 = 60;

/// Minimum timeout for forced OCR pipelines, whose inference cost is substantially higher.
/// Five minutes leaves CI variance headroom while the outer safety timeout still bounds hangs. ~keep
const FORCED_OCR_TIMEOUT_BASE_SECS: u64 = 300;

/// Per-page timeout for PDF documents in milliseconds.
///
/// For layout-heavy PDFs (which require layout detection with ONNX inference),
/// allocate additional time proportional to page count. This prevents timeouts
/// on legitimately large documents like 214-page standards sheets.
///
/// Example: 214-page PDF = 60s base + (214 * 400ms) = 60s + 85.6s = 145.6s total
const EXTRACTION_TIMEOUT_PER_PAGE_MS: u64 = 400;

const PP_OCR_V5: &str = "pp-ocrv5";
const PP_OCR_V6: &str = "pp-ocrv6";
const PADDLE_DET_SIDE_1024: u32 = 1024;
const PADDLE_DET_SIDE_1536: u32 = 1536;
const PADDLE_DET_SIDE_2048: u32 = 2048;
const PADDLE_DET_DB_THRESH_020: f32 = 0.20;
const PADDLE_DET_DB_THRESH_030: f32 = 0.30;
const PADDLE_DET_DB_BOX_THRESH_035: f32 = 0.35;
const PADDLE_DET_DB_BOX_THRESH_050: f32 = 0.50;
const PADDLE_DROP_SCORE_030: f32 = 0.30;
const PADDLE_DROP_SCORE_040: f32 = 0.40;
const PADDLE_DROP_SCORE_050: f32 = 0.50;
const TESSERACT_PSM_VERTICAL_BLOCK: i32 = 5;
const TESSERACT_PSM_SINGLE_BLOCK: i32 = 6;
const TESSERACT_PSM_SPARSE_TEXT: i32 = 11;

/// Extraction pipeline identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Pipeline {
    /// Native PDF text extraction (no OCR, no layout)
    Baseline,
    /// Native PDF text extraction + layout detection
    Layout,
    /// Tesseract OCR (force_ocr)
    Tesseract,
    /// Tesseract OCR + layout detection
    TesseractLayout,
    /// Tesseract OCR with single-block page segmentation (PSM 6)
    #[serde(alias = "tesseract-psm6")]
    TesseractSingleBlock,
    /// Tesseract OCR with vertical single-block page segmentation (PSM 5)
    #[serde(alias = "tesseract-vertical", alias = "tesseract-psm5")]
    TesseractVerticalBlock,
    /// Tesseract OCR with sparse-text page segmentation (PSM 11)
    #[serde(alias = "tesseract-psm11")]
    TesseractSparseText,
    /// PP-OCRv6 medium tier (force_ocr)
    #[serde(rename = "paddle-v6-medium", alias = "paddle", alias = "paddle-mobile")]
    Paddle,
    /// PP-OCRv6 medium tier + layout detection
    #[serde(
        rename = "paddle-v6-medium+layout",
        alias = "paddle-layout",
        alias = "paddle-mobile-layout"
    )]
    PaddleLayout,
    /// PP-OCRv6 small tier (force_ocr)
    #[serde(rename = "paddle-v6-small")]
    PaddleV6Small,
    /// PP-OCRv6 small tier + layout detection
    #[serde(rename = "paddle-v6-small+layout", alias = "paddle-v6-small-layout")]
    PaddleV6SmallLayout,
    /// PP-OCRv6 small + layout with the fully pinned 1024-pixel quality control profile
    #[serde(rename = "paddle-v6-small+layout+det-side-1024")]
    PaddleV6SmallLayoutDetSide1024,
    /// PP-OCRv6 small + layout with a 1536-pixel detector input limit
    #[serde(rename = "paddle-v6-small+layout+det-side-1536")]
    PaddleV6SmallLayoutDetSide1536,
    /// PP-OCRv6 small + layout with a 2048-pixel detector input limit
    #[serde(rename = "paddle-v6-small+layout+det-side-2048")]
    PaddleV6SmallLayoutDetSide2048,
    /// PP-OCRv6 small + layout with a 0.20 detector pixel threshold
    #[serde(rename = "paddle-v6-small+layout+det-db-thresh-020")]
    PaddleV6SmallLayoutDetDbThresh020,
    /// PP-OCRv6 small + layout with a 0.35 detector box threshold
    #[serde(rename = "paddle-v6-small+layout+det-db-box-thresh-035")]
    PaddleV6SmallLayoutDetDbBoxThresh035,
    /// PP-OCRv6 small + layout with a 0.30 recognition drop score
    #[serde(rename = "paddle-v6-small+layout+drop-score-030")]
    PaddleV6SmallLayoutDropScore030,
    /// PP-OCRv6 small + layout with a 0.40 recognition drop score
    #[serde(rename = "paddle-v6-small+layout+drop-score-040")]
    PaddleV6SmallLayoutDropScore040,
    /// PP-OCRv6 tiny tier (force_ocr)
    #[serde(rename = "paddle-v6-tiny")]
    PaddleV6Tiny,
    /// PP-OCRv6 tiny tier + layout detection
    #[serde(rename = "paddle-v6-tiny+layout", alias = "paddle-v6-tiny-layout")]
    PaddleV6TinyLayout,
    /// Legacy PP-OCRv5 server tier (force_ocr)
    #[serde(rename = "paddle-v5-server", alias = "paddle-server")]
    PaddleServer,
    /// Legacy PP-OCRv5 server tier + layout detection
    #[serde(rename = "paddle-v5-server+layout", alias = "paddle-server-layout")]
    PaddleServerLayout,
    /// Tesseract OCR with auto_rotate enabled
    TesseractAutoRotate,
    /// PaddleOCR with auto_rotate enabled
    PaddleAutoRotate,
    /// PaddleOCR without auto_rotate (for comparison)
    PaddleNoRotate,
    /// Docling vendored extraction (read from file)
    Docling,
    /// PaddleOCR Python vendored extraction (read from file)
    PaddleOcrPython,
    /// RapidOCR vendored extraction (read from file)
    RapidOcr,
    /// Native PDF + layout detection + SLANeXT wired table model (forced)
    LayoutSlanetWired,
    /// Native PDF + layout detection + SLANeXT wireless table model (forced)
    LayoutSlanetWireless,
    /// Native PDF + layout detection + SLANet_plus table model
    LayoutSlanetPlus,
    /// Native PDF + layout detection + classifier-routed SLANeXT (wired/wireless auto)
    LayoutSlanetAuto,
    /// pdf_oxide backend text extraction (no OCR, no layout)
    PdfOxide,
    /// pdf_oxide backend + layout detection
    PdfOxideLayout,
    /// pdf_oxide backend + layout detection + reading-order reordering
    PdfOxideReadingOrder,
    /// Candle-based TrOCR (force_ocr, plain text)
    CandleTrocr,
    /// Candle-based PaddleOCR-VL (force_ocr, end-to-end markdown)
    CandlePaddleocrVl,
    /// Candle-based GLM-OCR vision-language backend (force_ocr)
    CandleGlmOcr,
    /// Candle-based GLM-OCR with layout detection + formula extraction
    CandleGlmOcrLayout,
    /// Candle-based GLM-OCR with layout detection + formula + chart understanding
    CandleGlmOcrLayoutChart,
    /// Candle-based DeepSeek-OCR vision-language backend (force_ocr)
    CandleDeepseekOcr,
    /// Candle-based PaddleOCR-VL 1.5 vision-language backend (force_ocr)
    CandlePaddleocrVl15,
}

impl Pipeline {
    pub fn name(&self) -> &'static str {
        match self {
            Pipeline::Baseline => "baseline",
            Pipeline::Layout => "layout",
            Pipeline::Tesseract => "tesseract",
            Pipeline::TesseractLayout => "tesseract+layout",
            Pipeline::TesseractSingleBlock => "tesseract-single-block",
            Pipeline::TesseractVerticalBlock => "tesseract-vertical-block",
            Pipeline::TesseractSparseText => "tesseract-sparse-text",
            Pipeline::Paddle => "paddle-v6-medium",
            Pipeline::PaddleLayout => "paddle-v6-medium+layout",
            Pipeline::PaddleV6Small => "paddle-v6-small",
            Pipeline::PaddleV6SmallLayout => "paddle-v6-small+layout",
            Pipeline::PaddleV6SmallLayoutDetSide1024 => "paddle-v6-small+layout+det-side-1024",
            Pipeline::PaddleV6SmallLayoutDetSide1536 => "paddle-v6-small+layout+det-side-1536",
            Pipeline::PaddleV6SmallLayoutDetSide2048 => "paddle-v6-small+layout+det-side-2048",
            Pipeline::PaddleV6SmallLayoutDetDbThresh020 => "paddle-v6-small+layout+det-db-thresh-020",
            Pipeline::PaddleV6SmallLayoutDetDbBoxThresh035 => "paddle-v6-small+layout+det-db-box-thresh-035",
            Pipeline::PaddleV6SmallLayoutDropScore030 => "paddle-v6-small+layout+drop-score-030",
            Pipeline::PaddleV6SmallLayoutDropScore040 => "paddle-v6-small+layout+drop-score-040",
            Pipeline::PaddleV6Tiny => "paddle-v6-tiny",
            Pipeline::PaddleV6TinyLayout => "paddle-v6-tiny+layout",
            Pipeline::PaddleServer => "paddle-v5-server",
            Pipeline::PaddleServerLayout => "paddle-v5-server+layout",
            Pipeline::TesseractAutoRotate => "tesseract-autorotate",
            Pipeline::PaddleAutoRotate => "paddle-autorotate",
            Pipeline::PaddleNoRotate => "paddle-norotate",
            Pipeline::Docling => "docling",
            Pipeline::PaddleOcrPython => "paddleocr-python",
            Pipeline::RapidOcr => "rapidocr",
            Pipeline::LayoutSlanetWired => "layout+slanet-wired",
            Pipeline::LayoutSlanetWireless => "layout+slanet-wireless",
            Pipeline::LayoutSlanetPlus => "layout+slanet-plus",
            Pipeline::LayoutSlanetAuto => "layout+slanet-auto",
            Pipeline::PdfOxide => "pdf-oxide",
            Pipeline::PdfOxideLayout => "pdf-oxide+layout",
            Pipeline::PdfOxideReadingOrder => "pdf-oxide+layout+reading-order",
            Pipeline::CandleTrocr => "candle-trocr",
            Pipeline::CandlePaddleocrVl => "candle-paddleocr-vl",
            Pipeline::CandleGlmOcr => "candle-glm-ocr",
            Pipeline::CandleGlmOcrLayout => "candle-glm-ocr+layout",
            Pipeline::CandleGlmOcrLayoutChart => "candle-glm-ocr+layout+chart",
            Pipeline::CandleDeepseekOcr => "candle-deepseek-ocr",
            Pipeline::CandlePaddleocrVl15 => "candle-paddleocr-vl-15",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "baseline" => Some(Pipeline::Baseline),
            "layout" => Some(Pipeline::Layout),
            "tesseract" => Some(Pipeline::Tesseract),
            "tesseract+layout" | "tesseract-layout" => Some(Pipeline::TesseractLayout),
            "tesseract-single-block" | "tesseract-psm6" => Some(Pipeline::TesseractSingleBlock),
            "tesseract-vertical-block" | "tesseract-vertical" | "tesseract-psm5" => {
                Some(Pipeline::TesseractVerticalBlock)
            }
            "tesseract-sparse-text" | "tesseract-psm11" => Some(Pipeline::TesseractSparseText),
            "paddle" | "paddle-mobile" | "paddle-v6-medium" => Some(Pipeline::Paddle),
            "paddle+layout"
            | "paddle-layout"
            | "paddle-mobile+layout"
            | "paddle-mobile-layout"
            | "paddle-v6-medium+layout"
            | "paddle-v6-medium-layout" => Some(Pipeline::PaddleLayout),
            "paddle-v6-small" => Some(Pipeline::PaddleV6Small),
            "paddle-v6-small+layout" | "paddle-v6-small-layout" => Some(Pipeline::PaddleV6SmallLayout),
            "paddle-v6-small+layout+det-side-1024" => Some(Pipeline::PaddleV6SmallLayoutDetSide1024),
            "paddle-v6-small+layout+det-side-1536" => Some(Pipeline::PaddleV6SmallLayoutDetSide1536),
            "paddle-v6-small+layout+det-side-2048" => Some(Pipeline::PaddleV6SmallLayoutDetSide2048),
            "paddle-v6-small+layout+det-db-thresh-020" => Some(Pipeline::PaddleV6SmallLayoutDetDbThresh020),
            "paddle-v6-small+layout+det-db-box-thresh-035" => Some(Pipeline::PaddleV6SmallLayoutDetDbBoxThresh035),
            "paddle-v6-small+layout+drop-score-030" => Some(Pipeline::PaddleV6SmallLayoutDropScore030),
            "paddle-v6-small+layout+drop-score-040" => Some(Pipeline::PaddleV6SmallLayoutDropScore040),
            "paddle-v6-tiny" => Some(Pipeline::PaddleV6Tiny),
            "paddle-v6-tiny+layout" | "paddle-v6-tiny-layout" => Some(Pipeline::PaddleV6TinyLayout),
            "paddle-server" | "paddle-v5-server" => Some(Pipeline::PaddleServer),
            "paddle-server+layout" | "paddle-server-layout" | "paddle-v5-server+layout" | "paddle-v5-server-layout" => {
                Some(Pipeline::PaddleServerLayout)
            }
            "tesseract-autorotate" => Some(Pipeline::TesseractAutoRotate),
            "paddle-autorotate" => Some(Pipeline::PaddleAutoRotate),
            "paddle-norotate" => Some(Pipeline::PaddleNoRotate),
            "docling" => Some(Pipeline::Docling),
            "paddleocr-python" => Some(Pipeline::PaddleOcrPython),
            "rapidocr" => Some(Pipeline::RapidOcr),
            "layout+slanet-wired" | "layout-slanet-wired" => Some(Pipeline::LayoutSlanetWired),
            "layout+slanet-wireless" | "layout-slanet-wireless" => Some(Pipeline::LayoutSlanetWireless),
            "layout+slanet-plus" | "layout-slanet-plus" => Some(Pipeline::LayoutSlanetPlus),
            "layout+slanet-auto" | "layout-slanet-auto" | "layout+slanet" | "layout-slanet" => {
                Some(Pipeline::LayoutSlanetAuto)
            }
            "pdf-oxide" | "pdf_oxide" | "oxide" => Some(Pipeline::PdfOxide),
            "pdf-oxide+layout" | "pdf-oxide-layout" | "oxide+layout" | "oxide-layout" => Some(Pipeline::PdfOxideLayout),
            "pdf-oxide+layout+reading-order"
            | "pdf-oxide-layout-reading-order"
            | "oxide+layout+reading-order"
            | "oxide-layout-reading-order" => Some(Pipeline::PdfOxideReadingOrder),
            "candle-trocr" | "candle_trocr" | "trocr" => Some(Pipeline::CandleTrocr),
            "candle-paddleocr-vl" | "candle_paddleocr_vl" | "paddleocr-vl" => Some(Pipeline::CandlePaddleocrVl),
            "candle-glm-ocr" | "candle_glm_ocr" | "glm-ocr" => Some(Pipeline::CandleGlmOcr),
            "candle-glm-ocr+layout" | "candle_glm_ocr_layout" | "glm-ocr+layout" | "glm-ocr-layout" => {
                Some(Pipeline::CandleGlmOcrLayout)
            }
            "candle-glm-ocr+layout+chart"
            | "candle_glm_ocr_layout_chart"
            | "glm-ocr+layout+chart"
            | "glm-ocr-layout-chart" => Some(Pipeline::CandleGlmOcrLayoutChart),
            "candle-deepseek-ocr" | "candle_deepseek_ocr" | "deepseek-ocr" => Some(Pipeline::CandleDeepseekOcr),
            "candle-paddleocr-vl-15" | "candle_paddleocr_vl_15" | "paddleocr-vl-15" => {
                Some(Pipeline::CandlePaddleocrVl15)
            }
            _ => None,
        }
    }

    /// All pipelines that use xberg in-process extraction.
    ///
    /// `CandleTrocr`, `CandlePaddleocrVl`, and the new Candle VLM backends
    /// (`CandleDeepseekOcr`, `CandlePaddleocrVl15`) are
    /// deliberately omitted from `all_xberg()`: they need large model
    /// downloads from HuggingFace and only build with their own feature flags,
    /// so default cross-pipeline runs do not include them.
    /// Named Paddle quality-sweep presets are also omitted so experimental configurations cannot
    /// silently expand release benchmark matrices. ~keep
    /// `CandleGlmOcr` is included because the `glm-ocr-bench` feature gates
    /// the entire harness build, making the inclusion safe.
    pub fn all_xberg() -> Vec<Pipeline> {
        vec![
            Pipeline::Baseline,
            Pipeline::Layout,
            Pipeline::Tesseract,
            Pipeline::TesseractLayout,
            Pipeline::Paddle,
            Pipeline::PaddleLayout,
            Pipeline::PaddleV6Small,
            Pipeline::PaddleV6SmallLayout,
            Pipeline::PaddleV6Tiny,
            Pipeline::PaddleV6TinyLayout,
            Pipeline::PdfOxide,
            Pipeline::PdfOxideLayout,
            Pipeline::CandleGlmOcr,
        ]
    }
}

/// Configuration for a comparison run.
pub struct ComparisonConfig {
    pub fixtures_dir: std::path::PathBuf,
    pub pipelines: Vec<Pipeline>,
    pub dump_outputs: bool,
    pub guardrails: bool,
    /// Path to a JSON guardrails file. When `guardrails` is true this file
    /// is loaded instead of using hardcoded thresholds.
    pub guardrails_file: Option<std::path::PathBuf>,
    /// Optional name filter (only run docs whose name contains this)
    pub name_filter: Option<String>,
    /// Optional exact fixture `metadata.category` filter.
    pub category_filter: Option<String>,
    /// Optional path to write full comparison results as JSON.
    pub json_output: Option<std::path::PathBuf>,
    /// Run noise detection on extracted outputs.
    pub noise: bool,
    /// Enable diagnostic diff mode for poor-scoring documents.
    pub diagnose: bool,
    /// SF1 threshold below which to generate diagnostics.
    pub diagnose_threshold: f64,
}

/// Result of running one pipeline on one document.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineResult {
    pub pipeline: Pipeline,
    pub sf1: f64,
    pub tf1: f64,
    /// Order-/character-sensitive text similarity (`1 − CER`, 0.0–1.0). Report-only
    /// OCR diagnostic; NaN for empty or very long documents (see `quality`).
    #[serde(default)]
    pub char_similarity: f64,
    /// Reading order score (LIS-based, 0.0-1.0).
    #[serde(default)]
    pub order_score: f64,
    /// Per-block-type structural F1 scores (e.g. "H1" -> 0.85).
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub per_type_sf1: HashMap<String, f64>,
    /// Per-dimension precision (report-only): low value ⇒ over-fabrication.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub per_type_precision: HashMap<String, f64>,
    /// Per-dimension recall (report-only): low value ⇒ omission.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub per_type_recall: HashMap<String, f64>,
    pub time_ms: f64,
    /// Top tokens present in GT but missing/under-represented in extraction (recall misses).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub missing_tokens: Vec<(String, usize)>,
    /// Top tokens present in extraction but absent/over-represented vs GT (precision misses).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extra_tokens: Vec<(String, usize)>,
    #[serde(skip)]
    pub content: String,
}

/// Result of running all pipelines on one document.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocResult {
    pub name: String,
    pub file_type: String,
    pub results: Vec<PipelineResult>,
}

/// Estimate PDF page count from file size (~12KB/page heuristic).
///
/// Fallback for when the PDF cannot be opened for a real page count.
fn estimate_pdf_page_count_from_size(file_size: u64) -> u64 {
    (file_size / 12_000).max(1)
}

/// Compute extraction timeout scaled by document page count.
///
/// Returns timeout in seconds. For PDFs, applies per-page scaling to accommodate
/// layout detection inference on large documents. Uses the real page count when
/// the document parses; falls back to a size-based estimate otherwise.
fn compute_extraction_timeout_secs(path: &Path, file_type: &str, force_ocr: bool) -> u64 {
    let base_timeout_secs = if force_ocr {
        FORCED_OCR_TIMEOUT_BASE_SECS
    } else {
        EXTRACTION_TIMEOUT_BASE_SECS
    };
    if file_type.to_lowercase() != "pdf" {
        return base_timeout_secs;
    }

    let page_count = std::fs::read(path)
        .ok()
        .and_then(|bytes| xberg::pdf_page_count(&bytes, None).ok())
        .map(|count| count as u64)
        .or_else(|| {
            std::fs::metadata(path)
                .ok()
                .map(|m| estimate_pdf_page_count_from_size(m.len()))
        });

    let Some(page_count) = page_count else {
        tracing::warn!("Could not size PDF file, using base timeout: {}", path.display());
        return base_timeout_secs;
    };

    let per_page_secs = EXTRACTION_TIMEOUT_PER_PAGE_MS as f64 / 1000.0;
    let scaled_timeout = base_timeout_secs as f64 + (page_count as f64 * per_page_secs);

    scaled_timeout.ceil() as u64
}

fn build_paddle_extraction_config(
    model_version: &str,
    model_tier: &str,
    layout: Option<xberg::core::config::layout::LayoutDetectionConfig>,
) -> xberg::ExtractionConfig {
    build_paddle_extraction_config_with_quality_profile(model_version, model_tier, layout, None)
}

#[derive(Debug, Clone, Copy)]
struct PaddleQualityProfile {
    det_limit_side_len: u32,
    det_db_thresh: f32,
    det_db_box_thresh: f32,
    drop_score: f32,
}

const PADDLE_DEFAULT_QUALITY_PROFILE: PaddleQualityProfile = PaddleQualityProfile {
    det_limit_side_len: PADDLE_DET_SIDE_1024,
    det_db_thresh: PADDLE_DET_DB_THRESH_030,
    det_db_box_thresh: PADDLE_DET_DB_BOX_THRESH_050,
    drop_score: PADDLE_DROP_SCORE_050,
};
const PADDLE_DET_SIDE_1536_QUALITY_PROFILE: PaddleQualityProfile = PaddleQualityProfile {
    det_limit_side_len: PADDLE_DET_SIDE_1536,
    ..PADDLE_DEFAULT_QUALITY_PROFILE
};
const PADDLE_DET_SIDE_2048_QUALITY_PROFILE: PaddleQualityProfile = PaddleQualityProfile {
    det_limit_side_len: PADDLE_DET_SIDE_2048,
    ..PADDLE_DEFAULT_QUALITY_PROFILE
};
const PADDLE_DET_DB_THRESH_020_QUALITY_PROFILE: PaddleQualityProfile = PaddleQualityProfile {
    det_db_thresh: PADDLE_DET_DB_THRESH_020,
    ..PADDLE_DEFAULT_QUALITY_PROFILE
};
const PADDLE_DET_DB_BOX_THRESH_035_QUALITY_PROFILE: PaddleQualityProfile = PaddleQualityProfile {
    det_db_box_thresh: PADDLE_DET_DB_BOX_THRESH_035,
    ..PADDLE_DEFAULT_QUALITY_PROFILE
};
const PADDLE_DROP_SCORE_030_QUALITY_PROFILE: PaddleQualityProfile = PaddleQualityProfile {
    drop_score: PADDLE_DROP_SCORE_030,
    ..PADDLE_DEFAULT_QUALITY_PROFILE
};
const PADDLE_DROP_SCORE_040_QUALITY_PROFILE: PaddleQualityProfile = PaddleQualityProfile {
    drop_score: PADDLE_DROP_SCORE_040,
    ..PADDLE_DEFAULT_QUALITY_PROFILE
};

fn build_paddle_extraction_config_with_quality_profile(
    model_version: &str,
    model_tier: &str,
    layout: Option<xberg::core::config::layout::LayoutDetectionConfig>,
    quality_profile: Option<PaddleQualityProfile>,
) -> xberg::ExtractionConfig {
    let mut paddle_ocr_config = serde_json::json!({
        "model_version": model_version,
        "model_tier": model_tier
    });
    if let Some(profile) = quality_profile {
        paddle_ocr_config["det_limit_side_len"] = serde_json::json!(profile.det_limit_side_len);
        paddle_ocr_config["det_db_thresh"] = serde_json::json!(profile.det_db_thresh);
        paddle_ocr_config["det_db_box_thresh"] = serde_json::json!(profile.det_db_box_thresh);
        paddle_ocr_config["drop_score"] = serde_json::json!(profile.drop_score);
    }

    xberg::ExtractionConfig {
        output_format: xberg::core::config::OutputFormat::Markdown,
        force_ocr: true,
        ocr: Some(xberg::core::config::OcrConfig {
            backend: "paddleocr".to_string(),
            language: vec!["eng".to_string()],
            auto_rotate: false,
            paddle_ocr_config: Some(paddle_ocr_config),
            ..Default::default()
        }),
        layout,
        ..Default::default()
    }
}

fn build_paddle_v6_small_layout_quality_config(profile: PaddleQualityProfile) -> xberg::ExtractionConfig {
    build_paddle_extraction_config_with_quality_profile(
        PP_OCR_V6,
        "small",
        Some(xberg::core::config::layout::LayoutDetectionConfig::default()),
        Some(profile),
    )
}

fn build_tesseract_extraction_config(psm: i32) -> xberg::ExtractionConfig {
    xberg::ExtractionConfig {
        output_format: xberg::core::config::OutputFormat::Markdown,
        force_ocr: true,
        ocr: Some(xberg::core::config::OcrConfig {
            backend: "tesseract".to_string(),
            language: vec!["eng".to_string()],
            tesseract_config: Some(xberg::TesseractConfig {
                language: vec!["eng".to_string()],
                psm,
                ..Default::default()
            }),
            ..Default::default()
        }),
        ..Default::default()
    }
}

fn disable_timed_extraction_caches(config: &mut xberg::ExtractionConfig) {
    config.use_cache = false;

    // Only flip `use_cache` on a `tesseract_config` that already exists — an explicit PSM preset
    // (`build_tesseract_extraction_config`). Never materialize one here: this runs before any
    // fixture-specific language is known (`build_extraction_config`'s own tail), so a pipeline
    // that leaves `tesseract_config` implicit must wait for `finalize_timed_ocr_result_cache`,
    // which runs after `apply_fixture_ocr_language` and can pick the PSM that matches the real
    // (final) language instead of a placeholder. ~keep
    flip_existing_tesseract_result_caches(config);
}

fn flip_existing_tesseract_result_caches(config: &mut xberg::ExtractionConfig) {
    let Some(ocr) = config.ocr.as_mut() else {
        return;
    };
    if let Some(pipeline) = ocr.pipeline.as_mut() {
        for stage in &mut pipeline.stages {
            if stage.backend == "tesseract"
                && let Some(tesseract_config) = stage.tesseract_config.as_mut()
            {
                tesseract_config.use_cache = false;
            }
        }
    } else if ocr.backend == "tesseract"
        && let Some(tesseract_config) = ocr.tesseract_config.as_mut()
    {
        tesseract_config.use_cache = false;
    }
}

/// Materializes xberg's own OCR fallback default (`OcrConfig::default()`, backend `"tesseract"`)
/// when a pipeline left `config.ocr` entirely `None` (e.g. `Pipeline::Baseline`/`PdfOxide`'s
/// native-with-OCR-fallback pipelines). Gives a fixture's OCR language somewhere to attach via
/// `apply_fixture_ocr_language`, without forcing `force_ocr`. Must run *before* that call. ~keep
fn materialize_implicit_ocr_config(config: &mut xberg::ExtractionConfig) {
    if config.ocr.is_none() {
        config.ocr = Some(xberg::OcrConfig::default());
    }
}

/// Finishes disabling the timed Tesseract OCR result cache once the fixture's language (if any)
/// has already been applied via `apply_fixture_ocr_language`. Must run *after* that call, or it
/// would compute PSM from the wrong (pre-fixture-language) language.
///
/// Flips `use_cache` on a `tesseract_config` that already exists (an explicit PSM preset) without
/// touching its `psm`. For a pipeline that leaves `tesseract_config` implicit (auto-PSM),
/// materializes one with `use_cache = false` and the PSM
/// `apply_default_whole_image_tesseract_psm` in `crates/xberg/src/extractors/image.rs` would
/// itself have picked for the config's current (now-final) language — so the result cache is
/// genuinely disabled without silently regressing to `TesseractConfig::default()`'s PSM 3. ~keep
fn finalize_timed_ocr_result_cache(config: &mut xberg::ExtractionConfig) {
    let Some(ocr) = config.ocr.as_mut() else {
        return;
    };
    let parent_language = ocr.language.clone();
    if let Some(pipeline) = ocr.pipeline.as_mut() {
        for stage in &mut pipeline.stages {
            if stage.backend != "tesseract" {
                continue;
            }
            // A present `stage.language` wins over the parent's, matching xberg's own stage
            // semantics ("None = use parent OcrConfig.language" — `OcrPipelineStage::language` in
            // `crates/xberg/src/core/config/ocr.rs`). This is only correct because
            // `apply_fixture_ocr_language` refreshes `stage.language` too, not just the parent's
            // — otherwise a stale per-stage override would compute PSM from the wrong language
            // here. ~keep
            let languages = stage.language.clone().unwrap_or_else(|| parent_language.clone());
            materialize_or_disable_tesseract_result_cache(&mut stage.tesseract_config, &languages);
        }
    } else if ocr.backend == "tesseract" {
        materialize_or_disable_tesseract_result_cache(&mut ocr.tesseract_config, &parent_language);
    }
}

fn materialize_or_disable_tesseract_result_cache(
    tesseract_config: &mut Option<xberg::TesseractConfig>,
    languages: &[String],
) {
    match tesseract_config.as_mut() {
        Some(existing) => existing.use_cache = false,
        None => {
            *tesseract_config = Some(xberg::TesseractConfig {
                language: languages.to_vec(),
                psm: crate::adapter::xberg_default_tesseract_psm(languages),
                use_cache: false,
                ..Default::default()
            });
        }
    }
}

/// Build a xberg ExtractionConfig for the given pipeline.
pub fn build_extraction_config(pipeline: Pipeline) -> xberg::ExtractionConfig {
    use xberg::core::config::{OutputFormat, layout::LayoutDetectionConfig};

    let base = xberg::ExtractionConfig {
        output_format: OutputFormat::Markdown,
        ..Default::default()
    };

    let mut config = match pipeline {
        Pipeline::Baseline => base,
        Pipeline::Layout => xberg::ExtractionConfig {
            layout: Some(LayoutDetectionConfig::default()),
            use_layout_for_markdown: true,
            ocr: Some(xberg::core::config::OcrConfig {
                backend: "tesseract".to_string(),
                language: vec!["eng".to_string()],
                auto_rotate: true,
                ..Default::default()
            }),
            ..base
        },
        Pipeline::Tesseract => xberg::ExtractionConfig {
            force_ocr: true,
            ocr: Some(xberg::core::config::OcrConfig {
                backend: "tesseract".to_string(),
                language: vec!["eng".to_string()],
                ..Default::default()
            }),
            ..base
        },
        Pipeline::TesseractLayout => xberg::ExtractionConfig {
            force_ocr: true,
            ocr: Some(xberg::core::config::OcrConfig {
                backend: "tesseract".to_string(),
                language: vec!["eng".to_string()],
                ..Default::default()
            }),
            layout: Some(LayoutDetectionConfig::default()),
            ..base
        },
        Pipeline::TesseractSingleBlock => build_tesseract_extraction_config(TESSERACT_PSM_SINGLE_BLOCK),
        Pipeline::TesseractVerticalBlock => build_tesseract_extraction_config(TESSERACT_PSM_VERTICAL_BLOCK),
        Pipeline::TesseractSparseText => build_tesseract_extraction_config(TESSERACT_PSM_SPARSE_TEXT),
        Pipeline::Paddle => build_paddle_extraction_config(PP_OCR_V6, "medium", None),
        Pipeline::PaddleLayout => {
            build_paddle_extraction_config(PP_OCR_V6, "medium", Some(LayoutDetectionConfig::default()))
        }
        Pipeline::PaddleV6Small => build_paddle_extraction_config(PP_OCR_V6, "small", None),
        Pipeline::PaddleV6SmallLayout => {
            build_paddle_extraction_config(PP_OCR_V6, "small", Some(LayoutDetectionConfig::default()))
        }
        Pipeline::PaddleV6SmallLayoutDetSide1024 => {
            build_paddle_v6_small_layout_quality_config(PADDLE_DEFAULT_QUALITY_PROFILE)
        }
        Pipeline::PaddleV6SmallLayoutDetSide1536 => {
            build_paddle_v6_small_layout_quality_config(PADDLE_DET_SIDE_1536_QUALITY_PROFILE)
        }
        Pipeline::PaddleV6SmallLayoutDetSide2048 => {
            build_paddle_v6_small_layout_quality_config(PADDLE_DET_SIDE_2048_QUALITY_PROFILE)
        }
        Pipeline::PaddleV6SmallLayoutDetDbThresh020 => {
            build_paddle_v6_small_layout_quality_config(PADDLE_DET_DB_THRESH_020_QUALITY_PROFILE)
        }
        Pipeline::PaddleV6SmallLayoutDetDbBoxThresh035 => {
            build_paddle_v6_small_layout_quality_config(PADDLE_DET_DB_BOX_THRESH_035_QUALITY_PROFILE)
        }
        Pipeline::PaddleV6SmallLayoutDropScore030 => {
            build_paddle_v6_small_layout_quality_config(PADDLE_DROP_SCORE_030_QUALITY_PROFILE)
        }
        Pipeline::PaddleV6SmallLayoutDropScore040 => {
            build_paddle_v6_small_layout_quality_config(PADDLE_DROP_SCORE_040_QUALITY_PROFILE)
        }
        Pipeline::PaddleV6Tiny => build_paddle_extraction_config(PP_OCR_V6, "tiny", None),
        Pipeline::PaddleV6TinyLayout => {
            build_paddle_extraction_config(PP_OCR_V6, "tiny", Some(LayoutDetectionConfig::default()))
        }
        Pipeline::PaddleServer => build_paddle_extraction_config(PP_OCR_V5, "server", None),
        Pipeline::PaddleServerLayout => {
            build_paddle_extraction_config(PP_OCR_V5, "server", Some(LayoutDetectionConfig::default()))
        }
        Pipeline::TesseractAutoRotate => xberg::ExtractionConfig {
            force_ocr: true,
            ocr: Some(xberg::core::config::OcrConfig {
                backend: "tesseract".to_string(),
                language: vec!["eng".to_string()],
                auto_rotate: true,
                ..Default::default()
            }),
            ..base
        },
        Pipeline::PaddleAutoRotate => {
            let mut config = build_paddle_extraction_config(PP_OCR_V6, "medium", None);
            config
                .ocr
                .as_mut()
                .expect("Paddle benchmark config must include OCR")
                .auto_rotate = true;
            config
        }
        Pipeline::PaddleNoRotate => build_paddle_extraction_config(PP_OCR_V6, "medium", None),
        Pipeline::LayoutSlanetAuto => xberg::ExtractionConfig {
            layout: Some(LayoutDetectionConfig {
                table_model: xberg::core::config::layout::TableModel::SlanetAuto,
                ..Default::default()
            }),
            ocr: Some(xberg::core::config::OcrConfig {
                backend: "tesseract".to_string(),
                language: vec!["eng".to_string()],
                ..Default::default()
            }),
            ..base
        },
        Pipeline::PdfOxide => xberg::ExtractionConfig {
            pdf_options: Some(xberg::PdfConfig { ..Default::default() }),
            ..base
        },
        Pipeline::PdfOxideLayout => xberg::ExtractionConfig {
            pdf_options: Some(xberg::PdfConfig { ..Default::default() }),
            layout: Some(LayoutDetectionConfig::default()),
            use_layout_for_markdown: true,
            ocr: Some(xberg::core::config::OcrConfig {
                backend: "tesseract".to_string(),
                language: vec!["eng".to_string()],
                ..Default::default()
            }),
            ..base
        },
        Pipeline::PdfOxideReadingOrder => xberg::ExtractionConfig {
            pdf_options: Some(xberg::PdfConfig {
                reading_order: true,
                ..Default::default()
            }),
            layout: Some(LayoutDetectionConfig::default()),
            use_layout_for_markdown: true,
            ocr: Some(xberg::core::config::OcrConfig {
                backend: "tesseract".to_string(),
                language: vec!["eng".to_string()],
                ..Default::default()
            }),
            ..base
        },
        Pipeline::Docling | Pipeline::PaddleOcrPython | Pipeline::RapidOcr => base,
        Pipeline::LayoutSlanetWired => xberg::ExtractionConfig {
            layout: Some(LayoutDetectionConfig {
                table_model: xberg::core::config::layout::TableModel::SlanetWired,
                ..Default::default()
            }),
            ocr: Some(xberg::core::config::OcrConfig {
                backend: "tesseract".to_string(),
                language: vec!["eng".to_string()],
                ..Default::default()
            }),
            ..base
        },
        Pipeline::LayoutSlanetWireless => xberg::ExtractionConfig {
            layout: Some(LayoutDetectionConfig {
                table_model: xberg::core::config::layout::TableModel::SlanetWireless,
                ..Default::default()
            }),
            ocr: Some(xberg::core::config::OcrConfig {
                backend: "tesseract".to_string(),
                language: vec!["eng".to_string()],
                ..Default::default()
            }),
            ..base
        },
        Pipeline::LayoutSlanetPlus => xberg::ExtractionConfig {
            layout: Some(LayoutDetectionConfig {
                table_model: xberg::core::config::layout::TableModel::SlanetPlus,
                ..Default::default()
            }),
            ocr: Some(xberg::core::config::OcrConfig {
                backend: "tesseract".to_string(),
                language: vec!["eng".to_string()],
                ..Default::default()
            }),
            ..base
        },
        Pipeline::CandleTrocr => xberg::ExtractionConfig {
            force_ocr: true,
            ocr: Some(xberg::core::config::OcrConfig {
                backend: "candle-trocr".to_string(),
                language: vec!["eng".to_string()],
                ..Default::default()
            }),
            ..base
        },
        Pipeline::CandlePaddleocrVl => xberg::ExtractionConfig {
            force_ocr: true,
            ocr: Some(xberg::core::config::OcrConfig {
                backend: "candle-paddleocr-vl".to_string(),
                language: vec!["eng".to_string()],
                ..Default::default()
            }),
            ..base
        },
        Pipeline::CandleGlmOcr => xberg::ExtractionConfig {
            force_ocr: true,
            ocr: Some(xberg::core::config::OcrConfig {
                backend: "candle-glm-ocr".to_string(),
                language: vec!["en".to_string()],
                ..Default::default()
            }),
            ..base
        },
        Pipeline::CandleGlmOcrLayout => xberg::ExtractionConfig {
            force_ocr: true,
            ocr: Some(xberg::core::config::OcrConfig {
                backend: "candle-glm-ocr".to_string(),
                language: vec!["en".to_string()],
                ..Default::default()
            }),
            layout: Some(LayoutDetectionConfig::default()),
            ..base
        },
        Pipeline::CandleGlmOcrLayoutChart => xberg::ExtractionConfig {
            force_ocr: true,
            ocr: Some(xberg::core::config::OcrConfig {
                backend: "candle-glm-ocr".to_string(),
                language: vec!["en".to_string()],
                ..Default::default()
            }),
            layout: Some(LayoutDetectionConfig {
                enable_chart_understanding: true,
                ..Default::default()
            }),
            ..base
        },
        Pipeline::CandleDeepseekOcr => xberg::ExtractionConfig {
            force_ocr: true,
            ocr: Some(xberg::core::config::OcrConfig {
                backend: "candle-deepseek-ocr".to_string(),
                language: vec!["en".to_string()],
                ..Default::default()
            }),
            ..base
        },
        Pipeline::CandlePaddleocrVl15 => xberg::ExtractionConfig {
            force_ocr: true,
            ocr: Some(xberg::core::config::OcrConfig {
                backend: "candle-paddleocr-vl".to_string(),
                language: vec!["en".to_string()],
                ..Default::default()
            }),
            ..base
        },
    };

    disable_timed_extraction_caches(&mut config);
    config
}

/// Structural scoring outputs for one document: the SF1 rollup, reading-order
/// score, and the per-dimension F1 / precision / recall maps used by triage.
/// Precision/recall are report-only diagnostics (not part of SF1).
#[derive(Debug, Clone, Default)]
pub struct StructuralBreakdown {
    pub sf1: f64,
    pub order_score: f64,
    pub per_type_sf1: HashMap<String, f64>,
    pub per_type_precision: HashMap<String, f64>,
    pub per_type_recall: HashMap<String, f64>,
}

impl StructuralBreakdown {
    fn unavailable() -> Self {
        Self {
            sf1: f64::NAN,
            order_score: f64::NAN,
            ..Default::default()
        }
    }

    /// Explode a canonical [`StructuralScore`] into per-dimension maps.
    pub fn from_score(score: &structural_sidecar::StructuralScore) -> Self {
        let mut per_type_sf1 = HashMap::new();
        let mut per_type_precision = HashMap::new();
        let mut per_type_recall = HashMap::new();
        for (name, bd) in score.dimensions_pr() {
            per_type_sf1.insert(name.to_string(), bd.f1);
            per_type_precision.insert(name.to_string(), bd.precision);
            per_type_recall.insert(name.to_string(), bd.recall);
        }
        Self {
            sf1: score.sf1,
            order_score: score.d5_order,
            per_type_sf1,
            per_type_precision,
            per_type_recall,
        }
    }
}

/// Score extracted content against ground truth, returning the text F1 and the
/// structural breakdown (SF1, reading order, per-dimension F1/precision/recall).
pub fn score_document(content: &str, gt_text: &str, gt_markdown: Option<&str>) -> (f64, StructuralBreakdown) {
    let tf1 = {
        let ext_tokens = tokenize(content);
        let gt_tokens = tokenize(gt_text);
        compute_f1(&ext_tokens, &gt_tokens)
    };
    let breakdown = match gt_markdown {
        Some(md) => StructuralBreakdown::from_score(&structural_sidecar::score_markdown(content, md)),
        None => StructuralBreakdown::unavailable(),
    };
    (tf1, breakdown)
}

fn resolve_vendored_dir(fixtures_path: &Path, vendored_name: &str) -> std::path::PathBuf {
    let search_start = if fixtures_path.is_file() {
        fixtures_path.parent().unwrap_or(fixtures_path)
    } else {
        fixtures_path
    };
    let fixtures_root = search_start
        .ancestors()
        .find(|ancestor| ancestor.file_name().is_some_and(|name| name == "fixtures"))
        .unwrap_or(search_start);

    fixtures_root
        .parent()
        .unwrap_or(fixtures_root)
        .join("vendored")
        .join(vendored_name)
}

/// Read vendored markdown + cached timing for a single document.
///
/// Returns an error when the cached markdown is missing or empty. A missing
/// timing remains valid for quality-only comparisons and is represented by
/// `NaN`.
pub fn read_vendored_cached(doc_name: &str, fixtures_path: &Path, vendored_name: &str) -> Result<(String, f64)> {
    let vendored_dir = resolve_vendored_dir(fixtures_path, vendored_name);
    let md_path = vendored_dir.join("md").join(format!("{}.md", doc_name));
    let timing_path = vendored_dir.join("timing").join(format!("{}.ms", doc_name));
    let md = std::fs::read_to_string(&md_path).map_err(|error| {
        crate::Error::Benchmark(format!(
            "Failed to read vendored {vendored_name} output for {doc_name} at {}: {error}",
            md_path.display()
        ))
    })?;
    if md.trim().is_empty() {
        return Err(crate::Error::Benchmark(format!(
            "Vendored {vendored_name} output for {doc_name} is empty at {}",
            md_path.display()
        )));
    }
    let cached_ms = std::fs::read_to_string(&timing_path)
        .ok()
        .and_then(|s| s.trim().parse::<f64>().ok())
        .unwrap_or(f64::NAN);
    Ok((md, cached_ms))
}

fn apply_fixture_ocr_language(config: &mut xberg::ExtractionConfig, doc: &CorpusDocument) {
    let Some(language) = doc.metadata.get("ocr_language").and_then(serde_json::Value::as_str) else {
        return;
    };
    let languages = crate::adapter::canonicalize_ocr_languages(language);
    if languages.is_empty() {
        return;
    }
    if let Some(ocr) = config.ocr.as_mut() {
        ocr.language = languages.clone();
        if let Some(pipeline) = ocr.pipeline.as_mut() {
            for stage in &mut pipeline.stages {
                if stage.backend != "tesseract" {
                    continue;
                }
                // A stage's own `language` override (`None` = inherit the parent
                // `OcrConfig.language`) must also be refreshed to the fixture's language, not
                // just the parent's — otherwise a stage that already pins its own language (e.g.
                // `Some(["eng"])`) stays stale at real extraction time (xberg prefers a present
                // stage override over the parent), AND `finalize_timed_ocr_result_cache` would
                // compute PSM from that stale language instead of the fixture's real one. ~keep
                stage.language = Some(languages.clone());
                if let Some(tesseract) = stage.tesseract_config.as_mut() {
                    tesseract.language = languages.clone();
                }
            }
        } else if ocr.backend == "tesseract"
            && let Some(tesseract) = ocr.tesseract_config.as_mut()
        {
            // The timed benchmark materializes this nested config to disable its cache. ~keep
            tesseract.language = languages;
        }
    }
}

/// Extract content from a document using the given pipeline.
/// Returns (content, time_ms).
pub async fn extract_pipeline(
    pipeline: Pipeline,
    doc: &crate::corpus::CorpusDocument,
    fixtures_dir: &std::path::Path,
) -> (Option<String>, f64) {
    match pipeline {
        Pipeline::Docling | Pipeline::PaddleOcrPython | Pipeline::RapidOcr => {
            let vendored_name = match pipeline {
                Pipeline::PaddleOcrPython => "paddleocr-python",
                Pipeline::RapidOcr => "rapidocr",
                _ => "docling",
            };
            match read_vendored_cached(&doc.name, fixtures_dir, vendored_name) {
                Ok((content, time_ms)) => (Some(content), time_ms),
                Err(error) => {
                    eprintln!("  ERROR {}/{}: {}", doc.name, pipeline.name(), error);
                    (None, f64::NAN)
                }
            }
        }
        _ => {
            let t = Instant::now();
            let mut config = build_extraction_config(pipeline);
            // Materialize implicit OCR before applying fixture metadata so baseline PDF extraction
            // keeps force_ocr disabled while its fallback OCR uses the requested language. Finalize
            // the Tesseract result-cache control (materializing an implicit tesseract_config, if
            // needed) only after the language is final, so the materialized PSM matches the real
            // fixture language instead of a pre-language placeholder. ~keep
            materialize_implicit_ocr_config(&mut config);
            apply_fixture_ocr_language(&mut config, doc);
            finalize_timed_ocr_result_cache(&mut config);
            let doc_path = doc.document_path.clone();
            let doc_name = doc.name.clone();
            let pipeline_name = pipeline.name().to_string();

            let extraction_timeout_secs = compute_extraction_timeout_secs(&doc_path, &doc.file_type, config.force_ocr);
            config.extraction_timeout_secs = Some(extraction_timeout_secs);

            let outer_timeout_secs = ((extraction_timeout_secs as f64 * 1.5).ceil() as u64).max(180);
            let extraction_future = async {
                tokio::time::timeout(
                    std::time::Duration::from_secs(outer_timeout_secs),
                    crate::extract_xberg_file(&doc_path, &config),
                )
                .await
            };

            let result = match std::panic::AssertUnwindSafe(extraction_future).catch_unwind().await {
                Ok(Ok(Ok(result))) => Some(result.content),
                Ok(Ok(Err(e))) => {
                    eprintln!("  ERROR {}/{}: {}", doc_name, pipeline_name, e);
                    None
                }
                Ok(Err(_)) => {
                    eprintln!(
                        "  TIMEOUT {}/{}: exceeded {}s (outer safety timeout)",
                        doc_name, pipeline_name, outer_timeout_secs
                    );
                    None
                }
                Err(panic_info) => {
                    let panic_msg = if let Some(s) = panic_info.downcast_ref::<String>() {
                        s.clone()
                    } else if let Some(s) = panic_info.downcast_ref::<&str>() {
                        (*s).to_string()
                    } else {
                        format!("{:?}", panic_info)
                    };
                    eprintln!(
                        "  PANIC {}/{}: {}\n    document: {}\n    pipeline: {}\n    file_type: {}",
                        doc_name,
                        pipeline_name,
                        panic_msg,
                        doc_path.display(),
                        pipeline_name,
                        doc.file_type,
                    );
                    None
                }
            };
            (result, t.elapsed().as_secs_f64() * 1000.0)
        }
    }
}

/// Run a single pipeline on a single document and score it.
///
/// Timeouts, errors, and panics produce NaN scores so they are tracked
/// in the results but excluded from aggregate averages.
async fn run_pipeline(
    pipeline: Pipeline,
    doc: &CorpusDocument,
    gt_text: &str,
    gt_markdown: Option<&str>,
    fixtures_dir: &std::path::Path,
) -> PipelineResult {
    let (content_opt, time_ms) = extract_pipeline(pipeline, doc, fixtures_dir).await;
    let extraction_failed = content_opt.is_none();
    let content = content_opt.unwrap_or_default();
    let (tf1, structural) = if extraction_failed || (content.is_empty() && time_ms > 170_000.0) {
        (
            f64::NAN,
            StructuralBreakdown {
                sf1: f64::NAN,
                order_score: f64::NAN,
                ..Default::default()
            },
        )
    } else {
        score_document(&content, gt_text, gt_markdown)
    };

    let char_similarity = if extraction_failed {
        f64::NAN
    } else {
        crate::quality::normalized_edit_similarity(&content, gt_text)
    };

    let ext_tokens = tokenize(&content);
    let gt_tokens = tokenize(gt_text);
    let (mut missing_tokens, mut extra_tokens) = compute_token_diff(&ext_tokens, &gt_tokens);
    missing_tokens.truncate(50);
    extra_tokens.truncate(50);

    PipelineResult {
        pipeline,
        sf1: structural.sf1,
        tf1,
        char_similarity,
        order_score: structural.order_score,
        per_type_sf1: structural.per_type_sf1,
        per_type_precision: structural.per_type_precision,
        per_type_recall: structural.per_type_recall,
        time_ms,
        missing_tokens,
        extra_tokens,
        content,
    }
}

/// Run the full comparison across all documents and pipelines.
pub async fn run_comparison(config: &ComparisonConfig) -> Result<Vec<DocResult>> {
    let filter = CorpusFilter {
        file_types: None,
        require_ground_truth: true,
        require_markdown_ground_truth: false,
        name_patterns: config.name_filter.clone().into_iter().collect(),
        category: config.category_filter.clone(),
        ..Default::default()
    };

    let docs = corpus::build_corpus(&config.fixtures_dir, &filter)?;
    eprintln!(
        "Comparing {} documents across {} pipelines",
        docs.len(),
        config.pipelines.len()
    );

    let mut results = Vec::new();

    for doc in &docs {
        let gt_text = doc
            .ground_truth_text
            .as_ref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .unwrap_or_default();

        let gt_markdown = doc
            .ground_truth_markdown
            .as_ref()
            .and_then(|p| std::fs::read_to_string(p).ok());

        let mut pipeline_results = Vec::new();

        for &pipeline in &config.pipelines {
            let result = run_pipeline(pipeline, doc, &gt_text, gt_markdown.as_deref(), &config.fixtures_dir).await;

            if config.dump_outputs {
                let dump_dir = std::path::PathBuf::from("/tmp/xberg_compare");
                let _ = std::fs::create_dir_all(&dump_dir);
                let _ = std::fs::write(
                    dump_dir.join(format!("{}_{}.md", doc.name, pipeline.name())),
                    &result.content,
                );
            }

            pipeline_results.push(result);
        }

        results.push(DocResult {
            name: doc.name.clone(),
            file_type: doc.file_type.clone(),
            results: pipeline_results,
        });
    }

    Ok(results)
}

/// Print a formatted comparison table to stderr.
pub fn print_comparison_table(results: &[DocResult]) {
    if results.is_empty() {
        eprintln!("No results to display.");
        return;
    }

    let pipeline_names: Vec<&str> = results
        .first()
        .map(|r| r.results.iter().map(|pr| pr.pipeline.name()).collect())
        .unwrap_or_default();

    eprint!("{:<25}", "Document");
    for name in &pipeline_names {
        eprint!(
            " {:>10} {:>10} {:>8}",
            format!("{} SF1", name),
            format!("{} TF1", name),
            format!("{} ms", name),
        );
    }
    eprintln!();
    eprintln!("{}", "-".repeat(25 + pipeline_names.len() * 30));

    for doc in results {
        eprint!("{:<25}", doc.name);
        for pr in &doc.results {
            let time_str = if !pr.time_ms.is_finite() || pr.time_ms <= 0.0 {
                "---".to_string()
            } else {
                format!("{:.0}", pr.time_ms)
            };
            eprint!(
                " {:>10} {:>10} {:>8}",
                format_percentage(pr.sf1),
                format_percentage(pr.tf1),
                time_str
            );
        }
        eprintln!();
    }

    eprintln!("{}", "-".repeat(25 + pipeline_names.len() * 30));
    eprint!("{:<25}", "AVERAGE");
    for (i, _) in pipeline_names.iter().enumerate() {
        let sf1_vals: Vec<f64> = results
            .iter()
            .map(|r| r.results[i].sf1)
            .filter(|v| v.is_finite())
            .collect();
        let tf1_vals: Vec<f64> = results
            .iter()
            .map(|r| r.results[i].tf1)
            .filter(|v| v.is_finite())
            .collect();
        let avg_sf1 = if sf1_vals.is_empty() {
            f64::NAN
        } else {
            sf1_vals.iter().sum::<f64>() / sf1_vals.len() as f64
        };
        let avg_tf1 = if tf1_vals.is_empty() {
            f64::NAN
        } else {
            tf1_vals.iter().sum::<f64>() / tf1_vals.len() as f64
        };
        let times: Vec<f64> = results
            .iter()
            .map(|r| r.results[i].time_ms)
            .filter(|t| t.is_finite() && *t > 0.0)
            .collect();
        let avg_time = if times.is_empty() {
            f64::NAN
        } else {
            times.iter().sum::<f64>() / times.len() as f64
        };
        let time_str = if !avg_time.is_finite() {
            "---".to_string()
        } else {
            format!("{:.0}", avg_time)
        };
        eprint!(
            " {:>10} {:>10} {:>8}",
            format_percentage(avg_sf1),
            format_percentage(avg_tf1),
            time_str
        );
    }
    eprintln!();

    for (i, name) in pipeline_names.iter().enumerate() {
        let failed = results.iter().filter(|r| !r.results[i].tf1.is_finite()).count();
        if failed > 0 {
            eprintln!("  {}: {} timeouts/errors (excluded from averages)", name, failed);
        }
    }
}

fn format_percentage(value: f64) -> String {
    if value.is_finite() {
        format!("{:.1}%", value * 100.0)
    } else {
        "---".to_string()
    }
}

/// Print a per-format summary table to stderr, grouping documents by file_type.
pub fn print_per_format_summary(results: &[DocResult]) {
    if results.is_empty() {
        return;
    }

    let pipeline_names: Vec<&str> = results
        .first()
        .map(|r| r.results.iter().map(|pr| pr.pipeline.name()).collect())
        .unwrap_or_default();

    let mut by_format: std::collections::BTreeMap<&str, Vec<&DocResult>> = std::collections::BTreeMap::new();
    for doc in results {
        by_format.entry(&doc.file_type).or_default().push(doc);
    }

    eprintln!("\nPer-Format Summary:");

    eprint!("{:<12} {:>5}", "Format", "Count");
    for name in &pipeline_names {
        eprint!("  {:>10} {:>10}", format!("{} SF1", name), format!("{} TF1", name));
    }
    eprintln!();
    let line_width = 12 + 5 + pipeline_names.len() * 22;
    eprintln!("{}", "\u{2500}".repeat(line_width));

    for (format, docs) in &by_format {
        eprint!("{:<12} {:>5}", format, docs.len());
        for (i, _) in pipeline_names.iter().enumerate() {
            let sf1_vals: Vec<f64> = docs
                .iter()
                .map(|d| d.results[i].sf1)
                .filter(|v| v.is_finite())
                .collect();
            let tf1_vals: Vec<f64> = docs
                .iter()
                .map(|d| d.results[i].tf1)
                .filter(|v| v.is_finite())
                .collect();
            let avg_sf1 = if sf1_vals.is_empty() {
                f64::NAN
            } else {
                sf1_vals.iter().sum::<f64>() / sf1_vals.len() as f64
            };
            let avg_tf1 = if tf1_vals.is_empty() {
                f64::NAN
            } else {
                tf1_vals.iter().sum::<f64>() / tf1_vals.len() as f64
            };
            eprint!(
                "  {:>10} {:>10}",
                format_percentage(avg_sf1),
                format_percentage(avg_tf1)
            );
        }
        eprintln!();
    }
}

/// Per-format aggregation entry for JSON output.
#[derive(Debug, Clone, Serialize)]
struct FormatSummary {
    count: usize,
    pipelines: Vec<FormatPipelineSummary>,
}

/// Per-pipeline averages within a format group.
#[derive(Debug, Clone, Serialize)]
struct FormatPipelineSummary {
    pipeline: String,
    avg_sf1: f64,
    avg_tf1: f64,
    sf1_count: usize,
    tf1_count: usize,
}

/// Overall summary for JSON output.
#[derive(Debug, Clone, Serialize)]
struct OverallSummary {
    total_documents: usize,
    pipelines: Vec<FormatPipelineSummary>,
}

/// Top-level JSON output structure.
#[derive(Debug, Clone, Serialize)]
struct ComparisonJsonOutput {
    documents: Vec<DocResult>,
    by_format: std::collections::BTreeMap<String, FormatSummary>,
    overall: OverallSummary,
}

/// Write full comparison results (documents + per-format + overall) to a JSON file.
pub fn write_comparison_json(results: &[DocResult], path: &std::path::Path) -> Result<()> {
    let pipeline_names: Vec<String> = results
        .first()
        .map(|r| r.results.iter().map(|pr| pr.pipeline.name().to_string()).collect())
        .unwrap_or_default();

    let mut by_format_map: std::collections::BTreeMap<String, Vec<&DocResult>> = std::collections::BTreeMap::new();
    for doc in results {
        by_format_map.entry(doc.file_type.clone()).or_default().push(doc);
    }

    let by_format: std::collections::BTreeMap<String, FormatSummary> = by_format_map
        .iter()
        .map(|(format, docs)| {
            let pipelines = pipeline_names
                .iter()
                .enumerate()
                .map(|(i, name)| {
                    let sf1_vals: Vec<f64> = docs
                        .iter()
                        .map(|d| d.results[i].sf1)
                        .filter(|v| v.is_finite())
                        .collect();
                    let tf1_vals: Vec<f64> = docs
                        .iter()
                        .map(|d| d.results[i].tf1)
                        .filter(|v| v.is_finite())
                        .collect();
                    let avg_sf1 = if sf1_vals.is_empty() {
                        f64::NAN
                    } else {
                        sf1_vals.iter().sum::<f64>() / sf1_vals.len() as f64
                    };
                    let avg_tf1 = if tf1_vals.is_empty() {
                        f64::NAN
                    } else {
                        tf1_vals.iter().sum::<f64>() / tf1_vals.len() as f64
                    };
                    FormatPipelineSummary {
                        pipeline: name.clone(),
                        avg_sf1,
                        avg_tf1,
                        sf1_count: sf1_vals.len(),
                        tf1_count: tf1_vals.len(),
                    }
                })
                .collect();
            (
                format.clone(),
                FormatSummary {
                    count: docs.len(),
                    pipelines,
                },
            )
        })
        .collect();

    let overall_pipelines = pipeline_names
        .iter()
        .enumerate()
        .map(|(i, name)| {
            let sf1_vals: Vec<f64> = results
                .iter()
                .map(|r| r.results[i].sf1)
                .filter(|v| v.is_finite())
                .collect();
            let tf1_vals: Vec<f64> = results
                .iter()
                .map(|r| r.results[i].tf1)
                .filter(|v| v.is_finite())
                .collect();
            let avg_sf1 = if sf1_vals.is_empty() {
                f64::NAN
            } else {
                sf1_vals.iter().sum::<f64>() / sf1_vals.len() as f64
            };
            let avg_tf1 = if tf1_vals.is_empty() {
                f64::NAN
            } else {
                tf1_vals.iter().sum::<f64>() / tf1_vals.len() as f64
            };
            FormatPipelineSummary {
                pipeline: name.clone(),
                avg_sf1,
                avg_tf1,
                sf1_count: sf1_vals.len(),
                tf1_count: tf1_vals.len(),
            }
        })
        .collect();

    let output = ComparisonJsonOutput {
        documents: results.to_vec(),
        by_format,
        overall: OverallSummary {
            total_documents: results.len(),
            pipelines: overall_pipelines,
        },
    };

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(crate::Error::Io)?;
    }
    let json = serde_json::to_string_pretty(&output)
        .map_err(|e| crate::Error::Benchmark(format!("Failed to serialize comparison JSON: {}", e)))?;
    std::fs::write(path, json).map_err(crate::Error::Io)?;

    Ok(())
}

/// Data-driven guardrails configuration loaded from JSON.
#[derive(Debug, Serialize, Deserialize)]
pub struct GuardrailsConfig {
    pub version: String,
    pub generated_at: String,
    pub threshold_factor: f64,
    pub contracts: Vec<GuardrailContract>,
}

/// A single guardrail contract: minimum threshold for a specific document + pipeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GuardrailContract {
    pub doc: String,
    /// Fixture file type used to disambiguate identical document names.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_type: Option<String>,
    pub pipeline: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_sf1: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_tf1: Option<f64>,
    /// Exact text anchors that must occur in this relative order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub relative_order: Vec<String>,
}

const READING_ORDER_GUARDRAIL_DOC: &str = "681693";
const READING_ORDER_GUARDRAIL_PIPELINE: &str = "pdf-oxide+layout+reading-order";
const READING_ORDER_GUARDRAIL_ANCHORS: &[&str] = &[
    "maintainers wanted",
    "See #182",
    "MongoKit",
    "MongoDB is a great schema-less document oriented database",
    "Philosophy",
];

/// Load a guardrails configuration from a JSON file.
pub fn load_guardrails(path: &Path) -> Result<GuardrailsConfig> {
    let data = std::fs::read_to_string(path)
        .map_err(|e| crate::Error::Benchmark(format!("Failed to read guardrails file {}: {}", path.display(), e)))?;
    let mut config: GuardrailsConfig = serde_json::from_str(&data)
        .map_err(|e| crate::Error::Benchmark(format!("Failed to parse guardrails file: {}", e)))?;
    install_reading_order_guardrail(&mut config);
    validate_guardrails_config(&config)?;
    Ok(config)
}

fn validate_guardrails_config(config: &GuardrailsConfig) -> Result<()> {
    if !config.threshold_factor.is_finite() || !(0.0..=1.0).contains(&config.threshold_factor) {
        return Err(crate::Error::Config(
            "guardrail threshold_factor must be finite and within [0, 1]".to_string(),
        ));
    }

    let mut identities = std::collections::HashSet::new();
    for contract in &config.contracts {
        if contract.doc.trim().is_empty() {
            return Err(crate::Error::Config(
                "guardrail document name must not be empty".to_string(),
            ));
        }
        if Pipeline::parse(&contract.pipeline).is_none_or(|pipeline| pipeline.name() != contract.pipeline) {
            return Err(crate::Error::Config(format!(
                "unknown guardrail pipeline '{}'",
                contract.pipeline
            )));
        }
        if contract.min_sf1.is_none() && contract.min_tf1.is_none() && contract.relative_order.is_empty() {
            return Err(crate::Error::Config(format!(
                "guardrail {} [{}] {} has no threshold or relative-order predicate",
                contract.doc,
                contract.file_type.as_deref().unwrap_or("any"),
                contract.pipeline
            )));
        }
        for (name, value) in [("min_sf1", contract.min_sf1), ("min_tf1", contract.min_tf1)] {
            if value.is_some_and(|value| !value.is_finite() || !(0.0..=1.0).contains(&value)) {
                return Err(crate::Error::Config(format!(
                    "guardrail {} {} must be finite and within [0, 1]",
                    contract.doc, name
                )));
            }
        }
        if contract.relative_order.iter().any(|anchor| anchor.trim().is_empty()) {
            return Err(crate::Error::Config(format!(
                "guardrail {} relative-order anchors must not be empty",
                contract.doc
            )));
        }
        let identity = (
            contract.doc.as_str(),
            contract.file_type.as_deref(),
            contract.pipeline.as_str(),
        );
        if !identities.insert(identity) {
            return Err(crate::Error::Config(format!(
                "duplicate guardrail contract: {} [{}] {}",
                contract.doc,
                contract.file_type.as_deref().unwrap_or("any"),
                contract.pipeline
            )));
        }
    }
    Ok(())
}

fn reading_order_anchors(doc: &str, pipeline: &str) -> Vec<String> {
    if doc != READING_ORDER_GUARDRAIL_DOC || pipeline != READING_ORDER_GUARDRAIL_PIPELINE {
        return Vec::new();
    }
    READING_ORDER_GUARDRAIL_ANCHORS
        .iter()
        .map(|anchor| (*anchor).to_string())
        .collect()
}

fn install_reading_order_guardrail(config: &mut GuardrailsConfig) {
    if let Some(contract) = config.contracts.iter_mut().find(|contract| {
        contract.doc == READING_ORDER_GUARDRAIL_DOC
            && contract.pipeline == READING_ORDER_GUARDRAIL_PIPELINE
            && contract.file_type.as_deref().is_none_or(|file_type| file_type == "pdf")
    }) {
        contract.file_type.get_or_insert_with(|| "pdf".to_string());
        if contract.relative_order.is_empty() {
            contract.relative_order =
                reading_order_anchors(READING_ORDER_GUARDRAIL_DOC, READING_ORDER_GUARDRAIL_PIPELINE);
        }
        return;
    }
    config.contracts.push(GuardrailContract {
        doc: READING_ORDER_GUARDRAIL_DOC.to_string(),
        file_type: Some("pdf".to_string()),
        pipeline: READING_ORDER_GUARDRAIL_PIPELINE.to_string(),
        min_sf1: None,
        min_tf1: None,
        relative_order: reading_order_anchors(READING_ORDER_GUARDRAIL_DOC, READING_ORDER_GUARDRAIL_PIPELINE),
    });
}

/// Check guardrails from a loaded config, returning a list of failure messages (empty = all passed).
pub fn check_guardrails(results: &[DocResult], config: &GuardrailsConfig) -> Vec<String> {
    let mut failures = Vec::new();

    for contract in &config.contracts {
        let doc = match resolve_guardrail_document(results, contract) {
            Ok(Some(doc)) => doc,
            Ok(None) => {
                failures.push(format!(
                    "missing guardrail document: {} [{}] {}",
                    contract.doc,
                    contract.file_type.as_deref().unwrap_or("any"),
                    contract.pipeline
                ));
                continue;
            }
            Err(failure) => {
                failures.push(failure);
                continue;
            }
        };
        let Some(pr) = doc.results.iter().find(|pr| pr.pipeline.name() == contract.pipeline) else {
            failures.push(format!(
                "missing guardrail pipeline result: {} [{}] {}",
                contract.doc, doc.file_type, contract.pipeline
            ));
            continue;
        };
        let target = format!("{} [{}] {}", contract.doc, doc.file_type, contract.pipeline);

        failures.extend(check_guardrail_score("SF1", pr.sf1, contract.min_sf1, &target));
        failures.extend(check_guardrail_score("TF1", pr.tf1, contract.min_tf1, &target));

        if let Err(reason) = check_relative_order(&pr.content, &contract.relative_order) {
            failures.push(format!("relative-order regression: {target} {reason}"));
        }
    }

    failures
}

fn resolve_guardrail_document<'a>(
    results: &'a [DocResult],
    contract: &GuardrailContract,
) -> std::result::Result<Option<&'a DocResult>, String> {
    let matching_docs: Vec<&DocResult> = results
        .iter()
        .filter(|result| {
            result.name == contract.doc
                && contract
                    .file_type
                    .as_deref()
                    .is_none_or(|file_type| result.file_type == file_type)
        })
        .collect();
    if matching_docs.len() <= 1 {
        return Ok(matching_docs.first().copied());
    }

    let mut file_types: Vec<&str> = matching_docs.iter().map(|doc| doc.file_type.as_str()).collect();
    file_types.sort_unstable();
    file_types.dedup();
    Err(format!(
        "ambiguous legacy guardrail target: {} {} matches file types {}",
        contract.doc,
        contract.pipeline,
        file_types.join(", ")
    ))
}

fn check_guardrail_score(metric: &str, value: f64, minimum: Option<f64>, target: &str) -> Option<String> {
    let minimum = minimum?;
    if !value.is_finite() {
        return Some(format!("{metric} unavailable: {target} has no finite score"));
    }
    (value < minimum).then(|| {
        format!(
            "{metric} regression: {target} {metric} {:.1}% < minimum {:.1}%",
            value * 100.0,
            minimum * 100.0
        )
    })
}

fn check_relative_order(content: &str, anchors: &[String]) -> std::result::Result<(), String> {
    let normalized_content = normalize_order_text(content);
    let mut remainder = normalized_content.as_str();
    for anchor in anchors {
        if anchor.is_empty() {
            return Err("contains an empty anchor".to_string());
        }
        let normalized_anchor = normalize_order_text(anchor);
        let Some(position) = remainder.find(&normalized_anchor) else {
            return Err(format!("missing or out-of-order anchor {anchor:?}"));
        };
        remainder = &remainder[position + normalized_anchor.len()..];
    }
    Ok(())
}

fn normalize_order_text(value: &str) -> String {
    let mut normalized = String::with_capacity(value.len());
    let mut characters = value.chars().peekable();
    while let Some(character) = characters.next() {
        if character == '\\'
            && let Some(escaped) = characters.peek()
            && escaped.is_ascii_punctuation()
        {
            normalized.push(characters.next().expect("peeked escaped punctuation"));
            continue;
        }
        match character {
            '\u{00a0}' => normalized.push(' '),
            '\u{2010}' | '\u{2011}' | '\u{2012}' | '\u{2013}' | '\u{2212}' => normalized.push('-'),
            _ => normalized.push(character),
        }
    }
    normalized
}

/// Run comparison with guardrails and return exit code (0 = pass, 1 = fail).
pub async fn run_with_guardrails(config: &ComparisonConfig) -> Result<i32> {
    let results = run_comparison(config).await?;
    print_comparison_table(&results);
    print_per_format_summary(&results);

    if let Some(ref json_path) = config.json_output {
        write_comparison_json(&results, json_path)?;
        eprintln!("\nComparison JSON written to: {}", json_path.display());
    }

    if config.guardrails {
        let guardrails_path = config
            .guardrails_file
            .as_deref()
            .unwrap_or_else(|| Path::new("guardrails.json"));
        let mut guardrails_config = load_guardrails(guardrails_path)?;
        guardrails_config.contracts.retain(|contract| {
            let pipeline_selected = config
                .pipelines
                .iter()
                .any(|pipeline| pipeline.name() == contract.pipeline);
            let document_selected = config
                .name_filter
                .as_deref()
                .is_none_or(|filter| contract.doc.contains(filter));
            pipeline_selected && document_selected
        });
        eprintln!(
            "Loaded {} guardrail contracts from {} (v{}, factor {})",
            guardrails_config.contracts.len(),
            guardrails_path.display(),
            guardrails_config.version,
            guardrails_config.threshold_factor,
        );
        if guardrails_config.contracts.is_empty() {
            eprintln!("\nGUARDRAIL FAILURE: no contracts apply to the selected pipelines and document filter");
            return Ok(1);
        }
        let failures = check_guardrails(&results, &guardrails_config);
        if !failures.is_empty() {
            eprintln!("\nGUARDRAIL FAILURES:");
            for f in &failures {
                eprintln!("  {}", f);
            }
            return Ok(1);
        }
        eprintln!("\nAll guardrails passed.");
    }

    if config.noise {
        print_noise_summary(&results);
    }

    if config.diagnose {
        run_diagnostics(config, &results)?;
    }

    Ok(0)
}

/// Run diagnostic diff mode on documents with SF1 below the threshold.
fn run_diagnostics(config: &ComparisonConfig, results: &[DocResult]) -> Result<()> {
    use crate::diagnostics::{diagnose_document, write_diagnostic_files};

    let filter = CorpusFilter {
        file_types: None,
        require_ground_truth: true,
        require_markdown_ground_truth: true,
        name_patterns: config.name_filter.clone().into_iter().collect(),
        category: config.category_filter.clone(),
        ..Default::default()
    };
    let docs = corpus::build_corpus(&config.fixtures_dir, &filter)?;
    let doc_map: HashMap<String, &CorpusDocument> = docs.iter().map(|d| (d.name.clone(), d)).collect();

    let mut diagnosed_count = 0;

    for doc_result in results {
        let corpus_doc = match doc_map.get(&doc_result.name) {
            Some(d) => d,
            None => continue,
        };

        let gt_text = corpus_doc
            .ground_truth_text
            .as_ref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .unwrap_or_default();

        let gt_markdown = corpus_doc
            .ground_truth_markdown
            .as_ref()
            .and_then(|p| std::fs::read_to_string(p).ok());

        for pr in &doc_result.results {
            if pr.sf1 < config.diagnose_threshold {
                let diag = diagnose_document(
                    &doc_result.name,
                    &doc_result.file_type,
                    pr.pipeline.name(),
                    &pr.content,
                    &gt_text,
                    gt_markdown.as_deref(),
                );

                if let Err(e) = write_diagnostic_files(&diag, gt_markdown.as_deref(), &pr.content) {
                    eprintln!(
                        "  Warning: failed to write diagnostics for {}/{}: {}",
                        doc_result.name,
                        pr.pipeline.name(),
                        e
                    );
                }

                diagnosed_count += 1;
            }
        }
    }

    if diagnosed_count > 0 {
        eprintln!(
            "\nDiagnosed {} document(s) with SF1 < {:.0}% -> /tmp/xberg_diagnose/",
            diagnosed_count,
            config.diagnose_threshold * 100.0
        );
    } else {
        eprintln!(
            "\nNo documents below SF1 threshold ({:.0}%) — no diagnostics generated.",
            config.diagnose_threshold * 100.0
        );
    }

    Ok(())
}

/// Run noise detection on all pipeline outputs and print summary.
fn print_noise_summary(results: &[DocResult]) {
    use crate::noise_detection::{Severity, detect_noise};
    use std::collections::HashMap;

    eprintln!("\n{:=<70}", "");
    eprintln!("NOISE DETECTION SUMMARY");
    eprintln!("{:=<70}", "");

    let mut total_docs_with_noise = 0;
    let mut total_issues = 0;
    let mut kind_counts: HashMap<String, usize> = HashMap::new();
    let mut noisy_docs: Vec<(String, String, usize, usize, usize)> = Vec::new();

    for doc_result in results {
        for pr in &doc_result.results {
            if pr.content.is_empty() {
                continue;
            }
            let report = detect_noise(&pr.content);
            if report.issues.is_empty() {
                continue;
            }
            total_docs_with_noise += 1;
            total_issues += report.issues.len();
            for issue in &report.issues {
                *kind_counts.entry(format!("{:?}", issue.kind)).or_insert(0) += 1;
            }
            let errors = report.issues.iter().filter(|i| i.severity == Severity::Error).count();
            let warnings = report.issues.iter().filter(|i| i.severity == Severity::Warning).count();
            let infos = report.issues.iter().filter(|i| i.severity == Severity::Info).count();
            noisy_docs.push((
                doc_result.name.clone(),
                pr.pipeline.name().to_string(),
                errors,
                warnings,
                infos,
            ));
        }
    }

    if total_docs_with_noise == 0 {
        eprintln!("No noise detected in any extracted output.");
        return;
    }

    eprintln!(
        "{} documents with noise issues ({} total issues)",
        total_docs_with_noise, total_issues
    );

    eprintln!("\nBy kind:");
    let mut sorted_kinds: Vec<_> = kind_counts.into_iter().collect();
    sorted_kinds.sort_by_key(|b| std::cmp::Reverse(b.1));
    for (kind, count) in &sorted_kinds {
        eprintln!("  {:<30} {}", kind, count);
    }

    noisy_docs.sort_by(|a, b| b.2.cmp(&a.2).then(b.3.cmp(&a.3)));
    let show = noisy_docs.len().min(20);
    eprintln!("\nTop {} noisy documents:", show);
    eprintln!(
        "  {:<30} {:<15} {:>6} {:>6} {:>6}",
        "Document", "Pipeline", "Errors", "Warns", "Infos"
    );
    for (doc, pipeline, errors, warnings, infos) in noisy_docs.iter().take(show) {
        eprintln!(
            "  {:<30} {:<15} {:>6} {:>6} {:>6}",
            doc, pipeline, errors, warnings, infos
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forced_ocr_uses_inference_appropriate_timeout() {
        let path = Path::new("fixture.png");

        assert_eq!(
            compute_extraction_timeout_secs(path, "png", false),
            EXTRACTION_TIMEOUT_BASE_SECS
        );
        assert_eq!(
            compute_extraction_timeout_secs(path, "png", true),
            FORCED_OCR_TIMEOUT_BASE_SECS
        );
    }

    #[test]
    fn test_score_document_identical() {
        let text = "Hello world this is a test document";
        let (tf1, structural) = score_document(text, text, None);
        assert!(
            (tf1 - 1.0).abs() < f64::EPSILON,
            "TF1 should be 1.0 for identical text, got {tf1}"
        );
        assert!(structural.sf1.is_nan(), "SF1 should be unavailable when no markdown GT");
        assert!(
            structural.order_score.is_nan(),
            "order_score should be unavailable when no markdown GT"
        );
        assert!(
            structural.per_type_sf1.is_empty(),
            "per_type should be empty when no markdown GT"
        );
    }

    #[test]
    fn test_score_document_no_markdown_gt() {
        let content = "Some extracted content here";
        let gt_text = "Some ground truth content here";
        let (tf1, structural) = score_document(content, gt_text, None);
        assert!(
            tf1 > 0.0 && tf1 < 1.0,
            "TF1 should be between 0 and 1 for partially matching text, got {tf1}"
        );
        assert!(structural.sf1.is_nan(), "SF1 should be unavailable when no markdown GT");
        assert!(structural.order_score.is_nan());
        assert!(structural.per_type_sf1.is_empty());
    }

    #[test]
    fn test_score_document_empty() {
        let (tf1, structural) = score_document("", "", None);
        let _ = tf1;
        assert!(structural.sf1.is_nan());
        assert!(structural.order_score.is_nan());
        assert!(structural.per_type_sf1.is_empty());
    }

    #[test]
    fn test_score_document_completely_different() {
        let content = "alpha bravo charlie";
        let gt_text = "delta echo foxtrot";
        let (tf1, _structural) = score_document(content, gt_text, None);
        assert!(
            (tf1 - 0.0).abs() < f64::EPSILON,
            "TF1 should be 0.0 for completely different text, got {tf1}"
        );
    }

    #[test]
    fn test_score_document_with_structure() {
        let content = "# Heading\n\nSome paragraph text.\n";
        let gt_markdown = "# Heading\n\nSome paragraph text.\n";
        let gt_text = "Heading Some paragraph text.";
        let (tf1, structural) = score_document(content, gt_text, Some(gt_markdown));
        assert!(tf1 > 0.0, "TF1 should be positive, got {tf1}");
        assert!(
            structural.sf1 > 0.0,
            "SF1 should be positive when structural GT is provided, got {}",
            structural.sf1
        );
        assert_eq!(structural.per_type_sf1.len(), structural.per_type_precision.len());
        assert_eq!(structural.per_type_sf1.len(), structural.per_type_recall.len());
    }

    #[test]
    fn test_pipeline_config_deterministic() {
        for pipeline in Pipeline::all_xberg() {
            let config = build_extraction_config(pipeline);
            assert_eq!(
                format!("{:?}", config.output_format),
                format!("{:?}", xberg::core::config::OutputFormat::Markdown),
                "Pipeline {:?} should produce Markdown config",
                pipeline
            );
        }
    }

    #[test]
    fn timed_xberg_pipeline_configs_disable_extraction_cache() {
        for pipeline in Pipeline::all_xberg() {
            let config = build_extraction_config(pipeline);

            assert!(
                !config.use_cache,
                "timed pipeline {} must measure extraction rather than a cache hit",
                pipeline.name()
            );

            let Some(ocr) = config.ocr.as_ref() else {
                continue;
            };
            // A `tesseract_config` that already existed (explicit PSM presets, e.g.
            // that leaves `tesseract_config` implicit (auto-PSM) must keep it absent — xberg only
            // auto-selects PSM when `ocr.tesseract_config` is absent, so materializing one here
            // to force `use_cache = false` would silently pin PSM to the default (3).
            if let Some(ocr_pipeline) = ocr.pipeline.as_ref() {
                for stage in &ocr_pipeline.stages {
                    if stage.backend == "tesseract"
                        && let Some(tesseract_config) = stage.tesseract_config.as_ref()
                    {
                        assert!(
                            !tesseract_config.use_cache,
                            "timed Tesseract stage in {} must disable its result cache",
                            pipeline.name()
                        );
                    }
                }
            } else if ocr.backend == "tesseract"
                && let Some(tesseract_config) = ocr.tesseract_config.as_ref()
            {
                assert!(
                    !tesseract_config.use_cache,
                    "timed pipeline {} must disable the Tesseract result cache",
                    pipeline.name()
                );
            }
        }
    }

    #[test]
    fn finalize_disables_cache_and_materializes_correct_psm_for_implicit_tesseract_stages() {
        let ocr = serde_json::from_value(serde_json::json!({
            "pipeline": {
                "stages": [
                    { "backend": "tesseract", "tesseract_config": { "psm": 6 } },
                    { "backend": "tesseract" },
                    { "backend": "paddleocr" }
                ]
            }
        }))
        .unwrap();
        let mut config = xberg::ExtractionConfig {
            ocr: Some(ocr),
            ..Default::default()
        };

        finalize_timed_ocr_result_cache(&mut config);

        let stages = &config.ocr.unwrap().pipeline.unwrap().stages;
        let explicit = stages[0].tesseract_config.as_ref().unwrap();
        assert!(
            !explicit.use_cache,
            "an already-configured stage must disable its cache"
        );
        assert_eq!(explicit.psm, 6, "disabling the cache must not touch an explicit PSM");

        let implicit = stages[1]
            .tesseract_config
            .as_ref()
            .expect("an implicit stage must be materialized so its result cache is genuinely disabled");
        assert!(!implicit.use_cache);
        assert_eq!(
            implicit.psm,
            crate::adapter::XBERG_WHOLE_IMAGE_TESSERACT_PSM,
            "an implicit stage's materialized PSM must match xberg's own auto-selection for the \
             default language, or it would silently regress to PSM 3"
        );
        assert_eq!(implicit.language, ["eng"]);

        assert!(stages[2].tesseract_config.is_none());
    }

    #[test]
    fn finalize_materializes_whole_image_psm_for_implicit_ocr_with_default_language() {
        let mut config = xberg::ExtractionConfig::default();
        materialize_implicit_ocr_config(&mut config);

        finalize_timed_ocr_result_cache(&mut config);

        let ocr = config.ocr.expect("materialize_implicit_ocr_config must configure OCR");
        assert_eq!(ocr.backend, "tesseract");
        assert_eq!(
            ocr.language,
            ["eng"],
            "the parent OcrConfig.language must also read 'eng'"
        );
        let tesseract = ocr
            .tesseract_config
            .expect("implicit timed OCR must materialize a tesseract_config to genuinely disable its cache");
        assert!(!tesseract.use_cache);
        assert_eq!(
            tesseract.psm,
            crate::adapter::XBERG_WHOLE_IMAGE_TESSERACT_PSM,
            "materialized PSM must match xberg's own auto-selection, or it would silently \
             regress to PSM 3"
        );
        assert_eq!(
            tesseract.language,
            ["eng"],
            "language must land on both ocr.language and the materialized tesseract_config.language"
        );
    }

    #[test]
    fn finalize_preserves_explicit_bare_tesseract_psm() {
        let ocr = xberg::OcrConfig {
            backend: "tesseract".to_string(),
            language: vec!["eng".to_string()],
            tesseract_config: Some(xberg::TesseractConfig {
                psm: 6,
                use_cache: true,
                ..Default::default()
            }),
            ..Default::default()
        };
        let mut config = xberg::ExtractionConfig {
            ocr: Some(ocr),
            ..Default::default()
        };

        finalize_timed_ocr_result_cache(&mut config);

        let tesseract = config.ocr.unwrap().tesseract_config.unwrap();
        assert!(!tesseract.use_cache);
        assert_eq!(tesseract.psm, 6, "disabling the cache must not touch an explicit PSM");
        assert_eq!(tesseract.language, ["eng"], "language must still be refreshed");
    }

    #[test]
    fn paddle_v6_presets_pin_model_tier_and_layout() {
        let presets = [
            (Pipeline::Paddle, "medium", false),
            (Pipeline::PaddleLayout, "medium", true),
            (Pipeline::PaddleV6Small, "small", false),
            (Pipeline::PaddleV6SmallLayout, "small", true),
            (Pipeline::PaddleV6Tiny, "tiny", false),
            (Pipeline::PaddleV6TinyLayout, "tiny", true),
        ];

        for (pipeline, expected_tier, expected_layout) in presets {
            let config = build_extraction_config(pipeline);
            let ocr = config.ocr.as_ref().expect("Paddle preset must configure OCR");
            let paddle = ocr
                .paddle_ocr_config
                .as_ref()
                .expect("Paddle preset must pin model identity");

            assert_eq!(ocr.backend, "paddleocr", "unexpected backend for {}", pipeline.name());
            assert!(!ocr.auto_rotate, "canonical Paddle preset must use the public default");
            assert_eq!(
                paddle["model_version"],
                PP_OCR_V6,
                "unexpected version for {}",
                pipeline.name()
            );
            assert_eq!(
                paddle["model_tier"],
                expected_tier,
                "unexpected tier for {}",
                pipeline.name()
            );
            assert_eq!(
                config.layout.is_some(),
                expected_layout,
                "unexpected layout for {}",
                pipeline.name()
            );
        }
    }

    #[test]
    fn paddle_quality_sweep_presets_pin_every_swept_dimension() {
        let cases = [
            (Pipeline::PaddleV6SmallLayoutDetSide1024, PADDLE_DEFAULT_QUALITY_PROFILE),
            (
                Pipeline::PaddleV6SmallLayoutDetSide1536,
                PADDLE_DET_SIDE_1536_QUALITY_PROFILE,
            ),
            (
                Pipeline::PaddleV6SmallLayoutDetSide2048,
                PADDLE_DET_SIDE_2048_QUALITY_PROFILE,
            ),
            (
                Pipeline::PaddleV6SmallLayoutDetDbThresh020,
                PADDLE_DET_DB_THRESH_020_QUALITY_PROFILE,
            ),
            (
                Pipeline::PaddleV6SmallLayoutDetDbBoxThresh035,
                PADDLE_DET_DB_BOX_THRESH_035_QUALITY_PROFILE,
            ),
            (
                Pipeline::PaddleV6SmallLayoutDropScore030,
                PADDLE_DROP_SCORE_030_QUALITY_PROFILE,
            ),
            (
                Pipeline::PaddleV6SmallLayoutDropScore040,
                PADDLE_DROP_SCORE_040_QUALITY_PROFILE,
            ),
        ];

        for (pipeline, expected) in cases {
            let config = build_extraction_config(pipeline);
            let ocr = config.ocr.expect("quality preset must configure OCR");
            let paddle = ocr.paddle_ocr_config.expect("quality preset must pin Paddle settings");

            assert!(config.force_ocr);
            assert!(config.layout.is_some());
            assert_eq!(ocr.backend, "paddleocr");
            assert_eq!(paddle["model_version"], PP_OCR_V6);
            assert_eq!(paddle["model_tier"], "small");
            assert_eq!(paddle["det_limit_side_len"], expected.det_limit_side_len);
            assert_eq!(paddle["det_db_thresh"], expected.det_db_thresh);
            assert_eq!(paddle["det_db_box_thresh"], expected.det_db_box_thresh);
            assert_eq!(paddle["drop_score"], expected.drop_score);
        }
    }

    #[test]
    fn paddle_rotation_presets_differ_only_by_auto_rotate() {
        let auto_rotate = build_extraction_config(Pipeline::PaddleAutoRotate);
        let no_rotate = build_extraction_config(Pipeline::PaddleNoRotate);
        let auto_ocr = auto_rotate.ocr.expect("auto-rotate preset must configure OCR");
        let no_rotate_ocr = no_rotate.ocr.expect("no-rotate preset must configure OCR");

        assert!(auto_ocr.auto_rotate);
        assert!(!no_rotate_ocr.auto_rotate);
        assert_eq!(auto_ocr.backend, no_rotate_ocr.backend);
        assert_eq!(auto_ocr.paddle_ocr_config, no_rotate_ocr.paddle_ocr_config);
    }

    #[test]
    fn paddle_quality_sweep_presets_are_opt_in() {
        let defaults = Pipeline::all_xberg();
        for pipeline in [
            Pipeline::PaddleV6SmallLayoutDetSide1024,
            Pipeline::PaddleV6SmallLayoutDetSide1536,
            Pipeline::PaddleV6SmallLayoutDetSide2048,
            Pipeline::PaddleV6SmallLayoutDetDbThresh020,
            Pipeline::PaddleV6SmallLayoutDetDbBoxThresh035,
            Pipeline::PaddleV6SmallLayoutDropScore030,
            Pipeline::PaddleV6SmallLayoutDropScore040,
        ] {
            assert!(!defaults.contains(&pipeline), "{} must remain opt-in", pipeline.name());
            assert_eq!(
                serde_json::to_value(pipeline).expect("quality preset must serialize"),
                serde_json::Value::String(pipeline.name().to_string()),
                "{} must preserve its result identity when serialized",
                pipeline.name()
            );
        }
    }

    #[test]
    fn legacy_paddle_server_presets_pin_v5_server_models() {
        for pipeline in [Pipeline::PaddleServer, Pipeline::PaddleServerLayout] {
            let config = build_extraction_config(pipeline);
            let ocr = config.ocr.as_ref().expect("legacy server preset must configure OCR");
            let paddle = ocr
                .paddle_ocr_config
                .as_ref()
                .expect("legacy server preset must pin model identity");

            assert!(!ocr.auto_rotate, "canonical Paddle preset must use the public default");
            assert_eq!(paddle["model_version"], PP_OCR_V5);
            assert_eq!(paddle["model_tier"], "server");
        }
    }

    #[test]
    fn fixture_language_applies_to_materialized_implicit_ocr_without_forcing_it() {
        let mut metadata = HashMap::new();
        metadata.insert("ocr_language".to_string(), serde_json::json!(" deu + eng "));
        let doc = CorpusDocument {
            name: "multilingual".to_string(),
            document_path: std::path::PathBuf::new(),
            file_type: "png".to_string(),
            file_size: 0,
            ground_truth_text: None,
            ground_truth_markdown: None,
            metadata,
            fixture_path: std::path::PathBuf::new(),
        };

        for pipeline in [Pipeline::Baseline, Pipeline::PdfOxide] {
            let mut config = build_extraction_config(pipeline);
            materialize_implicit_ocr_config(&mut config);
            apply_fixture_ocr_language(&mut config, &doc);
            finalize_timed_ocr_result_cache(&mut config);

            let ocr = config.ocr.expect("timed extraction must materialize fallback OCR");
            assert_eq!(ocr.language, ["deu", "eng"]);
            assert_eq!(ocr.backend, "tesseract");
            // `tesseract_config` must be materialized (only) once the fixture's language is
            // final, so its PSM matches what xberg's own auto-selection would have picked for
            let tesseract = ocr
                .tesseract_config
                .expect("finalize_timed_ocr_result_cache must materialize a tesseract_config");
            assert!(!tesseract.use_cache);
            assert_eq!(tesseract.psm, crate::adapter::XBERG_WHOLE_IMAGE_TESSERACT_PSM);
            assert_eq!(tesseract.language, ["deu", "eng"]);
            assert!(!config.force_ocr, "{} must retain fallback-only OCR", pipeline.name());
        }
    }

    #[test]
    fn fixture_language_materializes_vertical_psm_for_implicit_tesseract_pipeline() {
        let mut metadata = HashMap::new();
        metadata.insert("ocr_language".to_string(), serde_json::json!("jpn_vert"));
        let doc = CorpusDocument {
            name: "vertical-japanese".to_string(),
            document_path: std::path::PathBuf::new(),
            file_type: "jpeg".to_string(),
            file_size: 0,
            ground_truth_text: None,
            ground_truth_markdown: None,
            metadata,
            fixture_path: std::path::PathBuf::new(),
        };
        let mut config = build_extraction_config(Pipeline::TesseractLayout);

        materialize_implicit_ocr_config(&mut config);
        apply_fixture_ocr_language(&mut config, &doc);
        finalize_timed_ocr_result_cache(&mut config);

        let ocr = config.ocr.expect("Tesseract pipeline must configure OCR");
        assert_eq!(ocr.language, ["jpn_vert"]);
        // the timed result cache must materialize one with the vertical-language PSM 5 xberg
        // selects for `*_vert` languages, not the default PSM 3.
        let tesseract = ocr
            .tesseract_config
            .expect("finalize_timed_ocr_result_cache must materialize a tesseract_config");
        assert!(!tesseract.use_cache);
        assert_eq!(tesseract.psm, crate::adapter::XBERG_VERTICAL_BLOCK_TESSERACT_PSM);
        assert_eq!(tesseract.language, ["jpn_vert"]);
    }

    #[test]
    fn fixture_language_refreshes_stale_implicit_stage_language_before_finalizing_psm() {
        // BLOCKER 2 regression: a Tesseract pipeline stage that already pins its own `language`
        // must NOT stay stale after `apply_fixture_ocr_language` runs. If it did,
        // `finalize_timed_ocr_result_cache` would prefer that stale per-stage language over the
        // fixture's real one (matching xberg's own "stage language wins over parent" semantics)
        // and materialize the wrong PSM.
        let mut metadata = HashMap::new();
        metadata.insert("ocr_language".to_string(), serde_json::json!("jpn_vert"));
        let doc = CorpusDocument {
            name: "vertical-japanese".to_string(),
            document_path: std::path::PathBuf::new(),
            file_type: "jpeg".to_string(),
            file_size: 0,
            ground_truth_text: None,
            ground_truth_markdown: None,
            metadata,
            fixture_path: std::path::PathBuf::new(),
        };
        let ocr = serde_json::from_value(serde_json::json!({
            "enabled": true,
            "pipeline": {
                "stages": [
                    { "backend": "tesseract", "language": ["eng"] }
                ]
            }
        }))
        .unwrap();
        let mut config = xberg::ExtractionConfig {
            ocr: Some(ocr),
            ..Default::default()
        };

        materialize_implicit_ocr_config(&mut config);
        apply_fixture_ocr_language(&mut config, &doc);
        finalize_timed_ocr_result_cache(&mut config);

        let stages = config.ocr.unwrap().pipeline.unwrap().stages;
        assert_eq!(
            stages[0].language,
            Some(vec!["jpn_vert".to_string()]),
            "the stale per-stage language override must be refreshed to the fixture's language"
        );
        let tesseract = stages[0]
            .tesseract_config
            .as_ref()
            .expect("finalize_timed_ocr_result_cache must materialize a tesseract_config");
        assert!(!tesseract.use_cache);
        assert_eq!(
            tesseract.psm,
            crate::adapter::XBERG_VERTICAL_BLOCK_TESSERACT_PSM,
            "must materialize PSM 5 for jpn_vert, not PSM 11 from the stale stage language"
        );
        assert_eq!(tesseract.language, ["jpn_vert"]);
    }

    #[test]
    fn fixture_language_preserves_explicit_paddle_backend_and_model_tier() {
        let mut metadata = HashMap::new();
        metadata.insert("ocr_language".to_string(), serde_json::json!(" deu + eng "));
        let doc = CorpusDocument {
            name: "multilingual".to_string(),
            document_path: std::path::PathBuf::new(),
            file_type: "png".to_string(),
            file_size: 0,
            ground_truth_text: None,
            ground_truth_markdown: None,
            metadata,
            fixture_path: std::path::PathBuf::new(),
        };
        let mut config = build_extraction_config(Pipeline::PaddleV6SmallLayout);

        materialize_implicit_ocr_config(&mut config);
        apply_fixture_ocr_language(&mut config, &doc);
        finalize_timed_ocr_result_cache(&mut config);

        let ocr = config.ocr.expect("Paddle preset must configure OCR");
        assert_eq!(ocr.language, ["deu", "eng"]);
        assert_eq!(ocr.backend, "paddleocr");
        assert_eq!(ocr.paddle_ocr_config.expect("model identity")["model_tier"], "small");
    }

    fn create_vendored_fixture(root: &Path, content: &str) -> (std::path::PathBuf, std::path::PathBuf) {
        let fixtures_dir = root.join("fixtures");
        let fixture_path = fixtures_dir.join("example.json");
        let vendored_dir = root.join("vendored").join("docling");
        std::fs::create_dir_all(vendored_dir.join("md")).unwrap();
        std::fs::create_dir_all(vendored_dir.join("timing")).unwrap();
        std::fs::create_dir_all(&fixtures_dir).unwrap();
        std::fs::write(&fixture_path, "{}").unwrap();
        std::fs::write(vendored_dir.join("md/example.md"), content).unwrap();
        std::fs::write(vendored_dir.join("timing/example.ms"), "12.5").unwrap();
        (fixtures_dir, fixture_path)
    }

    #[test]
    fn read_vendored_cached_resolves_fixture_directory() {
        let temp = tempfile::tempdir().unwrap();
        let (fixtures_dir, _) = create_vendored_fixture(temp.path(), "cached markdown");

        let (content, time_ms) = read_vendored_cached("example", &fixtures_dir, "docling").unwrap();

        assert_eq!(content, "cached markdown");
        assert_eq!(time_ms, 12.5);
    }

    #[tokio::test]
    async fn comparison_includes_text_only_ground_truth() {
        let temp = tempfile::tempdir().unwrap();
        let (fixtures_dir, fixture_path) = create_vendored_fixture(temp.path(), "ground truth text");
        std::fs::write(fixtures_dir.join("document.txt"), "source document").unwrap();
        std::fs::write(fixtures_dir.join("ground-truth.txt"), "ground truth text").unwrap();
        std::fs::write(
            fixture_path,
            serde_json::json!({
                "document": "document.txt",
                "file_type": "txt",
                "file_size": 15,
                "ground_truth": {
                    "text_file": "ground-truth.txt",
                    "source": "manual"
                }
            })
            .to_string(),
        )
        .unwrap();
        let config = ComparisonConfig {
            fixtures_dir,
            pipelines: vec![Pipeline::Docling],
            dump_outputs: false,
            guardrails: false,
            guardrails_file: None,
            name_filter: None,
            category_filter: None,
            json_output: None,
            noise: false,
            diagnose: false,
            diagnose_threshold: 0.0,
        };

        let results = run_comparison(&config).await.unwrap();

        assert_eq!(results.len(), 1);
        assert!((results[0].results[0].tf1 - 1.0).abs() < f64::EPSILON);
        assert!(results[0].results[0].sf1.is_nan());
        assert!(results[0].results[0].order_score.is_nan());
    }

    #[test]
    fn read_vendored_cached_resolves_fixture_file() {
        let temp = tempfile::tempdir().unwrap();
        let (_, fixture_path) = create_vendored_fixture(temp.path(), "cached markdown");

        let (content, time_ms) = read_vendored_cached("example", &fixture_path, "docling").unwrap();

        assert_eq!(content, "cached markdown");
        assert_eq!(time_ms, 12.5);
    }

    #[test]
    fn read_vendored_cached_ignores_unrelated_nested_vendored_dir() {
        let temp = tempfile::tempdir().unwrap();
        let _fixture = create_vendored_fixture(temp.path(), "cached markdown");
        let nested_fixtures = temp.path().join("fixtures/nested");
        let nested_fixture = nested_fixtures.join("example.json");
        std::fs::create_dir_all(nested_fixtures.join("vendored/other-framework")).unwrap();
        std::fs::write(&nested_fixture, "{}").unwrap();

        for fixtures_path in [&nested_fixtures, &nested_fixture] {
            let (content, time_ms) = read_vendored_cached("example", fixtures_path, "docling").unwrap();
            assert_eq!(content, "cached markdown");
            assert_eq!(time_ms, 12.5);
        }
    }

    #[test]
    fn read_vendored_cached_rejects_missing_markdown() {
        let temp = tempfile::tempdir().unwrap();
        let fixtures_dir = temp.path().join("fixtures");
        std::fs::create_dir_all(temp.path().join("vendored/docling/md")).unwrap();
        std::fs::create_dir_all(&fixtures_dir).unwrap();

        let error = read_vendored_cached("missing", &fixtures_dir, "docling").unwrap_err();

        assert!(
            error
                .to_string()
                .contains("Failed to read vendored docling output for missing")
        );
    }

    #[test]
    fn read_vendored_cached_rejects_empty_markdown() {
        let temp = tempfile::tempdir().unwrap();
        let (fixtures_dir, _) = create_vendored_fixture(temp.path(), "  \n");

        let error = read_vendored_cached("example", &fixtures_dir, "docling").unwrap_err();

        assert!(
            error
                .to_string()
                .contains("Vendored docling output for example is empty")
        );
    }

    #[tokio::test]
    async fn vendored_cache_miss_produces_nan_pipeline_scores() {
        let temp = tempfile::tempdir().unwrap();
        let fixtures_dir = temp.path().join("fixtures");
        std::fs::create_dir_all(temp.path().join("vendored/docling/md")).unwrap();
        std::fs::create_dir_all(&fixtures_dir).unwrap();
        let doc = CorpusDocument {
            name: "missing".to_string(),
            document_path: temp.path().join("missing.pdf"),
            file_type: "pdf".to_string(),
            file_size: 0,
            ground_truth_text: None,
            ground_truth_markdown: None,
            metadata: HashMap::new(),
            fixture_path: fixtures_dir.join("missing.json"),
        };

        let result = run_pipeline(
            Pipeline::Docling,
            &doc,
            "ground truth",
            Some("# Ground truth"),
            &fixtures_dir,
        )
        .await;

        assert!(result.sf1.is_nan());
        assert!(result.tf1.is_nan());
        assert!(result.order_score.is_nan());
        assert!(result.time_ms.is_nan());
    }

    #[test]
    fn guardrails_fail_when_contracted_scores_are_unavailable() {
        let results = vec![DocResult {
            name: "missing".to_string(),
            file_type: "pdf".to_string(),
            results: vec![PipelineResult {
                pipeline: Pipeline::Docling,
                sf1: f64::NAN,
                tf1: f64::NAN,
                char_similarity: f64::NAN,
                order_score: f64::NAN,
                per_type_sf1: HashMap::new(),
                per_type_precision: HashMap::new(),
                per_type_recall: HashMap::new(),
                time_ms: f64::NAN,
                missing_tokens: Vec::new(),
                extra_tokens: Vec::new(),
                content: String::new(),
            }],
        }];
        let config = GuardrailsConfig {
            version: "1.0".to_string(),
            generated_at: String::new(),
            threshold_factor: 0.9,
            contracts: vec![GuardrailContract {
                doc: "missing".to_string(),
                file_type: Some("pdf".to_string()),
                pipeline: "docling".to_string(),
                min_sf1: Some(0.5),
                min_tf1: Some(0.5),
                relative_order: Vec::new(),
            }],
        };

        let failures = check_guardrails(&results, &config);

        assert_eq!(failures.len(), 2);
        assert!(failures.iter().any(|failure| failure.starts_with("SF1 unavailable:")));
        assert!(failures.iter().any(|failure| failure.starts_with("TF1 unavailable:")));
    }

    #[test]
    fn guardrails_fail_when_contracted_document_is_missing() {
        let config = GuardrailsConfig {
            version: "1.0".to_string(),
            generated_at: String::new(),
            threshold_factor: 0.9,
            contracts: vec![GuardrailContract {
                doc: "missing".to_string(),
                file_type: Some("pdf".to_string()),
                pipeline: "baseline".to_string(),
                min_sf1: Some(0.5),
                min_tf1: None,
                relative_order: Vec::new(),
            }],
        };

        let failures = check_guardrails(&[], &config);

        assert_eq!(
            failures,
            vec!["missing guardrail document: missing [pdf] baseline".to_string()]
        );
    }

    #[test]
    fn guardrails_fail_when_contracted_pipeline_result_is_missing() {
        let results = vec![DocResult {
            name: "example".to_string(),
            file_type: "pdf".to_string(),
            results: Vec::new(),
        }];
        let config = GuardrailsConfig {
            version: "1.0".to_string(),
            generated_at: String::new(),
            threshold_factor: 0.9,
            contracts: vec![GuardrailContract {
                doc: "example".to_string(),
                file_type: Some("pdf".to_string()),
                pipeline: "baseline".to_string(),
                min_sf1: Some(0.5),
                min_tf1: None,
                relative_order: Vec::new(),
            }],
        };

        let failures = check_guardrails(&results, &config);

        assert_eq!(
            failures,
            vec!["missing guardrail pipeline result: example [pdf] baseline".to_string()]
        );
    }

    #[test]
    fn relative_order_guardrail_rejects_out_of_order_content_independently_of_scores() {
        let config = GuardrailsConfig {
            version: "1.0".to_string(),
            generated_at: String::new(),
            threshold_factor: 0.9,
            contracts: vec![GuardrailContract {
                doc: "681693".to_string(),
                file_type: Some("pdf".to_string()),
                pipeline: "pdf-oxide+layout+reading-order".to_string(),
                min_sf1: Some(0.8),
                min_tf1: None,
                relative_order: reading_order_anchors(READING_ORDER_GUARDRAIL_DOC, READING_ORDER_GUARDRAIL_PIPELINE),
            }],
        };
        let make_results = |content: &str| {
            vec![DocResult {
                name: "681693".to_string(),
                file_type: "pdf".to_string(),
                results: vec![PipelineResult {
                    pipeline: Pipeline::PdfOxideReadingOrder,
                    sf1: 0.9,
                    tf1: 0.9,
                    char_similarity: 0.9,
                    order_score: 0.9,
                    per_type_sf1: HashMap::new(),
                    per_type_precision: HashMap::new(),
                    per_type_recall: HashMap::new(),
                    time_ms: 1.0,
                    missing_tokens: Vec::new(),
                    extra_tokens: Vec::new(),
                    content: content.to_string(),
                }],
            }]
        };

        let expected = "maintainers wanted ! See #182\n# MongoKit\n\
            MongoDB is a great schema-less document oriented database.\n## Philosophy";
        assert!(check_guardrails(&make_results(expected), &config).is_empty());

        let out_of_order = "MongoDB is a great schema-less document oriented database.\n\
            ## Philosophy\nmaintainers wanted ! See #182\n# MongoKit";
        let failures = check_guardrails(&make_results(out_of_order), &config);

        assert_eq!(failures.len(), 1);
        assert!(failures[0].starts_with("relative-order regression: 681693 [pdf] pdf-oxide+layout+reading-order"));
    }

    #[test]
    fn legacy_guardrail_json_defaults_relative_order_and_installs_active_contract() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("guardrails.json");
        std::fs::write(
            &path,
            r#"{
                "version": "1.0",
                "generated_at": "",
                "threshold_factor": 0.9,
                "contracts": [{
                    "doc": "legacy",
                    "pipeline": "baseline",
                    "min_sf1": 0.5
                }]
            }"#,
        )
        .unwrap();

        let config = load_guardrails(&path).unwrap();
        let legacy = config
            .contracts
            .iter()
            .find(|contract| contract.doc == "legacy")
            .unwrap();
        assert!(legacy.relative_order.is_empty());
        assert!(legacy.file_type.is_none());
        let active = config
            .contracts
            .iter()
            .find(|contract| {
                contract.doc == READING_ORDER_GUARDRAIL_DOC && contract.pipeline == READING_ORDER_GUARDRAIL_PIPELINE
            })
            .unwrap();
        assert_eq!(
            active.relative_order,
            reading_order_anchors(READING_ORDER_GUARDRAIL_DOC, READING_ORDER_GUARDRAIL_PIPELINE)
        );
        assert_eq!(active.file_type.as_deref(), Some("pdf"));
    }

    #[test]
    fn guardrail_config_rejects_unknown_pipeline_among_valid_contracts() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("guardrails.json");
        std::fs::write(
            &path,
            r#"{
                "version": "1.0",
                "generated_at": "",
                "threshold_factor": 0.9,
                "contracts": [
                    {"doc": "valid", "pipeline": "baseline", "min_tf1": 0.5},
                    {"doc": "invalid", "pipeline": "unknown-pipeline", "min_tf1": 0.5}
                ]
            }"#,
        )
        .unwrap();

        let error = load_guardrails(&path).unwrap_err().to_string();

        assert!(
            error.contains("unknown guardrail pipeline 'unknown-pipeline'"),
            "{error}"
        );
    }

    #[test]
    fn guardrail_config_rejects_contract_without_predicate() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("guardrails.json");
        std::fs::write(
            &path,
            r#"{
                "version": "1.0",
                "generated_at": "",
                "threshold_factor": 0.9,
                "contracts": [{"doc": "empty", "pipeline": "baseline"}]
            }"#,
        )
        .unwrap();

        let error = load_guardrails(&path).unwrap_err().to_string();

        assert!(
            error.contains("has no threshold or relative-order predicate"),
            "{error}"
        );
    }

    #[test]
    fn guardrails_require_file_type_for_duplicate_document_names() {
        let result_for = |file_type: &str, sf1: f64| DocResult {
            name: "shared".to_string(),
            file_type: file_type.to_string(),
            results: vec![PipelineResult {
                pipeline: Pipeline::Baseline,
                sf1,
                tf1: 1.0,
                char_similarity: 1.0,
                order_score: sf1,
                per_type_sf1: HashMap::new(),
                per_type_precision: HashMap::new(),
                per_type_recall: HashMap::new(),
                time_ms: 1.0,
                missing_tokens: Vec::new(),
                extra_tokens: Vec::new(),
                content: String::new(),
            }],
        };
        let results = vec![result_for("json", f64::NAN), result_for("pdf", 0.9)];
        let mut contract = GuardrailContract {
            doc: "shared".to_string(),
            file_type: None,
            pipeline: "baseline".to_string(),
            min_sf1: Some(0.8),
            min_tf1: None,
            relative_order: Vec::new(),
        };
        let config_for = |contract| GuardrailsConfig {
            version: "1.0".to_string(),
            generated_at: String::new(),
            threshold_factor: 0.9,
            contracts: vec![contract],
        };

        let failures = check_guardrails(&results, &config_for(contract.clone()));
        assert_eq!(failures.len(), 1);
        assert!(failures[0].contains("ambiguous legacy guardrail target"));
        assert!(failures[0].contains("json, pdf"));

        contract.file_type = Some("pdf".to_string());
        assert!(check_guardrails(&results, &config_for(contract)).is_empty());
    }

    #[test]
    fn relative_order_rejects_missing_repeated_and_empty_anchors() {
        assert_eq!(
            check_relative_order("first then third", &["first".to_string(), "second".to_string()]),
            Err("missing or out-of-order anchor \"second\"".to_string())
        );
        assert_eq!(
            check_relative_order("once", &["once".to_string(), "once".to_string()]),
            Err("missing or out-of-order anchor \"once\"".to_string())
        );
        assert_eq!(
            check_relative_order("content", &["".to_string()]),
            Err("contains an empty anchor".to_string())
        );
    }

    #[test]
    fn relative_order_normalizes_markdown_escapes_and_unicode_hyphens() {
        let anchors = vec![
            "maintainers wanted".to_string(),
            "See #182".to_string(),
            "MongoKit".to_string(),
            "MongoDB is a great schema-less document oriented database".to_string(),
            "Philosophy".to_string(),
        ];
        let equivalent = "maintainers wanted\nSee \\#182\n# MongoKit\n\
            MongoDB is a great schema‑less document oriented database\n### Philosophy";
        assert_eq!(check_relative_order(equivalent, &anchors), Ok(()));

        let wrong_order = "maintainers wanted\n\
            MongoDB is a great schema‑less document oriented database\n\
            See \\#182\n# MongoKit\n### Philosophy";
        assert_eq!(
            check_relative_order(wrong_order, &anchors),
            Err(
                "missing or out-of-order anchor \"MongoDB is a great schema-less document oriented database\""
                    .to_string()
            )
        );
    }

    #[test]
    fn comparison_json_preserves_unavailable_aggregate_scores() {
        let temp = tempfile::tempdir().unwrap();
        let output_path = temp.path().join("comparison.json");
        let results = vec![DocResult {
            name: "missing".to_string(),
            file_type: "pdf".to_string(),
            results: vec![PipelineResult {
                pipeline: Pipeline::Docling,
                sf1: f64::NAN,
                tf1: f64::NAN,
                char_similarity: f64::NAN,
                order_score: f64::NAN,
                per_type_sf1: HashMap::new(),
                per_type_precision: HashMap::new(),
                per_type_recall: HashMap::new(),
                time_ms: f64::NAN,
                missing_tokens: Vec::new(),
                extra_tokens: Vec::new(),
                content: String::new(),
            }],
        }];

        write_comparison_json(&results, &output_path).unwrap();
        let output: serde_json::Value = serde_json::from_slice(&std::fs::read(output_path).unwrap()).unwrap();

        assert!(output["by_format"]["pdf"]["pipelines"][0]["avg_sf1"].is_null());
        assert!(output["by_format"]["pdf"]["pipelines"][0]["avg_tf1"].is_null());
        assert_eq!(output["by_format"]["pdf"]["pipelines"][0]["sf1_count"], 0);
        assert_eq!(output["by_format"]["pdf"]["pipelines"][0]["tf1_count"], 0);
        assert!(output["overall"]["pipelines"][0]["avg_sf1"].is_null());
        assert!(output["overall"]["pipelines"][0]["avg_tf1"].is_null());
        assert_eq!(output["overall"]["pipelines"][0]["sf1_count"], 0);
        assert_eq!(output["overall"]["pipelines"][0]["tf1_count"], 0);
    }

    #[test]
    fn comparison_json_counts_metric_samples_independently() {
        let temp = tempfile::tempdir().unwrap();
        let output_path = temp.path().join("comparison.json");
        let result = |name: &str, sf1: f64, tf1: f64| DocResult {
            name: name.to_string(),
            file_type: "txt".to_string(),
            results: vec![PipelineResult {
                pipeline: Pipeline::Docling,
                sf1,
                tf1,
                char_similarity: tf1,
                order_score: sf1,
                per_type_sf1: HashMap::new(),
                per_type_precision: HashMap::new(),
                per_type_recall: HashMap::new(),
                time_ms: 1.0,
                missing_tokens: Vec::new(),
                extra_tokens: Vec::new(),
                content: String::new(),
            }],
        };
        let results = vec![result("markdown", 0.6, 0.5), result("text-only", f64::NAN, 0.9)];

        write_comparison_json(&results, &output_path).unwrap();
        let output: serde_json::Value = serde_json::from_slice(&std::fs::read(output_path).unwrap()).unwrap();
        let summary = &output["by_format"]["txt"]["pipelines"][0];

        assert_eq!(summary["sf1_count"], 1);
        assert_eq!(summary["tf1_count"], 2);
        assert!((summary["avg_sf1"].as_f64().unwrap() - 0.6).abs() < f64::EPSILON);
        assert!((summary["avg_tf1"].as_f64().unwrap() - 0.7).abs() < f64::EPSILON);
        assert_eq!(output["overall"]["pipelines"][0]["sf1_count"], 1);
        assert_eq!(output["overall"]["pipelines"][0]["tf1_count"], 2);
    }

    #[test]
    fn test_pipeline_parse_roundtrip() {
        let all_names = [
            "baseline",
            "layout",
            "tesseract",
            "tesseract+layout",
            "paddle-v6-medium",
            "paddle-v6-medium+layout",
            "paddle-v6-small",
            "paddle-v6-small+layout",
            "paddle-v6-small+layout+det-side-1024",
            "paddle-v6-small+layout+det-side-1536",
            "paddle-v6-small+layout+det-side-2048",
            "paddle-v6-small+layout+det-db-thresh-020",
            "paddle-v6-small+layout+det-db-box-thresh-035",
            "paddle-v6-small+layout+drop-score-030",
            "paddle-v6-small+layout+drop-score-040",
            "paddle-v6-tiny",
            "paddle-v6-tiny+layout",
            "paddle-v5-server",
            "paddle-v5-server+layout",
            "tesseract-autorotate",
            "paddle-autorotate",
            "tesseract-single-block",
            "tesseract-vertical-block",
            "tesseract-sparse-text",
            "paddle-norotate",
            "docling",
            "paddleocr-python",
            "rapidocr",
            "layout+slanet-wired",
            "layout+slanet-wireless",
            "layout+slanet-plus",
            "layout+slanet-auto",
            "pdf-oxide",
            "pdf-oxide+layout",
            "candle-trocr",
            "candle-paddleocr-vl",
            "candle-glm-ocr",
            "candle-deepseek-ocr",
            "candle-paddleocr-vl-15",
        ];
        for name in all_names {
            let pipeline = Pipeline::parse(name).unwrap_or_else(|| panic!("Failed to parse pipeline '{name}'"));
            let roundtrip = pipeline.name();
            let reparsed =
                Pipeline::parse(roundtrip).unwrap_or_else(|| panic!("Failed to reparse pipeline '{roundtrip}'"));
            assert_eq!(pipeline, reparsed, "Roundtrip failed for '{name}' -> '{roundtrip}'");
        }
    }

    #[test]
    fn legacy_paddle_names_parse_to_canonical_presets() {
        let aliases = [
            ("paddle", Pipeline::Paddle),
            ("paddle-mobile", Pipeline::Paddle),
            ("paddle+layout", Pipeline::PaddleLayout),
            ("paddle-layout", Pipeline::PaddleLayout),
            ("paddle-mobile+layout", Pipeline::PaddleLayout),
            ("paddle-server", Pipeline::PaddleServer),
            ("paddle-server+layout", Pipeline::PaddleServerLayout),
        ];

        for (alias, expected) in aliases {
            assert_eq!(
                Pipeline::parse(alias),
                Some(expected),
                "failed to preserve alias '{alias}'"
            );
        }
        assert_eq!(Pipeline::parse("paddle").expect("alias").name(), "paddle-v6-medium");
        assert_eq!(
            Pipeline::parse("paddle+layout").expect("alias").name(),
            "paddle-v6-medium+layout"
        );
    }

    #[test]
    fn tesseract_segmentation_presets_pin_psm_and_language() {
        let cases = [
            (Pipeline::TesseractSingleBlock, TESSERACT_PSM_SINGLE_BLOCK),
            (Pipeline::TesseractVerticalBlock, TESSERACT_PSM_VERTICAL_BLOCK),
            (Pipeline::TesseractSparseText, TESSERACT_PSM_SPARSE_TEXT),
        ];

        for (pipeline, expected_psm) in cases {
            let config = build_extraction_config(pipeline);
            let ocr = config.ocr.expect("Tesseract preset must configure OCR");
            let tesseract = ocr
                .tesseract_config
                .expect("segmentation preset must configure Tesseract");

            assert!(config.force_ocr);
            assert!(config.layout.is_none());
            assert_eq!(ocr.backend, "tesseract");
            assert_eq!(ocr.language, ["eng"]);
            assert_eq!(tesseract.language, ["eng"]);
            assert_eq!(tesseract.psm, expected_psm);
        }
    }

    #[test]
    fn tesseract_segmentation_aliases_use_canonical_names() {
        let cases = [
            (
                "tesseract-psm5",
                Pipeline::TesseractVerticalBlock,
                "tesseract-vertical-block",
            ),
            (
                "tesseract-vertical",
                Pipeline::TesseractVerticalBlock,
                "tesseract-vertical-block",
            ),
            (
                "tesseract-psm6",
                Pipeline::TesseractSingleBlock,
                "tesseract-single-block",
            ),
            (
                "tesseract-psm11",
                Pipeline::TesseractSparseText,
                "tesseract-sparse-text",
            ),
        ];

        for (alias, expected, canonical) in cases {
            assert_eq!(Pipeline::parse(alias), Some(expected));
            assert_eq!(expected.name(), canonical);
            assert_eq!(
                serde_json::from_str::<Pipeline>(&format!(r#""{alias}""#)).expect("deserialize alias"),
                expected
            );
        }
    }

    #[test]
    fn fixture_language_updates_vertical_segmentation_without_changing_psm() {
        let mut metadata = HashMap::new();
        metadata.insert("ocr_language".to_string(), serde_json::json!("jpn_vert"));
        let doc = CorpusDocument {
            name: "vertical-japanese".to_string(),
            document_path: std::path::PathBuf::new(),
            file_type: "jpeg".to_string(),
            file_size: 0,
            ground_truth_text: None,
            ground_truth_markdown: None,
            metadata,
            fixture_path: std::path::PathBuf::new(),
        };
        let mut config = build_extraction_config(Pipeline::TesseractVerticalBlock);

        apply_fixture_ocr_language(&mut config, &doc);

        let ocr = config.ocr.expect("Tesseract preset must configure OCR");
        let tesseract = ocr
            .tesseract_config
            .expect("segmentation preset must configure Tesseract");
        assert_eq!(ocr.language, ["jpn_vert"]);
        assert_eq!(tesseract.language, ["jpn_vert"]);
        assert_eq!(tesseract.psm, TESSERACT_PSM_VERTICAL_BLOCK);
    }

    #[test]
    fn paddle_v6_serialization_uses_canonical_names() {
        let serialized = serde_json::to_string(&Pipeline::PaddleLayout).expect("serialize pipeline");
        assert_eq!(serialized, r#""paddle-v6-medium+layout""#);
        assert_eq!(
            serde_json::from_str::<Pipeline>(r#""paddle-layout""#).expect("deserialize legacy name"),
            Pipeline::PaddleLayout
        );
    }
}
