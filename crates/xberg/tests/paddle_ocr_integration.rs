//! Integration tests for PaddleOCR functionality.
//!
//! These tests require:
//! - Network access to download models from HuggingFace
//! - ONNX Runtime installed on the system
//!
//! Run with: `cargo test -p xberg --features paddle-ocr --test paddle_ocr_integration -- --ignored`
//!
//! Model-manager-level tests use only the public `ModelManager` surface
//! (`new`, `ensure_all_models`, `manifest`) plus documented on-disk cache
//! layout; the previous version of this suite relied on `pub(crate)` helpers
//! (`ensure_models_exist`, `ensure_v2_det_model`, `resolve_rec_model`,
//! `cache_stats`, `are_models_cached`) that the alef-migration cleanup
//! deliberately narrowed to crate-internal visibility, which the rewritten
//! suite below no longer needs.

#![cfg(feature = "paddle-ocr")]

use std::path::PathBuf;

use xberg::core::config::OcrConfig;
use xberg::paddle_ocr::{ModelManager, PaddleOcrBackend, PaddleOcrConfig};
use xberg::plugins::OcrBackend;
use xberg::types::ExtractedDocument;

/// Helper to get the test documents directory
fn test_documents_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("test_documents")
}

/// Helper to get a temporary cache directory for tests
fn test_cache_dir() -> PathBuf {
    std::env::temp_dir().join("xberg_paddle_test")
}

/// Test that model manager can download models from HuggingFace.
///
/// This test downloads actual models and verifies they are cached correctly,
/// using only the public `ModelManager` surface (`new`, `ensure_all_models`)
/// plus the documented on-disk cache layout (`v2/det/<tier>`, `v2/cls`, etc.).
/// It's ignored by default since it requires network access and a large
/// (all-tier) download.
#[tokio::test]
#[ignore = "requires network access and a large (all-tier) download"]
async fn test_model_download_from_huggingface() {
    let cache_dir = test_cache_dir();

    let _ = std::fs::remove_dir_all(&cache_dir);

    let manager = ModelManager::new(cache_dir.clone());

    let result = manager.ensure_all_models();
    assert!(result.is_ok(), "Model download failed: {:?}", result.err());

    let det_dir = cache_dir.join("v2").join("det").join("server");
    let cls_dir = cache_dir.join("v2").join("cls");
    let ori_dir = cache_dir.join("v2").join("doc_ori");

    assert!(det_dir.join("model.onnx").exists(), "Detection ONNX file not found");
    assert!(
        cls_dir.join("model.onnx").exists(),
        "Classification ONNX file not found"
    );
    assert!(
        ori_dir.join("model.onnx").exists(),
        "Document orientation ONNX file not found"
    );

    let manifest = ModelManager::manifest();
    assert!(
        manifest.len() >= 3,
        "Expected at least 3 manifest entries, got {}",
        manifest.len()
    );

    println!("Manifest entries: {}", manifest.len());
    println!("Detection model: {:?}", det_dir);
    println!("Classification model: {:?}", cls_dir);
    println!("Document orientation model: {:?}", ori_dir);
}

/// Test OCR on a simple English "Hello World" image.
///
/// This test requires ONNX Runtime and downloaded models.
#[tokio::test]
#[ignore = "requires ONNX Runtime and downloaded models"]
async fn test_ocr_hello_world_english() {
    let image_path = test_documents_dir().join("images/test_hello_world.png");
    assert!(image_path.exists(), "Test image not found: {:?}", image_path);

    let image_bytes = std::fs::read(&image_path).expect("Failed to read image");

    let config = PaddleOcrConfig::new("en").with_cache_dir(test_cache_dir());

    let backend = PaddleOcrBackend::with_config(config).expect("Failed to create backend");

    let ocr_config = OcrConfig {
        backend: "paddle-ocr".to_string(),
        language: vec!["en".to_string()],
        ..Default::default()
    };

    let result: xberg::Result<ExtractedDocument> = backend.process_image(&image_bytes, &ocr_config).await;
    assert!(result.is_ok(), "OCR failed: {:?}", result.err());

    let extraction: ExtractedDocument = result.unwrap();
    let text = extraction.content.to_lowercase();

    println!("OCR result: {}", extraction.content);

    assert!(
        text.contains("hello") || text.contains("helo"),
        "Expected 'hello' in OCR result: {}",
        text
    );
    assert!(
        text.contains("world") || text.contains("worid"),
        "Expected 'world' in OCR result: {}",
        text
    );
}

