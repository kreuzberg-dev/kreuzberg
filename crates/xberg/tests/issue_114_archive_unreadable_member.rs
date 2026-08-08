//! Regression test for #114 — an archive member that fails to read is skipped silently.
//!
//! A ZIP entry whose payload does not match its stored CRC is listed in the
//! archive index but cannot be decompressed. Both `extract_zip_text_content` and
//! `extract_zip_file_bytes` drop it with a `continue`, so it vanished from the
//! extracted text *and* from `children` while the result still reported `Ok`
//! with no warning.

#![cfg(feature = "archives")]

use std::io::{Cursor, Write};
use xberg::core::config::ExtractionConfig;
use zip::write::{FileOptions, ZipWriter};

mod helpers;
use helpers::extract_bytes_document;

const GOOD_PAYLOAD: &[u8] = b"Good content that reads back fine.";
const BROKEN_PAYLOAD: &[u8] = b"CORRUPT-ME-PLEASE";

/// Build a ZIP holding one intact member and one stored member whose bytes are
/// flipped after the fact, so its CRC no longer matches and reading it fails.
fn zip_with_one_unreadable_member() -> Vec<u8> {
    let mut cursor = Cursor::new(Vec::new());
    {
        let mut zip = ZipWriter::new(&mut cursor);
        // Stored (uncompressed) so the payload appears verbatim in the output and can
        // be corrupted below without disturbing any offset in the archive.
        let options = FileOptions::<'_, ()>::default().compression_method(zip::CompressionMethod::Stored);

        zip.start_file("good.txt", options).expect("write good.txt");
        zip.write_all(GOOD_PAYLOAD).expect("write good payload");

        zip.start_file("broken.txt", options).expect("write broken.txt");
        zip.write_all(BROKEN_PAYLOAD).expect("write broken payload");

        zip.finish().expect("finish zip");
    }

    let mut bytes = cursor.into_inner();
    let offset = bytes
        .windows(BROKEN_PAYLOAD.len())
        .position(|window| window == BROKEN_PAYLOAD)
        .expect("stored payload must appear verbatim in the ZIP");
    bytes[offset] ^= 0xFF;
    bytes
}

#[tokio::test]
async fn should_name_the_archive_member_that_could_not_be_read() {
    let bytes = zip_with_one_unreadable_member();

    let result = extract_bytes_document(&bytes, "application/zip", &ExtractionConfig::default())
        .await
        .expect("a corrupt member must not fail the whole archive");

    assert_eq!(
        result.processing_warnings.len(),
        1,
        "expected exactly one skipped-member warning, got {:?}",
        result.processing_warnings
    );
    let warning = &result.processing_warnings[0];
    assert_eq!(warning.source, "archive");
    assert_eq!(
        warning.message,
        "Skipped 1 archive entry that could not be read: broken.txt"
    );

    assert!(
        result.content.contains("Good content that reads back fine."),
        "the readable member must still be extracted: {:?}",
        result.content
    );
}

#[tokio::test]
async fn should_not_warn_when_every_archive_member_reads() {
    let mut cursor = Cursor::new(Vec::new());
    {
        let mut zip = ZipWriter::new(&mut cursor);
        let options = FileOptions::<'_, ()>::default().compression_method(zip::CompressionMethod::Stored);
        zip.start_file("good.txt", options).expect("write good.txt");
        zip.write_all(GOOD_PAYLOAD).expect("write good payload");
        zip.finish().expect("finish zip");
    }
    let bytes = cursor.into_inner();

    let result = extract_bytes_document(&bytes, "application/zip", &ExtractionConfig::default())
        .await
        .expect("intact archive extracts");

    assert!(
        result.processing_warnings.is_empty(),
        "an intact archive must produce no warnings: {:?}",
        result.processing_warnings
    );
}
