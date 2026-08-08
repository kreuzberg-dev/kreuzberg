//! Regression tests for #157: EXIF metadata extraction was missing several
//! commonly-used tags (Artist, UserComment, and the Windows "XP" tag family),
//! silently swallowed parse failures without surfacing a `ProcessingWarning`,
//! and did nothing with an image's embedded XMP packet even though it often
//! carries Dublin Core title/description/keyword fields EXIF doesn't have.
//!
//! These tests drive the real extraction pipeline (`extract_bytes_document_blocking`)
//! over hand-built PNG/JPEG byte streams so the assertions exercise the same
//! code path production traffic does, rather than calling `pub(crate)` helpers
//! directly (which integration tests cannot see).

#![cfg(feature = "ocr")]

mod helpers;
use helpers::{extract_bytes_document_blocking, get_test_file_path, skip_if_missing};

use xberg::core::config::ExtractionConfig;
use xberg::types::FormatMetadata;

/// Standard PNG file signature.
const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";

/// IEEE CRC-32 (the checksum PNG chunks use), computed bit-by-bit — no need
/// to pull in a CRC crate for an 8-byte-at-a-time test helper.
fn crc32(bytes: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &byte in bytes {
        crc ^= byte as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

/// Encode one PNG chunk: 4-byte big-endian length, 4-byte type, data, then a
/// CRC-32 over `type || data`.
fn png_chunk(chunk_type: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(12 + data.len());
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(chunk_type);
    out.extend_from_slice(data);
    let mut crc_input = Vec::with_capacity(4 + data.len());
    crc_input.extend_from_slice(chunk_type);
    crc_input.extend_from_slice(data);
    out.extend_from_slice(&crc32(&crc_input).to_be_bytes());
    out
}

/// Build a minimal valid 2x2 RGB PNG, then splice an `eXIf` chunk (holding
/// `tiff_body` verbatim, per the PNG 1.5 EXIF chunk spec) in right after
/// `IHDR` and before the first `IDAT`, using `image` to produce a real,
/// decodable base image so `extract_image_metadata`'s `image::ImageReader`
/// decode succeeds.
fn png_with_exif_chunk(tiff_body: &[u8]) -> Vec<u8> {
    use image::{ImageBuffer, ImageFormat, Rgb, RgbImage};
    use std::io::Cursor;

    let img: RgbImage = ImageBuffer::from_fn(2, 2, |_, _| Rgb([10, 20, 30]));
    let mut base_png = Vec::new();
    img.write_to(&mut Cursor::new(&mut base_png), ImageFormat::Png)
        .expect("encoding the base fixture PNG must succeed");

    assert!(
        base_png.starts_with(PNG_SIGNATURE),
        "sanity: image crate must emit a standard PNG signature"
    );

    let mut out = PNG_SIGNATURE.to_vec();
    let mut offset = PNG_SIGNATURE.len();
    let mut spliced = false;

    while offset + 8 <= base_png.len() {
        let len = u32::from_be_bytes(base_png[offset..offset + 4].try_into().unwrap()) as usize;
        let chunk_end = offset + 12 + len;
        let chunk_type = &base_png[offset + 4..offset + 8];

        // Insert eXIf immediately after IHDR, before any IDAT chunk.
        if !spliced && chunk_type == b"IDAT" {
            out.extend_from_slice(&png_chunk(b"eXIf", tiff_body));
            spliced = true;
        }

        out.extend_from_slice(&base_png[offset..chunk_end]);
        offset = chunk_end;
    }

    assert!(
        spliced,
        "sanity: base fixture PNG must contain an IDAT chunk to splice before"
    );
    out
}

/// TIFF/EXIF IFD0 entry: `(tag code, type code, component count, inline-or-offset value)`.
struct IfdEntry {
    tag: u16,
    data_format: u16,
    count: u32,
    data: Vec<u8>,
}

/// Build a minimal little-endian TIFF/EXIF byte blob (the body of a PNG
/// `eXIf` chunk / JPEG EXIF APP1 payload after its signature) with a single
/// IFD0 containing `entries`. Handles the "data > 4 bytes needs an external,
/// word-aligned offset" TIFF rule; entries are assumed to always exceed 4
/// bytes here (true for every field these tests populate).
fn build_tiff_ifd0(entries: &[IfdEntry]) -> Vec<u8> {
    const HEADER_LEN: usize = 8;
    let ifd_len = 2 + entries.len() * 12 + 4;
    let mut external_offset = HEADER_LEN + ifd_len;

    let mut header = Vec::with_capacity(HEADER_LEN);
    header.extend_from_slice(b"II"); // little-endian byte order
    header.extend_from_slice(&42u16.to_le_bytes());
    header.extend_from_slice(&(HEADER_LEN as u32).to_le_bytes()); // IFD0 offset

    let mut ifd = Vec::with_capacity(ifd_len);
    ifd.extend_from_slice(&(entries.len() as u16).to_le_bytes());

    let mut external_data = Vec::new();
    for entry in entries {
        assert!(
            entry.data.len() > 4,
            "test entries are expected to need an external offset"
        );
        ifd.extend_from_slice(&entry.tag.to_le_bytes());
        ifd.extend_from_slice(&entry.data_format.to_le_bytes());
        ifd.extend_from_slice(&entry.count.to_le_bytes());
        ifd.extend_from_slice(&(external_offset as u32).to_le_bytes());

        let mut data = entry.data.clone();
        if data.len() % 2 != 0 {
            data.push(0); // word-align the next entry's offset
        }
        external_offset += data.len();
        external_data.extend_from_slice(&data);
    }
    ifd.extend_from_slice(&0u32.to_le_bytes()); // no next IFD

    let mut out = header;
    out.extend_from_slice(&ifd);
    out.extend_from_slice(&external_data);
    out
}

/// TIFF type codes used below (see TIFF 6.0 spec section 2).
const TIFF_TYPE_BYTE: u16 = 1;
const TIFF_TYPE_ASCII: u16 = 2;
const TIFF_TYPE_UNDEFINED: u16 = 7;

const ARTIST_TAG: u16 = 0x013B;
const USER_COMMENT_TAG: u16 = 0x9286;
const XP_TITLE_TAG: u16 = 0x9C9B;
const XP_COMMENT_TAG: u16 = 0x9C9C;
const XP_KEYWORDS_TAG: u16 = 0x9C9D;
const XP_SUBJECT_TAG: u16 = 0x9C9E;

/// UTF-16LE-encode `s` with a trailing null terminator, as Windows XP tags store text.
fn utf16le_null_terminated(s: &str) -> Vec<u8> {
    let mut bytes = Vec::new();
    for unit in s.encode_utf16() {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }
    bytes.extend_from_slice(&[0, 0]);
    bytes
}

fn ascii_null_terminated(s: &str) -> Vec<u8> {
    let mut bytes = s.as_bytes().to_vec();
    bytes.push(0);
    bytes
}

fn extraction_config() -> ExtractionConfig {
    // Metadata-only: OCR isn't what these tests are about, and disabling it
    // keeps the tests fast and independent of a Tesseract install.
    ExtractionConfig {
        disable_ocr: true,
        ..Default::default()
    }
}

#[test]
fn recognizes_artist_usercomment_and_xp_tag_family() {
    let artist = "Test Artist";
    let user_comment = "Sample user comment";
    let xp_title = "XP Title Value";
    let xp_comment = "XP Comment Value";
    let xp_keywords = "XP Keywords Value";
    let xp_subject = "XP Subject Value";

    let tiff_body = build_tiff_ifd0(&[
        IfdEntry {
            tag: ARTIST_TAG,
            data_format: TIFF_TYPE_ASCII,
            count: (artist.len() + 1) as u32,
            data: ascii_null_terminated(artist),
        },
        IfdEntry {
            tag: USER_COMMENT_TAG,
            data_format: TIFF_TYPE_UNDEFINED,
            count: user_comment.len() as u32,
            data: user_comment.as_bytes().to_vec(),
        },
        IfdEntry {
            tag: XP_TITLE_TAG,
            data_format: TIFF_TYPE_BYTE,
            count: utf16le_null_terminated(xp_title).len() as u32,
            data: utf16le_null_terminated(xp_title),
        },
        IfdEntry {
            tag: XP_COMMENT_TAG,
            data_format: TIFF_TYPE_BYTE,
            count: utf16le_null_terminated(xp_comment).len() as u32,
            data: utf16le_null_terminated(xp_comment),
        },
        IfdEntry {
            tag: XP_KEYWORDS_TAG,
            data_format: TIFF_TYPE_BYTE,
            count: utf16le_null_terminated(xp_keywords).len() as u32,
            data: utf16le_null_terminated(xp_keywords),
        },
        IfdEntry {
            tag: XP_SUBJECT_TAG,
            data_format: TIFF_TYPE_BYTE,
            count: utf16le_null_terminated(xp_subject).len() as u32,
            data: utf16le_null_terminated(xp_subject),
        },
    ]);

    let png_bytes = png_with_exif_chunk(&tiff_body);

    let result = extract_bytes_document_blocking(&png_bytes, "image/png", &extraction_config())
        .expect("extraction of a valid PNG with an eXIf chunk must succeed");

    let Some(FormatMetadata::Image(image_metadata)) = result.metadata.format else {
        panic!(
            "expected Metadata::format to be FormatMetadata::Image, got {:?}",
            result.metadata.format
        );
    };

    assert_eq!(image_metadata.exif.get("Artist"), Some(&artist.to_string()));
    // `nom-exif` renders printable `Undefined` values quoted (see
    // `EntryValue`'s `Display` impl) — this is the real behavior other
    // `Undefined`-typed tags (e.g. `ExifVersion`) already exhibit.
    assert_eq!(
        image_metadata.exif.get("UserComment"),
        Some(&format!("\"{user_comment}\""))
    );
    assert_eq!(image_metadata.exif.get("XPTitle"), Some(&xp_title.to_string()));
    assert_eq!(image_metadata.exif.get("XPComment"), Some(&xp_comment.to_string()));
    assert_eq!(image_metadata.exif.get("XPKeywords"), Some(&xp_keywords.to_string()));
    assert_eq!(image_metadata.exif.get("XPSubject"), Some(&xp_subject.to_string()));

    assert!(
        result.processing_warnings.is_empty(),
        "well-formed EXIF must not produce a processing warning, got {:?}",
        result.processing_warnings
    );
}

#[test]
fn emits_processing_warning_when_exif_block_is_malformed() {
    // A well-formed PNG `eXIf` chunk whose TIFF payload is truncated mid-header
    // (magic bytes present, IFD0 offset points past the end of the buffer).
    let malformed_tiff_body: &[u8] = b"II*\x00\xFF\xFF\xFF\xFF";
    let png_bytes = png_with_exif_chunk(malformed_tiff_body);

    let result = extract_bytes_document_blocking(&png_bytes, "image/png", &extraction_config())
        .expect("extraction must still succeed with a malformed EXIF block; metadata degrades, not the whole document");

    assert!(
        result
            .processing_warnings
            .iter()
            .any(|warning| warning.source == "exif" && warning.message.contains("EXIF metadata extraction failed")),
        "expected an 'exif' ProcessingWarning describing the parse failure, got {:?}",
        result.processing_warnings
    );
}

/// Splice a JPEG APP1 XMP segment carrying `xmp_xml` right after `bytes`'s SOI
/// marker (`0xFFD8`), ahead of whatever segments the source JPEG already has.
/// Multiple APPn segments in any relative order (before SOS) are valid JPEG,
/// so this doesn't disturb the fixture's own EXIF/JFIF segments or its
/// entropy-coded data.
fn jpeg_with_xmp_segment(bytes: &[u8], xmp_xml: &str) -> Vec<u8> {
    assert_eq!(
        &bytes[..2],
        &[0xFFu8, 0xD8],
        "sanity: fixture must start with a JPEG SOI marker"
    );

    let mut payload = b"http://ns.adobe.com/xap/1.0/\0".to_vec();
    payload.extend_from_slice(xmp_xml.as_bytes());

    let mut app1_segment = vec![0xFF, 0xE1];
    let segment_len = u16::try_from(payload.len() + 2).expect("test XMP payload fits in a u16 segment length");
    app1_segment.extend_from_slice(&segment_len.to_be_bytes());
    app1_segment.extend_from_slice(&payload);

    let mut out = bytes[..2].to_vec();
    out.extend_from_slice(&app1_segment);
    out.extend_from_slice(&bytes[2..]);
    out
}

#[test]
fn extracts_dublin_core_fields_from_embedded_xmp_packet() {
    if skip_if_missing("images/example.jpg") {
        return;
    }

    let title = "Sunset over the bay";
    let description = "A photo taken during golden hour";
    let subject = "sunset, bay, golden hour";

    let xmp_xml = format!(
        r#"<x:xmpmeta xmlns:x="adobe:ns:meta/">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about=""
    xmlns:dc="http://purl.org/dc/elements/1.1/">
   <dc:title>
    <rdf:Alt>
     <rdf:li xml:lang="x-default">{title}</rdf:li>
    </rdf:Alt>
   </dc:title>
   <dc:description>
    <rdf:Alt>
     <rdf:li xml:lang="x-default">{description}</rdf:li>
    </rdf:Alt>
   </dc:description>
   <dc:subject>
    <rdf:Bag>
     <rdf:li>{subject}</rdf:li>
    </rdf:Bag>
   </dc:subject>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>"#
    );

    let base_jpeg = std::fs::read(get_test_file_path("images/example.jpg")).expect("fixture JPEG must be readable");
    let jpeg_bytes = jpeg_with_xmp_segment(&base_jpeg, &xmp_xml);

    let result = extract_bytes_document_blocking(&jpeg_bytes, "image/jpeg", &extraction_config())
        .expect("extraction of a real JPEG with an added XMP APP1 segment must succeed");

    let Some(FormatMetadata::Image(image_metadata)) = result.metadata.format else {
        panic!(
            "expected Metadata::format to be FormatMetadata::Image, got {:?}",
            result.metadata.format
        );
    };

    assert_eq!(image_metadata.exif.get("XMPTitle"), Some(&title.to_string()));
    assert_eq!(
        image_metadata.exif.get("XMPDescription"),
        Some(&description.to_string())
    );
    assert_eq!(image_metadata.exif.get("XMPSubject"), Some(&subject.to_string()));
}
