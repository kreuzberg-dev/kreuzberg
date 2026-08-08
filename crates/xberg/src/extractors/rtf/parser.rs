//! Core RTF parsing logic.

use crate::extractors::rtf::encoding::{
    decode_ansi_bytes, fcharset_to_codepage, parse_hex_byte, parse_rtf_control_word,
};
use crate::extractors::rtf::formatting::{map_offset, normalize_whitespace_with_mapping};
use crate::extractors::rtf::images::{RtfImage, extract_pict_image};
use crate::extractors::rtf::tables::TableState;
use crate::types::Table;
use crate::types::TextAnnotation;
use crate::types::document_structure::AnnotationKind;
use std::collections::HashMap;

/// Metadata for a single paragraph extracted from RTF.
#[cfg_attr(alef, alef(skip))]
#[derive(Debug, Clone, Default)]
pub struct ParagraphMeta {
    /// Heading level (1-based): 1 = H1, 2 = H2, etc. 0 = not a heading.
    pub heading_level: u8,
    /// List nesting level (0-based). `None` means not a list item.
    pub list_level: Option<u8>,
    /// List override ID (\lsN). Used to detect list boundaries.
    pub list_id: Option<u16>,
    /// Whether this paragraph is a table placeholder (text is in tables vec).
    pub is_table: bool,
    /// Whether this list item is ordered (numbered/lettered). Detected from
    /// `\listtext` or `\pntext` content. `false` = unordered (bullet).
    pub ordered: bool,
}

/// A formatting span tracked during RTF parsing.
#[derive(Debug, Clone)]
pub struct RtfFormattingSpan {
    /// Byte offset in the output text where this format starts.
    pub start: usize,
    /// Byte offset in the output text where this format ends.
    pub end: usize,
    /// Whether bold was active.
    pub bold: bool,
    /// Whether italic was active.
    pub italic: bool,
    /// Whether underline was active.
    pub underline: bool,
    /// Whether strikethrough was active.
    pub strikethrough: bool,
    /// Color index into the color table (0 = default/auto).
    pub color_index: u16,
}

/// RTF formatting metadata extracted alongside text.
pub struct RtfFormattingData {
    /// Formatting spans corresponding to text regions.
    pub spans: Vec<RtfFormattingSpan>,
    /// Color table entries (index 0 is auto/default).
    pub color_table: Vec<String>,
    /// Header text content (from \header groups).
    pub header_text: Option<String>,
    /// Footer text content (from \footer groups).
    pub footer_text: Option<String>,
    /// Hyperlink spans: (start_byte, end_byte, url).
    pub hyperlinks: Vec<(usize, usize, String)>,
}

/// Tracks formatting state during the text extraction pass so that
/// formatting spans have byte offsets that exactly match the extracted text.
///
/// This is used inside `extract_text_from_rtf` to produce spans whose
/// byte ranges are guaranteed to align with the output text, eliminating
/// the offset-drift bug that occurred when formatting was tracked in a
/// separate pass.
#[derive(Clone, Default)]
struct FmtState {
    bold: bool,
    italic: bool,
    underline: bool,
    strikethrough: bool,
    color_idx: u16,
}

struct FormattingTracker {
    /// Current formatting state.
    fmt: FmtState,
    /// Stack of formatting states pushed on `{` and popped on `}`.
    fmt_stack: Vec<FmtState>,
    /// Byte offset where the current span started.
    span_start: usize,
    /// Accumulated formatting spans (byte offsets into pre-normalized result).
    spans: Vec<RtfFormattingSpan>,
}

impl FormattingTracker {
    fn new() -> Self {
        Self {
            fmt: FmtState::default(),
            fmt_stack: Vec::new(),
            span_start: 0,
            spans: Vec::new(),
        }
    }

    /// Push current formatting state onto the stack (called on `{`).
    fn push(&mut self) {
        self.fmt_stack.push(self.fmt.clone());
    }

    /// Pop formatting state from the stack (called on `}`).
    /// If formatting changed inside the group, close the current span.
    fn pop(&mut self, text_offset: usize) {
        if let Some(parent) = self.fmt_stack.pop() {
            let changed = self.fmt.bold != parent.bold
                || self.fmt.italic != parent.italic
                || self.fmt.underline != parent.underline
                || self.fmt.strikethrough != parent.strikethrough
                || self.fmt.color_idx != parent.color_idx;
            if changed {
                if text_offset > self.span_start {
                    self.spans.push(RtfFormattingSpan {
                        start: self.span_start,
                        end: text_offset,
                        bold: self.fmt.bold,
                        italic: self.fmt.italic,
                        underline: self.fmt.underline,
                        strikethrough: self.fmt.strikethrough,
                        color_index: self.fmt.color_idx,
                    });
                }
                self.span_start = text_offset;
                self.fmt = parent;
            }
        }
    }

    /// Update a formatting field, closing the current span if the value changed.
    fn update_bold(&mut self, text_offset: usize, val: bool) {
        if val != self.fmt.bold {
            self.close_span(text_offset);
            self.fmt.bold = val;
        }
    }

    fn update_italic(&mut self, text_offset: usize, val: bool) {
        if val != self.fmt.italic {
            self.close_span(text_offset);
            self.fmt.italic = val;
        }
    }

    fn update_underline(&mut self, text_offset: usize, val: bool) {
        if val != self.fmt.underline {
            self.close_span(text_offset);
            self.fmt.underline = val;
        }
    }

    fn update_strikethrough(&mut self, text_offset: usize, val: bool) {
        if val != self.fmt.strikethrough {
            self.close_span(text_offset);
            self.fmt.strikethrough = val;
        }
    }

    fn update_color(&mut self, text_offset: usize, val: u16) {
        if val != self.fmt.color_idx {
            self.close_span(text_offset);
            self.fmt.color_idx = val;
        }
    }

    /// Reset all formatting to default, closing the current span if needed.
    fn reset_all(&mut self, text_offset: usize) {
        if self.fmt.bold || self.fmt.italic || self.fmt.underline || self.fmt.strikethrough || self.fmt.color_idx != 0 {
            self.close_span(text_offset);
            self.fmt = FmtState::default();
        }
    }

    fn close_span(&mut self, text_offset: usize) {
        if text_offset > self.span_start {
            self.spans.push(RtfFormattingSpan {
                start: self.span_start,
                end: text_offset,
                bold: self.fmt.bold,
                italic: self.fmt.italic,
                underline: self.fmt.underline,
                strikethrough: self.fmt.strikethrough,
                color_index: self.fmt.color_idx,
            });
        }
        self.span_start = text_offset;
    }

    /// Close the final span at the end of parsing.
    fn finalize(&mut self, text_offset: usize) {
        if text_offset > self.span_start
            && (self.fmt.bold
                || self.fmt.italic
                || self.fmt.underline
                || self.fmt.strikethrough
                || self.fmt.color_idx != 0)
        {
            self.spans.push(RtfFormattingSpan {
                start: self.span_start,
                end: text_offset,
                bold: self.fmt.bold,
                italic: self.fmt.italic,
                underline: self.fmt.underline,
                strikethrough: self.fmt.strikethrough,
                color_index: self.fmt.color_idx,
            });
        }
    }

    /// Remap all span byte offsets using a normalization mapping.
    fn remap_spans(&mut self, mapping: &[(usize, usize)]) {
        for span in &mut self.spans {
            span.start = map_offset(mapping, span.start);
            span.end = map_offset(mapping, span.end);
        }
        self.spans.retain(|s| s.start < s.end);
    }
}

/// Extract the color table from RTF content.
///
/// Looks for `{\colortbl ...}` and parses semicolon-delimited color entries.
/// Each entry is formatted as `\red{R}\green{G}\blue{B};`.
fn parse_rtf_color_table(content: &str) -> Vec<String> {
    let mut colors = Vec::new();
    let Some(start) = content.find("{\\colortbl") else {
        return colors;
    };
    let rest = &content[start..];
    let mut depth = 0;
    let mut table_content = String::new();
    for ch in rest.chars() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            _ => {}
        }
        if depth > 0 {
            table_content.push(ch);
        }
    }
    let table_body = table_content.strip_prefix("{\\colortbl").unwrap_or(&table_content);

    for entry in table_body.split(';') {
        let entry = entry.trim();
        if entry.is_empty() {
            colors.push(String::new());
            continue;
        }
        let mut r = 0u8;
        let mut g = 0u8;
        let mut b = 0u8;
        for part in entry.split('\\') {
            let part = part.trim();
            if let Some(val) = part.strip_prefix("red") {
                r = val.parse().unwrap_or(0);
            } else if let Some(val) = part.strip_prefix("green") {
                g = val.parse().unwrap_or(0);
            } else if let Some(val) = part.strip_prefix("blue") {
                b = val.parse().unwrap_or(0);
            }
        }
        colors.push(format!("#{r:02x}{g:02x}{b:02x}"));
    }
    colors
}

