#![allow(clippy::print_stdout, clippy::print_stderr, clippy::dbg_macro)] // ~keep: test/bench binaries print by design; org logging policy exempts tests
//! Wiring tests for #268: sparse (SPLADE) and late-interaction (ColBERT) vectors attached
//! to chunks via `ChunkingConfig::sparse_embedding` / `ChunkingConfig::late_interaction`.
//!
//! `sparse_and_late_interaction_configs_unset_leave_chunks_without_vectors` is a fast,
//! network-free regression guard: it never exercises the ONNX-backed producers, so it
//! must pass regardless of which optional embedding features are compiled in.
//!
//! The `*_matches_direct_producer_call` tests are live: they download real ONNX models
//! from Hugging Face and cross-check that the vector the extraction pipeline attaches to
//! a chunk is bit-for-bit identical to calling the standalone `xberg::embed_sparse` /
//! `xberg::embed_multi_vector` producer directly on the same chunk content. Opt out on
//! offline dev with `XBERG_SKIP_LIVE_HF=1`.

mod helpers;

#[cfg(feature = "chunking")]
mod vectors_unset {
    use crate::helpers::extract_bytes_document;
    use xberg::core::config::{ChunkingConfig, ExtractionConfig};

    /// No behaviour change for existing callers (#268): leaving both new config fields
    /// unset must leave every chunk's `sparse_embedding`/`late_interaction` as `None`.
    #[tokio::test]
    async fn sparse_and_late_interaction_configs_unset_leave_chunks_without_vectors() {
        let config = ExtractionConfig {
            chunking: Some(ChunkingConfig {
                max_characters: 50,
                overlap: 10,
                ..Default::default()
            }),
            ..Default::default()
        };

        let text = "This is a long text that should be split into multiple chunks. ".repeat(5);

        let result = extract_bytes_document(text.as_bytes(), "text/plain", &config)
            .await
            .expect("extraction should succeed");
        let chunks = result
            .chunks
            .expect("chunks should be produced when chunking is configured");
        assert!(!chunks.is_empty(), "chunking must produce at least one chunk");

        for (i, chunk) in chunks.iter().enumerate() {
            assert!(
                chunk.sparse_embedding.is_none(),
                "chunk[{i}] must have no sparse_embedding when sparse_embedding config is not set"
            );
            assert!(
                chunk.late_interaction.is_none(),
                "chunk[{i}] must have no late_interaction when late_interaction config is not set"
            );
        }
    }
}

// Gated to the union of its two callers below. Without this, a narrow-feature leg that
// enables neither `sparse-embeddings` nor `late-interaction` (CI's `--no-default-features
// --features "ocr,auto-rotate-tract"` clippy run) sees an uncalled function and fails on
// `-D warnings`. ~keep
#[cfg(all(
    feature = "chunking",
    any(feature = "sparse-embeddings", feature = "late-interaction")
))]
fn should_skip_live() -> bool {
    std::env::var("XBERG_SKIP_LIVE_HF").is_ok()
}

#[cfg(all(feature = "chunking", feature = "sparse-embeddings"))]
mod sparse_wiring_live {
    use super::should_skip_live;
    use crate::helpers::extract_bytes_document;
    use xberg::core::config::{ChunkingConfig, ExtractionConfig, SparseEmbeddingConfig};

    /// The sparse vector attached to a chunk by the pipeline must be identical to calling
    /// the standalone producer directly on the same chunk content (#268).
    #[tokio::test]
    async fn chunk_sparse_embedding_matches_direct_producer_call() {
        if should_skip_live() {
            eprintln!("XBERG_SKIP_LIVE_HF=1, skipping");
            return;
        }

        let text = b"Hello world. This single short document must fit in one chunk so the \
                     pipeline's per-chunk sparse vector can be compared directly against a \
                     standalone producer call on identical content (#268).";

        let config = ExtractionConfig {
            chunking: Some(ChunkingConfig {
                max_characters: 10_000,
                overlap: 0,
                sparse_embedding: Some(SparseEmbeddingConfig::default()),
                ..Default::default()
            }),
            ..Default::default()
        };

        let result = extract_bytes_document(text, "text/plain", &config)
            .await
            .expect("extraction should succeed");
        let chunks = result.chunks.expect("chunks should be produced");
        assert_eq!(
            chunks.len(),
            1,
            "text must fit in a single chunk for a direct comparison"
        );

        let sparse = chunks[0]
            .sparse_embedding
            .as_ref()
            .expect("sparse_embedding must be attached when ChunkingConfig::sparse_embedding is set");

        let direct = xberg::embed_sparse(vec![chunks[0].content.clone()], &SparseEmbeddingConfig::default())
            .expect("direct sparse embed must succeed");
        let expected = &direct[0];

        assert_eq!(
            sparse.indices, expected.indices,
            "pipeline-attached indices must match the producer's own output"
        );
        assert_eq!(
            sparse.values, expected.values,
            "pipeline-attached values must match the producer's own output"
        );
    }
}

#[cfg(all(feature = "chunking", feature = "late-interaction"))]
mod late_interaction_wiring_live {
    use super::should_skip_live;
    use crate::helpers::extract_bytes_document;
    use xberg::core::config::{ChunkingConfig, ExtractionConfig, LateInteractionConfig};

    /// The late-interaction vector attached to a chunk by the pipeline must be identical
    /// to calling the standalone producer directly on the same chunk content (#268).
    #[tokio::test]
    async fn chunk_late_interaction_matches_direct_producer_call() {
        if should_skip_live() {
            eprintln!("XBERG_SKIP_LIVE_HF=1, skipping");
            return;
        }

        let text = b"Hello world. This single short document must fit in one chunk so the \
                     pipeline's per-chunk late-interaction vector can be compared directly \
                     against a standalone producer call on identical content (#268).";

        let config = ExtractionConfig {
            chunking: Some(ChunkingConfig {
                max_characters: 10_000,
                overlap: 0,
                late_interaction: Some(LateInteractionConfig::default()),
                ..Default::default()
            }),
            ..Default::default()
        };

        let result = extract_bytes_document(text, "text/plain", &config)
            .await
            .expect("extraction should succeed");
        let chunks = result.chunks.expect("chunks should be produced");
        assert_eq!(
            chunks.len(),
            1,
            "text must fit in a single chunk for a direct comparison"
        );

        let late = chunks[0]
            .late_interaction
            .as_ref()
            .expect("late_interaction must be attached when ChunkingConfig::late_interaction is set");

        let direct = xberg::embed_multi_vector(
            vec![chunks[0].content.clone()],
            &LateInteractionConfig::default(),
            false,
        )
        .expect("direct late-interaction embed must succeed");
        let expected = &direct[0];

        assert_eq!(
            late.num_tokens, expected.num_tokens,
            "pipeline-attached num_tokens must match the producer's own output"
        );
        assert_eq!(
            late.dim, expected.dim,
            "pipeline-attached dim must match the producer's own output"
        );
        assert_eq!(
            late.data, expected.data,
            "pipeline-attached data must match the producer's own output"
        );
    }
}
