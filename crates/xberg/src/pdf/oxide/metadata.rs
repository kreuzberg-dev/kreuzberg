//! PDF metadata extraction using the pdf_oxide backend.
//!
//! Extracts document info dictionary fields (title, author, keywords, dates,
//! producer, creator) and PDF-specific properties (version, encryption, dimensions,
//! page count). Also builds `PageStructure` from page boundaries.

use super::OxideDocument;
use crate::pdf::error::{PdfError, Result};
use crate::pdf::metadata::{CommonPdfMetadata, PdfExtractionMetadata, PdfMetadata};
use crate::types::{PageBoundary, PageInfo, PageStructure, PageUnitType};

/// Extract complete PDF metadata from an oxide document.
///
/// Combines common metadata (title, authors, dates, etc.) with PDF-specific
/// metadata (version, encryption, dimensions) and optional page structure.
/// This is the oxide equivalent of `extract_metadata_from_document_impl`.
pub(crate) fn extract_metadata_from_oxide_document(
    doc: &mut OxideDocument,
    page_boundaries: Option<&[PageBoundary]>,
    content: &str,
    scanned_min_confidence: f64,
    ocr_quality_thresholds: &crate::core::config::OcrQualityThresholds,
) -> Result<PdfExtractionMetadata> {
    let pdf_specific = extract_pdf_specific_metadata(doc, scanned_min_confidence, ocr_quality_thresholds)?;
    let common = extract_common_metadata(doc)?;

    let page_structure = if let Some(boundaries) = page_boundaries {
        Some(build_page_structure(doc, boundaries, content)?)
    } else {
        None
    };

    Ok(PdfExtractionMetadata {
        title: common.title,
        subject: common.subject,
        authors: common.authors,
        keywords: common.keywords,
        created_at: common.created_at,
        modified_at: common.modified_at,
        created_by: common.created_by,
        pdf_specific,
        page_structure,
    })
}

/// Extract only PDF-specific metadata (version, producer, encryption, dimensions, page count).
fn extract_pdf_specific_metadata(
    doc: &mut OxideDocument,
    scanned_min_confidence: f64,
    ocr_quality_thresholds: &crate::core::config::OcrQualityThresholds,
) -> Result<PdfMetadata> {
    let (major, minor) = doc.doc.version();
    let pdf_version = if major > 0 {
        Some(format!("{}.{}", major, minor))
    } else {
        None
    };

    let is_encrypted = Some(doc.doc.is_encrypted());

    let page_count = doc
        .doc
        .page_count()
        .map_err(|e| PdfError::MetadataExtractionFailed(format!("Failed to get page count: {}", e)))?;

    let (width, height) = if page_count > 0 {
        match doc.doc.get_page_media_box(0) {
            Ok((llx, lly, urx, ury)) => {
                let w = (urx - llx).abs().round() as i64;
                let h = (ury - lly).abs().round() as i64;
                (Some(w), Some(h))
            }
            Err(_) => (None, None),
        }
    } else {
        (None, None)
    };

    let producer = get_info_string(&mut doc.doc, "Producer");

    // Advisory: a document we cannot grade reports no scan evidence. ~keep
    let detection = crate::pdf::scan_detect::detect(&doc.doc);
    let scanned_confidence = detection.as_ref().map(|d| d.confidence);
    let mut scanned_pages: Option<Vec<u32>> = detection.as_ref().map(|d| {
        d.scanned_page_indices(scanned_min_confidence as f32)
            .into_iter()
            .map(|index| index as u32 + 1)
            .collect()
    });

    // A page whose text layer pdf_oxide could not read from the file (issue
    // #1254) has low image coverage and is never selected by `detect` above, so
    // it is unioned in separately here. ~keep
    if ocr_quality_thresholds.enable_provenance_ocr_routing {
        let fabricated_pages = crate::pdf::scan_detect::fabricated_provenance_page_indices(
            &doc.doc,
            ocr_quality_thresholds.min_provenance_fallback_ratio,
            ocr_quality_thresholds.min_total_non_whitespace,
        );
        if !fabricated_pages.is_empty() {
            let mut merged = scanned_pages.unwrap_or_default();
            merged.extend(fabricated_pages.into_iter().map(|index| index as u32 + 1));
            merged.sort_unstable();
            merged.dedup();
            scanned_pages = Some(merged);
        }
    }

    Ok(PdfMetadata {
        pdf_version,
        producer,
        is_encrypted,
        width,
        height,
        page_count: Some(page_count as u32),
        scanned_confidence,
        scanned_pages,
        // Filled by the extractor after the layout pass runs, not here:
        // metadata extraction never runs the layout gate itself. ~keep
        layout_gated_pages: None,
        layout_gate_reasons: None,
    })
}

