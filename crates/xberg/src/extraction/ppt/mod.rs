//! Native PPT (PowerPoint 97-2003) text extraction.
//!
//! Extracts text directly from PowerPoint Binary File Format using OLE/CFB
//! compound document parsing, without requiring LibreOffice.
//!
//! Supports PowerPoint 97, 2000, XP, and 2003 (.ppt) files.

use crate::error::{Result, XbergError};
use crate::types::{ExtractedImage, ProcessingWarning};
use bytes::Bytes;
use std::borrow::Cow;
use std::io::Cursor;

/// Warning source tag for `.ppt` extraction diagnostics (#171 convention).
const PPT_WARNING_SOURCE: &str = "ppt";

/// Result of PPT text extraction.
#[cfg_attr(alef, alef(skip))]
pub struct PptExtractionResult {
    /// Full document text: every slide's text joined by double newlines,
    /// kept for diagnostics/plain-text consumers. Slide *structure* (numbers,
    /// per-slide boundaries) must come from `slides`, not from re-splitting
    /// this string (see #1418 -- a slide's own text can itself contain an
    /// internal "\n\n", which makes re-splitting on it ambiguous).
    pub text: String,
    /// Per-slide text, in persist order. `number` is the slide's 1-based
    /// position among `RT_SLIDE` containers as they occur in the
    /// "PowerPoint Document" stream -- the same order `slide_count` counts.
    /// A slide with no text atoms still gets an entry (with an empty
    /// `text`), so slide numbers stay contiguous with the real deck.
    pub slides: Vec<PptSlideText>,
    /// Number of slides found.
    pub slide_count: usize,
    /// Document metadata.
    pub metadata: PptMetadata,
    /// Speaker notes text per slide (if available).
    pub speaker_notes: Vec<String>,
    /// Pictures recovered from the OLE `Pictures` stream (raw
    /// `OfficeArtBlip` payloads). Empty when the deck has no `Pictures`
    /// stream, the stream is empty, or image extraction was not requested.
    pub images: Vec<ExtractedImage>,
    /// Non-fatal degradations encountered while extracting (see
    /// `core::diagnostics`). Empty when extraction was complete.
    pub processing_warnings: Vec<ProcessingWarning>,
}

/// One slide's text, numbered by its position in the deck's own persist
/// order rather than by the position of a text block in a joined string.
#[cfg_attr(alef, alef(skip))]
pub struct PptSlideText {
    /// 1-based slide number, as encountered in persist order.
    pub number: u32,
    /// The slide's text (its atoms joined by `\n`). Empty for a slide with
    /// no text.
    pub text: String,
}

/// Metadata extracted from PPT files.
#[cfg_attr(alef, alef(skip))]
#[derive(Default)]
pub struct PptMetadata {
    /// Presentation title from the OLE summary information.
    pub title: Option<String>,
    /// Presentation subject from the OLE summary information.
    pub subject: Option<String>,
    /// Original author from the OLE summary information.
    pub author: Option<String>,
    /// Most recent editor from the OLE summary information.
    pub last_author: Option<String>,
}

const RT_TEXT_CHARS_ATOM: u16 = 0x0FA0;
const RT_TEXT_BYTES_ATOM: u16 = 0x0FA8;
/// A single slide's persisted content container (SlideAtom + shapes/text).
/// `SlideListWithText` (0x0FF0), by contrast, is a per-document container of
/// `SlidePersistAtom` entries used for the outline view -- it does not
/// enclose the slides' actual text and does not occur once per slide, so it
/// cannot be used as a slide boundary (#87).
const RT_SLIDE: u16 = 0x03EE;
const RT_MAIN_MASTER: u16 = 0x03F8;
const RT_NOTES: u16 = 0x03F0;

/// `OfficeArtBlip` record types for the raster formats a `Pictures` stream
/// can hold (MS-ODRAW 2.2.23). `RT_BLIP_JPEG_ALT` (0xF02A) is an alternate
/// `recType` documented for JPEG blips written by older Office versions; it
/// uses the same `OfficeArtBlipJPEG` layout as 0xF01D.
const RT_BLIP_JPEG: u16 = 0xF01D;
const RT_BLIP_JPEG_ALT: u16 = 0xF02A;
const RT_BLIP_PNG: u16 = 0xF01E;
const RT_BLIP_DIB: u16 = 0xF01F;

/// Maximum accepted size for a single embedded picture (100 MB), mirroring
/// the DOCX/PPTX image cap (`crate::extraction::docx::MAX_IMAGE_FILE_SIZE`).
/// Bounds allocation from a hostile `recLen` in the untrusted `Pictures`
/// stream.
const MAX_PICTURE_SIZE: usize = 100 * 1024 * 1024;

/// Extract text from PPT bytes.
///
/// Parses the OLE/CFB compound document, reads the "PowerPoint Document" stream,
/// and extracts text from TextCharsAtom and TextBytesAtom records.
///
/// When `include_master_slides` is `true`, master slide content (placeholder text
/// like "Click to edit Master title style") is included instead of being skipped.
#[cfg(test)]
pub(crate) fn extract_ppt_text(content: &[u8]) -> Result<PptExtractionResult> {
    extract_ppt_text_with_options(content, false, true)
}