/// Requested languages the selected recognition model cannot cover must surface
/// as a `ProcessingWarning` instead of silently dropping their text (#1346),
/// and OCR metadata must report the model actually used rather than every
/// requested language.
#[tokio::test]
#[ignore = "requires ONNX Runtime and downloaded models"]
async fn test_ocr_uncovered_language_emits_warning() {
    let image_path = test_documents_dir().join("images/test_hello_world.png");
    assert!(image_path.exists(), "Test image not found: {:?}", image_path);

    let image_bytes = std::fs::read(&image_path).expect("Failed to read image");

    let config = PaddleOcrConfig::new("en").with_cache_dir(test_cache_dir());
    let backend = PaddleOcrBackend::with_config(config).expect("Failed to create backend");

    let ocr_config = OcrConfig {
        backend: "paddle-ocr".to_string(),
        language: vec!["en".to_string(), "ru".to_string()],
        ..Default::default()
    };

    let extraction: ExtractedDocument = backend
        .process_image(&image_bytes, &ocr_config)
        .await
        .expect("OCR failed");

    assert_eq!(
        extraction.processing_warnings.len(),
        1,
        "expected one uncovered-language warning, got: {:?}",
        extraction.processing_warnings
    );
    let warning = &extraction.processing_warnings[0];
    assert_eq!(warning.source, "paddle-ocr");
    assert!(
        warning.message.contains("[ru]") && warning.message.contains("'en'"),
        "warning should name the uncovered language and the selected model: {}",
        warning.message
    );

    let language = match &extraction.metadata.format {
        Some(xberg::types::FormatMetadata::Ocr(ocr)) => ocr.language.clone(),
        other => panic!("expected OCR metadata, got: {:?}", other),
    };
    assert_eq!(language, "en", "metadata must report the model actually used");
}

/// Test PP-OCRv6 recognition on English across all three v6 tiers.
///
/// v6 routes English (a v6-unified family) to the unified recognition model at the
/// configured tier (medium/small/tiny), with a v6 detector and the shared PP-LCNet
/// classifier. Verifies the version-aware wiring end to end against the live models.
#[tokio::test]
#[ignore = "requires network access and ONNX Runtime (PP-OCRv6 model download)"]
async fn test_ocr_pp_ocrv6_english_tiers() {
    let image_path = test_documents_dir().join("images/test_hello_world.png");
    assert!(image_path.exists(), "Test image not found: {:?}", image_path);
    let image_bytes = std::fs::read(&image_path).expect("Failed to read image");

    for tier in ["medium", "small", "tiny"] {
        let config = PaddleOcrConfig::new("en")
            .with_model_version("pp-ocrv6")
            .with_model_tier(tier)
            .with_cache_dir(test_cache_dir());

        let backend = PaddleOcrBackend::with_config(config).expect("Failed to create backend");

        let ocr_config = OcrConfig {
            backend: "paddle-ocr".to_string(),
            language: vec!["en".to_string()],
            ..Default::default()
        };

        let result: xberg::Result<ExtractedDocument> = backend.process_image(&image_bytes, &ocr_config).await;
        assert!(result.is_ok(), "PP-OCRv6 {tier} OCR failed: {:?}", result.err());

        let text = result.unwrap().content.to_lowercase();
        println!("PP-OCRv6 {tier} OCR result: {text}");
        assert!(
            text.contains("hello") || text.contains("helo"),
            "Expected 'hello' in PP-OCRv6 {tier} result: {text}"
        );
        assert!(
            text.contains("world") || text.contains("worid"),
            "Expected 'world' in PP-OCRv6 {tier} result: {text}"
        );
    }
}