/// Extract common document metadata (title, author, keywords, dates, creator)
/// from the PDF Info dictionary, falling back to XMP metadata for any field
/// the Info dictionary leaves empty (issue #65).
///
/// Modern PDFs (commonly those produced by web exporters, office suites, or
/// design tools) often carry a sparse or empty Info dictionary and put the
/// authoritative metadata in the XMP packet instead (ISO 32000-1:2008
/// §14.3.2). Only fields absent from the Info dict are filled from XMP, so a
/// document's Info dict always wins where both are present.
fn extract_common_metadata(doc: &mut OxideDocument) -> Result<CommonPdfMetadata> {
    let title = get_info_string(&mut doc.doc, "Title");
    let subject = get_info_string(&mut doc.doc, "Subject");
    let created_by = get_info_string(&mut doc.doc, "Creator");

    let authors = get_info_string(&mut doc.doc, "Author")
        .map(|author_str| parse_authors(&author_str))
        .filter(|parsed| !parsed.is_empty());

    let keywords = get_info_string(&mut doc.doc, "Keywords")
        .map(|kw_str| parse_keywords(&kw_str))
        .filter(|parsed| !parsed.is_empty());

    let created_at = get_info_string(&mut doc.doc, "CreationDate").map(|d| parse_pdf_date(&d));
    let modified_at = get_info_string(&mut doc.doc, "ModDate").map(|d| parse_pdf_date(&d));

    let xmp = extract_xmp_metadata(&doc.doc);

    let title = title.or_else(|| xmp.as_ref().and_then(|x| non_empty(x.dc_title.clone())));
    // Adobe's XMP mapping convention: Info /Subject <-> dc:description (a
    // single descriptive string), distinct from dc:subject (a keyword bag,
    // mapped to Keywords below).
    let subject = subject.or_else(|| xmp.as_ref().and_then(|x| non_empty(x.dc_description.clone())));
    let created_by = created_by.or_else(|| xmp.as_ref().and_then(|x| non_empty(x.xmp_creator_tool.clone())));
    let authors = authors.or_else(|| xmp.as_ref().map(|x| x.dc_creator.clone()).filter(|c| !c.is_empty()));
    let keywords = keywords.or_else(|| xmp.as_ref().map(|x| x.dc_subject.clone()).filter(|s| !s.is_empty()));
    let created_at = created_at.or_else(|| xmp.as_ref().and_then(|x| non_empty(x.xmp_create_date.clone())));
    let modified_at = modified_at.or_else(|| xmp.as_ref().and_then(|x| non_empty(x.xmp_modify_date.clone())));

    Ok(CommonPdfMetadata {
        title,
        subject,
        authors,
        keywords,
        created_at,
        modified_at,
        created_by,
    })
}

/// `None` for `None`/empty strings, otherwise `Some`. XMP fields are optional
/// strings that may legally be present-but-empty; treat that the same as absent.
fn non_empty(value: Option<String>) -> Option<String> {
    value.filter(|s| !s.trim().is_empty())
}

/// Extract XMP metadata (ISO 32000-1:2008 §14.3.2) from the document catalog's
/// `/Metadata` stream, if present (issue #65).
///
/// Returns `None` when the document has no `/Metadata` entry, the XMP packet
/// could not be parsed, or it parsed but carried no recognized fields — all
/// non-error outcomes; XMP is optional and most PDFs of any age lack it.
fn extract_xmp_metadata(doc: &pdf_oxide::PdfDocument) -> Option<pdf_oxide::extractors::xmp::XmpMetadata> {
    match pdf_oxide::extractors::xmp::XmpExtractor::extract(doc) {
        Ok(Some(xmp)) if !xmp.is_empty() => Some(xmp),
        Ok(_) => None,
        Err(e) => {
            tracing::debug!("pdf_oxide: XMP extraction failed: {e}");
            None
        }
    }
}

/// Extract per-page display labels from `/PageLabels` (ISO 32000-1:2008
/// §12.4.2) — e.g. roman-numeral front matter followed by arabic body pages,
/// or per-section prefixed numbering (issue #66).
///
/// Returns `None` when the document defines no `/PageLabels` (the common
/// case), in which case every page uses its plain 1-based number, which
/// callers already have via `page_count`/`PageBoundary::page_number`.
pub(crate) fn extract_page_labels_all(doc: &mut OxideDocument) -> Result<Option<Vec<String>>> {
    let page_count = doc
        .doc
        .page_count()
        .map_err(|e| PdfError::MetadataExtractionFailed(format!("Failed to get page count for page labels: {}", e)))?;

    let ranges = match pdf_oxide::extractors::page_labels::PageLabelExtractor::extract(&doc.doc) {
        Ok(ranges) => ranges,
        Err(e) => {
            tracing::debug!("pdf_oxide: page label extraction failed: {e}");
            return Ok(None);
        }
    };

    if ranges.is_empty() {
        return Ok(None);
    }

    Ok(Some(
        pdf_oxide::extractors::page_labels::PageLabelExtractor::get_all_labels(&ranges, page_count),
    ))
}

