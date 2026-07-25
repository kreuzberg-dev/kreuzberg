#[cfg(not(target_arch = "wasm32"))]
use std::path::Path;

use crate::{GlinerError, Result};

/// Pre-tokenized encoding: token ids plus the source-word index of each token,
/// as returned by `tokenizers::Tokenizer::encode` in pre-tokenized mode.
pub struct PretokenizedEncoding {
    pub ids: Vec<i64>,
    pub word_ids: Vec<Option<u32>>,
}

/// Encodes a pre-split sequence of words, tracking which output token came from
/// which input word. GLiNER2's schema-prompt framing needs this mapping to locate
/// `[P]`/`[E]` marker tokens and text-word start positions in the final sequence.
pub trait PretokenizingTokenizer {
    fn encode_pretokenized(&self, words: Vec<&str>) -> Result<PretokenizedEncoding>;
}

/// Wraps a raw `tokenizers::Tokenizer` for GLiNER2's pre-tokenized encoding mode.
///
/// Unlike [`crate::tokenizer::HFTokenizer`] (whole-string encode), GLiNER2 requires
/// word-level pre-tokenized input so the resulting token-to-word mapping can be used
/// to locate `text_positions` and `schema_positions` in the encoded sequence.
pub struct V2Tokenizer {
    inner: tokenizers::Tokenizer,
}

impl V2Tokenizer {
    /// Not built for wasm32: there is no filesystem there, and every caller is
    /// itself native-only, so it would be dead code.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn from_file<P: AsRef<Path>>(path: P) -> Result<Self> {
        let inner = tokenizers::Tokenizer::from_file(path)
            .map_err(|error| GlinerError::Tokenizer(format!("failed to load tokenizer from file: {error}")))?;
        Ok(Self { inner })
    }

    /// Load from in-memory `tokenizer.json` bytes (wasm/no-fs path); only
    /// the `candle` backend loads tokenizers from memory.
    #[cfg(feature = "candle")]
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        let inner = tokenizers::Tokenizer::from_bytes(bytes)
            .map_err(|error| GlinerError::Tokenizer(format!("failed to load tokenizer from bytes: {error}")))?;
        Ok(Self { inner })
    }
}

impl PretokenizingTokenizer for V2Tokenizer {
    fn encode_pretokenized(&self, words: Vec<&str>) -> Result<PretokenizedEncoding> {
        let encoding = self
            .inner
            .encode(words, false)
            .map_err(|error| GlinerError::Tokenizer(format!("failed to encode pre-tokenized input: {error}")))?;
        Ok(PretokenizedEncoding {
            ids: encoding.get_ids().iter().map(|&id| i64::from(id)).collect(),
            word_ids: encoding.get_word_ids().to_vec(),
        })
    }
}
