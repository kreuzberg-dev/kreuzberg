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
//! - **Paddle / PaddleLayout / PaddleServer / PaddleServerLayout**: PaddleOCR
//!   at mobile or server model tier, with optional layout detection.
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

/// Per-page timeout for PDF documents in milliseconds.
///
/// For layout-heavy PDFs (which require layout detection with ONNX inference),
/// allocate additional time proportional to page count. This prevents timeouts
/// on legitimately large documents like 214-page standards sheets.
///
/// Example: 214-page PDF = 60s base + (214 * 400ms) = 60s + 85.6s = 145.6s total
const EXTRACTION_TIMEOUT_PER_PAGE_MS: u64 = 400;

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
    /// PaddleOCR mobile tier (force_ocr) — default model tier
    Paddle,
    /// PaddleOCR mobile tier + layout detection — default model tier
    PaddleLayout,
    /// PaddleOCR server tier (force_ocr, explicit server models)
    PaddleServer,
    /// PaddleOCR server tier + layout detection
    PaddleServerLayout,
    /// Tesseract OCR with auto_rotate enabled
    TesseractAutoRotate,
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
            Pipeline::Paddle => "paddle",
            Pipeline::PaddleLayout => "paddle+layout",
            Pipeline::PaddleServer => "paddle-server",
            Pipeline::PaddleServerLayout => "paddle-server+layout",
            Pipeline::TesseractAutoRotate => "tesseract-autorotate",
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
            "paddle" | "paddle-mobile" => Some(Pipeline::Paddle),
            "paddle+layout" | "paddle-layout" | "paddle-mobile+layout" | "paddle-mobile-layout" => {
                Some(Pipeline::PaddleLayout)
            }
            "paddle-server" => Some(Pipeline::PaddleServer),
            "paddle-server+layout" | "paddle-server-layout" => Some(Pipeline::PaddleServerLayout),
            "tesseract-autorotate" => Some(Pipeline::TesseractAutoRotate),
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
            Pipeline::PaddleServer,
            Pipeline::PaddleServerLayout,
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
    /// Reading order score (LIS-based, 0.0-1.0).
    #[serde(default)]
    pub order_score: f64,
    /// Per-block-type structural F1 scores (e.g. "H1" -> 0.85).
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub per_type_sf1: HashMap<String, f64>,
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
fn compute_extraction_timeout_secs(path: &Path, file_type: &str) -> u64 {
    if file_type.to_lowercase() != "pdf" {
        return EXTRACTION_TIMEOUT_BASE_SECS;
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
        return EXTRACTION_TIMEOUT_BASE_SECS;
    };

    let per_page_secs = EXTRACTION_TIMEOUT_PER_PAGE_MS as f64 / 1000.0;
    let scaled_timeout = EXTRACTION_TIMEOUT_BASE_SECS as f64 + (page_count as f64 * per_page_secs);

    scaled_timeout.ceil() as u64
}

