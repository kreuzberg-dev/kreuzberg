//! End-to-end diagram recovery from vector PDF (#579).
//!
//! The diagram is built here rather than read from the corpus. A vector PDF
//! fixture would have to be a binary, and building the drawing in the test
//! makes the input readable: the operators below are the whole diagram, so a
//! failure points at a coordinate someone can see rather than at bytes.
//!
//! Everything runs through the public extraction API with `output_format="dot"`,
//! so this covers the hook into the PDF extractor as well as the recogniser.

#![cfg(all(feature = "pdf", feature = "xml"))]

mod helpers;
use helpers::extract_bytes_document_blocking;

use xberg::core::config::{ExtractionConfig, OutputFormat};

/// Two stroked boxes joined by a line that ends in a filled arrowhead, with a
/// label centred in each box.
///
/// Coordinates are PDF user space, so the origin is the bottom-left corner and
/// `Alpha` at y=150 is the *upper* box. Recovery has to flip that; the
/// assertions below are written in reading order and would fail if it did not.
fn two_boxes_and_an_arrow() -> Vec<u8> {
    use lopdf::{Document, Object, Stream, dictionary};

    // Boxes are stroked with `S` and the arrowhead filled with `f`. `B`
    // (fill-and-stroke) is deliberately avoided: pdf_oxide 0.3.77 does not
    // finalize a path on `B`, so its geometry leaks into the next painted path
    // and the fixture would be testing that bug rather than this recogniser.
    let content = b"1 w 0 0 0 RG
10 150 80 30 re S
10 20 80 30 re S
50 150 m 50 56 l S
46 56 m 54 56 l 50 50 l h f
BT /F1 12 Tf 30 160 Td (Alpha) Tj ET
BT /F1 12 Tf 30 30 Td (Beta) Tj ET
"
    .to_vec();

    let mut document = Document::with_version("1.5");
    let pages_id = document.new_object_id();
    let font_id = document.add_object(dictionary! {
        "Type" => "Font",
        "Subtype" => "Type1",
        "BaseFont" => "Helvetica",
    });
    let content_id = document.add_object(Stream::new(dictionary! {}, content));
    let page_id = document.add_object(dictionary! {
        "Type" => "Page",
        "Parent" => pages_id,
        "Contents" => content_id,
        "Resources" => dictionary! { "Font" => dictionary! { "F1" => font_id } },
        "MediaBox" => vec![0.into(), 0.into(), 200.into(), 200.into()],
    });
    document.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! {
            "Type" => "Pages",
            "Kids" => vec![page_id.into()],
            "Count" => 1,
        }),
    );
    let catalog_id = document.add_object(dictionary! {
        "Type" => "Catalog",
        "Pages" => pages_id,
    });
    document.trailer.set("Root", catalog_id);

    let mut bytes = Vec::new();
    document.save_to(&mut bytes).expect("fixture PDF must save");
    bytes
}

fn extract_dot(bytes: &[u8]) -> String {
    let config = ExtractionConfig {
        output_format: OutputFormat::Custom("dot".to_string()),
        ..Default::default()
    };
    extract_bytes_document_blocking(bytes, "application/pdf", &config)
        .expect("extraction must succeed")
        .content
}

#[test]
fn recovers_a_graph_from_vector_pdf() {
    let dot = extract_dot(&two_boxes_and_an_arrow());

    assert!(dot.contains("digraph"), "expected a graph, got:\n{dot}");
    assert!(dot.contains(r#"label="Alpha""#), "missing Alpha:\n{dot}");
    assert!(dot.contains(r#"label="Beta""#), "missing Beta:\n{dot}");
    assert!(dot.contains("shape=box"), "boxes must be named as boxes:\n{dot}");

    // Reading order decides the ids, so the upper box is n0. An unflipped
    // y-axis would number them the other way round and reverse this edge.
    assert!(dot.contains("n0 -> n1"), "expected one edge Alpha to Beta:\n{dot}");
    assert_eq!(dot.matches("->").count(), 1, "expected exactly one edge:\n{dot}");
}

/// A PDF of prose draws no graph, and reporting one would be a false positive
/// on the most common input the extractor sees.
#[test]
fn a_text_only_pdf_recovers_nothing() {
    use lopdf::{Document, Object, Stream, dictionary};

    let mut document = Document::with_version("1.5");
    let pages_id = document.new_object_id();
    let font_id = document.add_object(dictionary! {
        "Type" => "Font",
        "Subtype" => "Type1",
        "BaseFont" => "Helvetica",
    });
    let content_id = document.add_object(Stream::new(
        dictionary! {},
        b"BT /F1 12 Tf 20 100 Td (Just a paragraph of ordinary text.) Tj ET\n".to_vec(),
    ));
    let page_id = document.add_object(dictionary! {
        "Type" => "Page",
        "Parent" => pages_id,
        "Contents" => content_id,
        "Resources" => dictionary! { "Font" => dictionary! { "F1" => font_id } },
        "MediaBox" => vec![0.into(), 0.into(), 200.into(), 200.into()],
    });
    document.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! {
            "Type" => "Pages",
            "Kids" => vec![page_id.into()],
            "Count" => 1,
        }),
    );
    let catalog_id = document.add_object(dictionary! {
        "Type" => "Catalog",
        "Pages" => pages_id,
    });
    document.trailer.set("Root", catalog_id);

    let mut bytes = Vec::new();
    document.save_to(&mut bytes).expect("fixture PDF must save");

    assert!(extract_dot(&bytes).trim().is_empty(), "prose is not a diagram");
}
