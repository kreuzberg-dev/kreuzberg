//! Per-download progress-reporting preference (#279).
//!
//! Every model-backed capability config — [`EmbeddingConfig`](super::processing::EmbeddingConfig),
//! [`SparseEmbeddingConfig`](super::sparse_embedding::SparseEmbeddingConfig),
//! [`RerankerConfig`](super::reranker::RerankerConfig) and
//! [`LateInteractionConfig`](super::late_interaction::LateInteractionConfig) — exposes a
//! `show_download_progress` setting. This type is what that setting becomes on its way to the
//! Hugging Face download layer, so the preference travels with the request rather than being read
//! back out of a global.
//!
//! Kept in the config module (not `crate::model_download`) so it compiles on every target,
//! including `wasm32`, where no Hugging Face client exists.

/// Whether a single model download renders progress while it transfers.
///
/// Convert from the capability config that triggered the download
/// (`DownloadProgress::from(&config)` / `config.into()`); use [`DownloadProgress::SILENT`] for
/// downloads no capability config governs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[allow(dead_code)]
pub(crate) struct DownloadProgress {
    enabled: bool,
}

#[allow(dead_code)]
impl DownloadProgress {
    /// Render no progress at all.
    ///
    /// Used by the artifact downloads that no capability config governs — OCR, layout,
    /// orientation, transcription and tokenizer models — which have never reported progress.
    pub(crate) const SILENT: Self = Self { enabled: false };

    /// Build the preference from a raw `show_download_progress` flag.
    pub(crate) const fn new(enabled: bool) -> Self {
        Self { enabled }
    }

    /// Whether the download should render progress.
    pub(crate) const fn is_enabled(self) -> bool {
        self.enabled
    }
}

impl From<&super::processing::EmbeddingConfig> for DownloadProgress {
    fn from(config: &super::processing::EmbeddingConfig) -> Self {
        Self::new(config.show_download_progress)
    }
}

impl From<&super::sparse_embedding::SparseEmbeddingConfig> for DownloadProgress {
    fn from(config: &super::sparse_embedding::SparseEmbeddingConfig) -> Self {
        Self::new(config.show_download_progress)
    }
}

impl From<&super::reranker::RerankerConfig> for DownloadProgress {
    fn from(config: &super::reranker::RerankerConfig) -> Self {
        Self::new(config.show_download_progress)
    }
}

impl From<&super::late_interaction::LateInteractionConfig> for DownloadProgress {
    fn from(config: &super::late_interaction::LateInteractionConfig) -> Self {
        Self::new(config.show_download_progress)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::config::late_interaction::LateInteractionConfig;
    use crate::core::config::processing::EmbeddingConfig;
    use crate::core::config::reranker::RerankerConfig;
    use crate::core::config::sparse_embedding::SparseEmbeddingConfig;

    #[test]
    fn silent_is_disabled_and_is_the_default() {
        assert!(!DownloadProgress::SILENT.is_enabled());
        assert_eq!(DownloadProgress::default(), DownloadProgress::SILENT);
    }

    #[test]
    fn new_round_trips_the_flag() {
        assert!(DownloadProgress::new(true).is_enabled());
        assert!(!DownloadProgress::new(false).is_enabled());
    }

    #[test]
    fn embedding_config_show_download_progress_reaches_the_download_layer() {
        for requested in [true, false] {
            let config = EmbeddingConfig {
                show_download_progress: requested,
                ..Default::default()
            };
            assert_eq!(
                DownloadProgress::from(&config),
                DownloadProgress::new(requested),
                "EmbeddingConfig::show_download_progress={requested} must reach the download layer"
            );
        }
    }

    #[test]
    fn sparse_embedding_config_show_download_progress_reaches_the_download_layer() {
        for requested in [true, false] {
            let config = SparseEmbeddingConfig {
                show_download_progress: requested,
                ..Default::default()
            };
            assert_eq!(
                DownloadProgress::from(&config),
                DownloadProgress::new(requested),
                "SparseEmbeddingConfig::show_download_progress={requested} must reach the download layer"
            );
        }
    }

    #[test]
    fn reranker_config_show_download_progress_reaches_the_download_layer() {
        for requested in [true, false] {
            let config = RerankerConfig {
                show_download_progress: requested,
                ..Default::default()
            };
            assert_eq!(
                DownloadProgress::from(&config),
                DownloadProgress::new(requested),
                "RerankerConfig::show_download_progress={requested} must reach the download layer"
            );
        }
    }

    #[test]
    fn late_interaction_config_show_download_progress_reaches_the_download_layer() {
        for requested in [true, false] {
            let config = LateInteractionConfig {
                show_download_progress: requested,
                ..Default::default()
            };
            assert_eq!(
                DownloadProgress::from(&config),
                DownloadProgress::new(requested),
                "LateInteractionConfig::show_download_progress={requested} must reach the download layer"
            );
        }
    }
}
