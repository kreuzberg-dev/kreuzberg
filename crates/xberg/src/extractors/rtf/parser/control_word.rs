//! RTF control-word dispatch used by [`super::text_extract::extract_text_from_rtf`].

use super::{FormattingTracker, ParagraphMeta};
use crate::extractors::rtf::images::{RtfImage, extract_pict_image};
use crate::extractors::rtf::tables::TableState;
use crate::types::Table;

/// Bundled mutable parser state threaded through control-word handling.
///
/// Groups the many independent pieces of state a single RTF control word may
/// read or update into one parameter, replacing a long positional argument
/// list (`handle_control_word` previously took 27). `chars`, `control_word`,
/// and `param` stay as direct arguments since every handler needs them
/// positionally rather than as shared state. ~keep
pub(super) struct ControlWordCtx<'a> {
    pub(super) result: &'a mut String,
    pub(super) table_state: &'a mut Option<TableState>,
    pub(super) tables: &'a mut Vec<Table>,
    pub(super) images: &'a mut Vec<RtfImage>,
    pub(super) ensure_table: &'a dyn Fn(&mut Option<TableState>),
    pub(super) finalize_table: &'a dyn Fn(&mut Option<TableState>, &mut Vec<Table>),
    pub(super) plain: bool,
    pub(super) group_has_text: &'a mut [bool],
    pub(super) cur_heading_level: &'a mut u8,
    pub(super) cur_list_level: &'a mut Option<u8>,
    pub(super) cur_list_id: &'a mut Option<u16>,
    pub(super) cur_ordered: &'a mut bool,
    pub(super) para_metas: &'a mut Vec<ParagraphMeta>,
    pub(super) para_meta_emitted: &'a mut bool,
    pub(super) uc_stack: &'a mut Vec<u8>,
    pub(super) ansi_codepage_stack: &'a mut [u32],
    pub(super) footnote_count: &'a mut usize,
    pub(super) pending_boundary_space: &'a mut bool,
    pub(super) hidden_stack: &'a mut Vec<bool>,
    pub(super) fmt_tracker: &'a mut FormattingTracker,
    pub(super) font_id_stack: &'a mut [Option<u16>],
    pub(super) default_font_id: &'a mut Option<u16>,
}

/// Dispatch an RTF control word to its handler.
///
/// Split into three category dispatchers below -- scope/state, text
/// emission, and table/formatting -- purely to keep each match's arm count
/// (and this function's own length) within the repository's limits. Control
/// words are disjoint string literals, so which group checks first cannot
/// change which arm ultimately handles a given word. ~keep
pub(super) fn handle_control_word(
    control_word: &str,
    param: Option<i32>,
    chars: &mut std::iter::Peekable<std::str::Chars>,
    ctx: &mut ControlWordCtx,
) {
    if handle_scope_control_word(control_word, param, ctx) {
        return;
    }
    if handle_text_emission_control_word(control_word, param, chars, ctx) {
        return;
    }
    handle_table_or_formatting_control_word(control_word, param, ctx);
}

/// Handle font/codepage/paragraph-scope control words. Returns `false` for
/// any word it does not recognize, leaving it for the next dispatcher.
fn handle_scope_control_word(control_word: &str, param: Option<i32>, ctx: &mut ControlWordCtx) -> bool {
    match control_word {
        "f" => {
            if let Some(val) = param
                && let Some(font_id) = ctx.font_id_stack.last_mut()
            {
                *font_id = Some(val.max(0) as u16);
            }
        }
        "deff" => {
            if let Some(val) = param {
                *ctx.default_font_id = Some(val.max(0) as u16);
            }
        }
        "v" => {
            let hidden = param.unwrap_or(1) != 0;
            if let Some(h) = ctx.hidden_stack.last_mut() {
                *h = hidden;
            }
        }
        "pard" => handle_pard_control_word(ctx),
        "outlinelevel" => {
            if let Some(level) = param {
                *ctx.cur_heading_level = (level as u8) + 1;
            }
        }
        "ilvl" => {
            *ctx.cur_list_level = Some(param.unwrap_or(0) as u8);
        }
        "ls" => {
            *ctx.cur_list_id = Some(param.unwrap_or(0) as u16);
        }
        "uc" => {
            if let Some(val) = param
                && let Some(uc) = ctx.uc_stack.last_mut()
            {
                *uc = val.max(0) as u8;
            }
        }
        "ansicpg" => {
            if let Some(val) = param
                && val > 0
                && let Some(codepage) = ctx.ansi_codepage_stack.last_mut()
            {
                *codepage = val as u32;
            }
        }
        _ => return false,
    }
    true
}

