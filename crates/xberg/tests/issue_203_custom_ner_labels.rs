//! Regression tests for xberg-io/xberg#203.
//!
//! `NerConfig::custom_labels` let a caller ask a zero-shot backend for
//! domain-specific entity types. The redaction engine then threw those
//! detections away: `collect_ner_matches` mapped only PERSON / ORGANIZATION /
//! LOCATION and returned `None` for `EntityCategory::Custom`, so a custom label
//! was never redacted — and when the caller asked for custom labels *only*, the
//! engine skipped the backend entirely.
//!
//! All PII in these tests is synthetic and built in-test.

#![cfg(feature = "redaction")]

use std::collections::HashSet;

use xberg::core::config::ner::NerConfig;
use xberg::core::config::redaction::RedactionConfig;
use xberg::text::redaction::redact_with_entities;
use xberg::types::ExtractedDocument;
use xberg::types::entity::{Entity, EntityCategory};
use xberg::types::redaction::PiiCategory;

const TREATMENT: &str = "Quorlixibene";
const LABEL: &str = "Treatment";

fn config_with_custom_label(label: &str) -> RedactionConfig {
    RedactionConfig {
        ner: Some(NerConfig {
            custom_labels: vec![label.to_string()],
            ..NerConfig::default()
        }),
        ..RedactionConfig::default()
    }
}

fn custom_entity(label: &str, text: &str) -> Entity {
    Entity {
        category: EntityCategory::Custom(label.to_string()),
        text: text.to_string(),
        start: 0,
        end: text.len() as u32,
        confidence: Some(0.91),
    }
}

#[test]
fn should_redact_a_custom_ner_label_requested_through_ner_config() {
    let mut document = ExtractedDocument::default();
    document.content = format!("Patient received {TREATMENT} twice; {TREATMENT} was effective.");
    document.metadata.subject = Some(format!("Course of {TREATMENT}"));

    redact_with_entities(
        &mut document,
        &config_with_custom_label(LABEL),
        &[custom_entity(LABEL, TREATMENT)],
    )
    .expect("redaction must succeed");

    assert_eq!(
        document.content, "Patient received [REDACTED] twice; [REDACTED] was effective.",
        "custom-label mentions must be redacted in content"
    );
    assert_eq!(
        document.metadata.subject.as_deref(),
        Some("Course of [REDACTED]"),
        "custom-label mentions must be redacted outside content too"
    );

    let report = document.redaction_report.expect("report must be attached");
    assert_eq!(report.total_redacted, 3);
    let categories: HashSet<PiiCategory> = report.findings.iter().map(|f| f.category.clone()).collect();
    assert_eq!(
        categories,
        HashSet::from([PiiCategory::Custom(LABEL.to_string())]),
        "findings must carry the caller's label, not a generic category"
    );
}

#[test]
fn should_redact_a_custom_label_declared_through_redaction_categories() {
    let mut document = ExtractedDocument::default();
    document.content = format!("Dose of {TREATMENT} recorded.");
    let config = RedactionConfig {
        categories: HashSet::from([PiiCategory::Custom(LABEL.to_string())]),
        ..RedactionConfig::default()
    };

    redact_with_entities(&mut document, &config, &[custom_entity(LABEL, TREATMENT)]).expect("redaction must succeed");

    assert_eq!(document.content, "Dose of [REDACTED] recorded.");
}

#[test]
fn should_ignore_a_custom_label_the_caller_never_asked_for() {
    let original = format!("Trace of {TREATMENT} noted.");
    let mut document = ExtractedDocument::default();
    document.content = original.clone();

    // The backend invented a label. Honouring it would silently widen redaction
    // beyond what the caller configured.
    redact_with_entities(
        &mut document,
        &config_with_custom_label(LABEL),
        &[custom_entity("HallucinatedLabel", TREATMENT)],
    )
    .expect("redaction must succeed");

    assert_eq!(document.content, original);
    let report = document.redaction_report.expect("report must be attached");
    assert_eq!(report.total_redacted, 0);
}
