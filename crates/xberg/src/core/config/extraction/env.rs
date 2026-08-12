//! Environment variable override support for extraction configuration.
//!
//! This module provides functionality to apply environment variable overrides
//! to extraction configuration, allowing runtime configuration changes.

use crate::{Result, XbergError};

use super::super::csv::CsvConfig;
use super::super::ocr::OcrConfig;
use super::super::processing::ChunkingConfig;
use super::core::ExtractionConfig;
use super::types::{BreadcrumbTarget, TokenReductionOptions};

impl ExtractionConfig {
    /// Apply environment variable overrides to configuration.
    ///
    /// Environment variables have the highest precedence and will override any values
    /// loaded from configuration files. This method supports the following environment variables:
    ///
    /// - `XBERG_OCR_LANGUAGE`: OCR language (ISO 639-1 or 639-3 code, e.g., "eng", "fra", "deu")
    /// - `XBERG_OCR_BACKEND`: OCR backend ("tesseract", "paddleocr", "paddle-ocr", or "vlm")
    /// - `XBERG_OCR_MODEL_VERSION`: PaddleOCR model generation ("pp-ocrv6" or "pp-ocrv5")
    /// - `XBERG_OCR_MODEL_TIER`: PaddleOCR model tier (e.g. "medium"/"small"/"tiny" for v6, "mobile"/"server" for v5)
    /// - `XBERG_CHUNKING_MAX_CHARS`: Maximum characters per chunk (positive integer)
    /// - `XBERG_CHUNKING_MAX_OVERLAP`: Maximum overlap between chunks (non-negative integer)
    /// - `XBERG_CHUNKING_BREADCRUMB_TARGET`: Where the heading-path breadcrumb is written
    ///   when `prepend_heading_context` is enabled ("content", "metadata", or "both")
    ///   — see [`BreadcrumbTarget`](crate::core::config::extraction::BreadcrumbTarget)
    /// - `XBERG_CACHE_ENABLED`: Cache enabled flag ("true" or "false")
    /// - `XBERG_TOKEN_REDUCTION_MODE`: Token reduction mode ("off", "light", "moderate", "aggressive", or "maximum")
    /// - `XBERG_CHUNKING_TOKENIZER`: HuggingFace tokenizer model ID for token-based chunk sizing (requires `chunking-tokenizers` feature)
    /// - `XBERG_DISABLE_OCR`: Disable OCR entirely ("true" or "false")
    /// - `XBERG_LLM_MODEL`: LLM model for structured extraction (e.g., "openai/gpt-4o")
    /// - `XBERG_LLM_API_KEY`: API key for the structured extraction LLM provider. Applied only
    ///   when structured extraction is already configured — a credential never enables it.
    /// - `XBERG_LLM_BASE_URL`: Custom base URL for the structured extraction LLM provider. Applied
    ///   only when structured extraction is already configured — an endpoint never enables it.
    /// - `XBERG_VLM_OCR_MODEL`: VLM model for vision-based OCR (e.g., "openai/gpt-4o")
    /// - `XBERG_VLM_EMBEDDING_MODEL`: LLM model for embedding generation (e.g., "openai/text-embedding-3-small")
    /// - `XBERG_EMBEDDING_PLUGIN_NAME`: Name of an in-process embedding backend registered via `plugins::register_embedding_backend`
    /// - `XBERG_MSG_FALLBACK_CODEPAGE`: (deferred) Windows codepage for MSG PT_STRING8 fallback
    /// - `XBERG_CSV_DELIMITER`: CSV/TSV field delimiter, exactly one ASCII character
    ///   (e.g. ",", ";", "\t", "|"). Overrides delimiter auto-detection.
    /// - `XBERG_CSV_COMMENT_PREFIXES`: comma-separated list of line prefixes marking a
    ///   CSV/TSV comment line to skip (e.g. "#,//")
    ///
    /// # Behavior
    ///
    /// - If an environment variable is set and valid, it overrides the current configuration value
    /// - If a required parent config is `None` (e.g., `self.ocr` is None), it's created with
    ///   defaults before applying the override. The exception is credentials and endpoints —
    ///   `XBERG_LLM_API_KEY` and `XBERG_LLM_BASE_URL` — which are applied to an existing config
    ///   or ignored, and never enable a feature that was not asked for (issue #1421)
    /// - Invalid values return a `XbergError::Validation` with helpful error messages
    /// - Missing or unset environment variables are silently ignored
    ///
    /// # Example
    ///
    /// ```rust
    /// # use xberg::core::config::ExtractionConfig;
    /// # fn example() -> xberg::Result<()> {
    /// let mut config = ExtractionConfig::from_file("config.toml")?;
    /// // Set XBERG_OCR_LANGUAGE=fra before calling
    /// config.apply_env_overrides()?; // OCR language is now "fra"
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Errors
    ///
    /// Returns `XbergError::Validation` if:
    /// - An environment variable contains an invalid value
    /// - A number cannot be parsed as the expected type
    /// - A boolean is not "true" or "false"
    pub fn apply_env_overrides(&mut self) -> Result<()> {
        use crate::core::config_validation::{
            validate_chunking_params, validate_language_code, validate_ocr_backend, validate_token_reduction_level,
        };

        if let Ok(lang) = std::env::var("XBERG_OCR_LANGUAGE") {
            validate_language_code(&lang)?;
            if self.ocr.is_none() {
                self.ocr = Some(OcrConfig::default());
            }
            if let Some(ref mut ocr) = self.ocr {
                ocr.language = vec![lang];
            }
        }

        if let Ok(backend) = std::env::var("XBERG_OCR_BACKEND") {
            validate_ocr_backend(&backend)?;
            if self.ocr.is_none() {
                self.ocr = Some(OcrConfig::default());
            }
            if let Some(ref mut ocr) = self.ocr {
                ocr.backend = backend;
            }
        }

        let paddle_model_version = std::env::var("XBERG_OCR_MODEL_VERSION").ok();
        let paddle_model_tier = std::env::var("XBERG_OCR_MODEL_TIER").ok();
        if paddle_model_version.is_some() || paddle_model_tier.is_some() {
            if self.ocr.is_none() {
                self.ocr = Some(OcrConfig::default());
            }
            if let Some(ref mut ocr) = self.ocr {
                let mut paddle = match ocr.paddle_ocr_config.take() {
                    Some(serde_json::Value::Object(map)) => map,
                    _ => serde_json::Map::new(),
                };
                if let Some(version) = paddle_model_version {
                    paddle.insert("model_version".to_string(), serde_json::Value::String(version));
                }
                if let Some(tier) = paddle_model_tier {
                    paddle.insert("model_tier".to_string(), serde_json::Value::String(tier));
                }
                ocr.paddle_ocr_config = Some(serde_json::Value::Object(paddle));
            }
        }

        if let Ok(max_chars_str) = std::env::var("XBERG_CHUNKING_MAX_CHARS") {
            let max_chars: usize = max_chars_str.parse().map_err(|_| XbergError::Validation {
                message: format!(
                    "Invalid value for XBERG_CHUNKING_MAX_CHARS: '{}'. Must be a positive integer.",
                    max_chars_str
                ),
                source: None,
            })?;

            if max_chars == 0 {
                return Err(XbergError::Validation {
                    message: "XBERG_CHUNKING_MAX_CHARS must be greater than 0".to_string(),
                    source: None,
                });
            }

            if self.chunking.is_none() {
                self.chunking = Some(ChunkingConfig::default());
            }

            if let Some(ref mut chunking) = self.chunking {
                validate_chunking_params(max_chars, chunking.overlap)?;
                chunking.max_characters = max_chars;
            }
        }

        if let Ok(max_overlap_str) = std::env::var("XBERG_CHUNKING_MAX_OVERLAP") {
            let max_overlap: usize = max_overlap_str.parse().map_err(|_| XbergError::Validation {
                message: format!(
                    "Invalid value for XBERG_CHUNKING_MAX_OVERLAP: '{}'. Must be a non-negative integer.",
                    max_overlap_str
                ),
                source: None,
            })?;

            if self.chunking.is_none() {
                self.chunking = Some(ChunkingConfig::default());
            }

            if let Some(ref mut chunking) = self.chunking {
                validate_chunking_params(chunking.max_characters, max_overlap)?;
                chunking.overlap = max_overlap;
            }
        }

        if let Ok(target_str) = std::env::var("XBERG_CHUNKING_BREADCRUMB_TARGET") {
            let target = match target_str.to_lowercase().as_str() {
                "content" => BreadcrumbTarget::Content,
                "metadata" => BreadcrumbTarget::Metadata,
                _ => {
                    return Err(XbergError::Validation {
                        message: format!(
                            "Invalid value for XBERG_CHUNKING_BREADCRUMB_TARGET: '{}'. Must be 'content' or 'metadata'.",
                            target_str
                        ),
                        source: None,
                    });
                }
            };

            if self.chunking.is_none() {
                self.chunking = Some(ChunkingConfig::default());
            }

            if let Some(ref mut chunking) = self.chunking {
                chunking.breadcrumb_target = target;
            }
        }

        if let Ok(cache_str) = std::env::var("XBERG_CACHE_ENABLED") {
            let cache_enabled = match cache_str.to_lowercase().as_str() {
                "true" => true,
                "false" => false,
                _ => {
                    return Err(XbergError::Validation {
                        message: format!(
                            "Invalid value for XBERG_CACHE_ENABLED: '{}'. Must be 'true' or 'false'.",
                            cache_str
                        ),
                        source: None,
                    });
                }
            };
            self.use_cache = cache_enabled;
        }

        if let Ok(mode) = std::env::var("XBERG_TOKEN_REDUCTION_MODE") {
            validate_token_reduction_level(&mode)?;
            if self.token_reduction.is_none() {
                self.token_reduction = Some(TokenReductionOptions {
                    mode: "off".to_string(),
                    preserve_important_words: true,
                });
            }
            if let Some(ref mut token_reduction) = self.token_reduction {
                token_reduction.mode = mode;
            }
        }

        if let Ok(val) = std::env::var("XBERG_OUTPUT_FORMAT") {
            self.output_format = val.parse().map_err(|e: String| XbergError::Validation {
                message: format!("Invalid value for XBERG_OUTPUT_FORMAT: {}", e),
                source: None,
            })?;
        }

        #[cfg(feature = "chunking-tokenizers")]
        if let Ok(model) = std::env::var("XBERG_CHUNKING_TOKENIZER") {
            if model.is_empty() {
                return Err(XbergError::Validation {
                    message: "XBERG_CHUNKING_TOKENIZER must not be empty".to_string(),
                    source: None,
                });
            }

            if self.chunking.is_none() {
                self.chunking = Some(ChunkingConfig::default());
            }

            if let Some(ref mut chunking) = self.chunking {
                chunking.sizing = crate::core::config::processing::ChunkSizing::Tokenizer { model, cache_dir: None };
            }
        }

        #[cfg(feature = "layout-detection")]
        if let Ok(preset) = std::env::var("XBERG_LAYOUT_PRESET") {
            let lower = preset.to_lowercase();
            if !["fast", "accurate", "yolo", "rtdetr", "rt-detr"].contains(&lower.as_str()) {
                return Err(XbergError::Validation {
                    message: format!(
                        "Invalid value for XBERG_LAYOUT_PRESET: '{}'. Valid presets: fast, accurate",
                        preset
                    ),
                    source: None,
                });
            }
            if self.layout.is_none() {
                self.layout = Some(super::super::layout::LayoutDetectionConfig::default());
            }
            let _ = lower;
        }

        if let Ok(val) = std::env::var("XBERG_DISABLE_OCR") {
            self.disable_ocr = match val.to_lowercase().as_str() {
                "true" | "1" => true,
                "false" | "0" => false,
                _ => {
                    return Err(XbergError::Validation {
                        message: format!(
                            "Invalid value for XBERG_DISABLE_OCR: '{}'. Must be 'true' or 'false'.",
                            val
                        ),
                        source: None,
                    });
                }
            };
        }

        if let Ok(value) = std::env::var("XBERG_LLM_MODEL") {
            if value.is_empty() {
                return Err(XbergError::Validation {
                    message: "XBERG_LLM_MODEL must not be empty".to_string(),
                    source: None,
                });
            }
            if self.structured_extraction.is_none() {
                self.structured_extraction = Some(super::super::llm::StructuredExtractionConfig {
                    schema: serde_json::Value::Object(Default::default()),
                    schema_name: "extraction".to_string(),
                    schema_description: None,
                    strict: false,
                    prompt: None,
                    llm: super::super::llm::LlmConfig {
                        model: value,
                        api_key: None,
                        base_url: None,
                        ..Default::default()
                    },
                });
            } else if let Some(ref mut config) = self.structured_extraction {
                config.llm.model = value;
            }
        }

        // A credential is not a request to run a feature. `XBERG_LLM_API_KEY` and
        // `XBERG_LLM_BASE_URL` therefore only ever *configure* a structured extraction
        // that is already enabled; neither may bring one into existence. Before this,
        // both fabricated a `StructuredExtractionConfig` with an empty `model` and an
        // empty schema whenever they were set, and because
        // `config.structured_extraction.is_some()` is the sole gate in the pipeline
        // (`core/pipeline/mod.rs:449`), any deployment that merely had a key in its
        // environment ran the post-processor on *every* document and failed on every
        // one of them with "model must not be empty" (issue #1421). This mirrors the
        // rule the VLM forwarding block below already follows for the same two vars.
        // `XBERG_LLM_MODEL`, handled above, is a different case: naming a model for
        // structured extraction is an explicit request for it. ~keep
        if let Ok(value) = std::env::var("XBERG_LLM_API_KEY") {
            if value.is_empty() {
                return Err(XbergError::Validation {
                    message: "XBERG_LLM_API_KEY must not be empty".to_string(),
                    source: None,
                });
            }
            if let Some(ref mut config) = self.structured_extraction {
                config.llm.api_key = Some(value);
            }
        }

        if let Ok(value) = std::env::var("XBERG_LLM_BASE_URL") {
            if value.is_empty() {
                return Err(XbergError::Validation {
                    message: "XBERG_LLM_BASE_URL must not be empty".to_string(),
                    source: None,
                });
            }
            if let Some(ref mut config) = self.structured_extraction {
                config.llm.base_url = Some(value);
            }
        }

        if let Ok(value) = std::env::var("XBERG_VLM_OCR_MODEL") {
            if value.is_empty() {
                return Err(XbergError::Validation {
                    message: "XBERG_VLM_OCR_MODEL must not be empty".to_string(),
                    source: None,
                });
            }
            if self.ocr.is_none() {
                self.ocr = Some(OcrConfig::default());
            }
            if let Some(ref mut ocr) = self.ocr {
                if ocr.vlm_config.is_none() {
                    ocr.vlm_config = Some(super::super::llm::LlmConfig {
                        model: value,
                        ..Default::default()
                    });
                } else if let Some(ref mut vlm) = ocr.vlm_config {
                    vlm.model = value;
                }
            }
        }

        // Forward the general LLM credential env vars onto the VLM OCR client. Before
        // this, `XBERG_LLM_API_KEY` / `XBERG_LLM_BASE_URL` only reached structured
        // extraction (handled above), so a VLM OCR backend configured with a custom
        // `base_url` could never resolve its key from the environment and failed with
        // an authentication error — the env vars the docs promise as highest priority
        // were silently ignored for the VLM path (issue #1339). This runs after the
        // `XBERG_VLM_OCR_MODEL` block so a `vlm_config` created there is also covered,
        // and only applies when VLM OCR is already configured, so it never silently
        // enables the VLM path. The empty-string guard in the `XBERG_LLM_*` blocks
        // above already rejects empty values before we get here.
        if let Some(ref mut ocr) = self.ocr
            && let Some(ref mut vlm) = ocr.vlm_config
        {
            if let Ok(value) = std::env::var("XBERG_LLM_API_KEY")
                && !value.is_empty()
            {
                vlm.api_key = Some(value);
            }
            if let Ok(value) = std::env::var("XBERG_LLM_BASE_URL")
                && !value.is_empty()
            {
                vlm.base_url = Some(value);
            }
        }

        if let Ok(value) = std::env::var("XBERG_VLM_EMBEDDING_MODEL") {
            if value.is_empty() {
                return Err(XbergError::Validation {
                    message: "XBERG_VLM_EMBEDDING_MODEL must not be empty".to_string(),
                    source: None,
                });
            }
            if self.chunking.is_none() {
                self.chunking = Some(ChunkingConfig::default());
            }
            if let Some(ref mut chunking) = self.chunking {
                chunking.embedding = Some(super::super::processing::EmbeddingConfig {
                    model: super::super::processing::EmbeddingModelType::Llm {
                        llm: Box::new(super::super::llm::LlmConfig {
                            model: value,
                            api_key: None,
                            base_url: None,
                            ..Default::default()
                        }),
                    },
                    ..super::super::processing::EmbeddingConfig::default()
                });
            }
        }

        let plugin_name = std::env::var("XBERG_EMBEDDING_PLUGIN_NAME").ok();
        if plugin_name.is_some() && std::env::var("XBERG_VLM_EMBEDDING_MODEL").is_ok() {
            return Err(XbergError::Validation {
                message:
                    "XBERG_EMBEDDING_PLUGIN_NAME and XBERG_VLM_EMBEDDING_MODEL are mutually exclusive — set one or the other, not both."
                        .to_string(),
                source: None,
            });
        }
        if let Some(value) = plugin_name {
            if value.is_empty() {
                return Err(XbergError::Validation {
                    message: "XBERG_EMBEDDING_PLUGIN_NAME must not be empty".to_string(),
                    source: None,
                });
            }
            if self.chunking.is_none() {
                self.chunking = Some(ChunkingConfig::default());
            }
            if let Some(ref mut chunking) = self.chunking {
                chunking.embedding = Some(super::super::processing::EmbeddingConfig {
                    model: super::super::processing::EmbeddingModelType::Plugin { name: value },
                    ..super::super::processing::EmbeddingConfig::default()
                });
            }
        }

        if let Ok(value) = std::env::var("XBERG_CSV_DELIMITER") {
            crate::core::config_validation::validate_csv_delimiter(&value)?;
            if self.csv.is_none() {
                self.csv = Some(CsvConfig::default());
            }
            if let Some(ref mut csv) = self.csv {
                csv.delimiter = Some(value);
            }
        }

        if let Ok(value) = std::env::var("XBERG_CSV_COMMENT_PREFIXES") {
            let prefixes: Vec<String> = value
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect();
            if prefixes.is_empty() {
                return Err(XbergError::Validation {
                    message: "XBERG_CSV_COMMENT_PREFIXES must contain at least one non-empty prefix".to_string(),
                    source: None,
                });
            }
            if self.csv.is_none() {
                self.csv = Some(CsvConfig::default());
            }
            if let Some(ref mut csv) = self.csv {
                csv.comment_prefixes = prefixes;
            }
        }

        Ok(())
    }
}