/// Retrieve a string value from the PDF Info dictionary.
///
/// Accesses the trailer `/Info` reference, resolves it, then looks up the given
/// key. Returns `None` if the Info dict is absent, the key is missing, or the
/// value cannot be decoded as a string.
fn get_info_string(doc: &mut pdf_oxide::PdfDocument, key: &str) -> Option<String> {
    let trailer = doc.trailer().clone();
    let info_ref_obj = trailer.as_dict()?.get("Info")?.clone();

    let info_obj = match info_ref_obj.as_reference() {
        Some(obj_ref) => doc.load_object(obj_ref).ok()?,
        None => info_ref_obj,
    };

    let info_dict = info_obj.as_dict()?;

    let value = info_dict.get(key)?;

    match value {
        pdf_oxide::object::Object::String(bytes) => decode_pdf_string(bytes),
        pdf_oxide::object::Object::Name(name) => {
            let trimmed = name.trim().to_string();
            if trimmed.is_empty() { None } else { Some(trimmed) }
        }
        _ => None,
    }
}

/// Decode a PDF string (byte vector) into a Rust String.
///
/// Handles UTF-16BE encoding (BOM: 0xFE 0xFF) and falls back to Latin-1
/// (PDFDocEncoding) for byte strings without a BOM.
///
/// `pub(crate)` so other oxide submodules (e.g. `hierarchy`'s `/Alt` text
/// reader, issue #62) can reuse this decoding instead of duplicating it.
pub(crate) fn decode_pdf_string(bytes: &[u8]) -> Option<String> {
    if bytes.is_empty() {
        return None;
    }

    if bytes.len() >= 2 && bytes[0] == 0xFE && bytes[1] == 0xFF {
        let utf16: Vec<u16> = bytes[2..]
            .chunks_exact(2)
            .map(|chunk| u16::from_be_bytes([chunk[0], chunk[1]]))
            .collect();
        let decoded = String::from_utf16_lossy(&utf16);
        let trimmed = decoded.trim().to_string();
        if trimmed.is_empty() { None } else { Some(trimmed) }
    } else if bytes.len() >= 3 && bytes[0] == 0xEF && bytes[1] == 0xBB && bytes[2] == 0xBF {
        let decoded = String::from_utf8_lossy(&bytes[3..]);
        let trimmed = decoded.trim().to_string();
        if trimmed.is_empty() { None } else { Some(trimmed) }
    } else {
        match std::str::from_utf8(bytes) {
            Ok(s) => {
                let trimmed = s.trim().to_string();
                if trimmed.is_empty() { None } else { Some(trimmed) }
            }
            Err(_) => {
                let decoded: String = bytes.iter().map(|&b| b as char).collect();
                let trimmed = decoded.trim().to_string();
                if trimmed.is_empty() { None } else { Some(trimmed) }
            }
        }
    }
}

/// Build a `PageStructure` from an oxide document and page boundaries.
///
/// Mirrors `build_page_structure` in the pdf metadata module: validates
/// boundary count against page count, collects per-page dimensions from
/// MediaBox, and determines blank status from content slices.
fn build_page_structure(doc: &mut OxideDocument, boundaries: &[PageBoundary], content: &str) -> Result<PageStructure> {
    let total_count = doc
        .doc
        .page_count()
        .map_err(|e| PdfError::MetadataExtractionFailed(format!("Failed to get page count: {}", e)))?;

    if boundaries.is_empty() {
        return Err(PdfError::MetadataExtractionFailed(
            "No page boundaries provided for PageStructure".to_string(),
        ));
    }

    if boundaries.len() != total_count {
        return Err(PdfError::MetadataExtractionFailed(format!(
            "Boundary count {} doesn't match page count {}",
            boundaries.len(),
            total_count,
        )));
    }

    let mut pages = Vec::new();
    for (index, boundary) in boundaries.iter().enumerate() {
        let page_number = boundary.page_number;

        let dimensions = match doc.doc.get_page_media_box(index) {
            Ok((llx, lly, urx, ury)) => {
                let w = (urx - llx).abs() as f64;
                let h = (ury - lly).abs() as f64;
                Some((w, h))
            }
            Err(_) => None,
        };

        let is_blank = if boundary.byte_start <= boundary.byte_end && boundary.byte_end <= content.len() {
            let page_text = &content[boundary.byte_start..boundary.byte_end];
            Some(crate::extraction::blank_detection::is_page_text_blank(page_text))
        } else {
            None
        };

        pages.push(PageInfo {
            number: page_number,
            title: None,
            dimensions,
            image_count: None,
            table_count: None,
            hidden: None,
            is_blank,
            has_vector_graphics: false,
        });
    }

    Ok(PageStructure {
        total_count: total_count as u32,
        unit_type: PageUnitType::Page,
        boundaries: Some(boundaries.to_vec()),
        pages: if pages.is_empty() { None } else { Some(pages) },
    })
}

