//! Regression tests for xberg-io/xberg#204.
//!
//! `RedactionReport::total_redacted` was `findings.len()` where `findings` only
//! ever held the matches from `ExtractedDocument::content`. Replacements made in
//! `formatted_content`, chunks, tables, pages and metadata were applied but
//! never counted, so the audit report undercounted the redactions the pass
//! actually performed.
//!
//! All PII in these tests is synthetic and built in-test.

#![cfg(feature = "redaction")]

use xberg::core::config::redaction::RedactionConfig;
use xberg::text::redaction::redact_with_entities;
use xberg::types::entity::{Entity, EntityCategory};
use xberg::types::redaction::PiiCategory;
use xberg::types::tables::Table;
use xberg::types::{ExtractedDocument, PageContent};

const SUBJECT: &str = "Zarnak Quorlim";

fn person_entity() -> Entity {
    Entity {
        category: EntityCategory::Person,
        text: SUBJECT.to_string(),
        start: 0,
        end: SUBJECT.len() as u32,
        confidence: Some(0.99),
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
        speaker_notes: None,
        section_name: None,
        sheet_name: None,
    }
}

#[test]
fn should_count_findings_from_every_field_not_only_content() {
    let mut document = ExtractedDocument::default();
    document.content = format!("{SUBJECT} signed.");
    document.formatted_content = Some(format!("**{SUBJECT}** signed."));
    document.tables = vec![Table {
        cells: vec![vec![SUBJECT.to_string()]],
        markdown: String::new(),
        page_number: 1,
        bounding_box: None,
        ..Default::default()
    }];
    document.pages = Some(vec![page(&format!("Page names {SUBJECT}."))]);
    document.metadata.title = Some(SUBJECT.to_string());

    redact_with_entities(&mut document, &RedactionConfig::default(), &[person_entity()])
        .expect("redaction must succeed");

    let report = document.redaction_report.expect("report must be attached");
    // content + formatted_content + table cell + page content + metadata.title.
    assert_eq!(report.total_redacted, 5, "findings: {:?}", report.findings);
    assert_eq!(report.findings.len(), 5);
    assert!(
        report.findings.iter().all(|f| f.category == PiiCategory::Person),
        "every finding must carry the detecting category"
    );
}

#[test]
fn should_report_zero_when_the_document_holds_no_pii() {
    let mut document = ExtractedDocument::default();
    document.content = "Nothing sensitive in this sentence.".to_string();

    redact_with_entities(&mut document, &RedactionConfig::default(), &[]).expect("redaction must succeed");

    let report = document.redaction_report.expect("report must be attached");
    assert_eq!(report.total_redacted, 0);
    assert!(report.findings.is_empty());
}

#[test]
fn should_keep_total_redacted_equal_to_the_findings_length() {
    let mut document = ExtractedDocument::default();
    document.content = format!("{SUBJECT} and qq-user@invalid.example and {SUBJECT}.");
    document.metadata.subject = Some(format!("Re: {SUBJECT}"));

    redact_with_entities(&mut document, &RedactionConfig::default(), &[person_entity()])
        .expect("redaction must succeed");

    let report = document.redaction_report.expect("report must be attached");
    assert_eq!(report.total_redacted as usize, report.findings.len());
    assert_eq!(report.total_redacted, 4);
}
