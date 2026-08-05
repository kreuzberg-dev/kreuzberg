//! Pins the `fvar` HIDDEN_AXIS decoding fixed by upstream harfbuzz/ttf-parser#216.
//!
//! xberg-added, kept out of `tests/tables/` so every file cherry-picked from upstream
//! stays byte-identical to it.

use ttf_parser::fvar::Table;

/// Builds a minimal single-axis `fvar` table with the given axis record flags.
fn fvar_with_flags(flags: u16) -> Vec<u8> {
    let mut data = Vec::new();
    // Header.
    data.extend_from_slice(&0x0001_0000u32.to_be_bytes()); // version 1.0
    data.extend_from_slice(&16u16.to_be_bytes()); // axesArrayOffset
    data.extend_from_slice(&2u16.to_be_bytes()); // reserved
    data.extend_from_slice(&1u16.to_be_bytes()); // axisCount
    data.extend_from_slice(&20u16.to_be_bytes()); // axisSize
    data.extend_from_slice(&0u16.to_be_bytes()); // instanceCount
    data.extend_from_slice(&0u16.to_be_bytes()); // instanceSize
    // One VariationAxisRecord.
    data.extend_from_slice(b"wght");
    data.extend_from_slice(&(100i32 << 16).to_be_bytes()); // minValue
    data.extend_from_slice(&(400i32 << 16).to_be_bytes()); // defaultValue
    data.extend_from_slice(&(900i32 << 16).to_be_bytes()); // maxValue
    data.extend_from_slice(&flags.to_be_bytes());
    data.extend_from_slice(&256u16.to_be_bytes()); // axisNameID
    data
}

fn hidden_for_flags(flags: u16) -> bool {
    let data = fvar_with_flags(flags);
    let table = Table::parse(&data).unwrap();
    table.axes.get(0).unwrap().hidden
}

#[test]
fn hidden_axis_is_bit_0() {
    assert!(hidden_for_flags(0x0001));
}

#[test]
fn visible_axis_has_no_flags_set() {
    assert!(!hidden_for_flags(0x0000));
}

/// Bit 3 is reserved and carries no meaning. Reading it as HIDDEN_AXIS is the
/// defect #216 fixes, so this case pins the change rather than smoke-testing it.
#[test]
fn reserved_bit_3_does_not_mean_hidden() {
    assert!(!hidden_for_flags(0x0008));
}
