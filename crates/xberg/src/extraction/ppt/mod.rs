//! Native PPT (PowerPoint 97-2003) text extraction.
//!
//! Extracts text directly from PowerPoint Binary File Format using OLE/CFB
//! compound document parsing, without requiring LibreOffice.
//!
//! Supports PowerPoint 97, 2000, XP, and 2003 (.ppt) files.

use crate::error::{Result, XbergError};
use crate::types::ProcessingWarning;
use std::io::Cursor;

/// Warning source tag for `.ppt` extraction diagnostics (#171 convention).
const PPT_WARNING_SOURCE: &str = "ppt";

/// Result of PPT text extraction.
#[cfg_attr(alef, alef(skip))]
pub struct PptExtractionResult {
    /// Extracted text content, with slides separated by double newlines.
    pub text: String,
    /// Number of slides found.
    pub slide_count: usize,
    /// Document metadata.
    pub metadata: PptMetadata,
    /// Speaker notes text per slide (if available).
    pub speaker_notes: Vec<String>,
    /// Non-fatal degradations encountered while extracting (see
    /// `core::diagnostics`). Empty when extraction was complete.
    pub processing_warnings: Vec<ProcessingWarning>,
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

/// Extract text from PPT bytes.
///
/// Parses the OLE/CFB compound document, reads the "PowerPoint Document" stream,
/// and extracts text from TextCharsAtom and TextBytesAtom records.
///
/// When `include_master_slides` is `true`, master slide content (placeholder text
/// like "Click to edit Master title style") is included instead of being skipped.
#[cfg(test)]
pub(crate) fn extract_ppt_text(content: &[u8]) -> Result<PptExtractionResult> {
    extract_ppt_text_with_options(content, false)
}

/// Extract text from PPT bytes with configurable master slide inclusion.
///
/// When `include_master_slides` is `true`, `RT_MAIN_MASTER` containers are not
/// skipped, so master slide placeholder text is included in the output.
pub(crate) fn extract_ppt_text_with_options(
    content: &[u8],
    include_master_slides: bool,
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
    let (texts, slide_count, speaker_notes) =
        extract_texts_from_records(&ppt_stream, include_master_slides, &mut processing_warnings)?;

    let text = texts
        .into_iter()
        .filter(|t| !t.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");

    Ok(PptExtractionResult {
        text: text.trim().to_string(),
        slide_count,
        metadata,
        speaker_notes,
        processing_warnings,
    })
}

/// Parse PowerPoint record headers and extract text atoms.
///
/// Returns `(slide_texts, slide_count, speaker_notes)`.
///
/// When `include_master_slides` is `true`, master slide containers are not
/// skipped, allowing their placeholder text to appear in the output.
fn extract_texts_from_records(
    data: &[u8],
    include_master_slides: bool,
    warnings: &mut Vec<ProcessingWarning>,
) -> Result<(Vec<String>, usize, Vec<String>)> {
    let mut texts = Vec::new();
    let mut slide_count = 0;
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
            if !current_slide_texts.is_empty() {
                texts.push(current_slide_texts.join("\n"));
                current_slide_texts.clear();
            }
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
                if in_slide_text && !current_slide_texts.is_empty() {
                    texts.push(current_slide_texts.join("\n"));
                    current_slide_texts.clear();
                }
                in_slide_text = true;
                slide_end = Some(content_end);
                slide_count += 1;
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
                            texts.push(cleaned);
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
                            texts.push(cleaned);
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

    if !current_slide_texts.is_empty() {
        texts.push(current_slide_texts.join("\n"));
    }

    if !current_notes_texts.is_empty() {
        let notes_text = current_notes_texts.join("\n");
        let trimmed = notes_text.trim().to_string();
        if !trimmed.is_empty() {
            speaker_notes.push(trimmed);
        }
    }

    if slide_count == 0 && !texts.is_empty() {
        slide_count = 1;
    }

    Ok((texts, slide_count, speaker_notes))
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
        let (texts, slide_count, notes) =
            extract_texts_from_records(&data, false, &mut warnings).expect("record parsing should succeed");

        assert_eq!(
            slide_count, 2,
            "each Slide container is one slide, not each SlideListWithText"
        );
        assert_eq!(texts, vec!["Slide One".to_string(), "Slide Two".to_string()]);
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
        let (texts, slide_count, speaker_notes) =
            extract_texts_from_records(&data, false, &mut warnings).expect("record parsing should succeed");

        assert_eq!(slide_count, 1);
        assert_eq!(texts, vec!["Slide One".to_string()]);
        assert_eq!(speaker_notes, vec!["Speaker notes".to_string()]);
    }
}