/// Test OCR on a complex English document (newspaper).
#[tokio::test]
#[ignore = "requires ONNX Runtime and downloaded models"]
async fn test_ocr_newspaper_english() {
    let image_path = test_documents_dir().join("images/ocr_image.jpg");
    assert!(image_path.exists(), "Test image not found: {:?}", image_path);

    let image_bytes = std::fs::read(&image_path).expect("Failed to read image");

    let config = PaddleOcrConfig::new("en").with_cache_dir(test_cache_dir());

    let backend = PaddleOcrBackend::with_config(config).expect("Failed to create backend");

    let ocr_config = OcrConfig {
        backend: "paddle-ocr".to_string(),
        language: vec!["en".to_string()],
        ..Default::default()
    };

    let result: xberg::Result<ExtractedDocument> = backend.process_image(&image_bytes, &ocr_config).await;
    assert!(result.is_ok(), "OCR failed: {:?}", result.err());

    let extraction: ExtractedDocument = result.unwrap();
    let text = extraction.content.to_uppercase();

    println!(
        "OCR result (first 500 chars): {}",
        &extraction.content[..extraction.content.len().min(500)]
    );

    assert!(
        text.contains("NASDAQ") || text.contains("NASOAQ"),
        "Expected 'NASDAQ' in OCR result"
    );
    assert!(
        text.contains("AMEX") || text.contains("STOCK"),
        "Expected 'AMEX' or 'STOCK' in OCR result"
    );
}

/// Test OCR on Chinese text image.
///
/// Note: Uses per-family PP-OCRv5 recognition models.
/// This test verifies the pipeline handles non-English images without crashing,
/// but requires the Chinese recognition model to be cached for accurate results.
#[tokio::test]
#[ignore = "requires ONNX Runtime and downloaded models"]
async fn test_ocr_chinese_text() {
    let image_path = test_documents_dir().join("images/chi_sim_image.jpeg");
    assert!(image_path.exists(), "Test image not found: {:?}", image_path);

    let image_bytes = std::fs::read(&image_path).expect("Failed to read image");

    let config = PaddleOcrConfig::new("ch").with_cache_dir(test_cache_dir());

    let backend = PaddleOcrBackend::with_config(config).expect("Failed to create backend");

    let ocr_config = OcrConfig {
        backend: "paddle-ocr".to_string(),
        language: vec!["ch".to_string()],
        ..Default::default()
    };

    let result: xberg::Result<ExtractedDocument> = backend.process_image(&image_bytes, &ocr_config).await;
    assert!(result.is_ok(), "OCR failed: {:?}", result.err());

    let extraction: ExtractedDocument = result.unwrap();

    println!("OCR result: {}", extraction.content);

    assert!(
        !extraction.content.is_empty(),
        "Expected non-empty OCR result for Chinese image"
    );
}

/// Test that the backend correctly reports supported languages.
#[test]
fn test_supported_languages() {
    let backend = PaddleOcrBackend::new().expect("Failed to create backend");

    assert!(backend.supports_language("ch"));
    assert!(backend.supports_language("en"));
    assert!(backend.supports_language("japan"));
    assert!(backend.supports_language("korean"));

    assert!(backend.supports_language("chi_sim"));
    assert!(backend.supports_language("eng"));
    assert!(backend.supports_language("jpn"));
    assert!(backend.supports_language("fra"));
    assert!(backend.supports_language("deu"));

    assert!(!backend.supports_language("xyz"));
    assert!(!backend.supports_language("klingon"));
}

/// Test that empty image returns an error.
#[tokio::test]
async fn test_empty_image_error() {
    let backend = PaddleOcrBackend::new().expect("Failed to create backend");

    let ocr_config = OcrConfig {
        backend: "paddle-ocr".to_string(),
        language: vec!["en".to_string()],
        ..Default::default()
    };

    let result: xberg::Result<ExtractedDocument> = backend.process_image(&[], &ocr_config).await;
    assert!(result.is_err(), "Expected error for empty image");
}

