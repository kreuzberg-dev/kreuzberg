//! Attach sparse (SPLADE) and late-interaction (ColBERT) vectors to chunks.
//!
//! Mirrors the dense-embedding attachment pattern in
//! [`crate::embeddings::generate_embeddings_for_chunks`]: given one precomputed vector
//! per chunk, in the same order as the chunks, zip them onto
//! [`crate::types::Chunk::sparse_embedding`] / [`crate::types::Chunk::late_interaction`].
//!
//! The `apply_*` functions below are pure zip/assign — they always compile and carry no
//! ONNX dependency, so they are unit-tested directly with hand-built vectors. The
//! `generate_*_for_chunks` functions additionally call the ONNX-backed
//! [`crate::sparse_embeddings::embed_sparse`] / [`crate::late_interaction::embed_multi_vector`]
//! engines and are gated behind the same Cargo features those functions require.
//!
//! # Pipeline wiring
//!
//! [`crate::core::config::ChunkingConfig`] (defined in
//! `crates/xberg/src/core/config/processing.rs`) carries `sparse_embedding:
//! Option<SparseEmbeddingConfig>` and `late_interaction: Option<LateInteractionConfig>`
//! fields alongside the existing `embedding: Option<EmbeddingConfig>`.
//! `crate::core::pipeline::features::execute_chunking` calls
//! [`generate_sparse_vectors_for_chunks`] / [`generate_late_interaction_vectors_for_chunks`]
//! the same way it calls `crate::embeddings::generate_embeddings_for_chunks` — guarded by
//! an `if let Some(ref config) = chunking_config.sparse_embedding` (respectively
//! `late_interaction`) check, with failures pushed onto `result.processing_warnings`
//! rather than aborting extraction.

use crate::error::{Result, XbergError};
use crate::types::Chunk;

/// Assign one precomputed [`crate::SparseEmbedding`] to each chunk, in order.
///
/// # Errors
///
/// Returns [`XbergError::Validation`] if `vectors.len() != chunks.len()` — a positional
/// zip would otherwise misalign vectors with the wrong chunks. On error, `chunks` is left
/// untouched.
///
/// `#[allow(dead_code)]`: always compiled (unlike [`generate_sparse_vectors_for_chunks`]),
/// but its only non-test caller is gated behind `sparse-embeddings`, so a build without
/// that feature (and without `--tests`) would otherwise flag this as dead code.
#[allow(dead_code)]
pub(crate) fn apply_sparse_vectors(chunks: &mut [Chunk], vectors: Vec<crate::SparseEmbedding>) -> Result<()> {
    if chunks.len() != vectors.len() {
        return Err(XbergError::Validation {
            message: format!(
                "Sparse-embedding generation returned {got} vectors for {expected} chunks; refusing to \
                 attach vectors because a positional zip would misalign them with the wrong chunks",
                got = vectors.len(),
                expected = chunks.len(),
            ),
            source: None,
        });
    }

    for (chunk, vector) in chunks.iter_mut().zip(vectors) {
        chunk.sparse_embedding = Some(vector);
    }

    Ok(())
}

/// Generate sparse (SPLADE) embeddings for `chunks`' content and attach them via
/// [`apply_sparse_vectors`].
///
/// Mirrors [`crate::embeddings::generate_embeddings_for_chunks`]. Called from
/// `crate::core::pipeline::features::execute_chunking` when
/// `ChunkingConfig::sparse_embedding` is set — see the module docs.
///
/// # Errors
///
/// Returns an error if sparse-embedding generation fails (model load, ONNX Runtime
/// unavailable, etc.) or if the returned vector count does not match `chunks.len()`.
#[cfg(feature = "sparse-embeddings")]
pub(crate) fn generate_sparse_vectors_for_chunks(
    chunks: &mut [Chunk],
    config: &crate::core::config::SparseEmbeddingConfig,
) -> Result<()> {
    if chunks.is_empty() {
        return Ok(());
    }

    let texts: Vec<&str> = chunks.iter().map(|c| c.content.as_str()).collect();
    let vectors = crate::sparse_embeddings::embed_sparse(&texts, config)?;
    apply_sparse_vectors(chunks, vectors)
}

/// Assign one precomputed [`crate::MultiVectorEmbedding`] to each chunk, in order.
///
/// # Errors
///
/// Returns [`XbergError::Validation`] if `vectors.len() != chunks.len()`. On error,
/// `chunks` is left untouched.
///
/// `#[allow(dead_code)]`: always compiled (unlike
/// [`generate_late_interaction_vectors_for_chunks`]), but its only non-test caller is
/// gated behind `late-interaction`, so a build without that feature (and without
/// `--tests`) would otherwise flag this as dead code.
#[allow(dead_code)]
pub(crate) fn apply_late_interaction_vectors(
    chunks: &mut [Chunk],
    vectors: Vec<crate::MultiVectorEmbedding>,
) -> Result<()> {
    if chunks.len() != vectors.len() {
        return Err(XbergError::Validation {
            message: format!(
                "Late-interaction embedding generation returned {got} vectors for {expected} chunks; \
                 refusing to attach vectors because a positional zip would misalign them with the wrong \
                 chunks",
                got = vectors.len(),
                expected = chunks.len(),
            ),
            source: None,
        });
    }

    for (chunk, vector) in chunks.iter_mut().zip(vectors) {
        chunk.late_interaction = Some(vector);
    }

    Ok(())
}

