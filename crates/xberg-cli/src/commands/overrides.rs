//! CLI extraction overrides via `#[derive(clap::Args)]`.
//!
//! Provides `ExtractionOverrides`, a flattened clap struct that captures all
//! optional CLI flags for extraction configuration. Call `validate()` then
//! `apply()` to layer these overrides onto an `ExtractionConfig`.

#[cfg(feature = "ocr-surface")]
use anyhow::Context as _;
use anyhow::{Result, bail};
#[cfg(any(feature = "core-cli", feature = "analysis"))]
use xberg::ChunkingConfig;
#[cfg(feature = "analysis")]
use xberg::LanguageDetectionConfig;
#[cfg(feature = "ocr-surface")]
use xberg::OcrConfig;
use xberg::{ExecutionProviderType, ExtractionConfig, LlmConfig};

use xberg::JupyterCellRendering;

use crate::ContentOutputFormatArg;

/// Which parts of a Jupyter code cell to render during extraction.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum JupyterCellRenderingArg {
    /// Render only the code source; omit saved outputs.
    Source,
    /// Render only the saved cell outputs; omit the code source.
    Outputs,
    /// Render both the code source and the saved outputs (default).
    Both,
}

impl From<JupyterCellRenderingArg> for JupyterCellRendering {
    fn from(arg: JupyterCellRenderingArg) -> Self {
        match arg {
            JupyterCellRenderingArg::Source => JupyterCellRendering::Source,
            JupyterCellRenderingArg::Outputs => JupyterCellRendering::Outputs,
            JupyterCellRenderingArg::Both => JupyterCellRendering::Both,
        }
    }
}

/// Accepted values for `--ocr-backend`.
#[cfg(feature = "ocr-surface")]
const VALID_OCR_BACKENDS: &[&str] = &[
    "tesseract",
    "paddle-ocr",
    "sceptre",
    "vlm",
    "candle-trocr",
    "candle-paddleocr-vl",
    "candle-glm-ocr",
    "candle-deepseek-ocr",
];

/// Hardware acceleration provider for ONNX Runtime models.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum AccelerationArg {
    /// Auto-detect best provider per platform.
    Auto,
    /// CPU execution provider (always available).
    Cpu,
    /// Apple CoreML (macOS/iOS Neural Engine + GPU).
    #[value(name = "coreml")]
    CoreMl,
    /// NVIDIA CUDA GPU acceleration.
    Cuda,
    /// NVIDIA TensorRT (optimized CUDA inference).
    #[value(name = "tensorrt")]
    TensorRt,
}

impl From<AccelerationArg> for ExecutionProviderType {
    fn from(arg: AccelerationArg) -> Self {
        match arg {
            AccelerationArg::Auto => ExecutionProviderType::Auto,
            AccelerationArg::Cpu => ExecutionProviderType::Cpu,
            AccelerationArg::CoreMl => ExecutionProviderType::CoreMl,
            AccelerationArg::Cuda => ExecutionProviderType::Cuda,
            AccelerationArg::TensorRt => ExecutionProviderType::TensorRt,
        }
    }
}

/// Token reduction intensity level.
#[cfg(feature = "analysis")]
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum ReductionLevelArg {
    /// Disable token reduction.
    Off,
    /// Remove only the most obvious filler.
    Light,
    /// Balanced reduction (default when enabled).
    Moderate,
    /// Heavy reduction, may lose some nuance.
    Aggressive,
    /// Maximum compression, lossy.
    Maximum,
}

#[cfg(feature = "analysis")]
impl ReductionLevelArg {
    /// Convert to the string mode expected by `TokenReductionConfig`.
    fn as_mode_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Light => "light",
            Self::Moderate => "moderate",
            Self::Aggressive => "aggressive",
            Self::Maximum => "maximum",
        }
    }
}

/// Optional CLI flags that override fields in `ExtractionConfig`.
///
/// Every field is `Option<T>` (or `Vec<T>` for repeatable flags) so that
/// only explicitly-provided flags take effect. Flatten this struct into any
/// clap command with `#[command(flatten)]`.
#[derive(Debug, Default, clap::Args)]
pub struct ExtractionOverrides {
    /// Enable or disable OCR. When true, configures an OCR backend
    /// (default: tesseract). When false, removes any OCR configuration.
    #[cfg(feature = "ocr-surface")]
    #[arg(long)]
    pub ocr: Option<bool>,

    /// OCR backend to use when --ocr is enabled (tesseract, paddle-ocr, sceptre, vlm, or candle-*).
    #[cfg(feature = "ocr-surface")]
    #[arg(long)]
    pub ocr_backend: Option<String>,

    /// OCR language code. Tesseract uses ISO 639-3 (eng, fra, deu).
    /// PaddleOCR uses short codes (en, ch, french, korean).
    #[cfg(feature = "ocr-surface")]
    #[arg(long)]
    pub ocr_language: Option<String>,

    /// Force OCR even if text extraction succeeds.
    #[cfg(feature = "ocr-surface")]
    #[arg(long)]
    pub force_ocr: Option<bool>,

    /// OCR pages that look like scans, keeping native text elsewhere.
    ///
    /// Detects pages that are full-page images, including scans whose hidden
    /// text layer would otherwise pass the default quality check.
    #[cfg(feature = "ocr-surface")]
    #[arg(long)]
    pub ocr_scanned_pages: bool,

    /// Minimum scan confidence (0.0-1.0) for --ocr-scanned-pages. Default: 0.7.
    ///
    /// A threshold of 0.50 or lower also OCRs born-digital slides that use a
    /// full-bleed background image.
    #[cfg(feature = "ocr-surface")]
    #[arg(long, requires = "ocr_scanned_pages")]
    pub scanned_min_confidence: Option<f64>,

    /// Disable OCR entirely (even for images)
    #[cfg(feature = "ocr-surface")]
    #[arg(long)]
    pub disable_ocr: Option<bool>,

    /// Disable extraction result caching.
    #[arg(long)]
    pub no_cache: Option<bool>,

    /// Enable automatic image rotation before OCR based on detected orientation.
    #[cfg(feature = "ocr-surface")]
    #[arg(long)]
    pub ocr_auto_rotate: Option<bool>,

    /// JSON object of per-backend OCR options (e.g. `{"layout_mode":"whole_page"}`).
    #[cfg(feature = "ocr-surface")]
    #[arg(long, value_name = "JSON")]
    pub ocr_backend_options: Option<String>,

    /// VLM model for OCR (implies --ocr-backend vlm). Uses liter-llm routing format
    /// (e.g., "openai/gpt-4o", "anthropic/claude-sonnet-4-20250514").
    #[cfg(feature = "ocr-surface")]
    #[arg(long)]
    pub vlm_model: Option<String>,

    /// VLM API key for OCR
    #[cfg(feature = "ocr-surface")]
    #[arg(long)]
    pub vlm_api_key: Option<String>,

    /// Default LLM API key shared across every LLM-backed feature
    /// (VLM OCR, structured extraction, translation, classification, captioning,
    /// summarisation, NER). Lower precedence than `--vlm-api-key` and any
    /// `api_key` set in the loaded config file, higher precedence than the
    /// `XBERG_LLM_API_KEY` environment variable.
    #[arg(long, value_name = "KEY")]
    pub api_key: Option<String>,

    /// Custom VLM OCR prompt template (Jinja2)
    #[cfg(feature = "ocr-surface")]
    #[arg(long)]
    pub vlm_prompt: Option<String>,

    /// Enable or disable text chunking.
    #[cfg(any(feature = "core-cli", feature = "analysis"))]
    #[arg(long)]
    pub chunk: Option<bool>,

    /// Maximum chunk size in characters.
    #[cfg(any(feature = "core-cli", feature = "analysis"))]
    #[arg(long)]
    pub chunk_size: Option<usize>,

    /// Overlap between consecutive chunks in characters.
    #[cfg(any(feature = "core-cli", feature = "analysis"))]
    #[arg(long)]
    pub chunk_overlap: Option<usize>,

    /// Tokenizer model for token-based chunk sizing (e.g. "Xenova/gpt-4o").
    /// Implicitly enables chunking. Requires the chunking-tokenizers feature.
    #[cfg(any(feature = "core-cli", feature = "analysis"))]
    #[arg(long)]
    pub chunking_tokenizer: Option<String>,

    /// Content rendering format (plain, markdown, djot, html).
    /// Controls the format of extracted content.
    #[arg(long, value_enum)]
    pub content_format: Option<ContentOutputFormatArg>,

