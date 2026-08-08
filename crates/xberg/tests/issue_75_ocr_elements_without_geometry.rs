//! Regression coverage for #75: OCR elements that carry no bounding geometry must
//! be kept (with a zero bounding box) rather than discarded.
//!
//! `build_ocr_elements` in `extraction::derive` used to short-circuit on
//! `elem.ocr_geometry.clone()?`, which silently dropped every recognised word or
//! block whose backend reported no box — VLM OCR and hOCR without `bbox`
//! properties produce exactly that. A word with no bbox is still a word.

use xberg::OutputFormat;
use xberg::extraction::derive::derive_extraction_result;
use xberg::types::internal::{ElementKind, InternalDocument, InternalElement};
use xberg::types::ocr_elements::{OcrBoundingGeometry, OcrConfidence, OcrElementLevel};

fn ocr_element(text: &str, geometry: Option<OcrBoundingGeometry>, page: u32) -> InternalElement {
    let mut element = InternalElement::text(
        ElementKind::OcrText {
            level: OcrElementLevel::Word,
        },
        text,
        0,
    );
    element.page = Some(page);
    element.ocr_geometry = geometry;
    element.ocr_confidence = Some(OcrConfidence {
        detection: None,
        recognition: 0.5,
    });
    element
}

#[test]
fn should_keep_ocr_element_when_geometry_is_missing() {
    let mut doc = InternalDocument::new("pdf");
    doc.mime_type = "application/pdf".to_string();
    doc.push_element(ocr_element(
        "boxed",
        Some(OcrBoundingGeometry::Rectangle {
            left: 10,
            top: 20,
            width: 30,
            height: 40,
        }),
        1,
    ));
    doc.push_element(ocr_element("unboxed", None, 1));

    let result = derive_extraction_result(doc, false, OutputFormat::Plain);
    let elements = result.ocr_elements.expect("OCR elements must be derived");

    assert_eq!(
        elements.len(),
        2,
        "the geometry-less element must be kept, not discarded"
    );
    assert_eq!(elements[0].text, "boxed");
    assert_eq!(
        elements[0].geometry,
        OcrBoundingGeometry::Rectangle {
            left: 10,
            top: 20,
            width: 30,
            height: 40,
        }
    );
    assert_eq!(elements[1].text, "unboxed");
    assert_eq!(
        elements[1].geometry,
        OcrBoundingGeometry::Rectangle {
            left: 0,
            top: 0,
            width: 0,
            height: 0,
        },
        "an element without geometry is kept with a zero bounding box"
    );
    assert_eq!(elements[1].level, OcrElementLevel::Word);
    assert_eq!(elements[1].page_number, 1);
    assert_eq!(elements[1].confidence.recognition, 0.5);
}

#[test]
fn should_derive_no_ocr_elements_when_document_has_none() {
    let mut doc = InternalDocument::new("pdf");
    doc.mime_type = "application/pdf".to_string();
    doc.push_element(InternalElement::text(ElementKind::Paragraph, "plain prose", 0));

    let result = derive_extraction_result(doc, false, OutputFormat::Plain);

    assert!(result.ocr_elements.is_none());
}
