//! Text chunking utilities.
//!
//! This module provides text chunking functionality using the `text-splitter` library.
//! It splits long text into smaller chunks while preserving semantic boundaries.
//!
//! # Features
//!
//! - **Smart splitting**: Respects word and sentence boundaries
//! - **Markdown-aware**: Preserves Markdown structure (headings, code blocks, lists)
//! - **Configurable overlap**: Overlap chunks to maintain context
//! - **Unicode support**: Handles CJK characters and emojis correctly
//! - **Batch processing**: Process multiple texts efficiently
//!
//! # Chunker Types
//!
//! - **Text**: Generic text splitter, splits on whitespace and punctuation
//! - **Markdown**: Markdown-aware splitter, preserves formatting and structure
//! - **Yaml**: YAML-aware splitter, creates one chunk per top-level key
//! - **Semantic**: Topic-aware chunker that groups paragraphs by semantic similarity
//!
//! # Example
//!
//! ```rust
//! use xberg::chunking::{chunk_text, ChunkingConfig, ChunkerType};
//!
//! # fn example() -> xberg::Result<()> {
//! let config = ChunkingConfig {
//!     max_characters: 500,
//!     overlap: 50,
//!     trim: true,
//!     chunker_type: ChunkerType::Text,
//!     ..Default::default()
//! };
//!
//! let long_text = "This is a very long document...".repeat(100);
//! let result = chunk_text(&long_text, &config, None)?;
//!
//! println!("Split into {} chunks", result.chunk_count);
//! for (i, chunk) in result.chunks.iter().enumerate() {
//!     println!("Chunk {}: {} chars", i + 1, chunk.content.len());
//! }
//! # Ok(())
//! # }
//! ```
//!
//! # Use Cases
//!
//! - Splitting documents for LLM context windows
//! - Creating overlapping chunks for semantic search
//! - Processing large documents in batches
//! - Maintaining context across chunk boundaries

pub mod boundaries;
pub mod boundary_detection;
mod builder;
pub mod classifier;
pub mod config;
pub mod core;
mod headings;
pub(crate) mod page_spans;
pub mod processor;
pub mod rag;
pub mod semantic;
mod text_splitter;
#[cfg(feature = "chunking-tokenizers")]
mod tokenizer_cache;
pub mod validation;
pub(crate) mod vectors;
mod yaml_section;

pub(crate) use builder::heading_path_from_context;
pub use config::{ChunkSizing, ChunkerType, ChunkingConfig, ChunkingResult};
pub use core::{chunk_text, render_heading_breadcrumb};
// Only used by `core::pipeline::features::try_code_chunks`, which is gated on
// `tree-sitter` in addition to `chunking` — unused (and so `allow`ed) under other
// feature combinations that enable `chunking` alone.
#[allow(unused_imports)]
pub(crate) use builder::resolve_token_counter;
pub(crate) use core::chunk_text_with_heading_source;
pub use processor::ChunkingProcessor;
pub use rag::chunk_for_rag;
#[cfg(feature = "chunking-tokenizers")]
pub use tokenizer_cache::{
    DEFAULT_COUNT_TOKENS_MODEL, TokenizerSource, count_tokens, preload_tokenizer, try_count_tokens,
};