    /// Content rendering format (DEPRECATED: use --content-format instead).
    #[arg(long, value_enum, hide = true)]
    pub output_format: Option<ContentOutputFormatArg>,

    /// Include hierarchical document structure in results.
    #[arg(long)]
    pub include_structure: Option<bool>,

    /// For Jupyter notebooks: render code cells as source, outputs, or both (default: both).
    /// Cells are never executed — outputs come only from those saved in the notebook.
    #[arg(long, value_enum)]
    pub jupyter_cell_rendering: Option<JupyterCellRenderingArg>,

    /// Enable quality post-processing.
    #[cfg(feature = "analysis")]
    #[arg(long)]
    pub quality: Option<bool>,

    /// Enable language detection on extracted text.
    #[cfg(feature = "analysis")]
    #[arg(long)]
    pub detect_language: Option<bool>,

    /// Enable layout detection with default model settings (RT-DETR v2).
    /// Use `--layout` to enable or `--layout false` to explicitly disable.
    #[cfg(feature = "layout-detection")]
    #[arg(long, default_missing_value = "true", num_args = 0..=1)]
    pub layout: Option<bool>,

    /// Layout detection confidence threshold (0.0 - 1.0).
    #[cfg(feature = "layout-detection")]
    #[arg(long)]
    pub layout_confidence: Option<f32>,

    /// Which pages the layout model runs on: always (default, every page) or
    /// auto (pre-screen each page and skip the model where it cannot help).
    #[cfg(feature = "layout-detection")]
    #[arg(
        long,
        help = "Layout page selection: always (default, every page) or auto (pre-screen pages)"
    )]
    pub layout_strategy: Option<String>,

    /// Table structure model: tatr (default), slanet_wired, slanet_wireless, slanet_plus, slanet_auto, disabled.
    #[cfg(feature = "layout-detection")]
    #[arg(
        long,
        help = "Table structure model: tatr (default), slanet_wired, slanet_wireless, slanet_plus, slanet_auto, disabled"
    )]
    pub layout_table_model: Option<String>,

    /// Feed layout detection regions into the non-OCR markdown pipeline to improve
    /// heading/table/list/figure structure. Requires `--layout` to be enabled.
    /// Default: false.
    #[cfg(feature = "layout-detection")]
    #[arg(long)]
    pub use_layout_for_markdown: bool,

    /// ONNX Runtime execution provider for model inference.
    #[arg(long, value_enum)]
    pub acceleration: Option<AccelerationArg>,

    /// Maximum number of concurrent extractions in batch mode.
    #[arg(long, help = "Limit parallel extractions in batch mode")]
    pub max_concurrent: Option<usize>,

    /// Cap all internal thread pools (Rayon, ONNX intra-op, batch semaphore).
    #[arg(long, help = "Limit total threads for constrained environments")]
    pub max_threads: Option<usize>,

    /// Extract pages as a separate array in results.
    #[arg(long)]
    pub extract_pages: Option<bool>,

    /// Insert page marker comments into the main content string.
    #[arg(long)]
    pub page_markers: Option<bool>,

    /// Enable image extraction from documents.
    #[arg(long)]
    pub extract_images: Option<bool>,

    /// Target DPI for image normalisation (e.g. 150, 300, 600).
    #[arg(long)]
    pub target_dpi: Option<i32>,

    /// Password(s) for encrypted PDFs. Can be specified multiple times.
    #[cfg(feature = "pdf-surface")]
    #[arg(long)]
    pub pdf_password: Vec<String>,

    /// Extract images embedded in PDF pages.
    #[cfg(feature = "pdf-surface")]
    #[arg(long)]
    pub pdf_extract_images: Option<bool>,

    /// Extract tables from PDF (pdf_oxide native grid + heuristic text-layer fallback).
    /// Default: true.
    #[cfg(feature = "pdf-surface")]
    #[arg(long)]
    pub pdf_extract_tables: Option<bool>,

    /// OCR extracted inline images and inject results into the document.
    #[cfg(all(feature = "pdf-surface", feature = "ocr-surface"))]
    #[arg(long)]
    pub pdf_ocr_inline_images: Option<bool>,

    /// Extract PDF metadata (title, author, etc.).
    #[cfg(feature = "pdf-surface")]
    #[arg(long)]
    pub pdf_extract_metadata: Option<bool>,

    /// PDF extraction backend to use (currently only "pdf-oxide" is supported).
    ///
    /// This flag is accepted for forward-compatibility with future backends.
    /// At present, any value other than "pdf-oxide" is rejected with an error.
    // NOTE: no effect on ExtractionConfig today; reserved for future backend selection.
    #[cfg(feature = "pdf-surface")]
    #[arg(long, value_name = "BACKEND")]
    pub pdf_backend: Option<String>,

    /// Token reduction level (off, light, moderate, aggressive, maximum).
    #[cfg(feature = "analysis")]
    #[arg(long, value_enum)]
    pub token_reduction: Option<ReductionLevelArg>,

    /// Windows codepage fallback for MSG files without codepage metadata.
    /// Common values: 1250 (Central European), 1251 (Cyrillic), 1252 (Western).
    #[arg(long)]
    pub msg_codepage: Option<u32>,

    /// Cache namespace for tenant isolation.
    #[arg(long)]
    pub cache_namespace: Option<String>,

    /// Per-request cache TTL in seconds (0 = skip cache).
    #[arg(long)]
    pub cache_ttl_secs: Option<u64>,

    /// Built-in colour theme for styled HTML output (default, github, dark, light, unstyled).
    /// Implies --content-format html and enables the styled HTML renderer.
    #[cfg(feature = "html")]
    #[arg(long, value_name = "THEME")]
    pub html_theme: Option<String>,

    /// Inline CSS string appended after the theme stylesheet in styled HTML output.
    #[cfg(feature = "html")]
    #[arg(long, value_name = "CSS")]
    pub html_css: Option<String>,

    /// Path to a CSS file loaded once and appended after the theme stylesheet in styled HTML output.
    #[cfg(feature = "html")]
    #[arg(long, value_name = "PATH")]
    pub html_css_file: Option<std::path::PathBuf>,

    /// CSS class prefix used on every emitted class name (default: "kb-").
    #[cfg(feature = "html")]
    #[arg(long, value_name = "PREFIX")]
    pub html_class_prefix: Option<String>,

    /// Suppress the embedded `<style>` block in styled HTML output.
    #[cfg(feature = "html")]
    #[arg(long)]
    pub html_no_embed_css: bool,
}

