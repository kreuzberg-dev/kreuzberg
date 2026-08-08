//! Core extraction orchestration module.
//!
//! This module contains the main extraction logic and orchestration layer for Xberg.
//! It provides the primary entry points for bytes and URI extraction, manages the
//! extractor registry, MIME type detection, configuration, and post-processing pipeline.
//!
//! # Architecture
//!
//! The core module is responsible for:
//! - **Entry Points**: Main `extract()` and `extract_batch()` functions
//! - **Registry**: Mapping MIME types to extractors with priority-based selection
//! - **MIME Detection**: Detecting and validating MIME types from files and extensions
//! - **Pipeline**: Orchestrating post-processing steps (chunking, quality, etc.)
//! - **Configuration**: Loading and managing extraction configuration
//! - **I/O**: File reading and validation utilities
//!
//! # Example
//!
//! ```rust,no_run
//! use xberg::core::extract::extract;
//! use xberg::core::config::{ExtractInput, ExtractionConfig};
//!
//! # async fn example() -> xberg::Result<()> {
//! let config = ExtractionConfig::default();
//! let output = extract(ExtractInput::from_uri("document.pdf"), &config).await?;
//! println!("Extracted content: {}", output.results[0].content);
//! # Ok(())
//! # }
//! ```

#[cfg(feature = "tokio-runtime")]
pub mod batch_mode;
#[cfg(feature = "tokio-runtime")]
pub mod batch_optimizations;
pub mod config;
pub mod config_validation;
// Unconditional: every extractor that reports partial extraction goes through
// these helpers, not just the pdf/ocr pipelines that first needed them (#171).
pub(crate) mod diagnostics;
pub mod extract;
pub(crate) mod extractor;
pub mod formats;
#[cfg(feature = "image-encode")]
pub(crate) mod image_encode;
pub mod io;
pub mod mime;
pub(crate) mod path_resolver;
pub mod pipeline;
pub(crate) mod runtime;
#[cfg(feature = "api-types")]
pub mod server_config;
/// Split-and-extract for multi-document PDFs. Core-only, not part of the
/// binding surface (see `alef.toml` `[crates.exclude]`).
#[cfg(feature = "pdf")]
pub mod split;

#[cfg(feature = "pdf")]
pub use config::HierarchyConfig;
pub use config::{
    ChunkingConfig, EmbeddingConfig, EmbeddingModelType, ExtractionConfig, ImageExtractionConfig,
    LanguageDetectionConfig, OcrConfig, OutputFormat, PageConfig, PostProcessorConfig, TokenReductionOptions,
};
#[cfg(feature = "api-types")]
pub use server_config::ServerConfig;

#[cfg(feature = "tokio-runtime")]
pub use batch_optimizations::{BatchProcessor, BatchProcessorConfig};
#[cfg(feature = "pdf")]
pub use config::PdfConfig;
pub use extract::{extract, extract_batch};