/// Handle control words that emit text into the output (Unicode escapes,
/// images, paragraph/line/tab breaks, fixed-text literals). Returns `false`
/// for any word it does not recognize, leaving it for the next dispatcher.
fn handle_text_emission_control_word(
    control_word: &str,
    param: Option<i32>,
    chars: &mut std::iter::Peekable<std::str::Chars>,
    ctx: &mut ControlWordCtx,
) -> bool {
    match control_word {
        "u" => handle_u_control_word(param, chars, ctx),
        "chftn" => {
            *ctx.footnote_count += 1;
            let marker = format!("[^{}]", *ctx.footnote_count);
            if let Some(state) = ctx.table_state.as_mut()
                && state.in_row
            {
                state.current_cell.push_str(&marker);
            } else {
                ctx.result.push_str(&marker);
                if let Some(flag) = ctx.group_has_text.last_mut() {
                    *flag = true;
                }
            }
        }
        "pict" => handle_pict_control_word(chars, ctx),
        "par" | "line" => handle_par_or_line(ctx),
        "tab" => {
            if let Some(state) = ctx.table_state.as_mut()
                && state.in_row
            {
                state.current_cell.push('\t');
            } else {
                ctx.result.push('\t');
                if let Some(flag) = ctx.group_has_text.last_mut() {
                    *flag = true;
                }
            }
        }
        "bullet" | "lquote" | "rquote" | "ldblquote" | "rdblquote" | "endash" | "emdash" => {
            if let Some(c) = literal_char_for_control_word(control_word) {
                push_literal_char(ctx, c);
            }
        }
        _ => return false,
    }
    true
}

/// Handle table-structure and character-formatting control words. Any word
/// unrecognized here (or by the two dispatchers above) is a no-op, matching
/// the original single match's `_ => {}` fallback.
fn handle_table_or_formatting_control_word(control_word: &str, param: Option<i32>, ctx: &mut ControlWordCtx) {
    match control_word {
        "trowd" => {
            (ctx.ensure_table)(ctx.table_state);
            if let Some(state) = ctx.table_state.as_mut() {
                state.start_row();
            }
        }
        "cell" => {
            if let Some(state) = ctx.table_state.as_mut()
                && state.in_row
            {
                state.push_cell();
            }
        }
        "row" => handle_row_control_word(ctx),
        "intbl" => {
            (ctx.ensure_table)(ctx.table_state);
            if let Some(state) = ctx.table_state.as_mut()
                && !state.in_row
            {
                state.start_row();
            }
        }
        "b" => {
            ctx.fmt_tracker.update_bold(ctx.result.len(), param.unwrap_or(1) != 0);
        }
        "i" => {
            ctx.fmt_tracker.update_italic(ctx.result.len(), param.unwrap_or(1) != 0);
        }
        "ul" => {
            ctx.fmt_tracker
                .update_underline(ctx.result.len(), param.unwrap_or(1) != 0);
        }
        "ulnone" => {
            ctx.fmt_tracker.update_underline(ctx.result.len(), false);
        }
        "strike" => {
            ctx.fmt_tracker
                .update_strikethrough(ctx.result.len(), param.unwrap_or(1) != 0);
        }
        "cf" => {
            ctx.fmt_tracker
                .update_color(ctx.result.len(), param.unwrap_or(0) as u16);
        }
        "plain" => {
            if let Some(h) = ctx.hidden_stack.last_mut() {
                *h = false;
            }
            ctx.fmt_tracker.reset_all(ctx.result.len());
        }
        _ => {}
    }
}

/// Map a fixed-text control word to the literal character it emits.
///
/// Only called for the seven words already matched by the caller's combined
/// arm, so `None` cannot actually occur there -- returning `Option` rather
/// than panicking keeps this helper safe to call on its own. ~keep
fn literal_char_for_control_word(word: &str) -> Option<char> {
    Some(match word {
        "bullet" => '\u{2022}',
        "lquote" => '\u{2018}',
        "rquote" => '\u{2019}',
        "ldblquote" => '\u{201C}',
        "rdblquote" => '\u{201D}',
        "endash" => '\u{2013}',
        "emdash" => '\u{2014}',
        _ => return None,
    })
}

/// Push a single literal character produced by a fixed-text control word
/// (`\bullet`, `\lquote`, etc.), matching the char-append + group-has-text
/// bookkeeping every such control word performs. ~keep
fn push_literal_char(ctx: &mut ControlWordCtx, ch: char) {
    ctx.result.push(ch);
    if let Some(flag) = ctx.group_has_text.last_mut() {
        *flag = true;
    }
}

/// Handle `\pard`: close the current paragraph and reset per-paragraph state.
fn handle_pard_control_word(ctx: &mut ControlWordCtx) {
    let in_table_row = ctx.table_state.as_ref().is_some_and(|s| s.in_row);
    if !in_table_row && !ctx.result.is_empty() && !ctx.result.ends_with('\n') && !*ctx.para_meta_emitted {
        ctx.para_metas.push(ParagraphMeta {
            heading_level: *ctx.cur_heading_level,
            list_level: *ctx.cur_list_level,
            list_id: *ctx.cur_list_id,
            is_table: false,
            ordered: *ctx.cur_ordered,
        });
        ctx.result.push('\n');
        ctx.result.push('\n');
        if let Some(flag) = ctx.group_has_text.last_mut() {
            *flag = true;
        }
    }
    *ctx.para_meta_emitted = false;
    *ctx.cur_heading_level = 0;
    *ctx.cur_list_level = None;
    *ctx.cur_list_id = None;
    *ctx.cur_ordered = false;
}

