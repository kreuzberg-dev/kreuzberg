//! Chunk construction and building logic.
//!
//! This module handles the construction of individual chunks from text segments,
//! including overlap calculation, offset tracking, and metadata assembly.

use crate::chunking::text_splitter::{Characters, ChunkCapacity, ChunkConfig};
use crate::error::{Result, XbergError};
use crate::types::{Chunk, ChunkMetadata, HeadingContext, PageBoundary};

use super::boundaries::{calculate_page_range, calculate_page_spans};
use super::classifier::classify_chunk;

#[cfg(feature = "chunking-tokenizers")]
use crate::chunking::text_splitter::ChunkSizer;
#[cfg(feature = "chunking-tokenizers")]
use crate::plugins::TokenizerBackend;
#[cfg(feature = "chunking-tokenizers")]
use std::sync::Arc;

/// Adapts a registered [`crate::plugins::TokenizerBackend`] to the splitter's
/// [`ChunkSizer`] interface: chunk size is the backend's token count.
///
/// A backend reporting zero tokens for non-empty text is not trusted: a zero
/// count would make every span appear to fit any budget and silently produce
/// oversized chunks. Host-language bridges surface backend exceptions as a
/// zero count, so this is also the failure mode of a backend that starts
/// erroring mid-run. The sizer falls back to the character count — tokens
/// don't exceed characters for practical tokenizers, so the budget degrades
/// to the conservative `max_characters` semantics instead of an unbounded
/// chunk — and logs the substitution.
#[cfg(feature = "chunking-tokenizers")]
pub(crate) struct TokenizerBackendSizer(pub(crate) Arc<dyn TokenizerBackend>);

#[cfg(feature = "chunking-tokenizers")]
impl ChunkSizer for TokenizerBackendSizer {
    fn size(&self, chunk: &str) -> usize {
        let count = self.0.count_tokens(chunk);
        if count == 0 && !chunk.is_empty() {
            tracing::warn!(
                backend = self.0.name(),
                chunk_len = chunk.len(),
                "Tokenizer backend reported zero tokens for non-empty text; using character count instead"
            );
            return chunk.chars().count();
        }
        count
    }
}

/// A function that counts "tokens" in a text span, per whatever tokenizer
/// [`resolve_token_counter`] resolved from a [`crate::core::config::ChunkSizing`].
pub(crate) type TokenCounter = Box<dyn Fn(&str) -> usize>;

/// Resolve a token-counting function for `sizing`, if it specifies a tokenizer.
///
/// Returns `None` for [`crate::core::config::ChunkSizing::Characters`] — character-based
/// sizing has no meaningful "token count" to report — or when the `chunking-tokenizers`
/// feature is not compiled in. When `sizing` is
/// [`crate::core::config::ChunkSizing::Tokenizer`], resolves a registered
/// [`TokenizerBackend`] by name first, falling back to a HuggingFace tokenizer id (see
/// [`super::tokenizer_cache::get_or_init_tokenizer`]). A resolution failure (unknown
/// model, no network) is logged and treated as "no counter available" rather than
/// aborting the caller's chunking run — `token_count` simply stays `None` for that run,
/// matching the existing best-effort semantics of `ChunkMetadata`'s other optional fields.
pub(crate) fn resolve_token_counter(sizing: &crate::core::config::ChunkSizing) -> Option<TokenCounter> {
    match sizing {
        crate::core::config::ChunkSizing::Characters => None,
        #[cfg(feature = "chunking-tokenizers")]
        crate::core::config::ChunkSizing::Tokenizer { model, .. } => resolve_tokenizer_counter(model),
    }
}

#[cfg(feature = "chunking-tokenizers")]
fn resolve_tokenizer_counter(model: &str) -> Option<TokenCounter> {
    if let Some(backend) = crate::plugins::registry::get_tokenizer_backend_registry()
        .read()
        .lookup(model)
    {
        return Some(Box::new(move |text: &str| {
            TokenizerBackendSizer(backend.clone()).size(text)
        }));
    }

    match super::tokenizer_cache::get_or_init_tokenizer(model) {
        Ok(tokenizer) => Some(Box::new(move |text: &str| tokenizer.size(text))),
        Err(e) => {
            tracing::warn!(
                model,
                error = %e,
                "Failed to resolve tokenizer for chunk token_count; leaving token_count unset"
            );
            None
        }
    }
}

