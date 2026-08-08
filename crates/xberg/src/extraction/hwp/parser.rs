/// HWP record, file-header, and body-text parsers.
///
/// Consolidated from hwpers parser/record.rs, parser/header.rs, and
/// parser/body_text.rs.
use super::equation;
use super::error::{HwpError, Result};
use super::model::{CharShape, HwpTable, ParaText, Paragraph, Section, fill_next_equation_placeholder};
use super::reader::{StreamReader, decompress_stream};

// ---------------------------------------------------------------------------

const HWP_SIGNATURE: &[u8] = b"HWP Document File";

/// The 256-byte file header at the start of every HWP 5.0 document.
#[cfg_attr(alef, alef(skip))]
#[derive(Debug, Clone)]
pub struct FileHeader {
    /// Flags word from bytes 36–39 (little-endian u32): bit 0 = compressed, bit 1 = encrypted.
    pub flags: u32,
}

impl FileHeader {
    pub(crate) fn parse(data: Vec<u8>) -> Result<Self> {
        if data.len() < 256 {
            return Err(HwpError::InvalidFormat(
                "FileHeader must be at least 256 bytes".to_string(),
            ));
        }

        if &data[..17] != HWP_SIGNATURE {
            return Err(HwpError::InvalidFormat("Invalid HWP signature".to_string()));
        }

        let flags = u32::from_le_bytes([data[36], data[37], data[38], data[39]]);

        Ok(Self { flags })
    }

    /// Whether section streams are zlib/deflate-compressed.
    pub(crate) fn is_compressed(&self) -> bool {
        (self.flags & 0x01) != 0
    }

    /// Whether the document is password-encrypted.
    pub(crate) fn is_encrypted(&self) -> bool {
        (self.flags & 0x02) != 0
    }
}

/// A single HWP binary record decoded from a stream.
///
/// Each record starts with a 32-bit packed header containing the tag ID,
/// nesting level, and payload size; followed by the raw payload bytes.
#[cfg_attr(alef, alef(skip))]
#[derive(Debug)]
pub struct Record {
    /// Tag identifier (10-bit value from the packed record header).
    pub tag_id: u16,
    /// Nesting level (10-bit value from the packed record header). Body-text records
    /// nested inside a control (table cells, embedded objects) carry a level higher
    /// than their containing paragraph; used to find where a table/cell ends (#105).
    pub level: u16,
    /// Raw payload bytes for this record.
    pub data: Vec<u8>,
}

impl Record {
    pub(crate) fn parse(reader: &mut StreamReader) -> Result<Self> {
        if reader.remaining() < 4 {
            return Err(HwpError::ParseError("Not enough data for record header".to_string()));
        }

        let header = reader.read_u32()?;
        let tag_id = (header & 0x3FF) as u16;
        let level = ((header >> 10) & 0x3FF) as u16;
        let mut size = header >> 20;

        if size == 0xFFF {
            size = reader.read_u32()?;
        }

        let data_size = size as usize;
        if data_size > reader.remaining() {
            return Err(HwpError::ParseError(format!(
                "Record size {data_size} exceeds remaining data {}",
                reader.remaining()
            )));
        }

        let data = reader.read_bytes(data_size)?;
        Ok(Self { tag_id, level, data })
    }

    /// Return a fresh `StreamReader` over this record's data bytes.
    pub(crate) fn data_reader(&self) -> StreamReader {
        StreamReader::new(self.data.clone())
    }
}

/// HWPTAG_BEGIN as defined by the HWP 5.x specification.
const HWPTAG_BEGIN: u16 = 0x010;

