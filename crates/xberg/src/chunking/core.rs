//! Core text chunking logic and public API.
//!
//! This module implements the main chunking algorithms and provides the primary
//! public API functions for splitting text into chunks.

use std::fmt::Write;

#[cfg(feature = "chunking-tokenizers")]
use crate::chunking::text_splitter::ChunkCapacity;
use crate::chunking::text_splitter::{ChunkConfig, ChunkSizer, MarkdownSplitter, TextSplitter};
use crate::error::Result;
use crate::types::{HeadingContext, PageBoundary};

#[cfg(feature = "chunking-tokenizers")]
use super::builder::TokenizerBackendSizer;
use super::builder::{build_chunk_config, build_chunks, resolve_token_counter};
use super::classifier::classify_chunk;
use super::config::{ChunkerType, ChunkingConfig, ChunkingResult, TableChunkingMode};
use super::headings::{build_heading_map, resolve_heading_context};
use super::validation::validate_utf8_boundaries;

/// Split text into chunks with optional page boundary tracking.
///
/// This is the primary API function for chunking text. It supports both plain text
/// and Markdown with configurable chunk size, overlap, and page boundary mapping.
///
/// # Arguments
///
/// * `text` - The text to split into chunks
/// * `config` - Chunking configuration (max size, overlap, type)
/// * `page_boundaries` - Optional page boundary markers for mapping chunks to pages
///
/// # Returns
///
/// A ChunkingResult containing all chunks and their metadata.
///
/// # Examples
///
/// ```rust
/// use xberg::chunking::{chunk_text, ChunkingConfig, ChunkerType};
///
/// # fn example() -> xberg::Result<()> {
/// let config = ChunkingConfig {
///     max_characters: 500,
///     overlap: 50,
///     trim: true,
///     chunker_type: ChunkerType::Text,
///     ..Default::default()
/// };
/// let result = chunk_text("Long text...", &config, None)?;
/// assert!(!result.chunks.is_empty());
/// # Ok(())
/// # }
/// ```
#[cfg_attr(alef, alef(skip))]
pub fn chunk_text(
    text: &str,
    config: &ChunkingConfig,
    page_boundaries: Option<&[PageBoundary]>,
) -> Result<ChunkingResult> {
    chunk_text_with_heading_source(text, config, page_boundaries, None)
}

/// Chunk text with an optional separate markdown source for heading context resolution.
///
/// When `heading_source` is provided, it is used instead of `text` for building the
/// heading map. This is needed when `text` is plain text (no markdown headings) but
/// the original document had headings that were stripped during rendering.
pub(crate) fn chunk_text_with_heading_source(
    text: &str,
    config: &ChunkingConfig,
    page_boundaries: Option<&[PageBoundary]>,
    heading_source: Option<&str>,
) -> Result<ChunkingResult> {
    if text.is_empty() {
        return Ok(ChunkingResult {
            chunks: vec![],
            chunk_count: 0,
        });
    }

    if let Some(boundaries) = page_boundaries {
        validate_utf8_boundaries(text, boundaries)?;
    }

    if config.chunker_type == ChunkerType::Yaml {
        return super::yaml_section::chunk_yaml_by_sections(text, config, page_boundaries);
    }

    if config.chunker_type == ChunkerType::Semantic {
        return super::semantic::chunk_semantic(text, config, page_boundaries);
    }

    let text_chunks: Vec<&str> = match &config.sizing {
        #[cfg(feature = "chunking-tokenizers")]
        crate::core::config::ChunkSizing::Tokenizer { model, .. } => {
            let registered = crate::plugins::registry::get_tokenizer_backend_registry()
                .read()
                .lookup(model);
            if let Some(backend) = registered {
                let chunk_config = ChunkConfig::new(ChunkCapacity::new(config.max_characters))
                    .with_sizer(TokenizerBackendSizer(backend))
                    .with_overlap(config.overlap)
                    .map(|c| c.with_trim(config.trim))
                    .map_err(|e| crate::XbergError::validation(format!("Invalid chunking configuration: {}", e)))?;
                split_with_config(text, &config.chunker_type, chunk_config)
            } else {
                let tokenizer = super::tokenizer_cache::get_or_init_tokenizer(model).map_err(|e| match e {
                    crate::XbergError::Validation { message, source } => crate::XbergError::Validation {
                        message: format!(
                            "{message}. No tokenizer backend is registered under '{model}' either; \
                             use a HuggingFace model id, or register your own tokenizer with \
                             register_tokenizer_backend."
                        ),
                        source,
                    },
                    other => other,
                })?;
                let chunk_config = ChunkConfig::new(ChunkCapacity::new(config.max_characters))
                    .with_sizer((*tokenizer).clone())
                    .with_overlap(config.overlap)
                    .map(|c| c.with_trim(config.trim))
                    .map_err(|e| crate::XbergError::validation(format!("Invalid chunking configuration: {}", e)))?;
                split_with_config(text, &config.chunker_type, chunk_config)
            }
        }
        _ => {
            let chunk_config = build_chunk_config(config.max_characters, config.overlap, config.trim)?;
            split_with_config(text, &config.chunker_type, chunk_config)
        }
    };

    let mut chunks = build_chunks(text, text_chunks, page_boundaries)?;

    if config.chunker_type == ChunkerType::Markdown {
        let heading_map = build_heading_map(heading_source.unwrap_or(text));
        if !heading_map.is_empty() {
            for chunk in &mut chunks {
                chunk.metadata.heading_context =
                    resolve_heading_context(chunk.metadata.byte_start, &heading_map, page_boundaries);
                // Reclassify now that heading context is known: heading-context-aware
                // rules (e.g. `is_schedule`) are dead at construction time, where
                // `build_chunks` always classifies with `heading_context = None` (#211).
                chunk.chunk_type = classify_chunk(&chunk.content, chunk.metadata.heading_context.as_ref());
            }

            // `chunk.content` is never mutated with a heading breadcrumb here (#1393
            // revised design, superseding the `BreadcrumbTarget::Content` mutation this
            // block used to perform for issue #337). `content` always stays exactly the
            // `[byte_start, byte_end)` source span — see the `#1294` note on
            // `ChunkMetadata::byte_start`/`byte_end` this closes: prepending used to make
            // `chunk.content.len() != byte_end - byte_start`, so slicing the original
            // document by the chunk's own offsets silently returned different text than
            // `content` whenever a breadcrumb had been prepended.
            //
            // `heading_context`/`heading_path` (set above, and derived from it by
            // `chunk_for_rag`) are the source of truth for the breadcrumb instead. Three
            // retrieval consumers want three different views of the same chunk — dense
            // wants the breadcrumb inline, BM25 wants it excluded or down-weighted,
            // SPLADE wants it out entirely — so rendering it into `content` is a
            // *consumer* decision applied at index time, not something the chunker
            // picks once for everyone. A caller that wants the `Content`-mode view for
            // the dense/embedding arm calls [`render_heading_breadcrumb`] explicitly with
            // the chunk's (clean) `content` and its `heading_context`.
            //
            // `prepend_heading_context` and `breadcrumb_target` are deprecated and now
            // inert: neither field has any effect on `content` any more. They are kept on
            // `ChunkingConfig` only so existing callers keep compiling; see
            // [`render_heading_breadcrumb`] for the explicit replacement.
        }
    }

    if config.chunker_type == ChunkerType::Markdown && config.table_chunking == TableChunkingMode::RepeatHeader {
        inject_table_headers(&mut chunks);
    }

    // Populate `token_count` from the configured tokenizer sizer, if any (#255).
    // Computed last so it reflects each chunk's final `content` (after any
    // table-header repetition above, the only remaining step that legitimately
    // changes `content` past `build_chunks`). `token_count` must describe the
    // content it is stored next to, so it is never counted against a
    // heading-breadcrumb prefix that no longer exists in `content` (#1393).
    if let Some(counter) = resolve_token_counter(&config.sizing) {
        for chunk in &mut chunks {
            chunk.metadata.token_count = Some(counter(&chunk.content));
        }
    }

    let chunk_count = chunks.len();

    Ok(ChunkingResult { chunks, chunk_count })
}