/// Extract text from PPT bytes with configurable master slide inclusion and
/// image extraction.
///
/// When `include_master_slides` is `true`, `RT_MAIN_MASTER` containers are not
/// skipped, so master slide placeholder text is included in the output.
///
/// When `extract_images` is `true`, the OLE `Pictures` stream (if present)
/// is walked for embedded raster images (#1417).
pub(crate) fn extract_ppt_text_with_options(
    content: &[u8],
    include_master_slides: bool,
    extract_images: bool,
) -> Result<PptExtractionResult> {
    let cursor = Cursor::new(content);
    let mut comp = cfb::CompoundFile::open(cursor)
        .map_err(|e| XbergError::parsing(format!("Failed to open PPT as OLE container: {e}")))?;

    let metadata = extract_ppt_metadata(&mut comp);

    let ppt_stream = read_stream(&mut comp, "/PowerPoint Document")?;
    if ppt_stream.is_empty() {
        return Err(XbergError::parsing("PowerPoint Document stream is empty"));
    }

    let mut processing_warnings = Vec::new();
    let (mut slides, loose_texts, speaker_notes) =
        extract_texts_from_records(&ppt_stream, include_master_slides, &mut processing_warnings)?;

    // Computed from the pre-fallback data so `loose_texts` is never counted
    // twice below (once here, once folded into the synthetic slide).
    let text = slides
        .iter()
        .map(|s| s.text.as_str())
        .chain(loose_texts.iter().map(String::as_str))
        .filter(|t| !t.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");

    // Defensive fallback for a stream with no `RT_SLIDE` containers at all
    // but with top-level text outside any slide/notes container: surface it
    // as a single synthetic slide rather than dropping it (matches the
    if slides.is_empty() && !loose_texts.is_empty() {
        slides.push(PptSlideText {
            number: 1,
            text: loose_texts.join("\n"),
        });
    }
    let slide_count = slides.len();

    let images = if extract_images {
        match read_stream(&mut comp, "/Pictures") {
            Ok(pictures_stream) if !pictures_stream.is_empty() => {
                extract_pictures_from_stream(&pictures_stream, &mut processing_warnings)
            }
            _ => Vec::new(),
        }
    } else {
        Vec::new()
    };

    Ok(PptExtractionResult {
        text: text.trim().to_string(),
        slides,
        slide_count,
        metadata,
        speaker_notes,
        images,
        processing_warnings,
    })
}

/// Parse PowerPoint record headers and extract text atoms.
///
/// Returns `(slides, loose_texts, speaker_notes)`, where `slides` carries
/// one entry per `RT_SLIDE` container in persist order (including empty
/// slides), and `loose_texts` carries text found outside any slide/notes
/// container (rare, but preserved for the `slide_count == 0` fallback).
///
/// When `include_master_slides` is `true`, master slide containers are not
/// skipped, allowing their placeholder text to appear in the output.
fn extract_texts_from_records(
    data: &[u8],
    include_master_slides: bool,
    warnings: &mut Vec<ProcessingWarning>,
) -> Result<(Vec<PptSlideText>, Vec<String>, Vec<String>)> {
    let mut slides: Vec<PptSlideText> = Vec::new();
    let mut loose_texts = Vec::new();
    let mut current_slide_number: u32 = 0;
    let mut pos = 0;
    let mut in_slide_text = false;
    let mut slide_end: Option<usize> = None;
    let mut current_slide_texts: Vec<String> = Vec::new();
    let mut speaker_notes = Vec::new();
    let mut in_notes = false;
    let mut notes_end: Option<usize> = None;
    let mut current_notes_texts: Vec<String> = Vec::new();

    while pos + 8 <= data.len() {
        // A slide/notes container's text only spans its own declared byte
        // range; close it out as soon as the walk passes that range's end,
        // rather than leaving `in_slide_text`/`in_notes` stuck on until the
        // *next* occurrence (which, for `in_slide_text`, previously ran on
        // to the end of the stream -- see `RT_SLIDE` below).
        if let Some(end) = slide_end
            && pos >= end
        {
            // Push even when empty: a slide with no text atoms still exists
            // and must keep its persist-order number (#1418), rather than
            // vanishing and shifting every later slide's number down.
            slides.push(PptSlideText {
                number: current_slide_number,
                text: current_slide_texts.join("\n"),
            });
            current_slide_texts.clear();
            in_slide_text = false;
            slide_end = None;
        }
        if let Some(end) = notes_end
            && pos >= end
        {
            if !current_notes_texts.is_empty() {
                let notes_text = current_notes_texts.join("\n");
                let trimmed = notes_text.trim().to_string();
                if !trimmed.is_empty() {
                    speaker_notes.push(trimmed);
                }
                current_notes_texts.clear();
            }
            in_notes = false;
            notes_end = None;
        }

        let rec_ver_instance = u16::from_le_bytes([data[pos], data[pos + 1]]);
        let rec_ver = rec_ver_instance & 0x000F;
        let rec_type = u16::from_le_bytes([data[pos + 2], data[pos + 3]]);
        let rec_len = u32::from_le_bytes([data[pos + 4], data[pos + 5], data[pos + 6], data[pos + 7]]) as usize;

        if rec_len > data.len() - pos {
            crate::core::diagnostics::push_warning(
                warnings,
                PPT_WARNING_SOURCE,
                "Record stream ended with a truncated record; the remaining presentation content was not extracted",
            );
            break;
        }

        let is_container = rec_ver == 0x0F;
        let content_start = pos + 8;
        let content_end = content_start + rec_len;

        match rec_type {
            RT_SLIDE => {
                if in_slide_text {
                    slides.push(PptSlideText {
                        number: current_slide_number,
                        text: current_slide_texts.join("\n"),
                    });
                    current_slide_texts.clear();
                }
                current_slide_number += 1;
                in_slide_text = true;
                slide_end = Some(content_end);
                pos += 8;
                continue;
            }
            RT_NOTES => {
                if in_notes && !current_notes_texts.is_empty() {
                    let notes_text = current_notes_texts.join("\n");
                    let trimmed = notes_text.trim().to_string();
                    if !trimmed.is_empty() {
                        speaker_notes.push(trimmed);
                    }
                    current_notes_texts.clear();
                }
                in_notes = true;
                notes_end = Some(content_end);
                pos += 8;
                continue;
            }
            RT_MAIN_MASTER if !include_master_slides => {
                pos = content_end;
                continue;
            }
            RT_TEXT_CHARS_ATOM => {
                if content_end <= data.len() {
                    let text_data = &data[content_start..content_end];
                    let chars: Vec<u16> = text_data
                        .chunks_exact(2)
                        .map(|c| u16::from_le_bytes([c[0], c[1]]))
                        .collect();
                    let text = String::from_utf16_lossy(&chars);
                    let cleaned = clean_ppt_text(&text);
                    if !cleaned.is_empty() {
                        if in_notes {
                            current_notes_texts.push(cleaned.clone());
                        }
                        if in_slide_text {
                            current_slide_texts.push(cleaned);
                        } else if !in_notes {
                            loose_texts.push(cleaned);
                        }
                    }
                }
                pos = content_end;
                continue;
            }
            RT_TEXT_BYTES_ATOM => {
                if content_end <= data.len() {
                    let text_data = &data[content_start..content_end];
                    let text: String = text_data.iter().map(|&b| cp1252_to_char(b)).collect();
                    let cleaned = clean_ppt_text(&text);
                    if !cleaned.is_empty() {
                        if in_notes {
                            current_notes_texts.push(cleaned.clone());
                        }
                        if in_slide_text {
                            current_slide_texts.push(cleaned);
                        } else if !in_notes {
                            loose_texts.push(cleaned);
                        }
                    }
                }
                pos = content_end;
                continue;
            }
            _ => {}
        }

        if is_container {
            pos += 8;
        } else {
            pos = content_end;
        }
    }

    // The stream ended while still inside a slide's declared byte range
    // (e.g. a truncated record broke the walk early): still record it,
    // rather than silently dropping the last slide's text and number.
    if in_slide_text {
        slides.push(PptSlideText {
            number: current_slide_number,
            text: current_slide_texts.join("\n"),
        });
    }

    if !current_notes_texts.is_empty() {
        let notes_text = current_notes_texts.join("\n");
        let trimmed = notes_text.trim().to_string();
        if !trimmed.is_empty() {
            speaker_notes.push(trimmed);
        }
    }

    Ok((slides, loose_texts, speaker_notes))
}

/// Walk a `Pictures` stream (a flat run of `OfficeArtBlip` records, MS-ODRAW
/// 2.2.23) and emit one `ExtractedImage` per raster blip.
///
/// Only the raster formats stored as `rgbUid` + optional second `rgbUid` +
/// `tag` + `BLIPFileData` are handled: JPEG (0xF01D / the alternate 0xF02A),
/// PNG (0xF01E), and DIB (0xF01F). Metafile blips (EMF/WMF/PICT) use a
/// different, larger header and are not raster images; they are skipped.
///
/// Every length is validated against the remaining buffer before slicing,
/// so a hostile `recLen` can only shrink the walk (skip a record or stop
/// early), never over-read or allocate unboundedly.
fn extract_pictures_from_stream(data: &[u8], warnings: &mut Vec<ProcessingWarning>) -> Vec<ExtractedImage> {
    let mut images = Vec::new();
    let mut pos = 0usize;
    let mut image_index: u32 = 0;

    while pos + 8 <= data.len() {
        let rec_ver_instance = u16::from_le_bytes([data[pos], data[pos + 1]]);
        // recInstance occupies the upper 12 bits of the packed 16-bit field
        // (recVer, the low 4 bits, is checked nowhere here -- MS-ODRAW
        // requires it to be 0 for blips, but a non-zero value doesn't change
        // where the UID/tag/data fields are).
        let rec_instance = rec_ver_instance >> 4;
        let rec_type = u16::from_le_bytes([data[pos + 2], data[pos + 3]]);
        let rec_len = u32::from_le_bytes([data[pos + 4], data[pos + 5], data[pos + 6], data[pos + 7]]) as usize;

        let remaining = data.len() - (pos + 8);
        if rec_len > remaining {
            crate::core::diagnostics::push_warning(
                warnings,
                PPT_WARNING_SOURCE,
                "Pictures stream ended with a truncated record; the remaining embedded images were not extracted",
            );
            break;
        }

        let content_start = pos + 8;
        let content_end = content_start + rec_len;

        let format: Option<Cow<'static, str>> = match rec_type {
            RT_BLIP_JPEG | RT_BLIP_JPEG_ALT => Some(Cow::Borrowed("jpeg")),
            RT_BLIP_PNG => Some(Cow::Borrowed("png")),
            RT_BLIP_DIB => Some(Cow::Borrowed("dib")),
            _ => None,
        };

        if let Some(format) = format {
            // One `rgbUid` (16 bytes) + `tag` (1 byte) = 17-byte header, or
            // two `rgbUid`s + `tag` = 33 bytes; per MS-ODRAW 2.2.27-2.2.29
            // the low bit of `recInstance` is what distinguishes the two
            // UID counts for every raster blip type (e.g. JPEG 0x46A vs
            // 0x46B, PNG 0x6E0 vs 0x6E1, DIB 0x7A8 vs 0x7A9).
            let header_len = if rec_instance & 0x1 == 1 { 33 } else { 17 };

            if rec_len < header_len {
                crate::core::diagnostics::push_warning(
                    warnings,
                    PPT_WARNING_SOURCE,
                    format!(
                        "Blip record at offset {pos} (recLen={rec_len}) is shorter than its UID header \
                         ({header_len} bytes); skipped"
                    ),
                );
                pos = content_end;
                continue;
            }

            let picture_len = rec_len - header_len;
            if picture_len == 0 {
                pos = content_end;
                continue;
            }

            if picture_len > MAX_PICTURE_SIZE {
                crate::core::diagnostics::push_warning(
                    warnings,
                    PPT_WARNING_SOURCE,
                    format!(
                        "Embedded picture at offset {pos} ({picture_len} bytes) exceeds the \
                         {MAX_PICTURE_SIZE}-byte size cap and was skipped"
                    ),
                );
                pos = content_end;
                continue;
            }

            let picture_start = content_start + header_len;
            let picture_bytes = &data[picture_start..content_end];

            images.push(ExtractedImage {
                data: Bytes::copy_from_slice(picture_bytes),
                format,
                image_index,
                page_number: None,
                width: None,
                height: None,
                colorspace: None,
                bits_per_component: None,
                is_mask: false,
                description: None,
                ocr_result: None,
                bounding_box: None,
                source_path: None,
                image_kind: None,
                kind_confidence: None,
                cluster_id: None,
                caption: None,
                qr_codes: None,
                data_base64: None,
            });
            image_index += 1;
        }

        pos = content_end;
    }

    images
}

