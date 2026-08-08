//! Native DOC (Word 97-2003) text extraction.
//!
//! Extracts text directly from Word Binary File Format using OLE/CFB
//! compound document parsing, without requiring LibreOffice.
//!
//! Supports Word 97, 2000, XP, and 2003 (.doc) files.

use crate::error::{Result, XbergError};
use crate::types::ProcessingWarning;
use std::io::Cursor;

/// Warning source tag for `.doc` extraction diagnostics (#171 convention).
const DOC_WARNING_SOURCE: &str = "doc";

/// Result of DOC text extraction.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct DocExtractionResult {
    /// Extracted text content. Aliased as `text` for back-compat.
    pub content: String,
    /// Document metadata.
    pub metadata: DocMetadata,
    /// Non-fatal degradations encountered while extracting (see
    /// `core::diagnostics`). Empty when extraction was complete.
    pub processing_warnings: Vec<ProcessingWarning>,
}

/// Metadata extracted from DOC files.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub(crate) struct DocMetadata {
    pub title: Option<String>,
    pub subject: Option<String>,
    pub author: Option<String>,
    pub last_author: Option<String>,
    pub created: Option<String>,
    pub modified: Option<String>,
    pub revision_number: Option<String>,
}

/// Extract text from DOC bytes.
///
/// Parses the OLE/CFB compound document, reads the FIB (File Information Block),
/// and extracts text from the piece table.
pub(crate) fn extract_doc_text(content: &[u8]) -> Result<DocExtractionResult> {
    let cursor = Cursor::new(content);
    let mut comp = cfb::CompoundFile::open(cursor)
        .map_err(|e| XbergError::parsing(format!("Failed to open DOC as OLE container: {e}")))?;

    let metadata = extract_doc_metadata(&mut comp);

    let word_doc = read_stream(&mut comp, "/WordDocument")?;
    if word_doc.len() < 12 {
        return Err(XbergError::parsing("WordDocument stream too short"));
    }

    let w_ident = u16::from_le_bytes([word_doc[0], word_doc[1]]);
    if w_ident != 0xA5EC {
        return Err(XbergError::parsing(format!(
            "Invalid DOC magic number: 0x{w_ident:04X}, expected 0xA5EC"
        )));
    }

    let n_fib = u16::from_le_bytes([word_doc[2], word_doc[3]]);

    let flags_a = u16::from_le_bytes([word_doc[0x0A], word_doc[0x0B]]);
    let use_1table = (flags_a & 0x0200) != 0;

    let table_stream_name = if use_1table { "/1Table" } else { "/0Table" };

    let table_stream = read_stream(&mut comp, table_stream_name)?;

    let mut processing_warnings = Vec::new();

    if n_fib >= 101 {
        extract_text_word97(&word_doc, &table_stream, &mut processing_warnings).map(|text| DocExtractionResult {
            content: text,
            metadata,
            processing_warnings,
        })
    } else {
        extract_text_word6(&word_doc).map(|text| DocExtractionResult {
            content: text,
            metadata,
            processing_warnings,
        })
    }
}

/// Index of `ccpText` (main document CP count) in the FIB's `FibRgLw97`
/// long-word array.
const FIB_LW_IDX_CCP_TEXT: usize = 3;
/// Index of `ccpFtn` (footnote subdocument CP count) in the FIB's `FibRgLw97`
/// long-word array.
const FIB_LW_IDX_CCP_FTN: usize = 4;
/// Index of `ccpHdd` (header/footer subdocument CP count).
const FIB_LW_IDX_CCP_HDD: usize = 5;
// Index 6 (`ccpMcr`) is a deprecated macro-subdocument CP count that the spec
// requires readers to ignore; it does not occupy space in the piece-table CP
// range and is intentionally not modeled here.
/// Index of `ccpAtn` (annotation/comment subdocument CP count).
const FIB_LW_IDX_CCP_ATN: usize = 7;
/// Index of `ccpEdn` (endnote subdocument CP count).
const FIB_LW_IDX_CCP_EDN: usize = 8;
/// Index of `ccpTxbx` (main-document text-box subdocument CP count).
const FIB_LW_IDX_CCP_TXBX: usize = 9;
/// Index of `ccpHdrTxbx` (header text-box subdocument CP count).
const FIB_LW_IDX_CCP_HDR_TXBX: usize = 10;

/// Read one `u32` field from the FIB's `FibRgLw97` long-word array.
///
/// Returns 0 when the FIB is too short to contain the field, matching this
/// module's existing "degrade gracefully" behavior for the base FIB reads.
fn read_lw_field(word_doc: &[u8], rg_lw_offset: usize, index: usize) -> usize {
    let off = rg_lw_offset + index * 4;
    if word_doc.len() < off + 4 {
        return 0;
    }
    u32::from_le_bytes([word_doc[off], word_doc[off + 1], word_doc[off + 2], word_doc[off + 3]]) as usize
}

