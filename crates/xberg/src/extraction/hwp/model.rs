/// Minimal document model for HWP text extraction.
///
/// Only the types needed to walk body-text sections and collect plain text.
use super::error::Result;
use super::parser::Record;
// ---------------------------------------------------------------------------
// ---------------------------------------------------------------------------

#[cfg_attr(alef, alef(skip))]
/// An extracted HWP document, consisting of one or more body-text sections.
#[derive(Debug, Default)]
pub struct HwpDocument {
    /// All sections from all BodyText/SectionN streams.
    pub sections: Vec<Section>,
    /// Global character shape table from DocInfo.
    pub char_shapes: Vec<CharShape>,
    /// Extracted images from BinData.
    pub images: Vec<HwpImage>,
    /// Document metadata from the `\x05HwpSummaryInformation` OLE property-set stream,
    /// or `None` if the stream was absent or unparsable (#105).
    pub summary_info: Option<SummaryInfo>,
    /// Human-readable descriptions of content that could not be fully parsed
    /// (truncated body-text streams, unsupported nested tables, etc.) — surfaced by
    /// the extractor as `ProcessingWarning`s instead of silently dropping the data
    /// (#236). Each entry names what was lost, per the extractor warning convention
    /// (`crate::core::diagnostics`).
    pub warnings: Vec<String>,
}

/// A body-text section containing a flat list of paragraphs and tables.
#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct Section {
    /// All paragraphs in this section, in document order.
    pub paragraphs: Vec<Paragraph>,
    /// All tables found in this section (#105, #236). Not interleaved with
    /// `paragraphs` in reading order — the same simplification already used for
    /// `HwpDocument::images`.
    #[serde(default)]
    pub tables: Vec<HwpTable>,
}

/// A table extracted from a `HWPTAG_TABLE` record and its nested `HWPTAG_LIST_HEADER`
/// cells.
#[cfg_attr(alef, alef(skip))]
#[derive(Debug, Default, Clone, serde::Serialize, serde::Deserialize)]
pub struct HwpTable {
    /// Cell text, as a row-major grid (`rows[r][c]`). Cells covered by a merged
    /// (rowspan/colspan > 1) neighbor are empty strings rather than omitted, so every
    /// row has `column_count` entries.
    pub rows: Vec<Vec<String>>,
}

/// Document metadata decoded from the OLE `SummaryInformation`-compatible property-set
/// stream HWP names `\x05HwpSummaryInformation` (same binary layout as the
/// `\x05SummaryInformation` stream used by Microsoft Office documents; see MS-OLEPS).
#[cfg_attr(alef, alef(skip))]
#[derive(Debug, Default, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SummaryInfo {
    /// PIDSI_TITLE (property ID 2).
    pub title: Option<String>,
    /// PIDSI_SUBJECT (property ID 3).
    pub subject: Option<String>,
    /// PIDSI_AUTHOR (property ID 4).
    pub author: Option<String>,
    /// PIDSI_KEYWORDS (property ID 5), stored verbatim (HWP does not split it into a list).
    pub keywords: Option<String>,
    /// PIDSI_COMMENTS (property ID 6).
    pub comments: Option<String>,
    /// PIDSI_LASTAUTHOR (property ID 8).
    pub last_author: Option<String>,
    /// PIDSI_CREATE_DTM (property ID 12), rendered as an RFC 3339 UTC timestamp.
    pub created: Option<String>,
    /// PIDSI_LASTSAVE_DTM (property ID 13), rendered as an RFC 3339 UTC timestamp.
    pub modified: Option<String>,
}

#[cfg_attr(alef, alef(skip))]
/// A single paragraph; may or may not carry a text payload.
#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct Paragraph {
    /// The decoded paragraph text, or `None` if this paragraph has no text record.
    pub text: Option<ParaText>,
    /// Outline level from the ParaShape record (0 = body text, 1–7 = headings).
    pub outline_level: u8,
    /// Mappings from character position to char_shape index.
    pub char_shape_runs: Vec<(u32, u16)>,
    /// The LaTeX of every equation this paragraph holds, in document order.
    ///
    /// The parser knows each equation exactly, because it substitutes the text
    /// itself. Reading it here is exact, where searching the finished text for
    /// `$` would take a price for an equation and an equation for a price.
    pub equations: Vec<String>,
}

/// Character formatting attributes from the HWP DocInfo CharShape table.
#[cfg_attr(alef, alef(skip))]
#[derive(Debug, Clone, Copy, Default)]
pub struct CharShape {
    /// Bold formatting flag.
    pub bold: bool,
    /// Italic formatting flag.
    pub italic: bool,
    /// Underline formatting flag.
    pub underline: bool,
}

/// A raw image blob extracted from a BinData stream in an HWP document.
#[cfg_attr(alef, alef(skip))]
#[derive(Debug, Clone, Default)]
pub struct HwpImage {
    /// Stream path used as a stable identifier (e.g. `"BinData/BIN0001.jpg"`).
    pub name: String,
    /// Raw image bytes as stored in the BinData stream.
    pub data: Vec<u8>,
}

/// Placeholder inserted into `ParaText::content` at the position of an inline
/// equation anchor (extended control `0x000B` with ctrl-id `"eqed"`), reserving a
/// spot for the script that arrives in a later `HWPTAG_EQEDIT` record (#99).
///
/// `U+E000` is the first codepoint of the Private Use Area (Supplementary, Plane 0
/// BMP) and is distinct from the `0xF020..=0xF07F` PUA range HWP itself uses for
/// other inline controls, so it cannot collide with decoded document text.
pub(crate) const EQUATION_PLACEHOLDER: char = '\u{E000}';