/// Clean PPT text: replace control characters and normalize whitespace.
fn clean_ppt_text(text: &str) -> String {
    let mut result = String::with_capacity(text.len());

    for c in text.chars() {
        match c {
            '\r' => result.push('\n'),
            '\x0B' => result.push('\n'),
            c if c < '\x20' && c != '\n' && c != '\t' => {}
            _ => result.push(c),
        }
    }

    let cleaned = result
        .lines()
        .map(|line| line.trim_end())
        .collect::<Vec<_>>()
        .join("\n");

    let trimmed = cleaned.trim();
    if trimmed.chars().all(|c| c == '*' || c == '\n' || c.is_whitespace()) {
        return String::new();
    }

    cleaned
}

/// Convert CP1252 byte to Unicode char.
fn cp1252_to_char(b: u8) -> char {
    match b {
        0x80 => '\u{20AC}',
        0x82 => '\u{201A}',
        0x83 => '\u{0192}',
        0x84 => '\u{201E}',
        0x85 => '\u{2026}',
        0x86 => '\u{2020}',
        0x87 => '\u{2021}',
        0x88 => '\u{02C6}',
        0x89 => '\u{2030}',
        0x8A => '\u{0160}',
        0x8B => '\u{2039}',
        0x8C => '\u{0152}',
        0x8E => '\u{017D}',
        0x91 => '\u{2018}',
        0x92 => '\u{2019}',
        0x93 => '\u{201C}',
        0x94 => '\u{201D}',
        0x95 => '\u{2022}',
        0x96 => '\u{2013}',
        0x97 => '\u{2014}',
        0x98 => '\u{02DC}',
        0x99 => '\u{2122}',
        0x9A => '\u{0161}',
        0x9B => '\u{203A}',
        0x9C => '\u{0153}',
        0x9E => '\u{017E}',
        0x9F => '\u{0178}',
        b => b as char,
    }
}