/// A named half-open range in the piece table's CP (character position) space.
///
/// Word lays the main document body and its subdocuments (footnotes,
/// headers/footers, comments, text boxes, ...) out as consecutive CP ranges
/// sharing a single piece table (MS-DOC 2.4.2 "Retrieving Text").
#[derive(Debug, Clone, Copy)]
struct SubdocRange {
    start: usize,
    end: usize,
}

impl SubdocRange {
    fn len(&self) -> usize {
        self.end.saturating_sub(self.start)
    }
}

/// The CP-space subdocument ranges this extractor recognizes and extracts,
/// plus whether any *unrecognized* subdocument (endnotes, header text boxes)
/// consumes CPs -- tracked only so callers can warn that that content was not
/// extracted (#77 covers footnotes/headers/comments/text boxes, not endnotes
/// or header text boxes).
struct SubdocRanges {
    main: SubdocRange,
    footnote: SubdocRange,
    header: SubdocRange,
    annotation: SubdocRange,
    /// Endnotes are not extracted (out of #77's stated scope), but their CP
    /// span must still be accounted for so later subdocuments (text boxes)
    /// resolve to the correct offsets.
    endnote: SubdocRange,
    textbox: SubdocRange,
    /// Header text boxes are not extracted (out of #77's stated scope); kept
    /// only to compute `total_cp` correctly and to detect that content was
    /// left out (see `has_unextracted_subdocument`).
    header_textbox: SubdocRange,
}

impl SubdocRanges {
    /// Derive the CP-space layout from the FIB's `ccp*` fields.
    ///
    /// Order matches the FIB's `FibRgLw97` field declaration order, which is
    /// also the documents' physical CP-space layout (MS-DOC 2.4.2): main
    /// document, footnotes, headers/footers, comments, endnotes, text boxes,
    /// header text boxes. `ccpMcr` (the deprecated macro subdocument) is
    /// skipped: it does not occupy CP space.
    fn from_fib(word_doc: &[u8], rg_lw_offset: usize, ccp_text: usize) -> Self {
        let ccp_ftn = read_lw_field(word_doc, rg_lw_offset, FIB_LW_IDX_CCP_FTN);
        let ccp_hdd = read_lw_field(word_doc, rg_lw_offset, FIB_LW_IDX_CCP_HDD);
        let ccp_atn = read_lw_field(word_doc, rg_lw_offset, FIB_LW_IDX_CCP_ATN);
        let ccp_edn = read_lw_field(word_doc, rg_lw_offset, FIB_LW_IDX_CCP_EDN);
        let ccp_txbx = read_lw_field(word_doc, rg_lw_offset, FIB_LW_IDX_CCP_TXBX);
        let ccp_hdr_txbx = read_lw_field(word_doc, rg_lw_offset, FIB_LW_IDX_CCP_HDR_TXBX);

        let main = SubdocRange {
            start: 0,
            end: ccp_text,
        };
        let footnote = SubdocRange {
            start: main.end,
            end: main.end + ccp_ftn,
        };
        let header = SubdocRange {
            start: footnote.end,
            end: footnote.end + ccp_hdd,
        };
        let annotation = SubdocRange {
            start: header.end,
            end: header.end + ccp_atn,
        };
        let endnote = SubdocRange {
            start: annotation.end,
            end: annotation.end + ccp_edn,
        };
        let textbox = SubdocRange {
            start: endnote.end,
            end: endnote.end + ccp_txbx,
        };
        let header_textbox = SubdocRange {
            start: textbox.end,
            end: textbox.end + ccp_hdr_txbx,
        };

        Self {
            main,
            footnote,
            header,
            annotation,
            endnote,
            textbox,
            header_textbox,
        }
    }

    /// Whether the document has endnote or header-text-box content that this
    /// extractor does not extract.
    fn has_unextracted_subdocument(&self) -> bool {
        self.endnote.len() > 0 || self.header_textbox.len() > 0
    }

    /// Total CP-space size across every subdocument, including the ones this
    /// extractor does not (yet) extract -- needed so the piece-table walk
    /// doesn't stop before consuming pieces that belong to earlier
    /// subdocuments just because a later one is unaccounted for.
    fn total_cp(&self) -> usize {
        self.header_textbox.end
    }
}

/// Text collected from each recognized subdocument during a single pass over
/// the piece table.
#[derive(Default)]
struct SubdocumentText {
    main: String,
    footnote: String,
    header: String,
    annotation: String,
    textbox: String,
}

