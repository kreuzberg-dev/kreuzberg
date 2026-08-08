#![cfg(feature = "qr-codes")]
//! Regression test for #156: a decoded QR payload must reach document
//! `content`. Previously the payload was written only to
//! `ExtractedImage::qr_codes`, and nothing under `rendering/` read that field,
//! so the text never surfaced anywhere a consumer (search, chunking,
//! embeddings) could see it.

use std::borrow::Cow;
use std::io::Cursor;

use bytes::Bytes;
use image::{ExtendedColorType, ImageEncoder, Luma};
use qrcode::QrCode;
use xberg::core::config::ExtractionConfig;
use xberg::plugins::PostProcessor;
use xberg::plugins::processor::builtin::qr::QrCodeProcessor;
use xberg::types::{ExtractedDocument, ExtractedImage};

const PAYLOAD: &str = "https://xberg.io/qr-content-test";

fn render_qr_png(payload: &str) -> Vec<u8> {
    let code = QrCode::new(payload.as_bytes()).expect("failed to build QR code");
    let image = code.render::<Luma<u8>>().min_dimensions(256, 256).build();

    let mut buf = Cursor::new(Vec::<u8>::new());
    let encoder = image::codecs::png::PngEncoder::new(&mut buf);
    encoder
        .write_image(image.as_raw(), image.width(), image.height(), ExtendedColorType::L8)
        .expect("PNG encode failed");
    buf.into_inner()
}

#[tokio::test]
async fn decoded_qr_payload_reaches_document_content() {
    let png = render_qr_png(PAYLOAD);
    let mut result = ExtractedDocument::default();
    result.mime_type = Cow::Borrowed("application/octet-stream");
    result.content = "Original document text.".to_string();
    result.images = Some(vec![ExtractedImage {
        data: Bytes::from(png),
        format: Cow::Borrowed("png"),
        ..Default::default()
    }]);
    let config = ExtractionConfig {
        qr_codes: Some(true),
        ..Default::default()
    };

    QrCodeProcessor
        .process(&mut result, &config)
        .await
        .expect("qr post-processor failed");

    assert!(
        result.content.contains("Original document text."),
        "existing content must be preserved: {}",
        result.content
    );
    assert!(
        result.content.contains(PAYLOAD),
        "decoded QR payload must be present in content, got: {}",
        result.content
    );
}

/// When no QR codes are found, content must be left untouched.
#[tokio::test]
async fn no_qr_codes_leaves_content_unchanged() {
    let mut result = ExtractedDocument::default();
    result.mime_type = Cow::Borrowed("application/octet-stream");
    result.content = "Original document text.".to_string();
    result.images = Some(vec![ExtractedImage {
        data: Bytes::from_static(&[]),
        format: Cow::Borrowed("png"),
        ..Default::default()
    }]);
    let config = ExtractionConfig {
        qr_codes: Some(true),
        ..Default::default()
    };

    QrCodeProcessor
        .process(&mut result, &config)
        .await
        .expect("qr post-processor failed");

    assert_eq!(result.content, "Original document text.");
}