/// Extract per-font Windows codepages from the RTF font table.
///
/// Looks for `{\fonttbl ...}` (or the ignorable-destination form `{\*\fonttbl ...}`)
/// and parses each `{\fN ... fontname;}` entry, mapping the font id to a codepage
/// derived from `\fcharsetN` (preferred) or a literal `\cpgN` fallback.
///
/// Per the RTF 1.9.1 spec, `\cpgN` on a font entry is ignored when `\fcharsetN` is
/// present — even if the fcharset value itself has no fixed codepage (e.g. Default
/// or Symbol) — so callers should fall further back to `\ansicpg` in that case, not
/// to this font's `\cpgN`. Fonts with neither `\fcharset` nor `\cpg` get no entry.
fn parse_font_charset_table(content: &str) -> HashMap<u16, u32> {
    let mut map = HashMap::new();
    let Some(start) = content.find("{\\*\\fonttbl").or_else(|| content.find("{\\fonttbl")) else {
        return map;
    };
    let rest = &content[start..];
    let mut depth = 0;
    let mut table_content = String::new();
    for ch in rest.chars() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            _ => {}
        }
        if depth > 0 {
            table_content.push(ch);
        }
    }

    let mut chars = table_content.chars().peekable();
    let mut entry_depth: i32 = 0;
    let mut current_font_id: Option<u16> = None;
    let mut current_fcharset: Option<u8> = None;
    let mut current_cpg: Option<u32> = None;

    while let Some(ch) = chars.next() {
        match ch {
            '{' => {
                entry_depth += 1;
                if entry_depth == 2 {
                    current_font_id = None;
                    current_fcharset = None;
                    current_cpg = None;
                }
            }
            '}' => {
                entry_depth -= 1;
                if entry_depth == 1
                    && let Some(id) = current_font_id
                {
                    let codepage = if current_fcharset.is_some() {
                        current_fcharset.and_then(fcharset_to_codepage)
                    } else {
                        current_cpg
                    };
                    if let Some(cp) = codepage {
                        map.insert(id, cp);
                    }
                }
            }
            '\\' => {
                if entry_depth < 2 {
                    continue;
                }
                let (word, param) = parse_rtf_control_word(&mut chars);
                match word.as_str() {
                    "f" => {
                        if let Some(val) = param {
                            current_font_id = Some(val.max(0) as u16);
                        }
                    }
                    "fcharset" => {
                        if let Some(val) = param {
                            current_fcharset = Some(val.max(0) as u8);
                        }
                    }
                    "cpg" => {
                        if let Some(val) = param
                            && val > 0
                        {
                            current_cpg = Some(val as u32);
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    map
}

/// Resolve the Windows codepage used to decode a `\'hh` hex escape run.
///
/// Priority: the active font's charset (via [`parse_font_charset_table`]) —
/// preferring an explicit `\fN` in the current scope, falling back to the
/// document default font set by `\deffN` — then the active `\ansicpgNNNN`,
/// then RTF's default of 1252.
///
/// `\deffN` is document-global rather than scoped: it is typically declared
/// once, before any nested group has a chance to inherit it, so it is tracked
/// separately from `font_id_stack` rather than written into the stack itself.
#[inline]
fn resolve_decode_codepage(
    font_id_stack: &[Option<u16>],
    default_font_id: Option<u16>,
    font_charsets: &HashMap<u16, u32>,
    ansi_codepage_stack: &[u32],
) -> u32 {
    font_id_stack
        .last()
        .copied()
        .flatten()
        .or(default_font_id)
        .and_then(|id| font_charsets.get(&id).copied())
        .or_else(|| ansi_codepage_stack.last().copied())
        .unwrap_or(1252)
}

/// Extract formatting metadata from RTF content.
///
/// This performs a lightweight pass over the RTF to extract:
/// - Bold/italic/underline formatting state changes
/// - Color table and color references
/// - Header/footer text
/// - Hyperlink field instructions
pub(crate) fn extract_rtf_formatting(content: &str) -> RtfFormattingData {
    let color_table = parse_rtf_color_table(content);
    let font_charsets = parse_font_charset_table(content);
    let mut spans = Vec::new();
    let mut hyperlinks = Vec::new();
    let mut text_offset: usize = 0;
    let mut span_start: usize = 0;

    let mut in_header = false;
    let mut in_footer = false;
    let mut header_depth: i32 = 0;
    let mut footer_depth: i32 = 0;
    let mut header_buf = String::new();
    let mut footer_buf = String::new();

    let mut in_fldinst = false;
    let mut fldinst_depth: i32 = 0;
    let mut fldinst_content = String::new();
    let mut in_fldrslt = false;
    let mut fldrslt_depth: i32 = 0;
    let mut fldrslt_start: usize = 0;
    let mut pending_hyperlink_url: Option<String> = None;

    #[derive(Clone)]
    struct FmtState {
        bold: bool,
        italic: bool,
        underline: bool,
        strikethrough: bool,
        color_idx: u16,
    }
    let mut fmt = FmtState {
        bold: false,
        italic: false,
        underline: false,
        strikethrough: false,
        color_idx: 0,
    };
    let mut fmt_stack: Vec<FmtState> = Vec::new();

    let mut group_depth: i32 = 0;
    let mut skip_depth: i32 = 0;
    let mut chars = content.chars().peekable();
    let mut expect_destination = false;
    let mut ignorable_pending = false;

    // Mirrors the extraction pass's codepage tracking so both passes count the
    // same number of output bytes for `\'hh` escape runs.
    let mut ansi_codepage_stack: Vec<u32> = vec![1252];
    // Mirrors the extraction pass's active-font tracking (see `font_id_stack`
    // in `extract_text_from_rtf`) so `\'hh` escapes decode identically in both.
    let mut font_id_stack: Vec<Option<u16>> = vec![None];
    let mut default_font_id: Option<u16> = None;

    let skip_dests = [
        "fonttbl",
        "stylesheet",
        "info",
        "listtable",
        "listoverridetable",
        "generator",
        "filetbl",
        "revtbl",
        "rsidtbl",
        "xmlnstbl",
        "mmathPr",
        "themedata",
        "colorschememapping",
        "datastore",
        "latentstyles",
        "datafield",
        "objdata",
        "objclass",
        "panose",
        "bkmkstart",
        "bkmkend",
        "wgrffmtfilter",
        "fcharset",
        "pgdsctbl",
        "colortbl",
        "pict",
    ];

    let mut group_has_text: Vec<bool> = Vec::new();
    let mut pending_boundary_space = false;

    while let Some(ch) = chars.next() {
        match ch {
            '{' => {
                group_depth += 1;
                expect_destination = true;
                fmt_stack.push(fmt.clone());
                group_has_text.push(false);
                pending_boundary_space = false;
                let current_codepage = ansi_codepage_stack.last().copied().unwrap_or(1252);
                ansi_codepage_stack.push(current_codepage);
                let current_font = font_id_stack.last().copied().flatten();
                font_id_stack.push(current_font);
            }
            '}' => {
                group_depth -= 1;
                expect_destination = false;
                ignorable_pending = false;
                if ansi_codepage_stack.len() > 1 {
                    ansi_codepage_stack.pop();
                }
                if font_id_stack.len() > 1 {
                    font_id_stack.pop();
                }
                if let Some(parent) = fmt_stack.pop() {
                    let changed = fmt.bold != parent.bold
                        || fmt.italic != parent.italic
                        || fmt.underline != parent.underline
                        || fmt.strikethrough != parent.strikethrough
                        || fmt.color_idx != parent.color_idx;
                    if changed {
                        if text_offset > span_start {
                            spans.push(RtfFormattingSpan {
                                start: span_start,
                                end: text_offset,
                                bold: fmt.bold,
                                italic: fmt.italic,
                                underline: fmt.underline,
                                strikethrough: fmt.strikethrough,
                                color_index: fmt.color_idx,
                            });
                        }
                        span_start = text_offset;
                        fmt = parent;
                    }
                }
                if skip_depth > 0 && group_depth < skip_depth {
                    skip_depth = 0;
                }
                if in_header && group_depth < header_depth {
                    in_header = false;
                }
                if in_footer && group_depth < footer_depth {
                    in_footer = false;
                }
                if in_fldinst && group_depth < fldinst_depth {
                    in_fldinst = false;
                    let trimmed = fldinst_content.trim();
                    if let Some(rest) = trimmed.strip_prefix("HYPERLINK") {
                        let url = rest.trim().trim_matches('"').trim().to_string();
                        let url = if let Some(bookmark) = url.strip_prefix("\\l ") {
                            format!("#{}", bookmark.trim().trim_matches('"'))
                        } else if let Some(bookmark) = url.strip_prefix("\\l\"") {
                            format!("#{}", bookmark.trim_matches('"'))
                        } else {
                            url
                        };
                        if !url.is_empty() {
                            pending_hyperlink_url = Some(url);
                        }
                    }
                    fldinst_content.clear();
                }
                if in_fldrslt && group_depth < fldrslt_depth {
                    in_fldrslt = false;
                    if let Some(url) = pending_hyperlink_url.take() {
                        hyperlinks.push((fldrslt_start, text_offset, url));
                    }
                }
                let produced_text = group_has_text.pop().unwrap_or(false);
                if produced_text && skip_depth == 0 {
                    pending_boundary_space = true;
                }
            }
            '\\' => {
                if let Some(&next_ch) = chars.peek() {
                    match next_ch {
                        '\\' | '{' | '}' => {
                            chars.next();
                            expect_destination = false;
                            if in_fldinst {
                                fldinst_content.push(next_ch);
                            }
                            if skip_depth > 0 {
                                continue;
                            }
                            if pending_boundary_space && text_offset > 0 {
                                text_offset += 1;
                            }
                            pending_boundary_space = false;
                            text_offset += next_ch.len_utf8();
                            if let Some(flag) = group_has_text.last_mut() {
                                *flag = true;
                            }
                            if in_header {
                                header_buf.push(next_ch);
                            }
                            if in_footer {
                                footer_buf.push(next_ch);
                            }
                        }
                        '\'' => {
                            chars.next();
                            expect_destination = false;
                            let hex1 = chars.next();
                            let hex2 = chars.next();
                            let bytes = if let (Some(h1), Some(h2)) = (hex1, hex2)
                                && let Some(byte) = parse_hex_byte(h1 as u8, h2 as u8)
                            {
                                let mut bytes = vec![byte];
                                while let Some(next_byte) = consume_adjacent_hex_escape(&mut chars) {
                                    bytes.push(next_byte);
                                }
                                Some(bytes)
                            } else {
                                None
                            };
                            if skip_depth > 0 {
                                continue;
                            }
                            if let Some(bytes) = bytes.as_deref() {
                                let codepage = resolve_decode_codepage(
                                    &font_id_stack,
                                    default_font_id,
                                    &font_charsets,
                                    &ansi_codepage_stack,
                                );
                                let decoded = decode_ansi_bytes(bytes, codepage);
                                if pending_boundary_space && text_offset > 0 {
                                    text_offset += 1;
                                }
                                pending_boundary_space = false;
                                text_offset += decoded.len();
                                if let Some(flag) = group_has_text.last_mut() {
                                    *flag = true;
                                }
                            }
                        }
                        '*' => {
                            chars.next();
                            ignorable_pending = true;
                        }
                        _ => {
                            let (word, param) = parse_rtf_control_word(&mut chars);

                            if expect_destination || ignorable_pending {
                                expect_destination = false;

                                if ignorable_pending {
                                    ignorable_pending = false;
                                    if word == "fldinst" {
                                        in_fldinst = true;
                                        fldinst_depth = group_depth;
                                        if skip_depth == 0 {
                                            skip_depth = group_depth;
                                        }
                                        continue;
                                    }
                                    if skip_depth == 0 {
                                        skip_depth = group_depth;
                                    }
                                    continue;
                                }

                                match word.as_str() {
                                    "fldinst" => {
                                        in_fldinst = true;
                                        fldinst_depth = group_depth;
                                        if skip_depth == 0 {
                                            skip_depth = group_depth;
                                        }
                                        continue;
                                    }
                                    "fldrslt" => {
                                        in_fldrslt = true;
                                        fldrslt_depth = group_depth;
                                        fldrslt_start = text_offset;
                                        continue;
                                    }
                                    _ => {}
                                }

                                if skip_dests.contains(&word.as_str()) {
                                    if skip_depth == 0 {
                                        skip_depth = group_depth;
                                    }
                                    continue;
                                }
                            }

                            if in_fldinst {
                                fldinst_content.push_str(&word);
                            }
                            if word == "ansicpg"
                                && let Some(val) = param
                                && val > 0
                                && let Some(codepage) = ansi_codepage_stack.last_mut()
                            {
                                *codepage = val as u32;
                            }
                            if word == "f"
                                && let Some(val) = param
                                && let Some(font_id) = font_id_stack.last_mut()
                            {
                                *font_id = Some(val.max(0) as u16);
                            }
                            if word == "deff"
                                && let Some(val) = param
                            {
                                default_font_id = Some(val.max(0) as u16);
                            }
                            if skip_depth > 0 {
                                continue;
                            }

                            macro_rules! update_fmt_field {
                                ($field:ident, $new_val:expr) => {
                                    let new_val = $new_val;
                                    if new_val != fmt.$field {
                                        if text_offset > span_start {
                                            spans.push(RtfFormattingSpan {
                                                start: span_start,
                                                end: text_offset,
                                                bold: fmt.bold,
                                                italic: fmt.italic,
                                                underline: fmt.underline,
                                                strikethrough: fmt.strikethrough,
                                                color_index: fmt.color_idx,
                                            });
                                        }
                                        span_start = text_offset;
                                        fmt.$field = new_val;
                                    }
                                };
                            }

                            match word.as_str() {
                                "b" => {
                                    update_fmt_field!(bold, param.unwrap_or(1) != 0);
                                }
                                "i" => {
                                    update_fmt_field!(italic, param.unwrap_or(1) != 0);
                                }
                                "ul" => {
                                    update_fmt_field!(underline, param.unwrap_or(1) != 0);
                                }
                                "ulnone" => {
                                    update_fmt_field!(underline, false);
                                }
                                "strike" => {
                                    update_fmt_field!(strikethrough, param.unwrap_or(1) != 0);
                                }
                                "cf" => {
                                    update_fmt_field!(color_idx, param.unwrap_or(0) as u16);
                                }
                                "plain"
                                    if (fmt.bold
                                        || fmt.italic
                                        || fmt.underline
                                        || fmt.strikethrough
                                        || fmt.color_idx != 0) =>
                                {
                                    if text_offset > span_start {
                                        spans.push(RtfFormattingSpan {
                                            start: span_start,
                                            end: text_offset,
                                            bold: fmt.bold,
                                            italic: fmt.italic,
                                            underline: fmt.underline,
                                            strikethrough: fmt.strikethrough,
                                            color_index: fmt.color_idx,
                                        });
                                    }
                                    span_start = text_offset;
                                    fmt.bold = false;
                                    fmt.italic = false;
                                    fmt.underline = false;
                                    fmt.strikethrough = false;
                                    fmt.color_idx = 0;
                                }
                                "header" | "headerl" | "headerr" | "headerf" => {
                                    in_header = true;
                                    header_depth = group_depth;
                                }
                                "footer" | "footerl" | "footerr" | "footerf" => {
                                    in_footer = true;
                                    footer_depth = group_depth;
                                }
                                "par" | "line" => {
                                    text_offset += 1;
                                    if in_header {
                                        header_buf.push('\n');
                                    }
                                    if in_footer {
                                        footer_buf.push('\n');
                                    }
                                }
                                "tab" => {
                                    text_offset += 1;
                                }
                                "bullet" => {
                                    text_offset += '\u{2022}'.len_utf8();
                                }
                                "lquote" => {
                                    text_offset += '\u{2018}'.len_utf8();
                                }
                                "rquote" => {
                                    text_offset += '\u{2019}'.len_utf8();
                                }
                                "ldblquote" => {
                                    text_offset += '\u{201C}'.len_utf8();
                                }
                                "rdblquote" => {
                                    text_offset += '\u{201D}'.len_utf8();
                                }
                                "endash" => {
                                    text_offset += '\u{2013}'.len_utf8();
                                }
                                "emdash" => {
                                    text_offset += '\u{2014}'.len_utf8();
                                }
                                "u" => {
                                    if let Some(code_num) = param {
                                        let code_u = if code_num < 0 {
                                            (code_num + 65536) as u32
                                        } else {
                                            code_num as u32
                                        };
                                        if let Some(c) = char::from_u32(code_u) {
                                            text_offset += c.len_utf8();
                                            if in_header {
                                                header_buf.push(c);
                                            }
                                            if in_footer {
                                                footer_buf.push(c);
                                            }
                                        }
                                    }
                                    if let Some(&next) = chars.peek()
                                        && next != '\\'
                                        && next != '{'
                                        && next != '}'
                                    {
                                        chars.next();
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                }
            }
            '\n' | '\r' => {}
            ' ' | '\t' => {
                if in_fldinst {
                    fldinst_content.push(' ');
                }
                if skip_depth > 0 {
                    continue;
                }
                if text_offset > 0 {
                    text_offset += 1;
                    if let Some(flag) = group_has_text.last_mut() {
                        *flag = true;
                    }
                }
            }
            _ => {
                if in_fldinst {
                    fldinst_content.push(ch);
                    continue;
                }
                if skip_depth > 0 {
                    continue;
                }
                if pending_boundary_space && text_offset > 0 {
                    text_offset += 1;
                }
                pending_boundary_space = false;
                text_offset += ch.len_utf8();
                if let Some(flag) = group_has_text.last_mut() {
                    *flag = true;
                }
                if in_header {
                    header_buf.push(ch);
                }
                if in_footer {
                    footer_buf.push(ch);
                }
            }
        }
    }

    if text_offset > span_start && (fmt.bold || fmt.italic || fmt.underline || fmt.strikethrough || fmt.color_idx != 0)
    {
        spans.push(RtfFormattingSpan {
            start: span_start,
            end: text_offset,
            bold: fmt.bold,
            italic: fmt.italic,
            underline: fmt.underline,
            strikethrough: fmt.strikethrough,
            color_index: fmt.color_idx,
        });
    }

    let header_trimmed = header_buf.trim().to_string();
    let footer_trimmed = footer_buf.trim().to_string();

    RtfFormattingData {
        spans,
        color_table,
        header_text: if header_trimmed.is_empty() {
            None
        } else {
            Some(header_trimmed)
        },
        footer_text: if footer_trimmed.is_empty() {
            None
        } else {
            Some(footer_trimmed)
        },
        hyperlinks,
    }
}

/// Convert RTF formatting spans into `TextAnnotation` vectors for a paragraph.
///
/// Given the byte range of a paragraph within the full extracted text,
/// produces annotations from the formatting spans that overlap.
pub(crate) fn spans_to_annotations(
    para_start: usize,
    para_end: usize,
    formatting: &RtfFormattingData,
) -> Vec<TextAnnotation> {
    let mut annotations = Vec::new();
    for span in &formatting.spans {
        if span.end <= para_start || span.start >= para_end {
            continue;
        }
        let ann_start = span.start.max(para_start) - para_start;
        let ann_end = span.end.min(para_end) - para_start;
        if ann_start >= ann_end {
            continue;
        }
        let s = ann_start as u32;
        let e = ann_end as u32;
        if span.bold {
            annotations.push(TextAnnotation {
                start: s,
                end: e,
                kind: AnnotationKind::Bold,
            });
        }
        if span.italic {
            annotations.push(TextAnnotation {
                start: s,
                end: e,
                kind: AnnotationKind::Italic,
            });
        }
        if span.underline {
            annotations.push(TextAnnotation {
                start: s,
                end: e,
                kind: AnnotationKind::Underline,
            });
        }
        if span.strikethrough {
            annotations.push(TextAnnotation {
                start: s,
                end: e,
                kind: AnnotationKind::Strikethrough,
            });
        }
        if span.color_index > 0
            && let Some(color) = formatting.color_table.get(span.color_index as usize)
            && !color.is_empty()
            && color != "#000000"
        {
            annotations.push(TextAnnotation {
                start: s,
                end: e,
                kind: AnnotationKind::Color { value: color.clone() },
            });
        }
    }

    for (link_start, link_end, url) in &formatting.hyperlinks {
        if *link_end <= para_start || *link_start >= para_end {
            continue;
        }
        let s = (link_start.max(&para_start) - para_start) as u32;
        let e = (link_end.min(&para_end) - para_start) as u32;
        if s < e {
            annotations.push(TextAnnotation {
                start: s,
                end: e,
                kind: AnnotationKind::Link {
                    url: url.clone(),
                    title: None,
                },
            });
        }
    }

    annotations
}

/// Known RTF destination groups whose content should be skipped entirely.
///
/// These are groups that start with a control word and contain metadata,
/// font tables, style sheets, or binary data — not document body text.
///
/// Note: `field` and `fldinst` are NOT in this list — they are handled
/// specially so that hyperlink text (`\fldrslt`) is extracted.
const SKIP_DESTINATIONS: &[&str] = &[
    "fonttbl",
    "colortbl",
    "stylesheet",
    "info",
    "listtable",
    "listoverridetable",
    "generator",
    "filetbl",
    "revtbl",
    "rsidtbl",
    "xmlnstbl",
    "mmathPr",
    "themedata",
    "colorschememapping",
    "datastore",
    "latentstyles",
    "datafield",
    "objdata",
    "objclass",
    "panose",
    "bkmkstart",
    "bkmkend",
    "wgrffmtfilter",
    "fcharset",
    "pgdsctbl",
];

/// Extract text and image metadata from RTF document.
///
/// This function extracts plain text from an RTF document by:
/// 1. Tracking group nesting depth with a state stack
/// 2. Skipping known destination groups (fonttbl, stylesheet, info, etc.)
/// 3. Skipping `{\*\...}` ignorable destination groups
/// 4. Converting encoded characters to Unicode
/// 5. Extracting text while skipping formatting groups
/// 6. Detecting and extracting image metadata (\pict sections)
/// 7. Normalizing whitespace
pub(crate) fn extract_text_from_rtf(
    content: &str,
    plain: bool,
) -> (String, Vec<Table>, Vec<RtfImage>, Vec<ParagraphMeta>, RtfFormattingData) {
    let color_table = parse_rtf_color_table(content);
    let font_charsets = parse_font_charset_table(content);
    let mut fmt_tracker = FormattingTracker::new();

    let mut result = String::new();
    let mut chars = content.chars().peekable();
    let mut tables: Vec<Table> = Vec::new();
    let mut images: Vec<RtfImage> = Vec::new();
    let mut table_state: Option<TableState> = None;

    let mut para_metas: Vec<ParagraphMeta> = Vec::new();
    let mut cur_heading_level: u8 = 0;
    let mut cur_list_level: Option<u8> = None;
    let mut cur_list_id: Option<u16> = None;
    let mut in_listtext = false;
    let mut listtext_depth: i32 = 0;
    let mut listtext_buf = String::new();
    let mut cur_ordered = false;
    let mut para_meta_emitted = false;

    let mut uc_stack: Vec<u8> = vec![1];

    let mut in_fldinst = false;
    let mut fldinst_depth: i32 = 0;
    let mut fldinst_content = String::new();
    let mut in_fldrslt = false;
    let mut fldrslt_depth: i32 = 0;
    let mut fldrslt_start: usize = 0;
    let mut pending_hyperlink_url: Option<String> = None;
    let mut hyperlinks: Vec<(usize, usize, String)> = Vec::new();

    let mut in_footnote = false;
    let mut footnote_depth: i32 = 0;
    let mut footnote_buf = String::new();
    let mut footnote_count: usize = 0;
    let mut footnotes: Vec<String> = Vec::new();

    // `\shptxt` (drawing-object / text-box text) and `\annotation` (comment
    // text) are ordinary content destinations, but real producers nest them
    // inside an *ignorable* ancestor (`{\*\shp{\*\shpinst{...{\shptxt ...}`
    // for text boxes) that this parser otherwise skips wholesale. Buffering
    // their content unconditionally -- the same trick `footnote_buf` uses --
    // lets them survive even while nested under an active `skip_depth` (#86).
    let mut in_shptxt = false;
    let mut shptxt_depth: i32 = 0;
    let mut shptxt_buf = String::new();
    let mut text_boxes: Vec<String> = Vec::new();

    let mut in_annotation = false;
    let mut annotation_depth: i32 = 0;
    let mut annotation_buf = String::new();
    let mut comments: Vec<String> = Vec::new();
    // Set by `\atnid` (the comment's numeric id, always written as an
    // ignorable `{\*\atnid N}` sibling of `\annotation`) and consumed when
    // the enclosing `\annotation` group closes.
    let mut pending_atnid: Option<i32> = None;

    let mut group_depth: i32 = 0;
    let mut skip_depth: i32 = 0;

    let mut ignorable_pending = false;
    let mut expect_destination = false;

    let mut group_has_text: Vec<bool> = Vec::new();

    let mut pending_boundary_space = false;

    let mut hidden_stack: Vec<bool> = vec![false];

    // ANSI codepage for \'hh escapes. RTF defaults to Windows-1252 unless
    // overridden by \ansicpgNNNN. Scoped like other document properties.
    let mut ansi_codepage_stack: Vec<u32> = vec![1252];

    // Active font id for \'hh escapes, set by \fN / \deffN. Used to look up a
    // per-font codepage in `font_charsets` (from \fcharsetN), which takes
    // priority over `ansi_codepage_stack`. Scoped like other document properties.
    let mut font_id_stack: Vec<Option<u16>> = vec![None];
    // Document default font set by \deffN. Unlike font_id_stack, this is not
    // scoped: \deff is typically declared once, before any nested group could
    // have inherited it, so it's tracked separately and consulted only when no
    // scope has set an explicit \fN. See `resolve_decode_codepage`.
    let mut default_font_id: Option<u16> = None;

    let ensure_table = |table_state: &mut Option<TableState>| {
        if table_state.is_none() {
            *table_state = Some(TableState::new());
        }
    };

    let finalize_table = move |state_opt: &mut Option<TableState>, tables: &mut Vec<Table>| {
        if let Some(state) = state_opt.take()
            && let Some(table) = state.finalize_with_format(plain)
        {
            tables.push(table);
        }
    };

    while let Some(ch) = chars.next() {
        match ch {
            '{' => {
                group_depth += 1;
                expect_destination = true;
                group_has_text.push(false);
                let current_uc = uc_stack.last().copied().unwrap_or(1);
                uc_stack.push(current_uc);
                let current_hidden = hidden_stack.last().copied().unwrap_or(false);
                hidden_stack.push(current_hidden);
                let current_codepage = ansi_codepage_stack.last().copied().unwrap_or(1252);
                ansi_codepage_stack.push(current_codepage);
                let current_font = font_id_stack.last().copied().flatten();
                font_id_stack.push(current_font);
                fmt_tracker.push();
                pending_boundary_space = false;
            }
            '}' => {
                group_depth -= 1;
                expect_destination = false;
                ignorable_pending = false;
                fmt_tracker.pop(result.len());
                if uc_stack.len() > 1 {
                    uc_stack.pop();
                }
                if hidden_stack.len() > 1 {
                    hidden_stack.pop();
                }
                if ansi_codepage_stack.len() > 1 {
                    ansi_codepage_stack.pop();
                }
                if font_id_stack.len() > 1 {
                    font_id_stack.pop();
                }
                if skip_depth > 0 && group_depth < skip_depth {
                    skip_depth = 0;
                }
                if in_listtext && group_depth < listtext_depth {
                    in_listtext = false;
                    let lt = listtext_buf.trim();
                    let is_ordered = lt
                        .strip_suffix('.')
                        .or_else(|| lt.strip_suffix(')'))
                        .is_some_and(|prefix| {
                            let p = prefix.trim();
                            if p.chars().all(|c| c.is_ascii_digit()) && !p.is_empty() {
                                return true;
                            }
                            if p.chars().all(|c| c.is_ascii_alphabetic()) && !p.is_empty() {
                                return true;
                            }
                            false
                        });
                    if is_ordered {
                        cur_ordered = true;
                    }
                    listtext_buf.clear();
                }
                if in_fldinst && group_depth < fldinst_depth {
                    in_fldinst = false;
                    let trimmed = fldinst_content.trim();
                    if let Some(rest) = trimmed.strip_prefix("HYPERLINK") {
                        let url = rest.trim().trim_matches('"').trim().to_string();
                        let url = if let Some(bookmark) = url.strip_prefix("\\l ") {
                            format!("#{}", bookmark.trim().trim_matches('"'))
                        } else if let Some(bookmark) = url.strip_prefix("\\l\"") {
                            format!("#{}", bookmark.trim_matches('"'))
                        } else {
                            url
                        };
                        if !url.is_empty() {
                            pending_hyperlink_url = Some(url);
                        }
                    }
                    fldinst_content.clear();
                }
                if in_fldrslt && group_depth < fldrslt_depth {
                    in_fldrslt = false;
                    if let Some(url) = pending_hyperlink_url.take() {
                        hyperlinks.push((fldrslt_start, result.len(), url));
                    }
                }
                if in_footnote && group_depth < footnote_depth {
                    in_footnote = false;
                    let note = footnote_buf.trim().to_string();
                    if !note.is_empty() {
                        footnotes.push(note);
                    }
                    footnote_buf.clear();
                }
                if in_shptxt && group_depth < shptxt_depth {
                    in_shptxt = false;
                    let text_box = shptxt_buf.trim().to_string();
                    if !text_box.is_empty() {
                        text_boxes.push(text_box);
                    }
                    shptxt_buf.clear();
                }
                if in_annotation && group_depth < annotation_depth {
                    in_annotation = false;
                    let comment = annotation_buf.trim().to_string();
                    if !comment.is_empty() {
                        let label = pending_atnid
                            .take()
                            .map(|id| id.to_string())
                            .unwrap_or_else(|| (comments.len() + 1).to_string());
                        comments.push(format!("[Comment {label}]: {comment}"));
                    } else {
                        pending_atnid = None;
                    }
                    annotation_buf.clear();
                }
                let produced_text = group_has_text.pop().unwrap_or(false);
                if produced_text && skip_depth == 0 {
                    pending_boundary_space = true;
                }
            }
            '\\' => {
                if let Some(&next_ch) = chars.peek() {
                    match next_ch {
                        '\n' | '\r' => {
                            chars.next();
                            if next_ch == '\r'
                                && let Some(&'\n') = chars.peek()
                            {
                                chars.next();
                            }
                            expect_destination = false;
                            if skip_depth > 0 {
                                continue;
                            }
                            handle_control_word(
                                "par",
                                None,
                                &mut chars,
                                &mut result,
                                &mut table_state,
                                &mut tables,
                                &mut images,
                                &ensure_table,
                                &finalize_table,
                                plain,
                                &mut group_has_text,
                                &mut cur_heading_level,
                                &mut cur_list_level,
                                &mut cur_list_id,
                                &mut cur_ordered,
                                &mut para_metas,
                                &mut para_meta_emitted,
                                &mut uc_stack,
                                &mut ansi_codepage_stack,
                                &mut footnote_count,
                                in_footnote,
                                &mut footnote_buf,
                                &mut pending_boundary_space,
                                &mut hidden_stack,
                                &mut fmt_tracker,
                                &mut font_id_stack,
                                &mut default_font_id,
                            );
                        }
                        '\\' | '{' | '}' => {
                            chars.next();
                            expect_destination = false;
                            if in_fldinst {
                                fldinst_content.push(next_ch);
                            }
                            if in_footnote {
                                footnote_buf.push(next_ch);
                            }
                            if in_shptxt {
                                shptxt_buf.push(next_ch);
                            }
                            if in_annotation {
                                annotation_buf.push(next_ch);
                            }
                            if skip_depth > 0 {
                                continue;
                            }
                            if hidden_stack.last().copied().unwrap_or(false) {
                                continue;
                            }
                            if pending_boundary_space
                                && !result.is_empty()
                                && !result.ends_with(' ')
                                && !result.ends_with('\n')
                            {
                                result.push(' ');
                            }
                            pending_boundary_space = false;
                            para_meta_emitted = false;
                            result.push(next_ch);
                            if let Some(flag) = group_has_text.last_mut() {
                                *flag = true;
                            }
                        }
                        '\'' => {
                            chars.next();
                            expect_destination = false;
                            let hex1 = chars.next();
                            let hex2 = chars.next();
                            let bytes = if let (Some(h1), Some(h2)) = (hex1, hex2)
                                && let Some(byte) = parse_hex_byte(h1 as u8, h2 as u8)
                            {
                                let mut bytes = vec![byte];
                                while let Some(next_byte) = consume_adjacent_hex_escape(&mut chars) {
                                    bytes.push(next_byte);
                                }
                                Some(bytes)
                            } else {
                                None
                            };

                            if (in_footnote || in_shptxt || in_annotation)
                                && let Some(bytes) = bytes.as_deref()
                            {
                                let codepage = resolve_decode_codepage(
                                    &font_id_stack,
                                    default_font_id,
                                    &font_charsets,
                                    &ansi_codepage_stack,
                                );
                                let decoded = decode_ansi_bytes(bytes, codepage);
                                if in_footnote {
                                    footnote_buf.push_str(&decoded);
                                }
                                if in_shptxt {
                                    shptxt_buf.push_str(&decoded);
                                }
                                if in_annotation {
                                    annotation_buf.push_str(&decoded);
                                }
                            }
                            if skip_depth > 0 {
                                continue;
                            }
                            if hidden_stack.last().copied().unwrap_or(false) {
                                continue;
                            }
                            if let Some(bytes) = bytes.as_deref() {
                                let codepage = resolve_decode_codepage(
                                    &font_id_stack,
                                    default_font_id,
                                    &font_charsets,
                                    &ansi_codepage_stack,
                                );
                                let decoded = decode_ansi_bytes(bytes, codepage);
                                if let Some(state) = table_state.as_mut()
                                    && state.in_row
                                {
                                    state.current_cell.push_str(&decoded);
                                } else {
                                    if pending_boundary_space
                                        && !result.is_empty()
                                        && !result.ends_with(' ')
                                        && !result.ends_with('\n')
                                    {
                                        result.push(' ');
                                    }
                                    pending_boundary_space = false;
                                    para_meta_emitted = false;
                                    result.push_str(&decoded);
                                    if let Some(flag) = group_has_text.last_mut() {
                                        *flag = true;
                                    }
                                }
                            }
                        }
                        '*' => {
                            chars.next();
                            ignorable_pending = true;
                        }
                        _ => {
                            let (control_word, _param) = parse_rtf_control_word(&mut chars);

                            if expect_destination || ignorable_pending {
                                expect_destination = false;

                                if ignorable_pending {
                                    ignorable_pending = false;
                                    if control_word == "fldinst" {
                                        in_fldinst = true;
                                        fldinst_depth = group_depth;
                                        if skip_depth == 0 {
                                            skip_depth = group_depth;
                                        }
                                        continue;
                                    }
                                    if control_word == "listtext" || control_word == "pntext" {
                                        in_listtext = true;
                                        listtext_depth = group_depth;
                                        listtext_buf.clear();
                                        if skip_depth == 0 {
                                            skip_depth = group_depth;
                                        }
                                        continue;
                                    }
                                    // `{\*\atnid N}` carries the enclosing comment's id as a plain
                                    // numeric parameter -- there is no destination content to
                                    // recurse into, just capture it and skip the (empty) group.
                                    if control_word == "atnid" {
                                        if let Some(id) = _param {
                                            pending_atnid = Some(id);
                                        }
                                        if skip_depth == 0 {
                                            skip_depth = group_depth;
                                        }
                                        continue;
                                    }
                                    if control_word != "shppict" {
                                        if skip_depth == 0 {
                                            skip_depth = group_depth;
                                        }
                                        continue;
                                    }
                                }

                                if control_word == "listtext" || control_word == "pntext" {
                                    in_listtext = true;
                                    listtext_depth = group_depth;
                                    listtext_buf.clear();
                                    if skip_depth == 0 {
                                        skip_depth = group_depth;
                                    }
                                    continue;
                                }

                                if control_word == "fldinst" {
                                    in_fldinst = true;
                                    fldinst_depth = group_depth;
                                    if skip_depth == 0 {
                                        skip_depth = group_depth;
                                    }
                                    continue;
                                }

                                if control_word == "fldrslt" {
                                    in_fldrslt = true;
                                    fldrslt_depth = group_depth;
                                    fldrslt_start = result.len();
                                    continue;
                                }

                                if control_word == "footnote" {
                                    in_footnote = true;
                                    footnote_depth = group_depth;
                                    footnote_buf.clear();
                                    if skip_depth == 0 {
                                        skip_depth = group_depth;
                                    }
                                    continue;
                                }

                                // `\shptxt` (drawing-object/text-box text) is a plain destination,
                                // but real producers nest it inside an ignorable `\*\shp{\*\shpinst
                                // ...}` ancestor. Setting `skip_depth` only when it is not already
                                // active (matching `footnote`/`fldinst` above) would still lose this
                                // content -- an outer skip is already active by the time we get here.
                                // Buffering unconditionally via `in_shptxt` (checked ahead of every
                                // `skip_depth` gate below) is what actually rescues the text (#86).
                                if control_word == "shptxt" {
                                    in_shptxt = true;
                                    shptxt_depth = group_depth;
                                    shptxt_buf.clear();
                                    if skip_depth == 0 {
                                        skip_depth = group_depth;
                                    }
                                    continue;
                                }

                                // `\annotation` (Word comment text) is likewise a plain destination
                                // that this parser previously treated as an unrecognized ignorable
                                // destination and skipped whole (#86).
                                if control_word == "annotation" {
                                    in_annotation = true;
                                    annotation_depth = group_depth;
                                    annotation_buf.clear();
                                    if skip_depth == 0 {
                                        skip_depth = group_depth;
                                    }
                                    continue;
                                }

                                // Non-ignorable form fallback; the common form is `{\*\atnid N}`,
                                // handled above under `ignorable_pending`.
                                if control_word == "atnid" {
                                    if let Some(id) = _param {
                                        pending_atnid = Some(id);
                                    }
                                    if skip_depth == 0 {
                                        skip_depth = group_depth;
                                    }
                                    continue;
                                }

                                if SKIP_DESTINATIONS.contains(&control_word.as_str()) {
                                    if skip_depth == 0 {
                                        skip_depth = group_depth;
                                    }
                                    continue;
                                }
                            }

                            if skip_depth > 0 {
                                if control_word == "uc"
                                    && let Some(val) = _param
                                    && let Some(uc) = uc_stack.last_mut()
                                {
                                    *uc = val.max(0) as u8;
                                }
                                if control_word == "ansicpg"
                                    && let Some(val) = _param
                                    && val > 0
                                    && let Some(codepage) = ansi_codepage_stack.last_mut()
                                {
                                    *codepage = val as u32;
                                }
                                if control_word == "f"
                                    && let Some(val) = _param
                                    && let Some(font_id) = font_id_stack.last_mut()
                                {
                                    *font_id = Some(val.max(0) as u16);
                                }
                                if control_word == "deff"
                                    && let Some(val) = _param
                                {
                                    default_font_id = Some(val.max(0) as u16);
                                }
                                if (in_footnote || in_shptxt || in_annotation)
                                    && control_word == "u"
                                    && let Some(code_num) = _param
                                {
                                    let code_u = if code_num < 0 {
                                        (code_num + 65536) as u32
                                    } else {
                                        code_num as u32
                                    };
                                    if let Some(c) = char::from_u32(code_u) {
                                        if in_footnote {
                                            footnote_buf.push(c);
                                        }
                                        if in_shptxt {
                                            shptxt_buf.push(c);
                                        }
                                        if in_annotation {
                                            annotation_buf.push(c);
                                        }
                                    }
                                    let uc_count = uc_stack.last().copied().unwrap_or(1);
                                    for _ in 0..uc_count {
                                        if let Some(&next) = chars.peek()
                                            && next != '\\'
                                            && next != '{'
                                            && next != '}'
                                        {
                                            chars.next();
                                        }
                                    }
                                }
                                if (in_footnote || in_shptxt || in_annotation)
                                    && (control_word == "par" || control_word == "line")
                                {
                                    if in_footnote {
                                        footnote_buf.push(' ');
                                    }
                                    if in_shptxt {
                                        shptxt_buf.push(' ');
                                    }
                                    if in_annotation {
                                        annotation_buf.push(' ');
                                    }
                                }
                                continue;
                            }

                            handle_control_word(
                                &control_word,
                                _param,
                                &mut chars,
                                &mut result,
                                &mut table_state,
                                &mut tables,
                                &mut images,
                                &ensure_table,
                                &finalize_table,
                                plain,
                                &mut group_has_text,
                                &mut cur_heading_level,
                                &mut cur_list_level,
                                &mut cur_list_id,
                                &mut cur_ordered,
                                &mut para_metas,
                                &mut para_meta_emitted,
                                &mut uc_stack,
                                &mut ansi_codepage_stack,
                                &mut footnote_count,
                                in_footnote,
                                &mut footnote_buf,
                                &mut pending_boundary_space,
                                &mut hidden_stack,
                                &mut fmt_tracker,
                                &mut font_id_stack,
                                &mut default_font_id,
                            );
                        }
                    }
                }
            }
            '\n' | '\r' => {}
            ' ' | '\t' => {
                if in_fldinst {
                    fldinst_content.push(' ');
                }
                if in_footnote {
                    footnote_buf.push(' ');
                }
                if in_shptxt {
                    shptxt_buf.push(' ');
                }
                if in_annotation {
                    annotation_buf.push(' ');
                }
                if skip_depth > 0 && !in_footnote && !in_shptxt && !in_annotation {
                    continue;
                }
                if in_footnote || in_shptxt || in_annotation {
                    continue;
                }
                if let Some(state) = table_state.as_mut()
                    && state.in_row
                {
                    if !state.current_cell.ends_with(' ') {
                        state.current_cell.push(' ');
                    }
                } else if !result.is_empty() && !result.ends_with(' ') && !result.ends_with('\n') {
                    result.push(' ');
                    if let Some(flag) = group_has_text.last_mut() {
                        *flag = true;
                    }
                }
            }
            _ => {
                expect_destination = false;
                if in_fldinst {
                    fldinst_content.push(ch);
                }
                if in_footnote {
                    footnote_buf.push(ch);
                }
                if in_shptxt {
                    shptxt_buf.push(ch);
                }
                if in_annotation {
                    annotation_buf.push(ch);
                }
                if in_listtext {
                    listtext_buf.push(ch);
                }
                if skip_depth > 0 {
                    continue;
                }
                if hidden_stack.last().copied().unwrap_or(false) {
                    continue;
                }
                if let Some(state) = table_state.as_ref()
                    && !state.in_row
                    && !state.rows.is_empty()
                {
                    finalize_table(&mut table_state, &mut tables);
                }
                if let Some(state) = table_state.as_mut()
                    && state.in_row
                {
                    state.current_cell.push(ch);
                } else {
                    if pending_boundary_space && !result.is_empty() && !result.ends_with(' ') && !result.ends_with('\n')
                    {
                        result.push(' ');
                    }
                    pending_boundary_space = false;
                    para_meta_emitted = false;
                    result.push(ch);
                    if let Some(flag) = group_has_text.last_mut() {
                        *flag = true;
                    }
                }
            }
        }
    }

    if table_state.is_some() {
        finalize_table(&mut table_state, &mut tables);
    }

    fmt_tracker.finalize(result.len());

    let (normalized, mapping) = normalize_whitespace_with_mapping(&result);
    let final_text = normalized.trim_end();
    if !final_text.is_empty() {
        let para_count = normalized.split("\n\n").filter(|p| !p.trim().is_empty()).count();
        while para_metas.len() < para_count {
            para_metas.push(ParagraphMeta {
                heading_level: cur_heading_level,
                list_level: cur_list_level,
                list_id: cur_list_id,
                is_table: false,
                ordered: cur_ordered,
            });
        }
    }

    let mut final_result = normalized;
    if !footnotes.is_empty() {
        if !final_result.ends_with('\n') {
            final_result.push('\n');
            final_result.push('\n');
        }
        for (i, note) in footnotes.iter().enumerate() {
            final_result.push_str(&format!("[^{}]: {}", i + 1, note.trim()));
            final_result.push('\n');
            final_result.push('\n');
        }
    }

    if !text_boxes.is_empty() {
        if !final_result.ends_with('\n') {
            final_result.push('\n');
            final_result.push('\n');
        }
        for text_box in &text_boxes {
            final_result.push_str(text_box);
            final_result.push('\n');
            final_result.push('\n');
        }
    }

    if !comments.is_empty() {
        if !final_result.ends_with('\n') {
            final_result.push('\n');
            final_result.push('\n');
        }
        for comment in &comments {
            final_result.push_str(comment);
            final_result.push('\n');
            final_result.push('\n');
        }
    }

    fmt_tracker.remap_spans(&mapping);

    for link in &mut hyperlinks {
        link.0 = map_offset(&mapping, link.0);
        link.1 = map_offset(&mapping, link.1);
    }
    hyperlinks.retain(|l| l.0 < l.1);

    let formatting_data = RtfFormattingData {
        spans: fmt_tracker.spans,
        color_table,
        header_text: None,
        footer_text: None,
        hyperlinks,
    };

    (final_result, tables, images, para_metas, formatting_data)
}

/// Consume the next `\'hh` hex escape if it immediately follows the current one.
///
/// Adjacent hex escapes form one multi-byte run that must be decoded together
/// so multi-byte ANSI codepages (e.g. Shift-JIS, GBK) decode correctly. Raw
/// CR/LF between escapes is skipped: RTF readers ignore bare line breaks, and
/// writers wrap lines freely, including between the bytes of one character.
fn consume_adjacent_hex_escape(chars: &mut std::iter::Peekable<std::str::Chars>) -> Option<u8> {
    let mut lookahead = chars.clone();
    let mut skipped = 0usize;
    while matches!(lookahead.peek(), Some('\r' | '\n')) {
        lookahead.next();
        skipped += 1;
    }
    if lookahead.next()? != '\\' || lookahead.next()? != '\'' {
        return None;
    }
    let h1 = lookahead.next()?;
    let h2 = lookahead.next()?;
    let byte = parse_hex_byte(h1 as u8, h2 as u8)?;

    for _ in 0..skipped + 4 {
        chars.next();
    }

    Some(byte)
}

/// Handle an RTF control word during parsing.
#[allow(clippy::too_many_arguments, clippy::ptr_arg)]
fn handle_control_word(
    control_word: &str,
    param: Option<i32>,
    chars: &mut std::iter::Peekable<std::str::Chars>,
    result: &mut String,
    table_state: &mut Option<TableState>,
    tables: &mut Vec<Table>,
    images: &mut Vec<RtfImage>,
    ensure_table: &dyn Fn(&mut Option<TableState>),
    finalize_table: &dyn Fn(&mut Option<TableState>, &mut Vec<Table>),
    plain: bool,
    group_has_text: &mut [bool],
    cur_heading_level: &mut u8,
    cur_list_level: &mut Option<u8>,
    cur_list_id: &mut Option<u16>,
    cur_ordered: &mut bool,
    para_metas: &mut Vec<ParagraphMeta>,
    para_meta_emitted: &mut bool,
    uc_stack: &mut Vec<u8>,
    ansi_codepage_stack: &mut [u32],
    footnote_count: &mut usize,
    _in_footnote: bool,
    _footnote_buf: &mut String,
    pending_boundary_space: &mut bool,
    hidden_stack: &mut Vec<bool>,
    fmt_tracker: &mut FormattingTracker,
    font_id_stack: &mut [Option<u16>],
    default_font_id: &mut Option<u16>,
) {
    match control_word {
        "f" => {
            if let Some(val) = param
                && let Some(font_id) = font_id_stack.last_mut()
            {
                *font_id = Some(val.max(0) as u16);
            }
        }
        "deff" => {
            if let Some(val) = param {
                *default_font_id = Some(val.max(0) as u16);
            }
        }
        "v" => {
            let hidden = param.unwrap_or(1) != 0;
            if let Some(h) = hidden_stack.last_mut() {
                *h = hidden;
            }
        }
        "pard" => {
            let in_table_row = table_state.as_ref().is_some_and(|s| s.in_row);
            if !in_table_row && !result.is_empty() && !result.ends_with('\n') && !*para_meta_emitted {
                para_metas.push(ParagraphMeta {
                    heading_level: *cur_heading_level,
                    list_level: *cur_list_level,
                    list_id: *cur_list_id,
                    is_table: false,
                    ordered: *cur_ordered,
                });
                result.push('\n');
                result.push('\n');
                if let Some(flag) = group_has_text.last_mut() {
                    *flag = true;
                }
            }
            *para_meta_emitted = false;
            *cur_heading_level = 0;
            *cur_list_level = None;
            *cur_list_id = None;
            *cur_ordered = false;
        }
        "outlinelevel" => {
            if let Some(level) = param {
                *cur_heading_level = (level as u8) + 1;
            }
        }
        "ilvl" => {
            *cur_list_level = Some(param.unwrap_or(0) as u8);
        }
        "ls" => {
            *cur_list_id = Some(param.unwrap_or(0) as u16);
        }
        "uc" => {
            if let Some(val) = param
                && let Some(uc) = uc_stack.last_mut()
            {
                *uc = val.max(0) as u8;
            }
        }
        "ansicpg" => {
            if let Some(val) = param
                && val > 0
                && let Some(codepage) = ansi_codepage_stack.last_mut()
            {
                *codepage = val as u32;
            }
        }
        "u" => {
            if let Some(code_num) = param {
                let code_u = if code_num < 0 {
                    (code_num + 65536) as u32
                } else {
                    code_num as u32
                };
                if let Some(c) = char::from_u32(code_u) {
                    if let Some(state) = table_state.as_mut()
                        && state.in_row
                    {
                        state.current_cell.push(c);
                    } else {
                        if *pending_boundary_space
                            && !result.is_empty()
                            && !result.ends_with(' ')
                            && !result.ends_with('\n')
                        {
                            result.push(' ');
                        }
                        *pending_boundary_space = false;
                        result.push(c);
                        if let Some(flag) = group_has_text.last_mut() {
                            *flag = true;
                        }
                    }
                }
                let uc_count = uc_stack.last().copied().unwrap_or(1);
                let mut skipped = 0u8;
                while skipped < uc_count {
                    if let Some(&next) = chars.peek() {
                        if next == '\\' {
                            chars.next();
                            if let Some(&apos) = chars.peek() {
                                if apos == '\'' {
                                    chars.next();
                                    chars.next();
                                    chars.next();
                                    skipped += 1;
                                    continue;
                                }
                                break;
                            }
                            break;
                        } else if next == '{' || next == '}' {
                            break;
                        } else {
                            chars.next();
                            skipped += 1;
                        }
                    } else {
                        break;
                    }
                }
            }
        }
        "chftn" => {
            *footnote_count += 1;
            let marker = format!("[^{}]", *footnote_count);
            if let Some(state) = table_state.as_mut()
                && state.in_row
            {
                state.current_cell.push_str(&marker);
            } else {
                result.push_str(&marker);
                if let Some(flag) = group_has_text.last_mut() {
                    *flag = true;
                }
            }
        }
        "pict" => {
            let (image_metadata, rtf_image) = extract_pict_image(chars);
            if let Some(img) = rtf_image {
                images.push(img);
            }
            if !image_metadata.is_empty() && !plain {
                let img_md = format!("![image]({image_metadata}) ");
                if let Some(state) = table_state.as_mut()
                    && state.in_row
                {
                    state.current_cell.push_str(&img_md);
                } else {
                    if let Some(flag) = group_has_text.last_mut() {
                        *flag = true;
                    }
                    result.push_str(&img_md);
                }
            }
        }
        "par" | "line" => {
            *pending_boundary_space = false;
            let in_table_row = table_state.as_ref().is_some_and(|s| s.in_row);
            if in_table_row {
                if let Some(state) = table_state.as_mut()
                    && !state.current_cell.is_empty()
                    && !state.current_cell.ends_with(' ')
                {
                    state.current_cell.push(' ');
                }
            } else {
                let still_in_table = table_state.as_ref().is_some_and(|s| s.expecting_next_row);
                if table_state.is_some() && !still_in_table {
                    finalize_table(table_state, tables);
                }
                if !result.is_empty() && !result.ends_with('\n') {
                    if !*para_meta_emitted {
                        para_metas.push(ParagraphMeta {
                            heading_level: *cur_heading_level,
                            list_level: *cur_list_level,
                            list_id: *cur_list_id,
                            is_table: false,
                            ordered: *cur_ordered,
                        });
                        *para_meta_emitted = true;
                    }
                    result.push('\n');
                    result.push('\n');
                }
                if let Some(flag) = group_has_text.last_mut() {
                    *flag = true;
                }
            }
        }
        "tab" => {
            if let Some(state) = table_state.as_mut()
                && state.in_row
            {
                state.current_cell.push('\t');
            } else {
                result.push('\t');
                if let Some(flag) = group_has_text.last_mut() {
                    *flag = true;
                }
            }
        }
        "bullet" => {
            result.push('\u{2022}');
            if let Some(flag) = group_has_text.last_mut() {
                *flag = true;
            }
        }
        "lquote" => {
            result.push('\u{2018}');
            if let Some(flag) = group_has_text.last_mut() {
                *flag = true;
            }
        }
        "rquote" => {
            result.push('\u{2019}');
            if let Some(flag) = group_has_text.last_mut() {
                *flag = true;
            }
        }
        "ldblquote" => {
            result.push('\u{201C}');
            if let Some(flag) = group_has_text.last_mut() {
                *flag = true;
            }
        }
        "rdblquote" => {
            result.push('\u{201D}');
            if let Some(flag) = group_has_text.last_mut() {
                *flag = true;
            }
        }
        "endash" => {
            result.push('\u{2013}');
            if let Some(flag) = group_has_text.last_mut() {
                *flag = true;
            }
        }
        "emdash" => {
            result.push('\u{2014}');
            if let Some(flag) = group_has_text.last_mut() {
                *flag = true;
            }
        }
        "trowd" => {
            ensure_table(table_state);
            if let Some(state) = table_state.as_mut() {
                state.start_row();
            }
        }
        "cell" => {
            if let Some(state) = table_state.as_mut()
                && state.in_row
            {
                state.push_cell();
            }
        }
        "row" => {
            ensure_table(table_state);
            if let Some(state) = table_state.as_mut()
                && (state.in_row || !state.current_cell.is_empty())
            {
                state.push_row();
            }
            if !result.is_empty() && !result.ends_with('\n') {
                result.push('\n');
                result.push('\n');
            }
            result.push_str("[TABLE_ROW]");
            result.push('\n');
            result.push('\n');
            if let Some(flag) = group_has_text.last_mut() {
                *flag = true;
            }
            *para_meta_emitted = true;
            para_metas.push(ParagraphMeta {
                is_table: true,
                ..Default::default()
            });
        }
        "intbl" => {
            ensure_table(table_state);
            if let Some(state) = table_state.as_mut()
                && !state.in_row
            {
                state.start_row();
            }
        }
        "b" => {
            fmt_tracker.update_bold(result.len(), param.unwrap_or(1) != 0);
        }
        "i" => {
            fmt_tracker.update_italic(result.len(), param.unwrap_or(1) != 0);
        }
        "ul" => {
            fmt_tracker.update_underline(result.len(), param.unwrap_or(1) != 0);
        }
        "ulnone" => {
            fmt_tracker.update_underline(result.len(), false);
        }
        "strike" => {
            fmt_tracker.update_strikethrough(result.len(), param.unwrap_or(1) != 0);
        }
        "cf" => {
            fmt_tracker.update_color(result.len(), param.unwrap_or(0) as u16);
        }
        "plain" => {
            if let Some(h) = hidden_stack.last_mut() {
                *h = false;
            }
            fmt_tracker.reset_all(result.len());
        }
        _ => {}
    }
}

#[cfg(test)]
mod issue_86_destination_tests {
    use super::extract_text_from_rtf;

    /// #86: `\shptxt` (drawing-object/text-box text) is a plain destination,
    /// but real producers nest it inside an ignorable `{\*\shp{\*\shpinst
    /// ...}}` ancestor. Before the fix, the outer ignorable-and-unrecognized
    /// destination set `skip_depth` for the whole subtree, and the nested
    /// `\shptxt` group -- despite being recognized -- had no way to escape
    /// that already-active skip, so its text was dropped.
    #[test]
    fn test_shptxt_survives_nested_ignorable_ancestor() {
        let rtf = r"{\rtf1\ansi
{\*\shp{\*\shpinst{\sp{\sn shapeType}{\sv202}}{\shptxt Text box content}}}
Body text after the shape.\par
}";
        let (text, _tables, _images, _para_metas, _fmt) = extract_text_from_rtf(rtf, false);

        assert!(
            text.contains("Text box content"),
            "text-box content should be extracted, got: {text:?}"
        );
        assert!(
            text.contains("Body text after the shape."),
            "ordinary body text should be unaffected, got: {text:?}"
        );
    }

    /// #86: `\annotation` (Word comment text) was previously treated as an
    /// unrecognized ignorable destination and skipped whole. `\atnid` carries
    /// the comment's numeric id and should label the extracted comment.
    #[test]
    fn test_annotation_and_atnid_are_extracted_as_labeled_comment() {
        let rtf = r"{\rtf1\ansi
Body text.\par
{\annotation{\*\atnid7}Reviewer comment here}
}";
        let (text, _tables, _images, _para_metas, _fmt) = extract_text_from_rtf(rtf, false);

        assert!(
            text.contains("[Comment 7]: Reviewer comment here"),
            "comment should be extracted and labeled with its atnid, got: {text:?}"
        );
        assert!(
            text.contains("Body text."),
            "ordinary body text should be unaffected, got: {text:?}"
        );
    }

    /// A comment with no `\atnid` falls back to a running 1-based counter
    /// rather than being dropped or mislabeled.
    #[test]
    fn test_annotation_without_atnid_falls_back_to_counter_label() {
        let rtf = r"{\rtf1\ansi
{\annotation First comment}
{\annotation Second comment}
}";
        let (text, _tables, _images, _para_metas, _fmt) = extract_text_from_rtf(rtf, false);

        assert!(text.contains("[Comment 1]: First comment"), "got: {text:?}");
        assert!(text.contains("[Comment 2]: Second comment"), "got: {text:?}");
    }
}