/// Generate ColBERT late-interaction embeddings for `chunks`' content (as documents,
/// i.e. `is_query = false`) and attach them via [`apply_late_interaction_vectors`].
///
/// See [`generate_sparse_vectors_for_chunks`] and the module docs for the pipeline
/// call-site wiring.
///
/// # Errors
///
/// Returns an error if late-interaction generation fails (model load, ONNX Runtime
/// unavailable, etc.) or if the returned vector count does not match `chunks.len()`.
#[cfg(feature = "late-interaction")]
pub(crate) fn generate_late_interaction_vectors_for_chunks(
    chunks: &mut [Chunk],
    config: &crate::core::config::LateInteractionConfig,
) -> Result<()> {
    if chunks.is_empty() {
        return Ok(());
    }

    let texts: Vec<&str> = chunks.iter().map(|c| c.content.as_str()).collect();
    let vectors = crate::late_interaction::embed_multi_vector(&texts, config, false)?;
    apply_late_interaction_vectors(chunks, vectors)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ChunkMetadata, ChunkType};

    fn test_chunk(content: &str) -> Chunk {
        Chunk {
            content: content.to_string(),
            chunk_type: ChunkType::default(),
            embedding: None,
            sparse_embedding: None,
            late_interaction: None,
            metadata: ChunkMetadata {
                byte_start: 0,
                byte_end: content.len(),
                token_count: None,
                chunk_index: 0,
                total_chunks: 1,
                first_page: None,
                last_page: None,
                heading_context: None,
                heading_path: Vec::new(),
                image_indices: Vec::new(),
                node_ids: Vec::new(),
                page_spans: Vec::new(),
                classifications: Vec::new(),
            },
        }
    }

    #[test]
    fn should_attach_exact_sparse_vector_to_each_chunk_when_applied() {
        let mut chunks = vec![test_chunk("alpha"), test_chunk("beta")];
        let vectors = vec![
            crate::SparseEmbedding {
                indices: vec![1, 4],
                values: vec![0.9, 0.1],
            },
            crate::SparseEmbedding {
                indices: vec![2],
                values: vec![0.5],
            },
        ];

        apply_sparse_vectors(&mut chunks, vectors).expect("lengths match");

        assert_eq!(chunks[0].sparse_embedding.as_ref().unwrap().indices, vec![1, 4]);
        assert_eq!(chunks[0].sparse_embedding.as_ref().unwrap().values, vec![0.9, 0.1]);
        assert_eq!(chunks[1].sparse_embedding.as_ref().unwrap().indices, vec![2]);
        assert_eq!(chunks[1].sparse_embedding.as_ref().unwrap().values, vec![0.5]);
        assert!(
            chunks[0].late_interaction.is_none(),
            "late_interaction must stay untouched"
        );
    }

    #[test]
    fn should_leave_sparse_embedding_none_when_apply_is_never_called() {
        let chunks = [test_chunk("alpha"), test_chunk("beta")];
        assert!(chunks.iter().all(|c| c.sparse_embedding.is_none()));
    }

    #[test]
    fn should_error_and_leave_chunks_untouched_when_sparse_vector_count_mismatches() {
        let mut chunks = vec![test_chunk("alpha"), test_chunk("beta")];
        let vectors = vec![crate::SparseEmbedding {
            indices: vec![1],
            values: vec![1.0],
        }];

        let err = apply_sparse_vectors(&mut chunks, vectors).expect_err("length mismatch must error");
        assert!(err.to_string().contains("1 vectors for 2 chunks"), "got: {err}");
        assert!(
            chunks.iter().all(|c| c.sparse_embedding.is_none()),
            "chunks must be untouched on error"
        );
    }

    #[test]
    fn should_attach_exact_late_interaction_vector_to_each_chunk_when_applied() {
        let mut chunks = vec![test_chunk("alpha")];
        let vectors = vec![crate::MultiVectorEmbedding {
            num_tokens: 2,
            dim: 2,
            data: vec![0.1, 0.2, 0.3, 0.4],
        }];

        apply_late_interaction_vectors(&mut chunks, vectors).expect("lengths match");

        let late = chunks[0].late_interaction.as_ref().unwrap();
        assert_eq!(late.num_tokens, 2);
        assert_eq!(late.dim, 2);
        assert_eq!(late.data, vec![0.1, 0.2, 0.3, 0.4]);
        assert!(
            chunks[0].sparse_embedding.is_none(),
            "sparse_embedding must stay untouched"
        );
    }

    #[test]
    fn should_leave_late_interaction_none_when_apply_is_never_called() {
        let chunks = [test_chunk("alpha")];
        assert!(chunks.iter().all(|c| c.late_interaction.is_none()));
    }

    #[test]
    fn should_error_and_leave_chunks_untouched_when_late_interaction_vector_count_mismatches() {
        let mut chunks = vec![test_chunk("alpha"), test_chunk("beta")];
        let vectors = vec![];

        let err = apply_late_interaction_vectors(&mut chunks, vectors).expect_err("length mismatch must error");
        assert!(err.to_string().contains("0 vectors for 2 chunks"), "got: {err}");
        assert!(
            chunks.iter().all(|c| c.late_interaction.is_none()),
            "chunks must be untouched on error"
        );
    }
}