impl ExtractionOverrides {
    /// Validate flag combinations before applying.
    ///
    /// Call this before `apply()` to surface user-friendly errors for
    /// invalid or contradictory options.
    pub fn validate(&self) -> Result<()> {
        #[cfg(any(feature = "core-cli", feature = "analysis"))]
        if let Some(size) = self.chunk_size {
            if size == 0 {
                bail!("Invalid chunk size: {size}. Chunk size must be greater than 0.");
            }
            if size > 1_000_000 {
                bail!(
                    "Invalid chunk size: {size}. Chunk size must be less than 1,000,000 characters to avoid excessive memory usage."
                );
            }
        }

        #[cfg(any(feature = "core-cli", feature = "analysis"))]
        if let Some(overlap) = self.chunk_overlap
            && let Some(size) = self.chunk_size
            && overlap >= size
        {
            bail!("Invalid chunk overlap: {overlap}. Overlap ({overlap}) must be less than chunk size ({size}).");
        }

        if let Some(dpi) = self.target_dpi
            && (!(36..=2400).contains(&dpi))
        {
            bail!("Invalid target DPI: {dpi}. Value must be between 36 and 2400.");
        }

        #[cfg(feature = "layout-detection")]
        {
            if let Some(conf) = self.layout_confidence
                && !(0.0..=1.0).contains(&conf)
            {
                bail!("Invalid layout confidence: {conf}. Value must be between 0.0 and 1.0.");
            }
            if self.layout == Some(false) && (self.layout_confidence.is_some() || self.layout_table_model.is_some()) {
                bail!("--layout false cannot be combined with --layout-confidence or --layout-table-model");
            }
            if self.layout == Some(false) && self.layout_strategy.is_some() {
                bail!("--layout false cannot be combined with --layout-strategy");
            }
            if let Some(ref strategy) = self.layout_strategy
                && strategy.parse::<xberg::LayoutStrategy>().is_err()
            {
                bail!("Invalid layout strategy: '{strategy}'. Valid: always, auto.");
            }
        }

        #[cfg(feature = "ocr-surface")]
        {
            if self.ocr_scanned_pages && self.disable_ocr == Some(true) {
                bail!("--ocr-scanned-pages cannot be combined with --disable-ocr");
            }
            if let Some(confidence) = self.scanned_min_confidence
                && !(0.0..=1.0).contains(&confidence)
            {
                bail!("Invalid scan confidence: {confidence}. Value must be between 0.0 and 1.0.");
            }
            if self.force_ocr == Some(true) && self.disable_ocr == Some(true) {
                bail!("--force-ocr and --disable-ocr cannot both be true");
            }

            if let Some(ref backend) = self.ocr_backend
                && !VALID_OCR_BACKENDS.contains(&backend.as_str())
            {
                bail!(
                    "Invalid OCR backend '{}'. Valid backends: {}",
                    backend,
                    VALID_OCR_BACKENDS.join(", ")
                );
            }

            self.parsed_backend_options()?;

            if self.vlm_api_key.is_some() && self.vlm_model.is_none() {
                bail!("--vlm-api-key requires --vlm-model to be specified");
            }
            if self.vlm_prompt.is_some() && self.vlm_model.is_none() {
                bail!("--vlm-prompt requires --vlm-model to be specified");
            }
            if self.ocr_backend.as_deref() == Some("vlm") && self.vlm_model.is_none() {
                bail!("--ocr-backend vlm requires --vlm-model to be specified");
            }
        }

        #[cfg(all(
            any(feature = "core-cli", feature = "analysis"),
            not(feature = "chunking-tokenizers")
        ))]
        if self.chunking_tokenizer.is_some() {
            bail!(
                "--chunking-tokenizer requires the chunking-tokenizers feature. \
                 Rebuild with --features chunking-tokenizers"
            );
        }

        if let Some(0) = self.max_concurrent {
            bail!("--max-concurrent must be at least 1");
        }
        if let Some(0) = self.max_threads {
            bail!("--max-threads must be at least 1");
        }

        #[cfg(feature = "pdf-surface")]
        if let Some(ref backend) = self.pdf_backend
            && backend.as_str() != "pdf-oxide"
        {
            bail!(
                "Invalid PDF backend '{}'. Only 'pdf-oxide' is currently supported.",
                backend
            );
        }

        Ok(())
    }

    /// Apply these overrides onto an existing `ExtractionConfig`.
    ///
    /// Only fields that were explicitly provided on the command line take
    /// effect; everything else is left untouched.
    pub fn apply(self, config: &mut ExtractionConfig) {
        let resolved_api_key = resolve_llm_api_key(self.api_key.as_deref());
        #[cfg(feature = "ocr-surface")]
        self.apply_ocr(config);
        #[cfg(feature = "ocr-surface")]
        self.apply_vlm_ocr(config);
        #[cfg(any(feature = "core-cli", feature = "analysis"))]
        self.apply_chunking(config);
        #[cfg(feature = "analysis")]
        self.apply_quality_and_detection(config);
        self.apply_output_format(config);
        self.apply_include_structure(config);
        self.apply_jupyter_cell_rendering(config);
        self.apply_layout(config);
        self.apply_acceleration(config);
        self.apply_concurrency(config);
        self.apply_pages(config);
        self.apply_images(config);
        #[cfg(feature = "pdf-surface")]
        self.apply_pdf(config);
        #[cfg(feature = "analysis")]
        self.apply_token_reduction(config);
        self.apply_email(config);
        self.apply_cache(config);
        self.apply_html_styled(config);
        if let Some(key) = resolved_api_key {
            apply_llm_api_key(config, &key);
        }
    }

    #[cfg(feature = "ocr-surface")]
    fn apply_ocr(&self, config: &mut ExtractionConfig) {
        if let Some(ocr_flag) = self.ocr {
            if ocr_flag {
                let backend = self.ocr_backend.as_deref().unwrap_or("tesseract");
                let language = match &self.ocr_language {
                    Some(lang) => vec![lang.clone()],
                    None => match backend {
                        "paddle-ocr" | "candle-paddleocr-vl" | "candle-glm-ocr" => vec!["en".to_string()],
                        _ => vec!["eng".to_string()],
                    },
                };
                let existing_paddle_config = config.ocr.as_ref().and_then(|o| o.paddle_ocr_config.clone());
                let existing_element_config = config.ocr.as_ref().and_then(|o| o.element_config.clone());
                let auto_rotate = self.ocr_auto_rotate.unwrap_or(false);
                let backend_options = self.parsed_backend_options().ok().flatten();
                config.ocr = Some(OcrConfig {
                    enabled: true,
                    backend: backend.to_string(),
                    language,
                    tesseract_config: None,
                    output_format: None,
                    paddle_ocr_config: existing_paddle_config,
                    element_config: existing_element_config,
                    quality_thresholds: None,
                    pipeline: None,
                    auto_rotate,
                    vlm_config: None,
                    vlm_fallback: Default::default(),
                    vlm_prompt: None,
                    acceleration: None,
                    tessdata_bytes: None,
                    tessdata_path: None,
                    backend_options,
                });
            } else {
                config.ocr = None;
            }
        }

        if self.ocr.is_none()
            && let Some(ref lang) = self.ocr_language
            && let Some(ref mut existing_ocr) = config.ocr
        {
            let language = vec![lang.clone()];
            existing_ocr.language = language.clone();
            if let Some(tesseract_config) = existing_ocr.tesseract_config.as_mut() {
                tesseract_config.language = language.clone();
            }
            if let Some(pipeline) = existing_ocr.pipeline.as_mut() {
                for stage in &mut pipeline.stages {
                    if stage.backend != "tesseract" {
                        continue;
                    }
                    stage.language = Some(language.clone());
                    if let Some(tesseract_config) = stage.tesseract_config.as_mut() {
                        tesseract_config.language = language.clone();
                    }
                }
            }
        }

        if self.ocr.is_none()
            && let Some(rotate) = self.ocr_auto_rotate
            && let Some(ref mut existing_ocr) = config.ocr
        {
            existing_ocr.auto_rotate = rotate;
        }

        if let Some(force_ocr_flag) = self.force_ocr {
            config.force_ocr = force_ocr_flag;
        }
        if let Some(disable_ocr_flag) = self.disable_ocr {
            config.disable_ocr = disable_ocr_flag;
        }
        if self.ocr_scanned_pages {
            config.ocr_strategy = xberg::OcrStrategy::ScannedPages {
                min_confidence: self
                    .scanned_min_confidence
                    .unwrap_or(xberg::core::config::DEFAULT_SCANNED_MIN_CONFIDENCE),
            };
        }
    }

    #[cfg(feature = "ocr-surface")]
    fn apply_vlm_ocr(&self, config: &mut ExtractionConfig) {
        if let Some(ref vlm_model) = self.vlm_model {
            let vlm_llm_config = LlmConfig {
                model: vlm_model.clone(),
                api_key: self.vlm_api_key.clone(),
                ..Default::default()
            };

            let backend_options = self.parsed_backend_options().ok().flatten();
            let ocr = config.ocr.get_or_insert_with(|| OcrConfig {
                enabled: true,
                backend: "vlm".to_string(),
                language: vec!["eng".to_string()],
                tesseract_config: None,
                output_format: None,
                paddle_ocr_config: None,
                element_config: None,
                quality_thresholds: None,
                pipeline: None,
                auto_rotate: false,
                vlm_config: None,
                vlm_fallback: Default::default(),
                vlm_prompt: None,
                acceleration: None,
                tessdata_bytes: None,
                tessdata_path: None,
                backend_options,
            });

            ocr.backend = "vlm".to_string();
            ocr.vlm_config = Some(vlm_llm_config);

            if let Some(ref prompt) = self.vlm_prompt {
                ocr.vlm_prompt = Some(prompt.clone());
            }
        }
    }

    /// Parse `--ocr-backend-options` into a `serde_json::Value`, enforcing that it is a JSON object.
    #[cfg(feature = "ocr-surface")]
    fn parsed_backend_options(&self) -> Result<Option<serde_json::Value>> {
        let Some(ref s) = self.ocr_backend_options else {
            return Ok(None);
        };
        let value: serde_json::Value =
            serde_json::from_str(s).with_context(|| format!("invalid --ocr-backend-options JSON: {s}"))?;
        if !value.is_object() {
            bail!("--ocr-backend-options must be a JSON object");
        }
        Ok(Some(value))
    }

    #[cfg(any(feature = "core-cli", feature = "analysis"))]
    fn apply_chunking(&self, config: &mut ExtractionConfig) {
        let chunk = if self.chunking_tokenizer.is_some() && self.chunk.is_none() {
            Some(true)
        } else {
            self.chunk
        };

        if let Some(chunk_flag) = chunk {
            if chunk_flag {
                let max_characters = self.chunk_size.unwrap_or(1000);
                let overlap = self.chunk_overlap.unwrap_or(200);
                let chunking_config = ChunkingConfig {
                    max_characters,
                    overlap,
                    trim: true,
                    chunker_type: xberg::ChunkerType::Text,
                    ..Default::default()
                };

                #[cfg(feature = "chunking-tokenizers")]
                let chunking_config = if let Some(ref model) = self.chunking_tokenizer {
                    ChunkingConfig {
                        sizing: xberg::ChunkSizing::Tokenizer {
                            model: model.clone(),
                            cache_dir: None,
                        },
                        ..chunking_config
                    }
                } else {
                    chunking_config
                };

                config.chunking = Some(chunking_config);
            } else {
                config.chunking = None;
            }
        } else if let Some(ref mut chunking) = config.chunking {
            if let Some(max_characters) = self.chunk_size {
                chunking.max_characters = max_characters;
            }
            if let Some(overlap) = self.chunk_overlap {
                chunking.overlap = overlap;
            }

            if chunking.overlap >= chunking.max_characters {
                chunking.overlap = chunking.max_characters / 4;
            }

            #[cfg(feature = "chunking-tokenizers")]
            if let Some(ref model) = self.chunking_tokenizer {
                chunking.sizing = xberg::ChunkSizing::Tokenizer {
                    model: model.clone(),
                    cache_dir: None,
                };
            }
        }
    }

    #[cfg(feature = "analysis")]
    fn apply_quality_and_detection(&self, config: &mut ExtractionConfig) {
        if let Some(quality_flag) = self.quality {
            config.enable_quality_processing = quality_flag;
        }
        if let Some(detect_language_flag) = self.detect_language {
            if detect_language_flag {
                config.language_detection = Some(LanguageDetectionConfig {
                    enabled: true,
                    min_confidence: 0.8,
                    detect_multiple: false,
                });
            } else {
                config.language_detection = None;
            }
        }
    }

    fn apply_output_format(&self, config: &mut ExtractionConfig) {
        let final_format = self.content_format.or_else(|| {
            if self.output_format.is_some() {
                tracing::warn!("'--output-format' is deprecated, use '--content-format' instead");
            }
            self.output_format
        });

        if let Some(content_fmt) = final_format {
            config.output_format = content_fmt.into();
        }
    }

    fn apply_include_structure(&self, config: &mut ExtractionConfig) {
        if let Some(flag) = self.include_structure {
            config.include_document_structure = flag;
        }
    }

    fn apply_jupyter_cell_rendering(&self, config: &mut ExtractionConfig) {
        if let Some(rendering) = self.jupyter_cell_rendering {
            config.jupyter_cell_rendering = rendering.into();
        }
    }

    #[allow(unused_variables)]
    fn apply_layout(&self, config: &mut ExtractionConfig) {
        #[cfg(feature = "layout-detection")]
        {
            if self.layout == Some(false) {
                config.layout = None;
                return;
            }

            let has_layout_flag = self.layout == Some(true)
                || self.layout_confidence.is_some()
                || self.layout_table_model.is_some()
                || self.layout_strategy.is_some()
                || self.use_layout_for_markdown;
            if has_layout_flag {
                let mut layout = config.layout.clone().unwrap_or_default();
                if let Some(confidence) = self.layout_confidence {
                    layout.confidence_threshold = Some(confidence);
                }
                if let Some(ref table_model) = self.layout_table_model {
                    layout.table_model = table_model.parse().unwrap_or_default();
                }
                if let Some(ref strategy) = self.layout_strategy {
                    layout.strategy = strategy.parse().unwrap_or_default();
                }
                config.layout = Some(layout);
            }
            if self.use_layout_for_markdown {
                config.use_layout_for_markdown = true;
            }
        }
    }

    fn apply_acceleration(&self, config: &mut ExtractionConfig) {
        if let Some(accel) = self.acceleration {
            let mut accel_config = config.acceleration.clone().unwrap_or_default();
            accel_config.provider = accel.into();
            config.acceleration = Some(accel_config);
        }
    }

    fn apply_concurrency(&self, config: &mut ExtractionConfig) {
        if let Some(max_concurrent) = self.max_concurrent {
            config.max_concurrent_extractions = Some(max_concurrent);
        }
        if let Some(max_threads) = self.max_threads {
            let concurrency = config.concurrency.get_or_insert_with(Default::default);
            concurrency.max_threads = Some(max_threads);
        }
    }

    fn apply_pages(&self, config: &mut ExtractionConfig) {
        let has_page_flag = self.extract_pages.is_some() || self.page_markers.is_some();
        if has_page_flag {
            let mut page_config = config.pages.clone().unwrap_or_default();
            if let Some(extract) = self.extract_pages {
                page_config.extract_pages = extract;
            }
            if let Some(markers) = self.page_markers {
                page_config.insert_page_markers = markers;
            }
            config.pages = Some(page_config);
        }
    }

    fn apply_images(&self, config: &mut ExtractionConfig) {
        let has_image_flag = self.extract_images.is_some() || self.target_dpi.is_some();
        if has_image_flag {
            let mut img = config.images.clone().unwrap_or_default();
            if let Some(extract) = self.extract_images {
                img.extract_images = extract;
            }
            if let Some(dpi) = self.target_dpi {
                img.target_dpi = dpi;
            }
            config.images = Some(img);
        }
    }

    #[cfg(feature = "pdf-surface")]
    fn apply_pdf(&self, config: &mut ExtractionConfig) {
        let has_pdf_flag = self.pdf_extract_images.is_some()
            || self.pdf_extract_tables.is_some()
            || self.pdf_extract_metadata.is_some()
            || !self.pdf_password.is_empty();
        #[cfg(feature = "ocr-surface")]
        let has_pdf_flag = has_pdf_flag || self.pdf_ocr_inline_images.is_some();
        if has_pdf_flag {
            let pdf_opts = config.pdf_options.get_or_insert_with(Default::default);
            if let Some(extract_img) = self.pdf_extract_images {
                pdf_opts.extract_images = extract_img;
            }
            if let Some(extract_tables) = self.pdf_extract_tables {
                pdf_opts.extract_tables = extract_tables;
            }
            #[cfg(feature = "ocr-surface")]
            if let Some(ocr_img) = self.pdf_ocr_inline_images {
                pdf_opts.ocr_inline_images = ocr_img;
            }
            if let Some(extract_meta) = self.pdf_extract_metadata {
                pdf_opts.extract_metadata = extract_meta;
            }
            if !self.pdf_password.is_empty() {
                pdf_opts.passwords = Some(self.pdf_password.clone());
            }
        }
    }

    #[cfg(feature = "analysis")]
    fn apply_token_reduction(&self, config: &mut ExtractionConfig) {
        if let Some(level) = self.token_reduction {
            config.token_reduction = Some(xberg::TokenReductionOptions {
                mode: level.as_mode_str().to_string(),
                preserve_important_words: true,
            });
        }
    }

    fn apply_email(&self, config: &mut ExtractionConfig) {
        if let Some(codepage) = self.msg_codepage {
            let email = config.email.get_or_insert_with(Default::default);
            email.msg_fallback_codepage = Some(codepage);
        }
    }

    fn apply_cache(&self, config: &mut ExtractionConfig) {
        if let Some(no_cache_flag) = self.no_cache {
            config.use_cache = !no_cache_flag;
        }
        if let Some(ns) = &self.cache_namespace {
            config.cache_namespace = Some(ns.clone());
        }
        if let Some(ttl) = self.cache_ttl_secs {
            config.cache_ttl_secs = Some(ttl);
        }
    }

    #[allow(unused_variables)]
    fn apply_html_styled(&self, config: &mut ExtractionConfig) {
        #[cfg(feature = "html")]
        {
            let has_flag = self.html_theme.is_some()
                || self.html_css.is_some()
                || self.html_css_file.is_some()
                || self.html_class_prefix.is_some()
                || self.html_no_embed_css;

            if has_flag {
                config.output_format = xberg::OutputFormat::Html;

                let mut html_cfg = config.html_output.clone().unwrap_or_default();

                if let Some(ref theme_str) = self.html_theme {
                    html_cfg.theme = match theme_str.to_lowercase().as_str() {
                        "github" => xberg::HtmlTheme::GitHub,
                        "dark" => xberg::HtmlTheme::Dark,
                        "light" => xberg::HtmlTheme::Light,
                        "unstyled" => xberg::HtmlTheme::Unstyled,
                        _ => xberg::HtmlTheme::Default,
                    };
                }

                if let Some(ref css) = self.html_css {
                    html_cfg.css = Some(css.clone());
                }

                if let Some(ref path) = self.html_css_file {
                    html_cfg.css_file = Some(path.clone());
                }

                if let Some(ref prefix) = self.html_class_prefix {
                    html_cfg.class_prefix = prefix.clone();
                }

                if self.html_no_embed_css {
                    html_cfg.embed_css = false;
                }

                config.html_output = Some(html_cfg);
            }
        }
    }
}