/// Test that invalid image data returns an error (requires ONNX Runtime).
#[tokio::test]
#[ignore = "requires ONNX Runtime and downloaded models"]
async fn test_invalid_image_error() {
    let config = PaddleOcrConfig::new("en").with_cache_dir(test_cache_dir());
    let backend = PaddleOcrBackend::with_config(config).expect("Failed to create backend");

    let ocr_config = OcrConfig {
        backend: "paddle-ocr".to_string(),
        language: vec!["en".to_string()],
        ..Default::default()
    };

    let invalid_bytes = vec![0u8, 1, 2, 3, 4, 5, 6, 7, 8, 9];

    let result: xberg::Result<ExtractedDocument> = backend.process_image(&invalid_bytes, &ocr_config).await;
    assert!(result.is_err(), "Expected error for invalid image data");
}

/// Test processing an image file directly.
#[tokio::test]
#[ignore = "requires ONNX Runtime and downloaded models"]
async fn test_process_image_file() {
    let image_path = test_documents_dir().join("images/test_hello_world.png");
    assert!(image_path.exists(), "Test image not found: {:?}", image_path);

    let config = PaddleOcrConfig::new("en").with_cache_dir(test_cache_dir());
    let backend = PaddleOcrBackend::with_config(config).expect("Failed to create backend");

    let ocr_config = OcrConfig {
        backend: "paddle-ocr".to_string(),
        language: vec!["en".to_string()],
        ..Default::default()
    };

    let result: xberg::Result<ExtractedDocument> = backend.process_image_file(&image_path, &ocr_config).await;
    assert!(result.is_ok(), "OCR failed: {:?}", result.err());

    let extraction: ExtractedDocument = result.unwrap();
    let text = extraction.content.to_lowercase();

    assert!(
        text.contains("hello") || text.contains("helo"),
        "Expected 'hello' in OCR result"
    );
}

/// Test that explicit cache_dir in config overrides default.
#[test]
fn test_cache_dir_explicit_config() {
    let config = PaddleOcrConfig::new("en").with_cache_dir(PathBuf::from("/explicit/path"));
    let resolved = config.resolve_cache_dir();

    assert_eq!(resolved, PathBuf::from("/explicit/path"));
}

/// Test that OCR elements have proper geometry (quadrilateral bounding boxes).
#[tokio::test]
#[ignore = "requires ONNX Runtime and downloaded models"]
async fn test_paddle_ocr_elements_geometry() {
    use xberg::types::OcrBoundingGeometry;

    let image_path = test_documents_dir().join("images/test_hello_world.png");
    assert!(image_path.exists(), "Test image not found: {:?}", image_path);

    let image_bytes = std::fs::read(&image_path).expect("Failed to read image");

    let config = PaddleOcrConfig::new("en").with_cache_dir(test_cache_dir());
    let backend = PaddleOcrBackend::with_config(config).expect("Failed to create backend");

    let ocr_config = OcrConfig {
        backend: "paddle-ocr".to_string(),
        language: vec!["en".to_string()],
        ..Default::default()
    };

    let result: xberg::Result<ExtractedDocument> = backend.process_image(&image_bytes, &ocr_config).await;
    assert!(result.is_ok(), "OCR failed: {:?}", result.err());

    let extraction: ExtractedDocument = result.unwrap();

    assert!(
        extraction.ocr_elements.is_some(),
        "Expected ocr_elements to be populated"
    );

    let elements = extraction.ocr_elements.as_ref().unwrap();
    assert!(!elements.is_empty(), "Expected at least one OCR element");

    for element in elements {
        match &element.geometry {
            OcrBoundingGeometry::Quadrilateral { points } => {
                assert_eq!(points.len(), 4, "Quadrilateral should have 4 points");
                println!("Quadrilateral with 4 points");
            }
            OcrBoundingGeometry::Rectangle {
                left,
                top,
                width,
                height,
            } => {
                assert!(*width > 0, "Width should be positive");
                assert!(*height > 0, "Height should be positive");
                println!("Rectangle at ({}, {}) size {}x{}", left, top, width, height);
            }
        }
    }

    println!("Found {} OCR elements with valid geometry", elements.len());
}

