//! Content stream parser.
//!
//! This module parses PDF content streams into a sequence of operators.
//! Content streams are fundamentally different from the main PDF structure:
//! they use a postfix notation where operands come before operators.
//!
//! Example content stream:
//! ```text
//! BT
//!   /F1 12 Tf
//!   100 700 Td
//!   (Hello, World!) Tj
//! ET
//! ```

// TODO(xberg-io/xberg#1567): 4 cyclomatic-complexity and 12 size/complexity findings
// in this file, currently excluded via the quality-debt baseline in alef.toml. Splitting
// these needs compiler-in-the-loop verification, not a mechanical pass. Delete this
// note and the file's baseline entry together once it goes green. Help wanted.

use crate::content::operators::{Operator, TextElement};
use crate::error::Result;
use crate::object::Object;
use crate::parser::parse_object;
use nom::IResult;
use nom::Parser;
use nom::bytes::complete::take_while1;
use nom::character::complete::multispace0;
use smallvec::SmallVec;
use std::collections::HashMap;

/// Default maximum number of operators to parse from a single content
/// stream. Prevents pathological inputs (e.g., Isartor 6.1.12) from
/// consuming unbounded time and memory.
///
/// Callers can override via [`set_max_ops_per_stream`] to raise the
/// cap (or set `usize::MAX` for effectively unbounded — use with
/// caution on adversarial PDFs).
const MAX_OPERATORS: usize = 1_000_000;

/// Global cap override for content-stream operator count. `0`
/// means "use [`MAX_OPERATORS`] default"; any other value is the
/// effective cap. Atomic so it's safe to set from one thread while
/// extraction runs on another (e.g. parallel-page extraction).
static MAX_OPERATORS_OVERRIDE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Set the global content-stream operator cap. `None` keeps the
/// default of `MAX_OPERATORS` (1,000,000). `Some(n)` overrides to `n`
/// — pass `Some(usize::MAX)` for effectively unbounded.
///
/// Returns the previous override (or `None` if the default was active).
/// The override is process-global; setting it on one thread affects all
/// concurrent extractions.
///
/// **Use case**: large technical PDFs (textbooks, ISO standards) that
/// have legitimate content streams exceeding 1,000,000 operators. The
/// default cap exists to bound the cost of adversarial inputs; raise
/// it when you know the inputs are trusted.
pub fn set_max_ops_per_stream(limit: Option<usize>) -> Option<usize> {
    let new = limit.unwrap_or(0);
    let prev = MAX_OPERATORS_OVERRIDE.swap(new, std::sync::atomic::Ordering::SeqCst);
    if prev == 0 { None } else { Some(prev) }
}

/// Current effective operator cap. Reads the override if set; otherwise
/// returns [`MAX_OPERATORS`]. Internal hot-path helper.
#[inline]
fn effective_max_operators() -> usize {
    let override_val = MAX_OPERATORS_OVERRIDE.load(std::sync::atomic::Ordering::Relaxed);
    if override_val == 0 { MAX_OPERATORS } else { override_val }
}

/// Maximum consecutive parse errors (byte skips) before bailing out.
///
/// If we skip this many bytes without finding a valid operator, the
/// remaining data is likely junk, not a parseable content stream.
const MAX_CONSECUTIVE_ERRORS: usize = 1024;

/// Emit the operator-cap-exceeded warning at the actual *effective* cap
/// (which may have been overridden via `set_max_ops_per_stream`). PDF
/// Spec Annex C documents implementation limits; the cap exists to
/// bound parser cost on adversarial inputs.
#[inline]
fn push_operator_cap_warning() {
    let cap = effective_max_operators();
    let msg = format!("Content stream exceeded {cap} operators, truncating");
    tracing::warn!(count = cap, "content stream exceeded operator cap; truncating");
    crate::extractors::warnings::push_global_warning(crate::extractors::warnings::Warning {
        category: crate::extractors::warnings::WarningCategory::OperatorCapExceeded,
        page: None,
        message: msg,
        spec_section: Some("Annex C"),
    });
}

/// Parse a content stream into a sequence of operators.
///
/// Content streams use postfix notation where operands precede the operator.
/// For example: `100 200 Td` means "move text position to (100, 200)".
///
/// Includes safety limits: bails out after `MAX_OPERATORS` operators or
/// `MAX_CONSECUTIVE_ERRORS` consecutive parse failures.
pub fn parse_content_stream(data: &[u8]) -> Result<Vec<Operator>> {
    let estimated_capacity = data.len() / 20;
    let mut operators = Vec::with_capacity(estimated_capacity.min(100_000));
    let mut input = data;
    let mut consecutive_errors: usize = 0;

    while !input.is_empty() {
        if let Ok((rest, _)) = multispace0::<&[u8], nom::error::Error<&[u8]>>.parse(input) {
            input = rest;
        }

        if input.is_empty() {
            break;
        }

        match parse_operator_with_operands(input) {
            Ok((rest, op)) => {
                operators.push(op);
                input = rest;
                consecutive_errors = 0;

                if operators.len() >= effective_max_operators() {
                    push_operator_cap_warning();
                    break;
                }
            }
            Err(_) => {
                consecutive_errors += 1;
                if consecutive_errors >= MAX_CONSECUTIVE_ERRORS {
                    tracing::warn!(
                        count = MAX_CONSECUTIVE_ERRORS,
                        bytes = input.len(),
                        "content stream had too many consecutive parse errors; bailing out"
                    );
                    break;
                }
                // If we can't parse, skip the problematic byte and continue
                // This makes us more resilient to malformed streams ~keep
                if input.len() > 1 {
                    input = &input[1..];
                } else {
                    break;
                }
            }
        }
    }

    Ok(operators)
}

/// Parse a content stream for path extraction, skipping BT/ET text blocks.
///
/// This is a performance-optimized variant of [`parse_content_stream`] that:
/// 1. Skips text object blocks (BT…ET) at the byte level
/// 2. Parses common path/state/color operators at the byte level using fast
///    float parsing — no nom overhead, no Object heap allocation
/// 3. Falls back to full `parse_operator_with_operands` only for complex
///    operators (Do, gs, d, inline images)
///
/// # Performance
///
/// For graphics-heavy pages (e.g., 15 MB of vector paths with 50K cm operators),
/// this is 2–4× faster than the full parser because it avoids allocating
/// `Object::Real` for each numeric operand. Common operators like `m`, `l`, `c`,
/// `re`, `cm`, `rg` are parsed entirely from raw bytes.
pub fn parse_content_stream_paths_only(data: &[u8]) -> Result<Vec<Operator>> {
    let estimated_capacity = data.len() / 20;
    let mut operators = Vec::with_capacity(estimated_capacity.min(100_000));
    let len = data.len();
    let mut i: usize = 0;
    let mut operand_start: usize = 0;
    let mut consecutive_errors: usize = 0;

    loop {
        while i < len && BYTE_CLASS[data[i] as usize] == SCAN_SKIP {
            i += 1;
        }
        if i >= len {
            break;
        }
        if operators.len() >= effective_max_operators() {
            push_operator_cap_warning();
            break;
        }

        match BYTE_CLASS[data[i] as usize] {
            SCAN_ALPHA => {
                let first_byte = data[i];
                let second_is_non_alpha = i + 1 >= len || BYTE_CLASS[data[i + 1] as usize] != SCAN_ALPHA;

                if second_is_non_alpha {
                    let operands = &data[operand_start..i];
                    let op = match first_byte {
                        b'S' => Some(Operator::Stroke),
                        b'n' => Some(Operator::EndPath),
                        b'h' => Some(Operator::ClosePath),
                        b'q' => Some(Operator::SaveState),
                        b'Q' => Some(Operator::RestoreState),
                        b'm' => parse_floats::<2>(operands).map(|f| Operator::MoveTo { x: f[0], y: f[1] }),
                        b'l' => parse_floats::<2>(operands).map(|f| Operator::LineTo { x: f[0], y: f[1] }),
                        b'c' => parse_floats::<6>(operands).map(|f| Operator::CurveTo {
                            x1: f[0],
                            y1: f[1],
                            x2: f[2],
                            y2: f[3],
                            x3: f[4],
                            y3: f[5],
                        }),
                        b'v' => parse_floats::<4>(operands).map(|f| Operator::CurveToV {
                            x2: f[0],
                            y2: f[1],
                            x3: f[2],
                            y3: f[3],
                        }),
                        b'y' => parse_floats::<4>(operands).map(|f| Operator::CurveToY {
                            x1: f[0],
                            y1: f[1],
                            x3: f[2],
                            y3: f[3],
                        }),
                        b'w' => parse_floats::<1>(operands).map(|f| Operator::SetLineWidth { width: f[0] }),
                        b'J' => parse_floats::<1>(operands).map(|f| Operator::SetLineCap { cap_style: f[0] as u8 }),
                        b'j' => parse_floats::<1>(operands).map(|f| Operator::SetLineJoin { join_style: f[0] as u8 }),
                        b'M' => parse_floats::<1>(operands).map(|f| Operator::SetMiterLimit { limit: f[0] }),
                        b'g' => parse_floats::<1>(operands).map(|f| Operator::SetFillGray { gray: f[0] }),
                        b'G' => parse_floats::<1>(operands).map(|f| Operator::SetStrokeGray { gray: f[0] }),
                        b'f' | b'F' => Some(Operator::Fill),
                        b'B' => Some(Operator::FillStroke),
                        b'b' => Some(Operator::CloseFillStroke),
                        // s = close path + stroke (not a named variant, emit ClosePath + Stroke)
                        // ~keep
                        b's' => {
                            operators.push(Operator::ClosePath);
                            Some(Operator::Stroke)
                        }
                        b'W' => Some(Operator::ClipNonZero),
                        b'i' => {
                            operand_start = i + 1;
                            i += 1;
                            consecutive_errors = 0;
                            continue;
                        }
                        _ => None,
                    };
                    if let Some(op) = op {
                        operators.push(op);
                        i += 1;
                        operand_start = i;
                        consecutive_errors = 0;
                        continue;
                    }
                }

                let op_start = i;
                while i < len
                    && (data[i].is_ascii_alphanumeric() || data[i] == b'\'' || data[i] == b'"' || data[i] == b'*')
                {
                    i += 1;
                }
                let op = &data[op_start..i];

                if op == b"true" || op == b"false" || op == b"null" {
                    consecutive_errors = 0;
                    continue;
                }

                consecutive_errors = 0;
                let operands = &data[operand_start..op_start];

                if op == b"BT" {
                    match scan_to_et(&data[i..]) {
                        Some(rest) => {
                            i = len - rest.len();
                            operand_start = i;
                        }
                        None => break,
                    }
                    continue;
                }

                let fast_op = match op {
                    b"cm" => parse_six_floats(operands).map(|(a, b, c, d, e, f)| Operator::Cm { a, b, c, d, e, f }),
                    b"re" => parse_floats::<4>(operands).map(|f| Operator::Rectangle {
                        x: f[0],
                        y: f[1],
                        width: f[2],
                        height: f[3],
                    }),
                    b"rg" => parse_floats::<3>(operands).map(|f| Operator::SetFillRgb {
                        r: f[0],
                        g: f[1],
                        b: f[2],
                    }),
                    b"RG" => parse_floats::<3>(operands).map(|f| Operator::SetStrokeRgb {
                        r: f[0],
                        g: f[1],
                        b: f[2],
                    }),
                    b"k" => parse_floats::<4>(operands).map(|f| Operator::SetFillCmyk {
                        c: f[0],
                        m: f[1],
                        y: f[2],
                        k: f[3],
                    }),
                    b"K" => parse_floats::<4>(operands).map(|f| Operator::SetStrokeCmyk {
                        c: f[0],
                        m: f[1],
                        y: f[2],
                        k: f[3],
                    }),
                    b"f*" => Some(Operator::FillEvenOdd),
                    b"B*" => Some(Operator::FillStrokeEvenOdd),
                    b"b*" => Some(Operator::CloseFillStrokeEvenOdd),
                    b"W*" => Some(Operator::ClipEvenOdd),
                    // Skip text/color-space/shading operators that don't affect paths ~keep
                    b"ET" | b"Tc" | b"Tw" | b"Tz" | b"TL" | b"Tf" | b"Tr" | b"Ts" | b"Td" | b"TD" | b"Tm" | b"Tj"
                    | b"TJ" | b"T*" | b"cs" | b"CS" | b"sc" | b"SC" | b"scn" | b"SCN" | b"ri" | b"sh" | b"EI" => {
                        operand_start = i;
                        continue;
                    }
                    _ => None,
                };

                if let Some(op) = fast_op {
                    operators.push(op);
                    operand_start = i;
                    continue;
                }

                match parse_operator_with_operands(&data[operand_start..]) {
                    Ok((rest, op)) => {
                        operators.push(op);
                        i = len - rest.len();
                        operand_start = i;
                    }
                    Err(_) => {
                        operand_start = i;
                    }
                }
            }

            SCAN_PAREN => match skip_literal_string_raw(data, i) {
                Some(end) => {
                    i = end;
                    consecutive_errors = 0;
                }
                None => {
                    i += 1;
                    consecutive_errors += 1;
                }
            },
            SCAN_ANGLE => {
                if i + 1 < len && data[i + 1] == b'<' {
                    // Dictionary — skip (shouldn't appear in content streams) ~keep
                    i += 2;
                } else {
                    match skip_hex_string_raw(data, i) {
                        Some(end) => {
                            i = end;
                            consecutive_errors = 0;
                        }
                        None => {
                            i += 1;
                            consecutive_errors += 1;
                        }
                    }
                }
            }
            SCAN_BRACKET => {
                i = skip_bracket_array_scan(data, i);
            }
            SCAN_SLASH => {
                i = skip_name_raw(data, i);
            }
            SCAN_PERCENT => {
                while i < len && data[i] != b'\n' && data[i] != b'\r' {
                    i += 1;
                }
            }
            _ => {
                i += 1;
                consecutive_errors += 1;
                if consecutive_errors >= MAX_CONSECUTIVE_ERRORS {
                    tracing::warn!(
                        count = MAX_CONSECUTIVE_ERRORS,
                        bytes = len - i,
                        "content stream had too many consecutive parse errors; bailing out"
                    );
                    break;
                }
            }
        }
    }

    Ok(operators)
}

/// Skips a `[...]`-delimited array in the paths-only fast scanner, honoring
/// nested arrays and literal-string parens so a `)` or `]` inside a string
/// doesn't terminate the array early. Pure code motion out of the
/// `SCAN_BRACKET` arm of `parse_content_stream_paths_only`. ~keep
fn skip_bracket_array_scan(data: &[u8], start: usize) -> usize {
    let len = data.len();
    let mut i = start + 1;
    let mut depth = 1u32;
    while i < len && depth > 0 {
        match data[i] {
            b'[' => depth += 1,
            b']' => depth -= 1,
            b'(' => {
                if let Some(end) = skip_literal_string_raw(data, i) {
                    i = end;
                    continue;
                }
            }
            _ => {}
        }
        i += 1;
    }
    i
}

/// Parse a content stream for text extraction, skipping pure graphics operators.
///
/// This is a performance-optimized variant of [`parse_content_stream`] that
/// avoids constructing `Object` operands for operators that only affect paths,
/// clipping, and non-text graphics state. Inside BT/ET text blocks, parsing is
/// identical to the full parser.
///
/// # Performance
///
/// For graphics-heavy pages (e.g., 1–12 MB of path data), this can be 3–5x
/// faster than full parsing while producing identical text extraction results.
/// The speedup comes from byte-level operand skipping (no `f64` parsing, no
/// heap allocation) and discarding path/clipping operators entirely.
///
/// # Safety limits
///
/// Same as [`parse_content_stream`]: bails out after `MAX_OPERATORS`
/// operators or `MAX_CONSECUTIVE_ERRORS` consecutive parse failures.
#[deprecated(
    since = "0.3.78",
    note = "not equivalent to `parse_and_execute_text_only`, which reconstructs the graphics \
            state around >256KB text regions by a different route; extraction no longer calls \
            this parser"
)]
pub fn parse_content_stream_text_only(data: &[u8]) -> Result<Vec<Operator>> {
    let estimated_capacity = data.len() / 40;
    let mut operators = Vec::with_capacity(estimated_capacity.min(50_000));
    let mut input = data;
    let mut consecutive_errors: usize = 0;
    let mut inside_text = false;

    while !input.is_empty() {
        if let Ok((rest, _)) = multispace0::<&[u8], nom::error::Error<&[u8]>>.parse(input) {
            input = rest;
        }
        if input.is_empty() {
            break;
        }

        if operators.len() >= effective_max_operators() {
            push_operator_cap_warning();
            break;
        }

        if inside_text {
            match parse_operator_with_operands(input) {
                Ok((rest, op)) => {
                    if matches!(op, Operator::EndText) {
                        inside_text = false;
                    }
                    operators.push(op);
                    input = rest;
                    consecutive_errors = 0;
                }
                Err(_) => {
                    consecutive_errors += 1;
                    if consecutive_errors >= MAX_CONSECUTIVE_ERRORS {
                        tracing::warn!(
                            count = MAX_CONSECUTIVE_ERRORS,
                            bytes = input.len(),
                            "content stream had too many consecutive parse errors; bailing out"
                        );
                        break;
                    }
                    if input.len() > 1 {
                        input = &input[1..];
                    } else {
                        break;
                    }
                }
            }
        } else {
            // Outside BT/ET: byte-level scan — skip operands and graphics
            // operators using raw index arithmetic (no nom IResult overhead). ~keep
            let should_continue = handle_graphics_region_step_buffered(
                &mut input,
                &mut consecutive_errors,
                &mut operators,
                &mut inside_text,
            );
            if !should_continue {
                break;
            }
        }
    }

    Ok(operators)
}

/// Handles one iteration of `parse_content_stream_text_only`'s outer loop
/// while outside a `BT`/`ET` text block: dispatches on `scan_graphics_region`
/// and pushes any operator it should emit into `operators`. Returns `false`
/// when the caller's loop should stop (end of data or too many consecutive
/// errors). Pure code motion out of the function's `else` branch. ~keep
fn handle_graphics_region_step_buffered(
    input: &mut &[u8],
    consecutive_errors: &mut usize,
    operators: &mut Vec<Operator>,
    inside_text: &mut bool,
) -> bool {
    match scan_graphics_region(input, consecutive_errors) {
        ScanResult::EndOfData => false,
        ScanResult::FoundBT { rest } => {
            operators.push(Operator::BeginText);
            *input = rest;
            *inside_text = true;
            true
        }
        ScanResult::InlineImage { rest } => {
            match parse_inline_image(rest) {
                Ok((rest2, _)) => *input = rest2,
                Err(_) => *input = rest,
            }
            true
        }
        ScanResult::NeedFullParse {
            operand_start,
            after_op,
        } => {
            match parse_operator_with_operands(operand_start) {
                Ok((rest2, op)) => {
                    operators.push(op);
                    *input = rest2;
                }
                Err(_) => *input = after_op,
            }
            true
        }
        ScanResult::DeferredThenText {
            deferred_start,
            trigger_start,
        } => {
            // Re-parse the deferred q/cm/Q region to emit CTM-affecting ops.
            // The trigger (BT/BI/Do/etc.) is NOT included — the next iteration
            // of the outer loop re-enters scan_graphics_region which returns
            // the trigger via FoundBT / InlineImage / NeedFullParse. ~keep
            replay_deferred_operators(deferred_start, trigger_start, operators);
            *input = trigger_start;
            *consecutive_errors = 0;
            true
        }
        ScanResult::SimpleOp { op, rest } => {
            operators.push(op);
            *input = rest;
            true
        }
        ScanResult::TooManyErrors { remaining } => {
            tracing::warn!(
                count = MAX_CONSECUTIVE_ERRORS,
                bytes = remaining.len(),
                "content stream had too many consecutive parse errors; bailing out"
            );
            false
        }
    }
}

/// Replays operators from `deferred_start` up to (but not including)
/// `trigger_start`, pushing each into `operators`. Used when the
/// graphics-region scanner defers CTM-affecting operators (q/cm/Q) ahead of a
/// BT/BI/Do trigger it hasn't consumed yet. Pure code motion out of the
/// `DeferredThenText` handling shared by the vec-building content-stream
/// parsers. ~keep
fn replay_deferred_operators(deferred_start: &[u8], trigger_start: &[u8], operators: &mut Vec<Operator>) {
    let mut remaining = deferred_start;
    while remaining.len() > trigger_start.len() {
        match parse_operator_with_operands(remaining) {
            Ok((rest2, op)) => {
                operators.push(op);
                remaining = rest2;
            }
            Err(_) => {
                if remaining.len() > 1 {
                    remaining = &remaining[1..];
                } else {
                    break;
                }
            }
        }
    }
}

/// Text-state parameters tracked by [`forward_scan_ctm`].
///
/// These persist across `BT`/`ET` (ISO 32000-1 §9.3 — `BT` resets only the
/// text and line matrices, §9.4.1) and are saved/restored by `q`/`Q`, so a
/// BT block may rely on a value set by an earlier block or between blocks.
/// The prescan path parses each region in isolation inside a synthesized
/// SaveState/RestoreState wrap, which would silently reset them; the scan
/// tracks them so the region wrapper can re-inject the inherited values.
#[derive(Debug, Clone, Copy, PartialEq)]
struct PrescanTextState {
    /// Character spacing (`Tc`).
    char_spacing: f32,
    /// Word spacing (`Tw`).
    word_spacing: f32,
    /// Horizontal scaling percentage (`Tz`).
    horiz_scaling: f32,
    /// Text leading (`TL`, and `TD`'s implicit `TL = -ty`).
    leading: f32,
    /// Text rise (`Ts`).
    rise: f32,
    /// Text rendering mode (`Tr`).
    render_mode: u8,
}

impl Default for PrescanTextState {
    fn default() -> Self {
        PrescanTextState {
            char_spacing: 0.0,
            word_spacing: 0.0,
            horiz_scaling: 100.0,
            leading: 0.0,
            rise: 0.0,
            render_mode: 0,
        }
    }
}

/// Graphics state snapshot captured at a text position by [`forward_scan_ctm`].
#[derive(Debug)]
struct PrescanState {
    /// Accumulated CTM components (a, b, c, d, e, f).
    ctm: (f32, f32, f32, f32, f32, f32),
    /// Current font name and size from the most recent `Tf` operator, if any.
    font: Option<(String, f32)>,
    /// Persistent text-state parameters at this position.
    text: PrescanTextState,
    /// Fill color space set by `cs`, if any. Replayed before `fill_op` so the
    /// components are interpreted in the right space.
    fill_space: Option<String>,
    /// Most recent fill-color operator (`rg`/`g`/`k`/`sc`/`scn`), if any.
    /// Glyph color comes from the fill state, so losing this across region
    /// boundaries turns colored text black (`chars_hash`-only sweep diffs).
    fill_op: Option<Operator>,
}

/// Lightweight forward scan that tracks graphics state across the full content stream.
///
/// Scans the stream recognizing only `q`, `Q`, `cm`, `Tf`, the persistent
/// text-state operators (`Tc`, `Tw`, `Tz`, `TL`, `Ts`, `Tr`, plus `TD`'s
/// implicit leading and `"`'s implicit `Tw`/`Tc`), and the fill-color
/// operators (`rg`, `g`, `k`, `cs`, `sc`, `scn`), skipping all path and
/// text-showing operators.
/// Records the accumulated CTM, font, and text state at each position in
/// `text_positions`.
///
/// This is much cheaper than full parsing. Numeric operands are tracked in a
/// rolling buffer so `cm` operands are always available when the operator is
/// encountered.
///
/// # Arguments
///
/// * `data` - Raw content stream bytes
/// * `text_positions` - Byte offsets of BT/Do operators to record state at
///
/// # Returns
///
/// One [`PrescanState`] per entry in `text_positions` (same order).
/// Returns `None` if the scan encounters unrecoverable problems.
fn forward_scan_ctm(data: &[u8], text_positions: &[usize]) -> Option<Vec<PrescanState>> {
    use crate::content::graphics_state::Matrix;

    if text_positions.is_empty() {
        return Some(Vec::new());
    }

    // Font tracking: store (name, size) pairs in a table, reference by index
    // in ctm_stack to avoid String cloning on every q/Q. ~keep
    let mut font_table: Vec<(String, f32)> = Vec::new();
    let mut current_font_idx: Option<usize> = None;
    let mut ctm_stack: Vec<(
        Matrix,
        Option<usize>,
        PrescanTextState,
        Option<String>,
        Option<Operator>,
    )> = Vec::with_capacity(32);
    let mut ctm = Matrix::identity();
    let mut text_state = PrescanTextState::default();
    let mut fill_space: Option<String> = None;
    let mut fill_op: Option<Operator> = None;

    // Rolling buffer of recent numeric operands (for cm's 6 floats) ~keep
    let mut num_buf: [f32; 6] = [0.0; 6];
    let mut num_count: usize = 0;

    let mut last_name: Option<String> = None;

    let mut sorted_positions: Vec<(usize, usize)> = text_positions.iter().copied().enumerate().collect();
    sorted_positions.sort_by_key(|&(_, pos)| pos);
    let mut next_tp_idx = 0;

    let mut results: Vec<PrescanState> = (0..text_positions.len())
        .map(|_| PrescanState {
            ctm: (0.0, 0.0, 0.0, 0.0, 0.0, 0.0),
            font: None,
            text: PrescanTextState::default(),
            fill_space: None,
            fill_op: None,
        })
        .collect();

    let len = data.len();
    let mut i = 0;

    while i < len {
        while next_tp_idx < sorted_positions.len() && sorted_positions[next_tp_idx].1 <= i {
            let (orig_idx, _) = sorted_positions[next_tp_idx];
            results[orig_idx] = PrescanState {
                ctm: (ctm.a, ctm.b, ctm.c, ctm.d, ctm.e, ctm.f),
                font: current_font_idx.map(|idx| font_table[idx].clone()),
                text: text_state,
                fill_space: fill_space.clone(),
                fill_op: fill_op.clone(),
            };
            next_tp_idx += 1;
        }

        if next_tp_idx >= sorted_positions.len() {
            break;
        }

        let b = data[i];

        if b.is_ascii_whitespace() {
            i += 1;
            continue;
        }

        if b.is_ascii_digit() || b == b'-' || b == b'+' || (b == b'.' && i + 1 < len && data[i + 1].is_ascii_digit()) {
            let start = i;
            i += 1;
            while i < len && (data[i].is_ascii_digit() || data[i] == b'.') {
                i += 1;
            }
            if let Ok(val) = std::str::from_utf8(&data[start..i]).unwrap_or("").parse::<f32>() {
                if num_count < 6 {
                    num_buf[num_count] = val;
                    num_count += 1;
                } else {
                    num_buf.rotate_left(1);
                    num_buf[5] = val;
                }
            }
            continue;
        }

        if b.is_ascii_alphabetic() {
            let op_start = i;
            i += 1;
            while i < len && (data[i].is_ascii_alphabetic() || data[i] == b'*' || data[i] == b'\'' || data[i] == b'"') {
                i += 1;
            }
            let op = &data[op_start..i];

            match op {
                b"BI" => {
                    // Inline image (§8.9.7): `BI key/val pairs ID
                    // <binary data> EI`. The binary image bytes can
                    // contain stray q/Q/cm-shaped ASCII sequences
                    // that would corrupt the CTM stack if parsed as
                    // operators. Skip the whole block by scanning
                    // to the first whitespace-bounded `EI` (`EI`
                    // embedded inside the binary data is tolerated
                    // because the surrounding bytes are unlikely to
                    // be whitespace). ~keep
                    num_count = 0;
                    i = skip_inline_image_scan(data, i);
                    continue;
                }
                b"q" => {
                    ctm_stack.push((ctm, current_font_idx, text_state, fill_space.clone(), fill_op.clone()));
                    num_count = 0;
                }
                b"Q" => {
                    if let Some((saved_ctm, saved_font_idx, saved_text, saved_space, saved_fill)) = ctm_stack.pop() {
                        ctm = saved_ctm;
                        current_font_idx = saved_font_idx;
                        text_state = saved_text;
                        fill_space = saved_space;
                        fill_op = saved_fill;
                    }
                    num_count = 0;
                }
                b"cm" => {
                    if num_count >= 6 {
                        let base = num_count - 6;
                        let new_ctm = Matrix {
                            a: num_buf[base],
                            b: num_buf[base + 1],
                            c: num_buf[base + 2],
                            d: num_buf[base + 3],
                            e: num_buf[base + 4],
                            f: num_buf[base + 5],
                        };
                        ctm = new_ctm.multiply(&ctm);
                    }
                    num_count = 0;
                }
                b"Tf" => {
                    record_scan_font(&num_buf, num_count, &last_name, &mut font_table, &mut current_font_idx);
                    num_count = 0;
                }
                b"Tc" => {
                    apply_scan_text_state_f32(&num_buf, num_count, &mut text_state.char_spacing);
                    num_count = 0;
                }
                b"Tw" => {
                    apply_scan_text_state_f32(&num_buf, num_count, &mut text_state.word_spacing);
                    num_count = 0;
                }
                b"Tz" => {
                    apply_scan_text_state_f32(&num_buf, num_count, &mut text_state.horiz_scaling);
                    num_count = 0;
                }
                b"TL" => {
                    apply_scan_text_state_f32(&num_buf, num_count, &mut text_state.leading);
                    num_count = 0;
                }
                b"Ts" => {
                    apply_scan_text_state_f32(&num_buf, num_count, &mut text_state.rise);
                    num_count = 0;
                }
                b"Tr" => {
                    apply_scan_render_mode(&num_buf, num_count, &mut text_state.render_mode);
                    num_count = 0;
                }
                b"TD" => {
                    // `tx ty TD` also sets the leading to -ty (§9.4.2). ~keep
                    apply_scan_leading_from_td(&num_buf, num_count, &mut text_state.leading);
                    num_count = 0;
                }
                b"rg" => {
                    apply_scan_fill_rgb(&num_buf, num_count, &mut fill_op, &mut fill_space);
                    num_count = 0;
                }
                b"g" => {
                    apply_scan_fill_gray(&num_buf, num_count, &mut fill_op, &mut fill_space);
                    num_count = 0;
                }
                b"k" => {
                    apply_scan_fill_cmyk(&num_buf, num_count, &mut fill_op, &mut fill_space);
                    num_count = 0;
                }
                b"cs" => {
                    // `cs` resets the fill color to the space's initial value,
                    // so any earlier color operator no longer applies. ~keep
                    apply_scan_fill_colorspace(&mut last_name, &mut fill_space, &mut fill_op);
                    num_count = 0;
                }
                b"sc" => {
                    // Operands are the TAIL of the rolling buffer, like every
                    // other arm. The buffer holds 6, so at 7+ operands the
                    // leading ones have rotated away and a full count no
                    // longer proves a complete window; replay only the 1-4
                    // component device-sized spaces rather than a truncated,
                    // plausible-looking wrong color. ~keep
                    apply_scan_fill_sc(&num_buf, num_count, &mut fill_op);
                    num_count = 0;
                }
                b"scn" => {
                    apply_scan_fill_scn(&num_buf, num_count, &mut last_name, &mut fill_op);
                    num_count = 0;
                }
                _ => {
                    num_count = 0;
                }
            }
            // A name operand belongs to the operator that follows it; once any
            // operator has executed, a surviving `last_name` is stale. Without
            // this, `/GS0 gs 0.5 scn` fabricated a pattern named GS0. ~keep
            last_name = None;
            continue;
        }

        // `'` and `"` are the only operators that don't start with an
        // alphabetic byte. `aw ac string "` also persistently sets the word
        // and character spacing, like `Tw`/`Tc` (§9.4.3); the operands
        // survive the string skip because strings don't reset the buffer. ~keep
        if b == b'"' || b == b'\'' {
            if b == b'"' && num_count >= 2 {
                text_state.word_spacing = num_buf[num_count - 2];
                text_state.char_spacing = num_buf[num_count - 1];
            }
            num_count = 0;
            last_name = None;
            i += 1;
            continue;
        }

        // Skip string literals to avoid false matches. Numeric operands are
        // kept: they may belong to a following `"`, and any other operator
        // resets the buffer itself. ~keep
        if b == b'(' {
            i += 1;
            let mut depth = 1u32;
            while i < len && depth > 0 {
                match data[i] {
                    b'\\' => i += 1,
                    b'(' => depth += 1,
                    b')' => depth -= 1,
                    _ => {}
                }
                i += 1;
            }
            continue;
        }
        if b == b'<' {
            let (new_i, was_dict) = skip_dict_or_hex_string_scan(data, i);
            i = new_i;
            if was_dict {
                num_count = 0;
            }
            continue;
        }

        if b == b'/' {
            let name_start = i + 1;
            i += 1;
            while i < len
                && !data[i].is_ascii_whitespace()
                && !matches!(
                    data[i],
                    b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%'
                )
            {
                i += 1;
            }
            last_name = std::str::from_utf8(&data[name_start..i]).ok().map(|s| s.to_string());
            num_count = 0;
            continue;
        }
        if b == b'%' {
            while i < len && data[i] != b'\n' && data[i] != b'\r' {
                i += 1;
            }
            continue;
        }

        i += 1;
    }

    while next_tp_idx < sorted_positions.len() {
        let (orig_idx, _) = sorted_positions[next_tp_idx];
        results[orig_idx] = PrescanState {
            ctm: (ctm.a, ctm.b, ctm.c, ctm.d, ctm.e, ctm.f),
            font: current_font_idx.map(|idx| font_table[idx].clone()),
            text: text_state,
            fill_space: fill_space.clone(),
            fill_op: fill_op.clone(),
        };
        next_tp_idx += 1;
    }

    Some(results)
}

