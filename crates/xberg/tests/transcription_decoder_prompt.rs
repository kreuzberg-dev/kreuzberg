//! Regression test for issue #158: the Whisper decoder prompt must actually
//! react to the caller-requested `timestamps` flag.
//!
//! Previously `WhisperEngine::transcribe` accepted a `_timestamps` parameter
//! that was discarded entirely, and `transcribe_chunk` unconditionally pushed
//! the `<|notimestamps|>` token into the decoder prompt regardless of what
//! was requested. This test exercises the pure token-building helper
//! `build_decoder_prompt_tokens` directly (no ONNX session, no model
//! download) and asserts exact token vectors for both settings of the flag.

#![cfg(feature = "transcription")]

use xberg::transcription::engine::build_decoder_prompt_tokens;

const START_OF_TRANSCRIPT: u32 = 50_258;
const LANG_EN: u32 = 50_259;
const TRANSCRIBE: u32 = 50_359;
const NO_TIMESTAMPS: u32 = 50_363;

#[test]
fn should_omit_no_timestamps_token_when_timestamps_requested() {
    let tokens = build_decoder_prompt_tokens(START_OF_TRANSCRIPT, LANG_EN, TRANSCRIBE, NO_TIMESTAMPS, true);

    assert_eq!(
        tokens,
        vec![START_OF_TRANSCRIPT as i64, LANG_EN as i64, TRANSCRIBE as i64],
        "the no_timestamps token must be absent from the prompt when timestamps=true"
    );
    assert!(
        !tokens.contains(&(NO_TIMESTAMPS as i64)),
        "no_timestamps token id must not appear anywhere in the prompt"
    );
}

#[test]
fn should_include_no_timestamps_token_when_timestamps_not_requested() {
    let tokens = build_decoder_prompt_tokens(START_OF_TRANSCRIPT, LANG_EN, TRANSCRIBE, NO_TIMESTAMPS, false);

    assert_eq!(
        tokens,
        vec![
            START_OF_TRANSCRIPT as i64,
            LANG_EN as i64,
            TRANSCRIBE as i64,
            NO_TIMESTAMPS as i64,
        ],
        "the no_timestamps token must be present (trailing) when timestamps=false"
    );
}

#[test]
fn should_use_provided_language_and_transcribe_token_ids_verbatim() {
    let custom_lang_de: u32 = 50_261;
    let tokens = build_decoder_prompt_tokens(START_OF_TRANSCRIPT, custom_lang_de, TRANSCRIBE, NO_TIMESTAMPS, false);

    assert_eq!(
        tokens,
        vec![
            START_OF_TRANSCRIPT as i64,
            custom_lang_de as i64,
            TRANSCRIBE as i64,
            NO_TIMESTAMPS as i64,
        ]
    );
}
