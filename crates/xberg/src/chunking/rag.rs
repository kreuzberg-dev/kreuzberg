//! RAG-shaped chunking composer.
//!
//! Provides [`chunk_for_rag`], a thin composer that delegates to the existing
//! chunking pipeline and then derives the flat
//! [`heading_path`](crate::types::ChunkMetadata::heading_path) breadcrumb from
//! whatever `heading_context` the underlying chunker emits.
//!
//! # `heading_path` population by chunker type
//!
//! | Chunker              | `heading_context` set? | `heading_path` populated? |
//! |----------------------|------------------------|---------------------------|
//! | `Text` (auto-upgraded) | yes (→ Markdown)     | yes                       |
//! | `Markdown`           | yes                    | yes                       |
//! | `Semantic`           | yes (uses Markdown internally) | yes             |
//! | `Yaml`               | **no**                 | **always empty**          |
//!
//! The `Yaml` chunker splits on top-level YAML keys and does not have a concept
//! of heading hierarchy, so `heading_path` will always be `[]` for every chunk
//! produced by a `Yaml`-typed config.
//!
//! # Breadcrumb placement: render at index time, don't mutate (#1393)
//!
//! `chunk.content` is **always** exactly the `[byte_start, byte_end)` span of the
//! source document — the chunker never prepends a heading breadcrumb into it, and
//! the deprecated `prepend_heading_context`/`breadcrumb_target` config fields have
//! no effect on `content` any more (kept only so existing callers keep compiling).
//! `heading_context`/`heading_path` (which `chunk_for_rag` always populates) are
//! the source of truth for the breadcrumb; rendering it into a chunk's text — e.g.
//! `"# Guide > ## Setup\n\n"` prepended ahead of the body — is a step a *consumer*
//! applies at index time via [`render_heading_breadcrumb`], not something the
//! chunker decides once for every downstream retrieval arm.
//!
//! This matters because three retrieval consumers want three different views of
//! the same chunk, and a single config flag on the chunker forces one answer for
//! all three:
//!
//! * **Dense/embedding retrieval** benefits from the breadcrumb inline — a
//!   self-contained passage that carries its own structural context embeds
//!   better. Call [`render_heading_breadcrumb`] on the chunk's `content` and
//!   `heading_context` before embedding.
//! * **Lexical retrieval (BM25/TF-IDF)** is actively harmed by it: every chunk
//!   under the same section would repeat the same heading tokens, so a heading
//!   word's document frequency approaches the number of chunks in that section
//!   and its IDF collapses toward zero — the heading text can dominate, or drown
//!   out, real match signal. Intra-section queries, the common case, lose exactly
//!   the discrimination that made the term useful in the first place. Index
//!   `chunk.content` as-is (or use `heading_path` for a separate, down-weighted
//!   field) — no stripping required, because it was never prepended.
//! * **Sparse learned retrieval (SPLADE) is worse off than BM25, not merely a
//!   softer version of the same problem.** BM25's IDF is computed over the
//!   caller's own collection, so a repeated heading term is at least visible in
//!   the statistics: the damage is bounded to the literal breadcrumb tokens, and
//!   it partly self-corrects. SPLADE's term weights instead come from an encoder
//!   trained on a *general* corpus; it has no way to learn that a term is
//!   uninformative in *this* collection, because it never sees collection
//!   statistics at inference time. Worse, SPLADE's term *expansion* means a
//!   prepended heading does not just add its own literal tokens — it pulls in the
//!   heading's whole learned neighbourhood (e.g. `"Authentication"` expands toward
//!   `auth`, `login`, `credential`, `oauth`, …) and injects that neighbourhood into
//!   every chunk in the section. Discrimination degrades across a whole semantic
//!   region rather than one token, and **re-indexing cannot fix it** — the
//!   expansion is a property of the pretrained encoder, not of the collection.
//!   Never feed it the breadcrumb; index `chunk.content` as-is.
//!
//! Because `content` stays clean by construction, BM25 and SPLADE need no special
//! handling at all — they simply index the chunk as returned. Only the dense arm
//! needs an extra step, and it is explicit:
//! [`render_heading_breadcrumb`](crate::chunking::render_heading_breadcrumb) takes
//! a chunk's `content` and `heading_context` and returns the breadcrumb-prefixed
//! text for that one retrieval arm, without a second chunking pass, a chunker-level
//! flag, or hand-rolling the format string.
//!
//! # Design
//!
//! - Delegates all splitting to [`super::core::chunk_text`] with
//!   `ChunkerType::Markdown` (sensible for most document types) unless the caller
//!   supplies a config that already selects a different chunker, in which case the
//!   caller's config is honoured and `heading_path` is derived post-hoc from
//!   whatever `heading_context` the underlying chunker emits.
//! - Does **not** reimplement any splitting logic.
//! - Does **not** add new fields to `ChunkMetadata`; it only populates existing
//!   `heading_path` entries.
//!
//! # Example
//!
//! ```rust,no_run
//! use xberg::chunking::{chunk_for_rag, ChunkingConfig, ChunkerType};
//!
//! # fn example() -> xberg::Result<()> {
//! let markdown = "# Introduction\n\nWelcome.\n\n## Details\n\nMore text here.";
//! let config = ChunkingConfig {
//!     max_characters: 512,
//!     overlap: 50,
//!     chunker_type: ChunkerType::Markdown,
//!     ..Default::default()
//! };
//! let result = chunk_for_rag(markdown, &config)?;
//! for chunk in &result.chunks {
//!     println!("{:?} -> {:?}", chunk.metadata.heading_path, chunk.content);
//! }
//! # Ok(())
//! # }
//! ```

