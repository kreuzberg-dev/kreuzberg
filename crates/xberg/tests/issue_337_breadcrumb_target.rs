//! Regression tests for issue #337, superseded by the #1393 revised design.
//!
//! `ChunkingConfig::breadcrumb_target` originally controlled *where* the Markdown
//! heading-path breadcrumb was written when `prepend_heading_context` was enabled:
//! into chunk `content` (good for dense/embedding retrieval), or into
//! `ChunkMetadata::heading_path` only (good for lexical retrieval, since every
//! chunk under a heading otherwise repeats the same tokens and collapses that
//! heading word's IDF toward zero).
//!
//! GH#1393's follow-up discussion replaced that design: `chunk.content` is now
//! **always** the exact `[byte_start, byte_end)` source span, for both
//! `BreadcrumbTarget` variants and regardless of `prepend_heading_context`.
//! Prepending the breadcrumb is now a rendering step a consumer applies at index
//! time via [`render_heading_breadcrumb`], not a mutation the chunker performs.
//! Both config fields are deprecated and inert; this suite locks down that they
//! no longer change chunking output, and that the explicit renderer reproduces
//! the old inline form on demand.
#![cfg(feature = "chunking")]

use xberg::BreadcrumbTarget;
use xberg::chunking::{ChunkerType, ChunkingConfig, chunk_for_rag, render_heading_breadcrumb};

/// A single H1 heading (`# Setup`) at byte offset 0, followed by prose long enough
/// (with `max_characters` below) to force multiple chunks. The heading text is the
/// document's only `#` character, so any chunk whose `byte_start` is greater than 0
/// is *guaranteed* not to start with a `#` — its raw content can never be mistaken
/// for a heading line by `strip_leading_heading`, making the breadcrumb-prepend
/// transformation on those chunks fully predictable without depending on exactly
/// where the underlying Markdown splitter chose to cut.
const MARKDOWN: &str = concat!(
    "# Setup\n\n",
    "First paragraph has enough distinct words to occupy a good portion of the very first chunk by itself here.\n\n",
    "Second paragraph continues on a totally different note with entirely separate content and phrasing here.\n\n",
    "Third paragraph wraps up the section with a final thought that is unrelated to the heading text above.",
);

/// `max_characters` deliberately smaller than `MARKDOWN.len()`: a chunker's content
/// contract guarantees no single chunk exceeds this cap, so an input longer than the
/// cap is guaranteed to produce more than one chunk — regardless of the splitter's
/// internal boundary choices.
const MAX_CHARACTERS: usize = 60;

fn config(breadcrumb_target: BreadcrumbTarget) -> ChunkingConfig {
    ChunkingConfig {
        max_characters: MAX_CHARACTERS,
        overlap: 0,
        trim: true,
        chunker_type: ChunkerType::Markdown,
        prepend_heading_context: true,
        breadcrumb_target,
        ..Default::default()
    }
}

#[test]
fn default_breadcrumb_target_is_still_content_even_though_it_is_now_inert() {
    assert_eq!(
        ChunkingConfig::default().breadcrumb_target,
        BreadcrumbTarget::Content,
        "the default variant is unchanged for wire compatibility (#337); it no longer prepends \
         anything since #1393 made the whole enum inert, which is what the other tests here pin"
    );
}

#[test]
fn default_config_matches_explicit_content_target() {
    let default_config = ChunkingConfig {
        max_characters: MAX_CHARACTERS,
        overlap: 0,
        trim: true,
        chunker_type: ChunkerType::Markdown,
        prepend_heading_context: true,
        // `breadcrumb_target` intentionally left unset — exercises the default.
        ..Default::default()
    };
    let explicit_content_config = config(BreadcrumbTarget::Content);

    let default_result = chunk_for_rag(MARKDOWN, &default_config).unwrap();
    let explicit_result = chunk_for_rag(MARKDOWN, &explicit_content_config).unwrap();

    assert_eq!(default_result.chunks.len(), explicit_result.chunks.len());
    for (default_chunk, explicit_chunk) in default_result.chunks.iter().zip(explicit_result.chunks.iter()) {
        assert_eq!(
            default_chunk.content, explicit_chunk.content,
            "an unset breadcrumb_target must produce byte-identical content to an explicit Content target"
        );
    }
}

#[test]
fn metadata_target_never_mutates_content() {
    let metadata_result = chunk_for_rag(MARKDOWN, &config(BreadcrumbTarget::Metadata)).unwrap();

    let mut prepend_disabled_config = config(BreadcrumbTarget::Content);
    prepend_disabled_config.prepend_heading_context = false;
    let prepend_disabled_result = chunk_for_rag(MARKDOWN, &prepend_disabled_config).unwrap();

    assert_eq!(metadata_result.chunks.len(), prepend_disabled_result.chunks.len());
    for (metadata_chunk, disabled_chunk) in metadata_result.chunks.iter().zip(prepend_disabled_result.chunks.iter()) {
        assert_eq!(
            metadata_chunk.content, disabled_chunk.content,
            "Metadata target must leave content exactly as if prepend_heading_context were disabled"
        );
    }
}

#[test]
fn metadata_target_still_populates_heading_path() {
    let result = chunk_for_rag(MARKDOWN, &config(BreadcrumbTarget::Metadata)).unwrap();
    assert!(!result.chunks.is_empty());
    for chunk in &result.chunks {
        assert_eq!(
            chunk.metadata.heading_path,
            vec!["Setup".to_string()],
            "heading_path must be populated regardless of breadcrumb_target, content: {:?}",
            chunk.content
        );
        assert!(
            !chunk.content.contains(" > "),
            "Metadata target must never write the breadcrumb separator into content, got: {:?}",
            chunk.content
        );
    }
}

