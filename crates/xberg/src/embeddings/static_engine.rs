//! Pure-Rust static (model2vec) embedding inference engine.
//!
//! Wraps [`model2vec_rs::model::StaticModel`]: a tokenize → embedding-table
//! lookup → mean-pool pipeline with no ONNX Runtime dependency. This is the
//! only dense-embedding backend available on `no-ort-target` (WASM, Android
//! x86_64 emulator).
//!
//! Model acquisition is target-split:
//! - Native and Android (`not(target_arch = "wasm32")`): downloads
//!   `tokenizer.json`, `model.safetensors`, and `config.json` from HuggingFace
//!   via xberg's own `hf-hub` dependency (declared for every target except
//!   `wasm32` in `Cargo.toml`), then loads them with
//!   [`StaticEmbeddingEngine::from_bytes`].
//! - WASM (and any other target without `hf-hub`): only
//!   [`StaticEmbeddingEngine::from_bytes`] is reachable — callers must supply
//!   the three files' bytes themselves. This module vendors no JS-fetch
//!   integration; `from_bytes` is the seam a WASM host binds to.
//!
//! Since v5.1.0 (`lightweight` preset).

use model2vec_rs::model::StaticModel;

/// A loaded static (model2vec) embedding model with thread-safe inference.
///
/// Rust-only: an opaque handle with no faithful binding representation (mirrors
/// `embeddings::engine::EmbeddingEngine` / `sparse_embeddings::engine::SparseEmbeddingEngine`).
/// Bindings drive inference through the module-level [`super::embed_texts`]
/// dispatch, not this type directly.
#[cfg_attr(alef, alef(skip))]
#[derive(Debug)]
pub struct StaticEmbeddingEngine {
    model: StaticModel,
}

impl StaticEmbeddingEngine {
    /// Build an engine directly from in-memory model bytes.
    ///
    /// Available on every target (including WASM) since it performs no I/O of
    /// its own — the caller is responsible for sourcing the three byte buffers
    /// (e.g. bundling them into the binary, or fetching them via a
    /// host-provided transport before calling in).
    pub fn from_bytes(tokenizer_bytes: &[u8], model_bytes: &[u8], config_bytes: &[u8]) -> crate::Result<Self> {
        let model = StaticModel::from_bytes(tokenizer_bytes, model_bytes, config_bytes, None)
            .map_err(|e| crate::XbergError::embedding(format!("Failed to load static embedding model: {e}")))?;
        Ok(Self { model })
    }

    /// Embed a batch of texts.
    ///
    /// Mean-pools token embedding-table lookups via model2vec's own `encode`
    /// (tokenize → lookup → pool); the preset's `pooling` field is informational
    /// only since model2vec has no CLS-token concept. `max_length` truncates
    /// each text (in tokens, char-approximated) before pooling.
    pub(crate) fn embed<S: AsRef<str>>(
        &self,
        texts: &[S],
        batch_size: usize,
        max_length: Option<usize>,
    ) -> Vec<Vec<f32>> {
        if texts.is_empty() {
            return Vec::new();
        }
        let batch_size = if batch_size == 0 { 32 } else { batch_size };
        let owned: Vec<String> = texts.iter().map(|t| t.as_ref().to_string()).collect();
        self.model.encode_with_args(&owned, max_length, batch_size)
    }
}

#[allow(unsafe_code)]
unsafe impl Send for StaticEmbeddingEngine {}
#[allow(unsafe_code)]
unsafe impl Sync for StaticEmbeddingEngine {}

/// Native (including Android): download a static-embedding model's files from
/// HuggingFace and build an engine. Not compiled on WASM — that target has no
/// `hf-hub` dependency declared anywhere in this crate; callers there must go
/// through [`StaticEmbeddingEngine::from_bytes`] directly.
#[cfg(not(target_arch = "wasm32"))]
mod download {
    use super::StaticEmbeddingEngine;
    use std::path::{Path, PathBuf};

