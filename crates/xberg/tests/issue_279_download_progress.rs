//! Regression tests for issue #279: `show_download_progress` was declared on all four
//! model-download configs (`EmbeddingConfig`, `SparseEmbeddingConfig`, `RerankerConfig`,
//! `LateInteractionConfig`) but never read — no call site passed it anywhere near the Hugging
//! Face download layer, so users could neither turn progress on nor off.
//!
//! Two things are covered here:
//!
//! 1. The setting survives the config-file path (serde) for all four configs, both values.
//! 2. Each capability hands its config to the download layer, and the download layer registers
//!    the resulting handler with hf-hub. The download itself needs the network, so the wiring is
//!    asserted against the source of the four call sites; the value-level behaviour (a `true`
//!    setting yields a handler, a `false` setting yields none) is asserted in the unit tests in
//!    `src/core/config/download_progress.rs` and `src/model_download.rs`.

use xberg::{EmbeddingConfig, LateInteractionConfig, RerankerConfig, SparseEmbeddingConfig};

const PROCESSING_SRC: &str = include_str!("../src/core/config/processing.rs");
const SPARSE_EMBEDDING_SRC: &str = include_str!("../src/core/config/sparse_embedding.rs");
const RERANKER_SRC: &str = include_str!("../src/core/config/reranker.rs");
const LATE_INTERACTION_SRC: &str = include_str!("../src/core/config/late_interaction.rs");

const EMBEDDINGS_SRC: &str = include_str!("../src/embeddings/mod.rs");
const SPARSE_EMBEDDINGS_SRC: &str = include_str!("../src/sparse_embeddings/mod.rs");
const RERANKING_SRC: &str = include_str!("../src/reranking/mod.rs");
const LATE_INTERACTION_ENGINE_SRC: &str = include_str!("../src/late_interaction/mod.rs");
const ONNX_SRC: &str = include_str!("../src/onnx/mod.rs");
const STATIC_ENGINE_SRC: &str = include_str!("../src/embeddings/static_engine.rs");
const MODEL_DOWNLOAD_SRC: &str = include_str!("../src/model_download.rs");

/// Collapse every run of whitespace to a single space so source assertions survive rustfmt
/// re-wrapping and indentation changes.
fn flatten(source: &str) -> String {
    source.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[test]
fn embedding_config_carries_show_download_progress_through_a_config_file() {
    for requested in [true, false] {
        let parsed = reparse_with_progress(&EmbeddingConfig::default(), requested);
        assert_eq!(
            parsed.show_download_progress, requested,
            "EmbeddingConfig must deserialize show_download_progress={requested} verbatim"
        );
    }
}

#[test]
fn sparse_embedding_config_carries_show_download_progress_through_a_config_file() {
    for requested in [true, false] {
        let parsed = reparse_with_progress(&SparseEmbeddingConfig::default(), requested);
        assert_eq!(
            parsed.show_download_progress, requested,
            "SparseEmbeddingConfig must deserialize show_download_progress={requested} verbatim"
        );
    }
}

#[test]
fn reranker_config_carries_show_download_progress_through_a_config_file() {
    for requested in [true, false] {
        let parsed = reparse_with_progress(&RerankerConfig::default(), requested);
        assert_eq!(
            parsed.show_download_progress, requested,
            "RerankerConfig must deserialize show_download_progress={requested} verbatim"
        );
    }
}

#[test]
fn late_interaction_config_carries_show_download_progress_through_a_config_file() {
    for requested in [true, false] {
        let parsed = reparse_with_progress(&LateInteractionConfig::default(), requested);
        assert_eq!(
            parsed.show_download_progress, requested,
            "LateInteractionConfig must deserialize show_download_progress={requested} verbatim"
        );
    }
}

#[test]
fn all_four_configs_default_to_no_progress() {
    assert!(!EmbeddingConfig::default().show_download_progress);
    assert!(!SparseEmbeddingConfig::default().show_download_progress);
    assert!(!RerankerConfig::default().show_download_progress);
    assert!(!LateInteractionConfig::default().show_download_progress);
}

/// Serialize a config, flip `show_download_progress`, and read it back — the same path a
/// `xberg.{toml,yaml,json}` file takes. A `#[serde(skip)]`ed or renamed field would drop the
/// override and fail here.
fn reparse_with_progress<T>(config: &T, requested: bool) -> T
where
    T: serde::Serialize + serde::de::DeserializeOwned,
{
    let mut value = serde_json::to_value(config).expect("config must serialize");
    value
        .as_object_mut()
        .expect("config must serialize as a JSON object")
        .insert("show_download_progress".to_string(), serde_json::Value::Bool(requested));
    serde_json::from_value(value).expect("config must deserialize")
}

#[test]
fn every_capability_hands_its_config_to_the_engine_download() {
    for (capability, source) in [
        ("embeddings", EMBEDDINGS_SRC),
        ("sparse_embeddings", SPARSE_EMBEDDINGS_SRC),
        ("reranking", RERANKING_SRC),
        ("late_interaction", LATE_INTERACTION_ENGINE_SRC),
    ] {
        assert!(
            flatten(source).contains("config.cache_dir.clone(), config.into(),"),
            "{capability}: the engine download must receive the config's DownloadProgress, \
             otherwise show_download_progress is inert again (#279)"
        );
    }
}

#[test]
fn static_embedding_downloads_also_receive_the_embedding_config() {
    assert!(
        flatten(EMBEDDINGS_SRC).contains("cache_directory.as_deref(), config.into(),"),
        "the static (model2vec) embedding path must receive the config's DownloadProgress too"
    );
    assert!(
        flatten(STATIC_ENGINE_SRC).contains("hf_resolve_file_with_progress("),
        "the static engine must resolve model files through the progress-aware download entry point"
    );
}

#[test]
fn the_shared_onnx_downloader_forwards_progress_to_every_file_it_fetches() {
    let onnx = flatten(ONNX_SRC);
    assert_eq!(
        onnx.matches("hf_resolve_file_with_progress(").count(),
        3,
        "model, sibling weights and companion files must all be fetched through the \
         progress-aware download entry point"
    );
    assert!(
        !onnx.contains("model_download::hf_resolve_file("),
        "no ONNX capability file may fall back to the progress-less download entry point"
    );
}

#[test]
fn the_download_layer_registers_the_handler_with_hf_hub() {
    let model_download = flatten(MODEL_DOWNLOAD_SRC);
    assert_eq!(
        model_download.matches(".maybe_progress(handler)").count(),
        4,
        "every hf-hub download variant (pinned/floating revision, forced/not) must register the \
         progress handler the caller's setting produced"
    );
}

#[test]
fn no_config_still_documents_show_download_progress_as_inert() {
    for (name, source) in [
        ("EmbeddingConfig", PROCESSING_SRC),
        ("SparseEmbeddingConfig", SPARSE_EMBEDDING_SRC),
        ("RerankerConfig", RERANKER_SRC),
        ("LateInteractionConfig", LATE_INTERACTION_SRC),
    ] {
        assert!(
            !source.contains("no effect for any"),
            "{name}: show_download_progress is honoured now — its doc must no longer say the \
             setting has no effect (#279)"
        );
    }
}