/// Handle `\uN`: emit the Unicode fallback character and skip the following
/// `\ucN`-controlled number of fallback characters (or `\'hh` escapes).
fn handle_u_control_word(
    param: Option<i32>,
    chars: &mut std::iter::Peekable<std::str::Chars>,
    ctx: &mut ControlWordCtx,
) {
    let Some(code_num) = param else { return };
    let code_u = if code_num < 0 {
        (code_num + 65536) as u32
    } else {
        code_num as u32
    };
    if let Some(c) = char::from_u32(code_u) {
        if let Some(state) = ctx.table_state.as_mut()
            && state.in_row
        {
            state.current_cell.push(c);
        } else {
            if *ctx.pending_boundary_space
                && !ctx.result.is_empty()
                && !ctx.result.ends_with(' ')
                && !ctx.result.ends_with('\n')
            {
                ctx.result.push(' ');
            }
            *ctx.pending_boundary_space = false;
            ctx.result.push(c);
            if let Some(flag) = ctx.group_has_text.last_mut() {
                *flag = true;
            }
        }
    }
    let uc_count = ctx.uc_stack.last().copied().unwrap_or(1);
    let mut skipped = 0u8;
    while skipped < uc_count {
        let Some(&next) = chars.peek() else { break };
        if next == '{' || next == '}' {
            break;
        }
        if next != '\\' {
            chars.next();
            skipped += 1;
            continue;
        }
        chars.next();
        let Some(&apos) = chars.peek() else { break };
        if apos != '\'' {
            break;
        }
        chars.next();
        chars.next();
        chars.next();
        skipped += 1;
    }
}

/// Handle `\pict`: extract image metadata/bytes and emit a markdown image
/// placeholder unless plain-text output was requested.
fn handle_pict_control_word(chars: &mut std::iter::Peekable<std::str::Chars>, ctx: &mut ControlWordCtx) {
    let (image_metadata, rtf_image) = extract_pict_image(chars);
    if let Some(img) = rtf_image {
        ctx.images.push(img);
    }
    if !image_metadata.is_empty() && !ctx.plain {
        let img_md = format!("![image]({image_metadata}) ");
        if let Some(state) = ctx.table_state.as_mut()
            && state.in_row
        {
            state.current_cell.push_str(&img_md);
        } else {
            if let Some(flag) = ctx.group_has_text.last_mut() {
                *flag = true;
            }
            ctx.result.push_str(&img_md);
        }
    }
}

/// Handle `\par`/`\line`: close the current cell/paragraph as appropriate.
fn handle_par_or_line(ctx: &mut ControlWordCtx) {
    *ctx.pending_boundary_space = false;
    let in_table_row = ctx.table_state.as_ref().is_some_and(|s| s.in_row);
    if in_table_row {
        if let Some(state) = ctx.table_state.as_mut()
            && !state.current_cell.is_empty()
            && !state.current_cell.ends_with(' ')
        {
            state.current_cell.push(' ');
        }
        return;
    }
    let still_in_table = ctx.table_state.as_ref().is_some_and(|s| s.expecting_next_row);
    if ctx.table_state.is_some() && !still_in_table {
        (ctx.finalize_table)(ctx.table_state, ctx.tables);
    }
    if !ctx.result.is_empty() && !ctx.result.ends_with('\n') {
        if !*ctx.para_meta_emitted {
            ctx.para_metas.push(ParagraphMeta {
                heading_level: *ctx.cur_heading_level,
                list_level: *ctx.cur_list_level,
                list_id: *ctx.cur_list_id,
                is_table: false,
                ordered: *ctx.cur_ordered,
            });
            *ctx.para_meta_emitted = true;
        }
        ctx.result.push('\n');
        ctx.result.push('\n');
    }
    if let Some(flag) = ctx.group_has_text.last_mut() {
        *flag = true;
    }
}

/// Handle `\row`: close the current table row and emit its text placeholder.
fn handle_row_control_word(ctx: &mut ControlWordCtx) {
    (ctx.ensure_table)(ctx.table_state);
    if let Some(state) = ctx.table_state.as_mut()
        && (state.in_row || !state.current_cell.is_empty())
    {
        state.push_row();
    }
    if !ctx.result.is_empty() && !ctx.result.ends_with('\n') {
        ctx.result.push('\n');
        ctx.result.push('\n');
    }
    ctx.result.push_str("[TABLE_ROW]");
    ctx.result.push('\n');
    ctx.result.push('\n');
    if let Some(flag) = ctx.group_has_text.last_mut() {
        *flag = true;
    }
    *ctx.para_meta_emitted = true;
    ctx.para_metas.push(ParagraphMeta {
        is_table: true,
        ..Default::default()
    });
}
