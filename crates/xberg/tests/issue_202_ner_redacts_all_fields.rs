//! Regression tests for xberg-io/xberg#202.
//!
//! NER-detected PII was redacted in `ExtractedDocument::content` only. Every
//! other text-bearing field was scanned by the regex pattern engine alone, so a
//! person / organisation / location name survived in metadata, tables, pages,
//! chunks, elements, OCR elements and form fields.
//!
//! All PII in these tests is synthetic and built in-test.

#![cfg(feature = "redaction")]

use xberg::core::config::redaction::RedactionConfig;
use xberg::text::redaction::redact_with_entities;
use xberg::types::entity::{Entity, EntityCategory};
use xberg::types::form_field::{FormFieldType, PdfFormField};
use xberg::types::tables::Table;
use xberg::types::{Chunk, ChunkMetadata, ChunkType, ExtractedDocument, PageContent};

const SUBJECT: &str = "Zarnak Quorlim";
const EMPLOYER: &str = "Blorptech Interstellar";
const MASK: &str = "[REDACTED]";

fn entities() -> Vec<Entity> {
    vec![
        Entity {
            category: EntityCategory::Person,
            text: SUBJECT.to_string(),
            start: 0,
            end: SUBJECT.len() as u32,
            confidence: Some(0.98),
        },
        Entity {
            category: EntityCategory::Organization,
            text: EMPLOYER.to_string(),
            start: 0,
            end: EMPLOYER.len() as u32,
            confidence: Some(0.95),
        },
    ]
}

fn chunk(content: &str) -> Chunk {
    Chunk {
        content: content.to_string(),
        chunk_type: ChunkType::Unknown,
        embedding: None,
        sparse_embedding: None,
        late_interaction: None,
        metadata: ChunkMetadata {
            byte_start: 0,
            byte_end: content.len(),
            token_count: None,
            chunk_index: 0,
            total_chunks: 1,
            first_page: None,
            last_page: None,
            heading_context: None,
            heading_path: vec![SUBJECT.to_string()],
            image_indices: Vec::new(),
            node_ids: Vec::new(),
            page_spans: Vec::new(),
            classifications: Vec::new(),
        },
    }
}

fn page(content: &str) -> PageContent {
    PageContent {
        page_number: 1,
        content: content.to_string(),
        tables: Vec::new(),
        image_indices: Vec::new(),
        hierarchy: None,
        is_blank: None,
        layout_regions: None,
        speaker_notes: Some(format!("Notes about {SUBJECT}")),
        section_name: None,
        sheet_name: None,
    }
}

fn redacted_document() -> ExtractedDocument {
    let mut document = ExtractedDocument::default();
    document.content = format!("{SUBJECT} works at {EMPLOYER}.");
    document.tables = vec![Table {
        cells: vec![vec!["Employee".to_string(), SUBJECT.to_string()]],
        markdown: format!("| Employee | {SUBJECT} |"),
        page_number: 1,
        bounding_box: None,
        ..Default::default()
    }];
    document.chunks = Some(vec![chunk(&format!("Chunk mentions {SUBJECT}."))]);
    document.pages = Some(vec![page(&format!("Page mentions {EMPLOYER}."))]);
    document.form_fields = vec![PdfFormField {
        name: "employee".to_string(),
        full_name: "form.employee".to_string(),
        field_type: FormFieldType::Text,
        value: Some(SUBJECT.to_string()),
        default_value: None,
        flags: 0,
        page: None,
        bbox: None,
        max_length: None,
        tooltip: None,
    }];
    document.metadata.title = Some(format!("Personnel file: {SUBJECT}"));
    document.metadata.authors = Some(vec![SUBJECT.to_string()]);
    document.metadata.created_by = Some(SUBJECT.to_string());

    redact_with_entities(&mut document, &RedactionConfig::default(), &entities()).expect("redaction must succeed");
    document
}

#[test]
fn should_redact_ner_person_in_metadata_title() {
    let document = redacted_document();
    assert_eq!(
        document.metadata.title.as_deref(),
        Some("Personnel file: [REDACTED]"),
        "metadata.title must be redacted"
    );
}

#[test]
fn should_redact_ner_person_in_metadata_authors_and_created_by() {
    let document = redacted_document();
    assert_eq!(
        document.metadata.authors.as_deref(),
        Some([MASK.to_string()].as_slice())
    );
    assert_eq!(document.metadata.created_by.as_deref(), Some(MASK));
}

#[test]
fn should_redact_ner_person_in_table_cells_and_markdown() {
    let document = redacted_document();
    assert_eq!(document.tables[0].cells[0][1], MASK);
    assert_eq!(document.tables[0].markdown, "| Employee | [REDACTED] |");
}

#[test]
fn should_redact_ner_organization_in_page_content() {
    let document = redacted_document();
    let pages = document.pages.expect("pages present");
    assert_eq!(pages[0].content, "Page mentions [REDACTED].");
}

#[test]
fn should_redact_ner_person_in_page_speaker_notes() {
    let document = redacted_document();
    let pages = document.pages.expect("pages present");
    assert_eq!(pages[0].speaker_notes.as_deref(), Some("Notes about [REDACTED]"));
}

#[test]
fn should_redact_ner_person_in_chunk_content_and_heading_path() {
    let document = redacted_document();
    let chunks = document.chunks.expect("chunks present");
    assert_eq!(chunks[0].content, "Chunk mentions [REDACTED].");
    assert_eq!(chunks[0].metadata.heading_path, vec![MASK.to_string()]);
}

#[test]
fn should_redact_ner_person_in_form_field_value() {
    let document = redacted_document();
    assert_eq!(document.form_fields[0].value.as_deref(), Some(MASK));
}

#[test]
fn should_leave_no_occurrence_of_the_subject_anywhere_in_the_document() {
    let document = redacted_document();
    let serialised = serde_json::to_string(&document).expect("document serialises");
    assert!(
        !serialised.contains(SUBJECT),
        "person name survived somewhere in the serialised document"
    );
    assert!(
        !serialised.contains(EMPLOYER),
        "organisation name survived somewhere in the serialised document"
    );
}
