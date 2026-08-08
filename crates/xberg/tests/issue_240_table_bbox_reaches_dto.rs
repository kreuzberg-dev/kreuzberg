//! Regression test for xberg-io/xberg#240 — `push_table_from_cells` received a
//! bounding box but never put it on the `Table` DTO.
//!
//! The builder passed `bbox` through to the *element*, then constructed the
//! `Table` with `bounding_box: None` hard-coded. `Table::bounding_box` is a
//! serialized public field, so every table built through this path was published
//! to consumers without its geometry even when the caller knew it.
//!
//! Run with:
//!   cargo test -p xberg --test issue_240_table_bbox_reaches_dto
//! (no `--features` needed: the builder is ungated)

use xberg::types::extraction::BoundingBox;
use xberg::types::internal_builder::InternalDocumentBuilder;

/// Build a grid from string literals without the `to_string()` noise.
fn grid(rows: &[&[&str]]) -> Vec<Vec<String>> {
    rows.iter()
        .map(|row| row.iter().map(|c| (*c).to_string()).collect())
        .collect()
}

#[test]
fn should_put_the_bounding_box_on_the_table_dto() {
    let bbox = BoundingBox {
        x0: 50.0,
        y0: 100.0,
        x1: 500.0,
        y1: 700.0,
    };
    let cells = grid(&[&["Name", "Age"], &["Alice", "30"]]);

    let mut builder = InternalDocumentBuilder::new("test");
    builder.push_table_from_cells(&cells, Some(3), Some(bbox));
    let doc = builder.build();

    assert_eq!(doc.tables.len(), 1, "expected exactly one table");
    assert_eq!(
        doc.tables[0].bounding_box,
        Some(bbox),
        "the caller's bounding box must reach Table::bounding_box"
    );
    assert_eq!(doc.tables[0].page_number, 3, "the page number must survive too");
}

#[test]
fn should_leave_the_bounding_box_unset_when_the_caller_has_none() {
    let cells = grid(&[&["Name", "Age"], &["Alice", "30"]]);

    let mut builder = InternalDocumentBuilder::new("test");
    builder.push_table_from_cells(&cells, None, None);
    let doc = builder.build();

    assert_eq!(doc.tables.len(), 1, "expected exactly one table");
    assert_eq!(
        doc.tables[0].bounding_box, None,
        "no geometry must be invented when the caller supplied none"
    );
}

#[test]
fn should_carry_the_bounding_box_onto_the_element_as_well() {
    let bbox = BoundingBox {
        x0: 1.0,
        y0: 2.0,
        x1: 3.0,
        y1: 4.0,
    };
    let cells = grid(&[&["A"], &["1"]]);

    let mut builder = InternalDocumentBuilder::new("test");
    let element_index = builder.push_table_from_cells(&cells, None, Some(bbox));
    let doc = builder.build();

    assert_eq!(
        doc.elements[element_index as usize].bbox,
        Some(bbox),
        "the element keeps its bounding box (unchanged behaviour)"
    );
}
