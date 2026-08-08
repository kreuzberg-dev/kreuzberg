//! Regression test for defect #70.
//!
//! `build_pages` grouped every page-tagged element into `PageContent.content` with no
//! `ContentLayer` filter, while `render_plain` skips everything that is not
//! `ContentLayer::Body`. For `OutputFormat::Plain` the two disagreed: running headers and
//! footers were absent from `result.content` but present in `result.pages[n].content`.

use xberg::ContentLayer;
use xberg::core::config::OutputFormat;
use xberg::extraction::derive::derive_extraction_result;
use xberg::types::internal::{ElementKind, InternalDocument, InternalElement};

fn page_with_furniture() -> InternalDocument {
    let mut doc = InternalDocument::new("pdf");

    let mut header = InternalElement::text(ElementKind::Paragraph, "Running header", 0);
    header.page = Some(1);
    header.layer = ContentLayer::Header;

    let mut body = InternalElement::text(ElementKind::Paragraph, "Body sentence.", 0);
    body.page = Some(1);

    let mut footer = InternalElement::text(ElementKind::Paragraph, "Page 1 of 1", 0);
    footer.page = Some(1);
    footer.layer = ContentLayer::Footer;

    doc.push_element(header);
    doc.push_element(body);
    doc.push_element(footer);
    doc
}

#[test]
fn should_exclude_header_and_footer_furniture_from_page_content() {
    let result = derive_extraction_result(page_with_furniture(), false, OutputFormat::Plain);

    let pages = result.pages.expect("page-tagged elements must produce pages");
    assert_eq!(pages.len(), 1);
    assert_eq!(pages[0].page_number, 1);
    assert_eq!(pages[0].content, "Body sentence.");
}

#[test]
fn should_keep_page_content_in_agreement_with_plain_document_content() {
    let result = derive_extraction_result(page_with_furniture(), false, OutputFormat::Plain);

    let pages = result.pages.as_ref().expect("page-tagged elements must produce pages");
    assert_eq!(pages[0].content, result.content);
}

#[test]
fn should_keep_pages_that_contain_only_furniture() {
    let mut doc = InternalDocument::new("pdf");

    let mut header = InternalElement::text(ElementKind::Paragraph, "Running header", 0);
    header.page = Some(1);
    header.layer = ContentLayer::Header;

    let mut body = InternalElement::text(ElementKind::Paragraph, "Body sentence.", 0);
    body.page = Some(2);

    doc.push_element(header);
    doc.push_element(body);

    let result = derive_extraction_result(doc, false, OutputFormat::Plain);

    let pages = result.pages.expect("page-tagged elements must produce pages");
    assert_eq!(pages.len(), 2);
    assert_eq!(pages[0].page_number, 1);
    assert_eq!(pages[0].content, "");
    assert_eq!(pages[1].page_number, 2);
    assert_eq!(pages[1].content, "Body sentence.");
}
