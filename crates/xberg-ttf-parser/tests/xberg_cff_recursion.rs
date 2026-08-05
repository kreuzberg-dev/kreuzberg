//! Pins the total-visit budget on the CFF charstring interpreter.
//!
//! xberg-added. Upstream #224 / #225 cap total visits in glyf/gvar composite outlining
//! and in COLRv1 painting, but the CFF charstring interpreter had no equivalent: a glyph
//! whose charstring calls a subroutine over and over drives one `_parse_char_string`
//! invocation per call, bounded only by the per-path nesting limit, so a crafted font can
//! force unbounded work on the exact path a PDF renderer uses most.
//!
//! The regression font is a single glyph that calls one global subroutine far more times
//! than the budget allows. The subroutine just returns, so every call is flat (depth 1)
//! and slips past the nesting limit. With the budget in place, outlining the glyph gives
//! up and returns an error; without it, the glyph outlines successfully after doing all
//! the work. The assertion is on the return value, not on timing, so it is deterministic:
//! reverting the fix turns this red because the outline then succeeds.
//!
//! Kept out of `tests/tables/` so every file cherry-picked from upstream stays
//! byte-identical to it.

use ttf_parser::{cff, GlyphId, OutlineBuilder};

/// One more subroutine call than `MAX_CHARSTRING_VISITS` (100_000) allows, so the budget
/// is exhausted before the glyph reaches its `endchar`.
const CALL_COUNT: usize = 150_000;

// CFF1 charstring operators used below.
const MOVE_TO: u8 = 21; // rmoveto
const ENDCHAR: u8 = 14;
const RETURN: u8 = 11;
const CALL_GLOBAL_SUBROUTINE: u8 = 29;

struct Sink;
impl OutlineBuilder for Sink {
    fn move_to(&mut self, _: f32, _: f32) {}
    fn line_to(&mut self, _: f32, _: f32) {}
    fn quad_to(&mut self, _: f32, _: f32, _: f32, _: f32) {}
    fn curve_to(&mut self, _: f32, _: f32, _: f32, _: f32, _: f32, _: f32) {}
    fn close(&mut self) {}
}

/// Encodes a CFF DICT/charstring integer operand (Type 2 charstring number encoding).
fn push_cff_int(out: &mut Vec<u8>, n: i32) {
    match n {
        -107..=107 => out.push((n + 139) as u8),
        108..=1131 => {
            let n = n - 108;
            out.push(((n >> 8) + 247) as u8);
            out.push((n & 0xFF) as u8);
        }
        -1131..=-108 => {
            let n = -n - 108;
            out.push(((n >> 8) + 251) as u8);
            out.push((n & 0xFF) as u8);
        }
        -32768..=32767 => {
            out.push(28);
            out.extend_from_slice(&(n as i16).to_be_bytes());
        }
        _ => {
            out.push(29);
            out.extend_from_slice(&n.to_be_bytes());
        }
    }
}

/// Builds a minimal CFF1 font with one global subroutine and one glyph whose charstring
/// calls that subroutine `CALL_COUNT` times before its `endchar`.
///
/// Layout mirrors the vendored test harness in `tests/tables/cff1.rs`, with the CharString
/// INDEX widened to 4-byte offsets because the glyph charstring is far larger than 255
/// bytes. The Top DICT still records the CharString INDEX at offset 21, which the wider
/// offsets do not move, since they sit after the Top DICT, String INDEX and Global Subr
/// INDEX.
fn recursion_bomb_cff() -> Vec<u8> {
    // Global subroutine 0: just `return`. With one global subr the CFF1 bias is 107, so
    // operand -107 selects index 0.
    let subr: [u8; 1] = [RETURN];

    // Glyph charstring: a valid move, then CALL_COUNT flat subroutine calls, then endchar.
    let mut glyph = Vec::with_capacity(CALL_COUNT * 2 + 8);
    push_cff_int(&mut glyph, 10);
    push_cff_int(&mut glyph, 20);
    glyph.push(MOVE_TO);
    for _ in 0..CALL_COUNT {
        push_cff_int(&mut glyph, -107); // -107 + bias(107) = subr index 0
        glyph.push(CALL_GLOBAL_SUBROUTINE);
    }
    glyph.push(ENDCHAR);

    let charstr_offset: u32 = 21;

    let mut w: Vec<u8> = Vec::new();
    // Header.
    w.extend_from_slice(&[1, 0, 4, 0]); // major, minor, header size, absolute offset size
    // Name INDEX (empty).
    w.extend_from_slice(&0u16.to_be_bytes());
    // Top DICT INDEX: one entry, 1-byte offsets.
    w.extend_from_slice(&1u16.to_be_bytes()); // count
    w.push(1); // offSize
    w.push(1); // offset[0]
    w.push(3); // offset[1]
    push_cff_int(&mut w, charstr_offset as i32);
    w.push(17); // CharStrings offset operator
    // String INDEX (empty).
    w.extend_from_slice(&0u16.to_be_bytes());
    // Global Subroutines INDEX: one entry, 1-byte offsets.
    w.extend_from_slice(&1u16.to_be_bytes()); // count
    w.push(1); // offSize
    w.push(1); // offset[0]
    w.push(subr.len() as u8 + 1); // offset[1]
    w.extend_from_slice(&subr);
    // CharString INDEX: one entry, 4-byte offsets for the large glyph.
    assert_eq!(w.len() as u32, charstr_offset, "CharString INDEX must start at offset 21");
    w.extend_from_slice(&1u16.to_be_bytes()); // count
    w.push(4); // offSize
    w.extend_from_slice(&1u32.to_be_bytes()); // offset[0]
    w.extend_from_slice(&(glyph.len() as u32 + 1).to_be_bytes()); // offset[1]
    w.extend_from_slice(&glyph);

    w
}

#[test]
fn cff_charstring_recursion_is_budgeted() {
    let data = recursion_bomb_cff();
    let table = cff::Table::parse(&data).expect("synthetic CFF must parse");
    // Budget exhausted before `endchar`, so the glyph fails to outline. Without the budget
    // the same call returns `Ok`, which is what makes this a red-provable regression.
    assert!(table.outline(GlyphId(0), &mut Sink).is_err());
}
