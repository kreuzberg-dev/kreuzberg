//! Pins the `Face::set_variation` unknown-axis contract restored by upstream
//! harfbuzz/ttf-parser#207.
//!
//! xberg-added, kept out of `tests/tables/` so every file cherry-picked from upstream
//! stays byte-identical to it.

use ttf_parser::{Face, Tag};

/// 44 variation axes, well under `MAX_VAR_COORDS`, so `set_variation` reaches its loop.
const VARIABLE: &[u8] = include_bytes!("fonts/colr_1_variable.ttf");
const STATIC: &[u8] = include_bytes!("fonts/demo.ttf");

/// The case #207 actually fixes: before it, the loop matched nothing and still
/// reported `Some(())`. Reverting the fix turns this red.
#[test]
fn unknown_axis_returns_none() {
    let mut face = Face::parse(VARIABLE, 0).unwrap();
    assert_eq!(face.set_variation(Tag::from_bytes(b"ZZZZ"), 1.0), None);
}

#[test]
fn known_axis_returns_some() {
    let mut face = Face::parse(VARIABLE, 0).unwrap();
    assert_eq!(face.set_variation(Tag::from_bytes(b"SWPS"), 45.0), Some(()));
}

/// Already handled by the `!self.is_variable()` guard ahead of the loop, so this one
/// passes with or without #207. Kept because it pins the other half of the documented
/// contract, not because it exercises the fix.
#[test]
fn non_variable_face_returns_none() {
    let mut face = Face::parse(STATIC, 0).unwrap();
    assert_eq!(face.set_variation(Tag::from_bytes(b"wght"), 700.0), None);
}