/// Read a named stream from the CFB compound file.
fn read_stream(comp: &mut cfb::CompoundFile<Cursor<&[u8]>>, name: &str) -> Result<Vec<u8>> {
    use std::io::Read;
    let mut stream = comp
        .open_stream(name)
        .map_err(|e| XbergError::parsing(format!("Failed to open stream '{name}': {e}")))?;
    let mut data = Vec::new();
    stream
        .read_to_end(&mut data)
        .map_err(|e| XbergError::parsing(format!("Failed to read stream '{name}': {e}")))?;
    Ok(data)
}

/// Extract metadata from OLE summary information streams.
fn extract_ppt_metadata(comp: &mut cfb::CompoundFile<Cursor<&[u8]>>) -> PptMetadata {
    let mut meta = PptMetadata::default();

    if let Ok(data) = read_stream(comp, "/\x05SummaryInformation") {
        parse_summary_info(&data, &mut meta);
    }

    meta
}

/// Parse OLE SummaryInformation for PPT metadata.
fn parse_summary_info(data: &[u8], meta: &mut PptMetadata) {
    if data.len() < 48 {
        return;
    }

    let set_offset = u32::from_le_bytes([data[44], data[45], data[46], data[47]]) as usize;

    if set_offset + 8 > data.len() {
        return;
    }

    let num_props = u32::from_le_bytes([
        data[set_offset + 4],
        data[set_offset + 5],
        data[set_offset + 6],
        data[set_offset + 7],
    ]) as usize;

    let props_start = set_offset + 8;

    for i in 0..num_props {
        let entry_offset = props_start + i * 8;
        if entry_offset + 8 > data.len() {
            break;
        }

        let prop_id = u32::from_le_bytes([
            data[entry_offset],
            data[entry_offset + 1],
            data[entry_offset + 2],
            data[entry_offset + 3],
        ]);
        let prop_offset = u32::from_le_bytes([
            data[entry_offset + 4],
            data[entry_offset + 5],
            data[entry_offset + 6],
            data[entry_offset + 7],
        ]) as usize;

        let abs_offset = set_offset + prop_offset;
        if abs_offset + 8 > data.len() {
            continue;
        }

        if let Some(value) = read_property_value(data, abs_offset) {
            match prop_id {
                2 => meta.title = Some(value),
                3 => meta.subject = Some(value),
                4 => meta.author = Some(value),
                8 => meta.last_author = Some(value),
                _ => {}
            }
        }
    }
}