/// Build a xberg ExtractionConfig for the given pipeline.
pub fn build_extraction_config(pipeline: Pipeline) -> xberg::ExtractionConfig {
    use xberg::core::config::{OutputFormat, layout::LayoutDetectionConfig};

    let base = xberg::ExtractionConfig {
        output_format: OutputFormat::Markdown,
        ..Default::default()
    };

    match pipeline {
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
        Pipeline::Paddle => xberg::ExtractionConfig {
            force_ocr: true,
            ocr: Some(xberg::core::config::OcrConfig {
                backend: "paddleocr".to_string(),
                language: vec!["eng".to_string()],
                auto_rotate: true,
                ..Default::default()
            }),
            ..base
        },
        Pipeline::PaddleLayout => xberg::ExtractionConfig {
            force_ocr: true,
            ocr: Some(xberg::core::config::OcrConfig {
                backend: "paddleocr".to_string(),
                language: vec!["eng".to_string()],
                auto_rotate: true,
                ..Default::default()
            }),
            layout: Some(LayoutDetectionConfig::default()),
            ..base
        },
        Pipeline::PaddleServer => xberg::ExtractionConfig {
            force_ocr: true,
            ocr: Some(xberg::core::config::OcrConfig {
                backend: "paddleocr".to_string(),
                language: vec!["eng".to_string()],
                auto_rotate: true,
                paddle_ocr_config: Some(serde_json::json!({"model_tier": "server"})),
                ..Default::default()
            }),
            ..base
        },
        Pipeline::PaddleServerLayout => xberg::ExtractionConfig {
            force_ocr: true,
            ocr: Some(xberg::core::config::OcrConfig {
                backend: "paddleocr".to_string(),
                language: vec!["eng".to_string()],
                auto_rotate: true,
                paddle_ocr_config: Some(serde_json::json!({"model_tier": "server"})),
                ..Default::default()
            }),
            layout: Some(LayoutDetectionConfig::default()),
            ..base
        },
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
        Pipeline::PaddleNoRotate => xberg::ExtractionConfig {
            force_ocr: true,
            ocr: Some(xberg::core::config::OcrConfig {
                backend: "paddleocr".to_string(),
                language: vec!["eng".to_string()],
                auto_rotate: false,
                ..Default::default()
            }),
            ..base
        },
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
    }
}