/// The two UTF-16 code units spanning the 4-byte equation ctrl-id.
///
/// HWP stores a control's 4-character id byte-reversed, so the "eqed" (EQEdit)
/// anchor is written as the literal bytes `"deqe"` (`0x64 0x65 0x71 0x65`). Reading
/// those bytes back as two little-endian `u16`s (the same code-unit width the
/// surrounding text stream uses) gives `0x6564` then `0x6571`.
const EQED_CTRL_ID: [u16; 2] = [0x6564, 0x6571];

#[cfg_attr(alef, alef(skip))]
/// Plain text content decoded from a ParaText record (tag 0x43).
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct ParaText {
    /// Decoded UTF-8 text with HWP control characters mapped to whitespace or removed.
    /// May contain [`EQUATION_PLACEHOLDER`] markers awaiting substitution.
    pub content: String,
}

impl ParaText {
    /// Decode a ParaText record from raw bytes.
    ///
    /// The data field of a TAG_PARA_TEXT record is a sequence of UTF-16LE code
    /// units.  Control characters < 0x0020 are mapped to whitespace or skipped;
    /// characters in the private-use range 0xF020–0xF07F (HWP internal controls)
    /// are discarded. An inline equation anchor (extended control `0x000B` whose
    /// ctrl-id is `"eqed"`) is replaced with [`EQUATION_PLACEHOLDER`] instead of being
    /// skipped, so the caller can fill in the equation script once the matching
    /// `HWPTAG_EQEDIT` record is parsed.
    pub(crate) fn from_record(record: &Record) -> Result<Self> {
        let mut reader = record.data_reader();
        let mut chars: Vec<u16> = Vec::with_capacity(record.data.len() / 2);

        while reader.remaining() >= 2 {
            chars.push(reader.read_u16()?);
        }

        let mut content = String::with_capacity(chars.len());
        let mut i = 0;
        while i < chars.len() {
            let ch = chars[i];
            match ch {
                0x0000 => {}
                0x0001..=0x0008 => {
                    i += 7;
                }
                0x0009 => {
                    content.push('\t');
                    i += 7;
                }
                0x000A => content.push('\n'),
                0x000D => {}
                0x000B => {
                    if chars.get(i + 1..i + 3) == Some(&EQED_CTRL_ID) {
                        content.push(EQUATION_PLACEHOLDER);
                    }
                    i += 7;
                }
                0x000C | 0x000E..=0x001F => {
                    i += 7;
                }
                0xF020..=0xF07F => {}
                _ => {
                    if let Some(c) = char::from_u32(ch as u32) {
                        content.push(c);
                    }
                }
            }
            i += 1;
        }

        Ok(Self { content })
    }
}

/// Replaces the leftmost remaining [`EQUATION_PLACEHOLDER`] in `content` with
/// `replacement`, returning `true` if a placeholder was found. Equation scripts fill
/// slots in the same left-to-right order they were reserved in, so repeated calls
/// with successive `HWPTAG_EQEDIT` records naturally resolve to the correct slot
/// without tracking explicit indices.
pub(crate) fn fill_next_equation_placeholder(content: &mut String, replacement: &str) -> bool {
    if let Some(pos) = content.find(EQUATION_PLACEHOLDER) {
        content.replace_range(pos..pos + EQUATION_PLACEHOLDER.len_utf8(), replacement);
        true
    } else {
        false
    }
}

#[cfg(test)]
mod equation_placeholder_tests {
    use super::*;

    #[test]
    fn should_replace_leftmost_placeholder_first() {
        let mut content = format!("a{EQUATION_PLACEHOLDER}b{EQUATION_PLACEHOLDER}c");
        assert!(fill_next_equation_placeholder(&mut content, "X"));
        assert_eq!(content, format!("aXb{EQUATION_PLACEHOLDER}c"));
        assert!(fill_next_equation_placeholder(&mut content, "Y"));
        assert_eq!(content, "aXbYc");
    }

    #[test]
    fn should_return_false_when_no_placeholder_present() {
        let mut content = String::from("plain text");
        assert!(!fill_next_equation_placeholder(&mut content, "X"));
        assert_eq!(content, "plain text");
    }

    fn utf16le(s: &str) -> Vec<u8> {
        s.encode_utf16().flat_map(|u| u.to_le_bytes()).collect()
    }

    #[test]
    fn should_insert_equation_placeholder_for_eqed_anchor() {
        let mut data = utf16le("Result: ");
        data.extend_from_slice(&0x000Bu16.to_le_bytes());
        data.extend_from_slice(b"deqe"); // ctrl-id, byte-reversed "eqed"
        data.extend_from_slice(&[0u8; 10]); // instance id + reserved
        data.extend_from_slice(&utf16le(" done"));

        let record = Record {
            tag_id: 0x43,
            level: 1,
            data,
        };
        let para_text = ParaText::from_record(&record).expect("decode must succeed");
        assert_eq!(para_text.content, format!("Result: {EQUATION_PLACEHOLDER} done"));
    }

    #[test]
    fn should_not_insert_placeholder_for_other_extended_controls() {
        // Same extended-control shape (0x000B + 4-byte ctrl-id + 10 reserved bytes),
        // but a different ctrl-id (a table/GSO anchor, "gso " reversed) — must be
        // skipped like before, not mistaken for an equation.
        let mut data = utf16le("Before ");
        data.extend_from_slice(&0x000Bu16.to_le_bytes());
        data.extend_from_slice(b" osg");
        data.extend_from_slice(&[0u8; 10]);
        data.extend_from_slice(&utf16le("after"));

        let record = Record {
            tag_id: 0x43,
            level: 1,
            data,
        };
        let para_text = ParaText::from_record(&record).expect("decode must succeed");
        assert_eq!(para_text.content, "Before after");
        assert!(!para_text.content.contains(EQUATION_PLACEHOLDER));
    }
}