/// Skips the binary data of an inline image (`BI ... ID <bytes> EI`) during
/// the forward CTM scan, using an `EI` heuristic bounded by
/// whitespace/delimiter bytes so `EI`-shaped bytes inside the image data
/// don't terminate it early. Pure code motion out of forward_scan_ctm's `BI`
/// match arm. ~keep
fn skip_inline_image_scan(data: &[u8], start: usize) -> usize {
    let len = data.len();
    let mut j = start;
    while j + 1 < len {
        if data[j] == b'E' && data[j + 1] == b'I' {
            let before_ok = j == 0 || data[j - 1].is_ascii_whitespace();
            let after_ok = j + 2 >= len
                || data[j + 2].is_ascii_whitespace()
                || matches!(data[j + 2], b'(' | b'<' | b'[' | b'/' | b'%');
            if before_ok && after_ok {
                j += 2;
                break;
            }
        }
        j += 1;
    }
    j
}

/// Handles the `Tf` operator during the forward CTM scan: pushes the
/// resolved (font name, size) pair into `font_table` and records its index,
/// if both a font name and a size operand are available. Pure code motion
/// out of forward_scan_ctm's `Tf` match arm. ~keep
fn record_scan_font(
    num_buf: &[f32; 6],
    num_count: usize,
    last_name: &Option<String>,
    font_table: &mut Vec<(String, f32)>,
    current_font_idx: &mut Option<usize>,
) {
    if num_count < 1 {
        return;
    }
    let size = num_buf[num_count - 1];
    let Some(name) = last_name else {
        return;
    };
    let idx = font_table.len();
    font_table.push((name.clone(), size));
    *current_font_idx = Some(idx);
}

/// Sets a persistent text-state field from the last pending numeric operand,
/// mirroring the `if num_count >= 1 { field = num_buf[num_count - 1]; }`
/// shape shared by `Tc`, `Tw`, `Tz`, `TL`, and `Ts` in the forward CTM scan.
/// Pure code motion, generalized over which field is written. ~keep
fn apply_scan_text_state_f32(num_buf: &[f32; 6], num_count: usize, field: &mut f32) {
    if num_count >= 1 {
        *field = num_buf[num_count - 1];
    }
}

/// Handles the `Tr` operator during the forward CTM scan. Pure code motion. ~keep
fn apply_scan_render_mode(num_buf: &[f32; 6], num_count: usize, render_mode: &mut u8) {
    if num_count >= 1 {
        *render_mode = num_buf[num_count - 1] as u8;
    }
}

/// Handles the `TD` operator during the forward CTM scan: `tx ty TD` also
/// sets the leading to `-ty` (ISO 32000-1 §9.4.2). Pure code motion. ~keep
fn apply_scan_leading_from_td(num_buf: &[f32; 6], num_count: usize, leading: &mut f32) {
    if num_count >= 2 {
        *leading = -num_buf[num_count - 1];
    }
}

/// Handles the `rg` operator during the forward CTM scan. Pure code motion. ~keep
fn apply_scan_fill_rgb(
    num_buf: &[f32; 6],
    num_count: usize,
    fill_op: &mut Option<Operator>,
    fill_space: &mut Option<String>,
) {
    if num_count >= 3 {
        *fill_op = Some(Operator::SetFillRgb {
            r: num_buf[num_count - 3],
            g: num_buf[num_count - 2],
            b: num_buf[num_count - 1],
        });
    }
    *fill_space = None;
}

/// Handles the `g` operator during the forward CTM scan. Pure code motion. ~keep
fn apply_scan_fill_gray(
    num_buf: &[f32; 6],
    num_count: usize,
    fill_op: &mut Option<Operator>,
    fill_space: &mut Option<String>,
) {
    if num_count >= 1 {
        *fill_op = Some(Operator::SetFillGray {
            gray: num_buf[num_count - 1],
        });
    }
    *fill_space = None;
}

/// Handles the `k` operator during the forward CTM scan. Pure code motion. ~keep
fn apply_scan_fill_cmyk(
    num_buf: &[f32; 6],
    num_count: usize,
    fill_op: &mut Option<Operator>,
    fill_space: &mut Option<String>,
) {
    if num_count >= 4 {
        *fill_op = Some(Operator::SetFillCmyk {
            c: num_buf[num_count - 4],
            m: num_buf[num_count - 3],
            y: num_buf[num_count - 2],
            k: num_buf[num_count - 1],
        });
    }
    *fill_space = None;
}

/// Handles the `cs` operator during the forward CTM scan. `cs` resets the
/// fill color to the space's initial value, so any earlier color operator no
/// longer applies. Pure code motion. ~keep
fn apply_scan_fill_colorspace(
    last_name: &mut Option<String>,
    fill_space: &mut Option<String>,
    fill_op: &mut Option<Operator>,
) {
    if let Some(name) = last_name.take() {
        *fill_space = Some(name);
        *fill_op = None;
    }
}

/// Handles the `sc` operator during the forward CTM scan. Operands are the
/// TAIL of the rolling buffer, like every other arm. The buffer holds 6, so
/// at 7+ operands the leading ones have rotated away and a full count no
/// longer proves a complete window; replay only the 1-4 component
/// device-sized spaces rather than a truncated, plausible-looking wrong
/// color. Pure code motion. ~keep
fn apply_scan_fill_sc(num_buf: &[f32; 6], num_count: usize, fill_op: &mut Option<Operator>) {
    if (1..=4).contains(&num_count) {
        *fill_op = Some(Operator::SetFillColor {
            components: num_buf[..num_count].to_vec(),
        });
    }
}

/// Handles the `scn` operator during the forward CTM scan. Pure code motion. ~keep
fn apply_scan_fill_scn(
    num_buf: &[f32; 6],
    num_count: usize,
    last_name: &mut Option<String>,
    fill_op: &mut Option<Operator>,
) {
    if (1..=4).contains(&num_count) || (num_count == 0 && last_name.is_some()) {
        *fill_op = Some(Operator::SetFillColorN {
            components: num_buf[..num_count].to_vec(),
            name: last_name.take().map(Box::new),
        });
    }
}

/// Skips a `<<...>>` dictionary or `<...>` hex string during the forward CTM
/// scan, honoring nested dictionaries so a `>>` inside a nested dict doesn't
/// terminate the outer one early. Returns the new scan position and whether a
/// dictionary was skipped (which resets the pending numeric-operand buffer,
/// same as the original inline logic). Pure code motion out of
/// forward_scan_ctm. ~keep
fn skip_dict_or_hex_string_scan(data: &[u8], start: usize) -> (usize, bool) {
    let len = data.len();
    let mut i = start;
    if i + 1 < len && data[i + 1] == b'<' {
        i += 2;
        let mut depth = 1u32;
        while i + 1 < len && depth > 0 {
            if data[i] == b'<' && data[i + 1] == b'<' {
                depth += 1;
                i += 2;
            } else if data[i] == b'>' && data[i + 1] == b'>' {
                depth -= 1;
                i += 2;
            } else {
                i += 1;
            }
        }
        (i, true)
    } else {
        i += 1;
        while i < len && data[i] != b'>' {
            i += 1;
        }
        if i < len {
            i += 1;
        }
        (i, false)
    }
}

/// Result of pre-scanning a content stream for text regions.
#[derive(Debug)]
enum PrescanResult {
    /// No text operators in the stream.
    Empty,
    /// Text regions found with complete CTM context from backward scan alone.
    Regions(Vec<(usize, usize)>),
    /// Text regions with graphics state from a forward CTM scan.
    ///
    /// Used when the backward scan hit the 4KB limit for at least one BT,
    /// meaning outer CTM context may be missing. Each region is paired with
    /// the full graphics state at its BT/Do position.
    RegionsWithCtm {
        regions: Vec<(usize, usize)>,
        /// One entry per region, in the same order as `regions`.
        region_states: Vec<PrescanState>,
    },
}

impl PrescanResult {
    /// Get the regions from any variant (for testing).
    #[cfg(test)]
    fn regions(&self) -> &[(usize, usize)] {
        match self {
            PrescanResult::Empty => &[],
            PrescanResult::Regions(r) => r,
            PrescanResult::RegionsWithCtm { regions, .. } => regions,
        }
    }
}

/// SIMD-accelerated pre-scan to identify text-bearing regions in a content stream.
///
/// Finds BT/Do operators via memchr, then for each one determines the region
/// boundaries and required graphics state. When the backward scan can capture
/// all enclosing `q`/`cm` context within 4KB, returns [`PrescanResult::Regions`].
/// Otherwise, runs a lightweight forward CTM scan to capture the full graphics
/// state and returns [`PrescanResult::RegionsWithCtm`].
///
/// # Arguments
///
/// * `data` - Raw content stream bytes
///
/// # Returns
///
/// Returns `None` if the forward scan fails, signaling the caller to fall back
/// to full stream parsing.
fn prescan_text_regions(data: &[u8]) -> Option<PrescanResult> {
    let len = data.len();
    let mut text_positions: Vec<usize> = Vec::new();
    let mut offset = 0;

    loop {
        match memchr::memchr2(b'B', b'D', &data[offset..]) {
            None => break,
            Some(rel_pos) => {
                let pos = offset + rel_pos;
                offset = pos + 1;

                let is_match = match data[pos] {
                    b'B' => is_bounded_two_byte_operator(data, pos, b'T'),
                    b'D' => is_bounded_two_byte_operator(data, pos, b'o'),
                    _ => false,
                };
                if is_match {
                    text_positions.push(pos);
                }
            }
        }
    }

    if text_positions.is_empty() {
        return Some(PrescanResult::Empty);
    }

    // Drop Do positions when Do dominates BT (chart/figure graphics that
    // would merge prescan regions across the entire stream). ~keep
    let bt_count = text_positions
        .iter()
        .filter(|&&p| p + 1 < len && data[p] == b'B')
        .count();
    let do_count = text_positions.len() - bt_count;
    if do_count > 50 && do_count > bt_count * 10 {
        text_positions.retain(|&p| p + 1 < len && data[p] == b'B');
        if text_positions.is_empty() {
            return Some(PrescanResult::Empty);
        }
    }

    let mut regions: Vec<(usize, usize)> = Vec::new();
    let mut needs_forward_ctm = false;

    for &tp in &text_positions {
        let (region_start, hit_limit) = find_region_start(data, tp);

        if hit_limit {
            needs_forward_ctm = true;
        }

        let region_end = if data[tp] == b'B' {
            find_matching_et(data, tp + 2).unwrap_or(len)
        } else {
            tp + 2
        };

        let end = region_end.min(len);
        regions.push((region_start, end));
    }

    if regions.is_empty() {
        return Some(PrescanResult::Empty);
    }

    if needs_forward_ctm {
        return build_ctm_regions(data, &text_positions);
    }

    regions.sort_unstable_by_key(|r| r.0);
    let mut merged: Vec<(usize, usize)> = Vec::new();
    for r in regions {
        if let Some(last) = merged.last_mut()
            && r.0 <= last.1
        {
            last.1 = last.1.max(r.1);
            continue;
        }
        merged.push(r);
    }

    Some(PrescanResult::Regions(merged))
}

/// Whether `b` delimits a token boundary (whitespace or a PDF delimiter
/// byte), used to confirm a memchr hit is a real operator rather than a
/// substring inside another token. Pure code motion out of
/// prescan_text_regions. ~keep
fn is_boundary(b: u8) -> bool {
    b.is_ascii_whitespace() || matches!(b, b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%')
}

/// Checks whether `data[pos..pos+2]` is a boundary-delimited two-byte
/// operator (e.g. `BT` or `Do`), used to filter memchr matches down to real
/// operators. Pure code motion out of prescan_text_regions's memchr loop,
/// generalized over the second byte. ~keep
fn is_bounded_two_byte_operator(data: &[u8], pos: usize, second_byte: u8) -> bool {
    let len = data.len();
    if pos + 1 >= len || data[pos + 1] != second_byte {
        return false;
    }
    let before_ok = pos == 0 || is_boundary(data[pos - 1]);
    let after_ok = pos + 2 >= len || is_boundary(data[pos + 2]);
    before_ok && after_ok
}

/// Builds CTM-aware text regions once the backward scan couldn't capture
/// full enclosing graphics-state context for at least one BT/Do position. At
/// least one BT was too far from the start of the stream for the backward
/// scan to capture all enclosing CTM context, so this runs a lightweight
/// forward scan to get the full graphics state at each BT/Do position.
///
/// Regions start at the BT/Do position itself (not the backward-scanned q)
/// to avoid q/Q nesting issues with the SaveState/RestoreState wrapping. The
/// forward scan also tracks font state so BT blocks that inherit fonts from
/// prior state get the correct Tf injected. Extends each region to include
/// preceding BDC/BMC and following EMC so that marked-content operators are
/// preserved in tagged PDFs, then merges overlapping regions (keeping the
/// state of the first). Pure code motion out of prescan_text_regions. ~keep
fn build_ctm_regions(data: &[u8], text_positions: &[usize]) -> Option<PrescanResult> {
    let len = data.len();
    let states = forward_scan_ctm(data, text_positions)?;

    let mut ctm_regions: Vec<(usize, usize)> = Vec::new();
    for &tp in text_positions {
        let region_start = find_preceding_marked_content(data, tp);
        let region_end = if data[tp] == b'B' {
            let et_end = find_matching_et(data, tp + 2).unwrap_or(len);
            find_following_emc(data, et_end)
        } else {
            tp + 2
        };
        ctm_regions.push((region_start, region_end.min(len)));
    }

    let mut indexed: Vec<((usize, usize), PrescanState)> = ctm_regions.into_iter().zip(states).collect();
    indexed.sort_by_key(|&(r, _)| r.0);

    let mut merged: Vec<(usize, usize)> = Vec::new();
    let mut merged_states: Vec<PrescanState> = Vec::new();

    for (r, state) in indexed {
        if let Some(last) = merged.last_mut()
            && r.0 <= last.1
        {
            last.1 = last.1.max(r.1);
            continue; // Merged — keep the state from the first region ~keep
        }
        merged.push(r);
        merged_states.push(state);
    }

    Some(PrescanResult::RegionsWithCtm {
        regions: merged,
        region_states: merged_states,
    })
}

/// Scan backwards from `pos` to find the start of the graphics state context.
///
/// Looks for the nearest unmatched `q` operator within a 4KB window,
/// handling nested `q`/`Q` pairs.
///
/// # Arguments
///
/// * `data` - Full content stream bytes
/// * `pos` - Byte offset to scan backwards from (typically a BT/Do position)
///
/// # Returns
///
/// `(offset, hit_limit)` where `offset` is the position of the nearest
/// unmatched `q` (or `pos` if none found), and `hit_limit` is true if the
/// 4KB scan window didn't reach the beginning of the data. When `hit_limit`
/// is true, there may be additional enclosing `q`/`cm` operators beyond
/// the window that affect the CTM.
fn find_region_start(data: &[u8], pos: usize) -> (usize, bool) {
    // Simple backward scan: find the nearest line that starts with 'q' or
    // the beginning of data. We limit backward scan to 4KB for performance. ~keep
    let scan_start = pos.saturating_sub(4096);
    let region = &data[scan_start..pos];

    let mut q_depth: i32 = 0;
    let mut best_q_pos = pos;
    // A `cm` operator encountered while `q_depth == 0` sits outside any
    // enclosing `q`/`Q` bracket this scan can see — a common CAD-exporter
    // pattern is a single top-level `cm` right at the start of the stream
    // (no `q` at all) establishing a scale/offset for everything after it.
    // This function only tracks `q`/`Q`, so on its own it would report
    // `best_q_pos == pos` (nothing found) and, if the scan also reached
    // byte 0, `hit_limit == false` — silently discarding that transform. ~keep
    let mut saw_top_level_cm = false;
    let mut i = region.len();

    while i > 0 {
        i -= 1;
        let b = region[i];

        if b == b'q' || b == b'Q' {
            let abs_pos = scan_start + i;
            if is_bounded_q_or_capital_q(region, i) {
                if b == b'Q' {
                    q_depth += 1;
                } else if q_depth > 0 {
                    q_depth -= 1;
                } else {
                    best_q_pos = abs_pos;
                    break;
                }
            }
        } else if b == b'm' && i > 0 && region[i - 1] == b'c' && q_depth == 0 {
            let tok_start = i - 1;
            if is_bounded_top_level_cm(region, tok_start, i) {
                saw_top_level_cm = true;
            }
        }
    }

    // We can only guarantee complete CTM context if we scanned all the way
    // to the beginning of the data. Even if we found an unmatched 'q' within
    // 4KB, there may be additional enclosing q/cm operators before the scan
    // window that establish scaling transforms we're missing. A top-level
    // `cm` visible in the scanned window but outside any `q`/`Q` this
    // function tracks needs the same fallback, even when the scan reached
    // byte 0 of the stream. ~keep
    let hit_limit = scan_start > 0 || saw_top_level_cm;
    (best_q_pos, hit_limit)
}

/// Boundary check for a `q`/`Q` operator candidate during find_region_start's
/// backward scan: the operator must be delimited by whitespace/closing-delim
/// bytes before and whitespace/opening-delim/numeric bytes after, since a
/// bare `q`/`Q` byte can appear inside a longer token. Pure code motion out
/// of find_region_start. ~keep
fn is_bounded_q_or_capital_q(region: &[u8], i: usize) -> bool {
    let before_ok = i == 0 || {
        let prev = region[i - 1];
        prev.is_ascii_whitespace() || matches!(prev, b')' | b'>' | b']')
    };
    let after_ok = i + 1 >= region.len() || {
        let next = region[i + 1];
        next.is_ascii_whitespace()
            || matches!(next, b'(' | b'<' | b'[' | b'/' | b'%')
            || next.is_ascii_digit()
            || next == b'-'
            || next == b'.'
    };
    before_ok && after_ok
}

/// Boundary check for a top-level `cm` operator candidate during
/// find_region_start's backward scan. Pure code motion out of
/// find_region_start. ~keep
fn is_bounded_top_level_cm(region: &[u8], tok_start: usize, i: usize) -> bool {
    let before_ok = tok_start == 0 || {
        let prev = region[tok_start - 1];
        prev.is_ascii_whitespace() || matches!(prev, b')' | b'>' | b']')
    };
    let after_ok = i + 1 >= region.len() || {
        let next = region[i + 1];
        next.is_ascii_whitespace() || next == b'%'
    };
    before_ok && after_ok
}

/// Scan backward from `pos` to find any immediately preceding BDC/BMC operator.
/// Returns the position of the BDC/BMC if found within 256 bytes, otherwise `pos`.
fn find_preceding_marked_content(data: &[u8], pos: usize) -> usize {
    let scan_start = pos.saturating_sub(256);
    let mut i = pos;
    while i > scan_start {
        i -= 1;
        if data[i] == b'C' && i >= 2 && data[i - 2] == b'B' && (data[i - 1] == b'D' || data[i - 1] == b'M') {
            let op_start = i - 2;
            let before_ok = op_start == 0 || !data[op_start - 1].is_ascii_alphanumeric();
            let after_ok = i + 1 >= data.len() || !data[i + 1].is_ascii_alphanumeric();
            if before_ok && after_ok {
                // For BDC, scan further back to include the tag and properties dict
                // e.g., "/Span << /MCID 0 >> BDC"
                // Find the start of the line/command ~keep
                let mut line_start = op_start;
                while line_start > scan_start && data[line_start - 1] != b'\n' && data[line_start - 1] != b'\r' {
                    line_start -= 1;
                }
                return line_start;
            }
        }
    }
    pos
}

/// Scan forward from `pos` to find any immediately following EMC operator.
/// Returns the position after the EMC if found within 256 bytes, otherwise `pos`.
fn find_following_emc(data: &[u8], pos: usize) -> usize {
    let scan_end = (pos + 256).min(data.len());
    let mut i = pos;
    while i + 2 < scan_end {
        if data[i] == b'E' && data[i + 1] == b'M' && data[i + 2] == b'C' {
            let before_ok = i == 0 || data[i - 1].is_ascii_whitespace();
            let after_ok = i + 3 >= data.len() || data[i + 3].is_ascii_whitespace();
            if before_ok && after_ok {
                return i + 3;
            }
        }
        i += 1;
    }
    pos
}

/// Find the position after matching "ET" for a BT starting at `start`.
fn find_matching_et(data: &[u8], start: usize) -> Option<usize> {
    let mut offset = start;
    let len = data.len();
    loop {
        let rel = memchr::memchr(b'E', &data[offset..])?;
        let pos = offset + rel;
        offset = pos + 1;
        if pos + 1 < len && data[pos + 1] == b'T' {
            let before_ok = pos == 0
                || data[pos - 1].is_ascii_whitespace()
                || matches!(data[pos - 1], b')' | b'>' | b']' | b'}' | b'/' | b'%');
            let after_ok = pos + 2 >= len || {
                let next = data[pos + 2];
                next.is_ascii_whitespace() || matches!(next, b'(' | b'<' | b'[' | b'/' | b'%')
            };
            if before_ok && after_ok {
                return Some(pos + 2);
            }
        }
    }
}

/// Runs the per-region CTM/font/text-state injection for
/// `PrescanResult::RegionsWithCtm`: wraps each region in
/// `SaveState`/`RestoreState` so state from one region doesn't leak into the
/// next, and injects the graphics state the forward scan captured before
/// parsing the region itself. Pure code motion out of
/// `parse_and_execute_text_only`'s `RegionsWithCtm` match arm. ~keep
fn run_regions_with_ctm<F>(
    data: &[u8],
    regions: &[(usize, usize)],
    region_states: &[PrescanState],
    handler: &mut F,
) -> Result<()>
where
    F: FnMut(Operator) -> Result<()>,
{
    for (i, (start, end)) in regions.iter().enumerate() {
        let state = &region_states[i];
        let (a, b, c, d, e, f) = state.ctm;
        handler(Operator::SaveState)?;
        handler(Operator::Cm { a, b, c, d, e, f })?;
        // Re-inject inherited fill color: glyph color reads the
        // fill state, which the region wrap would otherwise
        // reset to black. Space first so components are
        // interpreted in the right color space. ~keep
        if let Some(ref name) = state.fill_space {
            handler(Operator::SetFillColorSpace { name: name.clone() })?;
        }
        if let Some(ref op) = state.fill_op {
            handler(op.clone())?;
        }
        // Inject font state if the forward scan tracked one.
        // This handles BT blocks that inherit Tf from a prior
        // scope instead of setting their own. ~keep
        if let Some((ref font_name, font_size)) = state.font {
            handler(Operator::Tf {
                font: font_name.clone(),
                size: font_size,
            })?;
        }
        // Re-inject inherited text state the same way. These
        // parameters persist across BT/ET, so a region may rely
        // on a value set by an earlier region — which the
        // SaveState/RestoreState wrap would otherwise reset.
        // Only non-default values are injected, keeping the
        // operator stream unchanged for the common case. ~keep
        let ts = &state.text;
        if ts.char_spacing != 0.0 {
            handler(Operator::Tc {
                char_space: ts.char_spacing,
            })?;
        }
        if ts.word_spacing != 0.0 {
            handler(Operator::Tw {
                word_space: ts.word_spacing,
            })?;
        }
        if ts.horiz_scaling != 100.0 {
            handler(Operator::Tz {
                scale: ts.horiz_scaling,
            })?;
        }
        if ts.leading != 0.0 {
            handler(Operator::TL { leading: ts.leading })?;
        }
        if ts.rise != 0.0 {
            handler(Operator::Ts { rise: ts.rise })?;
        }
        if ts.render_mode != 0 {
            handler(Operator::Tr { render: ts.render_mode })?;
        }
        parse_region_text_only(&data[*start..*end], handler)?;
        handler(Operator::RestoreState)?;
    }
    Ok(())
}

/// Streaming text-only parser: parse operators and call handler immediately.
///
/// Each operator is passed to `handler` as soon as it's parsed, eliminating the
/// intermediate operator vector (which can be 16MB+ for graphics-heavy pages).
///
/// Not equivalent to [`parse_content_stream_text_only`]: for streams over 256KB
/// this parser prescans for text regions and reconstructs the graphics state
/// around each one by injecting synthetic `q`/`cm`/`Tf`/`Q` operators, while the
/// vec-building parser walks the whole stream sequentially. The two can emit the
/// same text operators and still leave the handler with a different CTM, and
/// they truncate differently on hostile bytes between text regions. Extraction
/// must route every mode through this parser (see
/// `TextExtractor::run_content_stream`).
pub fn parse_and_execute_text_only<F>(data: &[u8], mut handler: F) -> Result<()>
where
    F: FnMut(Operator) -> Result<()>,
{
    // For large streams (>256KB), use SIMD pre-scan to identify text regions. ~keep
    // This avoids byte-by-byte scanning of megabytes of path/color operators.
    if data.len() > 256 * 1024
        && let Some(result) = prescan_text_regions(data)
    {
        match result {
            PrescanResult::Empty => return Ok(()),
            PrescanResult::Regions(regions) => {
                for (start, end) in &regions {
                    parse_region_text_only(&data[*start..*end], &mut handler)?;
                }
                return Ok(());
            }
            PrescanResult::RegionsWithCtm { regions, region_states } => {
                return run_regions_with_ctm(data, &regions, &region_states, &mut handler);
            }
        }
    }

    let mut input = data;
    let mut consecutive_errors: usize = 0;
    let mut inside_text = false;
    let mut op_count: usize = 0;

    while !input.is_empty() {
        // Skip leading whitespace (inline — both fast parser and scan_graphics
        // also handle whitespace, but this covers the initial entry and error
        // recovery paths without nom overhead). ~keep
        while !input.is_empty() && input[0].is_ascii_whitespace() {
            input = &input[1..];
        }
        if input.is_empty() {
            break;
        }

        if op_count >= effective_max_operators() {
            push_operator_cap_warning();
            break;
        }

        if inside_text {
            // Try fast path first (3-5x faster for common text operators) ~keep
            match parse_inside_text_operator(input) {
                Ok((rest, op)) => {
                    if matches!(op, Operator::EndText) {
                        inside_text = false;
                    }
                    handler(op)?;
                    op_count += 1;
                    input = rest;
                    consecutive_errors = 0;
                }
                Err(()) => {
                    consecutive_errors += 1;
                    if consecutive_errors >= MAX_CONSECUTIVE_ERRORS {
                        tracing::warn!(
                            count = MAX_CONSECUTIVE_ERRORS,
                            bytes = input.len(),
                            "content stream had too many consecutive parse errors; bailing out"
                        );
                        break;
                    }
                    if input.len() > 1 {
                        input = &input[1..];
                    } else {
                        break;
                    }
                }
            }
        } else {
            let should_continue = handle_graphics_region_step(
                &mut input,
                &mut consecutive_errors,
                &mut op_count,
                &mut inside_text,
                &mut handler,
            )?;
            if !should_continue {
                break;
            }
        }
    }

    Ok(())
}

/// Parses one operator inside a `BT`/`ET` text block, trying the fast
/// text-operator parser first and falling back to the full nom-based parser
/// so both paths agree on operator semantics. Pure code motion out of the
/// `inside_text` branch shared by `parse_and_execute_text_only` and
/// `parse_region_text_only` (both parsers already required the two paths to
/// produce identical results for text operators). ~keep
fn parse_inside_text_operator(input: &[u8]) -> std::result::Result<(&[u8], Operator), ()> {
    if let Some((rest, op)) = parse_text_operator_fast(input) {
        return Ok((rest, op));
    }
    parse_operator_with_operands(input).map_err(|_| ())
}

/// Replays operators from `deferred_start` up to (but not including)
/// `trigger_start`, routing each through `emit` and counting them into
/// `op_count`. Used when the graphics-region scanner defers CTM-affecting
/// operators (q/cm/Q) ahead of a BT/BI/Do trigger it hasn't consumed yet.
/// Pure code motion, generalized over the emit callback so both the
/// streaming (`FnMut(Operator) -> Result<()>`) and marked-content-tracking
/// callers can share it. ~keep
fn replay_deferred_operators_with(
    deferred_start: &[u8],
    trigger_start: &[u8],
    op_count: &mut usize,
    mut emit: impl FnMut(Operator) -> Result<()>,
) -> Result<()> {
    let mut remaining = deferred_start;
    while remaining.len() > trigger_start.len() {
        match parse_operator_with_operands(remaining) {
            Ok((rest2, op)) => {
                emit(op)?;
                *op_count += 1;
                remaining = rest2;
            }
            Err(_) => {
                if remaining.len() > 1 {
                    remaining = &remaining[1..];
                } else {
                    break;
                }
            }
        }
    }
    Ok(())
}

/// Handles one iteration of `parse_and_execute_text_only`'s outer loop while
/// outside a `BT`/`ET` text block: dispatches on `scan_graphics_region` and
/// invokes `handler` for any operator it should emit. Returns `false` when
/// the caller's loop should stop (end of data or too many consecutive
/// errors). Pure code motion out of its `else` branch. ~keep
fn handle_graphics_region_step<F>(
    input: &mut &[u8],
    consecutive_errors: &mut usize,
    op_count: &mut usize,
    inside_text: &mut bool,
    handler: &mut F,
) -> Result<bool>
where
    F: FnMut(Operator) -> Result<()>,
{
    match scan_graphics_region(input, consecutive_errors) {
        ScanResult::EndOfData => Ok(false),
        ScanResult::FoundBT { rest } => {
            handler(Operator::BeginText)?;
            *op_count += 1;
            *input = rest;
            *inside_text = true;
            Ok(true)
        }
        ScanResult::InlineImage { rest } => {
            match parse_inline_image(rest) {
                Ok((rest2, _)) => *input = rest2,
                Err(_) => *input = rest,
            }
            Ok(true)
        }
        ScanResult::NeedFullParse {
            operand_start,
            after_op,
        } => {
            match parse_operator_with_operands(operand_start) {
                Ok((rest2, op)) => {
                    handler(op)?;
                    *op_count += 1;
                    *input = rest2;
                }
                Err(_) => *input = after_op,
            }
            Ok(true)
        }
        ScanResult::DeferredThenText {
            deferred_start,
            trigger_start,
        } => {
            replay_deferred_operators_with(deferred_start, trigger_start, op_count, &mut *handler)?;
            *input = trigger_start;
            *consecutive_errors = 0;
            Ok(true)
        }
        ScanResult::SimpleOp { op, rest } => {
            handler(op)?;
            *op_count += 1;
            *input = rest;
            Ok(true)
        }
        ScanResult::TooManyErrors { remaining } => {
            tracing::warn!(
                count = MAX_CONSECUTIVE_ERRORS,
                bytes = remaining.len(),
                "content stream had too many consecutive parse errors; bailing out"
            );
            Ok(false)
        }
    }
}

/// Parse a sub-region of a content stream for text operators.
/// Used by the pre-scan path to parse only identified text-bearing regions.
///
/// Each region is bracketed in `SaveState`/`RestoreState` by the caller, and
/// CTM/font state is injected fresh per region — but the marked-content
/// stack gets no such treatment, and a scope this region *opens* (BDC/BMC)
/// without also *closing* (EMC) before the region ends would otherwise leak
/// into every subsequent region indefinitely. This is fatal when
/// the tag is `/PlacedPDF`: `is_content_suppressed` stays true forever,
/// discarding all remaining text on the page. Tracks this region's own
/// BDC/BMC vs EMC balance and emits synthetic `EndMarkedContent` operators
/// for anything still open when the region ends — closing only what THIS
/// region itself opened, never touching scopes open when it started (the
/// safer half of a full cross-region marked-content-state carry, which
/// isn't needed for the reported failure mode: it can only ever end scopes
/// this region opened, never fabricate a scope's original tag/properties).
fn parse_region_text_only<F>(data: &[u8], handler: &mut F) -> Result<()>
where
    F: FnMut(Operator) -> Result<()>,
{
    let mut input = data;
    let mut consecutive_errors: usize = 0;
    let mut inside_text = false;
    let mut op_count: usize = 0;
    let mut mc_depth: u32 = 0;
    let mut call = |op: Operator, handler: &mut F| -> Result<()> {
        match &op {
            Operator::BeginMarkedContent { .. } | Operator::BeginMarkedContentDict { .. } => {
                mc_depth += 1;
            }
            Operator::EndMarkedContent => {
                mc_depth = mc_depth.saturating_sub(1);
            }
            _ => {}
        }
        handler(op)
    };

    while !input.is_empty() {
        while !input.is_empty() && input[0].is_ascii_whitespace() {
            input = &input[1..];
        }
        if input.is_empty() {
            break;
        }

        if op_count >= effective_max_operators() {
            break;
        }

        if inside_text {
            match parse_inside_text_operator(input) {
                Ok((rest, op)) => {
                    if matches!(op, Operator::EndText) {
                        inside_text = false;
                    }
                    call(op, handler)?;
                    op_count += 1;
                    input = rest;
                    consecutive_errors = 0;
                }
                Err(()) => {
                    consecutive_errors += 1;
                    if consecutive_errors >= MAX_CONSECUTIVE_ERRORS {
                        break;
                    }
                    if input.len() > 1 {
                        input = &input[1..];
                    } else {
                        break;
                    }
                }
            }
        } else {
            let should_continue = handle_graphics_region_step_tracked(
                &mut input,
                &mut consecutive_errors,
                &mut op_count,
                &mut inside_text,
                handler,
                &mut call,
            )?;
            if !should_continue {
                break;
            }
        }
    }

    // Balance any marked-content scope this region opened but did not
    // close (BDC/BMC with no matching EMC before the region ended) — see
    // the function doc comment. Closing only what this region itself
    // opened is always safe: it never affects a scope that was already
    // open when the region started. ~keep
    for _ in 0..mc_depth {
        handler(Operator::EndMarkedContent)?;
    }

    Ok(())
}

/// Replays operators from `deferred_start` up to (but not including)
/// `trigger_start`, routing each through `call` (which tracks marked-content
/// depth) and `handler`. Used when the graphics-region scanner defers
/// CTM-affecting operators (q/cm/Q) ahead of a BT/BI/Do trigger it hasn't
/// consumed yet. Pure code motion out of `parse_region_text_only`. ~keep
fn replay_deferred_operators_tracked<F>(
    deferred_start: &[u8],
    trigger_start: &[u8],
    op_count: &mut usize,
    handler: &mut F,
    call: &mut impl FnMut(Operator, &mut F) -> Result<()>,
) -> Result<()>
where
    F: FnMut(Operator) -> Result<()>,
{
    let mut remaining = deferred_start;
    while remaining.len() > trigger_start.len() {
        match parse_operator_with_operands(remaining) {
            Ok((rest2, op)) => {
                call(op, handler)?;
                *op_count += 1;
                remaining = rest2;
            }
            Err(_) => {
                if remaining.len() > 1 {
                    remaining = &remaining[1..];
                } else {
                    break;
                }
            }
        }
    }
    Ok(())
}

/// Handles one iteration of `parse_region_text_only`'s outer loop while
/// outside a `BT`/`ET` text block: dispatches on `scan_graphics_region` and
/// routes any operator it should emit through `call` (which tracks
/// marked-content depth). Returns `false` when the caller's loop should stop
/// (end of data or too many consecutive errors). Pure code motion out of its
/// `else` branch. ~keep
fn handle_graphics_region_step_tracked<F>(
    input: &mut &[u8],
    consecutive_errors: &mut usize,
    op_count: &mut usize,
    inside_text: &mut bool,
    handler: &mut F,
    call: &mut impl FnMut(Operator, &mut F) -> Result<()>,
) -> Result<bool>
where
    F: FnMut(Operator) -> Result<()>,
{
    match scan_graphics_region(input, consecutive_errors) {
        ScanResult::EndOfData => Ok(false),
        ScanResult::FoundBT { rest } => {
            call(Operator::BeginText, handler)?;
            *op_count += 1;
            *input = rest;
            *inside_text = true;
            Ok(true)
        }
        ScanResult::InlineImage { rest } => {
            match parse_inline_image(rest) {
                Ok((rest2, _)) => *input = rest2,
                Err(_) => *input = rest,
            }
            Ok(true)
        }
        ScanResult::NeedFullParse {
            operand_start,
            after_op,
        } => {
            match parse_operator_with_operands(operand_start) {
                Ok((rest2, op)) => {
                    call(op, handler)?;
                    *op_count += 1;
                    *input = rest2;
                }
                Err(_) => *input = after_op,
            }
            Ok(true)
        }
        ScanResult::DeferredThenText {
            deferred_start,
            trigger_start,
        } => {
            replay_deferred_operators_tracked(deferred_start, trigger_start, op_count, handler, call)?;
            *input = trigger_start;
            *consecutive_errors = 0;
            Ok(true)
        }
        ScanResult::SimpleOp { op, rest } => {
            call(op, handler)?;
            *op_count += 1;
            *input = rest;
            Ok(true)
        }
        ScanResult::TooManyErrors { .. } => Ok(false),
    }
}

/// Image-only content stream parser: skips BT/ET text blocks entirely.
///
/// Only fully parses operators relevant to image extraction:
/// `cm`, `q`, `Q`, `Do`, `BI`/`ID`/`EI` (inline images).
/// All text and graphics drawing operators are skipped.
pub fn parse_content_stream_images_only(data: &[u8]) -> Result<Vec<Operator>> {
    let mut operators = Vec::with_capacity(256);
    let mut input = data;
    let mut consecutive_errors: usize = 0;
    let mut inside_text = false;

    while !input.is_empty() {
        if let Ok((rest, _)) = multispace0::<&[u8], nom::error::Error<&[u8]>>.parse(input) {
            input = rest;
        }
        if input.is_empty() {
            break;
        }

        if operators.len() >= effective_max_operators() {
            break;
        }

        if inside_text {
            match scan_to_et(input) {
                Some(rest) => {
                    input = rest;
                    inside_text = false;
                    consecutive_errors = 0;
                }
                None => break,
            }
        } else {
            match scan_graphics_region(input, &mut consecutive_errors) {
                ScanResult::EndOfData => break,
                ScanResult::FoundBT { rest } => {
                    input = rest;
                    inside_text = true;
                }
                ScanResult::InlineImage { rest } => match parse_inline_image(rest) {
                    Ok((rest2, op)) => {
                        operators.push(op);
                        input = rest2;
                    }
                    Err(_) => input = rest,
                },
                ScanResult::NeedFullParse {
                    operand_start,
                    after_op,
                } => match parse_operator_with_operands(operand_start) {
                    Ok((rest2, op)) => {
                        operators.push(op);
                        input = rest2;
                    }
                    Err(_) => input = after_op,
                },
                ScanResult::DeferredThenText {
                    deferred_start,
                    trigger_start,
                } => {
                    replay_deferred_operators(deferred_start, trigger_start, &mut operators);
                    input = trigger_start;
                    consecutive_errors = 0;
                }
                ScanResult::SimpleOp { op, rest } => {
                    operators.push(op);
                    input = rest;
                }
                ScanResult::TooManyErrors { .. } => break,
            }
        }
    }

    Ok(operators)
}

/// Skip forward until we find the ET operator (end text).
/// Returns the remaining input after ET, or None if not found.
fn scan_to_et(data: &[u8]) -> Option<&[u8]> {
    let mut i = 0;
    while i + 1 < data.len() {
        if data[i] == b'E' && data[i + 1] == b'T' {
            let before_ok = i == 0 || data[i - 1].is_ascii_whitespace() || data[i - 1] == b')' || data[i - 1] == b'>';
            let after_ok = i + 2 >= data.len() || data[i + 2].is_ascii_whitespace() || data[i + 2] == b'%';
            if before_ok && after_ok {
                return Some(&data[i + 2..]);
            }
        }
        if data[i] == b'(' {
            i = skip_literal_string_for_et_scan(data, i);
            continue;
        }
        if data[i] == b'<' && (i + 1 >= data.len() || data[i + 1] != b'<') {
            i = skip_hex_string_for_et_scan(data, i);
            continue;
        }
        i += 1;
    }
    None
}

/// Skips a literal string starting at `(` in `scan_to_et`'s ET search,
/// honoring nested parens and backslash escapes so a `)` inside a string
/// doesn't terminate it early. Pure code motion out of `scan_to_et`. ~keep
fn skip_literal_string_for_et_scan(data: &[u8], start: usize) -> usize {
    let mut i = start + 1;
    let mut depth = 1;
    while i < data.len() && depth > 0 {
        match data[i] {
            b'(' => depth += 1,
            b')' => depth -= 1,
            b'\\' => i += 1,
            _ => {}
        }
        i += 1;
    }
    i
}

/// Skips a hex string starting at `<` (not `<<`) in `scan_to_et`'s ET
/// search. Pure code motion out of `scan_to_et`. ~keep
fn skip_hex_string_for_et_scan(data: &[u8], start: usize) -> usize {
    let mut i = start + 1;
    while i < data.len() && data[i] != b'>' {
        i += 1;
    }
    if i < data.len() {
        i += 1;
    }
    i
}

/// Parse a single operator with its operands.
///
/// Returns the remaining input and the parsed operator.
///
/// Uses `SmallVec<[Object; 6]>` for the operand buffer to avoid heap
/// allocation for the common case (most PDF operators have 0-6 operands).
/// Only spills to the heap for rare operators with more than 6 operands.
fn parse_operator_with_operands(input: &[u8]) -> IResult<&[u8], Operator> {
    let mut operands: SmallVec<[Object; 6]> = SmallVec::new();
    let mut remaining = input;

    loop {
        let (inp, _) = multispace0.parse(remaining)?;
        remaining = inp;

        if remaining.is_empty() {
            return Err(nom::Err::Error(nom::error::Error::new(
                remaining,
                nom::error::ErrorKind::Eof,
            )));
        }

        if is_operator_start(remaining[0]) {
            let (rest, op_name) = parse_operator_name(remaining)?;

            if op_name == "BI" {
                // Parse inline image: BI <dict entries> ID <binary data> EI ~keep
                return parse_inline_image(rest);
            }

            let op = build_operator(op_name, operands);
            return Ok((rest, op));
        }

        let (inp, obj) = parse_object(remaining)?;
        operands.push(obj);
        remaining = inp;
    }
}

/// Check if a byte could start an operator name.
///
/// Operators start with alphabetic characters or special characters like ' or "
fn is_operator_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'\'' || byte == b'"' || byte == b'*'
}

