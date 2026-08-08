//! Regression test for #197 — the ZIP bomb validator's size accounting was bypassable.
//!
//! `ZipBombValidator::validate` summed the *declared* per-entry sizes into a `u64`
//! with an unchecked `+=`, and read those sizes with `ZipArchive::by_index`, silently
//! dropping any entry that failed to open. Both holes let a crafted archive walk past
//! `SecurityLimits::max_archive_size`:
//!
//! * ZIP64 headers carry 64-bit size fields, so two entries can declare sizes that sum
//!   to `2^64`. The accumulator wrapped back to a small value (or panicked under
//!   `debug-assertions`, which is itself a defect — extraction must never panic), and
//!   the post-loop `total_uncompressed > max_archive_size` check then passed.
//! * An entry using a compression method the `zip` crate cannot decode made
//!   `by_index` return `Err`, and the `if let Ok(file)` arm dropped it from the totals
//!   entirely while `validate` still returned `Ok(())` — the caller was told the
//!   archive was safe.
//!
//! Every archive below is assembled byte-by-byte in this file: the declared sizes are
//! deliberate lies, so no real payload (and no committed fixture) can express them.

#![cfg(feature = "archives")]

use std::io::{Cursor, Write};
use xberg::core::config::ExtractionConfig;
use zip::write::{FileOptions, ZipWriter};

mod helpers;
use helpers::extract_bytes_document;

const LOCAL_FILE_HEADER_SIGNATURE: u32 = 0x0403_4b50;
const CENTRAL_DIRECTORY_HEADER_SIGNATURE: u32 = 0x0201_4b50;
const END_OF_CENTRAL_DIRECTORY_SIGNATURE: u32 = 0x0605_4b50;

/// Tag of the ZIP64 extended information extra field (APPNOTE 4.5.3).
const ZIP64_EXTRA_FIELD_TAG: u16 = 0x0001;
/// Body length of a ZIP64 extra field carrying uncompressed + compressed sizes only.
const ZIP64_EXTRA_FIELD_BODY_LEN: u16 = 16;
/// A 32-bit size field set to this value means "the real size lives in the ZIP64 extra field".
const ZIP64_SIZE_SENTINEL: u32 = u32::MAX;

const METHOD_STORED: u16 = 0;
/// A compression method the `zip` crate has no decoder for, so `by_index` fails on it
/// while `by_index_raw` still yields the header. Real archives hit this with bzip2/zstd
/// entries in a build without those features.
const METHOD_UNSUPPORTED: u16 = 0xFFFF;

/// One central-directory entry with attacker-chosen declared sizes.
///
/// The sizes are what the central directory *claims*; the archive carries no payload at
/// all, which is exactly the point — size accounting must not trust these numbers.
struct CraftedEntry {
    name: &'static str,
    method: u16,
    declared_uncompressed_size: u64,
    declared_compressed_size: u64,
    /// Point the central-directory record at a byte offset that holds no local file
    /// header, so the entry cannot be read at all.
    corrupt_local_header_offset: bool,
}

impl CraftedEntry {
    fn stored(name: &'static str, declared_uncompressed_size: u64, declared_compressed_size: u64) -> Self {
        Self {
            name,
            method: METHOD_STORED,
            declared_uncompressed_size,
            declared_compressed_size,
            corrupt_local_header_offset: false,
        }
    }
}