/// Render `content` with its Markdown heading breadcrumb prepended.
///
/// This is the explicit, consumer-applied rendering step described by the
/// revised #1393 design: [`chunk_text`] never mutates `chunk.content` with a
/// heading breadcrumb — `content` always stays exactly the source span
/// (`chunk.content == &source[byte_start..byte_end]`), regardless of the
/// deprecated `prepend_heading_context`/`breadcrumb_target` config fields.
/// A caller that wants the breadcrumb inline — e.g. for the dense/embedding
/// retrieval arm, which benefits from a self-contained passage that carries
/// its own structural context — calls this function directly at index time
/// with the chunk's (clean) `content` and its
/// [`heading_context`](crate::types::ChunkMetadata::heading_context), and
/// keeps `content` itself untouched for BM25/SPLADE, which do not want the
/// breadcrumb at all. See the [`rag`](crate::chunking::rag) module docs for
/// the full retrieval-mode guidance.
///
/// Formats `context`'s heading hierarchy as a single breadcrumb line — each
/// heading rendered as ATX hashes followed by its text, joined by `" > "`
/// (e.g. `"# Guide > ## Setup"`) — then a blank line, then `content` with a
/// leading occurrence of the breadcrumb's deepest (last) heading stripped so
/// the heading is not duplicated when `content` itself already starts with
/// that heading line, as Markdown-chunked content commonly does (the
/// Markdown splitter keeps the heading that opens a section as the first
/// line of that section's first chunk).
///
/// # Examples
///
/// ```rust
/// use xberg::chunking::render_heading_breadcrumb;
/// use xberg::types::{HeadingContext, HeadingLevel};
///
/// let context = HeadingContext {
///     headings: vec![
///         HeadingLevel { level: 1, text: "Guide".to_string() },
///         HeadingLevel { level: 2, text: "Setup".to_string() },
///     ],
/// };
/// let rendered = render_heading_breadcrumb("## Setup\n\nInstall the dependencies.", &context);
/// assert_eq!(rendered, "# Guide > ## Setup\n\nInstall the dependencies.");
/// ```
#[cfg_attr(alef, alef(skip))]
pub fn render_heading_breadcrumb(content: &str, context: &HeadingContext) -> String {
    let mut new_content = String::with_capacity(content.len() + 64);
    for (i, h) in context.headings.iter().enumerate() {
        if i > 0 {
            new_content.push_str(" > ");
        }
        for _ in 0..h.level {
            new_content.push('#');
        }
        let _ = write!(new_content, " {}", h.text);
    }
    new_content.push_str("\n\n");

    let body = match context.headings.last() {
        Some(h) => strip_leading_heading(content, h.level, &h.text),
        None => content,
    };
    new_content.push_str(body);
    new_content
}

/// If `text` starts with a markdown ATX heading matching `level` and `title`,
/// return the remainder after that heading line with leading newlines trimmed.
/// Otherwise return the input unchanged.
///
/// Handles optional closing ATX hashes (e.g. `## Heading ##`).
fn strip_leading_heading<'a>(text: &'a str, level: u8, title: &str) -> &'a str {
    debug_assert!(level > 0, "heading level must be 1..=6");
    let n = level as usize;
    let bytes = text.as_bytes();
    if bytes.len() <= n || bytes[..n].iter().any(|&b| b != b'#') || bytes[n] != b' ' {
        return text;
    }
    let after_prefix = &text[n + 1..];
    if !after_prefix.starts_with(title) {
        return text;
    }
    let rest = &after_prefix[title.len()..];
    let line_end = rest.find('\n').unwrap_or(rest.len());
    rest[line_end..].trim_start_matches('\n')
}

/// Split text using the appropriate splitter type with a generic sizer.
fn split_with_config<'a, S: ChunkSizer>(
    text: &'a str,
    chunker_type: &ChunkerType,
    config: ChunkConfig<S>,
) -> Vec<&'a str> {
    match chunker_type {
        ChunkerType::Text | ChunkerType::Yaml | ChunkerType::Semantic => {
            TextSplitter::new(config).chunks(text).collect()
        }
        ChunkerType::Markdown => MarkdownSplitter::new(config).chunks(text).collect(),
    }
}

/// Prepend the table header to every chunk that is a continuation of a split table.
///
/// A "table header" is two consecutive lines where the first starts with `|` and
/// the second is a separator (`|---…` or `|:--…`). A "continuation chunk" is one
/// that starts with a `|`-prefixed line but does NOT already have a separator line
/// within its first two lines — i.e. it is mid-table rows without a header.
///
/// Only called when `table_chunking == RepeatHeader` and `chunker_type == Markdown`.
fn inject_table_headers(chunks: &mut [crate::types::Chunk]) {
    fn extract_table_header(content: &str) -> Option<String> {
        let line_ending = if content.contains("\r\n") { "\r\n" } else { "\n" };

        let mut lines = content.lines();
        let first_raw = lines.next()?;
        let first = strip_line_terminator(first_raw);
        if !first.starts_with('|') {
            return None;
        }
        let second_raw = lines.next()?;
        let second = strip_line_terminator(second_raw);
        if !is_table_separator(second) {
            return None;
        }
        Some(format!("{first}{line_ending}{second}{line_ending}"))
    }

    fn strip_line_terminator(line: &str) -> &str {
        line.strip_suffix("\r\n")
            .or_else(|| line.strip_suffix('\n'))
            .unwrap_or(line)
    }

    fn is_table_separator(line: &str) -> bool {
        if !line.starts_with('|') {
            return false;
        }
        let cells: Vec<&str> = line.split('|').filter(|c| !c.is_empty()).collect();
        if cells.is_empty() {
            return false;
        }
        cells.iter().all(|cell| is_separator_cell(cell))
    }

    fn is_separator_cell(cell: &str) -> bool {
        let s = cell.trim_matches(|c: char| c == ' ' || c == '\t');
        let s = s.strip_prefix(':').unwrap_or(s);
        if !s.starts_with('-') {
            return false;
        }
        let after_dashes = s.trim_start_matches('-');
        let dash_count = s.len() - after_dashes.len();
        if dash_count < 2 {
            return false;
        }
        matches!(after_dashes, "" | ":")
    }

    fn is_table_continuation(content: &str) -> bool {
        let trimmed = content.trim_start();
        if !trimmed.starts_with('|') {
            return false;
        }
        let mut lines = trimmed.lines();
        let first = lines.next().unwrap_or("").trim();
        if !first.starts_with('|') {
            return false;
        }
        let second = lines.next().unwrap_or("").trim();
        !is_table_separator(second)
    }

    let mut last_header: Option<String> = None;

    for chunk in chunks.iter_mut() {
        let content = &chunk.content;

        if let Some(header) = extract_table_header(content) {
            last_header = Some(header);
        } else if is_table_continuation(content) {
            if let Some(ref header) = last_header {
                chunk.content = format!("{header}{}", chunk.content);
            }
        } else {
            last_header = None;
        }
    }
}

/// Chunk text with explicit type specification.
///
/// This is a convenience function that constructs a ChunkingConfig from individual
/// parameters and calls `chunk_text`.
///
/// # Arguments
///
/// * `text` - The text to split into chunks
/// * `max_characters` - Maximum characters per chunk
/// * `overlap` - Character overlap between consecutive chunks
/// * `trim` - Whether to trim whitespace from boundaries
/// * `chunker_type` - Type of chunker to use (Text or Markdown)
///
/// # Returns
///
/// A ChunkingResult containing all chunks and their metadata.
///
/// # Examples
///
/// This function is `#[cfg(test)]`, so it does not exist in a normal build and the example
/// cannot be compiled. The public route is [`chunk_text`], which takes a
/// [`ChunkingConfig`](crate::chunking::ChunkingConfig) carrying the same knobs.
///
/// ```ignore
/// use xberg::chunking::{chunk_text_with_type, ChunkerType};
///
/// # fn example() -> xberg::Result<()> {
/// let result = chunk_text_with_type("Some text", 500, 50, true, ChunkerType::Text)?;
/// assert!(!result.chunks.is_empty());
/// # Ok(())
/// # }
/// ```
#[cfg(test)]
pub(crate) fn chunk_text_with_type(
    text: &str,
    max_characters: usize,
    overlap: usize,
    trim: bool,
    chunker_type: ChunkerType,
) -> Result<ChunkingResult> {
    let config = ChunkingConfig {
        max_characters,
        overlap,
        trim,
        chunker_type,
        ..Default::default()
    };
    chunk_text(text, &config, None)
}

