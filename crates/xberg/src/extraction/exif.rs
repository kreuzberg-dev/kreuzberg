//! EXIF metadata extraction, powered by `nom-exif`.
//!
//! Pure Rust — works on every target (including `wasm-target` and
//! `android-target`). Compiled under any of the `ocr`, `ocr-wasm`, or `heic`
//! features. Without those features, `extract_exif_data` falls back to
//! XMP-only extraction (see [`extract_xmp_data`]) so the rest of the image
//! extraction path keeps the same signature.

use std::collections::HashMap;

/// Extract EXIF data from image bytes.
///
/// Returns a HashMap of EXIF tag names to display strings. Empty when EXIF is
/// absent, malformed, or the container is unrecognised. Also merges in any
/// Dublin Core fields found in an embedded XMP packet (see
/// [`extract_xmp_data`]), since both live in the same metadata map on
/// [`crate::types::ImageMetadata`].
///
/// Backed by `nom-exif`, which supports EXIF blocks across JPEG, PNG, TIFF,
/// HEIC/HEIF, AVIF and several video containers in a single pure-Rust API.
#[cfg(any(feature = "ocr", feature = "ocr-wasm", feature = "heic"))]
pub(crate) fn extract_exif_data(bytes: &[u8]) -> HashMap<String, String> {
    use nom_exif::{Exif, ExifIter, ExifTag, MediaParser, MediaSource};

    let mut exif_map = HashMap::new();

    let bytes_owned = bytes::Bytes::copy_from_slice(bytes);
    let Ok(ms) = MediaSource::from_memory(bytes_owned) else {
        exif_map.extend(extract_xmp_data(bytes));
        return exif_map;
    };

    let mut parser = MediaParser::new();
    let Ok(iter): nom_exif::Result<ExifIter> = parser.parse_exif(ms) else {
        exif_map.extend(extract_xmp_data(bytes));
        return exif_map;
    };
    let exif: Exif = iter.into();

    // IFD0 tag codes not present in nom-exif 3.6.2's curated `ExifTag` enum.
    // Looked up via `Exif::get_by_code` at the main IFD, same as the enum-based
    // tags below but bypassing the enum since these codes have no variant.
    const ARTIST_TAG_CODE: u16 = 0x013B;
    // Windows Explorer's "XP" tag family: BYTE arrays holding null-terminated
    // UTF-16LE text. Not part of the base TIFF/EXIF spec, but commonly written
    // by Windows Photo/Explorer property sheets and read back by most tooling.
    const XP_TITLE_TAG_CODE: u16 = 0x9C9B;
    const XP_COMMENT_TAG_CODE: u16 = 0x9C9C;
    const XP_KEYWORDS_TAG_CODE: u16 = 0x9C9D;
    const XP_SUBJECT_TAG_CODE: u16 = 0x9C9E;

    const TAGS: &[(ExifTag, &str)] = &[
        (ExifTag::Make, "Make"),
        (ExifTag::Model, "Model"),
        (ExifTag::Software, "Software"),
        (ExifTag::HostComputer, "HostComputer"),
        (ExifTag::ImageDescription, "ImageDescription"),
        (ExifTag::Copyright, "Copyright"),
        (ExifTag::UserComment, "UserComment"),
        (ExifTag::CameraSerialNumber, "CameraSerialNumber"),
        (ExifTag::ImageUniqueID, "ImageUniqueID"),
        (ExifTag::ExifVersion, "ExifVersion"),
        (ExifTag::ModifyDate, "DateTime"),
        (ExifTag::DateTimeOriginal, "DateTimeOriginal"),
        (ExifTag::CreateDate, "DateTimeDigitized"),
        (ExifTag::OffsetTime, "OffsetTime"),
        (ExifTag::OffsetTimeOriginal, "OffsetTimeOriginal"),
        (ExifTag::OffsetTimeDigitized, "OffsetTimeDigitized"),
        (ExifTag::SubSecTime, "SubSecTime"),
        (ExifTag::SubSecTimeOriginal, "SubSecTimeOriginal"),
        (ExifTag::SubSecTimeDigitized, "SubSecTimeDigitized"),
        (ExifTag::ImageWidth, "ImageWidth"),
        (ExifTag::ImageHeight, "ImageHeight"),
        (ExifTag::ExifImageWidth, "ExifImageWidth"),
        (ExifTag::ExifImageHeight, "ExifImageHeight"),
        (ExifTag::Orientation, "Orientation"),
        (ExifTag::XResolution, "XResolution"),
        (ExifTag::YResolution, "YResolution"),
        (ExifTag::ResolutionUnit, "ResolutionUnit"),
        (ExifTag::ColorSpace, "ColorSpace"),
        (ExifTag::ExposureTime, "ExposureTime"),
        (ExifTag::FNumber, "FNumber"),
        (ExifTag::ApertureValue, "ApertureValue"),
        (ExifTag::ShutterSpeedValue, "ShutterSpeedValue"),
        (ExifTag::ExposureProgram, "ExposureProgram"),
        (ExifTag::ExposureMode, "ExposureMode"),
        (ExifTag::ExposureBiasValue, "ExposureBiasValue"),
        (ExifTag::ISOSpeedRatings, "ISO"),
        (ExifTag::SensitivityType, "SensitivityType"),
        (ExifTag::MeteringMode, "MeteringMode"),
        (ExifTag::LightSource, "LightSource"),
        (ExifTag::Flash, "Flash"),
        (ExifTag::WhiteBalanceMode, "WhiteBalance"),
        (ExifTag::SceneCaptureType, "SceneCaptureType"),
        (ExifTag::SubjectDistance, "SubjectDistance"),
        (ExifTag::SubjectDistanceRange, "SubjectDistanceRange"),
        (ExifTag::SubjectArea, "SubjectArea"),
        (ExifTag::DigitalZoomRatio, "DigitalZoomRatio"),
        (ExifTag::Contrast, "Contrast"),
        (ExifTag::Saturation, "Saturation"),
        (ExifTag::Sharpness, "Sharpness"),
        (ExifTag::FocalLength, "FocalLength"),
        (ExifTag::FocalLengthIn35mmFilm, "FocalLengthIn35mmFilm"),
        (ExifTag::LensMake, "LensMake"),
        (ExifTag::LensModel, "LensModel"),
        (ExifTag::LensSpecification, "LensSpecification"),
        (ExifTag::LensSerialNumber, "LensSerialNumber"),
        (ExifTag::GPSLatitudeRef, "GPSLatitudeRef"),
        (ExifTag::GPSLatitude, "GPSLatitude"),
        (ExifTag::GPSLongitudeRef, "GPSLongitudeRef"),
        (ExifTag::GPSLongitude, "GPSLongitude"),
        (ExifTag::GPSAltitudeRef, "GPSAltitudeRef"),
        (ExifTag::GPSAltitude, "GPSAltitude"),
        (ExifTag::GPSTimeStamp, "GPSTimeStamp"),
        (ExifTag::GPSDateStamp, "GPSDateStamp"),
        (ExifTag::GPSSpeed, "GPSSpeed"),
        (ExifTag::GPSSpeedRef, "GPSSpeedRef"),
        (ExifTag::GPSTrack, "GPSTrack"),
        (ExifTag::GPSTrackRef, "GPSTrackRef"),
        (ExifTag::GPSImgDirection, "GPSImgDirection"),
        (ExifTag::GPSImgDirectionRef, "GPSImgDirectionRef"),
        (ExifTag::GPSMapDatum, "GPSMapDatum"),
        (ExifTag::GPSProcessingMethod, "GPSProcessingMethod"),
        (ExifTag::ThumbnailOffset, "ThumbnailOffset"),
        (ExifTag::ThumbnailLength, "ThumbnailLength"),
    ];

    for (tag, field_name) in TAGS {
        if let Some(value) = exif.get(*tag) {
            exif_map.insert((*field_name).to_string(), value.to_string());
        }
    }

    if let Some(value) = exif.get_by_code(nom_exif::IfdIndex::MAIN, ARTIST_TAG_CODE) {
        exif_map.insert("Artist".to_string(), value.to_string());
    }

    const XP_TAGS: &[(u16, &str)] = &[
        (XP_TITLE_TAG_CODE, "XPTitle"),
        (XP_COMMENT_TAG_CODE, "XPComment"),
        (XP_KEYWORDS_TAG_CODE, "XPKeywords"),
        (XP_SUBJECT_TAG_CODE, "XPSubject"),
    ];
    for (code, field_name) in XP_TAGS {
        if let Some(value) = exif.get_by_code(nom_exif::IfdIndex::MAIN, *code)
            && let Some(text) = decode_xp_bytes(value)
        {
            exif_map.insert((*field_name).to_string(), text);
        }
    }

    exif_map.extend(extract_xmp_data(bytes));

    exif_map
}

