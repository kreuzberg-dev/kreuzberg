//! Regression test for issue #306: Whisper timestamp tokens must survive
//! decoding and be turned into real segment boundaries, not silently
//! dropped or left as one flat, unstructured transcript.
//!
//! `<|x.xx|>` timestamp tokens are ordinary (non-`special`) entries in the
//! Whisper tokenizer vocabulary, so `Tokenizer::decode(.., skip_special_tokens
//! = true)` does *not* strip them — verified against the real
//! `onnx-community/whisper-tiny` `tokenizer.json`, where every `<|x.xx|>`
//! added token has `"special": false`. Left unhandled, they would leak into
//! the transcript as literal text (`"hello <|4.34|> world"`) instead of being
//! parsed into timing information.
//!
//! This test exercises the pure token-level parser `parse_timestamped_segments`
//! and the `timestamp_token_to_ms` conversion directly (no ONNX session, no
//! model download, no tokenizer needed) with exact synthetic token sequences.

#![cfg(feature = "transcription")]

use xberg::transcription::engine::{parse_timestamped_segments, timestamp_token_to_ms};

/// `<|0.00|>` in the real `onnx-community/whisper-tiny` tokenizer.
const TIMESTAMP_BEGIN: u32 = 50_364;

fn ts(seconds_hundredths: u32) -> u32 {
    // ticks are 0.02s apart, matching Whisper's canonical tokenization.
    TIMESTAMP_BEGIN + seconds_hundredths / 2
}

#[test]
fn should_convert_timestamp_token_id_to_milliseconds() {
    assert_eq!(timestamp_token_to_ms(TIMESTAMP_BEGIN, TIMESTAMP_BEGIN), 0);
    assert_eq!(timestamp_token_to_ms(TIMESTAMP_BEGIN + 1, TIMESTAMP_BEGIN), 20);
    assert_eq!(timestamp_token_to_ms(TIMESTAMP_BEGIN + 250, TIMESTAMP_BEGIN), 5_000);
    assert_eq!(timestamp_token_to_ms(TIMESTAMP_BEGIN + 1_500, TIMESTAMP_BEGIN), 30_000);
}

#[test]
fn should_return_empty_when_no_timestamp_tokens_present() {
    // Plain vocabulary IDs only, as produced when timestamps = false and
    // <|notimestamps|> suppressed timestamp emission entirely.
    let tokens = vec![1_u32, 2, 3, 4];
    let segments = parse_timestamped_segments(&tokens, TIMESTAMP_BEGIN);
    assert!(
        segments.is_empty(),
        "expected no segments when the stream contains no timestamp tokens, got {segments:?}"
    );
}

#[test]
fn should_parse_single_bracketed_segment() {
    // <|0.00|> "hello" "world" <|1.00|>
    let hello = 100_u32;
    let world = 101_u32;
    let tokens = vec![ts(0), hello, world, ts(100)];

    let segments = parse_timestamped_segments(&tokens, TIMESTAMP_BEGIN);

    assert_eq!(segments.len(), 1, "expected exactly one segment, got {segments:?}");
    let (start_id, end_id, text_tokens) = &segments[0];
    assert_eq!(timestamp_token_to_ms(*start_id, TIMESTAMP_BEGIN), 0);
    assert_eq!(timestamp_token_to_ms(*end_id, TIMESTAMP_BEGIN), 1_000);
    assert_eq!(text_tokens, &vec![hello, world]);
}

#[test]
fn should_parse_multiple_adjacent_segments_with_exact_boundaries() {
    // <|0.00|> "a" <|2.50|> <|2.50|> "b" "c" <|5.00|> <|5.00|> "d" <|7.20|>
    let a = 10_u32;
    let b = 11_u32;
    let c = 12_u32;
    let d = 13_u32;
    let tokens = vec![ts(0), a, ts(250), ts(250), b, c, ts(500), ts(500), d, ts(720)];

    let segments = parse_timestamped_segments(&tokens, TIMESTAMP_BEGIN);

    assert_eq!(segments.len(), 3, "expected exactly three segments, got {segments:?}");

    let (s0, e0, t0) = &segments[0];
    assert_eq!(
        (
            timestamp_token_to_ms(*s0, TIMESTAMP_BEGIN),
            timestamp_token_to_ms(*e0, TIMESTAMP_BEGIN)
        ),
        (0, 2_500)
    );
    assert_eq!(t0, &vec![a]);

    let (s1, e1, t1) = &segments[1];
    assert_eq!(
        (
            timestamp_token_to_ms(*s1, TIMESTAMP_BEGIN),
            timestamp_token_to_ms(*e1, TIMESTAMP_BEGIN)
        ),
        (2_500, 5_000)
    );
    assert_eq!(t1, &vec![b, c]);

    let (s2, e2, t2) = &segments[2];
    assert_eq!(
        (
            timestamp_token_to_ms(*s2, TIMESTAMP_BEGIN),
            timestamp_token_to_ms(*e2, TIMESTAMP_BEGIN)
        ),
        (5_000, 7_200)
    );
    assert_eq!(t2, &vec![d]);
}

#[test]
fn should_drop_text_before_first_timestamp_and_after_trailing_unpaired_timestamp() {
    // Leading garbage before the first timestamp, and a dangling opening
    // timestamp at the end (generation cut off mid-segment) with trailing
    // text: neither has a complete (start, end) pair, so both are dropped.
    let leading_garbage = 999_u32;
    let hello = 100_u32;
    let trailing_text = 200_u32;
    let tokens = vec![leading_garbage, ts(0), hello, ts(100), ts(150), trailing_text];

    let segments = parse_timestamped_segments(&tokens, TIMESTAMP_BEGIN);

    assert_eq!(
        segments.len(),
        1,
        "expected only the one complete pair, got {segments:?}"
    );
    let (start_id, end_id, text_tokens) = &segments[0];
    assert_eq!(timestamp_token_to_ms(*start_id, TIMESTAMP_BEGIN), 0);
    assert_eq!(timestamp_token_to_ms(*end_id, TIMESTAMP_BEGIN), 1_000);
    assert_eq!(text_tokens, &vec![hello]);
}

#[test]
fn should_return_empty_for_single_unpaired_timestamp_token() {
    let tokens = vec![ts(0)];
    let segments = parse_timestamped_segments(&tokens, TIMESTAMP_BEGIN);
    assert!(segments.is_empty(), "a lone timestamp token has no closing pair");
}