fn parse_authors(author_str: &str) -> Vec<String> {
    let author_str = author_str.replace(" and ", ", ");
    let mut authors = Vec::new();

    for segment in author_str.split(';') {
        for author in segment.split(',') {
            let trimmed = author.trim();
            if !trimmed.is_empty() {
                authors.push(trimmed.to_string());
            }
        }
    }

    authors
}

fn parse_keywords(keywords_str: &str) -> Vec<String> {
    keywords_str
        .replace(';', ",")
        .split(',')
        .filter_map(|k| {
            let trimmed = k.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        })
        .collect()
}

fn parse_pdf_date(date_str: &str) -> String {
    let cleaned = date_str.trim();

    if cleaned.starts_with("D:") && cleaned.len() >= 10 {
        let year = &cleaned[2..6];
        let month = &cleaned[6..8];
        let day = &cleaned[8..10];

        if cleaned.len() >= 16 {
            let hour = &cleaned[10..12];
            let minute = &cleaned[12..14];
            let second = &cleaned[14..16];
            format!("{}-{}-{}T{}:{}:{}Z", year, month, day, hour, minute, second)
        } else if cleaned.len() >= 14 {
            let hour = &cleaned[10..12];
            let minute = &cleaned[12..14];
            format!("{}-{}-{}T{}:{}:00Z", year, month, day, hour, minute)
        } else {
            format!("{}-{}-{}T00:00:00Z", year, month, day)
        }
    } else if cleaned.len() >= 8 {
        let year = &cleaned[0..4];
        let month = &cleaned[4..6];
        let day = &cleaned[6..8];
        format!("{}-{}-{}T00:00:00Z", year, month, day)
    } else {
        date_str.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_authors_single() {
        assert_eq!(parse_authors("John Doe"), vec!["John Doe"]);
    }

    #[test]
    fn test_parse_authors_comma() {
        assert_eq!(parse_authors("John Doe, Jane Smith"), vec!["John Doe", "Jane Smith"]);
    }

    #[test]
    fn test_parse_authors_and() {
        assert_eq!(parse_authors("John Doe and Jane Smith"), vec!["John Doe", "Jane Smith"]);
    }

    #[test]
    fn test_parse_keywords_comma() {
        assert_eq!(parse_keywords("pdf, document, test"), vec!["pdf", "document", "test"]);
    }

    #[test]
    fn test_parse_keywords_semicolon() {
        assert_eq!(parse_keywords("pdf;document;test"), vec!["pdf", "document", "test"]);
    }

    #[test]
    fn test_parse_pdf_date_full() {
        assert_eq!(parse_pdf_date("D:20230115123045"), "2023-01-15T12:30:45Z");
    }

    #[test]
    fn test_parse_pdf_date_no_time() {
        assert_eq!(parse_pdf_date("D:20230115"), "2023-01-15T00:00:00Z");
    }

    #[test]
    fn test_parse_pdf_date_no_prefix() {
        assert_eq!(parse_pdf_date("20230115"), "2023-01-15T00:00:00Z");
    }

    #[test]
    fn test_decode_pdf_string_ascii() {
        assert_eq!(decode_pdf_string(b"Hello World"), Some("Hello World".to_string()));
    }

    #[test]
    fn test_decode_pdf_string_utf16be() {
        let mut bytes = vec![0xFE, 0xFF];
        bytes.extend_from_slice(&[0x00, b'H', 0x00, b'i']);
        assert_eq!(decode_pdf_string(&bytes), Some("Hi".to_string()));
    }

    #[test]
    fn test_decode_pdf_string_empty() {
        assert_eq!(decode_pdf_string(b""), None);
    }

    #[test]
    fn test_decode_pdf_string_whitespace_only() {
        assert_eq!(decode_pdf_string(b"   "), None);
    }
}