/// Read a property value from an OLE property entry.
fn read_property_value(data: &[u8], offset: usize) -> Option<String> {
    if offset + 8 > data.len() {
        return None;
    }

    let vt_type = u32::from_le_bytes([data[offset], data[offset + 1], data[offset + 2], data[offset + 3]]);

    match vt_type {
        30 => {
            let len =
                u32::from_le_bytes([data[offset + 4], data[offset + 5], data[offset + 6], data[offset + 7]]) as usize;
            if len == 0 || offset + 8 + len > data.len() {
                return None;
            }
            let bytes = &data[offset + 8..offset + 8 + len];
            let trimmed = bytes.iter().take_while(|&&b| b != 0).copied().collect::<Vec<_>>();
            Some(String::from_utf8_lossy(&trimmed).to_string())
        }
        31 => {
            let len =
                u32::from_le_bytes([data[offset + 4], data[offset + 5], data[offset + 6], data[offset + 7]]) as usize;
            if len == 0 || offset + 8 + len * 2 > data.len() {
                return None;
            }
            let bytes = &data[offset + 8..offset + 8 + len * 2];
            let chars: Vec<u16> = bytes
                .chunks_exact(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .take_while(|&c| c != 0)
                .collect();
            Some(String::from_utf16_lossy(&chars))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_clean_ppt_text() {
        assert_eq!(clean_ppt_text("Hello\rWorld"), "Hello\nWorld");
        assert_eq!(clean_ppt_text("A\x0BB"), "A\nB");
    }

    #[test]
    fn test_cp1252_to_char() {
        assert_eq!(cp1252_to_char(b'A'), 'A');
        assert_eq!(cp1252_to_char(0x80), '\u{20AC}');
    }

    #[test]
    fn test_extract_ppt_real_file() {
        let test_file = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test_documents/ppt/simple.ppt");
        if !test_file.exists() {
            return;
        }
        let content = std::fs::read(&test_file).expect("Failed to read test PPT");
        let result = extract_ppt_text(&content).expect("Failed to extract PPT text");
        assert!(!result.text.is_empty(), "PPT extraction should produce text");
    }

    #[test]
    fn test_extract_ppt_invalid_data() {
        let result = extract_ppt_text(b"not a ppt file");
        assert!(result.is_err());
    }

    /// #87 regression: `test_documents/ppt/simple.ppt` has exactly two
    /// top-level `Slide` (0x03EE) containers and three `SlideListWithText`
    /// (0x0FF0) containers holding only outline-view `SlidePersistAtom`
    /// entries. Segmenting on `SlideListWithText` collapsed all real slide
    /// text into a single trailing blob and reported the wrong slide count.
    #[test]
    fn test_extract_ppt_real_file_reports_two_slides() {
        let test_file = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test_documents/ppt/simple.ppt");
        if !test_file.exists() {
            return;
        }
        let content = std::fs::read(&test_file).expect("Failed to read test PPT");
        let result = extract_ppt_text(&content).expect("Failed to extract PPT text");
        assert_eq!(result.slide_count, 2, "simple.ppt has exactly two Slide containers");
    }

    /// Build one PowerPoint record header (8 bytes: recVerInstance, recType, recLen).
    fn record_header(rec_ver_instance: u16, rec_type: u16, rec_len: u32) -> Vec<u8> {
        let mut buf = Vec::with_capacity(8);
        buf.extend_from_slice(&rec_ver_instance.to_le_bytes());
        buf.extend_from_slice(&rec_type.to_le_bytes());
        buf.extend_from_slice(&rec_len.to_le_bytes());
        buf
    }

    /// Build a container record (recVer nibble = 0xF) wrapping `children`.
    fn container(rec_type: u16, children: &[u8]) -> Vec<u8> {
        let mut buf = record_header(0x000F, rec_type, children.len() as u32);
        buf.extend_from_slice(children);
        buf
    }

    /// Build a `TextCharsAtom` (UTF-16LE) record for `text`.
    fn text_chars_atom(text: &str) -> Vec<u8> {
        let utf16: Vec<u8> = text.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
        let mut buf = record_header(0x0000, RT_TEXT_CHARS_ATOM, utf16.len() as u32);
        buf.extend_from_slice(&utf16);
        buf
    }

    /// #87: `SlideListWithText` (0x0FF0) is a per-document container of
    /// `SlidePersistAtom` outline-view entries -- it does not occur once per
    /// slide, and (as in real files) commonly holds no text of its own. The
    /// actual per-slide text lives in each `Slide` (0x03EE) container.
    /// Segmenting on `SlideListWithText` merges every slide's text into a
    /// single blob attributed to the wrong slide count; segmenting on
    /// `Slide` keeps each slide's text separate.
    #[test]
    fn test_extract_texts_segments_on_slide_not_slide_list_with_text() {
        const RT_SLIDE_LIST_WITH_TEXT: u16 = 0x0FF0;
        const RT_SLIDE_PERSIST_ATOM: u16 = 0x03F3;

        // A SlideListWithText container holding only SlidePersistAtom entries
        // (no text), exactly as real files lay it out -- this used to be
        // mistaken for a slide boundary.
        let slide_persist_atom = record_header(0x0000, RT_SLIDE_PERSIST_ATOM, 0);
        let bogus_slwt = container(RT_SLIDE_LIST_WITH_TEXT, &slide_persist_atom);

        let slide1 = container(RT_SLIDE, &text_chars_atom("Slide One"));
        let slide2 = container(RT_SLIDE, &text_chars_atom("Slide Two"));

        let mut data = Vec::new();
        data.extend_from_slice(&bogus_slwt);
        data.extend_from_slice(&slide1);
        data.extend_from_slice(&slide2);

        let mut warnings = Vec::new();
        let (slides, loose_texts, notes) =
            extract_texts_from_records(&data, false, &mut warnings).expect("record parsing should succeed");

        assert_eq!(
            slides.len(),
            2,
            "each Slide container is one slide, not each SlideListWithText"
        );
        assert_eq!(slides[0].number, 1);
        assert_eq!(slides[0].text, "Slide One");
        assert_eq!(slides[1].number, 2);
        assert_eq!(slides[1].text, "Slide Two");
        assert!(loose_texts.is_empty());
        assert!(notes.is_empty());
        assert!(warnings.is_empty(), "well-formed records should not warn: {warnings:?}");
    }

    /// A `Notes` container's text must not bleed into the slide that follows
    /// it once its own byte range has ended.
    #[test]
    fn test_extract_texts_closes_notes_range_before_next_slide() {
        let notes = container(RT_NOTES, &text_chars_atom("Speaker notes"));
        let slide1 = container(RT_SLIDE, &text_chars_atom("Slide One"));

        let mut data = Vec::new();
        data.extend_from_slice(&notes);
        data.extend_from_slice(&slide1);

        let mut warnings = Vec::new();
        let (slides, loose_texts, speaker_notes) =
            extract_texts_from_records(&data, false, &mut warnings).expect("record parsing should succeed");

        assert_eq!(slides.len(), 1);
        assert_eq!(slides[0].number, 1);
        assert_eq!(slides[0].text, "Slide One");
        assert!(loose_texts.is_empty());
        assert_eq!(speaker_notes, vec!["Speaker notes".to_string()]);
    }

    /// #1418: a slide's number must come from its position among `RT_SLIDE`
    /// containers, not from the position of a text block after joining and
    /// re-splitting on `"\n\n"`. A slide with no text atoms must still get a
    /// number instead of vanishing and shifting every later slide down.
    #[test]
    fn should_number_slides_by_persist_order_when_a_middle_slide_has_no_text() {
        let slide1 = container(RT_SLIDE, &text_chars_atom("Slide One"));
        let slide2 = container(RT_SLIDE, &[]); // no text atoms at all
        let slide3 = container(RT_SLIDE, &text_chars_atom("Slide Three"));

        let mut data = Vec::new();
        data.extend_from_slice(&slide1);
        data.extend_from_slice(&slide2);
        data.extend_from_slice(&slide3);

        let mut warnings = Vec::new();
        let (slides, _loose_texts, _notes) =
            extract_texts_from_records(&data, false, &mut warnings).expect("record parsing should succeed");

        assert_eq!(
            slides.len(),
            3,
            "the empty middle slide must still produce a slide entry"
        );
        assert_eq!(slides[0].number, 1);
        assert_eq!(slides[0].text, "Slide One");
        assert_eq!(slides[1].number, 2);
        assert_eq!(
            slides[1].text, "",
            "a slide with no text atoms has empty text, not a missing entry"
        );
        assert_eq!(slides[2].number, 3);
        assert_eq!(slides[2].text, "Slide Three");
    }

    /// #1418 root-cause regression: a single slide whose own atoms, once
    /// joined by `clean_ppt_text`'s newline mapping, contain an internal
    /// `"\n\n"` (a text atom ending in a blank trailing paragraph, i.e. two
    /// consecutive `\r` paragraph marks) must still be reported as exactly
    /// one slide. The old algorithm re-split the whole document's text on
    /// `"\n\n"`, so this single slide's own text was itself indistinguishable
    /// from a slide boundary.
    #[test]
    fn should_keep_one_slide_entry_when_slide_text_contains_internal_blank_line() {
        let atom_with_trailing_blank_paragraph = text_chars_atom("Title\r\r");
        let atom_body = text_chars_atom("Body");
        let mut slide_children = Vec::new();
        slide_children.extend_from_slice(&atom_with_trailing_blank_paragraph);
        slide_children.extend_from_slice(&atom_body);
        let slide1 = container(RT_SLIDE, &slide_children);

        let mut data = Vec::new();
        data.extend_from_slice(&slide1);

        let mut warnings = Vec::new();
        let (slides, _loose_texts, _notes) =
            extract_texts_from_records(&data, false, &mut warnings).expect("record parsing should succeed");

        assert_eq!(
            slides.len(),
            1,
            "one Slide container is one slide, however its joined text looks"
        );
        assert_eq!(slides[0].number, 1);
        assert_eq!(
            slides[0].text, "Title\n\nBody",
            "the slide's own text legitimately contains an internal blank line"
        );
    }

    /// Build a raster `OfficeArtBlip` record with a single 16-byte UID
    /// (MS-ODRAW 2.2.27-2.2.29 "one UID" layout: `rgbUid1(16) + tag(1) +
    /// BLIPFileData`). `rec_instance` must be even per spec (e.g. JPEG
    /// 0x46A, PNG 0x6E0, DIB 0x7A8).
    fn blip_record_one_uid(rec_instance: u16, rec_type: u16, picture_bytes: &[u8]) -> Vec<u8> {
        assert_eq!(rec_instance & 0x1, 0, "one-UID recInstance must be even");
        let rec_ver_instance = rec_instance << 4;
        let rec_len = (17 + picture_bytes.len()) as u32;
        let mut buf = record_header(rec_ver_instance, rec_type, rec_len);
        buf.extend_from_slice(&[0u8; 16]);
        buf.push(0xFF);
        buf.extend_from_slice(picture_bytes);
        buf
    }

    /// Build a raster `OfficeArtBlip` record with two 16-byte UIDs
    /// ("two UID" layout: `rgbUid1(16) + rgbUid2(16) + tag(1) +
    /// BLIPFileData`). `rec_instance` must be odd per spec (e.g. JPEG
    /// 0x46B, PNG 0x6E1, DIB 0x7A9).
    fn blip_record_two_uid(rec_instance: u16, rec_type: u16, picture_bytes: &[u8]) -> Vec<u8> {
        assert_eq!(rec_instance & 0x1, 1, "two-UID recInstance must be odd");
        let rec_ver_instance = rec_instance << 4;
        let rec_len = (33 + picture_bytes.len()) as u32;
        let mut buf = record_header(rec_ver_instance, rec_type, rec_len);
        buf.extend_from_slice(&[0u8; 32]);
        buf.push(0xFF);
        buf.extend_from_slice(picture_bytes);
        buf
    }

    #[test]
    fn should_extract_jpeg_bytes_when_pictures_stream_has_one_uid_jpeg_blip() {
        let picture = b"\xFF\xD8\xFFfake-jpeg-payload";
        let data = blip_record_one_uid(0x46A, RT_BLIP_JPEG, picture);

        let mut warnings = Vec::new();
        let images = extract_pictures_from_stream(&data, &mut warnings);

        assert_eq!(images.len(), 1);
        assert_eq!(images[0].format, "jpeg");
        assert_eq!(images[0].image_index, 0);
        assert_eq!(&images[0].data[..], &picture[..]);
        assert!(warnings.is_empty());
    }

    #[test]
    fn should_extract_png_bytes_when_pictures_stream_has_two_uid_png_blip() {
        let picture = b"\x89PNG\r\n\x1a\nfake-png-payload";
        let data = blip_record_two_uid(0x6E1, RT_BLIP_PNG, picture);

        let mut warnings = Vec::new();
        let images = extract_pictures_from_stream(&data, &mut warnings);

        assert_eq!(images.len(), 1);
        assert_eq!(images[0].format, "png");
        assert_eq!(&images[0].data[..], &picture[..]);
        assert!(warnings.is_empty());
    }

    #[test]
    fn should_extract_dib_bytes_and_tag_format_dib_when_pictures_stream_has_dib_blip() {
        let picture = b"fake-dib-bitmap-payload";
        let data = blip_record_one_uid(0x7A8, RT_BLIP_DIB, picture);

        let mut warnings = Vec::new();
        let images = extract_pictures_from_stream(&data, &mut warnings);

        assert_eq!(images.len(), 1);
        assert_eq!(images[0].format, "dib");
        assert_eq!(&images[0].data[..], &picture[..]);
    }

    #[test]
    fn should_assign_sequential_image_index_when_pictures_stream_has_multiple_blips() {
        let jpeg = blip_record_one_uid(0x46A, RT_BLIP_JPEG, b"jpeg-one");
        let png = blip_record_one_uid(0x6E0, RT_BLIP_PNG, b"png-two");

        let mut data = Vec::new();
        data.extend_from_slice(&jpeg);
        data.extend_from_slice(&png);

        let mut warnings = Vec::new();
        let images = extract_pictures_from_stream(&data, &mut warnings);

        assert_eq!(images.len(), 2);
        assert_eq!(images[0].image_index, 0);
        assert_eq!(images[0].format, "jpeg");
        assert_eq!(images[1].image_index, 1);
        assert_eq!(images[1].format, "png");
    }

    #[test]
    fn should_skip_non_blip_records_when_walking_pictures_stream() {
        // An arbitrary non-blip OfficeArt record (a group shape record,
        // 0xF003) sitting between two real blips must not be mistaken for a
        // picture and must not stop the walk.
        const RT_UNRELATED: u16 = 0xF003;
        let unrelated = record_header(0x0000, RT_UNRELATED, 4)
            .into_iter()
            .chain([1, 2, 3, 4])
            .collect::<Vec<u8>>();
        let jpeg = blip_record_one_uid(0x46A, RT_BLIP_JPEG, b"real-jpeg");

        let mut data = Vec::new();
        data.extend_from_slice(&unrelated);
        data.extend_from_slice(&jpeg);

        let mut warnings = Vec::new();
        let images = extract_pictures_from_stream(&data, &mut warnings);

        assert_eq!(images.len(), 1);
        assert_eq!(images[0].format, "jpeg");
    }

    /// Safety: a record whose declared `recLen` overruns the remaining
    /// buffer must never panic or over-read -- the walk stops and a
    /// diagnostic warning is recorded instead.
    #[test]
    fn should_stop_without_panicking_when_blip_declares_length_past_buffer_end() {
        let mut data = record_header(0x46A << 4, RT_BLIP_JPEG, u32::MAX);
        data.extend_from_slice(&[0u8; 4]); // far short of the declared recLen

        let mut warnings = Vec::new();
        let images = extract_pictures_from_stream(&data, &mut warnings);

        assert!(images.is_empty());
        assert!(
            warnings.iter().any(|w| w.message.contains("truncated")),
            "expected a truncation warning, got: {warnings:?}"
        );
    }

    /// Safety: a blip record declaring fewer bytes than its own UID header
    /// requires must be skipped, not underflow-subtracted into a bogus
    /// picture length.
    #[test]
    fn should_skip_and_warn_when_blip_declared_length_is_shorter_than_uid_header() {
        // recLen = 5, far short of the 17-byte one-UID header.
        let data = record_header(0x46A << 4, RT_BLIP_JPEG, 5)
            .into_iter()
            .chain([0u8; 5])
            .collect::<Vec<u8>>();

        let mut warnings = Vec::new();
        let images = extract_pictures_from_stream(&data, &mut warnings);

        assert!(images.is_empty());
        assert!(
            warnings
                .iter()
                .any(|w| w.message.contains("shorter than its UID header")),
            "expected a UID-header-too-short warning, got: {warnings:?}"
        );
    }

    #[test]
    fn should_return_no_images_when_pictures_stream_is_empty() {
        let mut warnings = Vec::new();
        let images = extract_pictures_from_stream(&[], &mut warnings);
        assert!(images.is_empty());
        assert!(warnings.is_empty());
    }

    /// Build a minimal OLE/CFB container with a "PowerPoint Document" stream
    /// and, optionally, a "Pictures" stream, mirroring what a real `.ppt`
    /// looks like closely enough to drive `extract_ppt_text_with_options`
    /// end-to-end. `test_documents/ppt/simple.ppt` has a `Pictures` stream
    /// but it is empty (verified: 0 bytes), so this synthetic container is
    /// the only way to exercise the `/Pictures` read path with real blips.
    fn build_test_ppt_ole(ppt_document_stream: &[u8], pictures_stream: Option<&[u8]>) -> Vec<u8> {
        use std::io::Write;
        let cursor = Cursor::new(Vec::new());
        let mut comp = cfb::CompoundFile::create(cursor).expect("create in-memory OLE container");
        comp.create_stream("/PowerPoint Document")
            .expect("create PowerPoint Document stream")
            .write_all(ppt_document_stream)
            .expect("write PowerPoint Document stream");
        if let Some(pictures) = pictures_stream {
            comp.create_stream("/Pictures")
                .expect("create Pictures stream")
                .write_all(pictures)
                .expect("write Pictures stream");
        }
        comp.into_inner().into_inner()
    }

    #[test]
    fn should_populate_images_when_pictures_stream_has_a_blip_and_extract_images_is_true() {
        let ppt_stream = container(RT_SLIDE, &text_chars_atom("Slide One"));
        let picture = b"\xFF\xD8\xFFsynthetic-jpeg-bytes";
        let pictures_stream = blip_record_one_uid(0x46A, RT_BLIP_JPEG, picture);
        let content = build_test_ppt_ole(&ppt_stream, Some(&pictures_stream));

        let result =
            extract_ppt_text_with_options(&content, false, true).expect("synthetic OLE container should parse");

        assert_eq!(result.images.len(), 1);
        assert_eq!(result.images[0].format, "jpeg");
        assert_eq!(result.images[0].data.len(), picture.len());
        assert_eq!(&result.images[0].data[..], &picture[..]);
    }

    #[test]
    fn should_return_no_images_when_extract_images_is_false() {
        let ppt_stream = container(RT_SLIDE, &text_chars_atom("Slide One"));
        let pictures_stream = blip_record_one_uid(0x46A, RT_BLIP_JPEG, b"jpeg-bytes");
        let content = build_test_ppt_ole(&ppt_stream, Some(&pictures_stream));

        let result =
            extract_ppt_text_with_options(&content, false, false).expect("synthetic OLE container should parse");

        assert!(
            result.images.is_empty(),
            "extract_images=false must skip the Pictures stream entirely"
        );
    }

    #[test]
    fn should_return_no_images_when_pictures_stream_is_absent() {
        let ppt_stream = container(RT_SLIDE, &text_chars_atom("Slide One"));
        let content = build_test_ppt_ole(&ppt_stream, None);

        let result =
            extract_ppt_text_with_options(&content, false, true).expect("synthetic OLE container should parse");

        assert!(result.images.is_empty());
    }
}