/// Derive `heading_path` from a `HeadingContext`.
///
/// Returns the heading texts in outermost-first order (h1 → h2 → … → deepest).
/// Returns an empty `Vec` when `heading_context` is `None`.
///
/// # Example
///
/// ```rust,ignore
/// use xberg::types::{HeadingContext, HeadingLevel};
/// let ctx = HeadingContext {
///     headings: vec![
///         HeadingLevel { level: 1, text: "Guide".to_string() },
///         HeadingLevel { level: 2, text: "Setup".to_string() },
///     ],
/// };
/// let path = heading_path_from_context(&Some(ctx));
/// assert_eq!(path, vec!["Guide", "Setup"]);
/// ```
pub(crate) fn heading_path_from_context(heading_context: &Option<HeadingContext>) -> Vec<String> {
    match heading_context {
        Some(ctx) => ctx.headings.iter().map(|h| h.text.clone()).collect(),
        None => Vec::new(),
    }
}

/// Default chunk size used when a zero value is passed (mirrors `default_chunk_size()`).
const DEFAULT_CHUNK_SIZE: usize = 1000;

/// Build a ChunkConfig from chunking parameters.
///
/// # Arguments
///
/// * `max_characters` - Maximum characters per chunk. A value of `0` is clamped to
///   [`DEFAULT_CHUNK_SIZE`] (1000) to prevent panics from downstream `text-splitter`
///   which requires a non-zero capacity.
/// * `overlap` - Character overlap between consecutive chunks
/// * `trim` - Whether to trim whitespace from boundaries
///
/// # Returns
///
/// A configured ChunkConfig ready for use with text splitters.
///
/// # Errors
///
/// Returns `XbergError::Validation` if configuration is invalid.
pub(crate) fn build_chunk_config(max_characters: usize, overlap: usize, trim: bool) -> Result<ChunkConfig<Characters>> {
    let effective_max = if max_characters == 0 {
        tracing::warn!(
            clamped_to = DEFAULT_CHUNK_SIZE,
            "chunk max_characters is 0; clamping to default to avoid panic"
        );
        DEFAULT_CHUNK_SIZE
    } else {
        max_characters
    };
    ChunkConfig::new(ChunkCapacity::new(effective_max))
        .with_overlap(overlap)
        .map(|config| config.with_trim(trim))
        .map_err(|e| XbergError::validation(format!("Invalid chunking configuration: {}", e)))
}

