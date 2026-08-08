#![cfg(all(feature = "qr-codes", feature = "chunking"))]
//! Regression tests for #302: decoded QR payloads must participate in the
//! downstream enrichment stages (chunking, and therefore embeddings, which are
//! generated per chunk inside `execute_chunking`) exactly like ordinary
//! extracted text.
//!
//! Ground truth before the fix: the `qr-codes` post-processor appended a
//! "QR Codes" section to `ExtractedDocument::content` at the `Middle` stage,
//! but `apply_output_format` (`core/pipeline/format.rs:59`) replaces `content`
//! with `formatted_content` after that stage. For every output format other
//! than `Plain` the appended section was therefore discarded before the final
//! `execute_chunking` call (`core/pipeline/mod.rs:450`) ran — the payload
//! reached neither `content`, nor `chunks`, nor embeddings.
//!
//! The QR fixture is generated in-process with the `qrcode` dev-dependency;
//! `test_documents/` carries no QR image.

use std::io::Cursor;

use bytes::Bytes;
use image::{ExtendedColorType, ImageEncoder, Luma};
use qrcode::QrCode;
use xberg::core::config::{ChunkingConfig, ExtractionConfig, OutputFormat};
use xberg::core::pipeline::run_pipeline;
use xberg::types::internal::InternalDocument;
use xberg::types::{ExtractedDocument, ExtractedImage, UriKind};

const PAYLOAD: &str = "https://xberg.io/issue-302-qr-payload";
const PROSE: &str = "Ticket stub.";

fn render_qr_png(payload: &[u8]) -> Vec<u8> {
    let code = QrCode::new(payload).expect("failed to build QR code");
    let image = code.render::<Luma<u8>>().min_dimensions(256, 256).build();

    let mut buf = Cursor::new(Vec::<u8>::new());
    image::codecs::png::PngEncoder::new(&mut buf)
        .write_image(image.as_raw(), image.width(), image.height(), ExtendedColorType::L8)
        .expect("PNG encode failed");
    buf.into_inner()
}

/// Build an `InternalDocument` carrying `image_bytes` as its only image and
/// `text` as its only text.
///
/// The text is supplied through `pre_rendered_content` (with a matching
/// `metadata.output_format`) rather than through elements so that both the
/// plain `content` and the markdown `formatted_content` produced by
/// `derive_extraction_result` are byte-exact and independent of the renderers.
fn document_with_image(text: &str, image_bytes: Vec<u8>) -> InternalDocument {
    let mut doc = InternalDocument {
        source_format: "png".to_string(),
        mime_type: "image/png".to_string(),
        pre_rendered_content: Some(text.to_string()),
        ..Default::default()
    };
    doc.metadata.output_format = Some("markdown".to_string());
    doc.images = vec![ExtractedImage {
        data: Bytes::from(image_bytes),
        format: std::borrow::Cow::Borrowed("png"),
        ..Default::default()
    }];
    doc
}

fn config_for(output_format: OutputFormat) -> ExtractionConfig {
    ExtractionConfig {
        qr_codes: Some(true),
        output_format,
        chunking: Some(ChunkingConfig::default()),
        ..Default::default()
    }
}

fn chunk_contents(result: &ExtractedDocument) -> Vec<String> {
    result
        .chunks
        .as_ref()
        .expect("chunking was configured, so chunks must be populated")
        .iter()
        .map(|chunk| chunk.content.clone())
        .collect()
}

/// The headline defect: with a non-`Plain` output format the decoded payload
/// never reached `chunks`, because the QR section was appended to `content`
/// only and `content` is overwritten by `formatted_content` afterwards.
#[tokio::test]
async fn markdown_output_chunks_contain_decoded_qr_payload() {
    let doc = document_with_image(PROSE, render_qr_png(PAYLOAD.as_bytes()));
    let result = run_pipeline(doc, &config_for(OutputFormat::Markdown))
        .await
        .expect("pipeline must succeed");

    let expected_text = format!("{PROSE}\n\n## QR Codes\n\n- {PAYLOAD}");
    assert_eq!(result.content, format!("{expected_text}\n"));
    assert_eq!(chunk_contents(&result), vec![expected_text]);
}

/// A document whose only text lives inside the QR code must still produce a
/// chunk, and that chunk must carry the exact decoded payload.
#[tokio::test]
async fn qr_only_document_produces_one_chunk_with_the_payload() {
    let doc = document_with_image("", render_qr_png(PAYLOAD.as_bytes()));
    let result = run_pipeline(doc, &config_for(OutputFormat::Markdown))
        .await
        .expect("pipeline must succeed");

    let expected_text = format!("## QR Codes\n\n- {PAYLOAD}");
    assert_eq!(result.content, format!("{expected_text}\n"));
    assert_eq!(chunk_contents(&result), vec![expected_text]);
}

