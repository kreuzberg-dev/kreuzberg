//! #184: the Tesseract OCR path must never emit `script_name` /
//! `script_confidence` metadata.
//!
//! Orientation detection runs on the ONNX PP-LCNet classifier, which detects
//! *rotation only*. Tesseract's own `DetectOrientationScript()`/OSD — the one
//! API that could supply a script name — is never called. The old code
//! published a script name anyway, so callers saw a fabricated value dressed
//! up as a real detection. #184 was resolved as "stop emitting", and the only
//! thing that kept it closed was a source comment; this test is the guard.
//!
//! The assertions are deliberately negative, so the test is paired with a
//! positive guard that the orientation branch (where the fabricated value used
//! to be inserted) actually executed. Without that guard a regression could
//! hide behind orientation detection silently not running at all.

#![cfg(all(feature = "ocr", feature = "auto-rotate"))]

mod helpers;
use helpers::*;
use xberg::core::config::{ExtractionConfig, OcrConfig, OutputFormat};

fn auto_rotate_config() -> ExtractionConfig {
    ExtractionConfig {
        output_format: OutputFormat::Plain,
        ocr: Some(OcrConfig {
            backend: "tesseract".to_string(),
            language: vec!["eng".to_string()],
            auto_rotate: true,
            ..Default::default()
        }),
        force_ocr: false,
        ..Default::default()
    }
}

#[test]
fn should_not_emit_script_metadata_when_auto_rotate_runs_orientation_detection() {
    if skip_if_missing("images/test_hello_world.png") {
        return;
    }
    let file_path = get_test_file_path("images/test_hello_world.png");
    let result = extract_uri_document_blocking(&file_path, None, &auto_rotate_config())
        .expect("should extract test_hello_world.png with auto_rotate enabled");

    let additional = &result.metadata.additional;

    // Guard: prove the orientation branch ran. `orientation_degrees` is written
    // in the exact block that used to also write `script_name`, so if this key
    // is missing the negative assertions below would be vacuous.
    assert!(
        additional.contains_key("orientation_degrees"),
        "orientation detection did not run (is the PP-LCNet model available?); \
         without it the script_name assertions below prove nothing. keys: {:?}",
        additional.keys().collect::<Vec<_>>()
    );
    assert!(
        additional.contains_key("orientation_confidence"),
        "auto_rotate must publish the orientation confidence alongside the degrees"
    );

    assert_eq!(
        additional.get("script_name"),
        None,
        "script_name is fabricated — PP-LCNet detects rotation only and Tesseract OSD is never \
         called — so it must not be emitted (#184); got {:?}",
        additional.get("script_name")
    );
    assert_eq!(
        additional.get("script_confidence"),
        None,
        "script_confidence has no real detection behind it either (#184); got {:?}",
        additional.get("script_confidence")
    );

    let script_keys: Vec<&str> = additional
        .keys()
        .map(|key| &**key)
        .filter(|key: &&str| key.starts_with("script"))
        .collect();
    assert!(
        script_keys.is_empty(),
        "no script-derived OCR metadata may be published under any name (#184), found: {:?}",
        script_keys
    );
}

/// The same guarantee on the ordinary (no auto-rotate) path: `script_name` must
/// not reappear anywhere in OCR metadata.
#[test]
fn should_not_emit_script_metadata_when_auto_rotate_is_disabled() {
    if skip_if_missing("images/test_hello_world.png") {
        return;
    }
    let config = ExtractionConfig {
        output_format: OutputFormat::Plain,
        ocr: Some(OcrConfig {
            backend: "tesseract".to_string(),
            language: vec!["eng".to_string()],
            ..Default::default()
        }),
        force_ocr: false,
        ..Default::default()
    };
    let file_path = get_test_file_path("images/test_hello_world.png");
    let result =
        extract_uri_document_blocking(&file_path, None, &config).expect("should extract test_hello_world.png with OCR");

    let script_keys: Vec<&str> = result
        .metadata
        .additional
        .keys()
        .map(|key| &**key)
        .filter(|key: &&str| key.starts_with("script"))
        .collect();
    assert!(
        script_keys.is_empty(),
        "no script-derived OCR metadata may be published (#184), found: {:?}",
        script_keys
    );
}