/// HWP 5.x body-text record tag: paragraph header (HWPTAG_BEGIN + 50 = 0x42).
///
/// Confirmed against a genuine HWP 5.0 document (`test_documents/hwp/styled_document.hwp`):
/// level-0 records at this tag ID carry a fixed 24-byte payload matching the spec's
/// paragraph-header layout, and are followed by tag `0x43` records holding readable
/// UTF-16LE paragraph text — cross-checked against the independently published `unhwp`
/// crate's `TagId::ParaHeader = 66`. The previous value here (`HWPTAG_BEGIN + 64` =
/// `0x50`) never matched any record in that document, so every paragraph's text was
/// silently dropped rather than truncated (#236) — worse than truncation, since no
/// warning could even fire for a loop that "succeeded" by finding nothing to do.
const TAG_PARA_HEADER: u16 = HWPTAG_BEGIN + 50;
/// HWP 5.x body-text record tag: paragraph text, UTF-16LE (HWPTAG_BEGIN + 51 = 0x43).
///
/// See [`TAG_PARA_HEADER`] for how this value was verified. Note this also matches the
/// tag number already named in `ParaText::from_record`'s doc comment ("tag 0x43"),
/// which was inconsistent with the old `0x51` constant used here — independent
/// evidence the intended value was always `0x43`.
const TAG_PARA_TEXT: u16 = HWPTAG_BEGIN + 51;
/// HWP 5.x body-text record tag: list header, marks the start of a table cell's
/// content list (HWPTAG_BEGIN + 56 = 0x48).
const TAG_LIST_HEADER: u16 = HWPTAG_BEGIN + 56;
/// HWP 5.x body-text record tag: table properties (HWPTAG_BEGIN + 61 = 0x4D).
const TAG_TABLE: u16 = HWPTAG_BEGIN + 61;
/// HWP 5.x body-text record tag: equation-editor script (HWPTAG_BEGIN + 72 = 0x58).
const TAG_EQEDIT: u16 = HWPTAG_BEGIN + 72;

/// Paragraph-level tags below are **not** verified against a real document (neither
/// available fixture contains a styled/outlined paragraph exercising them) and are
/// left at their original, likely-incorrect values. They are dead code in practice
/// either way — on a genuine document their old values never matched anything, same
/// as `TAG_PARA_HEADER` before this fix — so leaving them unchanged introduces no
/// regression. Fixing them is out of scope for the issues this module addresses;
/// flagged here for whoever picks up heading/char-shape support next.
const TAG_PARA_SHAPE: u16 = HWPTAG_BEGIN + 66;
const TAG_CHAR_SHAPE: u16 = HWPTAG_BEGIN + 67;

const TAG_CHAR_SHAPE_INFO: u16 = HWPTAG_BEGIN + 30;

pub(crate) fn parse_doc_info(data: Vec<u8>) -> Result<Vec<CharShape>> {
    let mut reader = StreamReader::new(data);
    let mut char_shapes = Vec::new();

    while reader.remaining() >= 4 {
        let record = match Record::parse(&mut reader) {
            Ok(r) => r,
            Err(_) => break,
        };

        if record.tag_id == TAG_CHAR_SHAPE_INFO && record.data.len() >= 4 {
            let font_attr = u32::from_le_bytes([record.data[0], record.data[1], record.data[2], record.data[3]]);
            char_shapes.push(CharShape {
                bold: (font_attr & 0x01) != 0,
                italic: (font_attr & 0x02) != 0,
                underline: (font_attr & 0x04) != 0,
            });
        }
    }

    Ok(char_shapes)
}

/// Decodes the script from a `HWPTAG_EQEDIT` record.
///
/// Record layout (cross-checked against the independently published `unhwp` crate,
/// which cites the HWP 5.0 spec, pyhwp, and hwp-rs for the same layout): a `u32`
/// property field, then a `u16` character count, then that many UTF-16LE code units
/// holding the equation script. Trailing fields (font, baseline, color, version) are
/// not needed for text extraction.
fn parse_eqedit_script(data: &[u8]) -> Option<String> {
    if data.len() < 6 {
        return None;
    }
    let char_count = u16::from_le_bytes([data[4], data[5]]) as usize;
    let script_bytes = data.get(6..6 + char_count * 2)?;
    let units: Vec<u16> = script_bytes
        .chunks_exact(2)
        .map(|b| u16::from_le_bytes([b[0], b[1]]))
        .collect();
    Some(String::from_utf16_lossy(&units).trim().to_string())
}