/// Resolve the LLM API key the CLI should propagate to every `LlmConfig` slot.
///
/// Precedence (highest first):
/// 1. The `--api-key` CLI flag (`cli_api_key`).
/// 2. The `XBERG_LLM_API_KEY` environment variable.
/// 3. `None` — keep whatever the loaded config / inline JSON / overrides set.
///
/// Returns `None` when neither the CLI flag nor the environment variable
/// supplies a non-empty value. In that case [`apply_llm_api_key`] is not
/// called and liter-llm's per-provider env-var fallback runs at request time.
///
/// The resolved source is logged at `info!` level; the key value itself is
/// never logged.
pub(crate) fn resolve_llm_api_key(cli_api_key: Option<&str>) -> Option<String> {
    if let Some(key) = cli_api_key.map(str::trim).filter(|s| !s.is_empty()) {
        tracing::info!(source = "cli_flag", "Resolved LLM API key from --api-key flag");
        return Some(key.to_string());
    }
    if let Ok(value) = std::env::var("XBERG_LLM_API_KEY")
        && !value.is_empty()
    {
        tracing::info!(source = "xberg_env", "Resolved LLM API key from XBERG_LLM_API_KEY");
        return Some(value);
    }
    None
}

/// Write `key` into every [`LlmConfig`] field of `config` whose `api_key` is
/// `None`. Existing non-`None` values (from the loaded config file or inline
/// JSON) take precedence over the resolved key — the CLI never silently
/// overrides explicit configuration.
pub(crate) fn apply_llm_api_key(config: &mut ExtractionConfig, key: &str) {
    fn fill(slot: &mut LlmConfig, key: &str) {
        if slot.api_key.is_none() {
            slot.api_key = Some(key.to_string());
        }
    }

    if let Some(ocr) = config.ocr.as_mut()
        && let Some(vlm) = ocr.vlm_config.as_mut()
    {
        fill(vlm, key);
    }

    if let Some(ext) = config.structured_extraction.as_mut() {
        fill(&mut ext.llm, key);
    }

    if let Some(chunking) = config.chunking.as_mut()
        && let Some(embedding) = chunking.embedding.as_mut()
        && let xberg::EmbeddingModelType::Llm { llm } = &mut embedding.model
    {
        fill(llm, key);
    }

    if let Some(translation) = config.translation.as_mut() {
        fill(&mut translation.llm, key);
    }

    if let Some(pc) = config.page_classification.as_mut() {
        fill(&mut pc.llm, key);
    }

    if let Some(cap) = config.captioning.as_mut() {
        fill(&mut cap.llm, key);
    }

    if let Some(sum) = config.summarization.as_mut()
        && let Some(llm) = sum.llm.as_mut()
    {
        fill(llm, key);
    }

    if let Some(ner) = config.ner.as_mut()
        && let Some(llm) = ner.llm.as_mut()
    {
        fill(llm, key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use xberg::ExtractionConfig;

    fn default_overrides() -> ExtractionOverrides {
        ExtractionOverrides::default()
    }

    #[cfg(feature = "ocr-surface")]
    #[test]
    fn test_ocr_default_language_tesseract() {
        let mut config = ExtractionConfig::default();
        let overrides = ExtractionOverrides {
            ocr: Some(true),
            ..default_overrides()
        };
        overrides.apply(&mut config);
        let ocr = config.ocr.unwrap();
        assert_eq!(ocr.backend, "tesseract");
        assert_eq!(ocr.language, vec!["eng".to_string()]);
    }

    #[cfg(feature = "ocr-surface")]
    #[test]
    fn test_ocr_default_language_paddleocr() {
        let mut config = ExtractionConfig::default();
        let overrides = ExtractionOverrides {
            ocr: Some(true),
            ocr_backend: Some("paddle-ocr".to_string()),
            ..default_overrides()
        };
        overrides.apply(&mut config);
        let ocr = config.ocr.unwrap();
        assert_eq!(ocr.backend, "paddle-ocr");
        assert_eq!(ocr.language, vec!["en".to_string()]);
    }

    #[cfg(feature = "ocr-surface")]
    #[test]
    fn test_ocr_default_language_sceptre() {
        let mut config = ExtractionConfig::default();
        let overrides = ExtractionOverrides {
            ocr: Some(true),
            ocr_backend: Some("sceptre".to_string()),
            ..default_overrides()
        };
        overrides.apply(&mut config);
        let ocr = config.ocr.expect("OCR config should be set");
        assert_eq!(ocr.backend, "sceptre");
        assert_eq!(ocr.language, vec!["eng".to_string()]);
    }

    #[cfg(feature = "ocr-surface")]
    #[test]
    fn test_validate_unknown_ocr_backend_rejected() {
        let overrides = ExtractionOverrides {
            ocr_backend: Some("unsupported-ocr".to_string()),
            ..default_overrides()
        };
        let err = overrides.validate().unwrap_err();
        assert!(err.to_string().contains("Invalid OCR backend"));
    }

    #[cfg(feature = "ocr-surface")]
    #[test]
    fn test_ocr_language_override_tesseract() {
        let mut config = ExtractionConfig::default();
        let overrides = ExtractionOverrides {
            ocr: Some(true),
            ocr_language: Some("fra".to_string()),
            ..default_overrides()
        };
        overrides.apply(&mut config);
        let ocr = config.ocr.unwrap();
        assert_eq!(ocr.backend, "tesseract");
        assert_eq!(ocr.language, vec!["fra".to_string()]);
    }

    #[cfg(feature = "ocr-surface")]
    #[test]
    fn test_ocr_language_override_paddleocr() {
        let mut config = ExtractionConfig::default();
        let overrides = ExtractionOverrides {
            ocr: Some(true),
            ocr_backend: Some("paddle-ocr".to_string()),
            ocr_language: Some("ch".to_string()),
            ..default_overrides()
        };
        overrides.apply(&mut config);
        let ocr = config.ocr.unwrap();
        assert_eq!(ocr.backend, "paddle-ocr");
        assert_eq!(ocr.language, vec!["ch".to_string()]);
    }

    #[cfg(feature = "ocr-surface")]
    #[test]
    fn test_ocr_language_without_ocr_flag_no_existing_config() {
        let mut config = ExtractionConfig::default();
        let overrides = ExtractionOverrides {
            ocr_language: Some("deu".to_string()),
            ..default_overrides()
        };
        overrides.apply(&mut config);
        assert!(config.ocr.is_none());
    }

    #[cfg(feature = "ocr-surface")]
    #[test]
    fn test_ocr_language_without_ocr_flag_existing_config() {
        let mut config = ExtractionConfig {
            ocr: Some(OcrConfig {
                enabled: true,
                backend: "tesseract".to_string(),
                language: vec!["eng".to_string()],
                tesseract_config: None,
                output_format: None,
                paddle_ocr_config: None,
                element_config: None,
                quality_thresholds: None,
                pipeline: None,
                auto_rotate: false,
                vlm_config: None,
                vlm_fallback: Default::default(),
                vlm_prompt: None,
                acceleration: None,
                tessdata_bytes: None,
                tessdata_path: None,
                backend_options: None,
            }),
            ..Default::default()
        };
        let overrides = ExtractionOverrides {
            ocr_language: Some("deu".to_string()),
            ..default_overrides()
        };
        overrides.apply(&mut config);
        let ocr = config.ocr.unwrap();
        assert_eq!(ocr.backend, "tesseract");
        assert_eq!(ocr.language, vec!["deu".to_string()]);
    }

    #[cfg(feature = "ocr-surface")]
    #[test]
    fn test_ocr_language_updates_existing_nested_tesseract_config() {
        let mut config = ExtractionConfig {
            ocr: Some(OcrConfig {
                enabled: true,
                backend: "tesseract".to_string(),
                language: vec!["eng".to_string()],
                tesseract_config: Some(xberg::TesseractConfig {
                    language: vec!["eng".to_string()],
                    use_cache: false,
                    ..Default::default()
                }),
                output_format: None,
                paddle_ocr_config: None,
                element_config: None,
                quality_thresholds: None,
                pipeline: None,
                auto_rotate: false,
                vlm_config: None,
                vlm_fallback: Default::default(),
                vlm_prompt: None,
                acceleration: None,
                tessdata_bytes: None,
                tessdata_path: None,
                backend_options: None,
            }),
            ..Default::default()
        };
        let overrides = ExtractionOverrides {
            ocr_language: Some("deu".to_string()),
            ..default_overrides()
        };

        overrides.apply(&mut config);

        let ocr = config.ocr.unwrap();
        assert_eq!(ocr.language, vec!["deu".to_string()]);
        let tesseract = ocr.tesseract_config.unwrap();
        assert_eq!(tesseract.language, vec!["deu".to_string()]);
        assert!(!tesseract.use_cache);
    }

    #[cfg(feature = "ocr-surface")]
    #[test]
    fn test_ocr_language_updates_tesseract_pipeline_stages() {
        let tesseract_config = xberg::TesseractConfig {
            language: vec!["eng".to_string()],
            use_cache: false,
            ..Default::default()
        };
        let mut config = ExtractionConfig {
            ocr: Some(OcrConfig {
                pipeline: Some(xberg::OcrPipelineConfig {
                    stages: vec![
                        xberg::OcrPipelineStage {
                            backend: "tesseract".to_string(),
                            priority: 100,
                            language: Some(vec!["eng".to_string()]),
                            tesseract_config: Some(tesseract_config),
                            paddle_ocr_config: None,
                            vlm_config: None,
                            backend_options: None,
                        },
                        xberg::OcrPipelineStage {
                            backend: "paddle-ocr".to_string(),
                            priority: 90,
                            language: Some(vec!["en".to_string()]),
                            tesseract_config: None,
                            paddle_ocr_config: None,
                            vlm_config: None,
                            backend_options: None,
                        },
                    ],
                    quality_thresholds: Default::default(),
                }),
                ..OcrConfig::default()
            }),
            ..Default::default()
        };
        let overrides = ExtractionOverrides {
            ocr_language: Some("deu".to_string()),
            ..default_overrides()
        };

        overrides.apply(&mut config);

        let stages = &config.ocr.unwrap().pipeline.unwrap().stages;
        assert_eq!(stages[0].language, Some(vec!["deu".to_string()]));
        assert_eq!(
            stages[0].tesseract_config.as_ref().unwrap().language,
            vec!["deu".to_string()]
        );
        assert_eq!(stages[1].backend, "paddle-ocr");
        assert_eq!(stages[1].priority, 90);
        assert_eq!(stages[1].language, Some(vec!["en".to_string()]));
        assert!(stages[1].tesseract_config.is_none());
        assert!(stages[1].paddle_ocr_config.is_none());
        assert!(stages[1].vlm_config.is_none());
        assert!(stages[1].backend_options.is_none());
    }

    #[cfg(feature = "ocr-surface")]
    #[test]
    fn test_ocr_disabled_ignores_language() {
        let mut config = ExtractionConfig::default();
        let overrides = ExtractionOverrides {
            ocr: Some(false),
            ocr_language: Some("fra".to_string()),
            ..default_overrides()
        };
        overrides.apply(&mut config);
        assert!(config.ocr.is_none());
    }

    #[cfg(feature = "ocr-surface")]
    #[test]
    fn test_ocr_backend_options_parsed_and_applied() {
        let mut config = ExtractionConfig::default();
        let backend_options_json = r#"{"task":"chart","layout_mode":"whole_page"}"#;
        let overrides = ExtractionOverrides {
            ocr: Some(true),
            ocr_backend_options: Some(backend_options_json.to_string()),
            ..default_overrides()
        };

        assert!(overrides.validate().is_ok());

        overrides.apply(&mut config);
        let ocr = config.ocr.unwrap();

        assert!(ocr.backend_options.is_some());
        let opts = ocr.backend_options.unwrap();
        assert!(opts.is_object());
        assert_eq!(opts.get("task").and_then(|v| v.as_str()), Some("chart"));
        assert_eq!(opts.get("layout_mode").and_then(|v| v.as_str()), Some("whole_page"));
    }

    #[cfg(feature = "ocr-surface")]
    #[test]
    fn test_ocr_backend_options_invalid_json_fails_validation() {
        let overrides = ExtractionOverrides {
            ocr: Some(true),
            ocr_backend_options: Some("not-valid-json".to_string()),
            ..default_overrides()
        };
        assert!(overrides.validate().is_err());
    }

    #[cfg(feature = "ocr-surface")]
    #[test]
    fn test_ocr_backend_options_not_object_fails_validation() {
        let overrides = ExtractionOverrides {
            ocr: Some(true),
            ocr_backend_options: Some(r#"["array", "not", "object"]"#.to_string()),
            ..default_overrides()
        };
        assert!(overrides.validate().is_err());
    }

    #[cfg(any(feature = "core-cli", feature = "analysis"))]
    #[test]
    fn test_chunking_enabled_defaults() {
        let mut config = ExtractionConfig::default();
        let overrides = ExtractionOverrides {
            chunk: Some(true),
            ..default_overrides()
        };
        overrides.apply(&mut config);
        let chunking = config.chunking.unwrap();
        assert_eq!(chunking.max_characters, 1000);
        assert_eq!(chunking.overlap, 200);
    }

    #[cfg(any(feature = "core-cli", feature = "analysis"))]
    #[test]
    fn test_chunking_custom_size() {
        let mut config = ExtractionConfig::default();
        let overrides = ExtractionOverrides {
            chunk: Some(true),
            chunk_size: Some(500),
            chunk_overlap: Some(50),
            ..default_overrides()
        };
        overrides.apply(&mut config);
        let chunking = config.chunking.unwrap();
        assert_eq!(chunking.max_characters, 500);
        assert_eq!(chunking.overlap, 50);
    }

    #[cfg(any(feature = "core-cli", feature = "analysis"))]
    #[test]
    fn test_chunking_disabled() {
        let mut config = ExtractionConfig {
            chunking: Some(ChunkingConfig::default()),
            ..Default::default()
        };
        let overrides = ExtractionOverrides {
            chunk: Some(false),
            ..default_overrides()
        };
        overrides.apply(&mut config);
        assert!(config.chunking.is_none());
    }

    #[cfg(any(feature = "core-cli", feature = "analysis"))]
    #[test]
    fn test_validate_chunk_size_zero() {
        let overrides = ExtractionOverrides {
            chunk_size: Some(0),
            ..default_overrides()
        };
        assert!(overrides.validate().is_err());
    }

    #[cfg(any(feature = "core-cli", feature = "analysis"))]
    #[test]
    fn test_validate_chunk_size_too_large() {
        let overrides = ExtractionOverrides {
            chunk_size: Some(2_000_000),
            ..default_overrides()
        };
        assert!(overrides.validate().is_err());
    }

    #[cfg(any(feature = "core-cli", feature = "analysis"))]
    #[test]
    fn test_validate_overlap_exceeds_size() {
        let overrides = ExtractionOverrides {
            chunk_size: Some(100),
            chunk_overlap: Some(200),
            ..default_overrides()
        };
        assert!(overrides.validate().is_err());
    }

    #[test]
    fn test_validate_target_dpi_out_of_range() {
        let overrides = ExtractionOverrides {
            target_dpi: Some(5),
            ..default_overrides()
        };
        assert!(overrides.validate().is_err());

        let overrides = ExtractionOverrides {
            target_dpi: Some(5000),
            ..default_overrides()
        };
        assert!(overrides.validate().is_err());
    }

    #[test]
    fn test_validate_target_dpi_valid() {
        let overrides = ExtractionOverrides {
            target_dpi: Some(300),
            ..default_overrides()
        };
        assert!(overrides.validate().is_ok());
    }

    #[cfg(feature = "layout-detection")]
    #[test]
    fn test_validate_layout_confidence_out_of_range() {
        let overrides = ExtractionOverrides {
            layout_confidence: Some(1.5),
            ..default_overrides()
        };
        assert!(overrides.validate().is_err());

        let overrides = ExtractionOverrides {
            layout_confidence: Some(-0.1),
            ..default_overrides()
        };
        assert!(overrides.validate().is_err());
    }

    #[cfg(feature = "layout-detection")]
    #[test]
    fn test_validate_layout_confidence_valid() {
        let overrides = ExtractionOverrides {
            layout_confidence: Some(0.5),
            ..default_overrides()
        };
        assert!(overrides.validate().is_ok());
    }

    #[cfg(feature = "layout-detection")]
    #[test]
    fn test_layout_table_model_applied() {
        let mut config = ExtractionConfig::default();
        let overrides = ExtractionOverrides {
            layout_table_model: Some("slanet_wired".to_string()),
            ..default_overrides()
        };
        overrides.apply(&mut config);
        let layout = config.layout.unwrap();
        assert_eq!(layout.table_model, xberg::TableModel::SlanetWired);
    }

    #[cfg(feature = "layout-detection")]
    #[test]
    fn test_layout_strategy_applied() {
        let mut config = ExtractionConfig::default();
        let overrides = ExtractionOverrides {
            layout_strategy: Some("auto".to_string()),
            ..default_overrides()
        };
        overrides.apply(&mut config);
        let layout = config.layout.unwrap();
        assert_eq!(layout.strategy, xberg::LayoutStrategy::Auto);
    }

    #[cfg(feature = "layout-detection")]
    #[test]
    fn test_layout_strategy_rejects_unknown_value() {
        let overrides = ExtractionOverrides {
            layout_strategy: Some("adaptive".to_string()),
            ..default_overrides()
        };
        let error = overrides.validate().expect_err("unknown strategy must fail");
        assert!(error.to_string().contains("Invalid layout strategy"));
    }

    #[cfg(feature = "layout-detection")]
    #[test]
    fn test_layout_strategy_conflicts_with_layout_false() {
        let overrides = ExtractionOverrides {
            layout: Some(false),
            layout_strategy: Some("auto".to_string()),
            ..default_overrides()
        };
        let error = overrides.validate().expect_err("conflicting flags must fail");
        assert!(error.to_string().contains("--layout-strategy"));
    }

    #[cfg(feature = "layout-detection")]
    #[test]
    fn test_layout_confidence_applied() {
        let mut config = ExtractionConfig::default();
        let overrides = ExtractionOverrides {
            layout_confidence: Some(0.7),
            ..default_overrides()
        };
        overrides.apply(&mut config);
        let layout = config.layout.unwrap();
        assert_eq!(layout.confidence_threshold, Some(0.7));
    }

    #[test]
    fn test_acceleration_applied() {
        let mut config = ExtractionConfig::default();
        let overrides = ExtractionOverrides {
            acceleration: Some(AccelerationArg::Cpu),
            ..default_overrides()
        };
        overrides.apply(&mut config);
        let accel = config.acceleration.unwrap();
        assert_eq!(accel.provider, ExecutionProviderType::Cpu);
    }

    #[test]
    fn test_extract_pages_applied() {
        let mut config = ExtractionConfig::default();
        let overrides = ExtractionOverrides {
            extract_pages: Some(true),
            page_markers: Some(true),
            ..default_overrides()
        };
        overrides.apply(&mut config);
        let pages = config.pages.unwrap();
        assert!(pages.extract_pages);
        assert!(pages.insert_page_markers);
    }

    #[test]
    fn test_extract_images_applied() {
        let mut config = ExtractionConfig::default();
        let overrides = ExtractionOverrides {
            extract_images: Some(true),
            target_dpi: Some(150),
            ..default_overrides()
        };
        overrides.apply(&mut config);
        let images = config.images.unwrap();
        assert!(images.extract_images);
        assert_eq!(images.target_dpi, 150);
    }

    #[cfg(feature = "analysis")]
    #[test]
    fn test_token_reduction_applied() {
        let mut config = ExtractionConfig::default();
        let overrides = ExtractionOverrides {
            token_reduction: Some(ReductionLevelArg::Aggressive),
            ..default_overrides()
        };
        overrides.apply(&mut config);
        let reduction = config.token_reduction.unwrap();
        assert_eq!(reduction.mode, "aggressive");
    }

    #[test]
    fn test_msg_codepage_applied() {
        let mut config = ExtractionConfig::default();
        let overrides = ExtractionOverrides {
            msg_codepage: Some(1251),
            ..default_overrides()
        };
        overrides.apply(&mut config);
        let email = config.email.unwrap();
        assert_eq!(email.msg_fallback_codepage, Some(1251));
    }

    #[test]
    fn test_max_concurrent_applied() {
        let mut config = ExtractionConfig::default();
        let overrides = ExtractionOverrides {
            max_concurrent: Some(4),
            ..default_overrides()
        };
        overrides.apply(&mut config);
        assert_eq!(config.max_concurrent_extractions, Some(4));
    }

    #[test]
    fn test_max_threads_applied() {
        let mut config = ExtractionConfig::default();
        let overrides = ExtractionOverrides {
            max_threads: Some(2),
            ..default_overrides()
        };
        overrides.apply(&mut config);
        let concurrency = config.concurrency.unwrap();
        assert_eq!(concurrency.max_threads, Some(2));
    }

    #[test]
    fn test_include_structure_applied() {
        let mut config = ExtractionConfig::default();
        assert!(!config.include_document_structure);
        let overrides = ExtractionOverrides {
            include_structure: Some(true),
            ..default_overrides()
        };
        overrides.apply(&mut config);
        assert!(config.include_document_structure);
    }

    #[cfg(feature = "ocr-surface")]
    #[test]
    fn test_validate_invalid_ocr_backend() {
        let overrides = ExtractionOverrides {
            ocr_backend: Some("invalid-backend".to_string()),
            ..default_overrides()
        };
        let err = overrides.validate().unwrap_err();
        assert!(err.to_string().contains("Invalid OCR backend"));
    }

    #[test]
    fn test_validate_max_concurrent_zero() {
        let overrides = ExtractionOverrides {
            max_concurrent: Some(0),
            ..default_overrides()
        };
        let err = overrides.validate().unwrap_err();
        assert!(err.to_string().contains("--max-concurrent must be at least 1"));
    }

    #[test]
    fn test_validate_max_threads_zero() {
        let overrides = ExtractionOverrides {
            max_threads: Some(0),
            ..default_overrides()
        };
        let err = overrides.validate().unwrap_err();
        assert!(err.to_string().contains("--max-threads must be at least 1"));
    }

    #[cfg(feature = "ocr-surface")]
    #[test]
    fn test_validate_valid_ocr_backends() {
        for backend in &["tesseract", "paddle-ocr", "sceptre"] {
            let overrides = ExtractionOverrides {
                ocr_backend: Some(backend.to_string()),
                ..default_overrides()
            };
            assert!(overrides.validate().is_ok(), "Expected backend '{backend}' to be valid");
        }
    }

    #[cfg(feature = "ocr-surface")]
    #[test]
    fn test_apply_ocr_preserves_candle_deepseek_backend() {
        let mut config = ExtractionConfig::default();
        let overrides = ExtractionOverrides {
            ocr: Some(true),
            ocr_backend: Some("candle-deepseek-ocr".to_string()),
            ..default_overrides()
        };

        overrides.apply(&mut config);

        assert_eq!(
            config.ocr.expect("OCR config should be set").backend,
            "candle-deepseek-ocr"
        );
    }

    #[cfg(feature = "ocr-surface")]
    #[test]
    fn test_validate_vlm_backend_requires_model() {
        let overrides = ExtractionOverrides {
            ocr_backend: Some("vlm".to_string()),
            ..default_overrides()
        };

        let error = overrides
            .validate()
            .expect_err("VLM backend without a model should fail");

        assert_eq!(
            error.to_string(),
            "--ocr-backend vlm requires --vlm-model to be specified"
        );
    }

    #[cfg(feature = "ocr-surface")]
    #[test]
    fn test_ocr_backend_options_threaded_into_config() {
        let mut config = ExtractionConfig::default();
        let overrides = ExtractionOverrides {
            ocr: Some(true),
            ocr_backend: Some("candle-glm-ocr".to_string()),
            ocr_backend_options: Some(r#"{"layout_mode":"whole_page"}"#.to_string()),
            ..default_overrides()
        };
        overrides.apply(&mut config);
        let ocr = config.ocr.unwrap();
        assert_eq!(ocr.backend, "candle-glm-ocr");
        let opts = ocr.backend_options.expect("backend_options should be Some");
        assert_eq!(opts, serde_json::json!({"layout_mode": "whole_page"}));
    }

    #[cfg(feature = "ocr-surface")]
    #[test]
    fn test_validate_rejects_non_object_backend_options() {
        let overrides = ExtractionOverrides {
            ocr_backend_options: Some(r#"["not","an","object"]"#.to_string()),
            ..default_overrides()
        };
        let err = overrides.validate().unwrap_err();
        assert!(
            err.to_string().contains("--ocr-backend-options must be a JSON object"),
            "unexpected error: {err}"
        );
    }

    #[cfg(feature = "ocr-surface")]
    #[test]
    fn test_validate_rejects_invalid_json_backend_options() {
        let overrides = ExtractionOverrides {
            ocr_backend_options: Some("not-json".to_string()),
            ..default_overrides()
        };
        let err = overrides.validate().unwrap_err();
        assert!(
            err.to_string().contains("invalid --ocr-backend-options JSON"),
            "unexpected error: {err}"
        );
    }

    #[cfg(feature = "ocr-surface")]
    #[test]
    fn test_ocr_backend_options_none_when_absent() {
        let mut config = ExtractionConfig::default();
        let overrides = ExtractionOverrides {
            ocr: Some(true),
            ocr_backend: Some("candle-glm-ocr".to_string()),
            ..default_overrides()
        };
        overrides.apply(&mut config);
        let ocr = config.ocr.unwrap();
        assert!(ocr.backend_options.is_none());
    }

    #[cfg(any(feature = "core-cli", feature = "analysis"))]
    #[test]
    fn test_chunk_overlap_clamped_on_existing_config() {
        let mut config = ExtractionConfig {
            chunking: Some(ChunkingConfig {
                max_characters: 800,
                overlap: 100,
                ..Default::default()
            }),
            ..Default::default()
        };
        let overrides = ExtractionOverrides {
            chunk_overlap: Some(1500),
            ..default_overrides()
        };
        overrides.apply(&mut config);
        let chunking = config.chunking.unwrap();
        assert_eq!(chunking.overlap, 800 / 4);
        assert_eq!(chunking.max_characters, 800);
    }

    #[cfg(any(feature = "core-cli", feature = "analysis"))]
    #[test]
    fn test_chunk_overlap_valid_on_existing_config() {
        let mut config = ExtractionConfig {
            chunking: Some(ChunkingConfig {
                max_characters: 800,
                overlap: 100,
                ..Default::default()
            }),
            ..Default::default()
        };
        let overrides = ExtractionOverrides {
            chunk_overlap: Some(200),
            ..default_overrides()
        };
        overrides.apply(&mut config);
        let chunking = config.chunking.unwrap();
        assert_eq!(chunking.overlap, 200);
        assert_eq!(chunking.max_characters, 800);
    }

    #[cfg(all(
        any(feature = "core-cli", feature = "analysis"),
        not(feature = "chunking-tokenizers")
    ))]
    #[test]
    fn test_validate_chunking_tokenizer_requires_feature() {
        let overrides = ExtractionOverrides {
            chunking_tokenizer: Some("Xenova/gpt-4o".to_string()),
            ..default_overrides()
        };
        let err = overrides.validate().unwrap_err();
        assert!(
            err.to_string()
                .contains("--chunking-tokenizer requires the chunking-tokenizers feature")
        );
    }

    /// Lock around the `XBERG_LLM_API_KEY` env var to keep the resolution
    /// tests deterministic in the multi-threaded test runner. Tests that touch
    /// the environment must hold this guard for their full duration.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[allow(unsafe_code)]
    fn with_env_var<R>(key: &str, value: Option<&str>, f: impl FnOnce() -> R) -> R {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let previous = std::env::var(key).ok();
        unsafe {
            match value {
                Some(v) => std::env::set_var(key, v),
                None => std::env::remove_var(key),
            }
        }
        let result = f();
        unsafe {
            match previous {
                Some(v) => std::env::set_var(key, v),
                None => std::env::remove_var(key),
            }
        }
        result
    }

    #[test]
    fn cli_flag_takes_precedence_over_env_var() {
        with_env_var("XBERG_LLM_API_KEY", Some("env-value"), || {
            let resolved = resolve_llm_api_key(Some("cli-value"));
            assert_eq!(resolved.as_deref(), Some("cli-value"));
        });
    }

    #[test]
    fn env_var_used_when_cli_flag_absent() {
        with_env_var("XBERG_LLM_API_KEY", Some("env-value"), || {
            let resolved = resolve_llm_api_key(None);
            assert_eq!(resolved.as_deref(), Some("env-value"));
        });
    }

    #[test]
    fn returns_none_when_neither_source_is_set() {
        with_env_var("XBERG_LLM_API_KEY", None, || {
            let resolved = resolve_llm_api_key(None);
            assert!(resolved.is_none());
        });
    }

    #[test]
    fn empty_cli_flag_does_not_count() {
        with_env_var("XBERG_LLM_API_KEY", Some("env-value"), || {
            let resolved = resolve_llm_api_key(Some("   "));
            assert_eq!(resolved.as_deref(), Some("env-value"));
        });
    }

    #[test]
    fn apply_llm_api_key_fills_translation_slot() {
        use xberg::core::config::{LlmConfig, TranslationConfig};
        let mut config = ExtractionConfig {
            translation: Some(TranslationConfig {
                target_lang: "de".to_string(),
                source_lang: None,
                preserve_markup: false,
                llm: LlmConfig {
                    model: "openai/gpt-4o-mini".to_string(),
                    ..Default::default()
                },
            }),
            ..Default::default()
        };
        apply_llm_api_key(&mut config, "resolved");
        let t = config.translation.unwrap();
        assert_eq!(t.llm.api_key.as_deref(), Some("resolved"));
    }

    #[test]
    fn apply_llm_api_key_preserves_existing_key() {
        use xberg::core::config::{LlmConfig, TranslationConfig};
        let mut config = ExtractionConfig {
            translation: Some(TranslationConfig {
                target_lang: "de".to_string(),
                source_lang: None,
                preserve_markup: false,
                llm: LlmConfig {
                    model: "openai/gpt-4o-mini".to_string(),
                    api_key: Some("explicit".to_string()),
                    ..Default::default()
                },
            }),
            ..Default::default()
        };
        apply_llm_api_key(&mut config, "resolved");
        let t = config.translation.unwrap();
        assert_eq!(
            t.llm.api_key.as_deref(),
            Some("explicit"),
            "explicit config keys take precedence over the resolved value"
        );
    }

    #[test]
    fn apply_llm_api_key_fills_page_classification_slot() {
        use xberg::core::config::{LlmConfig, PageClassificationConfig};
        let mut config = ExtractionConfig {
            page_classification: Some(PageClassificationConfig {
                prompt_template: None,
                labels: vec!["a".to_string()],
                multi_label: false,
                llm: LlmConfig {
                    model: "openai/gpt-4o-mini".to_string(),
                    ..Default::default()
                },
            }),
            ..Default::default()
        };
        apply_llm_api_key(&mut config, "resolved");
        let pc = config.page_classification.unwrap();
        assert_eq!(pc.llm.api_key.as_deref(), Some("resolved"));
    }

    #[test]
    fn test_no_overrides_leaves_config_unchanged() {
        let original = ExtractionConfig::default();
        let mut config = original.clone();
        let overrides = default_overrides();
        overrides.apply(&mut config);

        assert!(config.ocr.is_none());
        assert!(config.chunking.is_none());
        assert!(config.use_cache);
        assert!(config.enable_quality_processing);
        assert!(!config.force_ocr);
        assert!(config.language_detection.is_none());
        assert!(config.pages.is_none());
        assert!(config.images.is_none());
        assert!(config.token_reduction.is_none());
        assert!(config.email.is_none());
        assert!(config.acceleration.is_none());
        assert!(config.concurrency.is_none());
        assert!(!config.include_document_structure);
    }

    #[cfg(feature = "ocr-surface")]
    #[test]
    fn test_ocr_backend_options_vlm_flow() {
        let mut config = ExtractionConfig::default();
        let backend_options_json = r#"{"task":"chart","layout_mode":"whole_page"}"#;
        let overrides = ExtractionOverrides {
            vlm_model: Some("openai/gpt-4o".to_string()),
            ocr_backend_options: Some(backend_options_json.to_string()),
            ..default_overrides()
        };

        assert!(overrides.validate().is_ok());

        overrides.apply(&mut config);

        let ocr = config.ocr.expect("OCR should be configured");
        assert_eq!(ocr.backend, "vlm");

        let opts = ocr.backend_options.expect("backend_options should be Some");
        assert!(opts.is_object());
        assert_eq!(opts.get("task").and_then(|v| v.as_str()), Some("chart"));
        assert_eq!(opts.get("layout_mode").and_then(|v| v.as_str()), Some("whole_page"));

        assert!(ocr.vlm_config.is_some());
        let vlm = ocr.vlm_config.unwrap();
        assert_eq!(vlm.model, "openai/gpt-4o");
    }
}
