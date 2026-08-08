mod cjk_utils;
mod config;
mod core;
mod filters;
mod semantic;
mod simd_text;

pub use config::{ReductionLevel, TokenReductionConfig};
pub use core::TokenReducer;

/// Reduces token count in text while preserving meaning and structure.
///
/// This function removes stopwords, redundancy, and applies compression techniques
/// based on the specified reduction level. Supports 64 languages with automatic
/// stopword removal and optional semantic clustering.
///
/// # Arguments
///
/// * `text` - The input text to reduce
/// * `config` - Configuration specifying reduction level and options
/// * `language_hint` - Optional ISO 639-3 language code (e.g., "eng", "spa")
///
/// # Returns
///
/// Returns the reduced text with preserved structure (markdown, code blocks).
///
/// # Errors
///
/// Returns an error if the language hint is invalid or stopwords cannot be loaded.
///
/// # Examples
///
/// Not run as a doctest: this function is `pub(crate)`. Downstream crates reach it by
/// setting [`crate::TokenReductionConfig`] on the extraction config; only the config
/// types ([`crate::TokenReductionConfig`], [`crate::ReductionLevel`]) are public.
///
/// ```ignore
/// use xberg::text::token_reduction::{reduce_tokens, TokenReductionConfig, ReductionLevel};
///
/// let text = "This is a simple example text with some stopwords.";
/// let config = TokenReductionConfig::default();
/// let reduced = reduce_tokens(text, &config, Some("eng"))?;
/// println!("Reduced: {}", reduced);
/// # Ok::<(), xberg::error::XbergError>(())
/// ```
pub(crate) fn reduce_tokens(
    text: &str,
    config: &TokenReductionConfig,
    language_hint: Option<&str>,
) -> crate::error::Result<String> {
    let reducer = TokenReducer::new(config, language_hint)?;
    Ok(reducer.reduce(text))
}