/// Parse a raw (possibly compressed) BodyText/SectionN stream.
///
/// Returns the list of sections found (always exactly one — a single flat record
/// stream never contains more than one `Section`, matching the pre-existing
/// behavior). Any record the byte stream fails to decode stops parsing and appends a
/// human-readable entry to `warnings` naming how much was abandoned, rather than
/// failing silently (#236).
pub(crate) fn parse_body_text(
    data: Vec<u8>,
    is_compressed: bool,
    stream_name: &str,
    warnings: &mut Vec<String>,
) -> Result<Vec<Section>> {
    let data = if is_compressed { decompress_stream(&data)? } else { data };

    let mut reader = StreamReader::new(data);
    let mut records: Vec<Record> = Vec::new();
    loop {
        if reader.remaining() < 4 {
            break;
        }
        match Record::parse(&mut reader) {
            Ok(record) => records.push(record),
            Err(e) => {
                warnings.push(format!(
                    "HWP body-text parsing in '{stream_name}' stopped after {} record(s); \
                     {} remaining byte(s) were not parsed and their content is missing: {e}",
                    records.len(),
                    reader.remaining()
                ));
                break;
            }
        }
    }

    let (paragraphs, tables) = parse_records_into_paragraphs_and_tables(&records, warnings, stream_name);

    Ok(vec![Section { paragraphs, tables }])
}

/// Walks a flat, already-collected record list, building paragraphs and (via
/// [`TAG_TABLE`]) tables. Equation scripts (`TAG_EQEDIT`) fill the placeholder left by
/// [`ParaText::from_record`] in the most recently decoded paragraph text.
fn parse_records_into_paragraphs_and_tables(
    records: &[Record],
    warnings: &mut Vec<String>,
    stream_name: &str,
) -> (Vec<Paragraph>, Vec<HwpTable>) {
    let mut paragraphs: Vec<Paragraph> = Vec::new();
    let mut tables: Vec<HwpTable> = Vec::new();
    let mut current_paragraph: Option<Paragraph> = None;

    let mut idx = 0;
    while idx < records.len() {
        let record = &records[idx];

        match record.tag_id {
            TAG_PARA_HEADER => {
                if let Some(para) = current_paragraph.take() {
                    paragraphs.push(para);
                }
                current_paragraph = Some(Paragraph::default());
            }
            TAG_PARA_TEXT => {
                if let Some(ref mut para) = current_paragraph
                    && let Ok(text) = ParaText::from_record(record)
                {
                    para.text = Some(text);
                }
            }
            TAG_PARA_SHAPE => {
                if let Some(ref mut para) = current_paragraph
                    && record.data.len() > 18
                {
                    para.outline_level = record.data[18];
                }
            }
            TAG_CHAR_SHAPE => {
                if let Some(ref mut para) = current_paragraph {
                    let mut reader = record.data_reader();
                    while reader.remaining() >= 6 {
                        let pos = reader.read_u32().unwrap_or(0);
                        let shape_idx = reader.read_u16().unwrap_or(0);
                        para.char_shape_runs.push((pos, shape_idx));
                    }
                }
            }
            TAG_EQEDIT => {
                if let Some(script) = parse_eqedit_script(&record.data) {
                    let latex = equation::to_latex(&script);
                    let replacement = format!("${latex}$");

                    match &mut current_paragraph {
                        Some(para) => {
                            // Common case: fill the placeholder the inline "eqed" control
                            // reserved in this paragraph's own text. Computed before the
                            // branch because a pattern guard cannot borrow mutably.
                            let filled = para
                                .text
                                .as_mut()
                                .is_some_and(|text| fill_next_equation_placeholder(&mut text.content, &replacement));
                            if !filled {
                                let existing = para.text.take().map(|t| t.content).unwrap_or_default();
                                warnings.push(format!(
                                    "HWP equation in '{stream_name}' had no reserved inline slot; \
                                     its LaTeX rendering was appended to the paragraph instead of placed inline"
                                ));
                                para.text = Some(super::model::ParaText {
                                    content: format!("{existing}{replacement}"),
                                });
                            }
                        }
                        // Orphan EqEdit with no open paragraph to anchor to — start one
                        // rather than dropping the equation (#99).
                        None => {
                            current_paragraph = Some(Paragraph {
                                text: Some(super::model::ParaText { content: replacement }),
                                ..Paragraph::default()
                            });
                        }
                    }
                }
            }
            TAG_TABLE => {
                let table_level = record.level;
                let table_end = find_block_end(records, idx, table_level);
                if let Some(table) = parse_table_at(&records[idx..table_end]) {
                    tables.push(table);
                }
                idx = table_end;
                continue;
            }
            _ => {}
        }

        idx += 1;
    }

    if let Some(para) = current_paragraph {
        paragraphs.push(para);
    }

    (paragraphs, tables)
}

