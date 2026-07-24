//! Provider-hosted embeddings via liter-llm.
//!
//! Generates text embeddings using cloud-hosted models (e.g., OpenAI
//! `text-embedding-3-small`, Cohere `embed-english-v3.0`) through the
//! liter-llm client.  This is an alternative to local ONNX-based embeddings
//! and is useful when a provider-hosted model is preferred or when ONNX
//! Runtime is not available.

// ~keep Module is already gated on `liter-llm` by `llm/mod.rs`. `tokio-runtime` and
// ~keep `not(wasm32)` match `embed_texts`'s Llm dispatch arm, this module's only
// ~keep caller: that arm drives `tokio::runtime::Handle`/`block_in_place`/
// ~keep `global_runtime` directly, and `liter-llm`'s own feature definition does not
// ~keep imply `tokio-runtime` (unlike `embeddings`, which always does) — so a
// ~keep `static-embeddings + liter-llm` build without `tokio-runtime` must not
// ~keep compile this module in, or it goes dead-code (the caller falls back to the
// ~keep MissingDependency arm instead). wasm32 has no LLM-hosted embedding transport
// ~keep wired up yet (see the wasm-llm notes at each call site).
#[cfg(all(
    feature = "tokio-runtime",
    any(feature = "embeddings", feature = "static-embeddings"),
    not(target_arch = "wasm32")
))]
use liter_llm::{EmbeddingFormat, EmbeddingInput, EmbeddingRequest, LlmClient};

#[cfg(all(
    feature = "tokio-runtime",
    any(feature = "embeddings", feature = "static-embeddings"),
    not(target_arch = "wasm32")
))]
use crate::core::config::LlmConfig;

/// Generate embeddings using a provider-hosted model via liter-llm.
///
/// Sends the input texts to a remote embedding model and returns one
/// embedding vector per input text, in the same order as the input.
///
/// # Arguments
///
/// * `texts` - Slice of strings to embed (must all be non-empty)
/// * `config` - LLM provider/model configuration
/// * `normalize` - Whether to L2-normalize the resulting vectors
///
/// # Returns
///
/// `Vec<Vec<f32>>` with one embedding per input text.
///
/// # Errors
///
/// - `XbergError::Embedding` if the API call fails or returns unexpected data
/// - `XbergError::MissingDependency` if the liter-llm client cannot be created
#[cfg(all(
    feature = "tokio-runtime",
    any(feature = "embeddings", feature = "static-embeddings"),
    not(target_arch = "wasm32")
))]
pub(crate) async fn embed_via_llm<T: AsRef<str>>(
    texts: &[T],
    config: &LlmConfig,
    normalize: bool,
) -> crate::Result<(Vec<Vec<f32>>, Option<crate::types::LlmUsage>)> {
    if texts.is_empty() {
        return Ok((Vec::new(), None));
    }

    let client = super::client::create_client(config)?;

    let input_strings: Vec<String> = texts.iter().map(|t| t.as_ref().to_string()).collect();
    let input = if input_strings.len() == 1 {
        EmbeddingInput::Single(input_strings.into_iter().next().expect("checked non-empty"))
    } else {
        EmbeddingInput::Multiple(input_strings)
    };

    let request = EmbeddingRequest {
        model: config.model.clone(),
        input,
        encoding_format: Some(EmbeddingFormat::Base64),
        dimensions: None,
        user: None,
    };

    let response = client.embed(request).await.map_err(|e| {
        crate::XbergError::embedding(format!("LLM embedding request failed (model={}): {e}", config.model))
    })?;

    let usage = super::usage::extract_usage_from_embedding(&response, "embeddings");

    let mut data = response.data;
    data.sort_by_key(|obj| obj.index);

    let sorted_indices: Vec<u32> = data.iter().map(|obj| obj.index).collect();
    validate_contiguous_indices(&sorted_indices, texts.len(), &config.model)?;

    let mut embeddings: Vec<Vec<f32>> = data.into_iter().map(|obj| obj.embedding).collect();

    if normalize {
        for embedding in &mut embeddings {
            normalize_l2(embedding);
        }
    }

    Ok((embeddings, usage))
}

/// Verify that a sorted list of provider-returned embedding indices exactly covers
/// `0..expected_len` with no gaps, duplicates, or out-of-range entries.
///
/// The liter-llm response is re-sorted by `index` and then mapped positionally
/// onto the input texts. A provider that omits an object or numbers the response
/// with a gap would otherwise shift every embedding after the gap by one
/// position, silently attaching the wrong vector to the wrong text. This check
/// makes that failure explicit instead of letting it through as `Ok`.
///
/// # Errors
///
/// - `XbergError::Embedding` if `sorted_indices.len() != expected_len`, or if any
///   `sorted_indices[i] != i` (indicating a gap, a duplicate, or an out-of-range index).
#[cfg(any(
    all(
        feature = "tokio-runtime",
        any(feature = "embeddings", feature = "static-embeddings"),
        not(target_arch = "wasm32")
    ),
    test
))]
fn validate_contiguous_indices(sorted_indices: &[u32], expected_len: usize, model: &str) -> crate::Result<()> {
    let is_contiguous = sorted_indices.len() == expected_len
        && sorted_indices
            .iter()
            .enumerate()
            .all(|(position, &index)| position as u64 == u64::from(index));

    if is_contiguous {
        return Ok(());
    }

    Err(crate::XbergError::embedding(format!(
        "LLM embedding response incomplete or non-contiguous (model={model}): expected indices 0..{expected_len} \
         (one per input text), got {got} objects with indices {sorted_indices:?}",
        got = sorted_indices.len(),
    )))
}