/// Decode a Windows "XP" tag value (a BYTE array holding null-terminated
/// UTF-16LE text) into a plain `String`.
///
/// `nom-exif` has no dedicated variant for these tags, so they surface as
/// `U8Array`/`Undefined` raw bytes depending on how the writer declared the
/// TIFF field type; both are handled the same way here.
#[cfg(any(feature = "ocr", feature = "ocr-wasm", feature = "heic"))]
fn decode_xp_bytes(value: &nom_exif::EntryValue) -> Option<String> {
    let bytes: &[u8] = match value {
        nom_exif::EntryValue::U8Array(bytes) => bytes,
        nom_exif::EntryValue::Undefined(bytes) => bytes,
        _ => return None,
    };

    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
        .take_while(|&unit| unit != 0)
        .collect();

    if units.is_empty() {
        return None;
    }

    Some(String::from_utf16_lossy(&units))
}

/// Stub EXIF extraction when no EXIF-capable feature is active.
///
/// Compiled under any feature combination so the rest of the image path keeps
/// the same signature, but reranker-only builds — which drop every caller —
/// would surface it as `dead_code` without the `#[allow]`.
#[cfg(not(any(feature = "ocr", feature = "ocr-wasm", feature = "heic")))]
#[allow(dead_code)]
pub(crate) fn extract_exif_data(bytes: &[u8]) -> HashMap<String, String> {
    extract_xmp_data(bytes)
}