/// Finds the end (exclusive) of the block starting at `records[start_idx]`: the index
/// of the first later record whose level drops below `base_level`, or `records.len()`
/// if none does.
fn find_block_end(records: &[Record], start_idx: usize, base_level: u16) -> usize {
    for (i, record) in records.iter().enumerate().skip(start_idx + 1) {
        if record.level < base_level {
            return i;
        }
    }
    records.len()
}

/// Finds the end (exclusive) of a table cell: the next `TAG_LIST_HEADER` at the same
/// level (the next cell), or a record whose level drops below the cell's level (end of
/// table).
fn find_cell_end(records: &[Record], start_idx: usize, cell_level: u16) -> usize {
    for (i, record) in records.iter().enumerate().skip(start_idx + 1) {
        if record.level < cell_level {
            return i;
        }
        if record.level == cell_level && record.tag_id == TAG_LIST_HEADER {
            return i;
        }
    }
    records.len()
}

/// Parses a `TAG_TABLE` record and its nested `TAG_LIST_HEADER` cells (`records[0]`
/// must be the `TAG_TABLE` record itself, as produced by [`find_block_end`]).
///
/// Table record layout (cross-checked against `unhwp`, itself cross-checked against
/// pyhwp): offset 0-3 ctrl-id (`"tbl "`), offset 4-5 row count (`u16`), offset 6-7
/// column count (`u16`). Cell (`TAG_LIST_HEADER`) layout: offset 8-9 column index,
/// offset 10-11 row index, offset 12-13 column span, offset 14-15 row span (all
/// `u16`).
fn parse_table_at(records: &[Record]) -> Option<HwpTable> {
    let table_record = records.first()?;
    if table_record.tag_id != TAG_TABLE || table_record.data.len() < 8 {
        return None;
    }

    let row_count = u16::from_le_bytes([table_record.data[4], table_record.data[5]]) as usize;
    let col_count = u16::from_le_bytes([table_record.data[6], table_record.data[7]]) as usize;
    if row_count == 0 || col_count == 0 {
        return None;
    }

    let mut grid: Vec<Vec<String>> = vec![vec![String::new(); col_count]; row_count];

    let mut i = 1;
    while i < records.len() {
        let record = &records[i];
        if record.tag_id == TAG_LIST_HEADER {
            let cell_level = record.level;
            let cell_end = find_cell_end(records, i, cell_level);
            if let Some((row, col, text)) = parse_cell_at(&records[i..cell_end])
                && row < row_count
                && col < col_count
            {
                grid[row][col] = text;
            }
            i = cell_end;
        } else {
            i += 1;
        }
    }

    Some(HwpTable { rows: grid })
}