/// Parse an operator name from the input.
///
/// Operator names are typically 1-3 letter alphabetic sequences, but can include:
/// - Single quote (') for the Quote operator
/// - Double quote (") for the DoubleQuote operator
/// - Star (*) for T* operator
fn parse_operator_name(input: &[u8]) -> IResult<&[u8], &str> {
    let (input, name_bytes) =
        take_while1(|c: u8| c.is_ascii_alphanumeric() || c == b'\'' || c == b'"' || c == b'*').parse(input)?;

    let name = std::str::from_utf8(name_bytes)
        .map_err(|_| nom::Err::Error(nom::error::Error::new(input, nom::error::ErrorKind::Char)))?;

    Ok((input, name))
}

/// Build an operator from its name and operands.
///
/// This function converts the raw operator name and operands into a strongly-typed
/// Operator enum variant. It handles type conversions and validates operand counts.
///
/// Accepts `SmallVec<[Object; 6]>` to avoid heap allocation for the common case
/// (most PDF operators have 0-6 operands). The operands are consumed and dropped
/// after extraction.
/// Builds `Td`/`TD`/`Tm`/`T*` operators. `None` if `name` isn't one of these.
/// Pure code motion out of `build_operator`'s dispatch match. ~keep
fn build_text_positioning_operator(name: &str, operands: &[Object]) -> Option<Operator> {
    Some(match name {
        "Td" => {
            let tx = get_number(operands, 0).unwrap_or(0.0);
            let ty = get_number(operands, 1).unwrap_or(0.0);
            Operator::Td { tx, ty }
        }
        "TD" => {
            let tx = get_number(operands, 0).unwrap_or(0.0);
            let ty = get_number(operands, 1).unwrap_or(0.0);
            Operator::TD { tx, ty }
        }
        "Tm" => {
            let a = get_number(operands, 0).unwrap_or(1.0);
            let b = get_number(operands, 1).unwrap_or(0.0);
            let c = get_number(operands, 2).unwrap_or(0.0);
            let d = get_number(operands, 3).unwrap_or(1.0);
            let e = get_number(operands, 4).unwrap_or(0.0);
            let f = get_number(operands, 5).unwrap_or(0.0);
            Operator::Tm { a, b, c, d, e, f }
        }
        "T*" => Operator::TStar,
        _ => return None,
    })
}

/// Builds `Tj`/`TJ`/`'`/`"` operators. `None` if `name` isn't one of these.
/// Pure code motion out of `build_operator`'s dispatch match. ~keep
fn build_text_showing_operator(name: &str, operands: &[Object]) -> Option<Operator> {
    Some(match name {
        "Tj" => {
            let text = get_string(operands, 0).unwrap_or_default();
            Operator::Tj { text }
        }
        "TJ" => {
            let elements = if let Some(array) = get_array(operands, 0) {
                array
                    .iter()
                    .filter_map(|obj| match obj {
                        Object::String(s) => Some(TextElement::String(s.clone())),
                        Object::Integer(i) => Some(TextElement::Offset(*i as f32)),
                        Object::Real(r) => Some(TextElement::Offset(*r as f32)),
                        _ => None,
                    })
                    .collect()
            } else {
                Vec::new()
            };
            Operator::TJ { array: elements }
        }
        "'" => {
            let text = get_string(operands, 0).unwrap_or_default();
            Operator::Quote { text }
        }
        "\"" => {
            let word_space = get_number(operands, 0).unwrap_or(0.0);
            let char_space = get_number(operands, 1).unwrap_or(0.0);
            let text = get_string(operands, 2).unwrap_or_default();
            Operator::DoubleQuote {
                word_space,
                char_space,
                text,
            }
        }
        _ => return None,
    })
}

/// Builds `Tc`/`Tw`/`Tz`/`TL`/`Tf`/`Tr`/`Ts` operators. `None` if `name`
/// isn't one of these. Pure code motion out of `build_operator`'s dispatch
/// match. ~keep
fn build_text_state_operator(name: &str, operands: &[Object]) -> Option<Operator> {
    Some(match name {
        "Tc" => {
            let char_space = get_number(operands, 0).unwrap_or(0.0);
            Operator::Tc { char_space }
        }
        "Tw" => {
            let word_space = get_number(operands, 0).unwrap_or(0.0);
            Operator::Tw { word_space }
        }
        "Tz" => {
            let scale = get_number(operands, 0).unwrap_or(100.0);
            Operator::Tz { scale }
        }
        "TL" => {
            let leading = get_number(operands, 0).unwrap_or(0.0);
            Operator::TL { leading }
        }
        "Tf" => {
            let font = get_name(operands, 0).unwrap_or("").to_string();
            let size = get_number(operands, 1).unwrap_or(12.0);
            Operator::Tf { font, size }
        }
        "Tr" => {
            let render = get_integer(operands, 0).unwrap_or(0) as u8;
            Operator::Tr { render }
        }
        "Ts" => {
            let rise = get_number(operands, 0).unwrap_or(0.0);
            Operator::Ts { rise }
        }
        _ => return None,
    })
}

/// Builds `q`/`Q`/`cm` operators. `None` if `name` isn't one of these. Pure
/// code motion out of `build_operator`'s dispatch match. ~keep
fn build_state_stack_operator(name: &str, operands: &[Object]) -> Option<Operator> {
    Some(match name {
        "q" => Operator::SaveState,
        "Q" => Operator::RestoreState,
        "cm" => {
            let a = get_number(operands, 0).unwrap_or(1.0);
            let b = get_number(operands, 1).unwrap_or(0.0);
            let c = get_number(operands, 2).unwrap_or(0.0);
            let d = get_number(operands, 3).unwrap_or(1.0);
            let e = get_number(operands, 4).unwrap_or(0.0);
            let f = get_number(operands, 5).unwrap_or(0.0);
            Operator::Cm { a, b, c, d, e, f }
        }
        _ => return None,
    })
}

/// Builds `rg`/`RG`/`g`/`G`/`k`/`K` operators. `None` if `name` isn't one of
/// these. Pure code motion out of `build_operator`'s dispatch match. ~keep
fn build_simple_color_operator(name: &str, operands: &[Object]) -> Option<Operator> {
    Some(match name {
        "rg" => {
            let r = get_number(operands, 0).unwrap_or(0.0);
            let g = get_number(operands, 1).unwrap_or(0.0);
            let b = get_number(operands, 2).unwrap_or(0.0);
            Operator::SetFillRgb { r, g, b }
        }
        "RG" => {
            let r = get_number(operands, 0).unwrap_or(0.0);
            let g = get_number(operands, 1).unwrap_or(0.0);
            let b = get_number(operands, 2).unwrap_or(0.0);
            Operator::SetStrokeRgb { r, g, b }
        }
        "g" => {
            let gray = get_number(operands, 0).unwrap_or(0.0);
            Operator::SetFillGray { gray }
        }
        "G" => {
            let gray = get_number(operands, 0).unwrap_or(0.0);
            Operator::SetStrokeGray { gray }
        }
        "k" => {
            let c = get_number(operands, 0).unwrap_or(0.0);
            let m = get_number(operands, 1).unwrap_or(0.0);
            let y = get_number(operands, 2).unwrap_or(0.0);
            let k = get_number(operands, 3).unwrap_or(0.0);
            Operator::SetFillCmyk { c, m, y, k }
        }
        "K" => {
            let c = get_number(operands, 0).unwrap_or(0.0);
            let m = get_number(operands, 1).unwrap_or(0.0);
            let y = get_number(operands, 2).unwrap_or(0.0);
            let k = get_number(operands, 3).unwrap_or(0.0);
            Operator::SetStrokeCmyk { c, m, y, k }
        }
        _ => return None,
    })
}

/// Builds `cs`/`CS`/`sc`/`SC`/`scn`/`SCN` operators. `None` if `name` isn't
/// one of these. Pure code motion out of `build_operator`'s dispatch
/// match. ~keep
fn build_indexed_color_operator(name: &str, operands: &[Object]) -> Option<Operator> {
    Some(match name {
        "cs" => {
            let name = get_name(operands, 0).unwrap_or("DeviceGray").to_string();
            Operator::SetFillColorSpace { name }
        }
        "CS" => {
            let name = get_name(operands, 0).unwrap_or("DeviceGray").to_string();
            Operator::SetStrokeColorSpace { name }
        }
        "sc" => {
            // Set fill color: c1 c2 ... cn sc
            // Number of components depends on current color space ~keep
            let components: Vec<f32> = operands
                .iter()
                .filter_map(|obj| match obj {
                    Object::Real(r) => Some(*r as f32),
                    Object::Integer(i) => Some(*i as f32),
                    _ => None,
                })
                .collect();
            Operator::SetFillColor { components }
        }
        "SC" => {
            let components: Vec<f32> = operands
                .iter()
                .filter_map(|obj| match obj {
                    Object::Real(r) => Some(*r as f32),
                    Object::Integer(i) => Some(*i as f32),
                    _ => None,
                })
                .collect();
            Operator::SetStrokeColor { components }
        }
        "scn" => {
            // Set fill color with pattern support: c1 c2 ... cn [name] scn
            // Last operand may be a name for pattern color spaces ~keep
            let name = if let Some(Object::Name(n)) = operands.last() {
                Some(n.clone())
            } else {
                None
            };
            let components: Vec<f32> = operands
                .iter()
                .filter_map(|obj| match obj {
                    Object::Real(r) => Some(*r as f32),
                    Object::Integer(i) => Some(*i as f32),
                    Object::Name(_) => None,
                    _ => None,
                })
                .collect();
            Operator::SetFillColorN {
                components,
                name: name.map(Box::new),
            }
        }
        "SCN" => {
            // Set stroke color with pattern support: c1 c2 ... cn [name] SCN ~keep
            let name = if let Some(Object::Name(n)) = operands.last() {
                Some(n.clone())
            } else {
                None
            };
            let components: Vec<f32> = operands
                .iter()
                .filter_map(|obj| match obj {
                    Object::Real(r) => Some(*r as f32),
                    Object::Integer(i) => Some(*i as f32),
                    Object::Name(_) => None,
                    _ => None,
                })
                .collect();
            Operator::SetStrokeColorN {
                components,
                name: name.map(Box::new),
            }
        }
        _ => return None,
    })
}

/// Builds `BT`/`ET`/`Do` operators. `None` if `name` isn't one of these.
/// Pure code motion out of `build_operator`'s dispatch match. ~keep
fn build_page_object_operator(name: &str, operands: &[Object]) -> Option<Operator> {
    Some(match name {
        "BT" => Operator::BeginText,
        "ET" => Operator::EndText,
        "Do" => {
            // Per ISO 32000-1:2008 §7.8.2, operands "shall immediately precede"
            // their operator and none "shall be left over" once it executes.
            // Do takes exactly one operand (the XObject name); if stray operands
            // accumulated ahead of it (e.g. a dropped/malformed `cm` before this
            // `Do`), the name is still the one immediately preceding the operator,
            // i.e. the LAST element, not necessarily the first. ~keep
            let name = get_name(operands, operands.len().saturating_sub(1))
                .unwrap_or("")
                .to_string();
            Operator::Do { name }
        }
        _ => return None,
    })
}

/// Builds path-construction and path-painting operators (`m`,`l`,`c`,`v`,
/// `y`,`h`,`re`,`S`,`f`,`F`,`f*`,`b`,`b*`,`B`,`B*`,`n`,`W`,`W*`). `None` if
/// `name` isn't one of these. Pure code motion out of `build_operator`'s
/// dispatch match. ~keep
fn build_path_operator(name: &str, operands: &[Object]) -> Option<Operator> {
    Some(match name {
        "m" => {
            let x = get_number(operands, 0).unwrap_or(0.0);
            let y = get_number(operands, 1).unwrap_or(0.0);
            Operator::MoveTo { x, y }
        }
        "l" => {
            let x = get_number(operands, 0).unwrap_or(0.0);
            let y = get_number(operands, 1).unwrap_or(0.0);
            Operator::LineTo { x, y }
        }
        "c" => {
            let x1 = get_number(operands, 0).unwrap_or(0.0);
            let y1 = get_number(operands, 1).unwrap_or(0.0);
            let x2 = get_number(operands, 2).unwrap_or(0.0);
            let y2 = get_number(operands, 3).unwrap_or(0.0);
            let x3 = get_number(operands, 4).unwrap_or(0.0);
            let y3 = get_number(operands, 5).unwrap_or(0.0);
            Operator::CurveTo { x1, y1, x2, y2, x3, y3 }
        }
        "v" => {
            // Bézier curve (first control point = current point) ~keep
            let x2 = get_number(operands, 0).unwrap_or(0.0);
            let y2 = get_number(operands, 1).unwrap_or(0.0);
            let x3 = get_number(operands, 2).unwrap_or(0.0);
            let y3 = get_number(operands, 3).unwrap_or(0.0);
            Operator::CurveToV { x2, y2, x3, y3 }
        }
        "y" => {
            // Bézier curve (second control point = end point) ~keep
            let x1 = get_number(operands, 0).unwrap_or(0.0);
            let y1 = get_number(operands, 1).unwrap_or(0.0);
            let x3 = get_number(operands, 2).unwrap_or(0.0);
            let y3 = get_number(operands, 3).unwrap_or(0.0);
            Operator::CurveToY { x1, y1, x3, y3 }
        }
        "h" => Operator::ClosePath,
        "re" => {
            let x = get_number(operands, 0).unwrap_or(0.0);
            let y = get_number(operands, 1).unwrap_or(0.0);
            let width = get_number(operands, 2).unwrap_or(0.0);
            let height = get_number(operands, 3).unwrap_or(0.0);
            Operator::Rectangle { x, y, width, height }
        }
        "S" => Operator::Stroke,
        "f" | "F" => Operator::Fill,
        // ~keep
        "f*" => Operator::FillEvenOdd,
        "b" => Operator::CloseFillStroke,
        "b*" => Operator::CloseFillStrokeEvenOdd,
        "B" => Operator::FillStroke,
        "B*" => Operator::FillStrokeEvenOdd,
        "n" => Operator::EndPath,
        "W" => Operator::ClipNonZero,
        "W*" => Operator::ClipEvenOdd,
        _ => return None,
    })
}

/// Builds `w`/`d`/`J`/`j`/`M`/`ri`/`i` operators. `None` if `name` isn't one
/// of these. Pure code motion out of `build_operator`'s dispatch match. ~keep
fn build_line_style_operator(name: &str, operands: &[Object]) -> Option<Operator> {
    Some(match name {
        "w" => {
            let width = get_number(operands, 0).unwrap_or(1.0);
            Operator::SetLineWidth { width }
        }
        "d" => {
            // d operator: array phase
            // Example: [3 2] 0 d means 3 on, 2 off, starting at phase 0 ~keep
            let array = if let Some(Object::Array(arr)) = operands.first() {
                arr.iter()
                    .filter_map(|obj| match obj {
                        Object::Integer(i) => Some(*i as f32),
                        Object::Real(r) => Some(*r as f32),
                        _ => None,
                    })
                    .collect()
            } else {
                Vec::new()
            };
            let phase = get_number(operands, 1).unwrap_or(0.0);
            Operator::SetDash { array, phase }
        }
        "J" => {
            // J operator: integer J
            // 0=butt cap, 1=round cap, 2=projecting square cap ~keep
            let cap_style = get_integer(operands, 0).unwrap_or(0) as u8;
            Operator::SetLineCap { cap_style }
        }
        "j" => {
            // j operator: integer j
            // 0=miter join, 1=round join, 2=bevel join ~keep
            let join_style = get_integer(operands, 0).unwrap_or(0) as u8;
            Operator::SetLineJoin { join_style }
        }
        "M" => {
            // M operator: number M
            // Miter limit (ratio of miter length to line width) ~keep
            let limit = get_number(operands, 0).unwrap_or(10.0);
            Operator::SetMiterLimit { limit }
        }
        "ri" => {
            // ri operator: name ri
            // Rendering intent: /AbsoluteColorimetric, /RelativeColorimetric, /Saturation, or /Perceptual
            // ~keep
            let intent = get_name(operands, 0).unwrap_or("RelativeColorimetric").to_string();
            Operator::SetRenderingIntent { intent }
        }
        "i" => {
            // i operator: number i
            // Flatness tolerance (0-100) ~keep
            let tolerance = get_number(operands, 0).unwrap_or(1.0);
            Operator::SetFlatness { tolerance }
        }
        _ => return None,
    })
}

/// Builds `gs`/`sh` operators. `None` if `name` isn't one of these. Pure
/// code motion out of `build_operator`'s dispatch match. ~keep
fn build_ext_state_operator(name: &str, operands: &[Object]) -> Option<Operator> {
    Some(match name {
        "gs" => {
            let dict_name = get_name(operands, 0).unwrap_or("").to_string();
            Operator::SetExtGState { dict_name }
        }
        "sh" => {
            let name = get_name(operands, 0).unwrap_or("").to_string();
            Operator::PaintShading { name }
        }
        _ => return None,
    })
}

/// Builds `BMC`/`BDC`/`EMC` marked content operators (for tagged PDF
/// structure), ISO 32000-1:2008 §14.6. `None` if `name` isn't one of these.
/// Pure code motion out of `build_operator`'s dispatch match. ~keep
fn build_marked_content_operator(name: &str, operands: &[Object]) -> Option<Operator> {
    Some(match name {
        "BMC" => {
            let tag = get_name(operands, 0).unwrap_or("").to_string();
            Operator::BeginMarkedContent { tag }
        }
        "BDC" => {
            // Begin marked content with properties: tag properties BDC
            // properties can be a dictionary or a name (reference to /Properties resource) ~keep
            let tag = get_name(operands, 0).unwrap_or("").to_string();
            let properties = Box::new(operands.get(1).cloned().unwrap_or(Object::Null));
            Operator::BeginMarkedContentDict { tag, properties }
        }
        "EMC" => Operator::EndMarkedContent,
        _ => return None,
    })
}

fn build_operator(name: &str, operands: SmallVec<[Object; 6]>) -> Operator {
    if let Some(op) = build_text_positioning_operator(name, &operands) {
        return op;
    }
    if let Some(op) = build_text_showing_operator(name, &operands) {
        return op;
    }
    if let Some(op) = build_text_state_operator(name, &operands) {
        return op;
    }
    if let Some(op) = build_state_stack_operator(name, &operands) {
        return op;
    }
    if let Some(op) = build_simple_color_operator(name, &operands) {
        return op;
    }
    if let Some(op) = build_indexed_color_operator(name, &operands) {
        return op;
    }
    if let Some(op) = build_page_object_operator(name, &operands) {
        return op;
    }
    if let Some(op) = build_path_operator(name, &operands) {
        return op;
    }
    if let Some(op) = build_line_style_operator(name, &operands) {
        return op;
    }
    if let Some(op) = build_ext_state_operator(name, &operands) {
        return op;
    }
    if let Some(op) = build_marked_content_operator(name, &operands) {
        return op;
    }

    // Unknown operator — convert SmallVec to Vec for the boxed storage.
    // This path is rare (only for unrecognized operators), so the
    // conversion cost is negligible. ~keep
    Operator::Other {
        name: name.to_string(),
        operands: Box::new(operands.into_vec()),
    }
}

fn get_number(operands: &[Object], index: usize) -> Option<f32> {
    operands.get(index).and_then(|obj| match obj {
        Object::Integer(i) => Some(*i as f32),
        Object::Real(r) => Some(*r as f32),
        _ => None,
    })
}

fn get_integer(operands: &[Object], index: usize) -> Option<i64> {
    operands.get(index).and_then(|obj| obj.as_integer())
}

fn get_string(operands: &[Object], index: usize) -> Option<Vec<u8>> {
    operands.get(index).and_then(|obj| obj.as_string().map(|s| s.to_vec()))
}

fn get_name(operands: &[Object], index: usize) -> Option<&str> {
    operands.get(index).and_then(|obj| obj.as_name())
}

fn get_array(operands: &[Object], index: usize) -> Option<&Vec<Object>> {
    operands.get(index).and_then(|obj| obj.as_array())
}