/// Test that OCR elements have confidence scores.
#[tokio::test]
#[ignore = "requires ONNX Runtime and downloaded models"]
async fn test_paddle_ocr_elements_confidence() {
    let image_path = test_documents_dir().join("images/test_hello_world.png");
    assert!(image_path.exists(), "Test image not found: {:?}", image_path);

    let image_bytes = std::fs::read(&image_path).expect("Failed to read image");

    let config = PaddleOcrConfig::new("en").with_cache_dir(test_cache_dir());
    let backend = PaddleOcrBackend::with_config(config).expect("Failed to create backend");

    let ocr_config = OcrConfig {
        backend: "paddle-ocr".to_string(),
        language: vec!["en".to_string()],
        ..Default::default()
    };

    let result: xberg::Result<ExtractedDocument> = backend.process_image(&image_bytes, &ocr_config).await;
    assert!(result.is_ok(), "OCR failed: {:?}", result.err());

    let extraction: ExtractedDocument = result.unwrap();

    assert!(
        extraction.ocr_elements.is_some(),
        "Expected ocr_elements to be populated"
    );

    let elements = extraction.ocr_elements.as_ref().unwrap();
    assert!(!elements.is_empty(), "Expected at least one OCR element");

    for element in elements {
        assert!(
            element.confidence.recognition >= 0.0 && element.confidence.recognition <= 1.0,
            "Recognition confidence should be between 0 and 1, got {}",
            element.confidence.recognition
        );

        if let Some(det_conf) = element.confidence.detection {
            assert!(
                (0.0..=1.0).contains(&det_conf),
                "Detection confidence should be between 0 and 1, got {}",
                det_conf
            );
        }

        println!(
            "Element '{}' has recognition confidence: {:.2}%",
            element.text,
            element.confidence.recognition * 100.0
        );
    }
}

/// Test rotation detection via angle classification.
#[tokio::test]
#[ignore = "requires ONNX Runtime and downloaded models"]
async fn test_paddle_ocr_rotation_detection() {
    let image_path = test_documents_dir().join("images/ocr_image.jpg");
    assert!(image_path.exists(), "Test image not found: {:?}", image_path);

    let image_bytes = std::fs::read(&image_path).expect("Failed to read image");

    let config = PaddleOcrConfig::new("en").with_cache_dir(test_cache_dir());

    let backend = PaddleOcrBackend::with_config(config).expect("Failed to create backend");

    let ocr_config = OcrConfig {
        backend: "paddle-ocr".to_string(),
        language: vec!["en".to_string()],
        ..Default::default()
    };

    let result: xberg::Result<ExtractedDocument> = backend.process_image(&image_bytes, &ocr_config).await;
    assert!(result.is_ok(), "OCR failed: {:?}", result.err());

    let extraction: ExtractedDocument = result.unwrap();

    assert!(
        extraction.ocr_elements.is_some(),
        "Expected ocr_elements to be populated"
    );

    let elements = extraction.ocr_elements.as_ref().unwrap();

    let elements_with_rotation = elements.iter().filter(|e| e.rotation.is_some()).count();

    println!(
        "Found {} elements total, {} with rotation info",
        elements.len(),
        elements_with_rotation
    );

    for element in elements.iter().filter(|e| e.rotation.is_some()) {
        let rotation = element.rotation.as_ref().unwrap();
        assert!(
            rotation.angle_degrees >= 0.0 && rotation.angle_degrees < 360.0,
            "Rotation angle should be between 0 and 360, got {}",
            rotation.angle_degrees
        );
    }
}

/// Test table reconstruction from OCR elements.
#[tokio::test]
#[ignore = "requires ONNX Runtime and downloaded models"]
async fn test_paddle_ocr_table_reconstruction() {
    let image_path = test_documents_dir().join("images/simple_table.png");
    assert!(image_path.exists(), "Test image not found: {:?}", image_path);

    let image_bytes = std::fs::read(&image_path).expect("Failed to read image");

    let config = PaddleOcrConfig::new("en")
        .with_cache_dir(test_cache_dir())
        .with_table_detection(true);

    let backend = PaddleOcrBackend::with_config(config).expect("Failed to create backend");

    let ocr_config = OcrConfig {
        backend: "paddle-ocr".to_string(),
        language: vec!["en".to_string()],
        ..Default::default()
    };

    let result: xberg::Result<ExtractedDocument> = backend.process_image(&image_bytes, &ocr_config).await;
    assert!(result.is_ok(), "OCR failed: {:?}", result.err());

    let extraction: ExtractedDocument = result.unwrap();

    println!(
        "OCR result (first 500 chars): {}",
        &extraction.content[..extraction.content.len().min(500)]
    );

    if !extraction.tables.is_empty() {
        println!("Found {} tables", extraction.tables.len());
        for (i, table) in extraction.tables.iter().enumerate() {
            println!(
                "Table {}: {} rows x {} cols",
                i,
                table.cells.len(),
                table.cells.first().map(|r| r.len()).unwrap_or(0)
            );
        }
    }

    if let Some(elements) = &extraction.ocr_elements {
        println!("Found {} OCR elements", elements.len());

        let non_empty_elements = elements.iter().filter(|e| !e.text.is_empty()).count();
        assert!(non_empty_elements > 0, "Expected at least one element with text");
    }
}