    /// Fetch a single file, trying `<model_dir>/<file_name>` before falling
    /// back to `<file_name>` at the repo root (mirrors `crate::onnx::fetch_companion`'s
    /// layout convention for `xberg-io/embedding-models`).
    ///
    /// Returns the local cache path plus the repo-relative path that resolved, so
    /// the caller can verify it against the pinned sha256 manifest.
    fn fetch(
        repo_name: &str,
        model_dir: &str,
        file_name: &str,
        cache_directory: Option<&Path>,
        progress: crate::core::config::DownloadProgress,
        manifest: &[(String, String)],
    ) -> crate::Result<(PathBuf, String)> {
        let candidates: Vec<String> = if model_dir.is_empty() {
            vec![file_name.to_string()]
        } else {
            vec![format!("{model_dir}/{file_name}"), file_name.to_string()]
        };

        let mut last_err = String::new();
        for candidate in candidates {
            let expected = manifest
                .iter()
                .find(|(path, _)| path == &candidate)
                .map(|(_, sha256)| sha256.as_str());
            if !manifest.is_empty() && expected.is_none() {
                last_err = format!("SHA-256 manifest does not list {candidate}");
                continue;
            }
            match crate::model_download::hf_resolve_file_with_progress(
                repo_name,
                &candidate,
                Some(super::super::EMBEDDING_MODEL_REVISION),
                cache_directory,
                expected,
                progress,
            ) {
                Ok(resolved) => return Ok((resolved, candidate)),
                Err(e) => last_err = e,
            }
        }
        Err(crate::XbergError::embedding(format!(
            "Failed to download {file_name} from {repo_name} (model_dir={model_dir}): {last_err}"
        )))
    }