use super::builder::heading_path_from_context;
use super::config::{ChunkerType, ChunkingConfig, ChunkingResult};
use super::core::chunk_text;
use crate::error::Result;

/// Chunk text for RAG retrieval, ensuring every chunk carries a `heading_path`.
///
/// Delegates to [`chunk_text`] using the caller's config (defaulting to
/// `ChunkerType::Markdown` when the config uses the default `Text` type, so that
/// heading hierarchy is resolved).  After chunking, derives
/// [`ChunkMetadata::heading_path`](crate::types::ChunkMetadata::heading_path) from each chunk's `heading_context`.
///
/// # Arguments
///
/// * `text` — Text to chunk. Markdown formatting enables heading-aware splitting.
/// * `config` — Chunking configuration.  The `chunker_type` field controls the
///   underlying splitter; use `ChunkerType::Markdown` for documents with ATX
///   headings.
///
/// # Returns
///
/// A [`ChunkingResult`] where every chunk's `heading_path` is populated from its
/// `heading_context` (empty when the chunk is not under any heading).
///
/// # Errors
///
/// Propagates any error from the underlying chunker (e.g. invalid overlap).
pub fn chunk_for_rag(text: &str, config: &ChunkingConfig) -> Result<ChunkingResult> {
    let effective_config;
    let config = if config.chunker_type == ChunkerType::Text {
        effective_config = ChunkingConfig {
            chunker_type: ChunkerType::Markdown,
            ..config.clone()
        };
        &effective_config
    } else {
        config
    };

    let mut result = chunk_text(text, config, None)?;

    for chunk in &mut result.chunks {
        if chunk.metadata.heading_path.is_empty() {
            chunk.metadata.heading_path = heading_path_from_context(&chunk.metadata.heading_context);
        }
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_rag_config() -> ChunkingConfig {
        ChunkingConfig {
            max_characters: 512,
            overlap: 0,
            trim: true,
            chunker_type: ChunkerType::Markdown,
            ..Default::default()
        }
    }

    #[test]
    fn chunk_for_rag_empty_input_returns_no_chunks() {
        let result = chunk_for_rag("", &default_rag_config()).unwrap();
        assert_eq!(result.chunks.len(), 0);
        assert_eq!(result.chunk_count, 0);
    }

    #[test]
    fn chunk_for_rag_text_without_headings_heading_path_empty() {
        let text = "Just plain text without any headings whatsoever.";
        let result = chunk_for_rag(text, &default_rag_config()).unwrap();
        assert_eq!(result.chunks.len(), 1);
        assert!(
            result.chunks[0].metadata.heading_path.is_empty(),
            "no headings → heading_path must be empty"
        );
    }

    #[test]
    fn chunk_for_rag_populates_heading_path_from_context() {
        let text = "# Introduction\n\nWelcome to the guide.\n\n## Setup\n\nInstall the dependencies.\n\n### Prerequisites\n\nYou need Rust installed.";
        let config = ChunkingConfig {
            max_characters: 100,
            overlap: 0,
            trim: true,
            chunker_type: ChunkerType::Markdown,
            ..Default::default()
        };
        let result = chunk_for_rag(text, &config).unwrap();
        assert!(
            !result.chunks.is_empty(),
            "should produce chunks from multi-heading doc"
        );

        for chunk in &result.chunks {
            if chunk.metadata.heading_context.is_some() {
                assert!(
                    !chunk.metadata.heading_path.is_empty(),
                    "chunk under heading must have non-empty heading_path, content: {:?}",
                    chunk.content
                );
            }
        }
    }

    #[test]
    fn chunk_for_rag_heading_path_order_outermost_first() {
        let text = "# Root\n\nSome root content here.\n\n## Child\n\nChild section content here.";
        let config = ChunkingConfig {
            max_characters: 200,
            overlap: 0,
            trim: true,
            chunker_type: ChunkerType::Markdown,
            ..Default::default()
        };
        let result = chunk_for_rag(text, &config).unwrap();

        let deep_chunk = result.chunks.iter().find(|c| c.metadata.heading_path.len() >= 2);

        if let Some(chunk) = deep_chunk {
            assert_eq!(
                chunk.metadata.heading_path[0], "Root",
                "outermost heading (h1) must be first in path"
            );
            assert_eq!(
                chunk.metadata.heading_path[1], "Child",
                "inner heading (h2) must follow in path"
            );
        }
    }

    #[test]
    fn chunk_for_rag_heading_path_matches_context_texts() {
        let text = "# Alpha\n\nAlpha content.\n\n## Beta\n\nBeta content.";
        let config = ChunkingConfig {
            max_characters: 300,
            overlap: 0,
            trim: true,
            chunker_type: ChunkerType::Markdown,
            ..Default::default()
        };
        let result = chunk_for_rag(text, &config).unwrap();

        for chunk in &result.chunks {
            if let Some(ref ctx) = chunk.metadata.heading_context {
                let expected: Vec<String> = ctx.headings.iter().map(|h| h.text.clone()).collect();
                assert_eq!(
                    chunk.metadata.heading_path, expected,
                    "heading_path must equal heading_context.headings[].text in order"
                );
            }
        }
    }

    #[test]
    fn chunk_for_rag_defaults_text_type_to_markdown_chunker() {
        let text = "# Title\n\nSome content here to chunk.\n\n## Section\n\nMore content here.";
        let config = ChunkingConfig {
            max_characters: 200,
            overlap: 0,
            trim: true,
            chunker_type: ChunkerType::Text,
            ..Default::default()
        };
        let result = chunk_for_rag(text, &config).unwrap();
        let has_path = result.chunks.iter().any(|c| !c.metadata.heading_path.is_empty());
        assert!(
            has_path,
            "upgrading Text → Markdown must produce heading_path on at least one chunk"
        );
    }

    #[test]
    fn chunk_for_rag_non_empty_output_on_multi_heading_doc() {
        let text = concat!(
            "# Chapter 1\n\n",
            "This chapter covers the basics of the system. ",
            "There is quite a lot of content here to ensure splitting occurs.\n\n",
            "## Section 1.1\n\n",
            "The first section dives into details. ",
            "More sentences follow to fill up the chunk budget adequately.\n\n",
            "## Section 1.2\n\n",
            "The second section covers advanced topics. ",
            "Even more text to ensure we get multiple chunks from this document.\n\n",
            "# Chapter 2\n\n",
            "Chapter two starts fresh. ",
            "Its content is completely independent of chapter one.\n\n",
        );
        let config = ChunkingConfig {
            max_characters: 150,
            overlap: 0,
            trim: true,
            chunker_type: ChunkerType::Markdown,
            ..Default::default()
        };
        let result = chunk_for_rag(text, &config).unwrap();
        assert!(
            result.chunks.len() >= 2,
            "multi-heading document should produce multiple chunks"
        );
        assert_eq!(result.chunks.len(), result.chunk_count);

        for chunk in &result.chunks {
            assert!(!chunk.content.is_empty());
        }
    }

    #[test]
    fn chunk_for_rag_does_not_overwrite_existing_heading_path() {
        let text = "# A\n\nContent under A.\n\n## B\n\nContent under B.";
        let config = ChunkingConfig {
            max_characters: 300,
            overlap: 0,
            trim: true,
            chunker_type: ChunkerType::Markdown,
            ..Default::default()
        };
        let result = chunk_for_rag(text, &config).unwrap();
        assert!(!result.chunks.is_empty(), "expected at least one chunk");

        let has_path = result.chunks.iter().any(|c| !c.metadata.heading_path.is_empty());
        assert!(has_path, "heading_path must be populated for chunks under headings");

        for chunk in &result.chunks {
            if let Some(ref ctx) = chunk.metadata.heading_context {
                let expected: Vec<String> = ctx.headings.iter().map(|h| h.text.clone()).collect();
                assert_eq!(
                    chunk.metadata.heading_path, expected,
                    "heading_path must equal heading_context texts in order"
                );
            }
        }
    }

    #[test]
    fn chunk_for_rag_yaml_chunker_yields_empty_heading_path() {
        let yaml = "key1: value one\nkey2: value two\nkey3: value three\n";
        let config = ChunkingConfig {
            max_characters: 512,
            overlap: 0,
            trim: true,
            chunker_type: ChunkerType::Yaml,
            ..Default::default()
        };
        let result = chunk_for_rag(yaml, &config).unwrap();
        for chunk in &result.chunks {
            assert!(
                chunk.metadata.heading_path.is_empty(),
                "Yaml chunker must produce empty heading_path; got: {:?}",
                chunk.metadata.heading_path
            );
        }
    }

    #[test]
    fn chunk_for_rag_semantic_chunker_populates_heading_path() {
        let text = concat!(
            "# Introduction\n\n",
            "This section introduces the topic in enough detail ",
            "that the semantic chunker will not merge it away.\n\n",
            "## Background\n\n",
            "Background context follows here, with sufficient content ",
            "to form its own coherent semantic unit.\n\n",
        );
        let config = ChunkingConfig {
            max_characters: 300,
            overlap: 0,
            trim: true,
            chunker_type: ChunkerType::Semantic,
            ..Default::default()
        };
        let result = chunk_for_rag(text, &config).unwrap();
        assert!(
            !result.chunks.is_empty(),
            "semantic chunker should produce at least one chunk"
        );

        let has_path = result.chunks.iter().any(|c| !c.metadata.heading_path.is_empty());
        assert!(
            has_path,
            "Semantic chunker must populate heading_path on at least one chunk in a headed document"
        );
    }
}