/// Compute Text F1 score: token-level precision/recall between predicted and reference text.
fn compute_tf1(predicted: &str, reference: &str) -> f64 {
    let pred_tokens: Vec<&str> = predicted.split_whitespace().collect();
    let ref_tokens: Vec<&str> = reference.split_whitespace().collect();

    if pred_tokens.is_empty() && ref_tokens.is_empty() {
        return 1.0;
    }
    if pred_tokens.is_empty() || ref_tokens.is_empty() {
        return 0.0;
    }

    let pred_set: std::collections::HashSet<&str> = pred_tokens.iter().copied().collect();
    let ref_set: std::collections::HashSet<&str> = ref_tokens.iter().copied().collect();

    let intersection = pred_set.intersection(&ref_set).count() as f64;
    let precision = intersection / pred_set.len() as f64;
    let recall = intersection / ref_set.len() as f64;

    if precision + recall == 0.0 {
        return 0.0;
    }
    2.0 * precision * recall / (precision + recall)
}

/// Ground truth for the complex_document test image.
const COMPLEX_DOC_GT: &str = "Sales Report 2024 This report contains quarterly sales data for our products. Q1 Sales: Product Units Revenue Widget A 150 ,500 Widget B 200 ,000 Widget C 100 ,000 Q2 Sales: Product Units Revenue Widget A 180 ,000 Widget B 220 ,200 Widget C 130 ,400 Summary: Total Q1 Revenue: ,500 Total Q2 Revenue: ,600 Prepared by: John Doe Date: 2024-03-15 Department: Finance";

/// Test mobile tier OCR on a document image, measuring TF1.
#[tokio::test]
#[ignore = "requires ONNX Runtime and downloaded models"]
async fn test_mobile_tier_ocr_quality() {
    let image_path = test_documents_dir().join("images/complex_document.png");
    assert!(image_path.exists(), "Test image not found: {:?}", image_path);

    let image_bytes = std::fs::read(&image_path).expect("Failed to read image");

    let config = PaddleOcrConfig::new("en")
        .with_cache_dir(test_cache_dir())
        .with_model_tier("mobile");

    let backend = PaddleOcrBackend::with_config(config).expect("Failed to create backend");

    let ocr_config = OcrConfig {
        backend: "paddle-ocr".to_string(),
        language: vec!["en".to_string()],
        paddle_ocr_config: Some(serde_json::json!({"model_tier": "mobile"})),
        ..Default::default()
    };

    let start = std::time::Instant::now();
    let result = backend.process_image(&image_bytes, &ocr_config).await;
    let elapsed_ms = start.elapsed().as_millis();

    assert!(result.is_ok(), "Mobile tier OCR failed: {:?}", result.err());

    let extraction = result.unwrap();
    let tf1 = compute_tf1(&extraction.content, COMPLEX_DOC_GT);

    println!("Mobile tier TF1: {:.1}% ({} ms)", tf1 * 100.0, elapsed_ms);
    println!(
        "Extracted text: {}",
        &extraction.content[..extraction.content.len().min(200)]
    );

    assert!(
        tf1 > 0.5,
        "Mobile tier TF1 too low: {:.1}% (expected >50%)",
        tf1 * 100.0
    );
}