/// Extract text from Word 97/2000/XP/2003 files using the piece table.
fn extract_text_word97(word_doc: &[u8], table_stream: &[u8], warnings: &mut Vec<ProcessingWarning>) -> Result<String> {
    let fib_base_size = 32;
    let csw_offset = fib_base_size;

    if word_doc.len() < csw_offset + 2 {
        return Err(XbergError::parsing("FIB too short for csw"));
    }

    let csw = u16::from_le_bytes([word_doc[csw_offset], word_doc[csw_offset + 1]]) as usize;
    let rg_w_offset = csw_offset + 2;
    let cslw_offset = rg_w_offset + csw * 2;

    if word_doc.len() < cslw_offset + 2 {
        return Err(XbergError::parsing("FIB too short for cslw"));
    }

    let cslw = u16::from_le_bytes([word_doc[cslw_offset], word_doc[cslw_offset + 1]]) as usize;
    let rg_lw_offset = cslw_offset + 2;

    let ccp_text_offset = rg_lw_offset + FIB_LW_IDX_CCP_TEXT * 4;
    if word_doc.len() < ccp_text_offset + 4 {
        return Err(XbergError::parsing("FIB too short for ccpText"));
    }

    let ccp_text = u32::from_le_bytes([
        word_doc[ccp_text_offset],
        word_doc[ccp_text_offset + 1],
        word_doc[ccp_text_offset + 2],
        word_doc[ccp_text_offset + 3],
    ]) as usize;

    let subdoc_ranges = SubdocRanges::from_fib(word_doc, rg_lw_offset, ccp_text);
    let mut total_cp = subdoc_ranges.total_cp();
    if total_cp > 0 {
        total_cp += 1;
    }

    let cbrgfclcb_offset = rg_lw_offset + cslw * 4;
    if word_doc.len() < cbrgfclcb_offset + 2 {
        return Err(XbergError::parsing("FIB too short for cbRgFcLcb"));
    }

    let _ = u16::from_le_bytes([word_doc[cbrgfclcb_offset], word_doc[cbrgfclcb_offset + 1]]) as usize;
    let rg_fc_lcb_offset = cbrgfclcb_offset + 2;

    let fc_clx_offset = rg_fc_lcb_offset + 66 * 8;
    let lcb_clx_offset = fc_clx_offset + 4;

    if word_doc.len() < lcb_clx_offset + 4 {
        return Err(XbergError::parsing("FIB too short for fcClx/lcbClx"));
    }

    let fc_clx = u32::from_le_bytes([
        word_doc[fc_clx_offset],
        word_doc[fc_clx_offset + 1],
        word_doc[fc_clx_offset + 2],
        word_doc[fc_clx_offset + 3],
    ]) as usize;
    let lcb_clx = u32::from_le_bytes([
        word_doc[lcb_clx_offset],
        word_doc[lcb_clx_offset + 1],
        word_doc[lcb_clx_offset + 2],
        word_doc[lcb_clx_offset + 3],
    ]) as usize;

    if fc_clx == 0 || lcb_clx == 0 {
        return extract_text_contiguous(word_doc, ccp_text);
    }

    if table_stream.len() < fc_clx + lcb_clx {
        return Err(XbergError::parsing("CLX extends beyond table stream"));
    }

    let clx = &table_stream[fc_clx..fc_clx + lcb_clx];

    let mut pos = 0;
    while pos < clx.len() {
        let clxt = clx[pos];
        if clxt == 0x02 {
            pos += 1;
            if pos + 4 > clx.len() {
                return Err(XbergError::parsing("Pcdt truncated at lcb"));
            }
            let _ = u32::from_le_bytes([clx[pos], clx[pos + 1], clx[pos + 2], clx[pos + 3]]) as usize;
            pos += 4;

            let plc_pcd = &clx[pos..];
            return extract_text_from_piece_table(word_doc, plc_pcd, &subdoc_ranges, total_cp, warnings);
        } else if clxt == 0x01 {
            pos += 1;
            if pos + 2 > clx.len() {
                break;
            }
            let cb_grpprl = u16::from_le_bytes([clx[pos], clx[pos + 1]]) as usize;
            pos += 2 + cb_grpprl;
        } else {
            break;
        }
    }

    extract_text_fallback(word_doc, ccp_text)
}

/// Record that a piece's declared byte range runs past the end of the
/// WordDocument stream (#92). The piece table on a malformed or truncated
/// `.doc` can declare an FC/length pair that overruns the stream; previously
/// this was silently clamped (compressed pieces) or dropped entirely
/// (uncompressed pieces), producing a document that looked complete but was
/// truncated.
fn push_piece_overrun_warning(
    warnings: &mut Vec<ProcessingWarning>,
    piece_index: usize,
    declared_end: usize,
    stream_len: usize,
) {
    let message = format!(
        "Piece {piece_index} in the .doc piece table declares a byte range ending at \
         byte {declared_end}, past the end of the WordDocument stream ({stream_len} bytes); \
         the piece's text beyond the stream end was dropped"
    );
    crate::core::diagnostics::push_warning(warnings, DOC_WARNING_SOURCE, message);
}