/// Diagnose why [`extract_exif_data`] returned no usable data, without redoing
/// the full per-tag extraction.
///
/// Returns `None` when the container was read and the EXIF block (if any)
/// parsed cleanly — including the common, non-degraded case where the image
/// simply carries no EXIF block at all. Returns `Some(reason)` when the media
/// source or the EXIF block itself could not be read, which is the case
/// [`extract_exif_data`] silently swallows into an empty map.
#[cfg(any(feature = "ocr", feature = "ocr-wasm", feature = "heic"))]
fn exif_parse_failure_reason(bytes: &[u8]) -> Option<String> {
    use nom_exif::{MediaParser, MediaSource};

    let bytes_owned = bytes::Bytes::copy_from_slice(bytes);
    let ms = match MediaSource::from_memory(bytes_owned) {
        Ok(ms) => ms,
        Err(error) => return Some(format!("failed to read media source: {error}")),
    };

    let mut parser = MediaParser::new();
    if let Err(error) = parser.parse_exif(ms) {
        return Some(format!("failed to parse EXIF block: {error}"));
    }

    None
}

/// Build a [`crate::types::ProcessingWarning`] describing degraded EXIF/image
/// metadata extraction, if any occurred, for `bytes`.
///
/// Returns `None` when EXIF extraction either succeeded or the image simply
/// has no EXIF block to read (not a degradation). Callers attach the returned
/// warning to the document's `processing_warnings` so silent degradation is
/// visible to consumers.
#[cfg(any(feature = "ocr", feature = "ocr-wasm", feature = "heic"))]
pub(crate) fn extract_exif_warning(bytes: &[u8]) -> Option<crate::types::ProcessingWarning> {
    let reason = exif_parse_failure_reason(bytes)?;
    Some(crate::types::ProcessingWarning {
        source: std::borrow::Cow::Borrowed("exif"),
        message: std::borrow::Cow::Owned(format!("EXIF metadata extraction failed: {reason}")),
    })
}

/// No EXIF-capable feature is compiled in, so metadata extraction never even
/// attempts to run. Always surfaces a warning so callers know why the
/// resulting metadata map is empty.
#[cfg(not(any(feature = "ocr", feature = "ocr-wasm", feature = "heic")))]
#[allow(dead_code)]
pub(crate) fn extract_exif_warning(_bytes: &[u8]) -> Option<crate::types::ProcessingWarning> {
    Some(crate::types::ProcessingWarning {
        source: std::borrow::Cow::Borrowed("exif"),
        message: std::borrow::Cow::Borrowed(
            "EXIF/image metadata extraction unavailable: no ocr, ocr-wasm, or heic feature compiled in",
        ),
    })
}

/// JPEG APP1 XMP packet signature (defined by Adobe's XMP specification, part
/// 3, section 1.1.3), distinct from the `Exif\0\0` APP1 signature used for
/// EXIF blocks.
const XMP_SIGNATURE: &[u8] = b"http://ns.adobe.com/xap/1.0/\0";

