//! Regression test for defect #52.
//!
//! `From<ExtractedDocument> for InternalDocument` is used at FFI/trait-bridge boundaries
//! when a foreign-language plugin returns the public shape. It copied only 8 fields and
//! silently dropped seven others that have an exact destination on `InternalDocument`:
//! `annotations`, `uris`, `children`, `processing_warnings`, `llm_usage`,
//! `pages` -> `prebuilt_pages`, and `ocr_elements` -> `prebuilt_ocr_elements`.

use std::borrow::Cow;

use xberg::types::internal::InternalDocument;
use xberg::{
    ArchiveEntry, ExtractedDocument, ExtractedUri, LlmUsage, OcrElement, PageContent, PdfAnnotation, PdfAnnotationType,
    ProcessingWarning, UriKind,
};

/// `ExtractedDocument` has crate-private fields, so it cannot be built with a struct
/// literal from an integration test — build it by assignment instead.
fn source_document() -> ExtractedDocument {
    let mut result = ExtractedDocument::default();
    result.content = "body text".to_string();
    result.mime_type = Cow::Borrowed("application/pdf");
    result.annotations = Some(vec![PdfAnnotation {
        annotation_type: PdfAnnotationType::Highlight,
        content: Some("a highlighted note".to_string()),
        page_number: 7,
        bounding_box: None,
        author: None,
        modified: None,
        color: None,
        subject: None,
        quad_points: None,
        marked_text: None,
    }]);
    result.uris = Some(vec![ExtractedUri {
        url: "https://example.com/a".to_string(),
        label: Some("Example".to_string()),
        page: Some(2),
        kind: UriKind::Hyperlink,
    }]);
    result.children = Some(vec![ArchiveEntry {
        path: "inner/report.txt".to_string(),
        mime_type: "text/plain".to_string(),
        result: Box::new(ExtractedDocument::default()),
    }]);
    result.processing_warnings = vec![ProcessingWarning {
        source: Cow::Borrowed("chunking"),
        message: Cow::Borrowed("chunk stage skipped"),
    }];
    result.llm_usage = Some(vec![LlmUsage {
        model: "openai/gpt-4o".to_string(),
        source: "vlm_ocr".to_string(),
        total_tokens: Some(42),
        ..Default::default()
    }]);
    result.pages = Some(vec![PageContent {
        page_number: 3,
        content: "page three".to_string(),
        tables: Vec::new(),
        image_indices: Vec::new(),
        hierarchy: None,
        is_blank: None,
        layout_regions: None,
        speaker_notes: None,
        section_name: None,
        sheet_name: None,
    }]);
    result.ocr_elements = Some(vec![OcrElement {
        text: "ocr token".to_string(),
        page_number: 4,
        ..Default::default()
    }]);
    result
}

#[test]
fn should_preserve_annotations_when_converting_to_internal_document() {
    let doc = InternalDocument::from(source_document());

    let annotations = doc.annotations.expect("annotations must survive the conversion");
    assert_eq!(annotations.len(), 1);
    assert_eq!(annotations[0].page_number, 7);
    assert_eq!(annotations[0].content.as_deref(), Some("a highlighted note"));
}

#[test]
fn should_preserve_uris_when_converting_to_internal_document() {
    let doc = InternalDocument::from(source_document());

    assert_eq!(doc.uris.len(), 1);
    assert_eq!(doc.uris[0].url, "https://example.com/a");
    assert_eq!(doc.uris[0].label.as_deref(), Some("Example"));
    assert_eq!(doc.uris[0].page, Some(2));
    assert_eq!(doc.uris[0].kind, UriKind::Hyperlink);
}

#[test]
fn should_preserve_archive_children_when_converting_to_internal_document() {
    let doc = InternalDocument::from(source_document());

    let children = doc.children.expect("archive children must survive the conversion");
    assert_eq!(children.len(), 1);
    assert_eq!(children[0].path, "inner/report.txt");
    assert_eq!(children[0].mime_type, "text/plain");
}

#[test]
fn should_preserve_processing_warnings_when_converting_to_internal_document() {
    let doc = InternalDocument::from(source_document());

    assert_eq!(doc.processing_warnings.len(), 1);
    assert_eq!(doc.processing_warnings[0].source, "chunking");
    assert_eq!(doc.processing_warnings[0].message, "chunk stage skipped");
}

#[test]
fn should_preserve_llm_usage_when_converting_to_internal_document() {
    let doc = InternalDocument::from(source_document());

    let llm_usage = doc.llm_usage.expect("llm_usage must survive the conversion");
    assert_eq!(llm_usage.len(), 1);
    assert_eq!(llm_usage[0].model, "openai/gpt-4o");
    assert_eq!(llm_usage[0].source, "vlm_ocr");
    assert_eq!(llm_usage[0].total_tokens, Some(42));
}

#[test]
fn should_map_pages_to_prebuilt_pages_when_converting_to_internal_document() {
    let doc = InternalDocument::from(source_document());

    let pages = doc.prebuilt_pages.expect("pages must survive as prebuilt_pages");
    assert_eq!(pages.len(), 1);
    assert_eq!(pages[0].page_number, 3);
    assert_eq!(pages[0].content, "page three");
}

#[test]
fn should_map_ocr_elements_to_prebuilt_ocr_elements_when_converting_to_internal_document() {
    let doc = InternalDocument::from(source_document());

    let ocr_elements = doc
        .prebuilt_ocr_elements
        .expect("ocr_elements must survive as prebuilt_ocr_elements");
    assert_eq!(ocr_elements.len(), 1);
    assert_eq!(ocr_elements[0].text, "ocr token");
    assert_eq!(ocr_elements[0].page_number, 4);
}

#[test]
fn should_keep_copying_the_fields_that_already_worked() {
    let doc = InternalDocument::from(source_document());

    assert_eq!(doc.mime_type, "application/pdf");
    assert_eq!(doc.pre_rendered_content.as_deref(), Some("body text"));
}