/// Test server tier OCR on the same document for comparison.
#[tokio::test]
#[ignore = "requires ONNX Runtime and downloaded models"]
async fn test_server_tier_ocr_quality() {
    let image_path = test_documents_dir().join("images/complex_document.png");
    assert!(image_path.exists(), "Test image not found: {:?}", image_path);

    let image_bytes = std::fs::read(&image_path).expect("Failed to read image");

    let config = PaddleOcrConfig::new("en")
        .with_cache_dir(test_cache_dir())
        .with_model_tier("server");

    let backend = PaddleOcrBackend::with_config(config).expect("Failed to create backend");

    let ocr_config = OcrConfig {
        backend: "paddle-ocr".to_string(),
        language: vec!["en".to_string()],
        ..Default::default()
    };

    let start = std::time::Instant::now();
    let result = backend.process_image(&image_bytes, &ocr_config).await;
    let elapsed_ms = start.elapsed().as_millis();

    assert!(result.is_ok(), "Server tier OCR failed: {:?}", result.err());

    let extraction = result.unwrap();
    let tf1 = compute_tf1(&extraction.content, COMPLEX_DOC_GT);

    println!("Server tier TF1: {:.1}% ({} ms)", tf1 * 100.0, elapsed_ms);

    assert!(
        tf1 > 0.6,
        "Server tier TF1 too low: {:.1}% (expected >60%)",
        tf1 * 100.0
    );
}

/// Test mobile tier with auto_rotate on rotated images.
/// Verifies that doc_ori detection + rotation correction produces consistent TF1.
#[tokio::test]
#[ignore = "requires ONNX Runtime and downloaded models"]
async fn test_mobile_tier_auto_rotate() {
    let base_dir = test_documents_dir().join("images");

    let test_cases = vec![
        ("complex_document.png", "original (0°)"),
        ("complex_document_rotated_90.png", "rotated 90°"),
        ("complex_document_rotated_180.png", "rotated 180°"),
        ("complex_document_rotated_270.png", "rotated 270°"),
    ];

    let config = PaddleOcrConfig::new("en")
        .with_cache_dir(test_cache_dir())
        .with_model_tier("mobile");

    let backend = PaddleOcrBackend::with_config(config).expect("Failed to create backend");

    let mut tf1_scores = Vec::new();

    for (filename, label) in &test_cases {
        let image_path = base_dir.join(filename);
        assert!(image_path.exists(), "Test image not found: {:?}", image_path);

        let image_bytes = std::fs::read(&image_path).expect("Failed to read image");

        let ocr_config = OcrConfig {
            backend: "paddle-ocr".to_string(),
            language: vec!["en".to_string()],
            auto_rotate: true,
            paddle_ocr_config: Some(serde_json::json!({"model_tier": "mobile"})),
            ..Default::default()
        };

        let start = std::time::Instant::now();
        let result = backend.process_image(&image_bytes, &ocr_config).await;
        let elapsed_ms = start.elapsed().as_millis();

        assert!(result.is_ok(), "OCR failed on {}: {:?}", label, result.err());

        let extraction = result.unwrap();
        let tf1 = compute_tf1(&extraction.content, COMPLEX_DOC_GT);
        tf1_scores.push(tf1);

        println!("{}: TF1={:.1}% ({} ms)", label, tf1 * 100.0, elapsed_ms);
    }

    let min_tf1 = tf1_scores.iter().cloned().fold(f64::INFINITY, f64::min);
    let max_tf1 = tf1_scores.iter().cloned().fold(f64::NEG_INFINITY, f64::max);

    println!(
        "TF1 range: {:.1}% - {:.1}% (spread: {:.1}%)",
        min_tf1 * 100.0,
        max_tf1 * 100.0,
        (max_tf1 - min_tf1) * 100.0
    );

    assert!(
        min_tf1 > 0.4,
        "Worst orientation TF1 too low: {:.1}% (expected >40% with auto_rotate)",
        min_tf1 * 100.0
    );

    assert!(
        max_tf1 - min_tf1 < 0.3,
        "TF1 spread too large: {:.1}% (expected <30% with auto_rotate)",
        (max_tf1 - min_tf1) * 100.0
    );
}

