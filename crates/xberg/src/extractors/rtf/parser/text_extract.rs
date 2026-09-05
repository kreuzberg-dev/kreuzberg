//! The text-extraction pass: [`extract_text_from_rtf`] and RTF control-word
//! dispatch (`handle_control_word` and its per-word helpers).

use super::control_word::{ControlWordCtx, handle_control_word};
use super::{
    FldrsltCloseState, FormattingTracker, ParagraphMeta, RtfFormattingData, close_fldinst_group, close_fldrslt_group,
    consume_adjacent_hex_escape, parse_font_charset_table, parse_rtf_color_table, resolve_decode_codepage,
};
use crate::extractors::rtf::encoding::{decode_ansi_bytes, parse_hex_byte, parse_rtf_control_word};
use crate::extractors::rtf::formatting::{map_offset, normalize_whitespace_with_mapping};
use crate::extractors::rtf::images::RtfImage;
use crate::extractors::rtf::tables::TableState;
use crate::types::Table;

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

/// Close a `\listtext`/`\pntext` group if this `}` ends it, marking the
/// current list item as ordered when its buffered text looks like a numbered
/// or lettered marker (e.g. `1.` or `a)`). ~keep
fn close_listtext_group(
    group_depth: i32,
    in_listtext: &mut bool,
    listtext_depth: i32,
    listtext_buf: &mut String,
    cur_ordered: &mut bool,
) {
    if !*in_listtext || group_depth >= listtext_depth {
        return;
    }
    *in_listtext = false;
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
        *cur_ordered = true;
    }
    listtext_buf.clear();
}

/// Close a `\footnote` destination if this `}` ends it, recording the
/// buffered note text.
fn close_footnote_group(
    group_depth: i32,
    in_footnote: &mut bool,
    footnote_depth: i32,
    footnote_buf: &mut String,
    footnotes: &mut Vec<String>,
) {
    if !*in_footnote || group_depth >= footnote_depth {
        return;
    }
    *in_footnote = false;
    let note = footnote_buf.trim().to_string();
    if !note.is_empty() {
        footnotes.push(note);
    }
    footnote_buf.clear();
}

/// Close a `\shptxt` (drawing-object/text-box text) destination if this `}`
/// ends it, recording the buffered text-box text.
fn close_shptxt_group(
    group_depth: i32,
    in_shptxt: &mut bool,
    shptxt_depth: i32,
    shptxt_buf: &mut String,
    text_boxes: &mut Vec<String>,
) {
    if !*in_shptxt || group_depth >= shptxt_depth {
        return;
    }
    *in_shptxt = false;
    let text_box = shptxt_buf.trim().to_string();
    if !text_box.is_empty() {
        text_boxes.push(text_box);
    }
    shptxt_buf.clear();
}

/// Close an `\annotation` (Word comment) destination if this `}` ends it,
/// labeling the comment with its `\atnid` (if any) or a running counter.
fn close_annotation_group(
    group_depth: i32,
    in_annotation: &mut bool,
    annotation_depth: i32,
    annotation_buf: &mut String,
    comments: &mut Vec<String>,
    pending_atnid: &mut Option<i32>,
) {
    if !*in_annotation || group_depth >= annotation_depth {
        return;
    }
    *in_annotation = false;
    let comment = annotation_buf.trim().to_string();
    if !comment.is_empty() {
        let label = pending_atnid
            .take()
            .map(|id| id.to_string())
            .unwrap_or_else(|| (comments.len() + 1).to_string());
        comments.push(format!("[Comment {label}]: {comment}"));
    } else {
        *pending_atnid = None;
    }
    annotation_buf.clear();
}

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
    // lets them survive even while nested under an active `skip_depth` (#86). ~keep
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
                close_listtext_group(
                    group_depth,
                    &mut in_listtext,
                    listtext_depth,
                    &mut listtext_buf,
                    &mut cur_ordered,
                );
                close_fldinst_group(
                    group_depth,
                    &mut in_fldinst,
                    fldinst_depth,
                    &mut fldinst_content,
                    &mut pending_hyperlink_url,
                );
                close_fldrslt_group(
                    group_depth,
                    result.len(),
                    FldrsltCloseState {
                        in_fldrslt: &mut in_fldrslt,
                        fldrslt_depth,
                        fldrslt_start,
                        pending_hyperlink_url: &mut pending_hyperlink_url,
                        hyperlinks: &mut hyperlinks,
                    },
                );
                close_footnote_group(
                    group_depth,
                    &mut in_footnote,
                    footnote_depth,
                    &mut footnote_buf,
                    &mut footnotes,
                );
                close_shptxt_group(
                    group_depth,
                    &mut in_shptxt,
                    shptxt_depth,
                    &mut shptxt_buf,
                    &mut text_boxes,
                );
                close_annotation_group(
                    group_depth,
                    &mut in_annotation,
                    annotation_depth,
                    &mut annotation_buf,
                    &mut comments,
                    &mut pending_atnid,
                );
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
                                &mut ControlWordCtx {
                                    result: &mut result,
                                    table_state: &mut table_state,
                                    tables: &mut tables,
                                    images: &mut images,
                                    ensure_table: &ensure_table,
                                    finalize_table: &finalize_table,
                                    plain,
                                    group_has_text: &mut group_has_text,
                                    cur_heading_level: &mut cur_heading_level,
                                    cur_list_level: &mut cur_list_level,
                                    cur_list_id: &mut cur_list_id,
                                    cur_ordered: &mut cur_ordered,
                                    para_metas: &mut para_metas,
                                    para_meta_emitted: &mut para_meta_emitted,
                                    uc_stack: &mut uc_stack,
                                    ansi_codepage_stack: &mut ansi_codepage_stack,
                                    footnote_count: &mut footnote_count,
                                    pending_boundary_space: &mut pending_boundary_space,
                                    hidden_stack: &mut hidden_stack,
                                    fmt_tracker: &mut fmt_tracker,
                                    font_id_stack: &mut font_id_stack,
                                    default_font_id: &mut default_font_id,
                                },
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
                                &mut ControlWordCtx {
                                    result: &mut result,
                                    table_state: &mut table_state,
                                    tables: &mut tables,
                                    images: &mut images,
                                    ensure_table: &ensure_table,
                                    finalize_table: &finalize_table,
                                    plain,
                                    group_has_text: &mut group_has_text,
                                    cur_heading_level: &mut cur_heading_level,
                                    cur_list_level: &mut cur_list_level,
                                    cur_list_id: &mut cur_list_id,
                                    cur_ordered: &mut cur_ordered,
                                    para_metas: &mut para_metas,
                                    para_meta_emitted: &mut para_meta_emitted,
                                    uc_stack: &mut uc_stack,
                                    ansi_codepage_stack: &mut ansi_codepage_stack,
                                    footnote_count: &mut footnote_count,
                                    pending_boundary_space: &mut pending_boundary_space,
                                    hidden_stack: &mut hidden_stack,
                                    fmt_tracker: &mut fmt_tracker,
                                    font_id_stack: &mut font_id_stack,
                                    default_font_id: &mut default_font_id,
                                },
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