/// Decode up to `char_count` characters for one piece-table piece, mirroring
/// the compressed (CP1252, 1 byte/char) vs. uncompressed (UTF-16LE, 2
/// bytes/char) layouts used by the legacy `.doc` piece table, plus the
/// CJK-heuristic fallback for uncompressed pieces that decode as mostly CJK
/// ideographs (a strong signal the piece is actually CP1252, not UTF-16).
///
/// Returns fewer than `char_count` characters -- and records a warning via
/// [`push_piece_overrun_warning`] -- when the piece's declared FC/length
/// overruns `word_doc` (#92).
fn decode_piece_chars(
    word_doc: &[u8],
    fc_raw: u32,
    char_count: usize,
    piece_index: usize,
    warnings: &mut Vec<ProcessingWarning>,
) -> Vec<char> {
    let is_compressed = (fc_raw & 0x4000_0000) != 0;
    let fc = (fc_raw & 0x3FFF_FFFF) as usize;
    // Compressed (CP1252) pieces address `word_doc` at half the raw FC value;
    // uncompressed (UTF-16LE) pieces address it directly.
    let byte_offset = if is_compressed { fc / 2 } else { fc };

    if is_compressed {
        let end = byte_offset + char_count;
        let available_end = if end > word_doc.len() {
            push_piece_overrun_warning(warnings, piece_index, end, word_doc.len());
            word_doc.len()
        } else {
            end
        };
        if byte_offset >= available_end {
            return Vec::new();
        }
        word_doc[byte_offset..available_end]
            .iter()
            .map(|&b| cp1252_to_char(b))
            .collect()
    } else {
        let end = byte_offset + char_count * 2;
        let available_end = if end > word_doc.len() {
            push_piece_overrun_warning(warnings, piece_index, end, word_doc.len());
            byte_offset + ((word_doc.len().saturating_sub(byte_offset)) / 2) * 2
        } else {
            end
        };
        let chars: Vec<char> = if byte_offset >= available_end {
            Vec::new()
        } else {
            word_doc[byte_offset..available_end]
                .chunks_exact(2)
                .filter_map(|c| char::from_u32(u16::from_le_bytes([c[0], c[1]]) as u32))
                .collect()
        };

        let suspicious = chars
            .iter()
            .filter(|c| (0x4E00..=0x9FFF).contains(&(**c as u32)))
            .count();
        if chars.len() > 4 && suspicious > chars.len() / 4 {
            let cp1252_end = (byte_offset + char_count).min(word_doc.len());
            if byte_offset >= cp1252_end {
                return Vec::new();
            }
            return word_doc[byte_offset..cp1252_end]
                .iter()
                .map(|&b| cp1252_to_char(b))
                .collect();
        }
        chars
    }
}

/// Append the overlap between `[cp_start, cp_start + chars.len())` and `range`
/// to `out`, translating the overlap into an index range on `chars`.
fn append_range_overlap(chars: &[char], cp_start: usize, range: SubdocRange, out: &mut String) {
    if range.len() == 0 {
        return;
    }
    let piece_end = cp_start + chars.len();
    let overlap_start = cp_start.max(range.start);
    let overlap_end = piece_end.min(range.end);
    if overlap_start < overlap_end {
        out.extend(&chars[overlap_start - cp_start..overlap_end - cp_start]);
    }
}

/// Extract text from the piece table (PlcPcd), bucketing each piece's
/// characters into the subdocument range they fall in (#77: previously any
/// piece whose CP range started at or after `ccpText` -- i.e. every
/// footnote, header/footer, comment and text-box piece -- was silently
/// skipped).
fn extract_text_from_piece_table(
    word_doc: &[u8],
    plc_pcd: &[u8],
    ranges: &SubdocRanges,
    total_cp: usize,
    warnings: &mut Vec<ProcessingWarning>,
) -> Result<String> {
    let plc_size = plc_pcd.len();
    if plc_size < 16 {
        return Err(XbergError::parsing("PlcPcd too small"));
    }

    let n = (plc_size - 4) / 12;
    if n == 0 {
        return Ok(String::new());
    }

    let mut text = SubdocumentText::default();

    for i in 0..n {
        let cp_start_off = i * 4;
        let cp_end_off = (i + 1) * 4;
        let pcd_off = (n + 1) * 4 + i * 8;

        if cp_end_off + 4 > plc_size || pcd_off + 8 > plc_size {
            crate::core::diagnostics::push_warning(
                warnings,
                DOC_WARNING_SOURCE,
                format!(
                    "Piece table truncated after {i} of {n} declared pieces; remaining document text was not extracted"
                ),
            );
            break;
        }

        let cp_start = u32::from_le_bytes([
            plc_pcd[cp_start_off],
            plc_pcd[cp_start_off + 1],
            plc_pcd[cp_start_off + 2],
            plc_pcd[cp_start_off + 3],
        ]) as usize;

        let cp_end = u32::from_le_bytes([
            plc_pcd[cp_end_off],
            plc_pcd[cp_end_off + 1],
            plc_pcd[cp_end_off + 2],
            plc_pcd[cp_end_off + 3],
        ]) as usize;

        if cp_start >= total_cp {
            break;
        }

        let fc_raw = u32::from_le_bytes([
            plc_pcd[pcd_off + 2],
            plc_pcd[pcd_off + 3],
            plc_pcd[pcd_off + 4],
            plc_pcd[pcd_off + 5],
        ]);

        let mut char_count = cp_end.saturating_sub(cp_start);
        if cp_start + char_count > total_cp {
            char_count = total_cp.saturating_sub(cp_start);
        }
        if char_count == 0 {
            continue;
        }

        let chars = decode_piece_chars(word_doc, fc_raw, char_count, i, warnings);
        if chars.is_empty() {
            continue;
        }

        append_range_overlap(&chars, cp_start, ranges.main, &mut text.main);
        append_range_overlap(&chars, cp_start, ranges.footnote, &mut text.footnote);
        append_range_overlap(&chars, cp_start, ranges.header, &mut text.header);
        append_range_overlap(&chars, cp_start, ranges.annotation, &mut text.annotation);
        append_range_overlap(&chars, cp_start, ranges.textbox, &mut text.textbox);
    }

    if ranges.has_unextracted_subdocument() {
        crate::core::diagnostics::push_warning(
            warnings,
            DOC_WARNING_SOURCE,
            "Document contains endnote and/or header-text-box content that is not extracted",
        );
    }

    let mut content = normalize_doc_text(&text.main);
    for (label, section) in [
        ("Footnotes", &text.footnote),
        ("Headers and Footers", &text.header),
        ("Comments", &text.annotation),
        ("Text Boxes", &text.textbox),
    ] {
        let normalized_section = normalize_doc_text(section);
        if !normalized_section.is_empty() {
            if !content.is_empty() {
                content.push_str("\n\n");
            }
            content.push_str(label);
            content.push_str("\n\n");
            content.push_str(&normalized_section);
        }
    }

    Ok(content)
}

