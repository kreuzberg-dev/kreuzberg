//! Regression tests for xberg-io/xberg#262.
//!
//! NER ran the whole document through one inference call. The GLiNER
//! preprocessor truncates its input at `Parameters::max_length` words (512 by
//! default), so on a long document every entity past that point was never
//! detected — and PII that is never detected is never redacted. Detection
//! degraded silently: no error, no warning, just a shorter entity list.
//!
//! All PII in these tests is synthetic and built in-test.

#![cfg(feature = "ner")]

use xberg::text::ner::offsets::{
    GLINER_WINDOW_OVERLAP_TOKENS, GLINER_WINDOW_TOKENS, merge_windowed_entities, split_into_windows,
};
use xberg::types::entity::{Entity, EntityCategory};

const SUBJECT: &str = "Zarnak Quorlim";

/// Filler text with `SUBJECT` planted after `lead_words` words — far past the
/// single-call word budget when `lead_words` exceeds it.
fn document_with_subject_at(lead_words: usize) -> String {
    let mut text: Vec<String> = (0..lead_words).map(|index| format!("filler{index}")).collect();
    text.push(SUBJECT.to_string());
    text.push("closed the file.".to_string());
    text.join(" ")
}

#[test]
fn should_window_input_that_exceeds_the_single_call_word_budget() {
    let text = document_with_subject_at(3_000);
    let windows = split_into_windows(&text, GLINER_WINDOW_TOKENS, GLINER_WINDOW_OVERLAP_TOKENS);

    assert!(
        windows.len() > 1,
        "a 3000-word document must be windowed, got {} window(s)",
        windows.len()
    );
    assert_eq!(
        windows[0].byte_offset, 0,
        "the first window must start at the source head"
    );
}

#[test]
fn should_keep_a_mention_past_the_truncation_point_inside_some_window() {
    let text = document_with_subject_at(3_000);
    let windows = split_into_windows(&text, GLINER_WINDOW_TOKENS, GLINER_WINDOW_OVERLAP_TOKENS);

    // Pre-fix, only the first `GLINER_WINDOW_TOKENS` words reached the model, so
    // this mention was invisible to detection.
    assert!(
        !windows[0].text.contains(SUBJECT),
        "the mention must sit past the first window, or this test proves nothing"
    );
    assert!(
        windows.iter().any(|window| window.text.contains(SUBJECT)),
        "no window contains the mention — detection would silently miss it"
    );
}

#[test]
fn should_hold_every_window_within_the_word_budget() {
    let text = document_with_subject_at(3_000);
    let windows = split_into_windows(&text, GLINER_WINDOW_TOKENS, GLINER_WINDOW_OVERLAP_TOKENS);

    for window in &windows {
        assert!(
            window.text.split_whitespace().count() <= GLINER_WINDOW_TOKENS,
            "window exceeds the model's word budget and would be truncated again"
        );
    }
}

#[test]
fn should_map_a_windowed_detection_back_to_its_source_offsets() {
    let text = document_with_subject_at(3_000);
    let windows = split_into_windows(&text, GLINER_WINDOW_TOKENS, GLINER_WINDOW_OVERLAP_TOKENS);
    let window_offsets: Vec<usize> = windows.iter().map(|window| window.byte_offset).collect();

    // Simulate each window's backend output: a detection wherever the window
    // itself contains the mention, at window-local offsets.
    let per_window: Vec<Vec<Entity>> = windows
        .iter()
        .map(|window| match window.text.find(SUBJECT) {
            Some(local_start) => vec![Entity {
                category: EntityCategory::Person,
                text: SUBJECT.to_string(),
                start: local_start as u32,
                end: (local_start + SUBJECT.len()) as u32,
                confidence: Some(0.96),
            }],
            None => Vec::new(),
        })
        .collect();

    let merged = merge_windowed_entities(&window_offsets, per_window);

    assert_eq!(
        merged.len(),
        1,
        "the overlap region must not report the same mention twice: {merged:?}"
    );
    let entity = &merged[0];
    assert_eq!(entity.category, EntityCategory::Person);
    assert_eq!(
        &text[entity.start as usize..entity.end as usize],
        SUBJECT,
        "merged offsets must address the mention in the *source*, not in the window"
    );
    assert_eq!(
        entity.start as usize,
        text.find(SUBJECT).expect("planted mention"),
        "merged start offset must equal the source offset"
    );
}

#[test]
fn should_return_no_windows_for_whitespace_only_input() {
    assert!(split_into_windows("   \n\t ", GLINER_WINDOW_TOKENS, GLINER_WINDOW_OVERLAP_TOKENS).is_empty());
}