    /// Download a static-embedding model's three files and build an engine.
    ///
    /// `model_file` is the path (within `repo_name`) to `model.safetensors`;
    /// `tokenizer.json` and `config.json` are fetched from the same directory,
    /// falling back to the repo root.
    pub(crate) fn download_and_build(
        repo_name: &str,
        model_file: &str,
        cache_directory: Option<&Path>,
        progress: crate::core::config::DownloadProgress,
    ) -> crate::Result<StaticEmbeddingEngine> {
        let model_dir = Path::new(model_file)
            .parent()
            .and_then(|p| p.to_str())
            .unwrap_or_default();
        let model_file_name = Path::new(model_file)
            .file_name()
            .and_then(|f| f.to_str())
            .unwrap_or("model.safetensors");

        let manifest = crate::model_download::parse_sha256_manifest(super::super::EMBEDDING_SHA256_MANIFEST)
            .map_err(|e| crate::XbergError::embedding(format!("Invalid embedding sha256 manifest: {e}")))?;
        let verify = |repo_path: &str, local: &Path| -> crate::Result<()> {
            if let Some((_, sha256)) = manifest.iter().find(|(path, _)| path == repo_path) {
                crate::model_download::verify_sha256(local, sha256, repo_path).map_err(crate::XbergError::embedding)?;
            }
            Ok(())
        };

        let (model_path, model_rel) = fetch(
            repo_name,
            model_dir,
            model_file_name,
            cache_directory,
            progress,
            &manifest,
        )?;
        verify(&model_rel, &model_path)?;
        let (tokenizer_path, tokenizer_rel) = fetch(
            repo_name,
            model_dir,
            "tokenizer.json",
            cache_directory,
            progress,
            &manifest,
        )?;
        verify(&tokenizer_rel, &tokenizer_path)?;
        let (config_path, config_rel) = fetch(
            repo_name,
            model_dir,
            "config.json",
            cache_directory,
            progress,
            &manifest,
        )?;
        verify(&config_rel, &config_path)?;

        let model_bytes = std::fs::read(&model_path)
            .map_err(|e| crate::XbergError::embedding(format!("Failed to read {model_path:?}: {e}")))?;
        let tokenizer_bytes = std::fs::read(&tokenizer_path)
            .map_err(|e| crate::XbergError::embedding(format!("Failed to read {tokenizer_path:?}: {e}")))?;
        let config_bytes = std::fs::read(&config_path)
            .map_err(|e| crate::XbergError::embedding(format!("Failed to read {config_path:?}: {e}")))?;

        StaticEmbeddingEngine::from_bytes(&tokenizer_bytes, &model_bytes, &config_bytes)
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) use download::download_and_build;

/// WASM stub: there is no on-target download path for static-embedding models
/// (no `hf-hub` dependency is declared for `wasm32` anywhere in this crate).
/// Callers on this target must build the engine themselves via
/// [`StaticEmbeddingEngine::from_bytes`] and register it through the
/// [`crate::plugins::EmbeddingBackend`] plugin path instead of a `Preset`.
#[cfg(target_arch = "wasm32")]
pub(crate) fn download_and_build(
    repo_name: &str,
    _model_file: &str,
    _cache_directory: Option<&std::path::Path>,
    _progress: crate::core::config::DownloadProgress,
) -> crate::Result<StaticEmbeddingEngine> {
    Err(crate::XbergError::embedding(format!(
        "Static embedding model download ({repo_name}) is not available on this target (WASM); \
         load model bytes yourself via StaticEmbeddingEngine::from_bytes, or register a Plugin backend."
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal fixture: a 4-token vocabulary with a 3-dimensional embedding
    /// table, built directly via safetensors bytes (no network access) so this
    /// test runs offline and deterministically.
    fn build_fixture_bytes() -> (Vec<u8>, Vec<u8>, Vec<u8>) {
        use tokenizers::models::wordlevel::WordLevel;
        use tokenizers::{AddedToken, Tokenizer};

        let vocab: ahash::AHashMap<String, u32> = [
            ("[UNK]".to_string(), 0u32),
            ("hello".to_string(), 1),
            ("world".to_string(), 2),
            ("test".to_string(), 3),
        ]
        .into_iter()
        .collect();

        let model = WordLevel::builder()
            .vocab(vocab)
            .unk_token("[UNK]".to_string())
            .build()
            .expect("build WordLevel model");
        let mut tokenizer = Tokenizer::new(model);
        let _ = tokenizer.add_special_tokens([AddedToken::from("[UNK]", true)]);
        tokenizer.with_pre_tokenizer(Some(tokenizers::pre_tokenizers::whitespace::Whitespace {}));

        let tokenizer_json = tokenizer.to_string(false).expect("serialize tokenizer");

        const ROWS: usize = 4;
        const COLS: usize = 3;
        let mut embeddings = Vec::with_capacity(ROWS * COLS);
        for row in 0..ROWS {
            for col in 0..COLS {
                embeddings.push((row * COLS + col) as f32);
            }
        }
        let embedding_bytes: Vec<u8> = embeddings.iter().flat_map(|f| f.to_le_bytes()).collect();

        let tensors = std::collections::HashMap::from([(
            "embeddings".to_string(),
            safetensors::tensor::TensorView::new(safetensors::Dtype::F32, vec![ROWS, COLS], &embedding_bytes)
                .expect("build tensor view"),
        )]);
        let model_bytes = safetensors::serialize(&tensors, None).expect("serialize safetensors");

        let config_bytes = br#"{"normalize": false}"#.to_vec();

        (tokenizer_json.into_bytes(), model_bytes, config_bytes)
    }

    #[test]
    fn from_bytes_produces_expected_shape_and_dims() {
        let (tokenizer_bytes, model_bytes, config_bytes) = build_fixture_bytes();
        let engine = StaticEmbeddingEngine::from_bytes(&tokenizer_bytes, &model_bytes, &config_bytes)
            .expect("engine should build from valid fixture bytes");

        let texts = ["hello world", "test"];
        let embeddings = engine.embed(&texts, 32, Some(512));

        assert_eq!(embeddings.len(), 2, "one embedding per input text");
        for vector in &embeddings {
            assert_eq!(
                vector.len(),
                3,
                "embedding dimension must match the fixture's embedding table"
            );
        }
    }

    #[test]
    fn from_bytes_is_deterministic() {
        let (tokenizer_bytes, model_bytes, config_bytes) = build_fixture_bytes();
        let engine = StaticEmbeddingEngine::from_bytes(&tokenizer_bytes, &model_bytes, &config_bytes)
            .expect("engine should build from valid fixture bytes");

        let first = engine.embed(&["hello world"], 32, Some(512));
        let second = engine.embed(&["hello world"], 32, Some(512));
        assert_eq!(first, second, "identical input must produce identical output");
    }

    #[test]
    fn from_bytes_rejects_malformed_model_bytes() {
        let (tokenizer_bytes, _model_bytes, config_bytes) = build_fixture_bytes();
        let err = StaticEmbeddingEngine::from_bytes(&tokenizer_bytes, b"not-a-safetensors-file", &config_bytes)
            .expect_err("malformed safetensors bytes must be rejected, not panic");
        assert!(matches!(err, crate::XbergError::Embedding { .. }));
    }

    #[test]
    fn embed_empty_texts_returns_empty() {
        let (tokenizer_bytes, model_bytes, config_bytes) = build_fixture_bytes();
        let engine = StaticEmbeddingEngine::from_bytes(&tokenizer_bytes, &model_bytes, &config_bytes)
            .expect("engine should build from valid fixture bytes");

        let texts: [&str; 0] = [];
        let embeddings = engine.embed(&texts, 32, Some(512));
        assert!(embeddings.is_empty());
    }
}
