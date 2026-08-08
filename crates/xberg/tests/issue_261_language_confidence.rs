#![cfg(feature = "language-detection")]
//! Regression test for #261.
//!
//! `ExtractedDocument::detected_languages` only ever carried ISO codes, so a
//! caller had no way to tell a 95%-English/5%-other document apart from a
//! genuinely 50/50 mix, and no access to whatlang's confidence, script, or
//! reliability signals. `ExtractedDocument::detected_language_confidences`
//! (`LanguageConfidence`) now carries that structured data alongside the
//! existing `detected_languages` list.
//!
//! This test builds a document that is 95% English and 5% Chinese (English
//! and Chinese, rather than the issue's illustrative English/French, to keep
//! the fixture immune to whatlang misclassifying one Romance language as a
//! neighbouring one — Latin vs. Mandarin script leaves no ambiguity) and
//! checks the exact chunk-share proportions and the dominance ordering.

use std::borrow::Cow;

use xberg::core::config::{ExtractionConfig, LanguageDetectionConfig};
use xberg::language_detection::LanguageDetector;
use xberg::plugins::PostProcessor;
use xberg::types::ExtractedDocument;

/// Language detection chunks text into fixed 200-character windows. Repeating a
/// phrase out to an exact multiple of that window size keeps every chunk
/// entirely in one language, so the resulting chunk-share proportions are exact
/// integers-over-total rather than approximate.
fn cycle_to_char_len(phrase: &str, len: usize) -> String {
    phrase.chars().cycle().take(len).collect()
}

const CHUNK_SIZE: usize = 200;
const ENGLISH_CHUNKS: usize = 19;
const CHINESE_CHUNKS: usize = 1;
const TOTAL_CHUNKS: usize = ENGLISH_CHUNKS + CHINESE_CHUNKS;

fn mixed_english_chinese_document() -> String {
    let english = cycle_to_char_len(
        "The global economy continues to grow as international trade expands across many \
         different countries and regions worldwide, driving innovation, employment, and \
         economic development at an unprecedented scale. ",
        ENGLISH_CHUNKS * CHUNK_SIZE,
    );
    let chinese = cycle_to_char_len(
        "中国是世界上人口最多的国家之一，拥有悠久的历史和灿烂多样的文化传统，吸引着世界各地的游客前来观光旅游。",
        CHINESE_CHUNKS * CHUNK_SIZE,
    );
    format!("{english}{chinese}")
}

#[tokio::test]
async fn mixed_language_document_reports_exact_proportions_and_ordering() {
    let text = mixed_english_chinese_document();
    assert_eq!(
        text.chars().count(),
        TOTAL_CHUNKS * CHUNK_SIZE,
        "fixture must land on exact chunk boundaries"
    );

    // `ExtractedDocument` has private fields, so a struct literal with
    // `..Default::default()` is E0451 outside the crate (see #313).
    let mut result = ExtractedDocument::default();
    result.content = text;
    result.mime_type = Cow::Borrowed("text/plain");
    let config = ExtractionConfig {
        language_detection: Some(LanguageDetectionConfig {
            enabled: true,
            min_confidence: 0.3,
            detect_multiple: true,
        }),
        ..Default::default()
    };

    LanguageDetector
        .process(&mut result, &config)
        .await
        .expect("language detection post-processor failed");

    let details = result
        .detected_language_confidences
        .expect("detected_language_confidences must be populated");
    assert_eq!(details.len(), 2, "expected exactly two languages, got: {details:?}");

    // Dominance ordering: English (19/20 chunks) must sort before Chinese (1/20 chunks).
    assert_eq!(
        details[0].language, "eng",
        "English must be the dominant language: {details:?}"
    );
    assert_eq!(
        details[1].language, "cmn",
        "Chinese must be the minority language: {details:?}"
    );

    // Exact chunk-share proportions — computed the same way the implementation does,
    // from the same integer chunk counts, so this is bit-for-bit exact, not approximate.
    assert_eq!(
        details[0].proportion,
        ENGLISH_CHUNKS as f64 / TOTAL_CHUNKS as f64,
        "English proportion must equal its exact chunk share: {details:?}"
    );
    assert_eq!(
        details[1].proportion,
        CHINESE_CHUNKS as f64 / TOTAL_CHUNKS as f64,
        "Chinese proportion must equal its exact chunk share: {details:?}"
    );
    assert!(
        (details.iter().map(|d| d.proportion).sum::<f64>() - 1.0).abs() < 1e-9,
        "proportions must sum to 1.0: {details:?}"
    );

    // Script: English is Latin-script, Chinese is Mandarin/Han-script — unambiguous
    // given the two languages chosen for this fixture.
    assert_eq!(details[0].script, "Latin", "unexpected script for English: {details:?}");
    assert_eq!(
        details[1].script, "Mandarin",
        "unexpected script for Chinese: {details:?}"
    );

    // Confidence must be in range and self-consistent with `reliable`, which is
    // documented (see `language_detection::AGGREGATE_RELIABLE_THRESHOLD`) as the
    // chunk-averaged confidence exceeding 0.9.
    for detail in &details {
        assert!(
            (0.0..=1.0).contains(&detail.confidence),
            "confidence must be a valid probability: {detail:?}"
        );
        assert_eq!(
            detail.reliable,
            detail.confidence > 0.9,
            "reliable must track the documented 0.9 aggregate threshold: {detail:?}"
        );
    }

    // `detected_languages` (the pre-existing field) must stay in lockstep with the
    // structured `detected_language_confidences` list, in the same order.
    assert_eq!(
        result.detected_languages,
        Some(details.iter().map(|d| d.language.clone()).collect::<Vec<_>>()),
        "detected_languages must mirror detected_language_confidences order exactly"
    );
}
