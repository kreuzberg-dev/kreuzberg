//! Regression test for #55: the embedded-image OCR merge rebuilt the backend's
//! `ExtractedDocument` field-by-field, keeping only `content` / `mime_type` /
//! `ocr_elements` and dropping everything else the backend produced — tables,
//! metadata, formulas, `llm_usage` (VLM cost data), `detected_languages` and
//! `processing_warnings`.
//!
//! Drives the real pipeline over a Markdown document with a base64 data-URI image,
//! using a stub OCR backend that populates every one of those fields. No tesseract
//! model or fixture file is involved.

#![cfg(all(feature = "ocr", feature = "tokio-runtime"))]

mod helpers;
use helpers::extract_bytes_document;

use async_trait::async_trait;
use serial_test::serial;
use std::borrow::Cow;
use std::sync::Arc;
use xberg::Result;
use xberg::core::config::{ExtractionConfig, ImageExtractionConfig, OcrConfig};
use xberg::plugins::registry::get_ocr_backend_registry;
use xberg::plugins::{OcrBackend, OcrBackendType, Plugin};
use xberg::types::{ExtractedDocument, LlmUsage, ProcessingWarning, Table};

const BACKEND_NAME: &str = "issue55_stub";
const OCR_TEXT: &str = "stub ocr text";
const TABLE_MARKDOWN: &str = "| a | b |\n| - | - |";
const WARNING_MESSAGE: &str = "stub backend warning";

/// 1x1 transparent PNG, base64-encoded.
const PNG_BASE64: &str =
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8/5+hHgAHggJ/PchI7wAAAABJRU5ErkJggg==";

/// Stub OCR backend that returns a result populating every field the merge used to drop.
struct RichOcrBackend;

impl Plugin for RichOcrBackend {
    fn name(&self) -> &str {
        BACKEND_NAME
    }

    fn version(&self) -> String {
        "1.0.0".to_string()
    }

    fn initialize(&self) -> Result<()> {
        Ok(())
    }

    fn shutdown(&self) -> Result<()> {
        Ok(())
    }
}

#[async_trait]
impl OcrBackend for RichOcrBackend {
    async fn process_image(&self, _image_bytes: &[u8], _config: &OcrConfig) -> Result<ExtractedDocument> {
        // `ExtractedDocument` has private internal fields, so it cannot be built with
        // a struct literal from outside the crate.
        let mut document = ExtractedDocument::default();
        document.content = OCR_TEXT.to_string();
        document.mime_type = Cow::Borrowed("text/plain");
        document.tables = vec![Table {
            markdown: TABLE_MARKDOWN.to_string(),
            ..Default::default()
        }];
        document.detected_languages = Some(vec!["eng".to_string()]);
        document.llm_usage = Some(vec![LlmUsage {
            model: "stub/vlm".to_string(),
            source: "image_ocr".to_string(),
            total_tokens: Some(42),
            ..Default::default()
        }]);
        document.processing_warnings = vec![ProcessingWarning {
            source: Cow::Borrowed(BACKEND_NAME),
            message: Cow::Borrowed(WARNING_MESSAGE),
        }];
        document
            .metadata
            .additional
            .insert(Cow::Borrowed("stub_confidence"), serde_json::json!(0.91));
        Ok(document)
    }

    fn supports_language(&self, _lang: &str) -> bool {
        true
    }

    fn backend_type(&self) -> OcrBackendType {
        OcrBackendType::Custom
    }

    fn supported_languages(&self) -> Vec<String> {
        vec!["eng".to_string()]
    }
}

fn install_stub_backend() {
    let registry = get_ocr_backend_registry();
    let mut reg = registry.write();
    reg.register(Arc::new(RichOcrBackend)).expect("stub backend registers");
}

fn markdown_with_data_uri_image() -> Vec<u8> {
    format!("# Title\n\nbody text\n\n![alt](data:image/png;base64,{PNG_BASE64})\n").into_bytes()
}

fn ocr_config() -> ExtractionConfig {
    ExtractionConfig {
        ocr: Some(OcrConfig {
            backend: BACKEND_NAME.to_string(),
            ..Default::default()
        }),
        images: Some(ImageExtractionConfig {
            extract_images: true,
            run_ocr_on_images: true,
            ..Default::default()
        }),
        use_cache: false,
        ..Default::default()
    }
}

#[tokio::test]
#[serial]
async fn embedded_image_ocr_result_keeps_every_backend_field() {
    install_stub_backend();

    let result = extract_bytes_document(&markdown_with_data_uri_image(), "text/markdown", &ocr_config())
        .await
        .expect("extraction succeeds");

    let images = result.images.expect("markdown data-URI image must be extracted");
    assert_eq!(images.len(), 1, "exactly one embedded image expected");
    let ocr = images[0].ocr_result.as_ref().expect("image must carry an OCR result");

    assert_eq!(ocr.content, OCR_TEXT);
    assert_eq!(ocr.mime_type.as_ref(), "text/plain");

    assert_eq!(ocr.tables.len(), 1, "OCR tables must survive the merge");
    assert_eq!(ocr.tables[0].markdown, TABLE_MARKDOWN);

    assert_eq!(
        ocr.metadata
            .additional
            .get("stub_confidence")
            .and_then(serde_json::Value::as_f64),
        Some(0.91),
        "OCR metadata must survive the merge"
    );

    assert_eq!(ocr.detected_languages.as_deref(), Some(["eng".to_string()].as_slice()));

    assert_eq!(ocr.processing_warnings.len(), 1);
    assert_eq!(ocr.processing_warnings[0].message.as_ref(), WARNING_MESSAGE);

    let usage = ocr.llm_usage.as_ref().expect("VLM cost data must survive the merge");
    assert_eq!(usage.len(), 1);
    assert_eq!(usage[0].model, "stub/vlm");
    assert_eq!(usage[0].total_tokens, Some(42));

    // Recursion guard documented on `extraction::image_ocr`: OCR output must never
    // carry nested images.
    assert!(ocr.images.is_none(), "OCR result must not carry nested images");
}
