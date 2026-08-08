//! Regression test for #115 — the archive extractor does not name the entries it skipped.
//!
//! The bookkeeping/binary filter reported only a count ("Filtered 2
//! bookkeeping/binary entries from archive children"), which tells the caller
//! that something was dropped but never which files, so there is no way to
//! decide whether the loss mattered.

#![cfg(feature = "archives")]

use std::io::{Cursor, Write};
use xberg::core::config::ExtractionConfig;
use zip::write::{FileOptions, ZipWriter};

mod helpers;
use helpers::extract_bytes_document;

fn zip_with_bookkeeping_junk() -> Vec<u8> {
    let mut cursor = Cursor::new(Vec::new());
    {
        let mut zip = ZipWriter::new(&mut cursor);
        let options = FileOptions::<'_, ()>::default();

        zip.start_file("report.txt", options).expect("write report.txt");
        zip.write_all(b"Quarterly report body.").expect("write report body");

        zip.start_file(".DS_Store", options).expect("write .DS_Store");
        zip.write_all(&[0x00, 0x00, 0x00, 0x01, b'B', b'u', b'd', b'1'])
            .expect("write ds_store body");

        zip.start_file("__pycache__/mod.cpython-311.pyc", options)
            .expect("write pyc");
        zip.write_all(&[0x42, 0x0d, 0x0d, 0x0a, 0x00, 0x00, 0x00, 0x00])
            .expect("write pyc body");

        zip.finish().expect("finish zip");
    }
    cursor.into_inner()
}

#[tokio::test]
async fn should_name_every_bookkeeping_entry_filtered_from_archive_children() {
    let bytes = zip_with_bookkeeping_junk();

    let result = extract_bytes_document(&bytes, "application/zip", &ExtractionConfig::default())
        .await
        .expect("archive with bookkeeping junk extracts");

    assert_eq!(
        result.processing_warnings.len(),
        1,
        "expected exactly one filtered-entries warning, got {:?}",
        result.processing_warnings
    );
    let warning = &result.processing_warnings[0];
    assert_eq!(warning.source, "archive");
    assert_eq!(
        warning.message,
        "Filtered 2 bookkeeping/binary entries (e.g. .DS_Store, __MACOSX, __pycache__, .pyc) \
         from archive children: .DS_Store, __pycache__/mod.cpython-311.pyc"
    );

    let children = result.children.expect("the real document must still be extracted");
    assert_eq!(children.len(), 1, "only report.txt should survive: {children:?}");
    assert_eq!(children[0].path, "report.txt");
}