/// Assemble a syntactically valid ZIP whose central directory declares the given sizes.
///
/// Each entry gets an empty local file header (no payload), so the archive stays a few
/// hundred bytes no matter how large the declared sizes are.
fn build_crafted_zip(entries: &[CraftedEntry]) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    let mut local_header_offsets: Vec<u32> = Vec::new();

    for entry in entries {
        local_header_offsets.push(u32::try_from(out.len()).expect("crafted archive stays small"));
        let name_len = u16::try_from(entry.name.len()).expect("entry name fits in u16");

        out.extend_from_slice(&LOCAL_FILE_HEADER_SIGNATURE.to_le_bytes());
        out.extend_from_slice(&20u16.to_le_bytes()); // version needed to extract
        out.extend_from_slice(&0u16.to_le_bytes()); // general purpose flags
        out.extend_from_slice(&entry.method.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // last mod time
        out.extend_from_slice(&0u16.to_le_bytes()); // last mod date
        out.extend_from_slice(&0u32.to_le_bytes()); // crc32
        out.extend_from_slice(&0u32.to_le_bytes()); // compressed size (no payload)
        out.extend_from_slice(&0u32.to_le_bytes()); // uncompressed size (no payload)
        out.extend_from_slice(&name_len.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // extra field length
        out.extend_from_slice(entry.name.as_bytes());
    }

    let central_directory_offset = u32::try_from(out.len()).expect("crafted archive stays small");

    for (index, entry) in entries.iter().enumerate() {
        let needs_zip64 = entry.declared_uncompressed_size > u64::from(ZIP64_SIZE_SENTINEL)
            || entry.declared_compressed_size > u64::from(ZIP64_SIZE_SENTINEL);

        let (compressed_32, uncompressed_32, extra_field) = if needs_zip64 {
            let mut extra_field = Vec::with_capacity(20);
            extra_field.extend_from_slice(&ZIP64_EXTRA_FIELD_TAG.to_le_bytes());
            extra_field.extend_from_slice(&ZIP64_EXTRA_FIELD_BODY_LEN.to_le_bytes());
            // APPNOTE order: original (uncompressed) size first, compressed size second.
            extra_field.extend_from_slice(&entry.declared_uncompressed_size.to_le_bytes());
            extra_field.extend_from_slice(&entry.declared_compressed_size.to_le_bytes());
            (ZIP64_SIZE_SENTINEL, ZIP64_SIZE_SENTINEL, extra_field)
        } else {
            (
                u32::try_from(entry.declared_compressed_size).expect("fits without zip64"),
                u32::try_from(entry.declared_uncompressed_size).expect("fits without zip64"),
                Vec::new(),
            )
        };

        let local_header_offset = if entry.corrupt_local_header_offset {
            // One byte into the first local file header: the magic no longer matches.
            1
        } else {
            local_header_offsets[index]
        };
        let name_len = u16::try_from(entry.name.len()).expect("entry name fits in u16");
        let extra_len = u16::try_from(extra_field.len()).expect("extra field fits in u16");

        out.extend_from_slice(&CENTRAL_DIRECTORY_HEADER_SIGNATURE.to_le_bytes());
        out.extend_from_slice(&20u16.to_le_bytes()); // version made by
        out.extend_from_slice(&45u16.to_le_bytes()); // version needed to extract (zip64)
        out.extend_from_slice(&0u16.to_le_bytes()); // general purpose flags
        out.extend_from_slice(&entry.method.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // last mod time
        out.extend_from_slice(&0u16.to_le_bytes()); // last mod date
        out.extend_from_slice(&0u32.to_le_bytes()); // crc32
        out.extend_from_slice(&compressed_32.to_le_bytes());
        out.extend_from_slice(&uncompressed_32.to_le_bytes());
        out.extend_from_slice(&name_len.to_le_bytes());
        out.extend_from_slice(&extra_len.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // file comment length
        out.extend_from_slice(&0u16.to_le_bytes()); // disk number start
        out.extend_from_slice(&0u16.to_le_bytes()); // internal file attributes
        out.extend_from_slice(&0u32.to_le_bytes()); // external file attributes
        out.extend_from_slice(&local_header_offset.to_le_bytes());
        out.extend_from_slice(entry.name.as_bytes());
        out.extend_from_slice(&extra_field);
    }

    let central_directory_size =
        u32::try_from(out.len()).expect("crafted archive stays small") - central_directory_offset;
    let entry_count = u16::try_from(entries.len()).expect("crafted archive has few entries");

    out.extend_from_slice(&END_OF_CENTRAL_DIRECTORY_SIGNATURE.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // number of this disk
    out.extend_from_slice(&0u16.to_le_bytes()); // disk with the central directory
    out.extend_from_slice(&entry_count.to_le_bytes());
    out.extend_from_slice(&entry_count.to_le_bytes());
    out.extend_from_slice(&central_directory_size.to_le_bytes());
    out.extend_from_slice(&central_directory_offset.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // ZIP file comment length

    out
}

/// Two ZIP64 entries whose declared uncompressed sizes sum to exactly `2^64`.
///
/// `1000 + (u64::MAX - 999) == 2^64`, so an unchecked `total_uncompressed += size`
/// wraps the running total to `0`; `total_compressed` wraps to `0` alongside it, which
/// also disables the aggregate ratio check. Each entry on its own declares a 1:1 ratio,
/// so the per-entry compression-ratio check never fires. With saturating arithmetic the
/// total pins at `u64::MAX` and the archive is refused.
#[tokio::test]
async fn should_refuse_archive_whose_declared_sizes_wrap_the_size_accumulator() {
    let wrapping_size = u64::MAX - 999;
    let bytes = build_crafted_zip(&[
        CraftedEntry::stored("small.bin", 1000, 1000),
        CraftedEntry::stored("wrap.bin", wrapping_size, wrapping_size),
    ]);

    assert_eq!(
        1000u64.wrapping_add(wrapping_size),
        0,
        "the crafted sizes must be the ones that wrap a u64 total"
    );

    let error = extract_bytes_document(&bytes, "application/zip", &ExtractionConfig::default())
        .await
        .expect_err("an archive declaring 2^64 bytes of output must be refused, not truncated");

    let message = error.to_string();
    assert!(
        message.contains("Archive too large: 18446744073709551615 bytes (max: 524288000 bytes)"),
        "expected the saturated total to be reported against the default 500 MiB cap, got: {message}"
    );
    assert_eq!(
        xberg::SecurityLimits::default().max_archive_size,
        524_288_000,
        "the cap quoted in the assertion above is SecurityLimits::default().max_archive_size"
    );
}

/// An entry the `zip` crate cannot build a decoder for used to disappear from the
/// totals. It declares 600 MiB, which alone exceeds the 500 MiB cap.
#[tokio::test]
async fn should_count_archive_entry_with_unsupported_compression_method_towards_the_size_cap() {
    let opaque_size = 629_145_600; // 600 MiB, declared 1:1 so the ratio check stays quiet.
    let bytes = build_crafted_zip(&[
        CraftedEntry::stored("readable.txt", 5, 5),
        CraftedEntry {
            name: "opaque.bin",
            method: METHOD_UNSUPPORTED,
            declared_uncompressed_size: opaque_size,
            declared_compressed_size: opaque_size,
            corrupt_local_header_offset: false,
        },
    ]);

    let error = extract_bytes_document(&bytes, "application/zip", &ExtractionConfig::default())
        .await
        .expect_err("an entry that cannot be decompressed must still be counted, not skipped");

    let message = error.to_string();
    assert!(
        message.contains("Archive too large: 629145605 bytes (max: 524288000 bytes)"),
        "expected 5 + 629145600 bytes to be counted against the cap, got: {message}"
    );
}

/// An entry whose central-directory record points at no local file header cannot be
/// accounted for at all. That must be reported, not swallowed into a passing validation.
#[tokio::test]
async fn should_report_archive_entry_whose_header_cannot_be_read() {
    let bytes = build_crafted_zip(&[
        CraftedEntry::stored("readable.txt", 5, 5),
        CraftedEntry {
            name: "broken.bin",
            method: METHOD_STORED,
            declared_uncompressed_size: 10,
            declared_compressed_size: 10,
            corrupt_local_header_offset: true,
        },
    ]);

    let error = extract_bytes_document(&bytes, "application/zip", &ExtractionConfig::default())
        .await
        .expect_err("an unreadable entry must surface, not be dropped from the accounting");

    let message = error.to_string();
    assert!(
        message.contains("Archive entry 1 could not be read for security accounting"),
        "expected the unreadable entry to be named by index, got: {message}"
    );
}

/// A zero compressed size with a non-zero uncompressed size is not a compression ratio,
/// it is a forged header — no compressor emits output from no input. The old code took
/// the `compressed_size > 0` branch as a precondition for the ratio check, so such an
/// entry skipped the check entirely.
#[tokio::test]
async fn should_reject_archive_entry_declaring_output_from_zero_compressed_bytes() {
    let bytes = build_crafted_zip(&[CraftedEntry::stored("lie.bin", 1_000_000, 0)]);

    let error = extract_bytes_document(&bytes, "application/zip", &ExtractionConfig::default())
        .await
        .expect_err("a zero-byte entry claiming a megabyte of output must be refused");

    let message = error.to_string();
    assert!(
        message.contains("compressed 0B -> uncompressed 1000000B"),
        "expected the forged entry to be reported as a bomb, got: {message}"
    );
}

/// Guard against the fix over-blocking: an ordinary archive written by a real ZIP writer
/// must still validate and extract.
#[tokio::test]
async fn should_still_accept_an_ordinary_archive() {
    let mut cursor = Cursor::new(Vec::new());
    {
        let mut zip = ZipWriter::new(&mut cursor);
        let options = FileOptions::<'_, ()>::default();
        zip.start_file("notes.txt", options).expect("write notes.txt");
        zip.write_all(b"Ordinary archive content.").expect("write payload");
        zip.finish().expect("finish zip");
    }
    let bytes = cursor.into_inner();

    let result = extract_bytes_document(&bytes, "application/zip", &ExtractionConfig::default())
        .await
        .expect("a well-formed archive must keep extracting");

    assert!(
        result.content.contains("Ordinary archive content."),
        "expected the payload to survive validation, got: {:?}",
        result.content
    );
}
