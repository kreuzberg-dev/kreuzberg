//! OCR (Optical Character Recognition) subsystem.
//!
//! This module provides OCR functionality using Tesseract as the backend.
//! It includes caching, table reconstruction, hOCR parsing, and batch processing.
//!
//! # Features
//!
//! - **Tesseract integration**: Native Tesseract backend via `xberg-tesseract`
//! - **Result caching**: Persistent cache for OCR results using file hashing
//! - **Table reconstruction**: Extract and reconstruct tables from hOCR/TSV output
//! - **hOCR to Markdown**: Convert hOCR format to clean Markdown
//! - **Batch processing**: Process multiple images efficiently
//! - **Language support**: Validate and configure Tesseract languages
//! - **PSM modes**: Support for all Tesseract Page Segmentation Modes
//!
//! # Example
//!
//! ```rust,no_run
//! use xberg::ocr::{OcrProcessor, TesseractConfig};
//!
//! # fn example() -> Result<(), xberg::ocr::error::OcrError> {
//! let processor = OcrProcessor::new(None)?;
//! let config = TesseractConfig::default();
//!
//! let image_bytes = std::fs::read("scanned.png").expect("failed to read image");
//! let result = processor.process_image(&image_bytes, &config)?;
//!
//! println!("Extracted text: {}", result.content);
//! # Ok(())
//! # }
//! ```
//!
//! # Optional Feature
//!
//! This module requires the `ocr` feature to be enabled:
//! ```toml
//! [dependencies]
//! xberg = { version = "4.0", features = ["ocr"] }
//! ```
mod backends;

// Written by OCR backends and read by shared image/PDF post-processing. The
// WASM and Sceptre-only surfaces enable `ocr-pipeline` without full native OCR. ~keep
#[cfg(feature = "ocr-pipeline")]
pub(crate) const OCR_PROCESSED_IMAGE_WIDTH_METADATA_KEY: &str = "ocr_processed_image_width";
#[cfg(feature = "ocr-pipeline")]
pub(crate) const OCR_PROCESSED_IMAGE_HEIGHT_METADATA_KEY: &str = "ocr_processed_image_height";
#[cfg(feature = "ocr-pipeline")]
#[allow(dead_code)] // Some portable OCR feature sets do not include orientation correction. ~keep
pub(crate) const OCR_ORIENTATION_DEGREES_METADATA_KEY: &str = "orientation_degrees";
#[cfg(feature = "ocr-pipeline")]
#[allow(dead_code)] // Some portable OCR feature sets do not include orientation correction. ~keep
pub(crate) const OCR_AUTO_ROTATED_METADATA_KEY: &str = "auto_rotated";

#[cfg(feature = "ocr")]
/// Persistent file-backed cache for OCR results keyed by image hash and config.
pub mod cache;
#[cfg(any(feature = "ocr", feature = "paddle-ocr"))]
/// Type conversions between internal OCR types and public API types.
pub mod conversion;
/// OCR error types.
pub mod error;
#[cfg(feature = "ocr")]
/// hOCR HTML output parser that extracts word bounding boxes and confidence scores.
pub mod hocr_parser;
/// Registry of Tesseract language codes and language-pack validation helpers.
pub mod language_registry;
#[cfg(all(
    feature = "layout-detection",
    feature = "pdf",
    any(feature = "ocr", feature = "ocr-pipeline")
))]
/// Assembles layout-detection bounding boxes with OCR word spans for region-level extraction.
pub mod layout_assembly;
#[cfg(feature = "ocr")]
/// High-level Tesseract OCR processor with caching and table reconstruction.
pub mod processor;
#[cfg(feature = "ocr")]
/// TSV and hOCR table reconstruction utilities.
pub mod table;
#[cfg(feature = "ocr")]
/// Runtime tessdata language pack download utilities.
pub mod tessdata_download;
#[cfg(feature = "ocr")]
/// Tessdata language-pack download and management utilities.
pub mod tessdata_manager;
#[cfg(feature = "ocr")]
/// Native Tesseract backend using `xberg-tesseract` (C FFI).
pub mod tesseract_backend;
#[cfg(all(feature = "ocr-wasm", not(feature = "ocr")))]
/// WebAssembly Tesseract backend using `tesseract-wasm`.
pub mod tesseract_wasm_backend;
/// OCR configuration and result types shared across all backends.
pub mod types;
#[cfg(feature = "ocr")]
/// Utility functions for OCR result hashing and formatting constants.
pub mod utils;
#[cfg(feature = "ocr")]
/// Validation helpers for language codes and Tesseract version constraints.
pub mod validation;

#[cfg(feature = "ocr")]
pub use cache::{OcrCache, OcrCacheStats};
pub use error::OcrError;
pub use language_registry::LanguageRegistry;
#[cfg(feature = "ocr")]
pub use processor::OcrProcessor;
#[cfg(feature = "ocr")]
pub use tessdata_manager::TessdataManager;
#[cfg(feature = "ocr")]
pub use tesseract_backend::TesseractBackend;
#[cfg(all(feature = "ocr-wasm", not(feature = "ocr")))]
pub use tesseract_wasm_backend::TesseractWasmBackend;
pub use types::{BatchItemResult, ExtractionResult, PSMMode, Table, TesseractConfig};
#[cfg(feature = "ocr")]
pub use utils::compute_hash;