#[cfg(test)]
#[allow(unsafe_code)]
mod tests {
    use super::*;
    use crate::core::config::processing::EmbeddingModelType;

    /// Lock guarding env-var mutation across tests in this module — `std::env::set_var`
    /// is process-global and concurrent tests would race.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn clear_embedding_env() {
        unsafe {
            std::env::remove_var("XBERG_EMBEDDING_PLUGIN_NAME");
            std::env::remove_var("XBERG_VLM_EMBEDDING_MODEL");
        }
    }

    #[test]
    fn embedding_plugin_and_vlm_embedding_model_are_mutually_exclusive() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        clear_embedding_env();
        unsafe {
            std::env::set_var("XBERG_EMBEDDING_PLUGIN_NAME", "my-embedder");
            std::env::set_var("XBERG_VLM_EMBEDDING_MODEL", "openai/text-embedding-3-small");
        }
        let mut config = ExtractionConfig::default();
        let err = config
            .apply_env_overrides()
            .expect_err("should reject conflicting embedding env vars");
        assert!(
            matches!(err, XbergError::Validation { .. }),
            "expected Validation, got {err:?}"
        );
        let msg = err.to_string();
        assert!(msg.contains("mutually exclusive"), "message: {msg}");
        clear_embedding_env();
    }

    #[test]
    fn empty_embedding_plugin_name_rejected() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        clear_embedding_env();
        unsafe { std::env::set_var("XBERG_EMBEDDING_PLUGIN_NAME", "") };
        let mut config = ExtractionConfig::default();
        let err = config
            .apply_env_overrides()
            .expect_err("should reject empty plugin name");
        assert!(
            matches!(err, XbergError::Validation { .. }),
            "expected Validation, got {err:?}"
        );
        clear_embedding_env();
    }

    #[test]
    fn embedding_plugin_env_sets_chunking_embedding_to_plugin_variant() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        clear_embedding_env();
        unsafe { std::env::set_var("XBERG_EMBEDDING_PLUGIN_NAME", "my-embedder") };
        let mut config = ExtractionConfig::default();
        config
            .apply_env_overrides()
            .expect("should succeed with only plugin name set");
        let chunking = config.chunking.as_ref().expect("chunking should be created");
        let embedding = chunking.embedding.as_ref().expect("embedding should be set");
        match &embedding.model {
            EmbeddingModelType::Plugin { name } => {
                assert_eq!(name, "my-embedder");
            }
            other => panic!("expected Plugin variant, got {other:?}"),
        }
        clear_embedding_env();
    }

    fn clear_llm_cred_env() {
        unsafe {
            std::env::remove_var("XBERG_LLM_API_KEY");
            std::env::remove_var("XBERG_LLM_BASE_URL");
            std::env::remove_var("XBERG_VLM_OCR_MODEL");
        }
    }

    /// Regression test for issue #1339: `XBERG_LLM_API_KEY` / `XBERG_LLM_BASE_URL`
    /// must be forwarded onto an existing VLM OCR config so a custom `base_url` can
    /// resolve its credential from the environment.
    #[test]
    fn llm_cred_env_forwarded_to_existing_vlm_config() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        clear_llm_cred_env();
        unsafe {
            std::env::set_var("XBERG_LLM_API_KEY", "sk-test-key");
            std::env::set_var("XBERG_LLM_BASE_URL", "https://eu.api.openai.com/v1/");
        }
        let mut config = ExtractionConfig {
            ocr: Some(crate::core::config::OcrConfig {
                vlm_config: Some(crate::core::config::LlmConfig {
                    model: "gpt-4o-mini".to_string(),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            ..Default::default()
        };
        config.apply_env_overrides().expect("env overrides should apply");
        let vlm = config.ocr.unwrap().vlm_config.unwrap();
        assert_eq!(vlm.api_key.as_deref(), Some("sk-test-key"));
        assert_eq!(vlm.base_url.as_deref(), Some("https://eu.api.openai.com/v1/"));
        clear_llm_cred_env();
    }

    /// The forwarding must never fabricate a VLM config: a credential in the
    /// environment configures an OCR path that is already enabled, and never turns
    /// one on (issue #1339). The same rule now holds for structured extraction —
    /// see `llm_cred_env_does_not_enable_structured_extraction` (issue #1421).
    #[test]
    fn llm_cred_env_does_not_create_vlm_config() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        clear_llm_cred_env();
        unsafe {
            std::env::set_var("XBERG_LLM_API_KEY", "sk-test-key");
        }
        let mut config = ExtractionConfig::default();
        config.apply_env_overrides().expect("env overrides should apply");
        assert!(
            config.ocr.as_ref().and_then(|o| o.vlm_config.as_ref()).is_none(),
            "VLM config must not be auto-created from LLM cred env vars"
        );
        clear_llm_cred_env();
    }

    /// Regression test for issue #1421. `config.structured_extraction.is_some()` is
    /// the *only* gate the pipeline consults (`core/pipeline/mod.rs`), so fabricating
    /// the config from a credential registered a post-processor that had neither a
    /// model nor a schema and failed on every single document with
    /// "model must not be empty". A key in the environment must leave the feature off.
    #[test]
    fn llm_cred_env_does_not_enable_structured_extraction() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        clear_llm_cred_env();
        unsafe {
            std::env::set_var("XBERG_LLM_API_KEY", "sk-test-key");
            std::env::set_var("XBERG_LLM_BASE_URL", "https://eu.api.openai.com/v1/");
        }
        let mut config = ExtractionConfig::default();
        config.apply_env_overrides().expect("env overrides should apply");
        assert!(
            config.structured_extraction.is_none(),
            "a credential and an endpoint must not enable structured extraction; \
             got {:?}",
            config.structured_extraction
        );
        clear_llm_cred_env();
    }

    /// The other half of #1421: suppressing the fabrication must not stop the
    /// credential reaching a structured extraction the user actually asked for.
    #[test]
    fn llm_cred_env_applies_to_existing_structured_extraction() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        clear_llm_cred_env();
        unsafe {
            std::env::set_var("XBERG_LLM_API_KEY", "sk-test-key");
            std::env::set_var("XBERG_LLM_BASE_URL", "https://eu.api.openai.com/v1/");
        }
        let mut config = ExtractionConfig {
            structured_extraction: Some(crate::core::config::StructuredExtractionConfig {
                schema: serde_json::json!({"type": "object"}),
                schema_name: "invoice".to_string(),
                schema_description: None,
                strict: false,
                prompt: None,
                llm: crate::core::config::LlmConfig {
                    model: "openai/gpt-4o".to_string(),
                    ..Default::default()
                },
            }),
            ..Default::default()
        };
        config.apply_env_overrides().expect("env overrides should apply");
        let structured = config
            .structured_extraction
            .expect("structured extraction stays enabled");
        assert_eq!(structured.llm.api_key.as_deref(), Some("sk-test-key"));
        assert_eq!(
            structured.llm.base_url.as_deref(),
            Some("https://eu.api.openai.com/v1/")
        );
        assert_eq!(
            structured.llm.model, "openai/gpt-4o",
            "the configured model must survive"
        );
        assert_eq!(structured.schema_name, "invoice", "the configured schema must survive");
        clear_llm_cred_env();
    }

    fn clear_breadcrumb_target_env() {
        unsafe {
            std::env::remove_var("XBERG_CHUNKING_BREADCRUMB_TARGET");
        }
    }

    #[test]
    fn breadcrumb_target_env_sets_chunking_field() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        clear_breadcrumb_target_env();
        unsafe { std::env::set_var("XBERG_CHUNKING_BREADCRUMB_TARGET", "metadata") };
        let mut config = ExtractionConfig::default();
        config
            .apply_env_overrides()
            .expect("valid breadcrumb target should apply");
        assert_eq!(
            config.chunking.as_ref().unwrap().breadcrumb_target,
            BreadcrumbTarget::Metadata
        );
        clear_breadcrumb_target_env();
    }

    #[test]
    fn breadcrumb_target_env_rejects_invalid_value() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        clear_breadcrumb_target_env();
        unsafe { std::env::set_var("XBERG_CHUNKING_BREADCRUMB_TARGET", "nonsense") };
        let mut config = ExtractionConfig::default();
        let err = config
            .apply_env_overrides()
            .expect_err("invalid breadcrumb target should be rejected");
        assert!(matches!(err, XbergError::Validation { .. }));
        clear_breadcrumb_target_env();
    }

    fn clear_paddle_model_env() {
        unsafe {
            std::env::remove_var("XBERG_OCR_MODEL_VERSION");
            std::env::remove_var("XBERG_OCR_MODEL_TIER");
        }
    }

    #[test]
    fn paddle_model_env_vars_populate_paddle_ocr_config() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        clear_paddle_model_env();
        unsafe {
            std::env::set_var("XBERG_OCR_MODEL_VERSION", "pp-ocrv5");
            std::env::set_var("XBERG_OCR_MODEL_TIER", "server");
        }
        let mut config = ExtractionConfig::default();
        config
            .apply_env_overrides()
            .expect("paddle model env vars should apply");
        let paddle = config
            .ocr
            .as_ref()
            .and_then(|o| o.paddle_ocr_config.as_ref())
            .expect("paddle_ocr_config should be populated");
        assert_eq!(paddle.get("model_version").and_then(|v| v.as_str()), Some("pp-ocrv5"));
        assert_eq!(paddle.get("model_tier").and_then(|v| v.as_str()), Some("server"));
        clear_paddle_model_env();
    }

    #[test]
    fn paddle_model_env_wins_over_existing_config_and_preserves_other_keys() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        clear_paddle_model_env();
        unsafe { std::env::set_var("XBERG_OCR_MODEL_VERSION", "pp-ocrv5") };
        let mut config = ExtractionConfig {
            ocr: Some(OcrConfig {
                paddle_ocr_config: Some(serde_json::json!({
                    "model_version": "pp-ocrv6",
                    "drop_score": 0.7,
                })),
                ..OcrConfig::default()
            }),
            ..ExtractionConfig::default()
        };
        config
            .apply_env_overrides()
            .expect("paddle model env override should apply");
        let paddle = config.ocr.as_ref().unwrap().paddle_ocr_config.as_ref().unwrap();
        assert_eq!(paddle.get("model_version").and_then(|v| v.as_str()), Some("pp-ocrv5"));
        assert_eq!(paddle.get("drop_score").and_then(|v| v.as_f64()), Some(0.7));
        assert!(paddle.get("model_tier").is_none());
        clear_paddle_model_env();
    }

    fn clear_csv_env() {
        unsafe {
            std::env::remove_var("XBERG_CSV_DELIMITER");
            std::env::remove_var("XBERG_CSV_COMMENT_PREFIXES");
        }
    }

    #[test]
    fn csv_delimiter_env_sets_csv_config() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        clear_csv_env();
        unsafe { std::env::set_var("XBERG_CSV_DELIMITER", ";") };
        let mut config = ExtractionConfig::default();
        config.apply_env_overrides().expect("valid delimiter should apply");
        assert_eq!(config.csv.as_ref().unwrap().delimiter.as_deref(), Some(";"));
        clear_csv_env();
    }

    #[test]
    fn csv_delimiter_env_rejects_multi_byte_value() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        clear_csv_env();
        unsafe { std::env::set_var("XBERG_CSV_DELIMITER", "::") };
        let mut config = ExtractionConfig::default();
        let err = config
            .apply_env_overrides()
            .expect_err("multi-byte delimiter should be rejected");
        assert!(matches!(err, XbergError::Validation { .. }));
        clear_csv_env();
    }

    #[test]
    fn csv_comment_prefixes_env_sets_csv_config() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        clear_csv_env();
        unsafe { std::env::set_var("XBERG_CSV_COMMENT_PREFIXES", "#, //") };
        let mut config = ExtractionConfig::default();
        config
            .apply_env_overrides()
            .expect("valid comment prefixes should apply");
        assert_eq!(
            config.csv.as_ref().unwrap().comment_prefixes,
            vec!["#".to_string(), "//".to_string()]
        );
        clear_csv_env();
    }

    #[test]
    fn csv_comment_prefixes_env_rejects_empty_value() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        clear_csv_env();
        unsafe { std::env::set_var("XBERG_CSV_COMMENT_PREFIXES", " , ") };
        let mut config = ExtractionConfig::default();
        let err = config
            .apply_env_overrides()
            .expect_err("blank comment prefixes should be rejected");
        assert!(matches!(err, XbergError::Validation { .. }));
        clear_csv_env();
    }
}