/// Extract text from a "simple" DOC file where text is stored contiguously.
///
/// When fcClx=0, the text is stored at offset `fcMin` in the WordDocument stream
/// as either CP1252 (compressed) or UTF-16LE (uncompressed).
fn extract_text_contiguous(word_doc: &[u8], ccp_text: usize) -> Result<String> {
    if word_doc.len() < 0x20 {
        return extract_text_fallback(word_doc, ccp_text);
    }

    let fc_min = u32::from_le_bytes([word_doc[0x18], word_doc[0x19], word_doc[0x1A], word_doc[0x1B]]) as usize;
    let fc_mac = u32::from_le_bytes([word_doc[0x1C], word_doc[0x1D], word_doc[0x1E], word_doc[0x1F]]) as usize;

    if fc_min == 0 || fc_min >= word_doc.len() {
        return extract_text_fallback(word_doc, ccp_text);
    }

    let data_len = fc_mac.saturating_sub(fc_min).min(word_doc.len() - fc_min);
    if data_len == 0 {
        return extract_text_fallback(word_doc, ccp_text);
    }

    let text_data = &word_doc[fc_min..fc_min + data_len];

    let null_count = text_data.iter().filter(|&&b| b == 0).count();
    let is_unicode = data_len >= ccp_text * 2 || null_count > data_len / 4;

    let text = if is_unicode {
        let chars: Vec<u16> = text_data
            .chunks_exact(2)
            .take(ccp_text)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        String::from_utf16_lossy(&chars)
    } else {
        text_data.iter().take(ccp_text).map(|&b| cp1252_to_char(b)).collect()
    };

    let normalized = normalize_doc_text(&text);
    if normalized.is_empty() {
        return extract_text_fallback(word_doc, ccp_text);
    }

    Ok(normalized)
}

/// Fallback text extraction for when the piece table is unavailable.
///
/// Attempts to extract readable text from the WordDocument stream directly.
fn extract_text_fallback(word_doc: &[u8], _ccp_text: usize) -> Result<String> {
    let mut result = String::new();
    let mut text_run = String::new();

    for &b in word_doc.iter().skip(256) {
        if b == 0x0D || b == 0x0A || b == 0x09 || (0x20..=0xFE).contains(&b) {
            text_run.push(cp1252_to_char(b));
        } else if !text_run.is_empty() {
            if text_run.len() >= 3 {
                if !result.is_empty() {
                    result.push(' ');
                }
                result.push_str(&text_run);
            }
            text_run.clear();
        }
    }

    if text_run.len() >= 3 {
        if !result.is_empty() {
            result.push(' ');
        }
        result.push_str(&text_run);
    }

    if result.is_empty() {
        return Err(XbergError::parsing("No text content found in DOC file"));
    }

    Ok(normalize_doc_text(&result))
}

/// Extract text from Word 6/95 files.
///
/// Word 6/95 has a simpler format where text starts at a known offset.
fn extract_text_word6(word_doc: &[u8]) -> Result<String> {
    if word_doc.len() < 0x50 {
        return Err(XbergError::parsing("Word 6/95 file too short"));
    }

    let ccp_text = u32::from_le_bytes([word_doc[0x4C], word_doc[0x4D], word_doc[0x4E], word_doc[0x4F]]) as usize;

    let fc_min = u32::from_le_bytes([word_doc[0x18], word_doc[0x19], word_doc[0x1A], word_doc[0x1B]]) as usize;

    if fc_min + ccp_text > word_doc.len() {
        return extract_text_fallback(word_doc, ccp_text);
    }

    let text_bytes = &word_doc[fc_min..fc_min + ccp_text];
    let mut result = String::with_capacity(ccp_text);

    for &b in text_bytes {
        result.push(cp1252_to_char(b));
    }

    Ok(normalize_doc_text(&result))
}

