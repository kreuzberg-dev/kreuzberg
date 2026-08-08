//! Regression tests for xberg-io/xberg#201.
//!
//! The redaction report recorded a [`RedactionFinding`] for every match the
//! engine *found*, while the rewrite step silently skipped any span that was
//! not a valid slice of the field. The audit trail therefore claimed
//! redactions that never happened — a false compliance record, which is worse
//! than a visibly missing one.
//!
//! All PII in these tests is synthetic and built in-test.

#![cfg(feature = "redaction")]

use xberg::core::config::redaction::RedactionConfig;
use xberg::text::redaction::redact_with_entities;
use xberg::types::ExtractedDocument;
use xberg::types::entity::{Entity, EntityCategory};
use xberg::types::tables::Table;

const SUBJECT: &str = "Zarnak Quorlim";
const MASK: &str = "[REDACTED]";

fn person(text: &str, start: u32, end: u32) -> Entity {
    Entity {
        category: EntityCategory::Person,
        text: text.to_string(),
        start,
        end,
        confidence: Some(0.97),
    }
}

/// Count the mask tokens the pass actually left behind, across every field the
/// test populated.
fn applied_mask_count(document: &ExtractedDocument) -> usize {
    let mut count = document.content.matches(MASK).count();
    count += document
        .metadata
        .title
        .as_deref()
        .map_or(0, |title| title.matches(MASK).count());
    for table in &document.tables {
        for row in &table.cells {
            for cell in row {
                count += cell.matches(MASK).count();
            }
        }
    }
    count
}

#[test]
fn should_report_exactly_as_many_findings_as_replacements_applied() {
    let mut document = ExtractedDocument::default();
    document.content = format!("{SUBJECT} mailed qq-user@invalid.example and {SUBJECT} replied.");
    document.tables = vec![Table {
        cells: vec![vec!["Owner".to_string(), SUBJECT.to_string()]],
        markdown: String::new(),
        page_number: 1,
        bounding_box: None,
        ..Default::default()
    }];
    document.metadata.title = Some(format!("Case file: {SUBJECT}"));
    let entities = vec![person(SUBJECT, 0, SUBJECT.len() as u32)];

    redact_with_entities(&mut document, &RedactionConfig::default(), &entities).expect("redaction must succeed");

    let report = document.redaction_report.clone().expect("report must be attached");
    let applied = applied_mask_count(&document);

    // 2 in content + 1 email in content + 1 in the table cell + 1 in the title.
    assert_eq!(applied, 5, "unexpected number of applied replacements");
    assert_eq!(
        report.total_redacted as usize, applied,
        "reported total must equal the replacements actually applied"
    );
    assert_eq!(
        report.findings.len(),
        applied,
        "one finding per applied replacement, no more"
    );
}

#[test]
fn should_report_nothing_when_the_detected_mention_is_absent_from_the_document() {
    let original = "Nothing sensitive here at all.";
    let mut document = ExtractedDocument::default();
    document.content = original.to_string();
    // A stale span from a backend that ran against different text: the engine
    // must neither rewrite an unrelated region nor claim it did.
    let entities = vec![person(SUBJECT, 0, SUBJECT.len() as u32)];

    redact_with_entities(&mut document, &RedactionConfig::default(), &entities).expect("redaction must succeed");

    assert_eq!(document.content, original, "content must be untouched");
    let report = document.redaction_report.expect("report must be attached");
    assert_eq!(report.total_redacted, 0);
    assert!(report.findings.is_empty(), "findings: {:?}", report.findings);
}

#[test]
fn should_never_report_a_finding_whose_replacement_token_is_missing_from_the_output() {
    let mut document = ExtractedDocument::default();
    document.content = format!("{SUBJECT} and {SUBJECT} again.");
    let entities = vec![person(SUBJECT, 0, SUBJECT.len() as u32)];

    redact_with_entities(&mut document, &RedactionConfig::default(), &entities).expect("redaction must succeed");

    let report = document.redaction_report.expect("report must be attached");
    for finding in &report.findings {
        assert!(
            document.content.contains(&finding.replacement_token),
            "reported replacement token is not present in the output"
        );
    }
    assert_eq!(report.findings.len(), 2);
}
