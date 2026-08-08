//! Regression tests for xberg-io/xberg#200.
//!
//! An entity detected by a NER backend was redacted at its first byte span
//! only: `crates/xberg/src/text/ner/llm.rs` resolved the mention with
//! `str::find`, and the redaction engine consumed that single span. Every later
//! occurrence of the same PII string survived into the "redacted" output.
//!
//! All PII in these tests is synthetic and built in-test.

#![cfg(feature = "redaction")]

use xberg::core::config::redaction::RedactionConfig;
use xberg::text::redaction::redact_with_entities;
use xberg::types::ExtractedDocument;
use xberg::types::entity::{Entity, EntityCategory};

/// Obviously-synthetic person name that no pattern rule can match on its own.
const SUBJECT: &str = "Zarnak Quorlim";

fn person(text: &str, start: u32, end: u32) -> Entity {
    Entity {
        category: EntityCategory::Person,
        text: text.to_string(),
        start,
        end,
        confidence: Some(0.97),
    }
}

#[test]
fn should_redact_every_occurrence_when_a_mention_repeats_in_content() {
    let mut document = ExtractedDocument::default();
    document.content = format!("{SUBJECT} signed it. Later {SUBJECT} paid. Finally {SUBJECT} left.");
    // The backend only ever reports the first span; the engine must not stop there.
    let entities = vec![person(SUBJECT, 0, SUBJECT.len() as u32)];

    redact_with_entities(&mut document, &RedactionConfig::default(), &entities).expect("redaction must succeed");

    assert_eq!(
        document.content, "[REDACTED] signed it. Later [REDACTED] paid. Finally [REDACTED] left.",
        "every occurrence must be replaced"
    );
    assert_eq!(document.content.matches("[REDACTED]").count(), 3);
    assert!(!document.content.contains(SUBJECT));

    let report = document.redaction_report.expect("report must be attached");
    assert_eq!(report.total_redacted, 3);
    assert_eq!(report.findings.len(), 3);
}

#[test]
fn should_not_redact_a_longer_word_that_merely_contains_the_mention() {
    let mut document = ExtractedDocument::default();
    document.content = "Quorlimated data and Quorlim alone.".to_string();
    let entities = vec![person("Quorlim", 21, 28)];

    redact_with_entities(&mut document, &RedactionConfig::default(), &entities).expect("redaction must succeed");

    assert_eq!(
        document.content, "Quorlimated data and [REDACTED] alone.",
        "word-boundary anchoring must keep the engine from shredding unrelated words"
    );
}

/// The offset-resolution helper the LLM backend now uses: one entity per
/// occurrence, in ascending byte order.
#[cfg(feature = "ner")]
#[test]
fn should_resolve_all_occurrences_when_backend_reports_a_bare_mention() {
    use xberg::text::ner::offsets::entities_for_every_occurrence;

    let text = format!("{SUBJECT} a {SUBJECT} b {SUBJECT}");
    let found = entities_for_every_occurrence(&text, SUBJECT, EntityCategory::Person, Some(0.9));

    assert_eq!(found.len(), 3, "expected one entity per occurrence, got {found:?}");
    assert_eq!(
        found.iter().map(|e| (e.start, e.end)).collect::<Vec<_>>(),
        vec![(0, 14), (17, 31), (34, 48)]
    );
    for entity in &found {
        assert_eq!(&text[entity.start as usize..entity.end as usize], SUBJECT);
        assert_eq!(entity.category, EntityCategory::Person);
    }
}