/// Parse an inline image sequence (BI...ID...EI).
///
/// PDF Spec: ISO 32000-1:2008, Section 8.9.7 - Inline Images
///
/// Inline images have the format:
/// BI <key value> <key value> ... ID <binary data> EI
///
/// The dictionary uses abbreviated keys:
/// - W: Width
/// - H: Height
/// - CS: ColorSpace
/// - BPC: BitsPerComponent
/// - F: Filter
/// - DP: DecodeParms
/// - I: Interpolate
///
/// The challenge is finding the EI operator in the binary data, as the bytes
/// for "EI" could appear in the image data itself. Per spec, EI must be:
/// - Preceded by whitespace (space, tab, CR, LF)
/// - Followed by whitespace or end of stream
fn parse_inline_image(input: &[u8]) -> IResult<&[u8], Operator> {
    let mut dict = HashMap::new();
    let mut remaining = input;

    loop {
        let (inp, _) = multispace0.parse(remaining)?;
        remaining = inp;

        if remaining.is_empty() {
            return Err(nom::Err::Error(nom::error::Error::new(
                remaining,
                nom::error::ErrorKind::Eof,
            )));
        }

        if remaining.len() >= 2
            && &remaining[0..2] == b"ID"
            && (remaining.len() == 2 || remaining.len() > 2 && is_whitespace(remaining[2]))
        {
            remaining = &remaining[2..];
            break;
        }

        let (inp, key_obj) = parse_object(remaining)?;
        remaining = inp;

        let (inp, _) = multispace0.parse(remaining)?;
        remaining = inp;

        let (inp, value_obj) = parse_object(remaining)?;
        remaining = inp;

        if let Some(key_str) = key_obj.as_name() {
            dict.insert(key_str.to_string(), value_obj);
        }
    }

    let (inp, _) = multispace0.parse(remaining)?;
    remaining = inp;

    let (_inp, data) = find_and_extract_image_data(remaining)?;
    let data_len = data.len();
    remaining = &remaining[data_len..];

    let (_inp, ei_pos) = find_ei_operator(remaining)?;
    remaining = &remaining[ei_pos + 2..];

    Ok((
        remaining,
        Operator::InlineImage {
            dict: Box::new(dict),
            data,
        },
    ))
}

/// Find the EI operator in the input, which must be preceded by whitespace.
/// Returns the position of the whitespace before EI.
fn find_ei_operator(input: &[u8]) -> IResult<&[u8], usize> {
    for i in 0..input.len().saturating_sub(2) {
        if is_whitespace(input[i])
            && input.len() > i + 2
            && &input[i + 1..i + 3] == b"EI"
            && (input.len() == i + 3 || is_whitespace_or_delimiter(input[i + 3]))
        {
            return Ok((input, i));
        }
    }

    Err(nom::Err::Error(nom::error::Error::new(
        input,
        nom::error::ErrorKind::Tag,
    )))
}

/// Extract image data up to (but not including) the whitespace before EI.
fn find_and_extract_image_data(input: &[u8]) -> IResult<&[u8], Vec<u8>> {
    let (inp, ei_pos) = find_ei_operator(input)?;
    Ok((inp, input[..ei_pos].to_vec()))
}

/// Check if a byte is whitespace (null, tab, LF, FF, CR, space — PDF spec Table 1).
fn is_whitespace(byte: u8) -> bool {
    matches!(byte, b'\x00' | b'\t' | b'\r' | b'\n' | b'\x0C' | b' ')
}

/// Check if a byte is whitespace or a PDF delimiter.
fn is_whitespace_or_delimiter(byte: u8) -> bool {
    is_whitespace(byte)
        || matches!(
            byte,
            b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%'
        )
}

// ── Nom-based operand skippers (test-only, superseded by raw variants) ───── ~keep

#[cfg(test)]
fn skip_operand_token(input: &[u8]) -> IResult<&[u8], ()> {
    if input.is_empty() {
        return Err(nom::Err::Error(nom::error::Error::new(
            input,
            nom::error::ErrorKind::Eof,
        )));
    }

    match input[0] {
        b'0'..=b'9' | b'.' | b'+' | b'-' => skip_number(input),
        b'(' => skip_literal_string(input),
        b'<' if input.len() > 1 && input[1] == b'<' => skip_dict(input),
        b'<' => skip_hex_string(input),
        b'/' => skip_name(input),
        b'[' => skip_array(input),
        _ => Err(nom::Err::Error(nom::error::Error::new(
            input,
            nom::error::ErrorKind::Char,
        ))),
    }
}

#[cfg(test)]
fn skip_number(input: &[u8]) -> IResult<&[u8], ()> {
    let mut i = 0;
    if i < input.len() && (input[i] == b'+' || input[i] == b'-') {
        i += 1;
    }
    let start = i;
    let mut has_dot = false;
    while i < input.len() {
        if input[i].is_ascii_digit() {
            i += 1;
        } else if input[i] == b'.' && !has_dot {
            has_dot = true;
            i += 1;
        } else {
            break;
        }
    }
    if i == start {
        return Err(nom::Err::Error(nom::error::Error::new(
            input,
            nom::error::ErrorKind::Digit,
        )));
    }
    Ok((&input[i..], ()))
}

#[cfg(test)]
fn skip_literal_string(input: &[u8]) -> IResult<&[u8], ()> {
    let mut i = 1;
    let mut depth: u32 = 1;
    while i < input.len() && depth > 0 {
        match input[i] {
            b'\\' if i + 1 < input.len() => i += 2,
            b'(' => {
                depth += 1;
                i += 1;
            }
            b')' => {
                depth -= 1;
                i += 1;
            }
            _ => i += 1,
        }
    }
    if depth != 0 {
        return Err(nom::Err::Error(nom::error::Error::new(
            input,
            nom::error::ErrorKind::Char,
        )));
    }
    Ok((&input[i..], ()))
}

#[cfg(test)]
fn skip_hex_string(input: &[u8]) -> IResult<&[u8], ()> {
    let mut i = 1;
    while i < input.len() {
        if input[i] == b'>' {
            return Ok((&input[i + 1..], ()));
        }
        i += 1;
    }
    Err(nom::Err::Error(nom::error::Error::new(
        input,
        nom::error::ErrorKind::Char,
    )))
}

#[cfg(test)]
fn skip_name(input: &[u8]) -> IResult<&[u8], ()> {
    let mut i = 1;
    while i < input.len() && !is_whitespace_or_delimiter(input[i]) {
        i += 1;
    }
    Ok((&input[i..], ()))
}

#[cfg(test)]
fn skip_array(input: &[u8]) -> IResult<&[u8], ()> {
    let mut i = 1;
    let mut depth: u32 = 1;
    while i < input.len() && depth > 0 {
        match input[i] {
            b'[' => {
                depth += 1;
                i += 1;
            }
            b']' => {
                depth -= 1;
                i += 1;
            }
            b'(' => {
                i += 1;
                let mut str_depth: u32 = 1;
                while i < input.len() && str_depth > 0 {
                    match input[i] {
                        b'\\' if i + 1 < input.len() => i += 2,
                        b'(' => {
                            str_depth += 1;
                            i += 1;
                        }
                        b')' => {
                            str_depth -= 1;
                            i += 1;
                        }
                        _ => i += 1,
                    }
                }
            }
            b'<' if i + 1 < input.len() && input[i + 1] == b'<' => {
                i += 2;
                let mut dict_depth: u32 = 1;
                while i + 1 < input.len() && dict_depth > 0 {
                    if input[i] == b'<' && input[i + 1] == b'<' {
                        dict_depth += 1;
                        i += 2;
                    } else if input[i] == b'>' && input[i + 1] == b'>' {
                        dict_depth -= 1;
                        i += 2;
                    } else {
                        i += 1;
                    }
                }
            }
            b'<' => {
                i += 1;
                while i < input.len() && input[i] != b'>' {
                    i += 1;
                }
                if i < input.len() {
                    i += 1;
                }
            }
            _ => i += 1,
        }
    }
    if depth != 0 {
        return Err(nom::Err::Error(nom::error::Error::new(
            input,
            nom::error::ErrorKind::Char,
        )));
    }
    Ok((&input[i..], ()))
}

#[cfg(test)]
fn skip_dict(input: &[u8]) -> IResult<&[u8], ()> {
    let mut i = 2;
    let mut depth: u32 = 1;
    while i < input.len() && depth > 0 {
        if i + 1 < input.len() && input[i] == b'<' && input[i + 1] == b'<' {
            depth += 1;
            i += 2;
        } else if i + 1 < input.len() && input[i] == b'>' && input[i + 1] == b'>' {
            depth -= 1;
            i += 2;
        } else if input[i] == b'(' {
            i += 1;
            let mut str_depth: u32 = 1;
            while i < input.len() && str_depth > 0 {
                match input[i] {
                    b'\\' if i + 1 < input.len() => i += 2,
                    b'(' => {
                        str_depth += 1;
                        i += 1;
                    }
                    b')' => {
                        str_depth -= 1;
                        i += 1;
                    }
                    _ => i += 1,
                }
            }
        } else if input[i] == b'<' {
            i += 1;
            while i < input.len() && input[i] != b'>' {
                i += 1;
            }
            if i < input.len() {
                i += 1;
            }
        } else {
            i += 1;
        }
    }
    if depth != 0 {
        return Err(nom::Err::Error(nom::error::Error::new(
            input,
            nom::error::ErrorKind::Char,
        )));
    }
    Ok((&input[i..], ()))
}

// ── Byte-level graphics region scanner ─────────────────────────────────────
//
// Replaces the nom-based operand loop in parse_content_stream_text_only with
// raw index arithmetic. >80% of bytes in graphics-heavy streams are digits,
// dots, and whitespace for path coordinates — a tight match loop processes
// these at near-memcpy speed vs per-operand nom IResult dispatch. ~keep