/// Build chunks from text segments with optional page boundary tracking.
///
/// This function takes a collection of text segments (produced by a text splitter)
/// and constructs Chunk objects with proper metadata, including:
/// - Byte offsets derived from the chunk's position in the source text
/// - Chunk indices and total count
/// - Page boundary information (if provided)
///
/// # Arguments
///
/// * `source_text` - The original text that the chunks were split from. Chunk
///   slices must borrow from this text (as `text-splitter` guarantees).
/// * `text_chunks` - Iterator of text segments to convert into chunks
/// * `page_boundaries` - Optional page boundary markers for mapping chunks to pages
///
/// # Returns
///
/// A vector of Chunk objects with complete metadata.
///
/// # Errors
///
/// Returns an error if page boundary calculation fails.
pub(crate) fn build_chunks<'a, I>(
    source_text: &'a str,
    text_chunks: I,
    page_boundaries: Option<&[PageBoundary]>,
) -> Result<Vec<Chunk>>
where
    I: IntoIterator<Item = &'a str>,
{
    let chunks_vec: Vec<&str> = text_chunks.into_iter().collect();
    let total_chunks = chunks_vec.len();
    let source_start = source_text.as_ptr() as usize;
    let mut chunks = Vec::with_capacity(total_chunks);

    for (index, chunk_text) in chunks_vec.into_iter().enumerate() {
        let byte_start = chunk_text.as_ptr() as usize - source_start;
        let byte_end = byte_start + chunk_text.len();

        let (first_page, last_page, page_spans) = if let Some(boundaries) = page_boundaries {
            let (first_page, last_page) = calculate_page_range(byte_start, byte_end, boundaries)?;
            let page_spans = calculate_page_spans(byte_start, byte_end, boundaries)?;
            (first_page, last_page, page_spans)
        } else {
            (None, None, Vec::new())
        };

        chunks.push(Chunk {
            content: chunk_text.to_string(),
            chunk_type: classify_chunk(chunk_text, None),
            embedding: None,
            sparse_embedding: None,
            late_interaction: None,
            metadata: ChunkMetadata {
                byte_start,
                byte_end,
                token_count: None,
                chunk_index: index,
                total_chunks,
                first_page,
                last_page,
                heading_context: None,
                heading_path: Vec::new(),
                image_indices: Vec::new(),
                node_ids: Vec::new(),
                page_spans,
                classifications: Vec::new(),
            },
        });
    }

    Ok(chunks)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{HeadingContext, HeadingLevel};

    #[test]
    fn heading_path_from_context_none_returns_empty() {
        let path = heading_path_from_context(&None);
        assert!(path.is_empty(), "None heading_context must produce empty path");
    }

    #[test]
    fn heading_path_from_context_single_heading() {
        let ctx = Some(HeadingContext {
            headings: vec![HeadingLevel {
                level: 1,
                text: "Introduction".to_string(),
            }],
        });
        let path = heading_path_from_context(&ctx);
        assert_eq!(path, vec!["Introduction"]);
    }

    #[test]
    fn heading_path_from_context_nested_headings_outermost_first() {
        let ctx = Some(HeadingContext {
            headings: vec![
                HeadingLevel {
                    level: 1,
                    text: "Guide".to_string(),
                },
                HeadingLevel {
                    level: 2,
                    text: "Setup".to_string(),
                },
                HeadingLevel {
                    level: 3,
                    text: "Prerequisites".to_string(),
                },
            ],
        });
        let path = heading_path_from_context(&ctx);
        assert_eq!(path, vec!["Guide", "Setup", "Prerequisites"]);
        assert_eq!(path[0], "Guide");
        assert_eq!(path[2], "Prerequisites");
    }

    #[test]
    fn heading_path_from_context_empty_headings_vec() {
        let ctx = Some(HeadingContext { headings: vec![] });
        let path = heading_path_from_context(&ctx);
        assert!(path.is_empty());
    }

    #[test]
    fn test_build_chunk_config_valid() {
        let result = build_chunk_config(100, 10, true);
        assert!(result.is_ok());
    }

    #[test]
    fn test_build_chunk_config_zero_clamps_to_default() {
        let result = build_chunk_config(0, 0, true);
        assert!(result.is_ok(), "zero max_characters must be clamped, not panic");
    }

    #[test]
    fn test_build_chunk_config_invalid_overlap() {
        let result = build_chunk_config(10, 20, true);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, XbergError::Validation { .. }));
    }

    #[test]
    fn test_build_chunks_empty() {
        let source = "";
        let text_chunks: Vec<&str> = vec![];
        let result = build_chunks(source, text_chunks, None).unwrap();
        assert_eq!(result.len(), 0);
    }

    #[test]
    fn test_build_chunks_single() {
        let source = "Single chunk";
        let result = build_chunks(source, vec![source], None).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].content, "Single chunk");
        assert_eq!(result[0].metadata.chunk_index, 0);
        assert_eq!(result[0].metadata.total_chunks, 1);
        assert_eq!(result[0].metadata.byte_start, 0);
        assert_eq!(result[0].metadata.byte_end, 12);
    }

    #[test]
    fn test_build_chunks_with_page_boundaries() {
        let source = "First chunkSecond chunk";
        let text_chunks = vec![&source[0..11], &source[11..23]];
        let boundaries = vec![
            PageBoundary {
                byte_start: 0,
                byte_end: 11,
                page_number: 1,
            },
            PageBoundary {
                byte_start: 11,
                byte_end: 23,
                page_number: 2,
            },
        ];

        let result = build_chunks(source, text_chunks, Some(&boundaries)).unwrap();

        assert_eq!(result.len(), 2);
        assert_eq!(result[0].metadata.first_page, Some(1));
        assert_eq!(result[1].metadata.first_page, Some(2));
    }

    #[test]
    fn test_build_chunks_offset_from_source() {
        let source = "AAAAABBBBBCCCCC";
        let text_chunks = vec![&source[0..5], &source[3..8], &source[6..11]];
        let result = build_chunks(source, text_chunks, None).unwrap();

        assert_eq!(result.len(), 3);

        assert_eq!(result[0].metadata.byte_start, 0);
        assert_eq!(result[0].metadata.byte_end, 5);

        assert_eq!(result[1].metadata.byte_start, 3);
        assert_eq!(result[1].metadata.byte_end, 8);

        assert_eq!(result[2].metadata.byte_start, 6);
        assert_eq!(result[2].metadata.byte_end, 11);
    }

    #[test]
    fn test_build_chunks_no_overlap() {
        let source = "AAAAABBBBBCCCCC";
        let text_chunks = vec![&source[0..5], &source[5..10], &source[10..15]];
        let result = build_chunks(source, text_chunks, None).unwrap();

        assert_eq!(result.len(), 3);

        assert_eq!(result[0].metadata.byte_start, 0);
        assert_eq!(result[0].metadata.byte_end, 5);

        assert_eq!(result[1].metadata.byte_start, 5);
        assert_eq!(result[1].metadata.byte_end, 10);

        assert_eq!(result[2].metadata.byte_start, 10);
        assert_eq!(result[2].metadata.byte_end, 15);
    }
}