/// Supersedes the pre-#1393 `content_target_prepends_breadcrumb_for_chunks_starting_after_the_heading_line`
/// test, which pinned `Content` mode mutating `chunk.content` in place. That mutation is
/// gone: `Content` mode now behaves identically to `Metadata` mode, `content` is always the
/// exact `[byte_start, byte_end)` source span (closing the #1294 offset-desync class of bug),
/// and a caller who wants the old inline breadcrumb form calls `render_heading_breadcrumb`
/// explicitly at index time.
#[test]
fn content_target_no_longer_mutates_content_render_heading_breadcrumb_does_it_explicitly() {
    let content_result = chunk_for_rag(MARKDOWN, &config(BreadcrumbTarget::Content)).unwrap();
    assert!(
        content_result.chunks.len() >= 2,
        "MAX_CHARACTERS must be small enough relative to MARKDOWN to force multiple chunks; got {} chunk(s)",
        content_result.chunks.len()
    );

    let mut checked_a_later_chunk = false;
    for chunk in &content_result.chunks {
        assert_eq!(
            chunk.content,
            &MARKDOWN[chunk.metadata.byte_start..chunk.metadata.byte_end],
            "content must be byte-identical to its own [byte_start, byte_end) source span, got: {:?}",
            chunk.content
        );

        if chunk.metadata.byte_start == 0 {
            // The first chunk starts with the literal "# Setup" heading line, whose
            // exact transformed shape depends on the heading-stripping logic this
            // test intentionally does not re-derive. Every later chunk below already
            // proves content is untouched and the explicit renderer works.
            continue;
        }

        assert_eq!(
            chunk.metadata.heading_path,
            vec!["Setup".to_string()],
            "every chunk must resolve heading_context from the single leading heading"
        );
        assert!(
            !chunk.content.starts_with('#'),
            "a chunk starting after byte 0 cannot contain the document's only '#', got: {:?}",
            chunk.content
        );

        let heading_context = chunk
            .metadata
            .heading_context
            .as_ref()
            .expect("a chunk with a non-empty heading_path must carry heading_context");
        let rendered = render_heading_breadcrumb(&chunk.content, heading_context);
        let expected = format!("# Setup\n\n{}", chunk.content);
        assert_eq!(
            rendered, expected,
            "render_heading_breadcrumb must reproduce the old Content-mode inline form on demand"
        );
        checked_a_later_chunk = true;
    }
    assert!(
        checked_a_later_chunk,
        "test fixture must produce at least one chunk starting after the heading line"
    );
}

/// `heading_path` is populated under BOTH targets — that is the whole reason there is
/// no `Both` variant. `Metadata` does not withhold the breadcrumb, it only keeps it out
/// of `content`, so a lexical index can be built from `content` directly while a dense
/// one still has the section context available from metadata.
#[test]
fn heading_path_is_populated_under_every_target() {
    let content_result = chunk_for_rag(MARKDOWN, &config(BreadcrumbTarget::Content)).unwrap();
    let metadata_result = chunk_for_rag(MARKDOWN, &config(BreadcrumbTarget::Metadata)).unwrap();

    assert_eq!(content_result.chunks.len(), metadata_result.chunks.len());

    let mut checked_a_chunk_under_a_heading = false;
    for (content_chunk, metadata_chunk) in content_result.chunks.iter().zip(metadata_result.chunks.iter()) {
        assert_eq!(
            metadata_chunk.metadata.heading_path, content_chunk.metadata.heading_path,
            "heading_path must be identical under both targets; only `content` differs"
        );
        if !content_chunk.metadata.heading_path.is_empty() {
            checked_a_chunk_under_a_heading = true;
        }
    }
    assert!(
        checked_a_chunk_under_a_heading,
        "test fixture must produce at least one chunk carrying a heading_path"
    );
}

#[test]
fn breadcrumb_target_is_inert_when_prepend_heading_context_disabled() {
    let mut baseline_config = config(BreadcrumbTarget::Content);
    baseline_config.prepend_heading_context = false;
    let baseline_result = chunk_for_rag(MARKDOWN, &baseline_config).unwrap();

    for target in [BreadcrumbTarget::Content, BreadcrumbTarget::Metadata] {
        let mut disabled_config = baseline_config.clone();
        disabled_config.breadcrumb_target = target;
        let result = chunk_for_rag(MARKDOWN, &disabled_config).unwrap();

        assert_eq!(result.chunks.len(), baseline_result.chunks.len());
        for (baseline_chunk, chunk) in baseline_result.chunks.iter().zip(result.chunks.iter()) {
            assert_eq!(
                chunk.content, baseline_chunk.content,
                "breadcrumb_target ({target:?}) must be inert while prepend_heading_context is false"
            );
        }
    }
}

#[test]
fn breadcrumb_target_serde_round_trips_snake_case() {
    for (value, json) in [
        (BreadcrumbTarget::Content, "\"content\""),
        (BreadcrumbTarget::Metadata, "\"metadata\""),
    ] {
        let serialized = serde_json::to_string(&value).unwrap();
        assert_eq!(serialized, json);
        let deserialized: BreadcrumbTarget = serde_json::from_str(json).unwrap();
        assert_eq!(deserialized, value);
    }
}
