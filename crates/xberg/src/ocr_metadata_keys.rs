//! Canonical names of the OCR metadata keys written into `Metadata::additional`.
//!
//! These strings are part of the public extraction output — consumers read them
//! off `Metadata::additional` — so a rename is a silent breaking change. They
//! live in an ungated crate-root module on purpose: producers and consumers sit
//! in different feature domains (native OCR under `ocr`/`ocr-wasm`, the portable
//! pipeline under `ocr-pipeline`, VLM OCR under liter-llm, the `candle-*`
//! backends, Sceptre, Paddle, and PDF layout under `layout-detection`, which
//! needs no OCR backend at all) and no single feature is implied by all of them.
//! Gating this module on any of them would force back the per-module copies of
//! the same literals that it exists to remove. ~keep

// No feature combination reads every key (builds without orientation correction
// never touch the rotation keys, for instance), so an unreferenced constant here
// is expected rather than dead. ~keep
#![allow(dead_code)]

/// Pixel width of the image actually handed to the OCR backend, after preprocessing.
pub(crate) const OCR_PROCESSED_IMAGE_WIDTH_METADATA_KEY: &str = "ocr_processed_image_width";
/// Pixel height of the image actually handed to the OCR backend, after preprocessing.
pub(crate) const OCR_PROCESSED_IMAGE_HEIGHT_METADATA_KEY: &str = "ocr_processed_image_height";
/// Clockwise rotation, in degrees, that orientation detection reported for the page.
pub(crate) const OCR_ORIENTATION_DEGREES_METADATA_KEY: &str = "orientation_degrees";
/// Confidence of the orientation detection that produced the degrees value.
pub(crate) const OCR_ORIENTATION_CONFIDENCE_METADATA_KEY: &str = "orientation_confidence";
/// Set when the pipeline actually rotated the image before running OCR.
pub(crate) const OCR_AUTO_ROTATED_METADATA_KEY: &str = "auto_rotated";