/// Batch process multiple texts with the same configuration.
///
/// This convenience function applies the same chunking configuration to multiple
/// texts in sequence.
///
/// # Arguments
///
/// * `texts` - Slice of text strings to chunk
/// * `config` - Chunking configuration to apply to all texts
///
/// # Returns
///
/// A vector of ChunkingResult objects, one per input text.
///
/// # Errors
///
/// Returns an error if chunking any individual text fails.
///
/// # Examples
///
/// This function is `#[cfg(test)]`, so it does not exist in a normal build and the example
/// cannot be compiled. The public route is [`chunk_text`], called once per input.
///
/// ```ignore
/// use xberg::chunking::{chunk_texts_batch, ChunkingConfig};
///
/// # fn example() -> xberg::Result<()> {
/// let config = ChunkingConfig::default();
/// let texts: Vec<String> = vec!["First text".to_string(), "Second text".to_string()];
/// let results = chunk_texts_batch(&texts, &config)?;
/// assert_eq!(results.len(), 2);
/// # Ok(())
/// # }
/// ```
#[cfg(test)]
pub(crate) fn chunk_texts_batch(texts: &[String], config: &ChunkingConfig) -> Result<Vec<ChunkingResult>> {
    texts.iter().map(|text| chunk_text(text, config, None)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::XbergError;

    #[test]
    fn test_chunk_empty_text() {
        let config = ChunkingConfig::default();
        let result = chunk_text("", &config, None).unwrap();
        assert_eq!(result.chunks.len(), 0);
        assert_eq!(result.chunk_count, 0);
    }

    #[test]
    fn test_chunk_short_text_single_chunk() {
        let config = ChunkingConfig {
            max_characters: 100,
            overlap: 10,
            trim: true,
            chunker_type: ChunkerType::Text,
            ..Default::default()
        };
        let text = "This is a short text.";
        let result = chunk_text(text, &config, None).unwrap();
        assert_eq!(result.chunks.len(), 1);
        assert_eq!(result.chunk_count, 1);
        assert_eq!(result.chunks[0].content, text);
    }

    #[test]
    fn test_chunk_long_text_multiple_chunks() {
        let config = ChunkingConfig {
            max_characters: 20,
            overlap: 5,
            trim: true,
            chunker_type: ChunkerType::Text,
            ..Default::default()
        };
        let text = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ";
        let result = chunk_text(text, &config, None).unwrap();
        assert!(result.chunk_count >= 2);
        assert_eq!(result.chunks.len(), result.chunk_count);
        assert!(result.chunks.iter().all(|chunk| chunk.content.len() <= 20));
    }

    #[test]
    fn test_chunk_text_with_overlap() {
        let config = ChunkingConfig {
            max_characters: 20,
            overlap: 5,
            trim: true,
            chunker_type: ChunkerType::Text,
            ..Default::default()
        };
        let text = "abcdefghijklmnopqrstuvwxyz0123456789";
        let result = chunk_text(text, &config, None).unwrap();
        assert!(result.chunk_count >= 2);

        if result.chunks.len() >= 2 {
            let first_chunk_end = &result.chunks[0].content[result.chunks[0].content.len().saturating_sub(5)..];
            assert!(
                result.chunks[1].content.starts_with(first_chunk_end),
                "Expected overlap '{}' at start of second chunk '{}'",
                first_chunk_end,
                result.chunks[1].content
            );
        }
    }

    #[test]
    fn test_chunk_markdown_preserves_structure() {
        let config = ChunkingConfig {
            max_characters: 50,
            overlap: 10,
            trim: true,
            chunker_type: ChunkerType::Markdown,
            ..Default::default()
        };
        let markdown = "# Title\n\nParagraph one.\n\n## Section\n\nParagraph two.";
        let result = chunk_text(markdown, &config, None).unwrap();
        assert!(result.chunk_count >= 1);
        assert!(result.chunks.iter().any(|chunk| chunk.content.contains("# Title")));
    }

    #[test]
    fn test_chunk_markdown_with_code_blocks() {
        let config = ChunkingConfig {
            max_characters: 100,
            overlap: 10,
            trim: true,
            chunker_type: ChunkerType::Markdown,
            ..Default::default()
        };
        let markdown = "# Code Example\n\n```python\nprint('hello')\n```\n\nSome text after code.";
        let result = chunk_text(markdown, &config, None).unwrap();
        assert!(result.chunk_count >= 1);
        assert!(result.chunks.iter().any(|chunk| chunk.content.contains("```")));
    }

    #[test]
    fn test_chunk_markdown_with_links() {
        let config = ChunkingConfig {
            max_characters: 80,
            overlap: 10,
            trim: true,
            chunker_type: ChunkerType::Markdown,
            ..Default::default()
        };
        let markdown = "Check out [this link](https://example.com) for more info.";
        let result = chunk_text(markdown, &config, None).unwrap();
        assert_eq!(result.chunk_count, 1);
        assert!(result.chunks[0].content.contains("[this link]"));
    }

    #[test]
    fn test_chunk_text_with_trim() {
        let config = ChunkingConfig {
            max_characters: 30,
            overlap: 5,
            trim: true,
            chunker_type: ChunkerType::Text,
            ..Default::default()
        };
        let text = "  Leading and trailing spaces  should be trimmed  ";
        let result = chunk_text(text, &config, None).unwrap();
        assert!(result.chunk_count >= 1);
        assert!(result.chunks.iter().all(|chunk| !chunk.content.starts_with(' ')));
    }

    #[test]
    fn test_chunk_text_without_trim() {
        let config = ChunkingConfig {
            max_characters: 30,
            overlap: 5,
            trim: false,
            chunker_type: ChunkerType::Text,
            ..Default::default()
        };
        let text = "  Text with spaces  ";
        let result = chunk_text(text, &config, None).unwrap();
        assert_eq!(result.chunk_count, 1);
        assert!(result.chunks[0].content.starts_with(' ') || result.chunks[0].content.len() < text.len());
    }

    #[test]
    fn test_chunk_with_invalid_overlap() {
        let config = ChunkingConfig {
            max_characters: 10,
            overlap: 20,
            trim: true,
            chunker_type: ChunkerType::Text,
            ..Default::default()
        };
        let result = chunk_text("Some text", &config, None);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, XbergError::Validation { .. }));
    }

    #[test]
    fn test_chunk_text_with_type_text() {
        let result = chunk_text_with_type("Simple text", 50, 10, true, ChunkerType::Text).unwrap();
        assert_eq!(result.chunk_count, 1);
        assert_eq!(result.chunks[0].content, "Simple text");
    }

    #[test]
    fn test_chunk_text_with_type_markdown() {
        let markdown = "# Header\n\nContent here.";
        let result = chunk_text_with_type(markdown, 50, 10, true, ChunkerType::Markdown).unwrap();
        assert_eq!(result.chunk_count, 1);
        assert!(result.chunks[0].content.contains("# Header"));
    }

    #[test]
    fn test_chunk_texts_batch_empty() {
        let config = ChunkingConfig::default();
        let texts: Vec<String> = vec![];
        let results = chunk_texts_batch(&texts, &config).unwrap();
        assert_eq!(results.len(), 0);
    }

    #[test]
    fn test_chunk_texts_batch_multiple() {
        let config = ChunkingConfig {
            max_characters: 30,
            overlap: 5,
            trim: true,
            chunker_type: ChunkerType::Text,
            ..Default::default()
        };
        let texts: Vec<String> = vec![
            "First text".to_string(),
            "Second text".to_string(),
            "Third text".to_string(),
        ];
        let results = chunk_texts_batch(&texts, &config).unwrap();
        assert_eq!(results.len(), 3);
        assert!(results.iter().all(|r| r.chunk_count >= 1));
    }

    #[test]
    fn test_chunk_texts_batch_mixed_lengths() {
        let config = ChunkingConfig {
            max_characters: 20,
            overlap: 5,
            trim: true,
            chunker_type: ChunkerType::Text,
            ..Default::default()
        };
        let texts: Vec<String> = vec![
            "Short".to_string(),
            "This is a longer text that should be split into multiple chunks".to_string(),
            String::new(),
        ];
        let results = chunk_texts_batch(&texts, &config).unwrap();
        assert_eq!(results.len(), 3);
        assert_eq!(results[0].chunk_count, 1);
        assert!(results[1].chunk_count > 1);
        assert_eq!(results[2].chunk_count, 0);
    }

    #[test]
    fn test_chunk_texts_batch_error_propagation() {
        let config = ChunkingConfig {
            max_characters: 10,
            overlap: 20,
            trim: true,
            chunker_type: ChunkerType::Text,
            ..Default::default()
        };
        let texts: Vec<String> = vec!["Text one".to_string(), "Text two".to_string()];
        let result = chunk_texts_batch(&texts, &config);
        assert!(result.is_err());
    }

    #[test]
    fn test_chunking_config_default() {
        let config = ChunkingConfig::default();
        assert_eq!(config.max_characters, 1000);
        assert_eq!(config.overlap, 200);
        assert!(config.trim);
        assert_eq!(config.chunker_type, ChunkerType::Text);
    }

    #[test]
    fn test_chunk_very_long_text() {
        let config = ChunkingConfig {
            max_characters: 100,
            overlap: 20,
            trim: true,
            chunker_type: ChunkerType::Text,
            ..Default::default()
        };
        let text = "a".repeat(1000);
        let result = chunk_text(&text, &config, None).unwrap();
        assert!(result.chunk_count >= 10);
        assert!(result.chunks.iter().all(|chunk| chunk.content.len() <= 100));
    }

    #[test]
    fn test_chunk_text_with_newlines() {
        let config = ChunkingConfig {
            max_characters: 30,
            overlap: 5,
            trim: true,
            chunker_type: ChunkerType::Text,
            ..Default::default()
        };
        let text = "Line one\nLine two\nLine three\nLine four\nLine five";
        let result = chunk_text(text, &config, None).unwrap();
        assert!(result.chunk_count >= 1);
    }

    #[test]
    fn test_chunk_markdown_with_lists() {
        let config = ChunkingConfig {
            max_characters: 100,
            overlap: 10,
            trim: true,
            chunker_type: ChunkerType::Markdown,
            ..Default::default()
        };
        let markdown = "# List Example\n\n- Item 1\n- Item 2\n- Item 3\n\nMore text.";
        let result = chunk_text(markdown, &config, None).unwrap();
        assert!(result.chunk_count >= 1);
        assert!(result.chunks.iter().any(|chunk| chunk.content.contains("- Item")));
    }

    #[test]
    fn test_chunk_markdown_with_tables() {
        let config = ChunkingConfig {
            max_characters: 150,
            overlap: 10,
            trim: true,
            chunker_type: ChunkerType::Markdown,
            ..Default::default()
        };
        let markdown = "# Table\n\n| Col1 | Col2 |\n|------|------|\n| A    | B    |\n| C    | D    |";
        let result = chunk_text(markdown, &config, None).unwrap();
        assert!(result.chunk_count >= 1);
        assert!(result.chunks.iter().any(|chunk| chunk.content.contains("|")));
    }

    #[test]
    fn test_chunk_special_characters() {
        let config = ChunkingConfig {
            max_characters: 50,
            overlap: 5,
            trim: true,
            chunker_type: ChunkerType::Text,
            ..Default::default()
        };
        let text = "Special chars: @#$%^&*()[]{}|\\<>?/~`";
        let result = chunk_text(text, &config, None).unwrap();
        assert_eq!(result.chunk_count, 1);
        assert!(result.chunks[0].content.contains("@#$%"));
    }

    #[test]
    fn test_chunk_unicode_characters() {
        let config = ChunkingConfig {
            max_characters: 50,
            overlap: 5,
            trim: true,
            chunker_type: ChunkerType::Text,
            ..Default::default()
        };
        let text = "Unicode: 你好世界 🌍 café résumé";
        let result = chunk_text(text, &config, None).unwrap();
        assert_eq!(result.chunk_count, 1);
        assert!(result.chunks[0].content.contains("你好"));
        assert!(result.chunks[0].content.contains("🌍"));
    }

    #[test]
    fn test_chunk_cjk_text() {
        let config = ChunkingConfig {
            max_characters: 30,
            overlap: 5,
            trim: true,
            chunker_type: ChunkerType::Text,
            ..Default::default()
        };
        let text = "日本語のテキストです。これは長い文章で、複数のチャンクに分割されるべきです。";
        let result = chunk_text(text, &config, None).unwrap();
        assert!(result.chunk_count >= 1);
    }

    /// Core invariant of the #1393 revised design: `chunk.content` is *always* the
    /// exact `[byte_start, byte_end)` span of the original document, byte-for-byte,
    /// even with the (deprecated) `prepend_heading_context` flag enabled. Before this
    /// fix, `BreadcrumbTarget::Content` (the default) prepended a breadcrumb into
    /// `content` without adjusting `byte_start`/`byte_end` — the `#1294` NOTE this
    /// function used to sit under admitted as much — so slicing the source document
    /// by a chunk's own offsets returned different text than `chunk.content` for
    /// every chunk under a heading. This test fails against that old behavior and
    /// passes now that `content` is never mutated.
    #[test]
    fn chunk_content_matches_source_span_byte_for_byte_when_heading_breadcrumb_enabled() {
        let original_document = "# Title\n\nSome text\n\n## Section\n\nMore text that will not be merged back up";
        let config = ChunkingConfig {
            max_characters: 50,
            overlap: 0,
            trim: true,
            chunker_type: ChunkerType::Markdown,
            prepend_heading_context: true,
            ..Default::default()
        };

        let result = chunk_text(original_document, &config, None).unwrap();
        assert!(!result.chunks.is_empty());

        let mut checked_a_chunk_under_a_heading = false;
        for chunk in &result.chunks {
            assert_eq!(
                chunk.content,
                &original_document[chunk.metadata.byte_start..chunk.metadata.byte_end],
                "chunk.content must be byte-identical to the source span it claims to cover"
            );
            if chunk.metadata.heading_context.is_some() {
                checked_a_chunk_under_a_heading = true;
            }
        }
        assert!(
            checked_a_chunk_under_a_heading,
            "test fixture must produce at least one chunk under a heading"
        );
    }

    /// `prepend_heading_context` is deprecated and now inert: setting it to `true`
    /// must not change `content` at all relative to leaving it `false` (#1393 revised
    /// design). `heading_context` is still populated so a caller can render the
    /// breadcrumb explicitly via `render_heading_breadcrumb` instead.
    #[test]
    fn prepend_heading_context_flag_is_deprecated_and_inert() {
        let markdown = "# Title\n\nSome text\n\n## Section\n\nMore text";
        let base_config = ChunkingConfig {
            max_characters: 50,
            overlap: 0,
            trim: true,
            chunker_type: ChunkerType::Markdown,
            ..Default::default()
        };
        let flagged_config = ChunkingConfig {
            prepend_heading_context: true,
            ..base_config.clone()
        };

        let baseline = chunk_text(markdown, &base_config, None).unwrap();
        let flagged = chunk_text(markdown, &flagged_config, None).unwrap();

        assert_eq!(baseline.chunks.len(), flagged.chunks.len());
        assert!(!flagged.chunks.is_empty());

        for (base, flagged) in baseline.chunks.iter().zip(flagged.chunks.iter()) {
            assert_eq!(
                base.content, flagged.content,
                "prepend_heading_context=true must produce byte-identical content to prepend_heading_context=false"
            );
        }

        let has_heading_context = flagged.chunks.iter().any(|c| c.metadata.heading_context.is_some());
        assert!(
            has_heading_context,
            "heading_context must still be populated even though content is never mutated"
        );
    }

    /// Regression test for #1294 item 3, re-scoped to the #1393 revised design:
    /// `byte_start`/`byte_end` must index the source text regardless of
    /// `prepend_heading_context`, and page-range attribution derived from those
    /// offsets must be unaffected. Since `content` is never mutated any more, the
    /// baseline and flagged runs must now match on `content` too, not just offsets.
    #[test]
    fn prepend_heading_context_does_not_desync_byte_offsets_or_page_range() {
        let p1 = "Body text for page one goes here";
        let p2 = "Body text for page two goes here";
        let source = format!("# Title\n\n{p1}\n\n## Section\n\n{p2}");

        let boundaries = vec![
            PageBoundary {
                page_number: 1,
                byte_start: 0,
                byte_end: "# Title\n\n".len() + p1.len(),
            },
            PageBoundary {
                page_number: 2,
                byte_start: "# Title\n\n".len() + p1.len(),
                byte_end: source.len(),
            },
        ];

        let base_config = ChunkingConfig {
            max_characters: 40,
            overlap: 0,
            trim: true,
            chunker_type: ChunkerType::Markdown,
            ..Default::default()
        };
        let prepend_config = ChunkingConfig {
            prepend_heading_context: true,
            ..base_config.clone()
        };

        let baseline = chunk_text(&source, &base_config, Some(&boundaries)).unwrap();
        let flagged = chunk_text(&source, &prepend_config, Some(&boundaries)).unwrap();

        assert_eq!(baseline.chunks.len(), flagged.chunks.len());
        assert!(!flagged.chunks.is_empty());

        for (base, flagged) in baseline.chunks.iter().zip(flagged.chunks.iter()) {
            assert_eq!(
                base.metadata.byte_start, flagged.metadata.byte_start,
                "prepend_heading_context must not shift byte_start"
            );
            assert_eq!(
                base.metadata.byte_end, flagged.metadata.byte_end,
                "prepend_heading_context must not shift byte_end"
            );
            assert_eq!(
                base.metadata.first_page, flagged.metadata.first_page,
                "prepend_heading_context must not change page-range attribution"
            );
            assert_eq!(base.metadata.last_page, flagged.metadata.last_page);
            assert!(
                flagged.metadata.byte_end <= source.len(),
                "byte_end must index into the source text length"
            );
            assert_eq!(
                base.content, flagged.content,
                "prepend_heading_context must not change content at all any more"
            );
        }

        let has_page_provenance = flagged.chunks.iter().any(|c| c.metadata.first_page.is_some());
        assert!(
            has_page_provenance,
            "page-range attribution must survive regardless of prepend_heading_context"
        );
    }

    /// `render_heading_breadcrumb` is the explicit rendering step that replaces the
    /// old in-place mutation (#1393 revised design): a caller who wants the
    /// `Content`-mode breadcrumb for the dense/embedding retrieval arm now calls it
    /// directly on a chunk's (always-clean) `content` instead of relying on the
    /// chunker to have mutated it. Uses an explicit `HeadingContext`, independent of
    /// the Markdown splitter's own boundary choices, so it only exercises the
    /// rendering logic itself.
    #[test]
    fn render_heading_breadcrumb_reconstructs_the_expected_breadcrumb_prefixed_text() {
        use crate::types::HeadingLevel;

        let context = HeadingContext {
            headings: vec![
                HeadingLevel {
                    level: 1,
                    text: "Title".to_string(),
                },
                HeadingLevel {
                    level: 2,
                    text: "Section".to_string(),
                },
            ],
        };
        let clean_content = "## Section\n\nMore text that will not be merged back up";

        let rendered = render_heading_breadcrumb(clean_content, &context);

        assert_eq!(
            rendered, "# Title > ## Section\n\nMore text that will not be merged back up",
            "render_heading_breadcrumb must prepend the full heading path and strip the duplicated leading heading line"
        );
    }

    #[test]
    fn should_join_heading_levels_with_the_configured_separator() {
        use crate::types::HeadingLevel;

        let context = HeadingContext {
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
        };

        let rendered = render_heading_breadcrumb("Install the dependencies.", &context);

        assert_eq!(
            rendered, "# Guide > ## Setup > ### Prerequisites\n\nInstall the dependencies.",
            "each heading level must render as ATX hashes and join with ' > ', in order"
        );
    }

    #[test]
    fn should_render_single_heading_without_a_separator() {
        use crate::types::HeadingLevel;

        let context = HeadingContext {
            headings: vec![HeadingLevel {
                level: 2,
                text: "Setup".to_string(),
            }],
        };

        let rendered = render_heading_breadcrumb("Install the dependencies.", &context);

        assert_eq!(
            rendered, "## Setup\n\nInstall the dependencies.",
            "a single heading must not have a ' > ' separator appended"
        );
    }

    #[test]
    fn should_prepend_only_a_blank_line_when_there_are_no_headings() {
        let context = HeadingContext { headings: vec![] };

        let rendered = render_heading_breadcrumb("Install the dependencies.", &context);

        assert_eq!(
            rendered, "\n\nInstall the dependencies.",
            "no headings means no breadcrumb text, but the blank-line separator is still emitted \
             and content is passed through unstripped"
        );
    }

    #[test]
    fn should_strip_a_leading_duplicate_of_the_deepest_heading_from_content() {
        use crate::types::HeadingLevel;

        let context = HeadingContext {
            headings: vec![
                HeadingLevel {
                    level: 1,
                    text: "Guide".to_string(),
                },
                HeadingLevel {
                    level: 2,
                    text: "Setup".to_string(),
                },
            ],
        };

        // The Markdown splitter commonly keeps the opening heading as the first
        // line of a section's first chunk; render_heading_breadcrumb must strip
        // that one leading occurrence so the heading is not duplicated.
        let rendered = render_heading_breadcrumb("## Setup\n\nInstall the dependencies.", &context);

        assert_eq!(
            rendered, "# Guide > ## Setup\n\nInstall the dependencies.",
            "a leading occurrence of the deepest heading in content must be stripped, not duplicated"
        );
    }

    #[test]
    fn test_strip_leading_heading_basic() {
        assert_eq!(strip_leading_heading("## Section\n\nBody", 2, "Section"), "Body");
    }

    #[test]
    fn test_strip_leading_heading_closing_atx() {
        assert_eq!(strip_leading_heading("## Section ##\n\nBody", 2, "Section"), "Body");
    }

    #[test]
    fn test_strip_leading_heading_no_match() {
        let text = "Some paragraph text";
        assert_eq!(strip_leading_heading(text, 2, "Section"), text);
    }

    #[test]
    fn test_strip_leading_heading_wrong_level() {
        let text = "### Section\n\nBody";
        assert_eq!(strip_leading_heading(text, 2, "Section"), text);
    }

    #[test]
    fn test_strip_leading_heading_single_newline() {
        assert_eq!(strip_leading_heading("# Title\nBody", 1, "Title"), "Body");
    }

    #[test]
    fn test_strip_leading_heading_no_body() {
        assert_eq!(strip_leading_heading("## Section", 2, "Section"), "");
    }

    #[test]
    fn test_strip_leading_heading_empty_input() {
        assert_eq!(strip_leading_heading("", 2, "Section"), "");
    }

    #[test]
    fn test_strip_leading_heading_unicode() {
        assert_eq!(
            strip_leading_heading("## Übersicht\n\nInhalt", 2, "Übersicht"),
            "Inhalt"
        );
        assert_eq!(strip_leading_heading("# 概要\n\n本文", 1, "概要"), "本文");
    }

    #[test]
    fn test_chunk_mixed_languages() {
        let config = ChunkingConfig {
            max_characters: 40,
            overlap: 5,
            trim: true,
            chunker_type: ChunkerType::Text,
            ..Default::default()
        };
        let text = "English text mixed with 中文文本 and some français";
        let result = chunk_text(text, &config, None).unwrap();
        assert!(result.chunk_count >= 1);
    }

    #[test]
    fn test_chunk_offset_calculation_with_overlap() {
        let config = ChunkingConfig {
            max_characters: 20,
            overlap: 5,
            trim: false,
            chunker_type: ChunkerType::Text,
            ..Default::default()
        };
        let text = "AAAAA BBBBB CCCCC DDDDD EEEEE FFFFF";
        let result = chunk_text(text, &config, None).unwrap();

        assert!(result.chunks.len() >= 2, "Expected at least 2 chunks");

        for i in 0..result.chunks.len() {
            let chunk = &result.chunks[i];
            let metadata = &chunk.metadata;

            assert_eq!(
                metadata.byte_end - metadata.byte_start,
                chunk.content.len(),
                "Chunk {} offset range doesn't match content length",
                i
            );

            assert_eq!(metadata.chunk_index, i);
            assert_eq!(metadata.total_chunks, result.chunks.len());
        }

        for i in 0..result.chunks.len() - 1 {
            let current_chunk = &result.chunks[i];
            let next_chunk = &result.chunks[i + 1];

            assert!(
                next_chunk.metadata.byte_start < current_chunk.metadata.byte_end,
                "Chunk {} and {} don't overlap: next starts at {} but current ends at {}",
                i,
                i + 1,
                next_chunk.metadata.byte_start,
                current_chunk.metadata.byte_end
            );

            let overlap_size = current_chunk.metadata.byte_end - next_chunk.metadata.byte_start;
            assert!(
                overlap_size <= config.overlap + 10,
                "Overlap between chunks {} and {} is too large: {}",
                i,
                i + 1,
                overlap_size
            );
        }
    }

    #[test]
    fn test_chunk_offset_calculation_without_overlap() {
        let config = ChunkingConfig {
            max_characters: 20,
            overlap: 0,
            trim: false,
            chunker_type: ChunkerType::Text,
            ..Default::default()
        };
        let text = "AAAAA BBBBB CCCCC DDDDD EEEEE FFFFF";
        let result = chunk_text(text, &config, None).unwrap();

        for i in 0..result.chunks.len() - 1 {
            let current_chunk = &result.chunks[i];
            let next_chunk = &result.chunks[i + 1];

            assert!(
                next_chunk.metadata.byte_start >= current_chunk.metadata.byte_end,
                "Chunk {} and {} overlap when they shouldn't: next starts at {} but current ends at {}",
                i,
                i + 1,
                next_chunk.metadata.byte_start,
                current_chunk.metadata.byte_end
            );
        }
    }

    #[test]
    fn test_chunk_offset_covers_full_text() {
        let config = ChunkingConfig {
            max_characters: 15,
            overlap: 3,
            trim: false,
            chunker_type: ChunkerType::Text,
            ..Default::default()
        };
        let text = "0123456789 ABCDEFGHIJ KLMNOPQRST UVWXYZ";
        let result = chunk_text(text, &config, None).unwrap();

        assert!(result.chunks.len() >= 2, "Expected multiple chunks");

        assert_eq!(
            result.chunks[0].metadata.byte_start, 0,
            "First chunk should start at position 0"
        );

        for i in 0..result.chunks.len() - 1 {
            let current_chunk = &result.chunks[i];
            let next_chunk = &result.chunks[i + 1];

            assert!(
                next_chunk.metadata.byte_start <= current_chunk.metadata.byte_end,
                "Gap detected between chunk {} (ends at {}) and chunk {} (starts at {})",
                i,
                current_chunk.metadata.byte_end,
                i + 1,
                next_chunk.metadata.byte_start
            );
        }
    }

    #[test]
    fn test_chunk_offset_with_various_overlap_sizes() {
        for overlap in [0, 5, 10, 20] {
            let config = ChunkingConfig {
                max_characters: 30,
                overlap,
                trim: false,
                chunker_type: ChunkerType::Text,
                ..Default::default()
            };
            let text = "Word ".repeat(30);
            let result = chunk_text(&text, &config, None).unwrap();

            for chunk in &result.chunks {
                assert!(
                    chunk.metadata.byte_end > chunk.metadata.byte_start,
                    "Invalid offset range for overlap {}: start={}, end={}",
                    overlap,
                    chunk.metadata.byte_start,
                    chunk.metadata.byte_end
                );
            }

            for chunk in &result.chunks {
                assert!(
                    chunk.metadata.byte_start < text.len(),
                    "char_start with overlap {} is out of bounds: {}",
                    overlap,
                    chunk.metadata.byte_start
                );
            }
        }
    }

    #[test]
    fn test_chunk_last_chunk_offset() {
        let config = ChunkingConfig {
            max_characters: 20,
            overlap: 5,
            trim: false,
            chunker_type: ChunkerType::Text,
            ..Default::default()
        };
        let text = "AAAAA BBBBB CCCCC DDDDD EEEEE";
        let result = chunk_text(text, &config, None).unwrap();

        assert!(result.chunks.len() >= 2, "Need multiple chunks for this test");

        let last_chunk = result.chunks.last().unwrap();
        let second_to_last = &result.chunks[result.chunks.len() - 2];

        assert!(
            last_chunk.metadata.byte_start < second_to_last.metadata.byte_end,
            "Last chunk should overlap with previous chunk"
        );

        let expected_end = text.len();
        let last_chunk_covers_end =
            last_chunk.content.trim_end() == text.trim_end() || last_chunk.metadata.byte_end >= expected_end - 5;
        assert!(last_chunk_covers_end, "Last chunk should cover the end of the text");
    }

    #[test]
    fn test_chunk_with_page_boundaries() {
        let config = ChunkingConfig {
            max_characters: 30,
            overlap: 5,
            trim: true,
            chunker_type: ChunkerType::Text,
            ..Default::default()
        };
        let text = "Page one content here. Page two starts here and continues.";

        let boundaries = vec![
            PageBoundary {
                byte_start: 0,
                byte_end: 21,
                page_number: 1,
            },
            PageBoundary {
                byte_start: 22,
                byte_end: 58,
                page_number: 2,
            },
        ];

        let result = chunk_text(text, &config, Some(&boundaries)).unwrap();
        assert!(result.chunks.len() >= 2);

        assert_eq!(result.chunks[0].metadata.first_page, Some(1));

        let last_chunk = result.chunks.last().unwrap();
        assert_eq!(last_chunk.metadata.last_page, Some(2));
    }

    #[test]
    fn test_chunk_without_page_boundaries() {
        let config = ChunkingConfig {
            max_characters: 30,
            overlap: 5,
            trim: true,
            chunker_type: ChunkerType::Text,
            ..Default::default()
        };
        let text = "This is some test content that should be split into multiple chunks.";

        let result = chunk_text(text, &config, None).unwrap();
        assert!(result.chunks.len() >= 2);

        for chunk in &result.chunks {
            assert_eq!(chunk.metadata.first_page, None);
            assert_eq!(chunk.metadata.last_page, None);
        }
    }

    #[test]
    fn test_chunk_empty_boundaries() {
        let config = ChunkingConfig {
            max_characters: 30,
            overlap: 5,
            trim: true,
            chunker_type: ChunkerType::Text,
            ..Default::default()
        };
        let text = "Some text content here.";
        let boundaries: Vec<PageBoundary> = vec![];

        let result = chunk_text(text, &config, Some(&boundaries)).unwrap();
        assert_eq!(result.chunks.len(), 1);

        assert_eq!(result.chunks[0].metadata.first_page, None);
        assert_eq!(result.chunks[0].metadata.last_page, None);
    }

    #[test]
    fn test_chunk_spanning_multiple_pages() {
        let config = ChunkingConfig {
            max_characters: 50,
            overlap: 5,
            trim: false,
            chunker_type: ChunkerType::Text,
            ..Default::default()
        };
        let text = "0123456789 AAAAAAAAAA 1111111111 BBBBBBBBBB 2222222222";

        let boundaries = vec![
            PageBoundary {
                byte_start: 0,
                byte_end: 20,
                page_number: 1,
            },
            PageBoundary {
                byte_start: 20,
                byte_end: 40,
                page_number: 2,
            },
            PageBoundary {
                byte_start: 40,
                byte_end: 54,
                page_number: 3,
            },
        ];

        let result = chunk_text(text, &config, Some(&boundaries)).unwrap();
        assert!(result.chunks.len() >= 2);

        for chunk in &result.chunks {
            assert!(chunk.metadata.first_page.is_some() || chunk.metadata.last_page.is_some());
        }
    }

    #[test]
    fn test_chunk_text_with_invalid_boundary_range() {
        let config = ChunkingConfig {
            max_characters: 30,
            overlap: 5,
            trim: true,
            chunker_type: ChunkerType::Text,
            ..Default::default()
        };
        let text = "Page one content here. Page two content.";

        let boundaries = vec![PageBoundary {
            byte_start: 10,
            byte_end: 5,
            page_number: 1,
        }];

        let result = chunk_text(text, &config, Some(&boundaries));
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("Invalid boundary range"));
        assert!(err.to_string().contains("byte_start"));
    }

    #[test]
    fn test_chunk_text_with_unsorted_boundaries() {
        let config = ChunkingConfig {
            max_characters: 30,
            overlap: 5,
            trim: true,
            chunker_type: ChunkerType::Text,
            ..Default::default()
        };
        let text = "Page one content here. Page two content.";

        let boundaries = vec![
            PageBoundary {
                byte_start: 22,
                byte_end: 40,
                page_number: 2,
            },
            PageBoundary {
                byte_start: 0,
                byte_end: 21,
                page_number: 1,
            },
        ];

        let result = chunk_text(text, &config, Some(&boundaries));
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("not sorted"));
        assert!(err.to_string().contains("boundaries"));
    }

    #[test]
    fn test_chunk_text_with_overlapping_boundaries() {
        let config = ChunkingConfig {
            max_characters: 30,
            overlap: 5,
            trim: true,
            chunker_type: ChunkerType::Text,
            ..Default::default()
        };
        let text = "Page one content here. Page two content.";

        let boundaries = vec![
            PageBoundary {
                byte_start: 0,
                byte_end: 25,
                page_number: 1,
            },
            PageBoundary {
                byte_start: 20,
                byte_end: 40,
                page_number: 2,
            },
        ];

        let result = chunk_text(text, &config, Some(&boundaries));
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("Overlapping"));
        assert!(err.to_string().contains("boundaries"));
    }

    #[test]
    fn test_chunk_with_pages_basic() {
        let config = ChunkingConfig {
            max_characters: 25,
            overlap: 5,
            trim: true,
            chunker_type: ChunkerType::Text,
            ..Default::default()
        };
        let text = "First page content here.Second page content here.Third page.";

        let boundaries = vec![
            PageBoundary {
                byte_start: 0,
                byte_end: 24,
                page_number: 1,
            },
            PageBoundary {
                byte_start: 24,
                byte_end: 50,
                page_number: 2,
            },
            PageBoundary {
                byte_start: 50,
                byte_end: 60,
                page_number: 3,
            },
        ];

        let result = chunk_text(text, &config, Some(&boundaries)).unwrap();

        if !result.chunks.is_empty() {
            assert!(result.chunks[0].metadata.first_page.is_some());
        }
    }

    #[test]
    fn test_chunk_with_pages_single_page_chunk() {
        let config = ChunkingConfig {
            max_characters: 100,
            overlap: 10,
            trim: true,
            chunker_type: ChunkerType::Text,
            ..Default::default()
        };
        let text = "All content on single page fits in one chunk.";

        let boundaries = vec![PageBoundary {
            byte_start: 0,
            byte_end: 45,
            page_number: 1,
        }];

        let result = chunk_text(text, &config, Some(&boundaries)).unwrap();
        assert_eq!(result.chunks.len(), 1);
        assert_eq!(result.chunks[0].metadata.first_page, Some(1));
        assert_eq!(result.chunks[0].metadata.last_page, Some(1));
    }

    #[test]
    fn test_chunk_with_pages_no_overlap() {
        let config = ChunkingConfig {
            max_characters: 20,
            overlap: 0,
            trim: false,
            chunker_type: ChunkerType::Text,
            ..Default::default()
        };
        let text = "AAAAA BBBBB CCCCC DDDDD";

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

        let result = chunk_text(text, &config, Some(&boundaries)).unwrap();
        assert!(!result.chunks.is_empty());

        for chunk in &result.chunks {
            if let (Some(first), Some(last)) = (chunk.metadata.first_page, chunk.metadata.last_page) {
                assert!(first <= last);
            }
        }
    }

    #[test]
    fn test_chunk_metadata_page_range_accuracy() {
        let config = ChunkingConfig {
            max_characters: 30,
            overlap: 5,
            trim: true,
            chunker_type: ChunkerType::Text,
            ..Default::default()
        };
        let text = "Page One Content Here.Page Two.";

        let boundaries = vec![
            PageBoundary {
                byte_start: 0,
                byte_end: 21,
                page_number: 1,
            },
            PageBoundary {
                byte_start: 21,
                byte_end: 31,
                page_number: 2,
            },
        ];

        let result = chunk_text(text, &config, Some(&boundaries)).unwrap();

        for chunk in &result.chunks {
            assert_eq!(chunk.metadata.byte_end - chunk.metadata.byte_start, chunk.content.len());
        }
    }

    /// Regression test for GitHub issue #439:
    /// Chunk metadata reports wrong page numbers for documents with many pages.
    /// The byte offset drift causes chunks near the end of the document to
    /// reference pages far earlier than where their content actually resides.
    #[test]
    fn test_issue_439_chunk_page_metadata_many_pages() {
        let num_pages = 50;
        let mut full_text = String::new();
        let mut boundaries = Vec::new();

        for p in 1..=num_pages {
            let page_content = format!(
                "Page {} content. This is the text on page number {}. It has some words to fill space here. ",
                p, p
            );
            let start = full_text.len();
            full_text.push_str(&page_content);
            let end = full_text.len();
            boundaries.push(PageBoundary {
                byte_start: start,
                byte_end: end,
                page_number: p,
            });
        }

        let config = ChunkingConfig {
            max_characters: 200,
            overlap: 50,
            trim: true,
            chunker_type: ChunkerType::Text,
            ..Default::default()
        };

        let result = chunk_text(&full_text, &config, Some(&boundaries)).unwrap();

        let last_chunk = result.chunks.last().unwrap();
        assert!(
            last_chunk.metadata.last_page.unwrap() >= num_pages - 2,
            "Last chunk should reference near the last page ({}), but got {:?}",
            num_pages,
            last_chunk.metadata.last_page
        );

        for (i, chunk) in result.chunks.iter().enumerate() {
            let actual_pos = full_text
                .find(&chunk.content)
                .expect("Chunk content must be a substring of the original text");
            let actual_page = boundaries
                .iter()
                .find(|b| actual_pos >= b.byte_start && actual_pos < b.byte_end)
                .map(|b| b.page_number);

            if let (Some(reported), Some(actual)) = (chunk.metadata.first_page, actual_page) {
                assert_eq!(
                    reported, actual,
                    "Chunk {} reports first_page={} but content starts on page {} \
                     (byte_start={}, actual_pos={})",
                    i, reported, actual, chunk.metadata.byte_start, actual_pos
                );
            }
        }
    }

    /// Verify that chunk byte_start/byte_end match the actual position of the
    /// chunk content within the original text.
    #[test]
    fn test_issue_439_chunk_byte_offsets_match_text_position() {
        let text = "Alpha bravo charlie delta echo foxtrot golf hotel india juliet kilo lima mike november oscar papa quebec romeo sierra tango uniform victor whiskey xray yankee zulu. ";
        let repeated = text.repeat(5);

        let boundaries = vec![
            PageBoundary {
                byte_start: 0,
                byte_end: text.len(),
                page_number: 1,
            },
            PageBoundary {
                byte_start: text.len(),
                byte_end: text.len() * 2,
                page_number: 2,
            },
            PageBoundary {
                byte_start: text.len() * 2,
                byte_end: text.len() * 3,
                page_number: 3,
            },
            PageBoundary {
                byte_start: text.len() * 3,
                byte_end: text.len() * 4,
                page_number: 4,
            },
            PageBoundary {
                byte_start: text.len() * 4,
                byte_end: text.len() * 5,
                page_number: 5,
            },
        ];

        let config = ChunkingConfig {
            max_characters: 80,
            overlap: 20,
            trim: true,
            chunker_type: ChunkerType::Text,
            ..Default::default()
        };

        let result = chunk_text(&repeated, &config, Some(&boundaries)).unwrap();

        for (i, chunk) in result.chunks.iter().enumerate() {
            let byte_start = chunk.metadata.byte_start;
            let byte_end = chunk.metadata.byte_end;
            assert!(
                byte_end <= repeated.len(),
                "Chunk {} byte_end ({}) exceeds text length ({})",
                i,
                byte_end,
                repeated.len()
            );
            assert_eq!(
                &repeated[byte_start..byte_end],
                chunk.content,
                "Chunk {} content doesn't match text at byte_start={}..byte_end={}",
                i,
                byte_start,
                byte_end
            );
        }
    }

    #[test]
    fn test_chunk_page_range_boundary_edge_cases() {
        let config = ChunkingConfig {
            max_characters: 10,
            overlap: 2,
            trim: false,
            chunker_type: ChunkerType::Text,
            ..Default::default()
        };
        let text = "0123456789ABCDEFGHIJ";

        let boundaries = vec![
            PageBoundary {
                byte_start: 0,
                byte_end: 10,
                page_number: 1,
            },
            PageBoundary {
                byte_start: 10,
                byte_end: 20,
                page_number: 2,
            },
        ];

        let result = chunk_text(text, &config, Some(&boundaries)).unwrap();

        for chunk in &result.chunks {
            let on_page1 = chunk.metadata.byte_start < 10;
            let on_page2 = chunk.metadata.byte_end > 10;

            if on_page1 && on_page2 {
                assert_eq!(chunk.metadata.first_page, Some(1));
                assert_eq!(chunk.metadata.last_page, Some(2));
            } else if on_page1 {
                assert_eq!(chunk.metadata.first_page, Some(1));
            } else if on_page2 {
                assert_eq!(chunk.metadata.first_page, Some(2));
            }
        }
    }

    fn make_large_table(rows: usize) -> String {
        let mut s = "| Name | Value | Description |\n|------|-------|-------------|\n".to_string();
        for i in 0..rows {
            s.push_str(&format!(
                "| item{i} | {i} | Description of item {i} with some extra text |\n"
            ));
        }
        s
    }

    #[test]
    fn table_repeat_header_prepends_to_continuation_chunks() {
        let markdown = make_large_table(40);
        let config = ChunkingConfig {
            max_characters: 300,
            overlap: 0,
            trim: true,
            chunker_type: ChunkerType::Markdown,
            table_chunking: TableChunkingMode::RepeatHeader,
            ..Default::default()
        };
        let result = chunk_text(&markdown, &config, None).unwrap();
        assert!(result.chunks.len() > 1, "table must split into multiple chunks");

        for chunk in &result.chunks {
            let trimmed = chunk.content.trim_start();
            if trimmed.starts_with('|') {
                assert!(
                    chunk.content.contains("|------|"),
                    "chunk missing separator row (header must be prepended):\n{:?}",
                    chunk.content,
                );
                assert!(
                    chunk.content.contains("| Name | Value | Description |"),
                    "chunk missing header row:\n{:?}",
                    chunk.content,
                );
            }
        }
    }

    #[test]
    fn table_split_mode_default_leaves_continuation_chunks_without_header() {
        let markdown = make_large_table(40);
        let config = ChunkingConfig {
            max_characters: 300,
            overlap: 0,
            trim: true,
            chunker_type: ChunkerType::Markdown,
            ..Default::default()
        };
        let result = chunk_text(&markdown, &config, None).unwrap();
        assert!(result.chunks.len() > 1, "table must split into multiple chunks");

        let continuation_without_header = result.chunks.iter().skip(1).any(|c| {
            let t = c.content.trim_start();
            t.starts_with('|') && !c.content.contains("|------|")
        });
        assert!(
            continuation_without_header,
            "default Split mode must not inject headers into continuation chunks"
        );
    }

    #[test]
    fn table_repeat_header_two_split_tables_each_get_own_header() {
        let table1 = "| Name | Value | Description |\n|------|-------|-------------|\n".to_string()
            + &"| item | val | some description text here |\n".repeat(30);
        let separator = "\n\n---\n\n";
        let table2 = "| Alpha | Beta | Gamma |\n|-------|------|-------|\n".to_string()
            + &"| alpha | beta | gamma value here |\n".repeat(30);
        let markdown = format!("{table1}{separator}{table2}");

        let config = ChunkingConfig {
            max_characters: 300,
            overlap: 0,
            trim: true,
            chunker_type: ChunkerType::Markdown,
            table_chunking: TableChunkingMode::RepeatHeader,
            ..Default::default()
        };
        let result = chunk_text(&markdown, &config, None).unwrap();
        assert!(result.chunks.len() > 2, "both tables must produce multiple chunks");

        for chunk in &result.chunks {
            let trimmed = chunk.content.trim_start();
            if !trimmed.starts_with('|') {
                continue;
            }
            let mut lines = trimmed.lines();
            lines.next();
            let second = lines.next().unwrap_or("").trim();
            assert!(
                second.starts_with('|') && second.contains('-'),
                "table chunk missing separator on second line:\n{:?}",
                chunk.content,
            );
        }

        for chunk in &result.chunks {
            if chunk.content.contains("alpha") || chunk.content.contains("Alpha") {
                assert!(
                    !chunk.content.contains("Name") || chunk.content.contains("Alpha"),
                    "table2 chunk must not be contaminated with table1 header:\n{:?}",
                    chunk.content,
                );
            }
        }
    }

    #[test]
    fn table_repeat_header_text_chunker_is_unaffected() {
        let markdown = make_large_table(40);
        let config = ChunkingConfig {
            max_characters: 300,
            overlap: 0,
            trim: true,
            chunker_type: ChunkerType::Text,
            table_chunking: TableChunkingMode::RepeatHeader,
            ..Default::default()
        };
        let result = chunk_text(&markdown, &config, None).unwrap();
        assert!(!result.chunks.is_empty());
    }

    #[test]
    fn table_repeat_header_single_chunk_table_unchanged() {
        let markdown = "| Col1 | Col2 |\n|------|------|\n| A    | B    |\n| C    | D    |\n";
        let config = ChunkingConfig {
            max_characters: 5000,
            overlap: 0,
            trim: true,
            chunker_type: ChunkerType::Markdown,
            table_chunking: TableChunkingMode::RepeatHeader,
            ..Default::default()
        };
        let result = chunk_text(markdown, &config, None).unwrap();
        assert_eq!(result.chunks.len(), 1, "small table must stay in one chunk");
        assert_eq!(
            result.chunks[0].content.matches("|------|").count(),
            1,
            "header must appear exactly once, not be duplicated"
        );
    }

    #[test]
    fn table_repeat_header_with_overlap_prepends_header_to_continuation_chunks() {
        let markdown = make_large_table(40);
        let config = ChunkingConfig {
            max_characters: 300,
            overlap: 50,
            trim: true,
            chunker_type: ChunkerType::Markdown,
            table_chunking: TableChunkingMode::RepeatHeader,
            ..Default::default()
        };
        let result = chunk_text(&markdown, &config, None).unwrap();
        assert!(
            result.chunks.len() > 2,
            "table must split into multiple chunks with overlap"
        );
        for chunk in &result.chunks {
            let lines: Vec<&str> = chunk.content.lines().filter(|l| !l.is_empty()).collect();
            if lines.first().is_some_and(|l| l.starts_with('|')) {
                let second_line_is_separator = lines
                    .get(1)
                    .is_some_and(|l| l.chars().all(|c| matches!(c, '|' | '-' | ' ')));
                assert!(
                    second_line_is_separator,
                    "table chunk must have separator on 2nd line; got:\n{}",
                    chunk.content
                );
            }
        }
    }

    #[test]
    fn table_repeat_header_oversized_header_does_not_panic() {
        let wide_cols: String = (0..20).map(|i| format!("| Column {i:02} ")).collect::<String>() + "|";
        let separator: String = (0..20).map(|_| "|----------").collect::<String>() + "|";
        let row: String = (0..20).map(|i| format!("| value  {i:02} ")).collect::<String>() + "|";
        let rows: String = (0..20).map(|_| format!("{row}\n")).collect::<String>();
        let markdown = format!("{wide_cols}\n{separator}\n{rows}");
        let config = ChunkingConfig {
            max_characters: 50,
            overlap: 0,
            trim: true,
            chunker_type: ChunkerType::Markdown,
            table_chunking: TableChunkingMode::RepeatHeader,
            ..Default::default()
        };
        let result = chunk_text(&markdown, &config, None).unwrap();
        assert!(
            !result.chunks.is_empty(),
            "must produce chunks even with oversized header"
        );
    }

    /// A data row whose cell contains a lone dash (`| - |`) must NOT be treated
    /// as a separator row and must NOT trigger spurious header injection.
    #[test]
    fn table_repeat_header_single_dash_data_cell_not_misidentified_as_separator() {
        let markdown = "| Item | Status |\n|------|--------|\n| foo  | - |\n| bar  | done |\n| baz  | :- |\n";
        let config = ChunkingConfig {
            max_characters: 5000,
            overlap: 0,
            trim: true,
            chunker_type: ChunkerType::Markdown,
            table_chunking: TableChunkingMode::RepeatHeader,
            ..Default::default()
        };
        let result = chunk_text(markdown, &config, None).unwrap();
        assert_eq!(result.chunks.len(), 1, "small table must fit in one chunk");
        assert_eq!(
            result.chunks[0].content.matches("|------|").count(),
            1,
            "separator must appear exactly once — no spurious injection from data rows with dashes:\n{:?}",
            result.chunks[0].content,
        );
        assert!(
            result.chunks[0].content.contains("| foo  | - |"),
            "data row with lone-dash cell must be preserved:\n{:?}",
            result.chunks[0].content,
        );
        assert!(
            result.chunks[0].content.contains("| baz  | :- |"),
            "data row with colon-dash cell must be preserved:\n{:?}",
            result.chunks[0].content,
        );
    }

    /// A CRLF-input table whose rows are split across chunks must produce continuation
    /// chunks where the injected header uses `\r\n` consistently with the body.
    #[test]
    fn table_repeat_header_crlf_input_header_uses_crlf_line_endings() {
        let header = "| Name | Value |\r\n|------|-------|\r\n";
        let data_rows: String = (0..40).map(|i| format!("| item{i:02} | {i:03} |\r\n")).collect();
        let markdown = format!("{header}{data_rows}");

        let config = ChunkingConfig {
            max_characters: 200,
            overlap: 0,
            trim: true,
            chunker_type: ChunkerType::Markdown,
            table_chunking: TableChunkingMode::RepeatHeader,
            ..Default::default()
        };
        let result = chunk_text(&markdown, &config, None).unwrap();
        assert!(result.chunks.len() > 1, "CRLF table must split into multiple chunks");

        for (i, chunk) in result.chunks.iter().enumerate().skip(1) {
            let trimmed = chunk.content.trim_start();
            if !trimmed.starts_with('|') {
                continue;
            }
            assert!(
                chunk.content.contains("\r\n"),
                "continuation chunk {} must contain \\r\\n line endings from injected header, but got:\n{:?}",
                i,
                chunk.content,
            );
            assert!(
                chunk.content.contains("| Name | Value |\r\n"),
                "continuation chunk {} injected header row must end with \\r\\n, not \\n:\n{:?}",
                i,
                chunk.content,
            );
            assert!(
                chunk.content.contains("|-------|\r\n"),
                "continuation chunk {} injected separator must end with \\r\\n, not \\n:\n{:?}",
                i,
                chunk.content,
            );
        }
    }

    /// Tables using GFM alignment separators (`:---`, `---:`, `:---:`) must still be
    /// correctly detected and their headers injected into continuation chunks.
    #[test]
    fn table_repeat_header_alignment_separators_are_detected_correctly() {
        let header = "| Left | Right | Center |\n|:-----|------:|:------:|\n";
        let data_rows: String = (0..40).map(|i| format!("| l{i:02} | r{i:02} | c{i:02} |\n")).collect();
        let markdown = format!("{header}{data_rows}");

        let config = ChunkingConfig {
            max_characters: 200,
            overlap: 0,
            trim: true,
            chunker_type: ChunkerType::Markdown,
            table_chunking: TableChunkingMode::RepeatHeader,
            ..Default::default()
        };
        let result = chunk_text(&markdown, &config, None).unwrap();
        assert!(
            result.chunks.len() > 1,
            "alignment-separator table must split into multiple chunks"
        );

        for chunk in &result.chunks {
            let trimmed = chunk.content.trim_start();
            if !trimmed.starts_with('|') {
                continue;
            }
            assert!(
                chunk.content.contains("| Left | Right | Center |"),
                "alignment-separator table chunk missing header row:\n{:?}",
                chunk.content,
            );
            assert!(
                chunk.content.contains("|:-----|------:|:------:|"),
                "alignment-separator table chunk missing alignment separator row:\n{:?}",
                chunk.content,
            );
        }
    }

    /// Regression test for #255: with `ChunkSizing::Tokenizer` pointed at a registered
    /// [`crate::plugins::TokenizerBackend`], `token_count` must be populated from that
    /// backend's count instead of staying `None` forever.
    #[cfg(feature = "chunking-tokenizers")]
    #[test]
    fn test_token_count_populated_from_registered_tokenizer_backend() {
        use crate::plugins::registry::test_support::TokenizerRegistryGuard;
        use crate::plugins::{Plugin, TokenizerBackend, register_tokenizer_backend};
        use std::sync::Arc;

        struct WordCountTokenizer;
        impl Plugin for WordCountTokenizer {
            fn name(&self) -> &str {
                "chunking-core-word-count-tokenizer"
            }
            fn version(&self) -> String {
                "1.0.0".to_string()
            }
            fn initialize(&self) -> crate::Result<()> {
                Ok(())
            }
            fn shutdown(&self) -> crate::Result<()> {
                Ok(())
            }
        }
        impl TokenizerBackend for WordCountTokenizer {
            fn count_tokens(&self, text: &str) -> usize {
                text.split_whitespace().count()
            }
        }

        let _guard = TokenizerRegistryGuard::acquire();
        register_tokenizer_backend(Arc::new(WordCountTokenizer)).unwrap();

        let config = ChunkingConfig {
            max_characters: 100,
            overlap: 0,
            trim: true,
            chunker_type: ChunkerType::Text,
            sizing: crate::core::config::ChunkSizing::Tokenizer {
                model: "chunking-core-word-count-tokenizer".to_string(),
                cache_dir: None,
            },
            ..Default::default()
        };
        let text = "one two three four five";
        let result = chunk_text(text, &config, None).unwrap();

        assert_eq!(result.chunks.len(), 1);
        assert_eq!(
            result.chunks[0].metadata.token_count,
            Some(5),
            "token_count must be populated from the registered tokenizer backend, not left None"
        );
    }

    /// `token_count` has no meaningful value for character-based sizing (the default);
    /// it must stay `None` rather than being populated with a bogus count.
    #[test]
    fn test_token_count_stays_none_for_character_sizing() {
        let config = ChunkingConfig::default();
        let result = chunk_text("hello world", &config, None).unwrap();
        assert_eq!(result.chunks.len(), 1);
        assert_eq!(result.chunks[0].metadata.token_count, None);
    }

    /// Regression test for #211: heading-context-aware classification rules (e.g.
    /// `is_schedule`, which inspects the *heading text*, not just the chunk body) are
    /// dead at `build_chunks` time because `heading_context` is always `None` there —
    /// it is only resolved afterward. After the fix, chunks must be reclassified using
    /// the now-known `heading_context`.
    #[test]
    fn test_chunk_reclassified_after_heading_context_resolved() {
        let heading = "# Schedule 1";
        // No schedule/annex/appendix/exhibit keyword in the body itself, and not
        // matching any other higher-priority rule (heading/code/table/formula/
        // definitions/signature/operative-clause/party-list), so this body classifies
        // as `Unknown` on its own — only the heading context can make it `Schedule`.
        let body = "This paragraph states general provisions applicable under the arrangement.";
        let markdown = format!("{heading}\n\n{body}");

        // Small enough that the heading and body land in separate chunks, so the body
        // chunk's own content never starts with '#' (which would otherwise trip the
        // higher-priority `is_heading` rule instead of exercising `is_schedule`).
        let config = ChunkingConfig {
            max_characters: body.len(),
            overlap: 0,
            trim: true,
            chunker_type: ChunkerType::Markdown,
            ..Default::default()
        };
        let result = chunk_text(&markdown, &config, None).unwrap();

        let body_chunk = result
            .chunks
            .iter()
            .find(|c| c.content.trim() == body)
            .unwrap_or_else(|| {
                panic!(
                    "expected a chunk containing exactly the body text, got: {:?}",
                    result.chunks
                )
            });

        assert!(
            body_chunk.metadata.heading_context.is_some(),
            "body chunk must have heading_context resolved"
        );
        assert_eq!(
            body_chunk.chunk_type,
            crate::types::ChunkType::Schedule,
            "chunk under a 'Schedule 1' heading must reclassify as Schedule once heading_context is known, got {:?}",
            body_chunk.chunk_type
        );
    }
}