/// Normalize extracted DOC text: convert special characters and clean up whitespace.
fn normalize_doc_text(text: &str) -> String {
    let mut result = String::with_capacity(text.len());

    for c in text.chars() {
        match c {
            '\r' => result.push('\n'),
            '\x07' => result.push('\t'),
            '\x0B' => result.push('\n'),
            '\x0C' => result.push('\n'),
            '\x01' | '\x08' | '\x13' | '\x14' | '\x15' => {}
            c if c < '\x20' && c != '\n' && c != '\t' => {}
            _ => result.push(c),
        }
    }

    let mut prev_newline = false;
    let mut prev_prev_newline = false;
    let mut cleaned = String::with_capacity(result.len());

    for c in result.chars() {
        if c == '\n' {
            if prev_prev_newline && prev_newline {
                continue;
            }
            prev_prev_newline = prev_newline;
            prev_newline = true;
        } else {
            prev_prev_newline = false;
            prev_newline = false;
        }
        cleaned.push(c);
    }

    cleaned.trim().to_string()
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
fn extract_doc_metadata(comp: &mut cfb::CompoundFile<Cursor<&[u8]>>) -> DocMetadata {
    let mut meta = DocMetadata::default();

    if let Ok(data) = read_stream(comp, "/\x05SummaryInformation") {
        parse_summary_info(&data, &mut meta);
    }

    if let Ok(data) = read_stream(comp, "/\x05DocumentSummaryInformation") {
        parse_doc_summary_info(&data, &mut meta);
    }

    meta
}

/// Parse OLE SummaryInformation property set.
fn parse_summary_info(data: &[u8], meta: &mut DocMetadata) {
    if data.len() < 28 {
        return;
    }

    let offset = 24;
    if data.len() < offset + 4 {
        return;
    }

    let num_sets = u32::from_le_bytes([data[offset], data[offset + 1], data[offset + 2], data[offset + 3]]) as usize;
    if num_sets == 0 {
        return;
    }

    if data.len() < 48 {
        return;
    }
    let set_offset = u32::from_le_bytes([data[44], data[45], data[46], data[47]]) as usize;

    parse_property_set(data, set_offset, meta, false);
}

/// Parse OLE DocumentSummaryInformation property set.
fn parse_doc_summary_info(data: &[u8], meta: &mut DocMetadata) {
    if data.len() < 48 {
        return;
    }

    let set_offset = u32::from_le_bytes([data[44], data[45], data[46], data[47]]) as usize;

    parse_property_set(data, set_offset, meta, true);
}

/// Parse a single property set from OLE property data.
fn parse_property_set(data: &[u8], set_offset: usize, meta: &mut DocMetadata, _is_doc_summary: bool) {
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
                9 => meta.revision_number = Some(value),
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
    fn test_cp1252_to_char_ascii() {
        assert_eq!(cp1252_to_char(b'A'), 'A');
        assert_eq!(cp1252_to_char(b' '), ' ');
        assert_eq!(cp1252_to_char(b'\n'), '\n');
    }

    #[test]
    fn test_cp1252_to_char_special() {
        assert_eq!(cp1252_to_char(0x80), '\u{20AC}');
        assert_eq!(cp1252_to_char(0x93), '\u{201C}');
        assert_eq!(cp1252_to_char(0x94), '\u{201D}');
        assert_eq!(cp1252_to_char(0x96), '\u{2013}');
    }

    #[test]
    fn test_normalize_doc_text() {
        assert_eq!(normalize_doc_text("Hello\rWorld"), "Hello\nWorld");
        assert_eq!(normalize_doc_text("A\x07B"), "A\tB");
        assert_eq!(normalize_doc_text("A\x0BB"), "A\nB");
        assert_eq!(normalize_doc_text("A\n\n\n\nB"), "A\n\nB");
    }

    #[test]
    fn test_normalize_doc_text_field_codes() {
        assert_eq!(normalize_doc_text("A\x13FIELD\x14result\x15B"), "AFIELDresultB");
    }

    #[test]
    fn test_extract_doc_real_file() {
        let test_file = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../test_documents/vendored/unstructured/doc/simple.doc");
        if !test_file.exists() {
            return;
        }
        let content = std::fs::read(&test_file).expect("Failed to read test DOC");
        let result = extract_doc_text(&content).expect("Failed to extract DOC text");
        assert!(!result.content.is_empty(), "DOC extraction should produce text");
    }

    #[test]
    fn test_extract_doc_fake_file() {
        let test_file = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../test_documents/vendored/unstructured/doc/fake.doc");
        if !test_file.exists() {
            return;
        }
        let content = std::fs::read(&test_file).expect("Failed to read test DOC");
        let result = extract_doc_text(&content).expect("Failed to extract DOC text");
        assert!(!result.content.is_empty(), "DOC extraction should produce text");
    }

    #[test]
    fn test_extract_doc_invalid_magic() {
        let result = extract_doc_text(b"not a doc file");
        assert!(result.is_err());
    }

    // --- Synthetic `.doc` byte-fixture helpers for issue #77 / #92 ---
    //
    // No vendored fixture under `test_documents/` has non-empty
    // ccpFtn/ccpAtn or a deliberately-overrunning piece, so these build a
    // minimal OLE compound file directly, matching the exact FIB layout this
    // module reads (fib_base_size=32, csw=14, cslw=22; see
    // `extract_text_word97`).

    const TEST_CSW: usize = 14;
    const TEST_CSLW: usize = 22;
    const TEST_FIB_BASE: usize = 32;

    fn write_u16(buf: &mut [u8], offset: usize, val: u16) {
        buf[offset..offset + 2].copy_from_slice(&val.to_le_bytes());
    }

    fn write_u32(buf: &mut [u8], offset: usize, val: u32) {
        buf[offset..offset + 4].copy_from_slice(&val.to_le_bytes());
    }

    /// `rg_lw_offset` for the layout built by `build_fib`.
    fn test_rg_lw_offset() -> usize {
        let csw_offset = TEST_FIB_BASE;
        let rg_w_offset = csw_offset + 2;
        let cslw_offset = rg_w_offset + TEST_CSW * 2;
        cslw_offset + 2
    }

    /// `fc_clx_offset` for the layout built by `build_fib` (`lcb_clx_offset`
    /// is always `fc_clx_offset + 4`).
    fn test_fc_clx_offset() -> usize {
        let rg_lw_offset = test_rg_lw_offset();
        let cbrgfclcb_offset = rg_lw_offset + TEST_CSLW * 4;
        let rg_fc_lcb_offset = cbrgfclcb_offset + 2;
        rg_fc_lcb_offset + 66 * 8
    }

    /// Build a `len`-byte WordDocument-stream FIB header with the given
    /// `ccp*` fields set. `len` must be large enough to hold the header
    /// (at least `test_fc_clx_offset() + 8`) plus any text placed after it.
    fn build_fib(len: usize, ccp_text: u32, ccp_ftn: u32, ccp_atn: u32, ccp_txbx: u32) -> Vec<u8> {
        let mut buf = vec![0u8; len];
        write_u16(&mut buf, 0, 0xA5EC); // wIdent
        write_u16(&mut buf, 2, 101); // nFib >= 101 selects the Word97+ path
        write_u16(&mut buf, 0x0A, 0x0200); // fWhichTblStm: use 1Table
        write_u16(&mut buf, TEST_FIB_BASE, TEST_CSW as u16);
        let cslw_offset = TEST_FIB_BASE + 2 + TEST_CSW * 2;
        write_u16(&mut buf, cslw_offset, TEST_CSLW as u16);
        let rg_lw_offset = test_rg_lw_offset();
        write_u32(&mut buf, rg_lw_offset + FIB_LW_IDX_CCP_TEXT * 4, ccp_text);
        write_u32(&mut buf, rg_lw_offset + FIB_LW_IDX_CCP_FTN * 4, ccp_ftn);
        write_u32(&mut buf, rg_lw_offset + FIB_LW_IDX_CCP_ATN * 4, ccp_atn);
        write_u32(&mut buf, rg_lw_offset + FIB_LW_IDX_CCP_TXBX * 4, ccp_txbx);
        buf
    }

    struct TestPiece {
        cp_start: u32,
        cp_end: u32,
        fc_raw: u32,
    }

    /// Build a `PlcPcd` (piece table) from a run of contiguous pieces.
    fn build_plc_pcd(pieces: &[TestPiece]) -> Vec<u8> {
        let mut buf = Vec::new();
        for p in pieces {
            buf.extend_from_slice(&p.cp_start.to_le_bytes());
        }
        buf.extend_from_slice(&pieces.last().expect("at least one piece").cp_end.to_le_bytes());
        for p in pieces {
            buf.extend_from_slice(&[0u8, 0u8]);
            buf.extend_from_slice(&p.fc_raw.to_le_bytes());
            buf.extend_from_slice(&[0u8, 0u8]);
        }
        buf
    }

    /// Wire a piece table's `fcClx`/`lcbClx` into `word_doc` and return the
    /// matching `1Table`-stream bytes.
    fn build_table_stream(word_doc: &mut [u8], plc_pcd: &[u8]) -> Vec<u8> {
        const FC_CLX: u32 = 8;
        let mut clx = vec![0x02u8]; // Pcdt marker
        clx.extend_from_slice(&0u32.to_le_bytes()); // lcb (unused by the reader)
        clx.extend_from_slice(plc_pcd);

        let fc_clx_offset = test_fc_clx_offset();
        write_u32(word_doc, fc_clx_offset, FC_CLX);
        write_u32(word_doc, fc_clx_offset + 4, clx.len() as u32);

        let mut table_stream = vec![0u8; FC_CLX as usize];
        table_stream.extend_from_slice(&clx);
        table_stream
    }

    /// A compressed (CP1252, 1 byte/char) FC pointing at `byte_offset` in the
    /// WordDocument stream.
    fn compressed_fc(byte_offset: u32) -> u32 {
        0x4000_0000 | (byte_offset * 2)
    }

    /// Assemble a minimal `.doc` OLE container from prebuilt streams.
    fn build_doc_ole(word_doc: &[u8], table_stream: &[u8]) -> Vec<u8> {
        let cursor = Cursor::new(Vec::new());
        let mut comp = cfb::CompoundFile::create(cursor).expect("create CFB container");
        {
            let mut stream = comp.create_stream("/WordDocument").expect("create WordDocument stream");
            std::io::Write::write_all(&mut stream, word_doc).expect("write WordDocument stream");
        }
        {
            let mut stream = comp.create_stream("/1Table").expect("create 1Table stream");
            std::io::Write::write_all(&mut stream, table_stream).expect("write 1Table stream");
        }
        comp.into_inner().into_inner()
    }

    /// #77: footnotes, headers, comments and text boxes live in subdocument
    /// CP ranges addressed by the FIB's `ccpFtn`/`ccpHdd`/`ccpAtn`/`ccpTxbx`
    /// fields. Previously any piece whose CP range started at or after
    /// `ccpText` was silently skipped, so this content never appeared.
    #[test]
    fn test_extract_doc_includes_footnote_and_comment_subdocuments() {
        let main_text = b"Hello";
        let footnote_text = b"Note one";
        let comment_text = b"See me";

        let ccp_text = main_text.len() as u32;
        let ccp_ftn = footnote_text.len() as u32;
        let ccp_atn = comment_text.len() as u32;

        let word_doc_len = 2048;
        let mut word_doc = build_fib(word_doc_len, ccp_text, ccp_ftn, ccp_atn, 0);

        let main_offset = 900usize;
        let footnote_offset = 950usize;
        let comment_offset = 1000usize;
        word_doc[main_offset..main_offset + main_text.len()].copy_from_slice(main_text);
        word_doc[footnote_offset..footnote_offset + footnote_text.len()].copy_from_slice(footnote_text);
        word_doc[comment_offset..comment_offset + comment_text.len()].copy_from_slice(comment_text);

        let pieces = vec![
            TestPiece {
                cp_start: 0,
                cp_end: ccp_text,
                fc_raw: compressed_fc(main_offset as u32),
            },
            TestPiece {
                cp_start: ccp_text,
                cp_end: ccp_text + ccp_ftn,
                fc_raw: compressed_fc(footnote_offset as u32),
            },
            TestPiece {
                cp_start: ccp_text + ccp_ftn,
                cp_end: ccp_text + ccp_ftn + ccp_atn,
                fc_raw: compressed_fc(comment_offset as u32),
            },
        ];
        let plc_pcd = build_plc_pcd(&pieces);
        let table_stream = build_table_stream(&mut word_doc, &plc_pcd);
        let doc_bytes = build_doc_ole(&word_doc, &table_stream);

        let result = extract_doc_text(&doc_bytes).expect("DOC extraction should succeed");

        assert_eq!(result.content, "Hello\n\nFootnotes\n\nNote one\n\nComments\n\nSee me");
        assert!(
            result.processing_warnings.is_empty(),
            "a complete, well-formed document should not warn: {:?}",
            result.processing_warnings
        );
    }

    /// #92: a piece table entry that declares a byte range past the end of
    /// the WordDocument stream must be reported, not silently clamped or
    /// dropped.
    #[test]
    fn test_extract_doc_warns_when_piece_range_overruns_stream() {
        let ccp_text = 10u32;
        let word_doc_len = 700usize;
        let mut word_doc = build_fib(word_doc_len, ccp_text, 0, 0, 0);

        // Only 3 bytes are actually available at this offset; the piece
        // claims 10 compressed (1 byte/char) characters.
        let byte_offset = (word_doc_len - 3) as u32;
        word_doc[word_doc_len - 3..word_doc_len].copy_from_slice(b"Hi!");

        let pieces = vec![TestPiece {
            cp_start: 0,
            cp_end: ccp_text,
            fc_raw: compressed_fc(byte_offset),
        }];
        let plc_pcd = build_plc_pcd(&pieces);
        let table_stream = build_table_stream(&mut word_doc, &plc_pcd);
        let doc_bytes = build_doc_ole(&word_doc, &table_stream);

        let result = extract_doc_text(&doc_bytes).expect("DOC extraction should succeed despite the overrun");

        assert_eq!(
            result.content, "Hi!",
            "should keep the bytes that ARE available, dropping only the overrun tail"
        );
        assert_eq!(result.processing_warnings.len(), 1);
        assert_eq!(result.processing_warnings[0].source, "doc");
        assert!(
            result.processing_warnings[0]
                .message
                .contains("past the end of the WordDocument stream"),
            "warning should name the overrun: {:?}",
            result.processing_warnings[0].message
        );
    }
}