/// L2-normalize an embedding vector in-place.
#[cfg(any(
    all(
        feature = "tokio-runtime",
        any(feature = "embeddings", feature = "static-embeddings"),
        not(target_arch = "wasm32")
    ),
    test
))]
fn normalize_l2(embedding: &mut [f32]) {
    let magnitude: f32 = embedding.iter().map(|x| x * x).sum::<f32>().sqrt();
    if magnitude > f32::EPSILON {
        let inv_mag = 1.0 / magnitude;
        embedding.iter_mut().for_each(|x| *x *= inv_mag);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_l2_unit_vector() {
        let mut v = vec![1.0f32, 0.0, 0.0];
        normalize_l2(&mut v);
        assert!((v[0] - 1.0).abs() < f32::EPSILON);
        assert!((v[1]).abs() < f32::EPSILON);
    }

    #[test]
    fn test_normalize_l2_arbitrary_vector() {
        let mut v = vec![3.0f32, 4.0];
        normalize_l2(&mut v);
        let magnitude: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((magnitude - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_normalize_l2_zero_vector() {
        let mut v = vec![0.0f32, 0.0, 0.0];
        normalize_l2(&mut v);
        assert!(v.iter().all(|&x| x == 0.0));
    }

    #[test]
    fn should_accept_complete_contiguous_indices() {
        let sorted_indices = vec![0u32, 1, 2, 3];
        let result = validate_contiguous_indices(&sorted_indices, 4, "test-model");
        assert!(result.is_ok());
    }

    #[test]
    fn should_reject_short_index_set_missing_last_entry() {
        let sorted_indices = vec![0u32, 1, 2];
        let result = validate_contiguous_indices(&sorted_indices, 4, "test-model");
        let err = result.expect_err("short index set must be rejected");
        let message = err.to_string();
        assert!(message.contains("test-model"), "error should name the model: {message}");
        assert!(
            message.contains("0..4"),
            "error should state the expected range: {message}"
        );
    }

    #[test]
    fn should_reject_index_set_with_gap() {
        let sorted_indices = vec![0u32, 1, 3];
        let result = validate_contiguous_indices(&sorted_indices, 4, "test-model");
        assert!(result.is_err(), "a gap in indices must be rejected");
    }

    #[test]
    fn should_reject_index_set_with_duplicate() {
        let sorted_indices = vec![0u32, 1, 1, 2];
        let result = validate_contiguous_indices(&sorted_indices, 4, "test-model");
        assert!(result.is_err(), "a duplicate index must be rejected");
    }

    /// Confirms that a base64-encoded provider response (the shape requested by
    /// `encoding_format: Some(EmbeddingFormat::Base64)` in `embed_via_llm`) decodes
    /// through `liter_llm::EmbeddingObject` to the exact same floats as an equivalent
    /// JSON float array response. This is the deserialization path
    /// `embed_via_llm`'s `response.data` relies on; if the request built the wrong
    /// `encoding_format` variant (or a provider only understands one shape), this
    /// test would not itself catch a request-side regression, but it pins the
    /// contract that both response shapes must keep producing bit-identical vectors.
    #[test]
    fn base64_embedding_response_matches_float_response_bit_exact() {
        use base64::Engine as _;

        let floats: [f32; 5] = [1.0, -2.5, 12.375, 0.0, f32::MIN_POSITIVE];

        let mut bytes = Vec::with_capacity(floats.len() * 4);
        for value in floats {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        let encoded = base64::engine::general_purpose::STANDARD.encode(&bytes);

        let base64_body = format!(r#"{{"object":"embedding","index":0,"embedding":{encoded:?}}}"#);
        let float_body = format!(
            r#"{{"object":"embedding","index":0,"embedding":{floats}}}"#,
            floats = serde_json::to_string(&floats.to_vec()).expect("serialize float array")
        );

        let base64_object: liter_llm::EmbeddingObject =
            serde_json::from_str(&base64_body).expect("base64 embedding response should deserialize");
        let float_object: liter_llm::EmbeddingObject =
            serde_json::from_str(&float_body).expect("float array embedding response should deserialize");

        assert_eq!(
            base64_object.embedding, floats,
            "base64-decoded floats must match the source bit-exactly"
        );
        assert_eq!(
            base64_object.embedding, float_object.embedding,
            "base64 and float-array response shapes must decode to identical vectors"
        );
    }
}