/// Score extracted content against ground truth, returning (tf1, sf1, order_score, per_type_sf1).
pub fn score_document(
    content: &str,
    gt_text: &str,
    gt_markdown: Option<&str>,
) -> (f64, f64, f64, HashMap<String, f64>) {
    let tf1 = {
        let ext_tokens = tokenize(content);
        let gt_tokens = tokenize(gt_text);
        compute_f1(&ext_tokens, &gt_tokens)
    };
    let (sf1, order_score, per_type_sf1) = match gt_markdown {
        Some(md) => {
            let score = structural_sidecar::score_markdown(content, md);
            let dimensions = score
                .dimensions()
                .into_iter()
                .map(|(name, value)| (name.to_string(), value))
                .collect();
            (score.sf1, score.d5_order, dimensions)
        }
        None => (0.0, 0.0, HashMap::new()),
    };
    (tf1, sf1, order_score, per_type_sf1)
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
            let doc_path = doc.document_path.clone();
            let doc_name = doc.name.clone();
            let pipeline_name = pipeline.name().to_string();

            let extraction_timeout_secs = compute_extraction_timeout_secs(&doc_path, &doc.file_type);
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
    let (tf1, sf1, order_score, per_type_sf1) = if extraction_failed || (content.is_empty() && time_ms > 170_000.0) {
        (f64::NAN, f64::NAN, f64::NAN, std::collections::HashMap::new())
    } else {
        score_document(&content, gt_text, gt_markdown)
    };

    let ext_tokens = tokenize(&content);
    let gt_tokens = tokenize(gt_text);
    let (mut missing_tokens, mut extra_tokens) = compute_token_diff(&ext_tokens, &gt_tokens);
    missing_tokens.truncate(50);
    extra_tokens.truncate(50);

    PipelineResult {
        pipeline,
        sf1,
        tf1,
        order_score,
        per_type_sf1,
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
        require_markdown_ground_truth: true,
        name_patterns: config.name_filter.clone().into_iter().collect(),
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
            let time_str = if pr.time_ms.is_nan() || pr.time_ms <= 0.0 {
                "---".to_string()
            } else {
                format!("{:.0}", pr.time_ms)
            };
            eprint!(" {:>9.1}% {:>9.1}% {:>8}", pr.sf1 * 100.0, pr.tf1 * 100.0, time_str);
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
        let time_str = if avg_time.is_nan() {
            "---".to_string()
        } else {
            format!("{:.0}", avg_time)
        };
        eprint!(" {:>9.1}% {:>9.1}% {:>8}", avg_sf1 * 100.0, avg_tf1 * 100.0, time_str);
    }
    eprintln!();

    for (i, name) in pipeline_names.iter().enumerate() {
        let failed = results.iter().filter(|r| !r.results[i].sf1.is_finite()).count();
        if failed > 0 {
            eprintln!("  {}: {} timeouts/errors (excluded from averages)", name, failed);
        }
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
            eprint!("  {:>9.1}% {:>9.1}%", avg_sf1 * 100.0, avg_tf1 * 100.0);
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
            let _n = docs.len() as f64;
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

    let _n = results.len() as f64;
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
#[derive(Debug, Serialize, Deserialize)]
pub struct GuardrailContract {
    pub doc: String,
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
    Ok(config)
}

/// Generate a guardrails configuration from benchmark results.
///
/// Each document+pipeline pair with meaningful scores produces a contract
/// whose minimum thresholds are the observed score multiplied by
/// `threshold_factor` (e.g. 0.9 means 90% of observed score).
pub fn generate_guardrails(results: &[DocResult], threshold_factor: f64) -> GuardrailsConfig {
    let mut contracts = Vec::new();
    for doc in results {
        for result in &doc.results {
            if result.sf1.is_nan() || result.tf1.is_nan() {
                continue;
            }
            if result.sf1 < 0.01 && result.tf1 < 0.01 {
                continue;
            }
            contracts.push(GuardrailContract {
                doc: doc.name.clone(),
                pipeline: result.pipeline.name().to_string(),
                min_sf1: if result.sf1 > 0.01 {
                    Some((result.sf1 * threshold_factor * 100.0).round() / 100.0)
                } else {
                    None
                },
                min_tf1: if result.tf1 > 0.01 {
                    Some((result.tf1 * threshold_factor * 100.0).round() / 100.0)
                } else {
                    None
                },
                relative_order: reading_order_anchors(&doc.name, result.pipeline.name()),
            });
        }
    }
    let mut config = GuardrailsConfig {
        version: "1.0".to_string(),
        generated_at: chrono::Utc::now().to_rfc3339(),
        threshold_factor,
        contracts,
    };
    install_reading_order_guardrail(&mut config);
    config
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
        contract.doc == READING_ORDER_GUARDRAIL_DOC && contract.pipeline == READING_ORDER_GUARDRAIL_PIPELINE
    }) {
        if contract.relative_order.is_empty() {
            contract.relative_order =
                reading_order_anchors(READING_ORDER_GUARDRAIL_DOC, READING_ORDER_GUARDRAIL_PIPELINE);
        }
        return;
    }
    config.contracts.push(GuardrailContract {
        doc: READING_ORDER_GUARDRAIL_DOC.to_string(),
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
        let Some(doc) = results.iter().find(|r| r.name == contract.doc) else {
            continue;
        };

        let Some(pr) = doc.results.iter().find(|pr| pr.pipeline.name() == contract.pipeline) else {
            continue;
        };

        if let Some(min_sf1) = contract.min_sf1 {
            if !pr.sf1.is_finite() {
                failures.push(format!(
                    "SF1 unavailable: {} {} has no finite score",
                    contract.doc, contract.pipeline
                ));
            } else if pr.sf1 < min_sf1 {
                failures.push(format!(
                    "SF1 regression: {} {} SF1 {:.1}% < minimum {:.1}%",
                    contract.doc,
                    contract.pipeline,
                    pr.sf1 * 100.0,
                    min_sf1 * 100.0,
                ));
            }
        }

        if let Some(min_tf1) = contract.min_tf1 {
            if !pr.tf1.is_finite() {
                failures.push(format!(
                    "TF1 unavailable: {} {} has no finite score",
                    contract.doc, contract.pipeline
                ));
            } else if pr.tf1 < min_tf1 {
                failures.push(format!(
                    "TF1 regression: {} {} TF1 {:.1}% < minimum {:.1}%",
                    contract.doc,
                    contract.pipeline,
                    pr.tf1 * 100.0,
                    min_tf1 * 100.0,
                ));
            }
        }

        if let Err(reason) = check_relative_order(&pr.content, &contract.relative_order) {
            failures.push(format!(
                "relative-order regression: {} {} {reason}",
                contract.doc, contract.pipeline
            ));
        }
    }

    failures
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
        let guardrails_config = load_guardrails(guardrails_path)?;
        eprintln!(
            "Loaded {} guardrail contracts from {} (v{}, factor {})",
            guardrails_config.contracts.len(),
            guardrails_path.display(),
            guardrails_config.version,
            guardrails_config.threshold_factor,
        );
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
    fn test_score_document_identical() {
        let text = "Hello world this is a test document";
        let (tf1, sf1, order_score, per_type) = score_document(text, text, None);
        assert!(
            (tf1 - 1.0).abs() < f64::EPSILON,
            "TF1 should be 1.0 for identical text, got {tf1}"
        );
        assert!(
            (sf1 - 0.0).abs() < f64::EPSILON,
            "SF1 should be 0.0 when no markdown GT"
        );
        assert!(
            (order_score - 0.0).abs() < f64::EPSILON,
            "order_score should be 0.0 when no markdown GT"
        );
        assert!(per_type.is_empty(), "per_type should be empty when no markdown GT");
    }

    #[test]
    fn test_score_document_no_markdown_gt() {
        let content = "Some extracted content here";
        let gt_text = "Some ground truth content here";
        let (tf1, sf1, order_score, per_type) = score_document(content, gt_text, None);
        assert!(
            tf1 > 0.0 && tf1 < 1.0,
            "TF1 should be between 0 and 1 for partially matching text, got {tf1}"
        );
        assert!(
            (sf1 - 0.0).abs() < f64::EPSILON,
            "SF1 should be 0.0 when no markdown GT"
        );
        assert!((order_score - 0.0).abs() < f64::EPSILON);
        assert!(per_type.is_empty());
    }

    #[test]
    fn test_score_document_empty() {
        let (tf1, sf1, order_score, per_type) = score_document("", "", None);
        let _ = tf1;
        assert!((sf1 - 0.0).abs() < f64::EPSILON);
        assert!((order_score - 0.0).abs() < f64::EPSILON);
        assert!(per_type.is_empty());
    }

    #[test]
    fn test_score_document_completely_different() {
        let content = "alpha bravo charlie";
        let gt_text = "delta echo foxtrot";
        let (tf1, _sf1, _order_score, _per_type) = score_document(content, gt_text, None);
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
        let (tf1, sf1, _order_score, per_type) = score_document(content, gt_text, Some(gt_markdown));
        assert!(tf1 > 0.0, "TF1 should be positive, got {tf1}");
        assert!(
            sf1 > 0.0,
            "SF1 should be positive when structural GT is provided, got {sf1}"
        );
        let _ = per_type;
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
                order_score: f64::NAN,
                per_type_sf1: HashMap::new(),
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
    fn relative_order_guardrail_rejects_out_of_order_content_independently_of_scores() {
        let config = GuardrailsConfig {
            version: "1.0".to_string(),
            generated_at: String::new(),
            threshold_factor: 0.9,
            contracts: vec![GuardrailContract {
                doc: "681693".to_string(),
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
                    order_score: 0.9,
                    per_type_sf1: HashMap::new(),
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
        assert!(failures[0].starts_with("relative-order regression: 681693 pdf-oxide+layout+reading-order"));
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
                order_score: f64::NAN,
                per_type_sf1: HashMap::new(),
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
        assert!(output["overall"]["pipelines"][0]["avg_sf1"].is_null());
        assert!(output["overall"]["pipelines"][0]["avg_tf1"].is_null());
    }

    #[test]
    fn test_pipeline_parse_roundtrip() {
        let all_names = [
            "baseline",
            "layout",
            "tesseract",
            "tesseract+layout",
            "paddle",
            "paddle+layout",
            "paddle-server",
            "paddle-server+layout",
            "tesseract-autorotate",
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
}