/// Test that mobile tier model download caches correctly.
///
/// Drives the download through the mobile-tier OCR path (`PaddleOcrBackend`
/// with `model_tier = "mobile"`) and verifies the resulting cache layout on
/// disk, since tier-specific model resolution (`ensure_v2_det_model`,
/// `resolve_rec_model`) is a crate-internal `ModelManager` detail.
#[tokio::test]
#[ignore = "requires network access"]
async fn test_mobile_tier_model_cache() {
    let cache_dir = test_cache_dir();
    let image_path = test_documents_dir().join("images/test_hello_world.png");
    assert!(image_path.exists(), "Test image not found: {:?}", image_path);
    let image_bytes = std::fs::read(&image_path).expect("Failed to read image");

    let config = PaddleOcrConfig::new("en")
        .with_cache_dir(cache_dir.clone())
        .with_model_tier("mobile");
    let backend = PaddleOcrBackend::with_config(config).expect("Failed to create backend");

    let ocr_config = OcrConfig {
        backend: "paddle-ocr".to_string(),
        language: vec!["en".to_string()],
        paddle_ocr_config: Some(serde_json::json!({"model_tier": "mobile"})),
        ..Default::default()
    };

    let result: xberg::Result<ExtractedDocument> = backend.process_image(&image_bytes, &ocr_config).await;
    assert!(result.is_ok(), "Mobile tier OCR failed: {:?}", result.err());

    let det_file = cache_dir.join("v2").join("det").join("mobile").join("model.onnx");
    assert!(det_file.exists(), "Mobile det model not cached at {:?}", det_file);

    let det_size = std::fs::metadata(&det_file).unwrap().len();
    assert!(
        det_size < 10_000_000,
        "Mobile det model too large: {} bytes (expected <10MB)",
        det_size
    );
    println!(
        "Mobile det model size: {} bytes ({:.1} MB)",
        det_size,
        det_size as f64 / 1_048_576.0
    );
}

/// Test that server and mobile tiers produce different cached model files.
#[tokio::test]
#[ignore = "requires network access"]
async fn test_tier_model_differentiation() {
    let cache_dir = test_cache_dir();
    let image_path = test_documents_dir().join("images/test_hello_world.png");
    assert!(image_path.exists(), "Test image not found: {:?}", image_path);
    let image_bytes = std::fs::read(&image_path).expect("Failed to read image");

    for tier in ["server", "mobile"] {
        let config = PaddleOcrConfig::new("en")
            .with_cache_dir(cache_dir.clone())
            .with_model_tier(tier);
        let backend = PaddleOcrBackend::with_config(config).expect("Failed to create backend");

        let ocr_config = OcrConfig {
            backend: "paddle-ocr".to_string(),
            language: vec!["en".to_string()],
            paddle_ocr_config: Some(serde_json::json!({"model_tier": tier})),
            ..Default::default()
        };

        let result: xberg::Result<ExtractedDocument> = backend.process_image(&image_bytes, &ocr_config).await;
        assert!(result.is_ok(), "{tier} tier OCR failed: {:?}", result.err());
    }

    let server_det = cache_dir.join("v2").join("det").join("server").join("model.onnx");
    let mobile_det = cache_dir.join("v2").join("det").join("mobile").join("model.onnx");
    assert!(server_det.exists(), "Server det model not cached");
    assert!(mobile_det.exists(), "Mobile det model not cached");
    assert_ne!(server_det, mobile_det, "Server and mobile det paths should differ");

    println!("Server det: {:?}", server_det);
    println!("Mobile det: {:?}", mobile_det);
}

/// Test default cache directory when no explicit config is set.
#[test]
#[allow(unsafe_code)]
fn test_cache_dir_default() {
    let original = std::env::var("XBERG_CACHE_DIR").ok();

    unsafe {
        std::env::remove_var("XBERG_CACHE_DIR");
    }

    let config = PaddleOcrConfig::new("en");
    let resolved = config.resolve_cache_dir();

    assert!(resolved.to_string_lossy().contains("xberg"));
    assert!(resolved.to_string_lossy().contains("paddle-ocr"));

    unsafe {
        if let Some(val) = original {
            std::env::set_var("XBERG_CACHE_DIR", val);
        }
    }
}