/// Parses one table cell (`records[0]` must be its `TAG_LIST_HEADER`), returning its
/// `(row, col, text)`.
fn parse_cell_at(records: &[Record]) -> Option<(usize, usize, String)> {
    let list_header = records.first()?;
    if list_header.data.len() < 16 {
        return None;
    }
    let col = u16::from_le_bytes([list_header.data[8], list_header.data[9]]) as usize;
    let row = u16::from_le_bytes([list_header.data[10], list_header.data[11]]) as usize;

    let mut lines: Vec<String> = Vec::new();
    let mut current: Option<Paragraph> = None;
    for record in records.iter().skip(1) {
        match record.tag_id {
            TAG_PARA_HEADER => {
                if let Some(para) = current.take()
                    && let Some(text) = para.text
                {
                    lines.push(text.content);
                }
                current = Some(Paragraph::default());
            }
            TAG_PARA_TEXT => {
                if let Some(ref mut para) = current
                    && let Ok(text) = ParaText::from_record(record)
                {
                    para.text = Some(text);
                }
            }
            _ => {}
        }
    }
    if let Some(para) = current
        && let Some(text) = para.text
    {
        lines.push(text.content);
    }

    Some((row, col, lines.join("\n")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hwp_extract_converted_output() {
        let path =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../test_documents/hwp/converted_output.hwp");
        if !path.exists() {
            println!("Skipping: test document not found at {}", path.display());
            return;
        }
        let bytes = std::fs::read(&path).expect("read file");
        let _doc = crate::extraction::hwp::extract_hwp_document(&bytes).expect("HWP extraction should succeed");
    }

    #[test]
    fn test_hwp_tag_constants() {
        assert_eq!(super::TAG_PARA_HEADER, 0x42);
        assert_eq!(super::TAG_PARA_TEXT, 0x43);
        assert_eq!(super::TAG_LIST_HEADER, 0x48);
        assert_eq!(super::TAG_TABLE, 0x4D);
        assert_eq!(super::TAG_EQEDIT, 0x58);
    }

    fn utf16le(s: &str) -> Vec<u8> {
        s.encode_utf16().flat_map(|u| u.to_le_bytes()).collect()
    }

    fn record_header(tag_id: u16, level: u16, size: u32) -> Vec<u8> {
        let packed = (tag_id as u32 & 0x3FF) | ((level as u32 & 0x3FF) << 10) | ((size.min(0xFFE)) << 20);
        packed.to_le_bytes().to_vec()
    }

    fn make_record(tag_id: u16, level: u16, data: &[u8]) -> Vec<u8> {
        let mut out = record_header(tag_id, level, data.len() as u32);
        out.extend_from_slice(data);
        out
    }

    #[test]
    fn should_stop_and_warn_on_malformed_record_instead_of_silently_truncating() {
        // A record header that claims more payload bytes than actually follow.
        let mut stream = make_record(TAG_PARA_HEADER, 0, &[0u8; 24]);
        stream.extend_from_slice(&utf16le("first paragraph"));
        // A record header claiming a huge size with no matching payload.
        let bad_header = record_header(TAG_PARA_TEXT, 1, 5000);
        stream.extend_from_slice(&bad_header);

        let mut warnings = Vec::new();
        let sections = parse_body_text(stream, false, "Section0", &mut warnings).expect("parse must not error");

        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("Section0"));
        assert!(warnings[0].contains("stopped after"));
        // The one well-formed paragraph parsed before the malformed record must
        // still be preserved (partial results, not an all-or-nothing failure).
        assert_eq!(sections[0].paragraphs.len(), 1);
    }

    #[test]
    fn should_extract_paragraph_header_and_text_at_verified_tag_ids() {
        let mut stream = make_record(TAG_PARA_HEADER, 0, &[0u8; 24]);
        stream.extend(make_record(TAG_PARA_TEXT, 1, &utf16le("Hello, HWP")));

        let mut warnings = Vec::new();
        let sections = parse_body_text(stream, false, "Section0", &mut warnings).unwrap();

        assert!(warnings.is_empty());
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].paragraphs.len(), 1);
        assert_eq!(
            sections[0].paragraphs[0].text.as_ref().map(|t| t.content.as_str()),
            Some("Hello, HWP")
        );
    }

    #[test]
    fn should_fill_inline_equation_slot_with_latex() {
        let mut para_text_data = utf16le("Result: ");
        para_text_data.extend_from_slice(&0x000Bu16.to_le_bytes());
        para_text_data.extend_from_slice(b"deqe");
        para_text_data.extend_from_slice(&[0u8; 10]);
        para_text_data.extend_from_slice(&utf16le(" done"));

        let mut eqedit_data = 0u32.to_le_bytes().to_vec();
        let script = "a OVER b";
        let units: Vec<u16> = script.encode_utf16().collect();
        eqedit_data.extend_from_slice(&(units.len() as u16).to_le_bytes());
        for u in units {
            eqedit_data.extend_from_slice(&u.to_le_bytes());
        }

        let mut stream = make_record(TAG_PARA_HEADER, 0, &[0u8; 24]);
        stream.extend(make_record(TAG_PARA_TEXT, 1, &para_text_data));
        stream.extend(make_record(TAG_EQEDIT, 1, &eqedit_data));

        let mut warnings = Vec::new();
        let sections = parse_body_text(stream, false, "Section0", &mut warnings).unwrap();

        assert!(warnings.is_empty());
        let text = sections[0].paragraphs[0].text.as_ref().unwrap();
        assert_eq!(text.content, "Result: $\\frac{a}{b}$ done");
    }

    #[test]
    fn should_extract_table_rows_and_cell_text_without_swallowing_trailing_paragraph() {
        // Realistic nesting: a paragraph anchors the table (the table's own records
        // sit one level deeper than their anchoring paragraph, matching `unhwp`'s
        // documented "table cells are at the SAME level as the Table record"), and a
        // further top-level paragraph follows the table. This exercises
        // `find_block_end` actually finding the table's end instead of it degenerating
        // to "the rest of the stream" — a table record sitting at level 0 could never
        // be distinguished from a trailing paragraph this way (no level is < 0), but
        // that only matters for a table anchored directly at the document's top level,
        // which HWP's own control-anchoring model does not produce.
        let mut stream = make_record(TAG_PARA_HEADER, 0, &[0u8; 24]);
        stream.extend(make_record(TAG_PARA_TEXT, 1, &utf16le("Table:")));

        let mut table_data = vec![b' ', b'l', b'b', b't']; // ctrl-id "tbl " reversed
        table_data.extend_from_slice(&2u16.to_le_bytes()); // rows
        table_data.extend_from_slice(&2u16.to_le_bytes()); // cols
        stream.extend(make_record(TAG_TABLE, 1, &table_data));

        for (row, col, text) in [(0u16, 0u16, "Name"), (0, 1, "Age"), (1, 0, "Alice"), (1, 1, "30")] {
            let mut list_header = vec![0u8; 16];
            list_header[8..10].copy_from_slice(&col.to_le_bytes());
            list_header[10..12].copy_from_slice(&row.to_le_bytes());
            list_header[12..14].copy_from_slice(&1u16.to_le_bytes());
            list_header[14..16].copy_from_slice(&1u16.to_le_bytes());
            stream.extend(make_record(TAG_LIST_HEADER, 1, &list_header));
            stream.extend(make_record(TAG_PARA_HEADER, 2, &[0u8; 24]));
            stream.extend(make_record(TAG_PARA_TEXT, 3, &utf16le(text)));
        }

        stream.extend(make_record(TAG_PARA_HEADER, 0, &[0u8; 24]));
        stream.extend(make_record(TAG_PARA_TEXT, 1, &utf16le("After table")));

        let mut warnings = Vec::new();
        let sections = parse_body_text(stream, false, "Section0", &mut warnings).unwrap();

        assert!(warnings.is_empty());
        assert_eq!(sections[0].tables.len(), 1);
        assert_eq!(
            sections[0].tables[0].rows,
            vec![
                vec!["Name".to_string(), "Age".to_string()],
                vec!["Alice".to_string(), "30".to_string()],
            ]
        );

        let paragraph_texts: Vec<&str> = sections[0]
            .paragraphs
            .iter()
            .map(|p| p.text.as_ref().map(|t| t.content.as_str()).unwrap_or(""))
            .collect();
        assert_eq!(paragraph_texts, vec!["Table:", "After table"]);
    }
}