/// Result of scanning a graphics region (outside BT/ET).
enum ScanResult<'a> {
    /// All data consumed, no more operators.
    EndOfData,
    /// Found a BT operator; `rest` points past "BT".
    FoundBT { rest: &'a [u8] },
    /// Found an inline image (BI); `rest` points past "BI".
    InlineImage { rest: &'a [u8] },
    /// Found a non-skippable operator; caller should backtrack to
    /// `operand_start` for full parsing. `after_op` points past the operator
    /// name (used as fallback if full parse fails).
    NeedFullParse {
        operand_start: &'a [u8],
        after_op: &'a [u8],
    },
    /// Found a non-skippable operator (BT/BI/Do/etc.) inside a deferred q/cm
    /// block. `deferred_start` points to the first deferred `q` so the caller
    /// can full-parse the q/cm/Q sequence to preserve CTM. `trigger_start`
    /// points to the operand_start of the triggering operator so the caller
    /// resumes scanning there (the next scan_graphics_region call will
    /// immediately return the trigger via FoundBT / InlineImage / NeedFullParse).
    DeferredThenText {
        deferred_start: &'a [u8],
        trigger_start: &'a [u8],
    },
    /// A simple no-operand operator that can be emitted directly without
    /// nom parsing. Used for unmatched Q (RestoreGraphicsState) to avoid
    /// expensive full-parse fallback.
    SimpleOp { op: Operator, rest: &'a [u8] },
    /// Too many consecutive errors; remaining data is likely junk.
    TooManyErrors { remaining: &'a [u8] },
}

/// Parse N float operands from a raw byte slice.
/// Returns a fixed-size array. Returns None if not enough parseable numbers.
#[inline]
fn parse_floats<const N: usize>(data: &[u8]) -> Option<[f32; N]> {
    let s = std::str::from_utf8(data).ok()?;
    let mut iter = s.split_ascii_whitespace();
    let mut result = [0.0f32; N];
    for val in &mut result {
        *val = iter.next()?.parse::<f32>().ok()?;
    }
    Some(result)
}

/// Parse 6 float operands from a raw byte slice (for inline `cm` parsing).
/// Returns None if the slice doesn't contain exactly 6 parseable numbers.
#[inline]
fn parse_six_floats(data: &[u8]) -> Option<(f32, f32, f32, f32, f32, f32)> {
    let f = parse_floats::<6>(data)?;
    Some((f[0], f[1], f[2], f[3], f[4], f[5]))
}

/// Byte-level check for pure graphics operators that can be skipped during
/// text-only extraction. Equivalent to [`is_skippable_graphics_op`] but
/// operates on raw `&[u8]` without UTF-8 conversion.
///
/// Includes the colour operators (rg/RG/g/G/k/K/cs/CS/sc/SC/scn/SCN):
/// skipping them here is only sound when the caller also guarantees a
/// matching `Q` will revert any colour change before it can reach a `BT`
/// (the `deferred_depth > 0` block in `scan_graphics_region`). Do not use
/// this predicate to decide skippability outside a deferred q/Q scope -
/// see [`is_color_op_bytes`] for that case.
fn is_skippable_graphics_op_bytes(op: &[u8]) -> bool {
    matches!(
        op,
        b"m" | b"l"
            | b"c"
            | b"v"
            | b"y"
            | b"h"
            | b"re"
            | b"S"
            | b"s"
            | b"f"
            | b"F"
            | b"f*"
            | b"B"
            | b"B*"
            | b"b"
            | b"b*"
            | b"n"
            | b"W"
            | b"W*"
            | b"w"
            | b"J"
            | b"j"
            | b"M"
            | b"d"
            | b"i"
            | b"ri"
            | b"sh"
            | b"rg"
            | b"RG"
            | b"g"
            | b"G"
            | b"k"
            | b"K"
            | b"cs"
            | b"CS"
            | b"sc"
            | b"SC"
            | b"scn"
            | b"SCN"
    )
}

/// Byte-level check for operators that set persistent fill/stroke colour
/// state (rg/RG/g/G/k/K/cs/CS/sc/SC/scn/SCN).
///
/// Used at the *top level* of `scan_graphics_region` (deferred_depth == 0,
/// i.e. no enclosing unmatched `q`). Unlike pure path/paint/clip operators,
/// a colour change here is not reverted by anything before the next `BT` -
/// per ISO 32000-1:2008 SS8.4 the graphics state (including colour) persists
/// across BT/ET boundaries. Discarding it as "skippable" left GraphicsState
/// stuck at its default black whenever a document set fill colour before
/// opening the text object (a common pattern: BDC, colour, BT, Tf, Tm, Tj),
/// so text that should render in colour was extracted as black even though
/// the identical scn issued *inside* an already-open BT worked correctly.
fn is_color_op_bytes(op: &[u8]) -> bool {
    matches!(
        op,
        b"rg" | b"RG" | b"g" | b"G" | b"k" | b"K" | b"cs" | b"CS" | b"sc" | b"SC" | b"scn" | b"SCN"
    )
}

// ── Raw index-returning skip functions ─────────────────────────────────────
//
// Same logic as the nom-based skip_*() functions above, but return a new
// index position instead of IResult. On malformed input, Option variants
// return None so the caller can skip one byte (matching current error
// recovery). ~keep

fn skip_literal_string_raw(data: &[u8], mut i: usize) -> Option<usize> {
    i += 1;
    let mut depth: u32 = 1;
    while i < data.len() && depth > 0 {
        match data[i] {
            b'\\' if i + 1 < data.len() => i += 2,
            b'(' => {
                depth += 1;
                i += 1;
            }
            b')' => {
                depth -= 1;
                i += 1;
            }
            _ => i += 1,
        }
    }
    if depth == 0 { Some(i) } else { None }
}

fn skip_hex_string_raw(data: &[u8], mut i: usize) -> Option<usize> {
    i += 1;
    while i < data.len() {
        if data[i] == b'>' {
            return Some(i + 1);
        }
        i += 1;
    }
    None
}

#[inline]
fn skip_name_raw(data: &[u8], mut i: usize) -> usize {
    i += 1;
    while i < data.len() && !is_whitespace_or_delimiter(data[i]) {
        i += 1;
    }
    i
}

fn skip_array_raw(data: &[u8], i: usize) -> Option<usize> {
    let mut pos = i + 1;
    let mut depth: u32 = 1;
    while pos < data.len() && depth > 0 {
        match data[pos] {
            b'[' => {
                depth += 1;
                pos += 1;
            }
            b']' => {
                depth -= 1;
                pos += 1;
            }
            b'(' => {
                pos += 1;
                let mut str_depth: u32 = 1;
                while pos < data.len() && str_depth > 0 {
                    match data[pos] {
                        b'\\' if pos + 1 < data.len() => pos += 2,
                        b'(' => {
                            str_depth += 1;
                            pos += 1;
                        }
                        b')' => {
                            str_depth -= 1;
                            pos += 1;
                        }
                        _ => pos += 1,
                    }
                }
            }
            b'<' if pos + 1 < data.len() && data[pos + 1] == b'<' => {
                pos += 2;
                let mut dict_depth: u32 = 1;
                while pos + 1 < data.len() && dict_depth > 0 {
                    if data[pos] == b'<' && data[pos + 1] == b'<' {
                        dict_depth += 1;
                        pos += 2;
                    } else if data[pos] == b'>' && data[pos + 1] == b'>' {
                        dict_depth -= 1;
                        pos += 2;
                    } else {
                        pos += 1;
                    }
                }
            }
            b'<' => {
                pos += 1;
                while pos < data.len() && data[pos] != b'>' {
                    pos += 1;
                }
                if pos < data.len() {
                    pos += 1;
                }
            }
            _ => pos += 1,
        }
    }
    if depth == 0 { Some(pos) } else { None }
}

fn skip_dict_raw(data: &[u8], i: usize) -> Option<usize> {
    let mut pos = i + 2;
    let mut depth: u32 = 1;
    while pos < data.len() && depth > 0 {
        if pos + 1 < data.len() && data[pos] == b'<' && data[pos + 1] == b'<' {
            depth += 1;
            pos += 2;
        } else if pos + 1 < data.len() && data[pos] == b'>' && data[pos + 1] == b'>' {
            depth -= 1;
            pos += 2;
        } else if data[pos] == b'(' {
            pos += 1;
            let mut str_depth: u32 = 1;
            while pos < data.len() && str_depth > 0 {
                match data[pos] {
                    b'\\' if pos + 1 < data.len() => pos += 2,
                    b'(' => {
                        str_depth += 1;
                        pos += 1;
                    }
                    b')' => {
                        str_depth -= 1;
                        pos += 1;
                    }
                    _ => pos += 1,
                }
            }
        } else if data[pos] == b'<' {
            pos += 1;
            while pos < data.len() && data[pos] != b'>' {
                pos += 1;
            }
            if pos < data.len() {
                pos += 1;
            }
        } else {
            pos += 1;
        }
    }
    if depth == 0 { Some(pos) } else { None }
}

// ── Fast BT/ET block parser ────────────────────────────────────────────
//
// Hand-written byte-level parser for operators inside text blocks.
// Avoids the nom tokenizer overhead (~3-5x faster than parse_operator_with_operands)
// by parsing numbers inline, skipping indirect-reference lookahead, and matching
// operator names as raw bytes. ~keep

/// Operand type for the fast parser's operand stack.
/// Uses `f32` for numbers and `Vec<u8>` for strings to avoid full Object creation.
enum FastOperand {
    Number(f32),
    /// Raw string bytes (already decoded from literal or hex encoding)
    StringBytes(Vec<u8>),
    /// Name string (without leading `/`)
    Name(String),
    /// Array of TextElements (for TJ operator)
    TextArray(Vec<TextElement>),
}

/// Parse a float directly from bytes. Returns (value, bytes_consumed).
#[inline]
fn parse_float_fast(data: &[u8]) -> Option<(f32, usize)> {
    let mut i = 0;
    let negative = if i < data.len() && (data[i] == b'-' || data[i] == b'+') {
        let neg = data[i] == b'-';
        i += 1;
        neg
    } else {
        false
    };

    let start = i;
    let mut int_part: f64 = 0.0;
    while i < data.len() && data[i].is_ascii_digit() {
        int_part = int_part * 10.0 + (data[i] - b'0') as f64;
        i += 1;
    }

    let mut frac_part: f64 = 0.0;
    let mut frac_scale: f64 = 1.0;
    if i < data.len() && data[i] == b'.' {
        i += 1;
        while i < data.len() && data[i].is_ascii_digit() {
            frac_part = frac_part * 10.0 + (data[i] - b'0') as f64;
            frac_scale *= 10.0;
            i += 1;
        }
    }

    if i == start {
        return None;
    }

    let value = int_part + frac_part / frac_scale;
    let value = if negative { -value } else { value };
    Some((value as f32, i))
}

/// Parse a literal string `(...)` from bytes. Returns (decoded_bytes, position_after_close_paren).
#[inline]
fn parse_literal_string_fast(data: &[u8], start: usize) -> Option<(Vec<u8>, usize)> {
    let mut i = start + 1;
    let mut depth: u32 = 1;

    // Fast path: scan for simple strings without escapes or nested parens.
    // Most PDF strings are simple ASCII text like "(Hello)" or single chars like "(A)". ~keep
    let scan_start = i;
    while i < data.len() {
        match data[i] {
            b')' => {
                return Some((data[scan_start..i].to_vec(), i + 1));
            }
            b'\\' | b'(' => break,
            _ => i += 1,
        }
    }

    i = scan_start;
    let mut result = Vec::new();
    while i < data.len() && depth > 0 {
        match data[i] {
            b'\\' if i + 1 < data.len() => {
                i = parse_string_escape(data, i, &mut result);
            }
            b'(' => {
                depth += 1;
                result.push(b'(');
                i += 1;
            }
            b')' => {
                depth -= 1;
                if depth > 0 {
                    result.push(b')');
                }
                i += 1;
            }
            _ => {
                result.push(data[i]);
                i += 1;
            }
        }
    }
    if depth == 0 { Some((result, i)) } else { None }
}

/// Parses one backslash escape sequence in a literal string, starting at the
/// backslash position `i` (with `i + 1 < data.len()` guaranteed by the
/// caller). Pushes the decoded byte(s) — zero for line-continuation escapes —
/// into `result` and returns the index just past the escape. Pure code
/// motion out of `parse_literal_string_fast`'s main scan loop. ~keep
fn parse_string_escape(data: &[u8], i: usize, result: &mut Vec<u8>) -> usize {
    match data[i + 1] {
        b'n' => {
            result.push(b'\n');
            i + 2
        }
        b'r' => {
            result.push(b'\r');
            i + 2
        }
        b't' => {
            result.push(b'\t');
            i + 2
        }
        b'b' => {
            result.push(0x08);
            i + 2
        }
        b'f' => {
            result.push(0x0C);
            i + 2
        }
        b'(' => {
            result.push(b'(');
            i + 2
        }
        b')' => {
            result.push(b')');
            i + 2
        }
        b'\\' => {
            result.push(b'\\');
            i + 2
        }
        b'0'..=b'7' => {
            let (byte, j) = parse_octal_escape(data, i + 1);
            result.push(byte);
            j
        }
        b'\r' => {
            let mut i = i + 2;
            if i < data.len() && data[i] == b'\n' {
                i += 1;
            }
            i
        }
        b'\n' => i + 2,
        _ => {
            result.push(data[i + 1]);
            i + 2
        }
    }
}

/// Parses a `\ddd` octal escape (1-3 octal digits) starting at
/// `first_digit_at` in a literal-string escape sequence, returning the
/// decoded byte and the index just past the digits consumed. Pure code
/// motion out of `parse_literal_string_fast`'s escape handling. ~keep
fn parse_octal_escape(data: &[u8], first_digit_at: usize) -> (u8, usize) {
    let mut octal: u32 = (data[first_digit_at] - b'0') as u32;
    let mut j = first_digit_at + 1;
    for _ in 0..2 {
        if j < data.len() && (b'0'..=b'7').contains(&data[j]) {
            octal = octal * 8 + (data[j] - b'0') as u32;
            j += 1;
        } else {
            break;
        }
    }
    ((octal & 0xFF) as u8, j)
}

/// Parse a hex string `<...>` from bytes. Returns (decoded_bytes, position_after_close_angle).
#[inline]
fn parse_hex_string_fast(data: &[u8], start: usize) -> Option<(Vec<u8>, usize)> {
    let mut i = start + 1;
    let mut result = Vec::new();
    let mut high_nibble: Option<u8> = None;
    while i < data.len() {
        let b = data[i];
        if b == b'>' {
            // If odd number of hex digits, append 0 to make final byte ~keep
            if let Some(h) = high_nibble {
                result.push(h << 4);
            }
            return Some((result, i + 1));
        }
        if let Some(nibble) = hex_nibble(b) {
            match high_nibble {
                None => high_nibble = Some(nibble),
                Some(h) => {
                    result.push((h << 4) | nibble);
                    high_nibble = None;
                }
            }
        }
        i += 1;
    }
    None
}

#[inline]
fn hex_nibble(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Parse a TJ array `[...]` from bytes. Returns (elements, position_after_close_bracket).
fn parse_tj_array_fast(data: &[u8], start: usize) -> Option<(Vec<TextElement>, usize)> {
    let mut i = start + 1;
    let mut elements = Vec::new();
    loop {
        while i < data.len() && is_whitespace(data[i]) {
            i += 1;
        }
        if i >= data.len() {
            return None;
        }

        match data[i] {
            b']' => return Some((elements, i + 1)),
            b'(' => {
                if let Some((bytes, end)) = parse_literal_string_fast(data, i) {
                    elements.push(TextElement::String(bytes));
                    i = end;
                } else {
                    return None;
                }
            }
            b'<' => {
                if let Some((bytes, end)) = parse_hex_string_fast(data, i) {
                    elements.push(TextElement::String(bytes));
                    i = end;
                } else {
                    return None;
                }
            }
            b'0'..=b'9' | b'.' | b'+' | b'-' => {
                if let Some((num, consumed)) = parse_float_fast(&data[i..]) {
                    elements.push(TextElement::Offset(num));
                    i += consumed;
                } else {
                    return None;
                }
            }
            _ => {
                i += 1;
            }
        }
    }
}

/// Parse a name `/Name` from bytes. Returns (name_string, position_after_name).
#[inline]
fn parse_name_fast(data: &[u8], start: usize) -> (String, usize) {
    let mut i = start + 1;
    let name_start = i;
    while i < data.len() && !is_whitespace_or_delimiter(data[i]) {
        i += 1;
    }
    let name = String::from_utf8_lossy(&data[name_start..i]).to_string();
    (name, i)
}

/// Builds `Td`/`TD`/`Tm`/`T*` operators for the fast text-operator parser.
/// `None` if `op_bytes` isn't one of these. Pure code motion out of
/// `parse_text_operator_fast`'s dispatch match. ~keep
fn parse_fast_op_positioning(op_bytes: &[u8], operands: &[Option<FastOperand>; 8]) -> Option<Operator> {
    Some(match op_bytes {
        b"Td" => {
            let tx = match &operands[0] {
                Some(FastOperand::Number(n)) => *n,
                _ => 0.0,
            };
            let ty = match &operands[1] {
                Some(FastOperand::Number(n)) => *n,
                _ => 0.0,
            };
            Operator::Td { tx, ty }
        }
        b"TD" => {
            let tx = match &operands[0] {
                Some(FastOperand::Number(n)) => *n,
                _ => 0.0,
            };
            let ty = match &operands[1] {
                Some(FastOperand::Number(n)) => *n,
                _ => 0.0,
            };
            Operator::TD { tx, ty }
        }
        b"Tm" => {
            let get_n = |i: usize, def: f32| match &operands[i] {
                Some(FastOperand::Number(n)) => *n,
                _ => def,
            };
            Operator::Tm {
                a: get_n(0, 1.0),
                b: get_n(1, 0.0),
                c: get_n(2, 0.0),
                d: get_n(3, 1.0),
                e: get_n(4, 0.0),
                f: get_n(5, 0.0),
            }
        }
        b"T*" => Operator::TStar,
        _ => return None,
    })
}

/// Builds `Tj`/`TJ`/`'`/`"` operators for the fast text-operator parser.
/// `None` if `op_bytes` isn't one of these. Takes `operands` mutably because
/// the string-bearing operands are moved out via `.take()`, matching the
/// original inline arms. Pure code motion out of
/// `parse_text_operator_fast`'s dispatch match. ~keep
fn parse_fast_op_showing(op_bytes: &[u8], operands: &mut [Option<FastOperand>; 8]) -> Option<Operator> {
    Some(match op_bytes {
        b"Tj" => {
            let text = match operands[0].take() {
                Some(FastOperand::StringBytes(b)) => b,
                _ => Vec::new(),
            };
            Operator::Tj { text }
        }
        b"TJ" => {
            let array = match operands[0].take() {
                Some(FastOperand::TextArray(a)) => a,
                _ => Vec::new(),
            };
            Operator::TJ { array }
        }
        b"'" => {
            let text = match operands[0].take() {
                Some(FastOperand::StringBytes(b)) => b,
                _ => Vec::new(),
            };
            Operator::Quote { text }
        }
        b"\"" => {
            let word_space = match &operands[0] {
                Some(FastOperand::Number(n)) => *n,
                _ => 0.0,
            };
            let char_space = match &operands[1] {
                Some(FastOperand::Number(n)) => *n,
                _ => 0.0,
            };
            let text = match operands[2].take() {
                Some(FastOperand::StringBytes(b)) => b,
                _ => Vec::new(),
            };
            Operator::DoubleQuote {
                word_space,
                char_space,
                text,
            }
        }
        _ => return None,
    })
}

/// Builds `Tc`/`Tw`/`Tz`/`TL`/`Tr`/`Ts` operators for the fast text-operator
/// parser. `None` if `op_bytes` isn't one of these. Pure code motion out of
/// `parse_text_operator_fast`'s dispatch match. ~keep
fn parse_fast_op_text_state(op_bytes: &[u8], operands: &[Option<FastOperand>; 8]) -> Option<Operator> {
    Some(match op_bytes {
        b"Tc" => {
            let char_space = match &operands[0] {
                Some(FastOperand::Number(n)) => *n,
                _ => 0.0,
            };
            Operator::Tc { char_space }
        }
        b"Tw" => {
            let word_space = match &operands[0] {
                Some(FastOperand::Number(n)) => *n,
                _ => 0.0,
            };
            Operator::Tw { word_space }
        }
        b"Tz" => {
            let scale = match &operands[0] {
                Some(FastOperand::Number(n)) => *n,
                _ => 100.0,
            };
            Operator::Tz { scale }
        }
        b"TL" => {
            let leading = match &operands[0] {
                Some(FastOperand::Number(n)) => *n,
                _ => 0.0,
            };
            Operator::TL { leading }
        }
        b"Tr" => {
            let render = match &operands[0] {
                Some(FastOperand::Number(n)) => *n as u8,
                _ => 0,
            };
            Operator::Tr { render }
        }
        b"Ts" => {
            let rise = match &operands[0] {
                Some(FastOperand::Number(n)) => *n,
                _ => 0.0,
            };
            Operator::Ts { rise }
        }
        _ => return None,
    })
}

/// Builds `q`/`Q`/`cm` operators for the fast text-operator parser. `None`
/// if `op_bytes` isn't one of these. Pure code motion out of
/// `parse_text_operator_fast`'s dispatch match. ~keep
fn parse_fast_op_state_stack(op_bytes: &[u8], operands: &[Option<FastOperand>; 8]) -> Option<Operator> {
    Some(match op_bytes {
        b"q" => Operator::SaveState,
        b"Q" => Operator::RestoreState,
        b"cm" => {
            let get_n = |i: usize, def: f32| match &operands[i] {
                Some(FastOperand::Number(n)) => *n,
                _ => def,
            };
            Operator::Cm {
                a: get_n(0, 1.0),
                b: get_n(1, 0.0),
                c: get_n(2, 0.0),
                d: get_n(3, 1.0),
                e: get_n(4, 0.0),
                f: get_n(5, 0.0),
            }
        }
        _ => return None,
    })
}

/// Builds `rg`/`RG`/`g`/`G`/`k`/`K` operators for the fast text-operator
/// parser. `None` if `op_bytes` isn't one of these. Pure code motion out of
/// `parse_text_operator_fast`'s dispatch match. ~keep
fn parse_fast_op_simple_color(op_bytes: &[u8], operands: &[Option<FastOperand>; 8]) -> Option<Operator> {
    Some(match op_bytes {
        b"rg" => {
            let get_n = |i: usize| match &operands[i] {
                Some(FastOperand::Number(n)) => *n,
                _ => 0.0,
            };
            Operator::SetFillRgb {
                r: get_n(0),
                g: get_n(1),
                b: get_n(2),
            }
        }
        b"RG" => {
            let get_n = |i: usize| match &operands[i] {
                Some(FastOperand::Number(n)) => *n,
                _ => 0.0,
            };
            Operator::SetStrokeRgb {
                r: get_n(0),
                g: get_n(1),
                b: get_n(2),
            }
        }
        b"g" => {
            let gray = match &operands[0] {
                Some(FastOperand::Number(n)) => *n,
                _ => 0.0,
            };
            Operator::SetFillGray { gray }
        }
        b"G" => {
            let gray = match &operands[0] {
                Some(FastOperand::Number(n)) => *n,
                _ => 0.0,
            };
            Operator::SetStrokeGray { gray }
        }
        b"k" => {
            let get_n = |i: usize| match &operands[i] {
                Some(FastOperand::Number(n)) => *n,
                _ => 0.0,
            };
            Operator::SetFillCmyk {
                c: get_n(0),
                m: get_n(1),
                y: get_n(2),
                k: get_n(3),
            }
        }
        b"K" => {
            let get_n = |i: usize| match &operands[i] {
                Some(FastOperand::Number(n)) => *n,
                _ => 0.0,
            };
            Operator::SetStrokeCmyk {
                c: get_n(0),
                m: get_n(1),
                y: get_n(2),
                k: get_n(3),
            }
        }
        _ => return None,
    })
}

/// Builds `cs`/`CS`/`sc`/`SC`/`scn`/`SCN` operators for the fast
/// text-operator parser. `None` if `op_bytes` isn't one of these. Pure code
/// motion out of `parse_text_operator_fast`'s dispatch match. ~keep
fn parse_fast_op_indexed_color(
    op_bytes: &[u8],
    operands: &[Option<FastOperand>; 8],
    op_count: usize,
) -> Option<Operator> {
    Some(match op_bytes {
        b"cs" => {
            let name = match &operands[0] {
                Some(FastOperand::Name(n)) => n.clone(),
                _ => "DeviceGray".to_string(),
            };
            Operator::SetFillColorSpace { name }
        }
        b"CS" => {
            let name = match &operands[0] {
                Some(FastOperand::Name(n)) => n.clone(),
                _ => "DeviceGray".to_string(),
            };
            Operator::SetStrokeColorSpace { name }
        }
        b"sc" => {
            let components: Vec<f32> = operands[..op_count]
                .iter()
                .filter_map(|o| match o {
                    Some(FastOperand::Number(n)) => Some(*n),
                    _ => None,
                })
                .collect();
            Operator::SetFillColor { components }
        }
        b"SC" => {
            let components: Vec<f32> = operands[..op_count]
                .iter()
                .filter_map(|o| match o {
                    Some(FastOperand::Number(n)) => Some(*n),
                    _ => None,
                })
                .collect();
            Operator::SetStrokeColor { components }
        }
        b"scn" => {
            let name = match &operands[op_count.saturating_sub(1)] {
                Some(FastOperand::Name(n)) => Some(n.clone()),
                _ => None,
            };
            let components: Vec<f32> = operands[..op_count]
                .iter()
                .filter_map(|o| match o {
                    Some(FastOperand::Number(n)) => Some(*n),
                    _ => None,
                })
                .collect();
            Operator::SetFillColorN {
                components,
                name: name.map(Box::new),
            }
        }
        b"SCN" => {
            let name = match &operands[op_count.saturating_sub(1)] {
                Some(FastOperand::Name(n)) => Some(n.clone()),
                _ => None,
            };
            let components: Vec<f32> = operands[..op_count]
                .iter()
                .filter_map(|o| match o {
                    Some(FastOperand::Number(n)) => Some(*n),
                    _ => None,
                })
                .collect();
            Operator::SetStrokeColorN {
                components,
                name: name.map(Box::new),
            }
        }
        _ => return None,
    })
}

/// Builds `gs`/`Do` operators for the fast text-operator parser. `None` if
/// `op_bytes` isn't one of these. Pure code motion out of
/// `parse_text_operator_fast`'s dispatch match. ~keep
fn parse_fast_op_ext_state_or_xobject(
    op_bytes: &[u8],
    operands: &[Option<FastOperand>; 8],
    op_count: usize,
) -> Option<Operator> {
    Some(match op_bytes {
        b"gs" => {
            let dict_name = match &operands[0] {
                Some(FastOperand::Name(n)) => n.clone(),
                _ => String::new(),
            };
            Operator::SetExtGState { dict_name }
        }
        b"Do" => {
            // See the nom-parser "Do" arm in `build_operator` for why
            // this reads the last operand, not operands[0]. ~keep
            let name = match &operands[op_count.saturating_sub(1)] {
                Some(FastOperand::Name(n)) => n.clone(),
                _ => String::new(),
            };
            Operator::Do { name }
        }
        _ => return None,
    })
}

/// Builds `w`/`J`/`j`/`i` operators for the fast text-operator parser.
/// `None` if `op_bytes` isn't one of these. Pure code motion out of
/// `parse_text_operator_fast`'s dispatch match. ~keep
fn parse_fast_op_line_style(op_bytes: &[u8], operands: &[Option<FastOperand>; 8]) -> Option<Operator> {
    Some(match op_bytes {
        b"w" => {
            let width = match &operands[0] {
                Some(FastOperand::Number(n)) => *n,
                _ => 1.0,
            };
            Operator::SetLineWidth { width }
        }
        b"J" => {
            let cap_style = match &operands[0] {
                Some(FastOperand::Number(n)) => *n as u8,
                _ => 0,
            };
            Operator::SetLineCap { cap_style }
        }
        b"j" => {
            let join_style = match &operands[0] {
                Some(FastOperand::Number(n)) => *n as u8,
                _ => 0,
            };
            Operator::SetLineJoin { join_style }
        }
        b"i" => {
            let tolerance = match &operands[0] {
                Some(FastOperand::Number(n)) => *n,
                _ => 0.0,
            };
            Operator::SetFlatness { tolerance }
        }
        _ => return None,
    })
}

/// Fast parser for a single operator inside a BT/ET text block.
///
/// Returns `Some((remaining_input, operator))` on success, `None` on failure
/// (caller should fall back to the generic `parse_operator_with_operands`).
fn parse_text_operator_fast(input: &[u8]) -> Option<(&[u8], Operator)> {
    let mut pos = 0;
    // Small inline operand stack (max 8 operands for any PDF operator) ~keep
    let mut operands: [Option<FastOperand>; 8] = [None, None, None, None, None, None, None, None];
    let mut op_count: usize = 0;

    loop {
        while pos < input.len() && is_whitespace(input[pos]) {
            pos += 1;
        }
        if pos >= input.len() {
            return None;
        }

        let b = input[pos];
        match b {
            b'0'..=b'9' | b'.' | b'+' | b'-' => {
                // Quick check: a lone '-' or '+' followed by non-digit is not a number ~keep
                if (b == b'-' || b == b'+')
                    && (pos + 1 >= input.len() || (!input[pos + 1].is_ascii_digit() && input[pos + 1] != b'.'))
                {
                    return None;
                }
                if let Some((num, consumed)) = parse_float_fast(&input[pos..]) {
                    if op_count < 8 {
                        operands[op_count] = Some(FastOperand::Number(num));
                        op_count += 1;
                    }
                    pos += consumed;
                } else {
                    return None;
                }
            }
            b'(' => {
                if let Some((bytes, end)) = parse_literal_string_fast(input, pos) {
                    if op_count < 8 {
                        operands[op_count] = Some(FastOperand::StringBytes(bytes));
                        op_count += 1;
                    }
                    pos = end;
                } else {
                    return None;
                }
            }
            b'<' => {
                if pos + 1 < input.len() && input[pos + 1] == b'<' {
                    return None;
                }
                if let Some((bytes, end)) = parse_hex_string_fast(input, pos) {
                    if op_count < 8 {
                        operands[op_count] = Some(FastOperand::StringBytes(bytes));
                        op_count += 1;
                    }
                    pos = end;
                } else {
                    return None;
                }
            }
            b'/' => {
                let (name, end) = parse_name_fast(input, pos);
                if op_count < 8 {
                    operands[op_count] = Some(FastOperand::Name(name));
                    op_count += 1;
                }
                pos = end;
            }
            b'[' => {
                if let Some((elements, end)) = parse_tj_array_fast(input, pos) {
                    if op_count < 8 {
                        operands[op_count] = Some(FastOperand::TextArray(elements));
                        op_count += 1;
                    }
                    pos = end;
                } else {
                    return None;
                }
            }
            c if c.is_ascii_alphabetic() || c == b'\'' || c == b'"' || c == b'*' => {
                let op_start = pos;
                while pos < input.len()
                    && (input[pos].is_ascii_alphanumeric()
                        || input[pos] == b'\''
                        || input[pos] == b'"'
                        || input[pos] == b'*')
                {
                    pos += 1;
                }
                let op_bytes = &input[op_start..pos];
                let rest = &input[pos..];

                if op_bytes == b"true" || op_bytes == b"false" || op_bytes == b"null" {
                    continue;
                }

                let operator = match op_bytes {
                    b"ET" => Operator::EndText,
                    b"BT" => Operator::BeginText,
                    b"Tf" => {
                        let font = match &operands[0] {
                            Some(FastOperand::Name(n)) => n.clone(),
                            _ => String::new(),
                        };
                        let size = match &operands[1] {
                            Some(FastOperand::Number(n)) => *n,
                            _ => 12.0,
                        };
                        Operator::Tf { font, size }
                    }
                    b"Td" | b"TD" | b"Tm" | b"T*" => parse_fast_op_positioning(op_bytes, &operands)?,
                    b"Tj" | b"TJ" | b"'" | b"\"" => parse_fast_op_showing(op_bytes, &mut operands)?,
                    b"Tc" | b"Tw" | b"Tz" | b"TL" | b"Tr" | b"Ts" => parse_fast_op_text_state(op_bytes, &operands)?,
                    b"q" | b"Q" | b"cm" => parse_fast_op_state_stack(op_bytes, &operands)?,
                    b"rg" | b"RG" | b"g" | b"G" | b"k" | b"K" => parse_fast_op_simple_color(op_bytes, &operands)?,
                    b"cs" | b"CS" | b"sc" | b"SC" | b"scn" | b"SCN" => {
                        parse_fast_op_indexed_color(op_bytes, &operands, op_count)?
                    }
                    b"gs" | b"Do" => parse_fast_op_ext_state_or_xobject(op_bytes, &operands, op_count)?,
                    b"w" | b"J" | b"j" | b"i" => parse_fast_op_line_style(op_bytes, &operands)?,
                    _ => {
                        return None;
                    }
                };

                return Some((rest, operator));
            }
            _ => {
                return None;
            }
        }
    }
}

// Byte classification for fast graphics scanning.
// 0 = skip (whitespace, digits, dot, sign) — bulk-skippable
// 1 = alpha/quote/star — operator start
// 2 = '(' — literal string start
// 3 = '<' — hex string or dict start
// 4 = '[' — array start
// 5 = '/' — name start
// 6 = '%' — comment start
// 7 = other (unknown byte) ~keep
const SCAN_SKIP: u8 = 0;
const SCAN_ALPHA: u8 = 1;
const SCAN_PAREN: u8 = 2;
const SCAN_ANGLE: u8 = 3;
const SCAN_BRACKET: u8 = 4;
const SCAN_SLASH: u8 = 5;
const SCAN_PERCENT: u8 = 6;
const SCAN_OTHER: u8 = 7;

static BYTE_CLASS: [u8; 256] = {
    let mut t = [SCAN_OTHER; 256];
    t[b' ' as usize] = SCAN_SKIP;
    t[b'\t' as usize] = SCAN_SKIP;
    t[b'\n' as usize] = SCAN_SKIP;
    t[b'\r' as usize] = SCAN_SKIP;
    t[0x00] = SCAN_SKIP;
    t[0x0C] = SCAN_SKIP;
    t[b'0' as usize] = SCAN_SKIP;
    t[b'1' as usize] = SCAN_SKIP;
    t[b'2' as usize] = SCAN_SKIP;
    t[b'3' as usize] = SCAN_SKIP;
    t[b'4' as usize] = SCAN_SKIP;
    t[b'5' as usize] = SCAN_SKIP;
    t[b'6' as usize] = SCAN_SKIP;
    t[b'7' as usize] = SCAN_SKIP;
    t[b'8' as usize] = SCAN_SKIP;
    t[b'9' as usize] = SCAN_SKIP;
    t[b'.' as usize] = SCAN_SKIP;
    t[b'+' as usize] = SCAN_SKIP;
    t[b'-' as usize] = SCAN_SKIP;
    let mut c = b'A';
    while c <= b'Z' {
        t[c as usize] = SCAN_ALPHA;
        c += 1;
    }
    c = b'a';
    while c <= b'z' {
        t[c as usize] = SCAN_ALPHA;
        c += 1;
    }
    t[b'\'' as usize] = SCAN_ALPHA;
    t[b'"' as usize] = SCAN_ALPHA;
    t[b'*' as usize] = SCAN_ALPHA;
    t[b'(' as usize] = SCAN_PAREN;
    t[b'<' as usize] = SCAN_ANGLE;
    t[b'[' as usize] = SCAN_BRACKET;
    t[b'/' as usize] = SCAN_SLASH;
    t[b'%' as usize] = SCAN_PERCENT;
    t
};

fn scan_graphics_region<'a>(data: &'a [u8], consecutive_errors: &mut usize) -> ScanResult<'a> {
    let mut i: usize = 0;
    let mut operand_start: usize = 0;
    let mut deferred_depth: u32 = 0;
    let mut deferred_start: usize = 0;
    let len = data.len();

    loop {
        // Bulk-skip whitespace, digits, dots, signs — the most common bytes in graphics streams
        // ~keep
        while i < len && BYTE_CLASS[data[i] as usize] == SCAN_SKIP {
            i += 1;
        }
        if i >= len {
            return ScanResult::EndOfData;
        }

        match BYTE_CLASS[data[i] as usize] {
            SCAN_ALPHA => {
                let first_byte = data[i];
                let second_is_non_alpha = i + 1 >= len || BYTE_CLASS[data[i + 1] as usize] != SCAN_ALPHA;

                // Fast path for common single-char skippable operators.
                // Avoids reading the full operator name and is_skippable check.
                // Path: m(moveto), l(lineto), c(curveto), v/y(curves), h(close)
                // Paint: f/F(fill), B/b(fill+stroke), S/s(stroke), n(endpath), W(clip)
                // State: w(linewidth), d(dash), i(flatness), J/j(cap/join), M(miter)
                // Note: q/Q excluded (need deferred depth tracking). g/G/k/K
                // (gray/cmyk fill-color) also excluded - they mutate persistent
                // colour state that must reach a later BT/Tj when found outside
                // a q/Q scope (see is_color_op_bytes below); they fall through to
                // the slow alpha-scan path so that check can see them. ~keep
                if second_is_non_alpha
                    && matches!(
                        first_byte,
                        b'm' | b'l'
                            | b'c'
                            | b'v'
                            | b'y'
                            | b'h'
                            | b'f'
                            | b'F'
                            | b'B'
                            | b'b'
                            | b'S'
                            | b's'
                            | b'n'
                            | b'W'
                            | b'w'
                            | b'd'
                            | b'i'
                            | b'J'
                            | b'j'
                            | b'M'
                    )
                {
                    i += 1;
                    *consecutive_errors = 0;
                    operand_start = i;
                    continue;
                }

                let op_start = i;
                while i < len
                    && (data[i].is_ascii_alphanumeric() || data[i] == b'\'' || data[i] == b'"' || data[i] == b'*')
                {
                    i += 1;
                }
                let op = &data[op_start..i];

                if op == b"true" || op == b"false" || op == b"null" {
                    *consecutive_errors = 0;
                    continue;
                }

                *consecutive_errors = 0;

                if op == b"q" {
                    if deferred_depth == 0 {
                        deferred_start = operand_start;
                    }
                    deferred_depth += 1;
                    operand_start = i;
                    continue;
                } else if op == b"Q" {
                    if deferred_depth > 0 {
                        deferred_depth -= 1;
                        operand_start = i;
                        continue;
                    }
                    // Unmatched Q outside deferred — emit directly.
                    // Q has no operands; NeedFullParse invokes full nom parser
                    // for a trivial no-operand op (116K triggers for Penrose). ~keep
                    return ScanResult::SimpleOp {
                        op: Operator::RestoreState,
                        rest: &data[i..],
                    };
                } else if deferred_depth > 0 {
                    if op == b"cm" || op == b"gs" || is_skippable_graphics_op_bytes(op) {
                        operand_start = i;
                        continue;
                    }
                    return ScanResult::DeferredThenText {
                        deferred_start: &data[deferred_start..],
                        trigger_start: &data[operand_start..],
                    };
                } else if op == b"BT" {
                    return ScanResult::FoundBT { rest: &data[i..] };
                } else if op == b"BI" {
                    return ScanResult::InlineImage { rest: &data[i..] };
                } else if op == b"cm" {
                    // ConcatMatrix: parse 6 floats inline to avoid nom overhead
                    // (171K triggers/PDF for Murphy). Falls back to NeedFullParse
                    // on malformed operands. ~keep
                    if let Some((a, b, c, d, e, f)) = parse_six_floats(&data[operand_start..op_start]) {
                        return ScanResult::SimpleOp {
                            op: Operator::Cm { a, b, c, d, e, f },
                            rest: &data[i..],
                        };
                    }
                    return ScanResult::NeedFullParse {
                        operand_start: &data[operand_start..],
                        after_op: &data[i..],
                    };
                } else if is_color_op_bytes(op) {
                    // Outside any q/Q scope (deferred_depth == 0) nothing
                    // will revert this colour change before the next BT -
                    // route through the full parser so the handler applies
                    // it to GraphicsState instead of silently dropping it. ~keep
                    return ScanResult::NeedFullParse {
                        operand_start: &data[operand_start..],
                        after_op: &data[i..],
                    };
                } else if is_skippable_graphics_op_bytes(op) {
                    operand_start = i;
                    continue;
                } else {
                    return ScanResult::NeedFullParse {
                        operand_start: &data[operand_start..],
                        after_op: &data[i..],
                    };
                }
            }

            SCAN_PAREN => match skip_literal_string_raw(data, i) {
                Some(end) => {
                    i = end;
                    *consecutive_errors = 0;
                }
                None => {
                    i += 1;
                    *consecutive_errors += 1;
                }
            },

            SCAN_ANGLE => {
                if i + 1 < len && data[i + 1] == b'<' {
                    match skip_dict_raw(data, i) {
                        Some(end) => {
                            i = end;
                            *consecutive_errors = 0;
                        }
                        None => {
                            i += 1;
                            *consecutive_errors += 1;
                        }
                    }
                } else {
                    match skip_hex_string_raw(data, i) {
                        Some(end) => {
                            i = end;
                            *consecutive_errors = 0;
                        }
                        None => {
                            i += 1;
                            *consecutive_errors += 1;
                        }
                    }
                }
            }

            SCAN_BRACKET => match skip_array_raw(data, i) {
                Some(end) => {
                    i = end;
                    *consecutive_errors = 0;
                }
                None => {
                    i += 1;
                    *consecutive_errors += 1;
                }
            },

            SCAN_SLASH => {
                i = skip_name_raw(data, i);
                *consecutive_errors = 0;
            }

            SCAN_PERCENT => {
                while i < len && data[i] != b'\n' && data[i] != b'\r' {
                    i += 1;
                }
                *consecutive_errors = 0;
            }

            _ => {
                i += 1;
                *consecutive_errors += 1;
            }
        }

        if *consecutive_errors >= MAX_CONSECUTIVE_ERRORS {
            return ScanResult::TooManyErrors { remaining: &data[i..] };
        }
    }
}

#[cfg(test)]
// Tests intentionally pin the deprecated `parse_content_stream_text_only`. ~keep
#[allow(deprecated)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_text() {
        let stream = b"BT /F1 12 Tf 100 700 Td (Hello) Tj ET";
        let ops = parse_content_stream(stream).unwrap();
        assert_eq!(ops.len(), 5);

        assert!(matches!(ops[0], Operator::BeginText));
        assert!(matches!(ops[1], Operator::Tf { ref font, size } if font == "F1" && size == 12.0));
        assert!(matches!(ops[2], Operator::Td { tx, ty } if tx == 100.0 && ty == 700.0));
        assert!(matches!(ops[3], Operator::Tj { .. }));
        assert!(matches!(ops[4], Operator::EndText));
    }

    #[test]
    fn test_parse_text_matrix() {
        let stream = b"1 0 0 1 100 200 Tm";
        let ops = parse_content_stream(stream).unwrap();
        assert_eq!(ops.len(), 1);

        match &ops[0] {
            Operator::Tm { a, b, c, d, e, f } => {
                assert_eq!(*a, 1.0);
                assert_eq!(*b, 0.0);
                assert_eq!(*c, 0.0);
                assert_eq!(*d, 1.0);
                assert_eq!(*e, 100.0);
                assert_eq!(*f, 200.0);
            }
            _ => panic!("Expected Tm operator"),
        }
    }

    #[test]
    fn test_parse_tj_array() {
        let stream = b"[(Hello) -100 (World)] TJ";
        let ops = parse_content_stream(stream).unwrap();
        assert_eq!(ops.len(), 1);

        match &ops[0] {
            Operator::TJ { array } => {
                assert_eq!(array.len(), 3);
                assert!(matches!(array[0], TextElement::String(_)));
                assert!(matches!(array[1], TextElement::Offset(-100.0)));
                assert!(matches!(array[2], TextElement::String(_)));
            }
            _ => panic!("Expected TJ operator"),
        }
    }

    #[test]
    fn test_parse_color_operators() {
        let stream = b"1 0 0 rg\n0 1 0 RG";
        let ops = parse_content_stream(stream).unwrap();
        assert_eq!(ops.len(), 2);

        match &ops[0] {
            Operator::SetFillRgb { r, g, b } => {
                assert_eq!(*r, 1.0);
                assert_eq!(*g, 0.0);
                assert_eq!(*b, 0.0);
            }
            _ => panic!("Expected rg operator"),
        }

        match &ops[1] {
            Operator::SetStrokeRgb { r, g, b } => {
                assert_eq!(*r, 0.0);
                assert_eq!(*g, 1.0);
                assert_eq!(*b, 0.0);
            }
            _ => panic!("Expected RG operator"),
        }
    }

    #[test]
    fn test_parse_graphics_state() {
        let stream = b"q 1 0 0 1 50 50 cm Q";
        let ops = parse_content_stream(stream).unwrap();
        assert_eq!(ops.len(), 3);

        assert!(matches!(ops[0], Operator::SaveState));
        assert!(matches!(ops[1], Operator::Cm { .. }));
        assert!(matches!(ops[2], Operator::RestoreState));
    }

    #[test]
    fn test_parse_t_star() {
        let stream = b"T*";
        let ops = parse_content_stream(stream).unwrap();
        assert_eq!(ops.len(), 1);
        assert!(matches!(ops[0], Operator::TStar));
    }

    #[test]
    fn test_parse_text_state() {
        let stream = b"2 Tc 3 Tw 50 Tz 14 TL";
        let ops = parse_content_stream(stream).unwrap();
        assert_eq!(ops.len(), 4);

        assert!(matches!(ops[0], Operator::Tc { char_space } if char_space == 2.0));
        assert!(matches!(ops[1], Operator::Tw { word_space } if word_space == 3.0));
        assert!(matches!(ops[2], Operator::Tz { scale } if scale == 50.0));
        assert!(matches!(ops[3], Operator::TL { leading } if leading == 14.0));
    }

    #[test]
    fn test_parse_quote_operators() {
        let stream = b"(Text1) ' 1 0.5 (Text2) \"";
        let ops = parse_content_stream(stream).unwrap();
        assert_eq!(ops.len(), 2);

        assert!(matches!(ops[0], Operator::Quote { .. }));
        assert!(matches!(ops[1], Operator::DoubleQuote { .. }));
    }

    #[test]
    fn test_parse_path_operators() {
        let stream = b"100 200 m 150 250 l 10 10 50 50 re S";
        let ops = parse_content_stream(stream).unwrap();
        assert_eq!(ops.len(), 4);

        assert!(matches!(ops[0], Operator::MoveTo { x, y } if x == 100.0 && y == 200.0));
        assert!(matches!(ops[1], Operator::LineTo { x, y } if x == 150.0 && y == 250.0));
        assert!(matches!(ops[2], Operator::Rectangle { .. }));
        assert!(matches!(ops[3], Operator::Stroke));
    }

    #[test]
    fn test_parse_do_operator() {
        let stream = b"/Im1 Do";
        let ops = parse_content_stream(stream).unwrap();
        assert_eq!(ops.len(), 1);

        match &ops[0] {
            Operator::Do { name } => {
                assert_eq!(name, "Im1");
            }
            _ => panic!("Expected Do operator"),
        }
    }

    #[test]
    fn test_parse_empty_stream() {
        let stream = b"";
        let ops = parse_content_stream(stream).unwrap();
        assert_eq!(ops.len(), 0);
    }

    #[test]
    fn test_parse_whitespace_only() {
        let stream = b"   \n  \t  ";
        let ops = parse_content_stream(stream).unwrap();
        assert_eq!(ops.len(), 0);
    }

    #[test]
    fn test_parse_real_numbers() {
        let stream = b"1.5 2.7 Td";
        let ops = parse_content_stream(stream).unwrap();
        assert_eq!(ops.len(), 1);

        match &ops[0] {
            Operator::Td { tx, ty } => {
                assert_eq!(*tx, 1.5);
                assert_eq!(*ty, 2.7);
            }
            _ => panic!("Expected Td operator"),
        }
    }

    #[test]
    fn test_content_stream_operator_limit() {
        let count = super::MAX_OPERATORS + 1000;
        let stream: Vec<u8> = "q\n".repeat(count).into_bytes();
        let ops = parse_content_stream(&stream).unwrap();
        assert_eq!(ops.len(), super::MAX_OPERATORS);
    }

    #[test]
    fn test_content_stream_consecutive_error_bailout() {
        let junk = vec![0xFFu8; super::MAX_CONSECUTIVE_ERRORS + 500];
        let ops = parse_content_stream(&junk).unwrap();
        assert!(ops.is_empty());
    }

    #[test]
    fn test_text_only_skips_graphics() {
        let stream = b"100 200 m 300 400 l S BT /F1 12 Tf (Hello) Tj ET";
        let ops = parse_content_stream_text_only(stream).unwrap();
        assert_eq!(ops.len(), 4);
        assert!(matches!(ops[0], Operator::BeginText));
        assert!(matches!(ops[1], Operator::Tf { ref font, size } if font == "F1" && size == 12.0));
        assert!(matches!(ops[2], Operator::Tj { .. }));
        assert!(matches!(ops[3], Operator::EndText));
    }

    #[test]
    fn test_text_only_preserves_state_ops() {
        let stream = b"q 1 0 0 1 50 50 cm /Im1 Do Q";
        let ops = parse_content_stream_text_only(stream).unwrap();
        assert_eq!(ops.len(), 4);
        assert!(matches!(ops[0], Operator::SaveState));
        assert!(matches!(ops[1], Operator::Cm { .. }));
        assert!(matches!(ops[2], Operator::Do { ref name } if name == "Im1"));
        assert!(matches!(ops[3], Operator::RestoreState));
    }

    #[test]
    fn test_do_resolves_name_when_stray_operands_precede_it() {
        // A `q ... /Name Do Q` sequence whose leading numeric operands were
        // never consumed by a `cm` (missing/dropped token, or any other
        // non-conformant producer quirk). Per ISO 32000-1:2008 §7.8.2 an
        // operator's operand is whatever immediately precedes it — here
        // that's the Name, not the stray numbers ahead of it — so Do must
        // still resolve "Overlay", not silently produce "". ~keep
        let stream = b"q 1 0 0 1 20 150 /Overlay Do Q";
        let ops = parse_content_stream_text_only(stream).unwrap();
        assert!(
            ops.iter()
                .any(|op| matches!(op, Operator::Do { name } if name == "Overlay")),
            "expected a Do operator naming \"Overlay\", got: {:?}",
            ops
        );

        // Same stream through the full parser (used when ink exclusion is active). ~keep
        let ops_full = parse_content_stream(stream).unwrap();
        assert!(
            ops_full
                .iter()
                .any(|op| matches!(op, Operator::Do { name } if name == "Overlay"))
        );
    }

    #[test]
    fn test_text_only_preserves_color_ops_outside_bt() {
        // Color operators outside BT/ET are NOT skipped: nothing reverts
        // them before a later BT, so a colour set here must still reach
        // GraphicsState for whatever text object comes next (regression
        // for the "colour set before BT extracted as black" bug fixed by ~keep
        // is_color_op_bytes in scan_graphics_region).
        let stream = b"1 0 0 rg 0.5 g /CS1 cs";
        let ops = parse_content_stream_text_only(stream).unwrap();
        assert_eq!(ops.len(), 3);
        assert!(matches!(ops[0], Operator::SetFillRgb { r, g, b } if r == 1.0 && g == 0.0 && b == 0.0));
        assert!(matches!(ops[1], Operator::SetFillGray { gray } if gray == 0.5));
        assert!(matches!(ops[2], Operator::SetFillColorSpace { ref name } if name == "CS1"));
    }

    #[test]
    fn test_text_only_skips_complex_paths() {
        let stream = b"0 0 m 100 0 l 100 100 l 0 100 l h f 50 50 m 60 50 70 60 80 70 c S \
              BT /F1 10 Tf 72 700 Td (Text after paths) Tj ET \
              200 200 m 300 300 l S";
        let ops = parse_content_stream_text_only(stream).unwrap();
        assert_eq!(ops.len(), 5);
        assert!(matches!(ops[0], Operator::BeginText));
        assert!(matches!(ops[1], Operator::Tf { .. }));
        assert!(matches!(ops[2], Operator::Td { .. }));
        assert!(matches!(ops[3], Operator::Tj { .. }));
        assert!(matches!(ops[4], Operator::EndText));
    }

    #[test]
    fn test_text_only_handles_marked_content() {
        let stream = b"/Span BMC BT (Hello) Tj ET EMC";
        let ops = parse_content_stream_text_only(stream).unwrap();
        assert_eq!(ops.len(), 5);
        assert!(matches!(ops[0], Operator::BeginMarkedContent { ref tag } if tag == "Span"));
        assert!(matches!(ops[1], Operator::BeginText));
        assert!(matches!(ops[2], Operator::Tj { .. }));
        assert!(matches!(ops[3], Operator::EndText));
        assert!(matches!(ops[4], Operator::EndMarkedContent));
    }

    #[test]
    fn test_text_only_empty_and_whitespace() {
        assert_eq!(parse_content_stream_text_only(b"").unwrap().len(), 0);
        assert_eq!(parse_content_stream_text_only(b"   \n\t  ").unwrap().len(), 0);
    }

    #[test]
    fn test_text_only_graphics_only_stream() {
        let stream = b"0 0 m 100 0 l 100 100 l 0 100 l h f";
        let ops = parse_content_stream_text_only(stream).unwrap();
        assert_eq!(ops.len(), 0);
    }

    #[test]
    fn test_text_only_dash_pattern_skipped() {
        let stream = b"[3 2] 0 d BT (Hi) Tj ET";
        let ops = parse_content_stream_text_only(stream).unwrap();
        assert_eq!(ops.len(), 3);
        assert!(matches!(ops[0], Operator::BeginText));
        assert!(matches!(ops[1], Operator::Tj { .. }));
        assert!(matches!(ops[2], Operator::EndText));
    }

    #[test]
    fn test_text_only_gs_operator_preserved() {
        let stream = b"/GS0 gs BT (text) Tj ET";
        let ops = parse_content_stream_text_only(stream).unwrap();
        assert_eq!(ops.len(), 4);
        assert!(matches!(ops[0], Operator::SetExtGState { ref dict_name } if dict_name == "GS0"));
        assert!(matches!(ops[1], Operator::BeginText));
    }

    #[test]
    fn test_text_only_matches_full_parse_for_text() {
        let stream = b"q 1 0 0 1 72 700 cm BT /F1 12 Tf 0 0 Td (Hello World) Tj ET Q";
        let full = parse_content_stream(stream).unwrap();
        let text_only = parse_content_stream_text_only(stream).unwrap();

        assert_eq!(full.len(), text_only.len());
    }

    #[test]
    fn test_skip_operand_token_numbers() {
        assert_eq!(skip_operand_token(b"123 ").unwrap().0, b" ");
        assert_eq!(skip_operand_token(b"-45.6 ").unwrap().0, b" ");
        assert_eq!(skip_operand_token(b"+0.5 ").unwrap().0, b" ");
        assert_eq!(skip_operand_token(b".002 ").unwrap().0, b" ");
    }

    #[test]
    fn test_skip_operand_token_strings() {
        assert_eq!(skip_operand_token(b"(hello) ").unwrap().0, b" ");
        assert_eq!(skip_operand_token(b"(nested (parens)) ").unwrap().0, b" ");
        assert_eq!(skip_operand_token(b"(escaped \\) paren) ").unwrap().0, b" ");
        assert_eq!(skip_operand_token(b"<48656C6C6F> ").unwrap().0, b" ");
    }

    #[test]
    fn test_skip_operand_token_names_arrays_dicts() {
        assert_eq!(skip_operand_token(b"/Name ").unwrap().0, b" ");
        assert_eq!(skip_operand_token(b"[1 2 3] ").unwrap().0, b" ");
        assert_eq!(skip_operand_token(b"[(text) -100] ").unwrap().0, b" ");
        assert_eq!(skip_operand_token(b"<< /K 1 >> ").unwrap().0, b" ");
    }

    #[test]
    fn test_text_only_consecutive_error_bailout() {
        let junk = vec![0xFFu8; super::MAX_CONSECUTIVE_ERRORS + 500];
        let ops = parse_content_stream_text_only(&junk).unwrap();
        assert!(ops.is_empty());
    }

    #[test]
    fn test_prescan_single_bt_et() {
        let stream = b"BT /F1 12 Tf (Hello) Tj ET";
        let result = prescan_text_regions(stream);
        assert!(result.is_some(), "Should return Some for valid stream");
        let regions = result.unwrap().regions().to_vec();
        assert!(!regions.is_empty(), "Should find at least 1 region");
        let (start, end) = regions[0];
        assert_eq!(start, 0, "Region should start at BT");
        assert!(end >= 26, "Region should extend to or past ET");
    }

    #[test]
    fn test_prescan_multiple_bt_et() {
        let stream = b"BT (A) Tj ET BT (B) Tj ET BT (C) Tj ET";
        let result = prescan_text_regions(stream);
        assert!(result.is_some());
        let regions = result.unwrap().regions().to_vec();
        assert!(!regions.is_empty(), "Should find regions for 3 BT/ET blocks");
    }

    #[test]
    fn test_prescan_do_operator() {
        let stream = b"/Im1 Do";
        let result = prescan_text_regions(stream);
        assert!(result.is_some());
        let regions = result.unwrap().regions().to_vec();
        assert!(!regions.is_empty(), "Should find region for Do operator");
    }

    #[test]
    fn test_prescan_no_text_ops() {
        let stream = b"100 200 m 300 400 l S 0 0 100 100 re f";
        let result = prescan_text_regions(stream);
        assert!(result.is_some());
        let regions = result.unwrap().regions().to_vec();
        assert!(
            regions.is_empty(),
            "Pure graphics should return empty regions, got {:?}",
            regions
        );
    }

    #[test]
    fn test_prescan_bt_in_string_literal() {
        let stream = b"(text BT here) Tj";
        let result = prescan_text_regions(stream);
        assert!(result.is_some(), "Should not return None for string containing BT");
    }

    #[test]
    fn test_prescan_merges_overlapping_regions() {
        let stream = b"q BT (A) Tj ET Q q BT (B) Tj ET Q";
        let result = prescan_text_regions(stream);
        assert!(result.is_some());
        let regions = result.unwrap().regions().to_vec();
        assert!(!regions.is_empty());
        for i in 1..regions.len() {
            assert!(
                regions[i].0 >= regions[i - 1].1,
                "Regions should not overlap after merge: {:?} and {:?}",
                regions[i - 1],
                regions[i]
            );
        }
    }

    #[test]
    fn test_parse_inline_image_basic() {
        let stream = b"BI /W 4 /H 4 /BPC 8 /CS /DeviceGray ID ABCD EI";
        let ops = parse_content_stream(stream).unwrap();
        assert!(ops.iter().any(|op| matches!(op, Operator::InlineImage { .. })));
        for op in &ops {
            if let Operator::InlineImage { dict, data } = op {
                assert_eq!(dict.get("W").and_then(|o| o.as_integer()), Some(4));
                assert_eq!(dict.get("H").and_then(|o| o.as_integer()), Some(4));
                assert_eq!(dict.get("BPC").and_then(|o| o.as_integer()), Some(8));
                assert!(!data.is_empty());
            }
        }
    }

    #[test]
    fn test_parse_inline_image_empty_data() {
        let stream = b"BI /W 1 /H 1 ID X EI";
        let ops = parse_content_stream(stream).unwrap();
        assert!(ops.iter().any(|op| matches!(op, Operator::InlineImage { .. })));
    }

    #[test]
    fn test_parse_inline_image_in_stream_context() {
        let stream = b"q 1 0 0 1 0 0 cm BI /W 2 /H 2 ID AB EI Q";
        let ops = parse_content_stream(stream).unwrap();
        assert!(ops.len() >= 3);
        assert!(matches!(ops[0], Operator::SaveState));
        assert!(matches!(ops[1], Operator::Cm { .. }));
        assert!(ops.iter().any(|op| matches!(op, Operator::InlineImage { .. })));
        assert!(ops.iter().any(|op| matches!(op, Operator::RestoreState)));
    }

    #[test]
    fn test_parse_inline_image_null_before_ei() {
        // PDF spec Table 1 lists NULL (0x00) as whitespace. ~keep
        let mut stream = b"q BI /W 2 /H 2 ID AB".to_vec();
        stream.extend_from_slice(b"\x00EI Q BT (Hi) Tj ET");
        let ops = parse_content_stream(&stream).unwrap();
        assert!(ops.iter().any(|op| matches!(op, Operator::InlineImage { .. })));
        assert!(ops.iter().any(|op| matches!(op, Operator::RestoreState)));
        assert!(ops.iter().any(|op| matches!(op, Operator::Tj { .. })));
    }

    #[test]
    fn test_parse_negative_numbers() {
        let stream = b"-100 -200 Td";
        let ops = parse_content_stream(stream).unwrap();
        assert_eq!(ops.len(), 1);
        match &ops[0] {
            Operator::Td { tx, ty } => {
                assert_eq!(*tx, -100.0);
                assert_eq!(*ty, -200.0);
            }
            _ => panic!("Expected Td"),
        }
    }

    #[test]
    fn test_parse_decimal_numbers() {
        let stream = b"0.001 99.999 Td";
        let ops = parse_content_stream(stream).unwrap();
        assert_eq!(ops.len(), 1);
        match &ops[0] {
            Operator::Td { tx, ty } => {
                assert!((tx - 0.001).abs() < 0.0001);
                assert!((ty - 99.999).abs() < 0.01);
            }
            _ => panic!("Expected Td"),
        }
    }

    #[test]
    fn test_parse_leading_dot_number() {
        let stream = b".5 .25 Td";
        let ops = parse_content_stream(stream).unwrap();
        assert_eq!(ops.len(), 1);
        match &ops[0] {
            Operator::Td { tx, ty } => {
                assert!((tx - 0.5).abs() < 0.001);
                assert!((ty - 0.25).abs() < 0.001);
            }
            _ => panic!("Expected Td"),
        }
    }

    #[test]
    fn test_parse_large_numbers() {
        let stream = b"99999 88888 Td";
        let ops = parse_content_stream(stream).unwrap();
        assert_eq!(ops.len(), 1);
        match &ops[0] {
            Operator::Td { tx, ty } => {
                assert_eq!(*tx, 99999.0);
                assert_eq!(*ty, 88888.0);
            }
            _ => panic!("Expected Td"),
        }
    }

    #[test]
    fn test_parse_zero() {
        let stream = b"0 0 Td";
        let ops = parse_content_stream(stream).unwrap();
        assert_eq!(ops.len(), 1);
        match &ops[0] {
            Operator::Td { tx, ty } => {
                assert_eq!(*tx, 0.0);
                assert_eq!(*ty, 0.0);
            }
            _ => panic!("Expected Td"),
        }
    }

    #[test]
    fn test_parse_string_with_nested_parens() {
        let stream = b"(Hello (World)) Tj";
        let ops = parse_content_stream(stream).unwrap();
        assert_eq!(ops.len(), 1);
        match &ops[0] {
            Operator::Tj { text } => {
                assert_eq!(text, b"Hello (World)");
            }
            _ => panic!("Expected Tj"),
        }
    }

    #[test]
    fn test_parse_string_with_escape_sequences() {
        let stream = b"(Line1\\nLine2\\r\\t) Tj";
        let ops = parse_content_stream(stream).unwrap();
        assert_eq!(ops.len(), 1);
        match &ops[0] {
            Operator::Tj { text } => {
                assert!(!text.is_empty());
            }
            _ => panic!("Expected Tj"),
        }
    }

    #[test]
    fn test_parse_empty_string() {
        let stream = b"() Tj";
        let ops = parse_content_stream(stream).unwrap();
        assert_eq!(ops.len(), 1);
        match &ops[0] {
            Operator::Tj { text } => {
                assert!(text.is_empty());
            }
            _ => panic!("Expected Tj"),
        }
    }

    #[test]
    fn test_parse_hex_string() {
        let stream = b"<48656C6C6F> Tj";
        let ops = parse_content_stream(stream).unwrap();
        assert_eq!(ops.len(), 1);
        match &ops[0] {
            Operator::Tj { text } => {
                assert_eq!(text, b"Hello");
            }
            _ => panic!("Expected Tj"),
        }
    }

    #[test]
    fn test_parse_hex_string_odd_digits() {
        // Odd number of hex digits: trailing nibble should be padded with 0 ~keep
        let stream = b"<ABC> Tj";
        let ops = parse_content_stream(stream).unwrap();
        assert_eq!(ops.len(), 1);
        match &ops[0] {
            Operator::Tj { text } => {
                assert_eq!(text.len(), 2);
                assert_eq!(text[0], 0xAB);
                assert_eq!(text[1], 0xC0);
            }
            _ => panic!("Expected Tj"),
        }
    }

    #[test]
    fn test_parse_empty_hex_string() {
        let stream = b"<> Tj";
        let ops = parse_content_stream(stream).unwrap();
        assert_eq!(ops.len(), 1);
        match &ops[0] {
            Operator::Tj { text } => {
                assert!(text.is_empty());
            }
            _ => panic!("Expected Tj"),
        }
    }

    #[test]
    fn test_parse_line_width() {
        let stream = b"2.5 w";
        let ops = parse_content_stream(stream).unwrap();
        assert_eq!(ops.len(), 1);
        assert!(matches!(ops[0], Operator::SetLineWidth { width } if (width - 2.5).abs() < 0.001));
    }

    #[test]
    fn test_parse_line_cap() {
        let stream = b"1 J";
        let ops = parse_content_stream(stream).unwrap();
        assert_eq!(ops.len(), 1);
        assert!(matches!(ops[0], Operator::SetLineCap { cap_style: 1 }));
    }

    #[test]
    fn test_parse_line_join() {
        let stream = b"2 j";
        let ops = parse_content_stream(stream).unwrap();
        assert_eq!(ops.len(), 1);
        assert!(matches!(ops[0], Operator::SetLineJoin { join_style: 2 }));
    }

    #[test]
    fn test_parse_miter_limit() {
        let stream = b"10 M";
        let ops = parse_content_stream(stream).unwrap();
        assert_eq!(ops.len(), 1);
        assert!(matches!(ops[0], Operator::SetMiterLimit { limit } if limit == 10.0));
    }

    #[test]
    fn test_parse_dash_pattern() {
        let stream = b"[3 2] 0 d";
        let ops = parse_content_stream(stream).unwrap();
        assert_eq!(ops.len(), 1);
        match &ops[0] {
            Operator::SetDash { array, phase } => {
                assert_eq!(array, &[3.0, 2.0]);
                assert_eq!(*phase, 0.0);
            }
            _ => panic!("Expected SetDash"),
        }
    }

    #[test]
    fn test_parse_rendering_intent() {
        let stream = b"/AbsoluteColorimetric ri";
        let ops = parse_content_stream(stream).unwrap();
        assert_eq!(ops.len(), 1);
        match &ops[0] {
            Operator::SetRenderingIntent { intent } => {
                assert_eq!(intent, "AbsoluteColorimetric");
            }
            _ => panic!("Expected SetRenderingIntent"),
        }
    }

    #[test]
    fn test_parse_flatness() {
        let stream = b"50 i";
        let ops = parse_content_stream(stream).unwrap();
        assert_eq!(ops.len(), 1);
        assert!(matches!(ops[0], Operator::SetFlatness { tolerance } if tolerance == 50.0));
    }

    #[test]
    fn test_parse_ext_gstate() {
        let stream = b"/GS0 gs";
        let ops = parse_content_stream(stream).unwrap();
        assert_eq!(ops.len(), 1);
        match &ops[0] {
            Operator::SetExtGState { dict_name } => {
                assert_eq!(dict_name, "GS0");
            }
            _ => panic!("Expected SetExtGState"),
        }
    }

    #[test]
    fn test_parse_paint_shading() {
        let stream = b"/Sh0 sh";
        let ops = parse_content_stream(stream).unwrap();
        assert_eq!(ops.len(), 1);
        match &ops[0] {
            Operator::PaintShading { name } => {
                assert_eq!(name, "Sh0");
            }
            _ => panic!("Expected PaintShading"),
        }
    }

    #[test]
    fn test_parse_gray_color() {
        let stream = b"0.5 g 0.8 G";
        let ops = parse_content_stream(stream).unwrap();
        assert_eq!(ops.len(), 2);
        assert!(matches!(ops[0], Operator::SetFillGray { gray } if (gray - 0.5).abs() < 0.001));
        assert!(matches!(ops[1], Operator::SetStrokeGray { gray } if (gray - 0.8).abs() < 0.001));
    }

    #[test]
    fn test_parse_cmyk_color() {
        let stream = b"0.1 0.2 0.3 0.4 k\n0.5 0.6 0.7 0.8 K";
        let ops = parse_content_stream(stream).unwrap();
        assert_eq!(ops.len(), 2);
        match &ops[0] {
            Operator::SetFillCmyk { c, m, y, k } => {
                assert!((c - 0.1).abs() < 0.01);
                assert!((m - 0.2).abs() < 0.01);
                assert!((y - 0.3).abs() < 0.01);
                assert!((k - 0.4).abs() < 0.01);
            }
            _ => panic!("Expected SetFillCmyk"),
        }
        match &ops[1] {
            Operator::SetStrokeCmyk { c, m, y, k } => {
                assert!((c - 0.5).abs() < 0.01);
                assert!((m - 0.6).abs() < 0.01);
                assert!((y - 0.7).abs() < 0.01);
                assert!((k - 0.8).abs() < 0.01);
            }
            _ => panic!("Expected SetStrokeCmyk"),
        }
    }

    #[test]
    fn test_parse_color_space_operators() {
        let stream = b"/DeviceRGB cs /DeviceCMYK CS";
        let ops = parse_content_stream(stream).unwrap();
        assert_eq!(ops.len(), 2);
        match &ops[0] {
            Operator::SetFillColorSpace { name } => assert_eq!(name, "DeviceRGB"),
            _ => panic!("Expected SetFillColorSpace"),
        }
        match &ops[1] {
            Operator::SetStrokeColorSpace { name } => assert_eq!(name, "DeviceCMYK"),
            _ => panic!("Expected SetStrokeColorSpace"),
        }
    }

    #[test]
    fn test_parse_sc_color_components() {
        let stream = b"0.1 0.2 0.3 sc";
        let ops = parse_content_stream(stream).unwrap();
        assert_eq!(ops.len(), 1);
        match &ops[0] {
            Operator::SetFillColor { components } => {
                assert_eq!(components.len(), 3);
                assert!((components[0] - 0.1).abs() < 0.01);
                assert!((components[1] - 0.2).abs() < 0.01);
                assert!((components[2] - 0.3).abs() < 0.01);
            }
            _ => panic!("Expected SetFillColor"),
        }
    }

    #[test]
    fn test_parse_sc_stroke_color_components() {
        let stream = b"0.5 0.6 SC";
        let ops = parse_content_stream(stream).unwrap();
        assert_eq!(ops.len(), 1);
        match &ops[0] {
            Operator::SetStrokeColor { components } => {
                assert_eq!(components.len(), 2);
            }
            _ => panic!("Expected SetStrokeColor"),
        }
    }

    #[test]
    fn test_parse_scn_with_pattern_name() {
        let stream = b"0.5 /Pattern1 scn";
        let ops = parse_content_stream(stream).unwrap();
        assert_eq!(ops.len(), 1);
        match &ops[0] {
            Operator::SetFillColorN { components, name } => {
                assert_eq!(components.len(), 1);
                assert!(name.is_some());
                assert_eq!(**name.as_ref().unwrap(), "Pattern1");
            }
            _ => panic!("Expected SetFillColorN"),
        }
    }

    #[test]
    fn test_parse_scn_without_pattern_name() {
        let stream = b"0.1 0.2 0.3 scn";
        let ops = parse_content_stream(stream).unwrap();
        assert_eq!(ops.len(), 1);
        match &ops[0] {
            Operator::SetFillColorN { components, name } => {
                assert_eq!(components.len(), 3);
                assert!(name.is_none());
            }
            _ => panic!("Expected SetFillColorN"),
        }
    }

    #[test]
    fn test_parse_scn_stroke_with_pattern() {
        let stream = b"0.5 /P1 SCN";
        let ops = parse_content_stream(stream).unwrap();
        assert_eq!(ops.len(), 1);
        match &ops[0] {
            Operator::SetStrokeColorN { components, name } => {
                assert_eq!(components.len(), 1);
                assert!(name.is_some());
            }
            _ => panic!("Expected SetStrokeColorN"),
        }
    }

    #[test]
    fn test_parse_bmc() {
        let stream = b"/Span BMC";
        let ops = parse_content_stream(stream).unwrap();
        assert_eq!(ops.len(), 1);
        match &ops[0] {
            Operator::BeginMarkedContent { tag } => assert_eq!(tag, "Span"),
            _ => panic!("Expected BeginMarkedContent"),
        }
    }

    #[test]
    fn test_parse_bdc_with_dict() {
        let stream = b"/Span << /MCID 0 >> BDC";
        let ops = parse_content_stream(stream).unwrap();
        assert_eq!(ops.len(), 1);
        match &ops[0] {
            Operator::BeginMarkedContentDict { tag, properties } => {
                assert_eq!(tag, "Span");
                assert!(!matches!(**properties, Object::Null));
            }
            _ => panic!("Expected BeginMarkedContentDict"),
        }
    }

    #[test]
    fn test_parse_bdc_with_name_ref() {
        let stream = b"/Span /MC0 BDC";
        let ops = parse_content_stream(stream).unwrap();
        assert_eq!(ops.len(), 1);
        match &ops[0] {
            Operator::BeginMarkedContentDict { tag, properties } => {
                assert_eq!(tag, "Span");
                assert_eq!(properties.as_name(), Some("MC0"));
            }
            _ => panic!("Expected BeginMarkedContentDict"),
        }
    }

    #[test]
    fn test_parse_emc() {
        let stream = b"EMC";
        let ops = parse_content_stream(stream).unwrap();
        assert_eq!(ops.len(), 1);
        assert!(matches!(ops[0], Operator::EndMarkedContent));
    }

    #[test]
    fn test_parse_marked_content_nesting() {
        let stream = b"/Article BMC /P << /MCID 1 >> BDC BT (text) Tj ET EMC EMC";
        let ops = parse_content_stream(stream).unwrap();
        assert_eq!(ops.len(), 7);
        assert!(matches!(ops[0], Operator::BeginMarkedContent { .. }));
        assert!(matches!(ops[1], Operator::BeginMarkedContentDict { .. }));
        assert!(matches!(ops[2], Operator::BeginText));
        assert!(matches!(ops[3], Operator::Tj { .. }));
        assert!(matches!(ops[4], Operator::EndText));
        assert!(matches!(ops[5], Operator::EndMarkedContent));
        assert!(matches!(ops[6], Operator::EndMarkedContent));
    }

    #[test]
    fn test_parse_bezier_curves() {
        let stream = b"10 20 30 40 50 60 c";
        let ops = parse_content_stream(stream).unwrap();
        assert_eq!(ops.len(), 1);
        match &ops[0] {
            Operator::CurveTo { x1, y1, x2, y2, x3, y3 } => {
                assert_eq!(*x1, 10.0);
                assert_eq!(*y1, 20.0);
                assert_eq!(*x2, 30.0);
                assert_eq!(*y2, 40.0);
                assert_eq!(*x3, 50.0);
                assert_eq!(*y3, 60.0);
            }
            _ => panic!("Expected CurveTo"),
        }
    }

    #[test]
    fn test_parse_curve_to_v() {
        let stream = b"10 20 30 40 v";
        let ops = parse_content_stream(stream).unwrap();
        assert_eq!(ops.len(), 1);
        match &ops[0] {
            Operator::CurveToV { x2, y2, x3, y3 } => {
                assert_eq!(*x2, 10.0);
                assert_eq!(*y2, 20.0);
                assert_eq!(*x3, 30.0);
                assert_eq!(*y3, 40.0);
            }
            _ => panic!("Expected CurveToV"),
        }
    }

    #[test]
    fn test_parse_curve_to_y() {
        let stream = b"10 20 30 40 y";
        let ops = parse_content_stream(stream).unwrap();
        assert_eq!(ops.len(), 1);
        match &ops[0] {
            Operator::CurveToY { x1, y1, x3, y3 } => {
                assert_eq!(*x1, 10.0);
                assert_eq!(*y1, 20.0);
                assert_eq!(*x3, 30.0);
                assert_eq!(*y3, 40.0);
            }
            _ => panic!("Expected CurveToY"),
        }
    }

    #[test]
    fn test_parse_close_path() {
        let stream = b"h";
        let ops = parse_content_stream(stream).unwrap();
        assert_eq!(ops.len(), 1);
        assert!(matches!(ops[0], Operator::ClosePath));
    }

    #[test]
    fn test_parse_fill_variants() {
        let stream = b"f\nf*\nn";
        let ops = parse_content_stream(stream).unwrap();
        assert_eq!(ops.len(), 3);
        assert!(matches!(ops[0], Operator::Fill));
        assert!(matches!(ops[1], Operator::FillEvenOdd));
        assert!(matches!(ops[2], Operator::EndPath));
    }

    #[test]
    fn test_parse_close_fill_stroke() {
        let stream = b"b";
        let ops = parse_content_stream(stream).unwrap();
        assert_eq!(ops.len(), 1);
        assert!(matches!(ops[0], Operator::CloseFillStroke));
    }

    #[test]
    fn test_parse_clipping() {
        let stream = b"W\nW*";
        let ops = parse_content_stream(stream).unwrap();
        assert_eq!(ops.len(), 2);
        assert!(matches!(ops[0], Operator::ClipNonZero));
        assert!(matches!(ops[1], Operator::ClipEvenOdd));
    }

    #[test]
    fn test_parse_rectangle() {
        let stream = b"10 20 100 50 re";
        let ops = parse_content_stream(stream).unwrap();
        assert_eq!(ops.len(), 1);
        match &ops[0] {
            Operator::Rectangle { x, y, width, height } => {
                assert_eq!(*x, 10.0);
                assert_eq!(*y, 20.0);
                assert_eq!(*width, 100.0);
                assert_eq!(*height, 50.0);
            }
            _ => panic!("Expected Rectangle"),
        }
    }

    #[test]
    fn test_parse_tr_render_mode() {
        let stream = b"1 Tr";
        let ops = parse_content_stream(stream).unwrap();
        assert_eq!(ops.len(), 1);
        assert!(matches!(ops[0], Operator::Tr { render: 1 }));
    }

    #[test]
    fn test_parse_ts_text_rise() {
        let stream = b"5 Ts";
        let ops = parse_content_stream(stream).unwrap();
        assert_eq!(ops.len(), 1);
        assert!(matches!(ops[0], Operator::Ts { rise } if rise == 5.0));
    }

    #[test]
    fn test_parse_double_quote_operator() {
        let stream = b"1.5 0.5 (Hello) \"";
        let ops = parse_content_stream(stream).unwrap();
        assert_eq!(ops.len(), 1);
        match &ops[0] {
            Operator::DoubleQuote {
                word_space,
                char_space,
                text,
            } => {
                assert!((word_space - 1.5).abs() < 0.001);
                assert!((char_space - 0.5).abs() < 0.001);
                assert_eq!(text, b"Hello");
            }
            _ => panic!("Expected DoubleQuote"),
        }
    }

    #[test]
    fn test_parse_single_quote_operator() {
        let stream = b"(NextLine) '";
        let ops = parse_content_stream(stream).unwrap();
        assert_eq!(ops.len(), 1);
        match &ops[0] {
            Operator::Quote { text } => {
                assert_eq!(text, b"NextLine");
            }
            _ => panic!("Expected Quote"),
        }
    }

    #[test]
    fn test_parse_unknown_operator() {
        let stream = b"42 XYZ";
        let ops = parse_content_stream(stream).unwrap();
        assert_eq!(ops.len(), 1);
        match &ops[0] {
            Operator::Other { name, operands } => {
                assert_eq!(name, "XYZ");
                assert_eq!(operands.len(), 1);
            }
            _ => panic!("Expected Other operator"),
        }
    }

    #[test]
    fn test_parse_unknown_operator_no_operands() {
        let stream = b"BX";
        let ops = parse_content_stream(stream).unwrap();
        assert_eq!(ops.len(), 1);
        match &ops[0] {
            Operator::Other { name, operands } => {
                assert_eq!(name, "BX");
                assert_eq!(operands.len(), 0);
            }
            _ => panic!("Expected Other for BX"),
        }
    }

    #[test]
    fn test_parse_compatibility_operators() {
        let stream = b"BX EX";
        let ops = parse_content_stream(stream).unwrap();
        assert_eq!(ops.len(), 2);
        assert!(matches!(&ops[0], Operator::Other { name, .. } if name == "BX"));
        assert!(matches!(&ops[1], Operator::Other { name, .. } if name == "EX"));
    }

    #[test]
    fn test_parse_mp_dp_marked_point() {
        let stream = b"/Tag MP /Tag2 << /K 1 >> DP";
        let ops = parse_content_stream(stream).unwrap();
        assert_eq!(ops.len(), 2);
        assert!(matches!(&ops[0], Operator::Other { name, .. } if name == "MP"));
        assert!(matches!(&ops[1], Operator::Other { name, .. } if name == "DP"));
    }

    #[test]
    fn test_images_only_captures_do() {
        let stream = b"q 1 0 0 1 0 0 cm /Im1 Do Q";
        let ops = parse_content_stream_images_only(stream).unwrap();
        assert!(
            ops.iter()
                .any(|op| matches!(op, Operator::Do { name } if name == "Im1"))
        );
        assert!(ops.iter().any(|op| matches!(op, Operator::SaveState)));
        assert!(ops.iter().any(|op| matches!(op, Operator::RestoreState)));
        assert!(ops.iter().any(|op| matches!(op, Operator::Cm { .. })));
    }

    #[test]
    fn test_images_only_skips_text_blocks() {
        let stream = b"BT /F1 12 Tf (Hello) Tj ET /Im1 Do";
        let ops = parse_content_stream_images_only(stream).unwrap();
        for op in &ops {
            assert!(!matches!(op, Operator::BeginText));
            assert!(!matches!(op, Operator::EndText));
            assert!(!matches!(op, Operator::Tf { .. }));
            assert!(!matches!(op, Operator::Tj { .. }));
        }
        assert!(ops.iter().any(|op| matches!(op, Operator::Do { .. })));
    }

    #[test]
    fn test_images_only_empty_stream() {
        let ops = parse_content_stream_images_only(b"").unwrap();
        assert!(ops.is_empty());
    }

    #[test]
    fn test_images_only_pure_text_stream() {
        let stream = b"BT /F1 12 Tf 100 700 Td (Hello) Tj ET";
        let ops = parse_content_stream_images_only(stream).unwrap();
        assert!(ops.is_empty() || !ops.iter().any(|op| matches!(op, Operator::Tj { .. })));
    }

    #[test]
    fn test_images_only_inline_image() {
        let stream = b"q 1 0 0 1 0 0 cm BI /W 2 /H 2 ID AB EI Q";
        let ops = parse_content_stream_images_only(stream).unwrap();
        assert!(ops.iter().any(|op| matches!(op, Operator::InlineImage { .. })));
    }

    #[test]
    fn test_images_only_multiple_text_blocks() {
        let stream = b"BT (A) Tj ET BT (B) Tj ET q /Im1 Do Q BT (C) Tj ET";
        let ops = parse_content_stream_images_only(stream).unwrap();
        assert!(ops.iter().any(|op| matches!(op, Operator::Do { .. })));
        assert!(!ops.iter().any(|op| matches!(op, Operator::Tj { .. })));
    }

    #[test]
    fn test_text_only_preserves_text_state_ops_inside_bt() {
        let stream = b"BT 2 Tc 3 Tw 50 Tz 14 TL 1 Tr 5 Ts ET";
        let ops = parse_content_stream_text_only(stream).unwrap();
        assert_eq!(ops.len(), 8);
        assert!(matches!(ops[0], Operator::BeginText));
        assert!(matches!(ops[1], Operator::Tc { .. }));
        assert!(matches!(ops[2], Operator::Tw { .. }));
        assert!(matches!(ops[3], Operator::Tz { .. }));
        assert!(matches!(ops[4], Operator::TL { .. }));
        assert!(matches!(ops[5], Operator::Tr { .. }));
        assert!(matches!(ops[6], Operator::Ts { .. }));
        assert!(matches!(ops[7], Operator::EndText));
    }

    #[test]
    fn test_text_only_inline_image_outside_bt_skipped() {
        let stream = b"BI /W 2 /H 2 ID XY EI BT (Hi) Tj ET";
        let ops = parse_content_stream_text_only(stream).unwrap();
        assert!(!ops.iter().any(|op| matches!(op, Operator::InlineImage { .. })));
        assert!(ops.iter().any(|op| matches!(op, Operator::Tj { .. })));
    }

    #[test]
    fn test_text_only_cm_before_bt_preserved() {
        let stream = b"q 1 0 0 1 72 700 cm BT (Hello) Tj ET Q";
        let ops = parse_content_stream_text_only(stream).unwrap();
        assert!(ops.iter().any(|op| matches!(op, Operator::Cm { .. })));
        assert!(ops.iter().any(|op| matches!(op, Operator::Tj { .. })));
    }

    #[test]
    fn test_parse_and_execute_text_only_basic() {
        let stream = b"BT /F1 12 Tf (Hello) Tj ET";
        let mut ops = Vec::new();
        parse_and_execute_text_only(stream, |op| {
            ops.push(op);
            Ok(())
        })
        .unwrap();
        assert!(ops.len() >= 3);
        assert!(matches!(ops[0], Operator::BeginText));
    }

    #[test]
    fn test_parse_and_execute_text_only_skips_graphics() {
        let stream = b"100 200 m 300 400 l S BT (Hello) Tj ET";
        let mut ops = Vec::new();
        parse_and_execute_text_only(stream, |op| {
            ops.push(op);
            Ok(())
        })
        .unwrap();
        assert!(!ops.iter().any(|op| matches!(op, Operator::MoveTo { .. })));
        assert!(ops.iter().any(|op| matches!(op, Operator::Tj { .. })));
    }

    #[test]
    fn test_parse_and_execute_text_only_empty() {
        let mut ops = Vec::new();
        parse_and_execute_text_only(b"", |op| {
            ops.push(op);
            Ok(())
        })
        .unwrap();
        assert!(ops.is_empty());
    }

    #[test]
    fn test_prescan_preserves_outer_ctm_for_deeply_nested_text() {
        // Regression test: prescan_text_regions loses outer CTM context when
        // text is deeply nested in graphics state with scaling cm operators
        // separated by >4KB of path data.
        //
        // Build a >256KB stream with:
        //   q / cm (scaling) / >4KB of filler / q / cm (translation) / BT...ET / Q / Q
        //
        // The forward CTM scan should capture the outer scaling and inject it
        // as part of the accumulated CTM before the text region. The injected
        // Cm should reflect the combined outer scaling × inner translation. ~keep
        let mut cs = Vec::new();

        cs.extend_from_slice(b"q\n");
        cs.extend_from_slice(b"0.1 0 0 0.1 50 50 cm\n");

        // >260KB of path filler to exceed the 256KB prescan threshold
        // and push the outer cm beyond the 4KB backward scan limit ~keep
        for i in 0..13000u32 {
            let line = format!(
                "{}.0 {}.0 m {}.0 {}.0 l n\n",
                i % 500,
                (i * 7) % 500,
                (i * 3) % 500,
                (i * 11) % 500
            );
            cs.extend_from_slice(line.as_bytes());
        }
        assert!(cs.len() > 256 * 1024, "stream must exceed 256KB prescan threshold");

        cs.extend_from_slice(b"q\n");
        cs.extend_from_slice(b"1 0 0 1 3000 4000 cm\n");
        cs.extend_from_slice(b"BT\n");
        cs.extend_from_slice(b"/F1 80 Tf\n");
        cs.extend_from_slice(b"(Test) Tj\n");
        cs.extend_from_slice(b"ET\n");
        cs.extend_from_slice(b"Q\n");
        cs.extend_from_slice(b"Q\n");

        let mut ops = Vec::new();
        parse_and_execute_text_only(&cs, |op| {
            ops.push(op);
            Ok(())
        })
        .unwrap();

        // The injected Cm should reflect the accumulated CTM at the BT position:
        // outer scaling (0.1) × inner translation (3000, 4000).
        // Net: a=0.1, d=0.1, e=0.1*3000+50=350, f=0.1*4000+50=450 ~keep
        let cm_ops: Vec<_> = ops.iter().filter(|op| matches!(op, Operator::Cm { .. })).collect();
        assert!(!cm_ops.is_empty(), "expected at least 1 Cm operator (injected CTM)");

        let has_scaled_cm = cm_ops
            .iter()
            .any(|op| matches!(op, Operator::Cm { a, d, .. } if (*a - 0.1).abs() < 0.01 && (*d - 0.1).abs() < 0.02));
        assert!(
            has_scaled_cm,
            "injected Cm should include outer 0.1x scaling, got: {:?}",
            cm_ops
        );
    }

    #[test]
    fn test_prescan_captures_top_level_cm_with_no_enclosing_q() {
        // Regression test: a common CAD-exporter pattern issues a single
        // top-level `cm` (no enclosing q/Q at all) right at
        // the start of the stream, with text shortly after — within the
        // same <4KB window `find_region_start` scans backward from BT.
        // That scan only tracks q/Q; when it found no unmatched q and
        // reached all the way back to byte 0, it reported "nothing
        // missing" (hit_limit == false), silently discarding the leading
        // cm. The resulting text region started exactly at BT — excluding
        // the cm's bytes entirely — and no forward-scan fallback ever ran
        // to recover it, so the text was parsed under an identity CTM
        // instead of the real ~16.67x scale (matching the reported
        // extract_spans-vs-extract_chars position mismatch). ~keep
        let mut cs = Vec::new();

        // Top-level scaling cm — NO enclosing q, unlike the other prescan
        // tests in this module. ~keep
        cs.extend_from_slice(b"16.6666667 0 0 16.6666667 0 0 cm\n");

        cs.extend_from_slice(b"BT\n");
        cs.extend_from_slice(b"/F1 80 Tf\n");
        cs.extend_from_slice(b"(G0.2) Tj\n");
        cs.extend_from_slice(b"ET\n");

        // >256KB of filler AFTER the text, so the overall stream crosses
        // the prescan fast-path threshold while the BT position itself
        // stays well within the first 4KB find_region_start scans. ~keep
        for i in 0..13000u32 {
            let line = format!(
                "{}.0 {}.0 m {}.0 {}.0 l n\n",
                i % 500,
                (i * 7) % 500,
                (i * 3) % 500,
                (i * 11) % 500
            );
            cs.extend_from_slice(line.as_bytes());
        }
        assert!(cs.len() > 256 * 1024, "stream must exceed 256KB prescan threshold");

        let mut ops = Vec::new();
        parse_and_execute_text_only(&cs, |op| {
            ops.push(op);
            Ok(())
        })
        .unwrap();

        let cm_ops: Vec<_> = ops.iter().filter(|op| matches!(op, Operator::Cm { .. })).collect();
        let has_scaled_cm = cm_ops.iter().any(|op| {
            matches!(
                op,
                Operator::Cm { a, d, .. }
                    if (*a - 16.666_666).abs() < 0.01 && (*d - 16.666_666).abs() < 0.01
            )
        });
        assert!(
            has_scaled_cm,
            "the top-level 16.6667x cm (no enclosing q) must not be silently \
             dropped, got: {:?}",
            cm_ops
        );
    }

    #[test]
    fn test_prescan_forward_ctm_injects_font_state() {
        // When BT blocks share font state (second BT has no Tf), the forward
        // CTM scan should track the font and inject it so font_size is correct. ~keep
        let mut cs = Vec::new();

        cs.extend_from_slice(b"q\n");
        cs.extend_from_slice(b"0.1 0 0 0.1 50 50 cm\n");

        for i in 0..13000u32 {
            let line = format!(
                "{}.0 {}.0 m {}.0 {}.0 l n\n",
                i % 500,
                (i * 7) % 500,
                (i * 3) % 500,
                (i * 11) % 500
            );
            cs.extend_from_slice(line.as_bytes());
        }
        assert!(cs.len() > 256 * 1024);

        cs.extend_from_slice(b"q\n");
        cs.extend_from_slice(b"1 0 0 1 1000 2000 cm\n");
        cs.extend_from_slice(b"BT\n");
        cs.extend_from_slice(b"/F1 80 Tf\n");
        cs.extend_from_slice(b"(First) Tj\n");
        cs.extend_from_slice(b"ET\n");

        // Second BT inherits font (no Tf) — same q scope ~keep
        cs.extend_from_slice(b"BT\n");
        cs.extend_from_slice(b"(Second) Tj\n");
        cs.extend_from_slice(b"ET\n");
        cs.extend_from_slice(b"Q\n");
        cs.extend_from_slice(b"Q\n");

        let mut ops = Vec::new();
        parse_and_execute_text_only(&cs, |op| {
            ops.push(op);
            Ok(())
        })
        .unwrap();

        let tf_ops: Vec<_> = ops.iter().filter(|op| matches!(op, Operator::Tf { .. })).collect();
        assert!(
            tf_ops.len() >= 2,
            "expected Tf for both BT blocks (explicit + inherited), got {}",
            tf_ops.len()
        );
    }

    #[test]
    fn test_prescan_merged_region_injects_earliest_state() {
        // Two BT blocks close enough that marked-content extension merges them
        // into one region. The merge keeps one state for the whole span, and it
        // must be the state at the EARLIEST text position — the state in effect
        // where parsing actually starts. Keeping a later block's state would
        // apply char spacing the first block never had.
        //
        // This holds because region starts are monotonic in BT position
        // (nearest-preceding BDC is monotonic, and the pos-256 clamp only
        // tightens as pos grows), so sorting by start never reorders. The test
        // pins that invariant rather than leaving it to inspection. ~keep
        let mut cs = Vec::new();
        for i in 0..14000u32 {
            let line = format!(
                "{}.0 {}.0 m {}.0 {}.0 l n\n",
                i % 500,
                (i * 7) % 500,
                (i * 3) % 500,
                (i * 11) % 500
            );
            cs.extend_from_slice(line.as_bytes());
        }
        assert!(cs.len() > 256 * 1024);

        cs.extend_from_slice(b"3 Tc\n");
        cs.extend_from_slice(b"/Span <</MCID 0>> BDC\n");
        cs.extend_from_slice(b"BT (First) Tj ET\n");
        cs.extend_from_slice(b"7 Tc\n");
        cs.extend_from_slice(b"BT (Second) Tj ET\n");
        cs.extend_from_slice(b"EMC\n");

        let mut ops = Vec::new();
        parse_and_execute_text_only(&cs, |op| {
            ops.push(op);
            Ok(())
        })
        .unwrap();

        let first_tc = ops.iter().find_map(|op| match op {
            Operator::Tc { char_space } => Some(*char_space),
            _ => None,
        });
        assert_eq!(
            first_tc,
            Some(3.0),
            "merged region must inject the earliest block's char spacing, got {first_tc:?}"
        );
    }

    /// Build a stream with two BT regions separated by >256KB of path filler,
    /// forcing the prescan's forward-scan path. `first_region` is the body of
    /// the first BT block (state setters + show ops).
    fn two_region_prescan_stream(first_region: &[u8]) -> Vec<u8> {
        let mut cs = Vec::new();
        cs.extend_from_slice(b"BT ");
        cs.extend_from_slice(first_region);
        cs.extend_from_slice(b" ET\n");
        for i in 0..14000u32 {
            let line = format!(
                "{}.0 {}.0 m {}.0 {}.0 l n\n",
                i % 500,
                (i * 7) % 500,
                (i * 3) % 500,
                (i * 11) % 500
            );
            cs.extend_from_slice(line.as_bytes());
        }
        assert!(cs.len() > 256 * 1024, "stream must exceed 256KB prescan threshold");
        cs.extend_from_slice(b"BT (B) Tj T* (C) Tj ET\n");
        cs
    }

    /// The operators injected for the LAST region: everything between the
    /// final SaveState and the final BeginText.
    fn last_region_injected(ops: &[Operator]) -> &[Operator] {
        let last_bt = ops
            .iter()
            .rposition(|op| matches!(op, Operator::BeginText))
            .expect("no BeginText");
        let save = ops[..last_bt]
            .iter()
            .rposition(|op| matches!(op, Operator::SaveState))
            .expect("no SaveState before last BeginText");
        &ops[save..last_bt]
    }

    #[test]
    fn test_prescan_injects_inherited_text_state() {
        // Tc, Tw, and the leading persist across BT/ET (ISO 32000-1 §9.3;
        // BT resets only the text and line matrices, §9.4.1), so the second
        // region legitimately runs with the first region's values. The
        // prescan path parses each region in an isolated
        // SaveState/RestoreState wrap; without reconstruction the
        // second region's T* would step by zero leading and every advance
        // would lose Tc — glyphs land on the wrong line, slightly compressed. ~keep
        let cs = two_region_prescan_stream(b"/F1 10 Tf 0.5 Tc 0.25 Tw 0 -12 TD (A) Tj");
        let mut ops = Vec::new();
        parse_and_execute_text_only(&cs, |op| {
            ops.push(op);
            Ok(())
        })
        .unwrap();

        let injected = last_region_injected(&ops);
        assert!(
            injected
                .iter()
                .any(|op| matches!(op, Operator::Tc { char_space } if (*char_space - 0.5).abs() < 1e-6)),
            "inherited Tc not injected: {:?}",
            injected
        );
        assert!(
            injected
                .iter()
                .any(|op| matches!(op, Operator::Tw { word_space } if (*word_space - 0.25).abs() < 1e-6)),
            "inherited Tw not injected: {:?}",
            injected
        );
        assert!(
            injected
                .iter()
                .any(|op| matches!(op, Operator::TL { leading } if (*leading - 12.0).abs() < 1e-6)),
            "leading set via TD not injected: {:?}",
            injected
        );
    }

    #[test]
    fn test_prescan_injects_double_quote_spacing() {
        // `aw ac string "` persistently sets the word and character spacing
        // (ISO 32000-1 §9.4.3), exactly like `Tw`/`Tc` — a later region
        // relies on the persisted values the same way. ~keep
        let cs = two_region_prescan_stream(b"/F1 10 Tf 12 TL 1.5 0.3 (A) \"");
        let mut ops = Vec::new();
        parse_and_execute_text_only(&cs, |op| {
            ops.push(op);
            Ok(())
        })
        .unwrap();

        let injected = last_region_injected(&ops);
        assert!(
            injected
                .iter()
                .any(|op| matches!(op, Operator::Tw { word_space } if (*word_space - 1.5).abs() < 1e-6)),
            "Tw persisted by \" not injected: {:?}",
            injected
        );
        assert!(
            injected
                .iter()
                .any(|op| matches!(op, Operator::Tc { char_space } if (*char_space - 0.3).abs() < 1e-6)),
            "Tc persisted by \" not injected: {:?}",
            injected
        );
    }

    #[test]
    fn test_prescan_double_quote_hex_string_operand() {
        // The string operand of `"` may be a hex string; the numeric
        // operands must survive the <...> skip too. ~keep
        let cs = two_region_prescan_stream(b"/F1 10 Tf 12 TL 2.0 0.5 <41> \"");
        let mut ops = Vec::new();
        parse_and_execute_text_only(&cs, |op| {
            ops.push(op);
            Ok(())
        })
        .unwrap();

        let injected = last_region_injected(&ops);
        assert!(
            injected
                .iter()
                .any(|op| matches!(op, Operator::Tw { word_space } if (*word_space - 2.0).abs() < 1e-6)),
            "Tw persisted by \" with hex string not injected: {:?}",
            injected
        );
        assert!(
            injected
                .iter()
                .any(|op| matches!(op, Operator::Tc { char_space } if (*char_space - 0.5).abs() < 1e-6)),
            "Tc persisted by \" with hex string not injected: {:?}",
            injected
        );
    }

    #[test]
    fn test_prescan_does_not_inject_default_text_state() {
        let cs = two_region_prescan_stream(b"/F1 10 Tf (A) Tj");
        let mut ops = Vec::new();
        parse_and_execute_text_only(&cs, |op| {
            ops.push(op);
            Ok(())
        })
        .unwrap();

        let injected = last_region_injected(&ops);
        assert!(
            !injected.iter().any(|op| matches!(
                op,
                Operator::Tc { .. }
                    | Operator::Tw { .. }
                    | Operator::Tz { .. }
                    | Operator::TL { .. }
                    | Operator::Ts { .. }
                    | Operator::Tr { .. }
            )),
            "default text state must not be injected: {:?}",
            injected
        );
    }

    #[test]
    fn test_prescan_injects_inherited_fill_color() {
        // Glyph color reads the fill state, which persists across BT/ET like
        // the rest of the graphics state. A region relying on a color set
        // before or in an earlier region must get it re-injected, or colored
        // text silently turns black on the prescan path (the old vec parser
        // preserved color operators, so this shows up as chars_hash-only
        // sweep diffs). ~keep
        let cs = two_region_prescan_stream(b"/F1 10 Tf (A) Tj ET 0.2 0.4 0.6 rg BT (X) Tj");
        let mut ops = Vec::new();
        parse_and_execute_text_only(&cs, |op| {
            ops.push(op);
            Ok(())
        })
        .unwrap();

        let injected = last_region_injected(&ops);
        assert!(
            injected.iter().any(|op| matches!(
                op,
                Operator::SetFillRgb { r, g, b }
                    if (*r - 0.2).abs() < 1e-6 && (*g - 0.4).abs() < 1e-6 && (*b - 0.6).abs() < 1e-6
            )),
            "inherited fill color not injected: {:?}",
            injected
        );
    }

    #[test]
    fn test_prescan_scn_does_not_inherit_stale_name() {
        // `/GS0 gs 0.5 scn`: the name operand belongs to `gs`; the scanner
        // must not hand it to `scn` as a pattern name. ~keep
        let cs = two_region_prescan_stream(b"/F1 10 Tf (A) Tj ET /GS0 gs 0.5 scn BT (X) Tj");
        let mut ops = Vec::new();
        parse_and_execute_text_only(&cs, |op| {
            ops.push(op);
            Ok(())
        })
        .unwrap();
        let injected = last_region_injected(&ops);
        assert!(
            !injected
                .iter()
                .any(|op| matches!(op, Operator::SetFillColorN { name: Some(_), .. })),
            "stale name fabricated a pattern: {:?}",
            injected
        );
    }

    #[test]
    fn test_prescan_scn_with_too_many_operands_injects_nothing() {
        // A 6-component scn (DeviceN) exceeds what the rolling buffer can
        // faithfully replay; a truncated color would be plausible and wrong. ~keep
        let cs = two_region_prescan_stream(b"/F1 10 Tf (A) Tj ET 0.1 0.2 0.3 0.4 0.5 0.6 scn BT (X) Tj");
        let mut ops = Vec::new();
        parse_and_execute_text_only(&cs, |op| {
            ops.push(op);
            Ok(())
        })
        .unwrap();
        let injected = last_region_injected(&ops);
        assert!(
            !injected
                .iter()
                .any(|op| matches!(op, Operator::SetFillColor { .. } | Operator::SetFillColorN { .. })),
            "truncated color injected: {:?}",
            injected
        );
    }

    #[test]
    fn test_prescan_text_state_respects_q_restore() {
        // Text state set inside a q/Q scope is restored at Q, so it must not
        // be injected into a region that begins after the Q. ~keep
        let cs = two_region_prescan_stream(b"/F1 10 Tf (A) Tj ET q 3 Tc BT (X) Tj ET Q BT (Y) Tj");
        let mut ops = Vec::new();
        parse_and_execute_text_only(&cs, |op| {
            ops.push(op);
            Ok(())
        })
        .unwrap();

        let injected = last_region_injected(&ops);
        assert!(
            !injected.iter().any(|op| matches!(op, Operator::Tc { .. })),
            "Tc from a closed q/Q scope must not leak into a later region: {:?}",
            injected
        );
    }

    #[test]
    fn test_prescan_forward_ctm_restores_after_nested_scope() {
        let mut cs = Vec::new();

        cs.extend_from_slice(b"q\n");
        cs.extend_from_slice(b"1 0 0 1 0 0 cm\n");

        for i in 0..13000u32 {
            let line = format!(
                "{}.0 {}.0 m {}.0 {}.0 l n\n",
                i % 500,
                (i * 7) % 500,
                (i * 3) % 500,
                (i * 11) % 500
            );
            cs.extend_from_slice(line.as_bytes());
        }
        assert!(cs.len() > 256 * 1024);

        cs.extend_from_slice(b"q\n");
        cs.extend_from_slice(b"10 0 0 10 0 0 cm\n");
        cs.extend_from_slice(b"BT\n");
        cs.extend_from_slice(b"/F1 12 Tf\n");
        cs.extend_from_slice(b"(Scaled) Tj\n");
        cs.extend_from_slice(b"ET\n");
        cs.extend_from_slice(b"Q\n");

        cs.extend_from_slice(b"BT\n");
        cs.extend_from_slice(b"/F1 12 Tf\n");
        cs.extend_from_slice(b"(Normal) Tj\n");
        cs.extend_from_slice(b"ET\n");
        cs.extend_from_slice(b"Q\n");

        let mut ops = Vec::new();
        parse_and_execute_text_only(&cs, |op| {
            ops.push(op);
            Ok(())
        })
        .unwrap();

        let cm_ops: Vec<_> = ops
            .iter()
            .filter_map(|op| {
                if let Operator::Cm { a, .. } = op {
                    Some(*a)
                } else {
                    None
                }
            })
            .collect();

        assert!(cm_ops.len() >= 2, "expected at least 2 Cm operators, got {:?}", cm_ops);

        let last_cm = cm_ops.last().unwrap();
        assert!(
            (*last_cm - 1.0).abs() < 0.1,
            "CTM after nested Q should be restored to ~1.0, got {}",
            last_cm
        );
    }

    #[test]
    fn test_prescan_forward_ctm_multiple_depths() {
        let mut cs = Vec::new();

        cs.extend_from_slice(b"q\n");
        cs.extend_from_slice(b"0.5 0 0 0.5 100 100 cm\n");

        for i in 0..13000u32 {
            let line = format!(
                "{}.0 {}.0 m {}.0 {}.0 l n\n",
                i % 500,
                (i * 7) % 500,
                (i * 3) % 500,
                (i * 11) % 500
            );
            cs.extend_from_slice(line.as_bytes());
        }
        assert!(cs.len() > 256 * 1024);

        cs.extend_from_slice(b"BT\n");
        cs.extend_from_slice(b"/F1 20 Tf\n");
        cs.extend_from_slice(b"(Depth1) Tj\n");
        cs.extend_from_slice(b"ET\n");

        cs.extend_from_slice(b"q\n");
        cs.extend_from_slice(b"2 0 0 2 0 0 cm\n");
        cs.extend_from_slice(b"BT\n");
        cs.extend_from_slice(b"/F1 20 Tf\n");
        cs.extend_from_slice(b"(Depth2) Tj\n");
        cs.extend_from_slice(b"ET\n");
        cs.extend_from_slice(b"Q\n");

        cs.extend_from_slice(b"Q\n");

        let mut ops = Vec::new();
        parse_and_execute_text_only(&cs, |op| {
            ops.push(op);
            Ok(())
        })
        .unwrap();

        let cm_ops: Vec<_> = ops
            .iter()
            .filter_map(|op| {
                if let Operator::Cm { a, .. } = op {
                    Some(*a)
                } else {
                    None
                }
            })
            .collect();

        assert!(
            cm_ops.len() >= 2,
            "expected at least 2 Cm operators for 2 BT blocks, got {:?}",
            cm_ops
        );

        assert!(
            (cm_ops[0] - 0.5).abs() < 0.05,
            "First BT should get outer 0.5x CTM, got {}",
            cm_ops[0]
        );

        assert!(
            (cm_ops[1] - 1.0).abs() < 0.1,
            "Second BT should get composed 0.5×2.0=1.0 CTM, got {}",
            cm_ops[1]
        );
    }

    #[test]
    fn test_parse_recovers_from_garbage_bytes() {
        let mut stream = vec![0xFF, 0xFE, 0xFD];
        stream.extend_from_slice(b" BT (Hello) Tj ET");
        let ops = parse_content_stream(&stream).unwrap();
        assert!(ops.iter().any(|op| matches!(op, Operator::BeginText)));
        assert!(ops.iter().any(|op| matches!(op, Operator::Tj { .. })));
    }

    #[test]
    fn test_parse_truncated_inline_image() {
        let stream = b"BI /W 2 /H 2 ID AAAA";
        let ops = parse_content_stream(stream).unwrap();
        let _ = ops;
    }

    #[test]
    fn test_parse_unbalanced_bt_et() {
        let stream = b"ET";
        let ops = parse_content_stream(stream).unwrap();
        assert_eq!(ops.len(), 1);
        assert!(matches!(ops[0], Operator::EndText));
    }

    #[test]
    fn test_parse_missing_operands() {
        let stream = b"Td";
        let ops = parse_content_stream(stream).unwrap();
        assert_eq!(ops.len(), 1);
        match &ops[0] {
            Operator::Td { tx, ty } => {
                assert_eq!(*tx, 0.0);
                assert_eq!(*ty, 0.0);
            }
            _ => panic!("Expected Td with default values"),
        }
    }

    #[test]
    fn test_parse_stream_with_only_comments() {
        let stream = b"% This is a comment\n% Another comment\n";
        let ops = parse_content_stream(stream).unwrap();
        let _ = ops;
    }

    #[test]
    fn test_parse_complete_page_stream() {
        let stream = b"q\n\
            1 0 0 1 72 720 cm\n\
            /GS0 gs\n\
            BT\n\
            /F1 12 Tf\n\
            0 0 Td\n\
            14 TL\n\
            (First line) Tj\n\
            T*\n\
            (Second line) Tj\n\
            ET\n\
            Q\n\
            0 0 612 792 re\n\
            S";
        let ops = parse_content_stream(stream).unwrap();
        assert!(ops.len() >= 10);

        assert!(matches!(ops[0], Operator::SaveState));
        assert!(matches!(ops[1], Operator::Cm { .. }));
        assert!(matches!(ops[2], Operator::SetExtGState { .. }));
        assert!(matches!(ops[3], Operator::BeginText));
        assert!(ops.iter().any(|op| matches!(op, Operator::TStar)));
        assert!(ops.iter().any(|op| matches!(op, Operator::EndText)));
        assert!(ops.iter().any(|op| matches!(op, Operator::RestoreState)));
        assert!(ops.iter().any(|op| matches!(op, Operator::Rectangle { .. })));
        assert!(ops.iter().any(|op| matches!(op, Operator::Stroke)));
    }

    #[test]
    fn test_parse_mixed_text_and_graphics() {
        let stream = b"q\n\
            0 0 100 100 re W n\n\
            BT /F1 10 Tf 0 0 Td (Hello) Tj ET\n\
            1 0 0 rg\n\
            0 0 m 100 0 l 100 100 l h f\n\
            Q";
        let ops = parse_content_stream(stream).unwrap();
        assert!(ops.len() > 5);
        assert!(ops.iter().any(|op| matches!(op, Operator::BeginText)));
        assert!(ops.iter().any(|op| matches!(op, Operator::Tj { .. })));
        assert!(ops.iter().any(|op| matches!(op, Operator::MoveTo { .. })));
        assert!(ops.iter().any(|op| matches!(op, Operator::Fill)));
    }

    #[test]
    fn test_parse_tj_with_hex_strings_in_array() {
        let stream = b"[<48656C6C6F> -50 <576F726C64>] TJ";
        let ops = parse_content_stream(stream).unwrap();
        assert_eq!(ops.len(), 1);
        match &ops[0] {
            Operator::TJ { array } => {
                assert_eq!(array.len(), 3);
                match &array[0] {
                    TextElement::String(s) => assert_eq!(s, b"Hello"),
                    _ => panic!("Expected string"),
                }
                assert!(matches!(array[1], TextElement::Offset(_)));
                match &array[2] {
                    TextElement::String(s) => assert_eq!(s, b"World"),
                    _ => panic!("Expected string"),
                }
            }
            _ => panic!("Expected TJ"),
        }
    }

    #[test]
    fn test_parse_td_with_varied_whitespace() {
        let stream = b"100\t200\nTd";
        let ops = parse_content_stream(stream).unwrap();
        assert_eq!(ops.len(), 1);
        match &ops[0] {
            Operator::Td { tx, ty } => {
                assert_eq!(*tx, 100.0);
                assert_eq!(*ty, 200.0);
            }
            _ => panic!("Expected Td"),
        }
    }

    #[test]
    fn test_parse_multiple_bt_et_blocks() {
        let stream = b"BT (A) Tj ET BT (B) Tj ET BT (C) Tj ET";
        let ops = parse_content_stream(stream).unwrap();
        assert_eq!(ops.len(), 9);
        let bt_count = ops.iter().filter(|op| matches!(op, Operator::BeginText)).count();
        let et_count = ops.iter().filter(|op| matches!(op, Operator::EndText)).count();
        assert_eq!(bt_count, 3);
        assert_eq!(et_count, 3);
    }

    #[test]
    fn test_fast_parser_tf_operator() {
        let stream = b"BT /Helvetica 14.5 Tf ET";
        let ops = parse_content_stream_text_only(stream).unwrap();
        assert_eq!(ops.len(), 3);
        match &ops[1] {
            Operator::Tf { font, size } => {
                assert_eq!(font, "Helvetica");
                assert!((size - 14.5).abs() < 0.01);
            }
            _ => panic!("Expected Tf from fast parser"),
        }
    }

    #[test]
    fn test_fast_parser_td_operator() {
        let stream = b"BT 72.5 -14.0 Td ET";
        let ops = parse_content_stream_text_only(stream).unwrap();
        assert!(ops.iter().any(|op| matches!(op, Operator::Td { tx, ty }
            if (*tx - 72.5).abs() < 0.01 && (*ty - (-14.0)).abs() < 0.01)));
    }

    #[test]
    fn test_fast_parser_td_upper_operator() {
        let stream = b"BT 10 -12 TD ET";
        let ops = parse_content_stream_text_only(stream).unwrap();
        assert!(ops.iter().any(|op| matches!(op, Operator::TD { tx, ty }
            if (*tx - 10.0).abs() < 0.01 && (*ty - (-12.0)).abs() < 0.01)));
    }

    #[test]
    fn test_fast_parser_tm_operator() {
        let stream = b"BT 1 0 0 1 72 700 Tm ET";
        let ops = parse_content_stream_text_only(stream).unwrap();
        assert!(ops.iter().any(|op| matches!(op, Operator::Tm { a, b, c, d, e, f }
            if *a == 1.0 && *b == 0.0 && *c == 0.0 && *d == 1.0 && *e == 72.0 && *f == 700.0)));
    }

    #[test]
    fn test_fast_parser_tstar_operator() {
        let stream = b"BT T* ET";
        let ops = parse_content_stream_text_only(stream).unwrap();
        assert!(ops.iter().any(|op| matches!(op, Operator::TStar)));
    }

    #[test]
    fn test_fast_parser_tj_with_hex_string() {
        let stream = b"BT <48656C6C6F> Tj ET";
        let ops = parse_content_stream_text_only(stream).unwrap();
        assert!(ops.iter().any(|op| {
            if let Operator::Tj { text } = op {
                text == b"Hello"
            } else {
                false
            }
        }));
    }

    #[test]
    fn test_fast_parser_tj_array() {
        let stream = b"BT [(AB) -100 (CD)] TJ ET";
        let ops = parse_content_stream_text_only(stream).unwrap();
        assert!(
            ops.iter()
                .any(|op| matches!(op, Operator::TJ { array } if array.len() == 3))
        );
    }

    #[test]
    fn test_fast_parser_quote_operator() {
        let stream = b"BT (Line2) ' ET";
        let ops = parse_content_stream_text_only(stream).unwrap();
        assert!(ops.iter().any(|op| matches!(op, Operator::Quote { .. })));
    }

    #[test]
    fn test_fast_parser_double_quote_operator() {
        let stream = b"BT 1 2 (text) \" ET";
        let ops = parse_content_stream_text_only(stream).unwrap();
        assert!(ops.iter().any(|op| matches!(op, Operator::DoubleQuote { .. })));
    }

    #[test]
    fn test_fast_parser_color_ops_inside_bt() {
        let stream = b"BT 1 0 0 rg 0 g ET";
        let ops = parse_content_stream_text_only(stream).unwrap();
        assert!(ops.iter().any(|op| matches!(op, Operator::SetFillRgb { .. })));
        assert!(ops.iter().any(|op| matches!(op, Operator::SetFillGray { .. })));
    }

    #[test]
    fn test_fast_parser_gs_inside_bt() {
        let stream = b"BT /GS1 gs ET";
        let ops = parse_content_stream_text_only(stream).unwrap();
        assert!(
            ops.iter()
                .any(|op| matches!(op, Operator::SetExtGState { dict_name } if dict_name == "GS1"))
        );
    }

    #[test]
    fn test_fast_parser_do_inside_bt() {
        let stream = b"BT /XObj1 Do ET";
        let ops = parse_content_stream_text_only(stream).unwrap();
        assert!(
            ops.iter()
                .any(|op| matches!(op, Operator::Do { name } if name == "XObj1"))
        );
    }

    #[test]
    fn test_is_operator_start() {
        assert!(is_operator_start(b'B'));
        assert!(is_operator_start(b'a'));
        assert!(is_operator_start(b'\''));
        assert!(is_operator_start(b'"'));
        assert!(is_operator_start(b'*'));
        assert!(!is_operator_start(b'0'));
        assert!(!is_operator_start(b' '));
        assert!(!is_operator_start(b'('));
        assert!(!is_operator_start(b'/'));
    }

    #[test]
    fn test_is_whitespace() {
        // NUL (0x00) is one of the six PDF white-space chars (ISO 32000-1:2008 §7.2, Table 1).
        // ~keep
        assert!(is_whitespace(0x00));
        assert!(is_whitespace(b' '));
        assert!(is_whitespace(b'\t'));
        assert!(is_whitespace(b'\r'));
        assert!(is_whitespace(b'\n'));
        assert!(is_whitespace(0x0C));
        assert!(!is_whitespace(b'A'));
        assert!(!is_whitespace(b'0'));
    }

    #[test]
    fn test_is_whitespace_or_delimiter() {
        assert!(is_whitespace_or_delimiter(b' '));
        assert!(is_whitespace_or_delimiter(b'('));
        assert!(is_whitespace_or_delimiter(b')'));
        assert!(is_whitespace_or_delimiter(b'<'));
        assert!(is_whitespace_or_delimiter(b'>'));
        assert!(is_whitespace_or_delimiter(b'['));
        assert!(is_whitespace_or_delimiter(b']'));
        assert!(is_whitespace_or_delimiter(b'{'));
        assert!(is_whitespace_or_delimiter(b'}'));
        assert!(is_whitespace_or_delimiter(b'/'));
        assert!(is_whitespace_or_delimiter(b'%'));
        assert!(!is_whitespace_or_delimiter(b'A'));
        assert!(!is_whitespace_or_delimiter(b'0'));
    }

    #[test]
    fn test_parse_float_fast() {
        assert_eq!(parse_float_fast(b"123"), Some((123.0, 3)));
        assert_eq!(parse_float_fast(b"-45"), Some((-45.0, 3)));
        assert_eq!(parse_float_fast(b"+10"), Some((10.0, 3)));
        assert_eq!(parse_float_fast(b"0.5"), Some((0.5, 3)));
        assert_eq!(parse_float_fast(b".25"), Some((0.25, 3)));
        assert_eq!(parse_float_fast(b"-0.001"), Some((-0.001, 6)));
        assert_eq!(parse_float_fast(b"0"), Some((0.0, 1)));
        assert_eq!(parse_float_fast(b"abc"), None);
        assert_eq!(parse_float_fast(b""), None);
        assert_eq!(parse_float_fast(b"-"), None);
        assert_eq!(parse_float_fast(b"+"), None);
    }

    #[test]
    fn test_parse_literal_string_fast_simple() {
        let data = b"(Hello)";
        let result = parse_literal_string_fast(data, 0);
        assert!(result.is_some());
        let (bytes, end) = result.unwrap();
        assert_eq!(bytes, b"Hello");
        assert_eq!(end, 7);
    }

    #[test]
    fn test_parse_literal_string_fast_with_escapes() {
        let data = b"(Hello\\nWorld)";
        let result = parse_literal_string_fast(data, 0);
        assert!(result.is_some());
        let (bytes, _end) = result.unwrap();
        assert!(bytes.contains(&b'\n'));
    }

    #[test]
    fn test_parse_literal_string_fast_nested_parens() {
        let data = b"(Hello (World))";
        let result = parse_literal_string_fast(data, 0);
        assert!(result.is_some());
        let (bytes, _) = result.unwrap();
        assert_eq!(bytes, b"Hello (World)");
    }

    #[test]
    fn test_parse_literal_string_fast_octal_escape() {
        let data = b"(\\101)"; // \101 = 'A' in octal ~keep
        let result = parse_literal_string_fast(data, 0);
        assert!(result.is_some());
        let (bytes, _) = result.unwrap();
        assert_eq!(bytes, b"A");
    }

    #[test]
    fn test_parse_literal_string_fast_all_escapes() {
        let data = b"(\\n\\r\\t\\b\\f\\\\\\(\\))";
        let result = parse_literal_string_fast(data, 0);
        assert!(result.is_some());
        let (bytes, _) = result.unwrap();
        assert_eq!(bytes, &[b'\n', b'\r', b'\t', 0x08, 0x0C, b'\\', b'(', b')']);
    }

    #[test]
    fn test_parse_literal_string_fast_line_continuation_cr() {
        // Backslash-CR should be ignored (line continuation) ~keep
        let data = b"(AB\\\rCD)";
        let result = parse_literal_string_fast(data, 0);
        assert!(result.is_some());
        let (bytes, _) = result.unwrap();
        assert_eq!(bytes, b"ABCD");
    }

    #[test]
    fn test_parse_literal_string_fast_line_continuation_lf() {
        // Backslash-LF should be ignored (line continuation) ~keep
        let data = b"(AB\\\nCD)";
        let result = parse_literal_string_fast(data, 0);
        assert!(result.is_some());
        let (bytes, _) = result.unwrap();
        assert_eq!(bytes, b"ABCD");
    }

    #[test]
    fn test_parse_literal_string_fast_line_continuation_crlf() {
        // Backslash-CRLF should be ignored (line continuation) ~keep
        let data = b"(AB\\\r\nCD)";
        let result = parse_literal_string_fast(data, 0);
        assert!(result.is_some());
        let (bytes, _) = result.unwrap();
        assert_eq!(bytes, b"ABCD");
    }

    #[test]
    fn test_parse_literal_string_fast_unterminated() {
        let data = b"(Hello";
        let result = parse_literal_string_fast(data, 0);
        assert!(result.is_none());
    }

    #[test]
    fn test_parse_hex_string_fast_basic() {
        let data = b"<48656C6C6F>";
        let result = parse_hex_string_fast(data, 0);
        assert!(result.is_some());
        let (bytes, end) = result.unwrap();
        assert_eq!(bytes, b"Hello");
        assert_eq!(end, 12);
    }

    #[test]
    fn test_parse_hex_string_fast_with_whitespace() {
        let data = b"<48 65 6C 6C 6F>";
        let result = parse_hex_string_fast(data, 0);
        assert!(result.is_some());
        let (bytes, _) = result.unwrap();
        assert_eq!(bytes, b"Hello");
    }

    #[test]
    fn test_parse_hex_string_fast_odd_nibbles() {
        let data = b"<ABC>";
        let result = parse_hex_string_fast(data, 0);
        assert!(result.is_some());
        let (bytes, _) = result.unwrap();
        assert_eq!(bytes.len(), 2);
        assert_eq!(bytes[0], 0xAB);
        assert_eq!(bytes[1], 0xC0);
    }

    #[test]
    fn test_parse_hex_string_fast_empty() {
        let data = b"<>";
        let result = parse_hex_string_fast(data, 0);
        assert!(result.is_some());
        let (bytes, _) = result.unwrap();
        assert!(bytes.is_empty());
    }

    #[test]
    fn test_parse_hex_string_fast_unterminated() {
        let data = b"<4865";
        let result = parse_hex_string_fast(data, 0);
        assert!(result.is_none());
    }

    #[test]
    fn test_hex_nibble() {
        assert_eq!(hex_nibble(b'0'), Some(0));
        assert_eq!(hex_nibble(b'9'), Some(9));
        assert_eq!(hex_nibble(b'a'), Some(10));
        assert_eq!(hex_nibble(b'f'), Some(15));
        assert_eq!(hex_nibble(b'A'), Some(10));
        assert_eq!(hex_nibble(b'F'), Some(15));
        assert_eq!(hex_nibble(b'G'), None);
        assert_eq!(hex_nibble(b' '), None);
    }

    #[test]
    fn test_parse_name_fast() {
        let data = b"/Font1 12";
        let (name, end) = parse_name_fast(data, 0);
        assert_eq!(name, "Font1");
        assert_eq!(end, 6);
    }

    #[test]
    fn test_parse_name_fast_empty() {
        let data = b"/ next";
        let (name, end) = parse_name_fast(data, 0);
        assert_eq!(name, "");
        assert_eq!(end, 1);
    }

    #[test]
    fn test_parse_tj_array_fast_basic() {
        let data = b"[(AB) -100 (CD)]";
        let result = parse_tj_array_fast(data, 0);
        assert!(result.is_some());
        let (elements, end) = result.unwrap();
        assert_eq!(elements.len(), 3);
        assert_eq!(end, 16);
        assert!(matches!(&elements[0], TextElement::String(s) if s == b"AB"));
        assert!(matches!(elements[1], TextElement::Offset(f) if (f + 100.0).abs() < 0.01));
        assert!(matches!(&elements[2], TextElement::String(s) if s == b"CD"));
    }

    #[test]
    fn test_parse_tj_array_fast_with_hex() {
        let data = b"[<4142> 50 <4344>]";
        let result = parse_tj_array_fast(data, 0);
        assert!(result.is_some());
        let (elements, _) = result.unwrap();
        assert_eq!(elements.len(), 3);
    }

    #[test]
    fn test_parse_tj_array_fast_empty() {
        let data = b"[]";
        let result = parse_tj_array_fast(data, 0);
        assert!(result.is_some());
        let (elements, _) = result.unwrap();
        assert!(elements.is_empty());
    }

    #[test]
    fn test_parse_tj_array_fast_unterminated() {
        let data = b"[(AB) -100";
        let result = parse_tj_array_fast(data, 0);
        assert!(result.is_none());
    }

    #[test]
    fn test_parse_six_floats_valid() {
        let data = b"1 0 0 1 72 700";
        let result = parse_six_floats(data);
        assert!(result.is_some());
        let (a, b, c, d, e, f) = result.unwrap();
        assert_eq!(a, 1.0);
        assert_eq!(b, 0.0);
        assert_eq!(c, 0.0);
        assert_eq!(d, 1.0);
        assert_eq!(e, 72.0);
        assert_eq!(f, 700.0);
    }

    #[test]
    fn test_parse_six_floats_too_few() {
        let data = b"1 0 0";
        let result = parse_six_floats(data);
        assert!(result.is_none());
    }

    #[test]
    fn test_parse_six_floats_with_negatives() {
        let data = b"-1 0.5 0 -0.5 -72 700.5";
        let result = parse_six_floats(data);
        assert!(result.is_some());
        let (a, _b, _c, d, e, f) = result.unwrap();
        assert_eq!(a, -1.0);
        assert_eq!(d, -0.5);
        assert_eq!(e, -72.0);
        assert!((f - 700.5).abs() < 0.01);
    }

    #[test]
    fn test_parse_six_floats_invalid() {
        let data = b"not numbers";
        let result = parse_six_floats(data);
        assert!(result.is_none());
    }

    #[test]
    fn test_scan_to_et_basic() {
        let data = b"/F1 12 Tf (Hello) Tj ET";
        let result = scan_to_et(data);
        assert!(result.is_some());
        let remaining = result.unwrap();
        assert!(remaining.is_empty() || remaining[0] != b'E');
    }

    #[test]
    fn test_scan_to_et_with_string_containing_et() {
        let data = b"(text ET here) Tj ET";
        let result = scan_to_et(data);
        assert!(result.is_some());
    }

    #[test]
    fn test_scan_to_et_no_et() {
        let data = b"/F1 12 Tf (Hello) Tj";
        let result = scan_to_et(data);
        assert!(result.is_none());
    }

    #[test]
    fn test_scan_to_et_in_hex_string() {
        let data = b"<4554> Tj ET";
        let result = scan_to_et(data);
        assert!(result.is_some());
    }

    #[test]
    fn test_scan_graphics_region_finds_bt() {
        let data = b"100 200 m 300 400 l BT";
        let mut errors = 0usize;
        let result = scan_graphics_region(data, &mut errors);
        assert!(matches!(result, ScanResult::FoundBT { .. }));
    }

    #[test]
    fn test_scan_graphics_region_finds_bi() {
        let data = b"100 200 m BI";
        let mut errors = 0usize;
        let result = scan_graphics_region(data, &mut errors);
        assert!(matches!(result, ScanResult::InlineImage { .. }));
    }

    #[test]
    fn test_scan_graphics_region_end_of_data() {
        let data = b"100 200 300 ";
        let mut errors = 0usize;
        let result = scan_graphics_region(data, &mut errors);
        assert!(matches!(result, ScanResult::EndOfData));
    }

    #[test]
    fn test_scan_graphics_region_skips_path_ops() {
        let data = b"100 200 m 300 400 l h f BT";
        let mut errors = 0usize;
        let result = scan_graphics_region(data, &mut errors);
        assert!(matches!(result, ScanResult::FoundBT { .. }));
    }

    #[test]
    fn test_scan_graphics_region_unmatched_q() {
        let data = b"Q";
        let mut errors = 0usize;
        let result = scan_graphics_region(data, &mut errors);
        assert!(matches!(
            result,
            ScanResult::SimpleOp {
                op: Operator::RestoreState,
                ..
            }
        ));
    }

    #[test]
    fn test_scan_graphics_region_deferred_q_with_trigger() {
        let data = b"q 1 0 0 1 0 0 cm /Im1 Do";
        let mut errors = 0usize;
        let result = scan_graphics_region(data, &mut errors);
        assert!(matches!(result, ScanResult::DeferredThenText { .. }));
    }

    #[test]
    fn test_scan_graphics_region_cm_with_inline_floats() {
        let data = b"1 0 0 1 72 700 cm";
        let mut errors = 0usize;
        let result = scan_graphics_region(data, &mut errors);
        match result {
            ScanResult::SimpleOp {
                op: Operator::Cm { a, b, c, d, e, f },
                ..
            } => {
                assert_eq!(a, 1.0);
                assert_eq!(b, 0.0);
                assert_eq!(c, 0.0);
                assert_eq!(d, 1.0);
                assert_eq!(e, 72.0);
                assert_eq!(f, 700.0);
            }
            _ => panic!(
                "Expected SimpleOp with Cm, got {:?}",
                match result {
                    ScanResult::EndOfData => "EndOfData",
                    ScanResult::FoundBT { .. } => "FoundBT",
                    ScanResult::InlineImage { .. } => "InlineImage",
                    ScanResult::NeedFullParse { .. } => "NeedFullParse",
                    ScanResult::DeferredThenText { .. } => "DeferredThenText",
                    ScanResult::SimpleOp { .. } => "SimpleOp (wrong variant)",
                    ScanResult::TooManyErrors { .. } => "TooManyErrors",
                }
            ),
        }
    }

    #[test]
    fn test_scan_graphics_region_skips_comments() {
        let data = b"% this is a comment\nBT";
        let mut errors = 0usize;
        let result = scan_graphics_region(data, &mut errors);
        assert!(matches!(result, ScanResult::FoundBT { .. }));
    }

    #[test]
    fn test_scan_graphics_region_skips_strings() {
        let data = b"(some string) BT";
        let mut errors = 0usize;
        let result = scan_graphics_region(data, &mut errors);
        // After skipping the string, should eventually find BT
        // (or NeedFullParse since string is an operand to an unknown op) ~keep
        assert!(!matches!(result, ScanResult::TooManyErrors { .. }));
    }

    #[test]
    fn test_scan_graphics_region_skips_hex_strings() {
        let data = b"<4142> BT";
        let mut errors = 0usize;
        let result = scan_graphics_region(data, &mut errors);
        assert!(!matches!(result, ScanResult::TooManyErrors { .. }));
    }

    #[test]
    fn test_scan_graphics_region_skips_arrays() {
        let data = b"[1 2 3] BT";
        let mut errors = 0usize;
        let result = scan_graphics_region(data, &mut errors);
        assert!(!matches!(result, ScanResult::TooManyErrors { .. }));
    }

    #[test]
    fn test_scan_graphics_region_skips_dicts() {
        let data = b"<< /K 1 >> BT";
        let mut errors = 0usize;
        let result = scan_graphics_region(data, &mut errors);
        assert!(!matches!(result, ScanResult::TooManyErrors { .. }));
    }

    #[test]
    fn test_scan_graphics_region_skips_names() {
        let data = b"/Name BT";
        let mut errors = 0usize;
        let result = scan_graphics_region(data, &mut errors);
        assert!(!matches!(result, ScanResult::TooManyErrors { .. }));
    }

    #[test]
    fn test_scan_graphics_region_keyword_operands() {
        let data = b"true false null BT";
        let mut errors = 0usize;
        let result = scan_graphics_region(data, &mut errors);
        assert!(matches!(result, ScanResult::FoundBT { .. }));
    }

    #[test]
    fn test_skip_literal_string_raw_basic() {
        let data = b"(Hello) rest";
        let result = skip_literal_string_raw(data, 0);
        assert_eq!(result, Some(7));
    }

    #[test]
    fn test_skip_literal_string_raw_nested() {
        let data = b"(Hello (World)) rest";
        let result = skip_literal_string_raw(data, 0);
        assert_eq!(result, Some(15));
    }

    #[test]
    fn test_skip_literal_string_raw_escaped() {
        let data = b"(Hello\\)World) rest";
        let result = skip_literal_string_raw(data, 0);
        assert_eq!(result, Some(14));
    }

    #[test]
    fn test_skip_literal_string_raw_unterminated() {
        let data = b"(Hello";
        let result = skip_literal_string_raw(data, 0);
        assert!(result.is_none());
    }

    #[test]
    fn test_skip_hex_string_raw_basic() {
        let data = b"<4142> rest";
        let result = skip_hex_string_raw(data, 0);
        assert_eq!(result, Some(6));
    }

    #[test]
    fn test_skip_hex_string_raw_unterminated() {
        let data = b"<4142";
        let result = skip_hex_string_raw(data, 0);
        assert!(result.is_none());
    }

    #[test]
    fn test_skip_name_raw_basic() {
        let data = b"/FontName 12";
        let result = skip_name_raw(data, 0);
        assert_eq!(result, 9);
    }

    #[test]
    fn test_skip_array_raw_basic() {
        let data = b"[1 2 3] rest";
        let result = skip_array_raw(data, 0);
        assert_eq!(result, Some(7));
    }

    #[test]
    fn test_skip_array_raw_nested() {
        let data = b"[1 [2 3] 4]";
        let result = skip_array_raw(data, 0);
        assert_eq!(result, Some(data.len()));
    }

    #[test]
    fn test_skip_array_raw_with_string() {
        let data = b"[(Hello) 1]";
        let result = skip_array_raw(data, 0);
        assert_eq!(result, Some(data.len()));
    }

    #[test]
    fn test_skip_array_raw_with_dict() {
        let data = b"[<< /K 1 >>]";
        let result = skip_array_raw(data, 0);
        assert_eq!(result, Some(data.len()));
    }

    #[test]
    fn test_skip_array_raw_with_hex_string() {
        let data = b"[<4142>] rest";
        let result = skip_array_raw(data, 0);
        assert_eq!(result, Some(8));
    }

    #[test]
    fn test_skip_array_raw_unterminated() {
        let data = b"[1 2 3";
        let result = skip_array_raw(data, 0);
        assert!(result.is_none());
    }

    #[test]
    fn test_skip_dict_raw_basic() {
        let data = b"<< /K 1 >>";
        let result = skip_dict_raw(data, 0);
        assert_eq!(result, Some(data.len()));
    }

    #[test]
    fn test_skip_dict_raw_nested() {
        let data = b"<< /A << /B 1 >> >>";
        let result = skip_dict_raw(data, 0);
        assert_eq!(result, Some(data.len()));
    }

    #[test]
    fn test_skip_dict_raw_with_string() {
        let data = b"<< /K (Hello) >>";
        let result = skip_dict_raw(data, 0);
        assert_eq!(result, Some(data.len()));
    }

    #[test]
    fn test_skip_dict_raw_with_hex_string() {
        let data = b"<< /K <4142> >>";
        let result = skip_dict_raw(data, 0);
        assert_eq!(result, Some(data.len()));
    }

    #[test]
    fn test_skip_dict_raw_unterminated() {
        let data = b"<< /K 1";
        let result = skip_dict_raw(data, 0);
        assert!(result.is_none());
    }

    #[test]
    fn test_is_skippable_path_construction() {
        assert!(is_skippable_graphics_op_bytes(b"m"));
        assert!(is_skippable_graphics_op_bytes(b"l"));
        assert!(is_skippable_graphics_op_bytes(b"c"));
        assert!(is_skippable_graphics_op_bytes(b"v"));
        assert!(is_skippable_graphics_op_bytes(b"y"));
        assert!(is_skippable_graphics_op_bytes(b"h"));
        assert!(is_skippable_graphics_op_bytes(b"re"));
    }

    #[test]
    fn test_is_skippable_path_painting() {
        assert!(is_skippable_graphics_op_bytes(b"S"));
        assert!(is_skippable_graphics_op_bytes(b"s"));
        assert!(is_skippable_graphics_op_bytes(b"f"));
        assert!(is_skippable_graphics_op_bytes(b"F"));
        assert!(is_skippable_graphics_op_bytes(b"f*"));
        assert!(is_skippable_graphics_op_bytes(b"B"));
        assert!(is_skippable_graphics_op_bytes(b"B*"));
        assert!(is_skippable_graphics_op_bytes(b"b"));
        assert!(is_skippable_graphics_op_bytes(b"b*"));
        assert!(is_skippable_graphics_op_bytes(b"n"));
    }

    #[test]
    fn test_is_skippable_clipping() {
        assert!(is_skippable_graphics_op_bytes(b"W"));
        assert!(is_skippable_graphics_op_bytes(b"W*"));
    }

    #[test]
    fn test_is_skippable_graphics_state() {
        assert!(is_skippable_graphics_op_bytes(b"w"));
        assert!(is_skippable_graphics_op_bytes(b"J"));
        assert!(is_skippable_graphics_op_bytes(b"j"));
        assert!(is_skippable_graphics_op_bytes(b"M"));
        assert!(is_skippable_graphics_op_bytes(b"d"));
        assert!(is_skippable_graphics_op_bytes(b"i"));
        assert!(is_skippable_graphics_op_bytes(b"ri"));
        assert!(is_skippable_graphics_op_bytes(b"sh"));
    }

    #[test]
    fn test_is_skippable_color() {
        assert!(is_skippable_graphics_op_bytes(b"rg"));
        assert!(is_skippable_graphics_op_bytes(b"RG"));
        assert!(is_skippable_graphics_op_bytes(b"g"));
        assert!(is_skippable_graphics_op_bytes(b"G"));
        assert!(is_skippable_graphics_op_bytes(b"k"));
        assert!(is_skippable_graphics_op_bytes(b"K"));
        assert!(is_skippable_graphics_op_bytes(b"cs"));
        assert!(is_skippable_graphics_op_bytes(b"CS"));
        assert!(is_skippable_graphics_op_bytes(b"sc"));
        assert!(is_skippable_graphics_op_bytes(b"SC"));
        assert!(is_skippable_graphics_op_bytes(b"scn"));
        assert!(is_skippable_graphics_op_bytes(b"SCN"));
    }

    #[test]
    fn test_is_not_skippable() {
        assert!(!is_skippable_graphics_op_bytes(b"BT"));
        assert!(!is_skippable_graphics_op_bytes(b"ET"));
        assert!(!is_skippable_graphics_op_bytes(b"Tj"));
        assert!(!is_skippable_graphics_op_bytes(b"TJ"));
        assert!(!is_skippable_graphics_op_bytes(b"Td"));
        assert!(!is_skippable_graphics_op_bytes(b"Tm"));
        assert!(!is_skippable_graphics_op_bytes(b"Tf"));
        assert!(!is_skippable_graphics_op_bytes(b"Do"));
        assert!(!is_skippable_graphics_op_bytes(b"cm"));
        assert!(!is_skippable_graphics_op_bytes(b"q"));
        assert!(!is_skippable_graphics_op_bytes(b"Q"));
        assert!(!is_skippable_graphics_op_bytes(b"gs"));
        assert!(!is_skippable_graphics_op_bytes(b"BI"));
    }

    #[test]
    fn test_byte_class_whitespace_and_digits() {
        assert_eq!(BYTE_CLASS[b' ' as usize], SCAN_SKIP);
        assert_eq!(BYTE_CLASS[b'\t' as usize], SCAN_SKIP);
        assert_eq!(BYTE_CLASS[b'\n' as usize], SCAN_SKIP);
        assert_eq!(BYTE_CLASS[b'\r' as usize], SCAN_SKIP);
        assert_eq!(BYTE_CLASS[0x0C], SCAN_SKIP);
        assert_eq!(BYTE_CLASS[0x00], SCAN_SKIP);
        for d in b'0'..=b'9' {
            assert_eq!(BYTE_CLASS[d as usize], SCAN_SKIP, "digit {} should be SKIP", d as char);
        }
        assert_eq!(BYTE_CLASS[b'.' as usize], SCAN_SKIP);
        assert_eq!(BYTE_CLASS[b'+' as usize], SCAN_SKIP);
        assert_eq!(BYTE_CLASS[b'-' as usize], SCAN_SKIP);
    }

    #[test]
    fn test_byte_class_alpha() {
        for c in b'A'..=b'Z' {
            assert_eq!(
                BYTE_CLASS[c as usize], SCAN_ALPHA,
                "uppercase {} should be ALPHA",
                c as char
            );
        }
        for c in b'a'..=b'z' {
            assert_eq!(
                BYTE_CLASS[c as usize], SCAN_ALPHA,
                "lowercase {} should be ALPHA",
                c as char
            );
        }
        assert_eq!(BYTE_CLASS[b'\'' as usize], SCAN_ALPHA);
        assert_eq!(BYTE_CLASS[b'"' as usize], SCAN_ALPHA);
        assert_eq!(BYTE_CLASS[b'*' as usize], SCAN_ALPHA);
    }

    #[test]
    fn test_byte_class_delimiters() {
        assert_eq!(BYTE_CLASS[b'(' as usize], SCAN_PAREN);
        assert_eq!(BYTE_CLASS[b'<' as usize], SCAN_ANGLE);
        assert_eq!(BYTE_CLASS[b'[' as usize], SCAN_BRACKET);
        assert_eq!(BYTE_CLASS[b'/' as usize], SCAN_SLASH);
        assert_eq!(BYTE_CLASS[b'%' as usize], SCAN_PERCENT);
    }

    #[test]
    fn test_find_ei_operator_basic() {
        let data = b"binary data \nEI ";
        let result = find_ei_operator(data);
        assert!(result.is_ok());
    }

    #[test]
    fn test_find_ei_operator_at_end() {
        let data = b"data \nEI";
        let result = find_ei_operator(data);
        assert!(result.is_ok());
    }

    #[test]
    fn test_find_ei_operator_not_found() {
        let data = b"binary data without end marker";
        let result = find_ei_operator(data);
        assert!(result.is_err());
    }

    #[test]
    fn test_find_ei_operator_ei_without_whitespace_prefix() {
        let data = b"dataEI ";
        let result = find_ei_operator(data);
        assert!(result.is_err());
    }

    #[test]
    fn test_text_only_operator_limit() {
        let count = super::MAX_OPERATORS + 500;
        let mut stream: Vec<u8> = Vec::new();
        stream.extend_from_slice(b"BT\n");
        for _ in 0..count {
            stream.extend_from_slice(b"T*\n");
        }
        stream.extend_from_slice(b"ET\n");
        let ops = parse_content_stream_text_only(&stream).unwrap();
        assert!(ops.len() <= super::MAX_OPERATORS + 1); // +1 for possible BT ~keep
    }

    #[test]
    fn test_images_only_operator_limit() {
        let count = super::MAX_OPERATORS + 500;
        let stream: Vec<u8> = "q\n".repeat(count).into_bytes();
        let ops = parse_content_stream_images_only(&stream).unwrap();
        assert!(ops.len() <= super::MAX_OPERATORS);
    }

    #[test]
    fn test_full_and_text_only_agree_on_text_operators() {
        let stream = b"BT /F1 12 Tf 72 700 Td (Test) Tj T* (Line2) Tj ET";
        let full = parse_content_stream(stream).unwrap();
        let text_only = parse_content_stream_text_only(stream).unwrap();
        assert_eq!(full.len(), text_only.len());
        for (f, t) in full.iter().zip(text_only.iter()) {
            assert_eq!(f, t);
        }
    }

    #[test]
    fn test_full_parse_and_execute_agree() {
        let stream = b"BT /F1 12 Tf (Hello) Tj ET";
        let full = parse_content_stream(stream).unwrap();
        let mut exec_ops = Vec::new();
        parse_and_execute_text_only(stream, |op| {
            exec_ops.push(op);
            Ok(())
        })
        .unwrap();
        assert_eq!(full.len(), exec_ops.len());
    }

    #[test]
    fn test_skip_operand_token_dict_with_strings() {
        assert_eq!(skip_operand_token(b"<< /Key (value) >> ").unwrap().0, b" ");
    }

    #[test]
    fn test_skip_operand_token_nested_array() {
        assert_eq!(skip_operand_token(b"[1 [2 3] 4] ").unwrap().0, b" ");
    }

    #[test]
    fn test_skip_operand_token_array_with_hex_string() {
        assert_eq!(skip_operand_token(b"[<4142> 1] ").unwrap().0, b" ");
    }

    #[test]
    fn test_skip_operand_token_array_with_dict() {
        assert_eq!(skip_operand_token(b"[<< /K 1 >>] ").unwrap().0, b" ");
    }

    #[test]
    fn test_skip_operand_token_dict_with_nested_dict() {
        assert_eq!(skip_operand_token(b"<< /A << /B 1 >> >> ").unwrap().0, b" ");
    }

    #[test]
    fn test_skip_operand_token_dict_with_hex_string() {
        assert_eq!(skip_operand_token(b"<< /K <4142> >> ").unwrap().0, b" ");
    }

    #[test]
    fn test_skip_operand_token_errors() {
        assert!(skip_operand_token(b"").is_err());
        assert!(skip_operand_token(b"@").is_err());
    }

    #[test]
    fn test_prescan_forward_ctm_preserves_marked_content() {
        // Regression test: when prescan forward CTM builds regions starting at BT,
        // preceding BDC/BMC operators must be preserved so tagged PDF structure
        // (BDC ... BT ... ET ... EMC) is not broken. ~keep
        let mut cs = Vec::new();

        cs.extend_from_slice(b"q\n");
        cs.extend_from_slice(b"1.5 0 0 1.5 50 50 cm\n");

        for i in 0..13000u32 {
            let line = format!(
                "{}.0 {}.0 m {}.0 {}.0 l n\n",
                i % 500,
                (i * 7) % 500,
                (i * 3) % 500,
                (i * 11) % 500
            );
            cs.extend_from_slice(line.as_bytes());
        }
        assert!(cs.len() > 256 * 1024);

        cs.extend_from_slice(b"/Span << /MCID 0 >> BDC\n");
        cs.extend_from_slice(b"BT\n");
        cs.extend_from_slice(b"/F1 12 Tf\n");
        cs.extend_from_slice(b"(Hello tagged) Tj\n");
        cs.extend_from_slice(b"ET\n");
        cs.extend_from_slice(b"EMC\n");

        cs.extend_from_slice(b"Q\n");

        let mut ops = Vec::new();
        parse_and_execute_text_only(&cs, |op| {
            ops.push(op);
            Ok(())
        })
        .unwrap();

        let has_bdc = ops
            .iter()
            .any(|op| matches!(op, Operator::BeginMarkedContentDict { tag, .. } if tag == "Span"));
        assert!(
            has_bdc,
            "BeginMarkedContentDict(/Span) must be preserved; got ops: {:?}",
            ops.iter()
                .map(|op| format!("{:?}", std::mem::discriminant(op)))
                .collect::<Vec<_>>()
        );

        let has_emc = ops.iter().any(|op| matches!(op, Operator::EndMarkedContent));
        assert!(has_emc, "EndMarkedContent (EMC) must be preserved in tagged PDF output");
    }
}