/// JPEG APP1 marker bytes (`0xFFE1`).
const JPEG_APP1_MARKER: [u8; 2] = [0xFF, 0xE1];

/// Extract Dublin Core fields from a JPEG's embedded XMP packet, if present.
///
/// Scans the byte stream for a JPEG APP1 segment carrying the XMP signature,
/// then pulls `dc:title`, `dc:description`, and `dc:subject` out of the
/// embedded XML packet by simple substring scanning rather than a full XML
/// parser: `quick-xml`/`roxmltree` are not part of the `ocr`/`ocr-wasm`/`heic`
/// dependency graph this module compiles under, and pulling in a full parser
/// for a handful of leaf-text lookups would be disproportionate.
///
/// Returns an empty map for non-JPEG bytes, JPEGs without an XMP packet, or
/// XMP packets that don't carry any of the three fields above.
pub(crate) fn extract_xmp_data(bytes: &[u8]) -> HashMap<String, String> {
    let mut xmp_map = HashMap::new();

    let Some(xml) = find_xmp_packet(bytes) else {
        return xmp_map;
    };

    const DUBLIN_CORE_FIELDS: &[(&str, &str)] = &[
        ("dc:title", "XMPTitle"),
        ("dc:description", "XMPDescription"),
        ("dc:subject", "XMPSubject"),
    ];

    for (tag, field_name) in DUBLIN_CORE_FIELDS {
        if let Some(value) = extract_xmp_leaf_text(xml, tag) {
            xmp_map.insert((*field_name).to_string(), value);
        }
    }

    xmp_map
}

/// Locate the XMP APP1 segment in a JPEG byte stream and return its XML
/// packet as a `&str`, if present and valid UTF-8.
fn find_xmp_packet(bytes: &[u8]) -> Option<&str> {
    let mut search_start = 0;

    while let Some(relative_marker_offset) = memchr::memmem::find(&bytes[search_start..], &JPEG_APP1_MARKER) {
        let marker_offset = search_start + relative_marker_offset;
        let length_offset = marker_offset + JPEG_APP1_MARKER.len();
        if length_offset + 2 > bytes.len() {
            return None;
        }

        let segment_len = u16::from_be_bytes([bytes[length_offset], bytes[length_offset + 1]]) as usize;
        let payload_start = length_offset + 2;
        // `segment_len` includes the two length bytes themselves.
        let payload_end = (length_offset + segment_len).min(bytes.len());

        if payload_end <= payload_start {
            search_start = payload_start.min(bytes.len());
            continue;
        }

        let payload = &bytes[payload_start..payload_end];
        if let Some(xml_bytes) = payload.strip_prefix(XMP_SIGNATURE) {
            return std::str::from_utf8(xml_bytes).ok();
        }

        search_start = payload_end;
    }

    None
}

/// Extract the leaf text content of `<tag>...</tag>` from an XMP XML packet.
///
/// Dublin Core fields are typically wrapped in an `rdf:Alt`/`rdf:Bag`/`rdf:Seq`
/// container holding one or more `rdf:li` entries (e.g.
/// `<dc:title><rdf:Alt><rdf:li xml:lang="x-default">Title</rdf:li></rdf:Alt></dc:title>`);
/// when present, the first `rdf:li` entry's text is returned. Otherwise the
/// raw element text is returned as-is.
fn extract_xmp_leaf_text(xml: &str, tag: &str) -> Option<String> {
    let open_needle = format!("<{tag}");
    let close_needle = format!("</{tag}>");

    let open_tag_start = xml.find(&open_needle)?;
    let open_tag_end = xml[open_tag_start..].find('>')? + open_tag_start + 1;
    let close_tag_start = xml[open_tag_end..].find(&close_needle)? + open_tag_end;
    let inner = &xml[open_tag_end..close_tag_start];

    let text = if let Some(li_start) = inner.find("<rdf:li") {
        let li_open_end = inner[li_start..].find('>')? + li_start + 1;
        let li_close_start = inner[li_open_end..].find("</rdf:li>")? + li_open_end;
        &inner[li_open_end..li_close_start]
    } else {
        inner
    };

    let trimmed = text.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn returns_empty_map_for_non_image_bytes() {
        assert!(extract_exif_data(b"hello world").is_empty());
        assert!(extract_exif_data(&[]).is_empty());
    }
}