/// The `Plain` path (which never lost the section) must keep producing exactly
/// one copy — appending to both `content` and `formatted_content` must not
/// double up when `formatted_content` is `None`.
#[tokio::test]
async fn plain_output_chunks_contain_the_payload_exactly_once() {
    let doc = document_with_image(PROSE, render_qr_png(PAYLOAD.as_bytes()));
    let result = run_pipeline(doc, &config_for(OutputFormat::Plain))
        .await
        .expect("pipeline must succeed");

    let expected_text = format!("{PROSE}\n\n## QR Codes\n\n- {PAYLOAD}");
    assert_eq!(result.content, format!("{expected_text}\n"));
    assert_eq!(chunk_contents(&result), vec![expected_text]);
}

/// No duplication: the payload must appear exactly once in the returned
/// content, once across all chunks, and once in the URI list — never twice
/// because it was appended to `content` and to `formatted_content`.
#[tokio::test]
async fn payload_is_not_duplicated_across_content_chunks_and_uris() {
    let doc = document_with_image(PROSE, render_qr_png(PAYLOAD.as_bytes()));
    let result = run_pipeline(doc, &config_for(OutputFormat::Markdown))
        .await
        .expect("pipeline must succeed");

    assert_eq!(result.content.matches(PAYLOAD).count(), 1);
    assert_eq!(result.content.matches("## QR Codes").count(), 1);

    let occurrences_in_chunks: usize = chunk_contents(&result)
        .iter()
        .map(|chunk| chunk.matches(PAYLOAD).count())
        .sum();
    assert_eq!(occurrences_in_chunks, 1);

    let uris = result.uris.as_ref().expect("a URL payload must reach the URI list");
    assert_eq!(uris.len(), 1);
    assert_eq!(uris[0].url, PAYLOAD);
    assert_eq!(uris[0].kind, UriKind::Hyperlink);
    assert_eq!(uris[0].label, None);
}

/// A URL payload must be routed into the document's existing URI collection
/// rather than a parallel list, and must not be added twice when the same URL
/// was already collected as a hyperlink during extraction.
#[tokio::test]
async fn url_payload_is_deduplicated_against_already_collected_uris() {
    let mut doc = document_with_image(PROSE, render_qr_png(PAYLOAD.as_bytes()));
    doc.push_uri(xberg::types::ExtractedUri {
        url: PAYLOAD.to_string(),
        label: Some("Buy tickets".to_string()),
        page: None,
        kind: UriKind::Hyperlink,
    });

    let result = run_pipeline(doc, &config_for(OutputFormat::Markdown))
        .await
        .expect("pipeline must succeed");

    let uris = result.uris.as_ref().expect("the seeded URI must survive");
    assert_eq!(uris.len(), 1, "QR payload must not re-add an existing URI: {uris:?}");
    assert_eq!(uris[0].label, Some("Buy tickets".to_string()));
}

/// A payload that is not a URL stays text-only: it reaches chunks but must not
/// be pushed into the URI list.
#[tokio::test]
async fn non_url_payload_reaches_chunks_but_not_the_uri_list() {
    const TEXT_PAYLOAD: &str = "SEAT 14C ROW 7";

    let doc = document_with_image(PROSE, render_qr_png(TEXT_PAYLOAD.as_bytes()));
    let result = run_pipeline(doc, &config_for(OutputFormat::Markdown))
        .await
        .expect("pipeline must succeed");

    assert_eq!(
        chunk_contents(&result),
        vec![format!("{PROSE}\n\n## QR Codes\n\n- {TEXT_PAYLOAD}")]
    );
    assert_eq!(result.uris, None);
}

/// An image that cannot be decoded at all must not fail the extraction, must
/// not add a QR section, and must leave the surrounding content and its chunks
/// untouched (partial results preserved).
#[tokio::test]
async fn undecodable_image_leaves_content_and_chunks_untouched() {
    let doc = document_with_image(PROSE, b"this is not an image".to_vec());
    let result = run_pipeline(doc, &config_for(OutputFormat::Markdown))
        .await
        .expect("a garbage image must not fail extraction");

    assert_eq!(result.content, PROSE);
    assert_eq!(chunk_contents(&result), vec![PROSE.to_string()]);
    assert_eq!(
        result
            .processing_warnings
            .iter()
            .filter(|warning| warning.source == "qr-codes")
            .count(),
        0
    );
}

/// A QR code whose payload is not valid UTF-8 must warn rather than fail, and
/// the lossily-decoded payload must still be preserved in the content.
#[tokio::test]
async fn non_utf8_payload_warns_and_preserves_the_partial_result() {
    let doc = document_with_image(PROSE, render_qr_png(&[0xff, 0xfe, 0xfd]));
    let result = run_pipeline(doc, &config_for(OutputFormat::Markdown))
        .await
        .expect("a lossy payload must not fail extraction");

    let warnings: Vec<&str> = result
        .processing_warnings
        .iter()
        .filter(|warning| warning.source == "qr-codes")
        .map(|warning| warning.message.as_ref())
        .collect();
    assert_eq!(warnings.len(), 1, "expected exactly one qr-codes warning: {warnings:?}");
    assert!(
        warnings[0].contains("not valid UTF-8"),
        "warning must name the cause: {}",
        warnings[0]
    );

    assert_eq!(
        chunk_contents(&result),
        vec![format!("{PROSE}\n\n## QR Codes\n\n- \u{fffd}\u{fffd}\u{fffd}")]
    );
    assert_eq!(result.uris, None);
}
