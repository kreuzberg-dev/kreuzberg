//! Xberg MCP server implementation.
//!
//! This module provides the main MCP server struct and startup functions.

use super::format::build_config;
use crate::ExtractionConfig;
use crate::service::{ExtractionRequest, ExtractionServiceBuilder};
use rmcp::{
    ServerHandler, ServiceExt,
    handler::server::{
        prompt::PromptContext,
        router::{prompt::PromptRouter, tool::ToolRouter},
        tool::ToolCallContext,
        wrapper::Parameters,
    },
    model::*,
    task_manager::{TaskExit, TaskManager, TaskOptions},
    tool, tool_handler, tool_router,
    transport::stdio,
};
use tower::util::BoxCloneService;

/// Tool names materialized as SEP-2663 tasks (`tasks/get`, `tasks/update`,
/// `tasks/cancel`) when the calling client declares the tasks extension
/// capability. These are the operations whose latency (large documents,
/// batches of inputs, or multi-gigabyte model downloads) benefits from
/// asynchronous polling instead of holding the `tools/call` request open
/// for the duration of the work. All other tools stay synchronous.
const TASK_ELIGIBLE_TOOLS: &[&str] = &["extract", "extract_batch", "cache_warm"];

/// Task TTL for `extract`/`extract_batch`: generous headroom above the
/// documented OCR performance targets (<2s single page, <10s 10-page
/// document) to cover pathological large or heavily-scanned documents.
const EXTRACTION_TASK_TTL_MS: u64 = 15 * 60 * 1000;

/// Task TTL for `cache_warm`: model downloads run over the network against
/// third-party hosts and can legitimately take longer than extraction.
const CACHE_WARM_TASK_TTL_MS: u64 = 30 * 60 * 1000;

#[cfg(any(
    test,
    paddle_ocr,
    feature = "layout-detection",
    feature = "embeddings",
    feature = "ner-onnx"
))]
#[allow(dead_code)] // Individual dispositions are feature-dependent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CacheWarmDisposition {
    AvailabilityConfirmed,
    Downloaded,
    AlreadyCached,
}

#[cfg(any(
    test,
    paddle_ocr,
    feature = "layout-detection",
    feature = "embeddings",
    feature = "ner-onnx"
))]
fn record_warmed_model(
    available: &mut Vec<String>,
    downloaded: &mut Vec<String>,
    already_cached: &mut Vec<String>,
    label: String,
    disposition: CacheWarmDisposition,
) {
    available.push(label.clone());
    match disposition {
        CacheWarmDisposition::AvailabilityConfirmed => {}
        CacheWarmDisposition::Downloaded => downloaded.push(label),
        CacheWarmDisposition::AlreadyCached => already_cached.push(label),
    }
}

#[cfg(feature = "mcp-http")]
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
};

/// Xberg MCP server.
///
/// Provides document extraction capabilities via MCP tools.
///
/// The server loads a default extraction configuration from xberg.toml/yaml/json
/// via discovery. Per-request OCR settings override the defaults.
#[cfg_attr(alef, alef(skip))]
pub struct XbergMcp {
    tool_router: ToolRouter<XbergMcp>,
    /// Prompt router for the three guided-workflow prompts.
    prompt_router: PromptRouter<XbergMcp>,
    /// Default extraction configuration loaded from config file via discovery
    default_config: std::sync::Arc<ExtractionConfig>,
    /// Tower service for extraction requests with tracing and metrics layers.
    ///
    /// Wrapped in `Mutex` because `BoxCloneService` is `Send` but not `Sync`,
    /// while `XbergMcp` must be `Sync` for the MCP handler trait.
    /// The lock is held only long enough to clone the service.
    extraction_service:
        std::sync::Mutex<BoxCloneService<ExtractionRequest, crate::types::ExtractedDocument, crate::XbergError>>,
    /// SEP-2663 task store for tools in [`TASK_ELIGIBLE_TOOLS`]. Cheaply
    /// cloneable; all clones of an `XbergMcp` share the same task state.
    tasks: TaskManager,
}

impl Clone for XbergMcp {
    fn clone(&self) -> Self {
        let svc = self
            .extraction_service
            .lock()
            .expect("extraction service lock poisoned")
            .clone();
        Self {
            tool_router: self.tool_router.clone(),
            prompt_router: self.prompt_router.clone(),
            default_config: self.default_config.clone(),
            extraction_service: std::sync::Mutex::new(svc),
            tasks: self.tasks.clone(),
        }
    }
}

#[tool_router]
impl XbergMcp {
    /// Create a new Xberg MCP server instance with default config.
    ///
    /// Uses `ExtractionConfig::discover()` to search for xberg.toml/yaml/json
    /// in current and parent directories. Falls back to default configuration if
    /// no config file is found.
    #[allow(clippy::manual_unwrap_or_default)]
    pub(crate) fn new() -> crate::Result<Self> {
        let config = match ExtractionConfig::discover()? {
            Some(config) => {
                #[cfg(feature = "api")]
                tracing::info!("Loaded extraction config from discovered file");
                config
            }
            None => {
                #[cfg(feature = "api")]
                tracing::info!("No config file found, using default configuration");
                ExtractionConfig::default()
            }
        };

        Ok(Self::with_config(config))
    }

    /// Create a new Xberg MCP server instance with explicit config.
    ///
    /// # Arguments
    ///
    /// * `config` - Default extraction configuration for all tool calls
    pub(crate) fn with_config(config: ExtractionConfig) -> Self {
        let extraction_service = ExtractionServiceBuilder::new().with_tracing().with_metrics().build();

        Self {
            tool_router: Self::tool_router(),
            prompt_router: super::prompts::build_prompt_router(),
            default_config: std::sync::Arc::new(config),
            extraction_service: std::sync::Mutex::new(extraction_service),
            tasks: TaskManager::new(),
        }
    }

    /// Extract content from bytes or a URI.
    #[tool(
        description = "Extract content from bytes, a local path, file:// URI, remote document URL, or website URL.",
        annotations(title = "Extract", read_only_hint = true, idempotent_hint = true, open_world_hint = true),
        output_schema = rmcp::handler::server::common::schema_for_output::<super::schema::ExtractionResult>()
    )]
    async fn extract(
        &self,
        Parameters(params): Parameters<super::params::ExtractParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        use super::errors::map_xberg_error_to_mcp;

        let use_toon = params
            .response_format
            .as_deref()
            .is_some_and(|f| f.eq_ignore_ascii_case("toon"));

        let mut config =
            build_config(&self.default_config, params.config).map_err(|e| rmcp::ErrorData::invalid_params(e, None))?;
        apply_pdf_password(&mut config, params.pdf_password)?;
        let input = parse_extract_input(params.input)?;

        let output = crate::extract(input, &config).await.map_err(map_xberg_error_to_mcp)?;
        let response = format_extraction_result_for_wire(&output, use_toon);
        let mut tool_result = CallToolResult::success(vec![ContentBlock::text(response)]);
        tool_result.structured_content = serde_json::to_value(&output).ok();
        Ok(tool_result)
    }

    /// Extract content from multiple bytes or URI inputs.
    #[tool(
        description = "Extract content from multiple bytes, local paths, file:// URIs, remote document URLs, or website URLs.",
        annotations(title = "Extract Batch", read_only_hint = true, idempotent_hint = true, open_world_hint = true),
        output_schema = rmcp::handler::server::common::schema_for_output::<super::schema::ExtractionResult>()
    )]
    async fn extract_batch(
        &self,
        Parameters(params): Parameters<super::params::ExtractBatchParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        use super::errors::map_xberg_error_to_mcp;

        let use_toon = params
            .response_format
            .as_deref()
            .is_some_and(|f| f.eq_ignore_ascii_case("toon"));

        let mut config =
            build_config(&self.default_config, params.config).map_err(|e| rmcp::ErrorData::invalid_params(e, None))?;
        apply_pdf_password(&mut config, params.pdf_password)?;
        let inputs = params
            .inputs
            .into_iter()
            .map(parse_extract_input)
            .collect::<Result<Vec<_>, _>>()?;

        let output = crate::extract_batch(inputs, &config)
            .await
            .map_err(map_xberg_error_to_mcp)?;
        let response = format_extraction_result_for_wire(&output, use_toon);
        let mut tool_result = CallToolResult::success(vec![ContentBlock::text(response)]);
        tool_result.structured_content = serde_json::to_value(&output).ok();
        Ok(tool_result)
    }

    /// Detect the MIME type of a file.
    ///
    /// This tool identifies the file format, useful for determining which extractor to use.
    #[tool(
        description = "Detect the MIME type of a file. Returns the detected MIME type string.",
        annotations(title = "Detect MIME Type", read_only_hint = true, idempotent_hint = true),
        output_schema = rmcp::handler::server::common::schema_for_output::<super::schema::DetectMimeTypeOutput>()
    )]
    fn detect_mime_type(
        &self,
        Parameters(params): Parameters<super::params::DetectMimeTypeParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        use super::errors::map_xberg_error_to_mcp;
        use crate::detect_mime_type;

        let mime_type = detect_mime_type(params.path.clone(), params.use_content).map_err(map_xberg_error_to_mcp)?;

        let dto = super::schema::DetectMimeTypeOutput {
            mime_type: mime_type.clone(),
        };
        let mut tool_result = CallToolResult::success(vec![ContentBlock::text(mime_type)]);
        tool_result.structured_content = serde_json::to_value(&dto).ok();
        Ok(tool_result)
    }

    /// Get cache statistics.
    ///
    /// This tool returns statistics about the cache including total files, size, and disk space.
    #[tool(
        description = "Get cache statistics including total files, size, and available disk space.",
        annotations(title = "Cache Stats", read_only_hint = true, idempotent_hint = true),
        output_schema = rmcp::handler::server::common::schema_for_output::<super::schema::CacheStatsOutput>()
    )]
    fn cache_stats(
        &self,
        Parameters(_): Parameters<super::params::EmptyParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        use super::errors::map_xberg_error_to_mcp;
        use crate::cache;

        let cache_dir = crate::cache_dir::resolve_cache_base();

        let stats = cache::get_cache_metadata(cache_dir.to_str().unwrap_or(".")).map_err(map_xberg_error_to_mcp)?;

        let response = format!(
            "Cache Statistics\n\
             ================\n\
             Directory: {}\n\
             Total files: {}\n\
             Total size: {:.2} MB\n\
             Available space: {:.2} MB\n\
             Oldest file age: {:.2} days\n\
             Newest file age: {:.2} days",
            cache_dir.to_string_lossy(),
            stats.total_files,
            stats.total_size_mb,
            stats.available_space_mb,
            stats.oldest_file_age_days,
            stats.newest_file_age_days
        );

        let dto = super::schema::CacheStatsOutput {
            directory: cache_dir.to_string_lossy().into_owned(),
            total_files: stats.total_files as u64,
            total_size_mb: stats.total_size_mb,
            available_space_mb: stats.available_space_mb,
        };
        let mut tool_result = CallToolResult::success(vec![ContentBlock::text(response)]);
        tool_result.structured_content = serde_json::to_value(&dto).ok();
        Ok(tool_result)
    }

    /// List all supported document formats.
    ///
    /// This tool returns all file extensions and MIME types that Xberg can process.
    #[tool(
        description = "List all supported document formats with their file extensions and MIME types.",
        annotations(title = "List Formats", read_only_hint = true, idempotent_hint = true),
        output_schema = rmcp::handler::server::common::schema_for_output::<super::schema::ListFormatsOutput>()
    )]
    fn list_formats(
        &self,
        Parameters(_): Parameters<super::params::EmptyParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let formats = crate::core::mime::list_supported_formats();
        let response = serde_json::to_string_pretty(&formats).unwrap_or_default();
        let dto = super::schema::ListFormatsOutput {
            formats: formats
                .into_iter()
                .map(|f| serde_json::to_value(f).unwrap_or_default())
                .collect(),
        };
        let mut tool_result = CallToolResult::success(vec![ContentBlock::text(response)]);
        tool_result.structured_content = serde_json::to_value(&dto).ok();
        Ok(tool_result)
    }

    /// Clear the Xberg-managed cache.
    ///
    /// Shared Hugging Face Hub model cache files are intentionally excluded.
    #[tool(
        description = "Clear Xberg-managed cache files. Shared Hugging Face Hub model cache files are not removed.",
        annotations(title = "Clear Cache", read_only_hint = false, destructive_hint = true),
        output_schema = rmcp::handler::server::common::schema_for_output::<super::schema::CacheClearOutput>()
    )]
    fn cache_clear(
        &self,
        Parameters(_): Parameters<super::params::EmptyParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        use super::errors::map_xberg_error_to_mcp;
        use crate::cache;

        let cache_dir = crate::cache_dir::resolve_cache_base();

        let (removed_files, freed_mb) =
            cache::clear_cache_directory(cache_dir.to_str().unwrap_or(".")).map_err(map_xberg_error_to_mcp)?;

        let response = format!(
            "Xberg-managed cache cleared successfully\n\
             Directory: {}\n\
             Removed files: {}\n\
             Freed space: {:.2} MB\n\
             Shared Hugging Face cache cleared: no",
            cache_dir.to_string_lossy(),
            removed_files,
            freed_mb
        );

        let dto = super::schema::CacheClearOutput {
            directory: cache_dir.to_string_lossy().into_owned(),
            removed_files: removed_files as u64,
            freed_mb,
        };
        let mut tool_result = CallToolResult::success(vec![ContentBlock::text(response)]);
        tool_result.structured_content = serde_json::to_value(&dto).ok();
        Ok(tool_result)
    }

    /// Get Xberg version information.
    ///
    /// Returns the current version of the Xberg library.
    #[tool(
        description = "Get the current Xberg library version.",
        annotations(title = "Get Version", read_only_hint = true, idempotent_hint = true),
        output_schema = rmcp::handler::server::common::schema_for_output::<super::schema::VersionOutput>()
    )]
    fn get_version(
        &self,
        Parameters(_): Parameters<super::params::EmptyParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let version = env!("CARGO_PKG_VERSION");
        let dto = super::schema::VersionOutput {
            version: version.to_string(),
        };
        let response = serde_json::to_string_pretty(&dto).unwrap_or_default();
        let mut tool_result = CallToolResult::success(vec![ContentBlock::text(response)]);
        tool_result.structured_content = serde_json::to_value(&dto).ok();
        Ok(tool_result)
    }

    /// Get model manifest with expected model files and checksums.
    ///
    /// Returns a manifest of all model files Xberg expects, including
    /// their sizes and SHA256 checksums.
    #[tool(
        description = "Get model manifest listing expected model files, sizes, and SHA256 checksums.",
        annotations(title = "Cache Manifest", read_only_hint = true, idempotent_hint = true),
        output_schema = rmcp::handler::server::common::schema_for_output::<super::schema::CacheManifestOutput>()
    )]
    fn cache_manifest(
        &self,
        Parameters(_): Parameters<super::params::EmptyParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        #[allow(unused_mut)]
        let mut entries: Vec<serde_json::Value> = Vec::new();

        #[cfg(paddle_ocr)]
        {
            let manifest = crate::paddle_ocr::ModelManager::manifest();
            for entry in manifest {
                entries.push(serde_json::to_value(&entry).unwrap_or_default());
            }
        }

        #[cfg(feature = "layout-detection")]
        {
            let manifest = crate::layout::LayoutModelManager::manifest();
            for entry in manifest {
                entries.push(serde_json::to_value(&entry).unwrap_or_default());
            }
        }

        #[cfg(feature = "formula-recognition")]
        {
            let manifest = crate::formula_recognition::manifest();
            for entry in manifest {
                entries.push(serde_json::to_value(&entry).unwrap_or_default());
            }
        }

        #[cfg(feature = "ner-onnx")]
        {
            let manifest = crate::text::ner::manifest();
            for entry in manifest {
                entries.push(serde_json::to_value(&entry).unwrap_or_default());
            }
        }

        let total_size_bytes: u64 = entries
            .iter()
            .filter_map(|e| e.get("size_bytes").and_then(|v| v.as_u64()))
            .sum();
        let version = env!("CARGO_PKG_VERSION");

        let dto = super::schema::CacheManifestOutput {
            xberg_version: version.to_string(),
            model_count: entries.len(),
            total_size_bytes,
            models: entries,
        };
        let response = serde_json::to_string_pretty(&dto).unwrap_or_default();
        let mut tool_result = CallToolResult::success(vec![ContentBlock::text(response)]);
        tool_result.structured_content = serde_json::to_value(&dto).ok();
        Ok(tool_result)
    }

    /// Download and cache model files.
    ///
    /// Eagerly downloads model files so they are available for offline use.
    /// Hugging Face artifacts remain in the standard shared HF cache.
    #[tool(
        description = "Download model files for offline use. Hugging Face artifacts, including GLiNER NER models, remain in the standard shared HF cache.",
        annotations(
            title = "Cache Warm",
            read_only_hint = false,
            destructive_hint = false,
            open_world_hint = true
        ),
        output_schema = rmcp::handler::server::common::schema_for_output::<super::schema::CacheWarmOutput>()
    )]
    #[allow(unused_mut)]
    fn cache_warm(
        &self,
        Parameters(params): Parameters<super::params::CacheWarmParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        if let Some(ref name) = params.embedding_model
            && name.trim().is_empty()
        {
            return Err(rmcp::ErrorData::invalid_params(
                "Field 'embedding_model' must not be empty. Omit the field or provide a valid preset name.".to_string(),
                None,
            ));
        }
        if let Some(ref name) = params.ner_model
            && name.trim().is_empty()
        {
            return Err(rmcp::ErrorData::invalid_params(
                "Field 'ner_model' must not be empty. Omit the field or provide a valid model name.".to_string(),
                None,
            ));
        }

        let cache_base = resolve_cache_base();

        let mut available: Vec<String> = Vec::new();
        let mut downloaded: Vec<String> = Vec::new();
        let mut already_cached: Vec<String> = Vec::new();

        #[cfg(paddle_ocr)]
        {
            let paddle_dir = cache_base.join("paddle-ocr");
            let manager = crate::paddle_ocr::ModelManager::new(paddle_dir);
            manager.ensure_all_models().map_err(|e| {
                rmcp::ErrorData::internal_error(format!("Failed to download PaddleOCR models: {}", e), None)
            })?;
            record_warmed_model(
                &mut available,
                &mut downloaded,
                &mut already_cached,
                "paddle-ocr v2 (server+mobile det, cls, doc_ori, unified+per-script rec)".to_string(),
                CacheWarmDisposition::AvailabilityConfirmed,
            );
        }

        #[cfg(feature = "formula-recognition")]
        {
            let was_cached = crate::formula_recognition::models_cached();
            crate::formula_recognition::ensure_models().map_err(|e| {
                rmcp::ErrorData::internal_error(format!("Failed to download formula recognition models: {e}"), None)
            })?;
            record_warmed_model(
                &mut available,
                &mut downloaded,
                &mut already_cached,
                "formula-recognition (latex-ocr)".to_string(),
                if was_cached {
                    CacheWarmDisposition::AlreadyCached
                } else {
                    CacheWarmDisposition::Downloaded
                },
            );
        }

        #[cfg(feature = "layout-detection")]
        {
            let layout_dir = cache_base.join("layout");
            let manager = crate::layout::LayoutModelManager::new(Some(layout_dir));
            let rtdetr_was_cached = manager.is_rtdetr_cached();
            let tatr_was_cached = manager.is_tatr_cached();
            if rtdetr_was_cached && tatr_was_cached {
                record_warmed_model(
                    &mut available,
                    &mut downloaded,
                    &mut already_cached,
                    "layout (rtdetr, tatr)".to_string(),
                    CacheWarmDisposition::AlreadyCached,
                );
            } else {
                manager.ensure_all_models().map_err(|e| {
                    rmcp::ErrorData::internal_error(format!("Failed to download layout models: {}", e), None)
                })?;
                let disposition = if !rtdetr_was_cached && !tatr_was_cached {
                    CacheWarmDisposition::Downloaded
                } else {
                    CacheWarmDisposition::AvailabilityConfirmed
                };
                record_warmed_model(
                    &mut available,
                    &mut downloaded,
                    &mut already_cached,
                    "layout (rtdetr, tatr)".to_string(),
                    disposition,
                );
            }
        }

        #[cfg(feature = "embeddings")]
        {
            let embeddings_dir = cache_base.join("embeddings");
            let presets_to_warm: Vec<crate::EmbeddingPreset> = if params.all_embeddings {
                crate::embeddings::EMBEDDING_PRESETS.clone()
            } else if let Some(ref name) = params.embedding_model {
                match crate::embeddings::get_preset(name) {
                    Some(preset) => vec![preset],
                    None => {
                        let available: Vec<String> = crate::embeddings::list_presets();
                        return Err(rmcp::ErrorData::invalid_params(
                            format!(
                                "Unknown embedding preset '{}'. Available: {}",
                                name,
                                available.join(", ")
                            ),
                            None,
                        ));
                    }
                }
            } else {
                vec![]
            };

            for preset in &presets_to_warm {
                let label = format!("embedding ({})", preset.name);
                crate::embeddings::warm_model(
                    &crate::core::config::EmbeddingModelType::Preset {
                        name: preset.name.clone(),
                    },
                    Some(embeddings_dir.clone()),
                )
                .map_err(|e| {
                    rmcp::ErrorData::internal_error(
                        format!("Failed to download embedding model '{}': {}", preset.name, e),
                        None,
                    )
                })?;
                record_warmed_model(
                    &mut available,
                    &mut downloaded,
                    &mut already_cached,
                    label,
                    CacheWarmDisposition::AvailabilityConfirmed,
                );
            }
        }

        #[cfg(not(feature = "embeddings"))]
        {
            if params.all_embeddings || params.embedding_model.is_some() {
                return Err(rmcp::ErrorData::invalid_params(
                    "Embedding model warming requires the 'embeddings' feature to be enabled".to_string(),
                    None,
                ));
            }
        }

        #[cfg(feature = "ner-onnx")]
        {
            if params.ner || params.all_ner_models || params.ner_model.is_some() {
                let models_to_warm: Vec<String> = if params.all_ner_models {
                    crate::text::ner::known_models().iter().map(|s| s.to_string()).collect()
                } else if let Some(ref name) = params.ner_model {
                    vec![name.clone()]
                } else {
                    vec![crate::text::ner::default_model_name().to_string()]
                };

                for model in &models_to_warm {
                    let path = crate::text::ner::download_model(model, None).map_err(|e| {
                        rmcp::ErrorData::internal_error(
                            format!("Failed to download NER model '{}': {}", model, e),
                            None,
                        )
                    })?;
                    record_warmed_model(
                        &mut available,
                        &mut downloaded,
                        &mut already_cached,
                        format!("ner gliner ({model}) -> {} (Hugging Face cache)", path.display()),
                        CacheWarmDisposition::AvailabilityConfirmed,
                    );
                }
            }
        }

        #[cfg(not(feature = "ner-onnx"))]
        {
            if params.ner || params.all_ner_models || params.ner_model.is_some() {
                return Err(rmcp::ErrorData::invalid_params(
                    "NER model warming requires the 'ner-onnx' feature to be enabled".to_string(),
                    None,
                ));
            }
        }

        let dto = super::schema::CacheWarmOutput {
            cache_dir: cache_base.to_string_lossy().into_owned(),
            available,
            downloaded,
            already_cached,
        };

        let response = serde_json::json!({
            "cache_dir": cache_base.to_string_lossy(),
            "xberg_cache_dir": cache_base.to_string_lossy(),
            "hugging_face_cache": if params.ner || params.all_ner_models || params.ner_model.is_some() {
                Some("HF_HUB_CACHE/HF_HOME/platform default")
            } else {
                None
            },
            "available": dto.available.clone(),
            "downloaded": dto.downloaded.clone(),
            "already_cached": dto.already_cached.clone(),
        });

        let mut tool_result = CallToolResult::success(vec![ContentBlock::text(
            serde_json::to_string_pretty(&response).unwrap_or_default(),
        )]);
        tool_result.structured_content = serde_json::to_value(&dto).ok();
        Ok(tool_result)
    }
}

impl XbergMcp {
    /// Materialize a SEP-2663 task for `request` and return its
    /// `CreateTaskResult` handle. Only called for tool names in
    /// [`TASK_ELIGIBLE_TOOLS`] once the caller has confirmed the client
    /// declared the tasks extension capability; unknown names are rejected
    /// rather than silently falling back to synchronous execution, since a
    /// caller reaching this method already committed to the task path.
    ///
    /// Cancellation is cooperative (SEP-2663): a `tasks/cancel` unblocks
    /// [`rmcp::task_manager::TaskContext::cancelled`] and the task settles as
    /// `cancelled` immediately, but the underlying extraction/download may
    /// keep running in the background to completion since the core
    /// extraction APIs do not accept a cancellation token today.
    fn spawn_task_for_tool(&self, request: CallToolRequestParams) -> Result<CallToolResponse, rmcp::ErrorData> {
        let arguments = serde_json::Value::Object(request.arguments.unwrap_or_default());
        let server = self.clone();

        let task = match request.name.as_ref() {
            "extract" => {
                let params: super::params::ExtractParams = serde_json::from_value(arguments)
                    .map_err(|e| rmcp::ErrorData::invalid_params(format!("Invalid extract params: {e}"), None))?;
                self.tasks
                    .spawn(TaskOptions::new().with_ttl_ms(EXTRACTION_TASK_TTL_MS), move |ctx| {
                        Box::pin(async move {
                            tokio::select! {
                                _ = ctx.cancelled() => Err(TaskExit::Cancelled),
                                result = server.extract(Parameters(params)) => result.map_err(TaskExit::Error),
                            }
                        })
                    })
            }
            "extract_batch" => {
                let params: super::params::ExtractBatchParams = serde_json::from_value(arguments)
                    .map_err(|e| rmcp::ErrorData::invalid_params(format!("Invalid extract_batch params: {e}"), None))?;
                self.tasks
                    .spawn(TaskOptions::new().with_ttl_ms(EXTRACTION_TASK_TTL_MS), move |ctx| {
                        Box::pin(async move {
                            tokio::select! {
                                _ = ctx.cancelled() => Err(TaskExit::Cancelled),
                                result = server.extract_batch(Parameters(params)) => result.map_err(TaskExit::Error),
                            }
                        })
                    })
            }
            "cache_warm" => {
                let params: super::params::CacheWarmParams = serde_json::from_value(arguments)
                    .map_err(|e| rmcp::ErrorData::invalid_params(format!("Invalid cache_warm params: {e}"), None))?;
                self.tasks
                    .spawn(TaskOptions::new().with_ttl_ms(CACHE_WARM_TASK_TTL_MS), move |ctx| {
                        Box::pin(async move {
                            // cache_warm is synchronous (blocking downloads/IO); run it off the
                            // async runtime per the async-and-concurrency rule.
                            let handle = tokio::task::spawn_blocking(move || server.cache_warm(Parameters(params)));
                            tokio::select! {
                                _ = ctx.cancelled() => Err(TaskExit::Cancelled),
                                joined = handle => match joined {
                                    Ok(result) => result.map_err(TaskExit::Error),
                                    Err(join_error) => Err(TaskExit::Error(rmcp::ErrorData::internal_error(
                                        format!("cache_warm task panicked: {join_error}"),
                                        None,
                                    ))),
                                },
                            }
                        })
                    })
            }
            other => {
                return Err(rmcp::ErrorData::invalid_params(
                    format!("'{other}' is not a task-eligible tool"),
                    None,
                ));
            }
        };

        Ok(CallToolResponse::Task(CreateTaskResult::new(task)))
    }
}

/// Resolve the cache base directory.
fn resolve_cache_base() -> std::path::PathBuf {
    crate::cache_dir::resolve_cache_base()
}

fn parse_extract_input(value: serde_json::Value) -> Result<crate::ExtractInput, rmcp::ErrorData> {
    serde_json::from_value::<crate::ExtractInput>(value)
        .map_err(|error| rmcp::ErrorData::invalid_params(format!("Invalid ExtractInput: {error}"), None))
}

fn format_extraction_result_for_wire(output: &crate::ExtractionResult, use_toon: bool) -> String {
    if use_toon {
        serde_toon::to_string(output).unwrap_or_else(|error| {
            tracing::error!(%error, "Failed to serialize extraction result to TOON, falling back to JSON");
            serde_json::to_string_pretty(output).unwrap_or_default()
        })
    } else {
        serde_json::to_string_pretty(output).unwrap_or_default()
    }
}

fn apply_pdf_password(config: &mut ExtractionConfig, password: Option<String>) -> Result<(), rmcp::ErrorData> {
    let Some(password) = password else {
        return Ok(());
    };
    if password.is_empty() {
        return Err(rmcp::ErrorData::invalid_params(
            "pdf_password must not be empty when set".to_string(),
            None,
        ));
    }

    #[cfg(feature = "pdf")]
    {
        let pdf_options = config
            .pdf_options
            .get_or_insert_with(crate::core::config::pdf::PdfConfig::default);
        pdf_options.passwords.get_or_insert_with(Vec::new).push(password);
        Ok(())
    }

    #[cfg(not(feature = "pdf"))]
    {
        let _ = config;
        Err(rmcp::ErrorData::invalid_params(
            "pdf_password requires the 'pdf' feature to be enabled".to_string(),
            None,
        ))
    }
}

/// Handle completion requests for prompt arguments and resource URIs.
fn complete_impl(request: CompleteRequestParams) -> Result<CompleteResult, rmcp::ErrorData> {
    use rmcp::model::{CompletionInfo, Reference};

    let arg_name = &request.argument.name;
    let arg_value = &request.argument.value;

    let candidates: Vec<String> = match &request.r#ref {
        Reference::Prompt(prompt_ref) => match (prompt_ref.name.as_str(), arg_name.as_str()) {
            (_, "languages") => complete_ocr_languages(arg_value),
            (_, "preset") => complete_embedding_presets(arg_value),
            (_, "chunker_type") => complete_chunker_types(arg_value),
            (_, "output_format") => complete_output_formats(arg_value),
            _ => vec![],
        },
        Reference::Resource(_) => vec![],
        _ => vec![],
    };

    let completion = CompletionInfo::with_all_values(candidates).unwrap_or_default();
    Ok(CompleteResult::new(completion))
}

/// Return OCR language code completions filtered by the given prefix.
fn complete_ocr_languages(prefix: &str) -> Vec<String> {
    let all = [
        "afr", "amh", "ara", "asm", "aze", "bel", "ben", "bod", "bos", "bul", "cat", "ceb", "ces", "chi_sim",
        "chi_tra", "chr", "cos", "cym", "dan", "deu", "div", "dzo", "ell", "eng", "enm", "epo", "est", "eus", "fao",
        "fas", "fil", "fin", "fra", "frm", "gle", "glg", "grc", "guj", "hat", "heb", "hin", "hrv", "hun", "hye", "iku",
        "ind", "isl", "ita", "ita_old", "jav", "jpn", "kan", "kat", "kaz", "khm", "kir", "kor", "kur", "lao", "lat",
        "lav", "lit", "ltz", "mal", "mar", "mkd", "mlt", "mon", "mri", "msa", "mya", "nep", "nor", "oci", "ori", "pan",
        "pol", "por", "pus", "ron", "rus", "san", "sin", "slk", "slv", "snd", "spa", "spa_old", "sqi", "srp", "swa",
        "swe", "syr", "tam", "tat", "tel", "tgk", "tgl", "tha", "tir", "ton", "tur", "uig", "ukr", "urd", "uzb", "vie",
        "yid", "yor",
    ];
    let last = prefix.split(',').next_back().unwrap_or(prefix).trim();
    all.iter()
        .filter(|lang| lang.starts_with(last))
        .map(|s| s.to_string())
        .take(20)
        .collect()
}

/// Return embedding preset completions filtered by prefix.
fn complete_embedding_presets(prefix: &str) -> Vec<String> {
    let presets = ["speed", "balanced", "quality"];
    presets
        .iter()
        .filter(|p| p.starts_with(prefix))
        .map(|s| s.to_string())
        .collect()
}

/// Return chunker type completions filtered by prefix.
fn complete_chunker_types(prefix: &str) -> Vec<String> {
    let types = ["text", "markdown", "yaml", "semantic"];
    types
        .iter()
        .filter(|t| t.starts_with(prefix))
        .map(|s| s.to_string())
        .collect()
}

/// Return output format completions filtered by prefix.
fn complete_output_formats(prefix: &str) -> Vec<String> {
    let formats = ["json", "toon"];
    formats
        .iter()
        .filter(|f| f.starts_with(prefix))
        .map(|s| s.to_string())
        .collect()
}

#[tool_handler]
impl ServerHandler for XbergMcp {
    fn get_info(&self) -> ServerInfo {
        let capabilities = ServerCapabilities::builder()
            .enable_tools()
            .enable_resources()
            .enable_prompts()
            .enable_completions()
            .enable_tasks()
            .build();

        let server_info = Implementation::new("xberg-mcp", env!("CARGO_PKG_VERSION"))
            .with_title("Xberg Document Intelligence MCP Server")
            .with_description(
                "Document intelligence library for extracting content from PDFs, images, office documents, and more.",
            )
            .with_website_url("https://docs.xberg.io");

        InitializeResult::new(capabilities)
            .with_server_info(server_info)
            .with_instructions(
                "Extract content from documents in various formats. Supports PDFs, Word documents, \
                 Excel spreadsheets, images (with OCR), HTML, emails, and more. Use enable_ocr=true \
                 for scanned documents, force_ocr=true to always use OCR even if text extraction \
                 succeeds. Use disable_ocr=true to skip OCR entirely (images return metadata only).",
            )
    }

    /// Dispatch `tools/call`, materializing a SEP-2663 task for
    /// [`TASK_ELIGIBLE_TOOLS`] when the client declared the tasks extension
    /// capability, and falling back to the standard synchronous tool router
    /// otherwise (including for clients that never declared the capability).
    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: rmcp::service::RequestContext<rmcp::service::RoleServer>,
    ) -> Result<CallToolResponse, rmcp::ErrorData> {
        let client_supports_tasks = context.client_capabilities().is_some_and(|caps| caps.supports_tasks());

        if client_supports_tasks && TASK_ELIGIBLE_TOOLS.contains(&request.name.as_ref()) {
            return self.spawn_task_for_tool(request);
        }

        let tcc = ToolCallContext::new(self, request, context);
        self.tool_router.call(tcc).await
    }

    /// SEP-2663 `tasks/get`: return the current task state.
    async fn get_task(
        &self,
        request: GetTaskParams,
        _context: rmcp::service::RequestContext<rmcp::service::RoleServer>,
    ) -> Result<GetTaskResult, rmcp::ErrorData> {
        Ok(GetTaskResult::new(self.tasks.get_task(&request.task_id)?))
    }

    /// SEP-2663 `tasks/update`: none of our task-eligible tools currently
    /// surface mid-task `inputRequests` (elicitation/sampling/roots), so
    /// this only exists to acknowledge the request per spec; it delegates to
    /// the shared [`TaskManager`], which ignores unknown/stale keys.
    async fn update_task(
        &self,
        request: UpdateTaskParams,
        _context: rmcp::service::RequestContext<rmcp::service::RoleServer>,
    ) -> Result<(), rmcp::ErrorData> {
        self.tasks.update_task(&request.task_id, request.input_responses)
    }

    /// SEP-2663 `tasks/cancel`: cooperative cancellation, see
    /// [`XbergMcp::spawn_task_for_tool`] for what "cooperative" means here.
    async fn cancel_task(
        &self,
        request: CancelTaskParams,
        _context: rmcp::service::RequestContext<rmcp::service::RoleServer>,
    ) -> Result<(), rmcp::ErrorData> {
        self.tasks.cancel_task(&request.task_id)
    }

    fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: rmcp::service::RequestContext<rmcp::service::RoleServer>,
    ) -> impl std::future::Future<Output = Result<ListResourcesResult, rmcp::ErrorData>> + rmcp::service::MaybeSendFuture + '_
    {
        std::future::ready(Ok(super::resources::list_resources()))
    }

    fn list_resource_templates(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: rmcp::service::RequestContext<rmcp::service::RoleServer>,
    ) -> impl std::future::Future<Output = Result<ListResourceTemplatesResult, rmcp::ErrorData>>
    + rmcp::service::MaybeSendFuture
    + '_ {
        std::future::ready(Ok(super::resources::list_resource_templates()))
    }

    fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: rmcp::service::RequestContext<rmcp::service::RoleServer>,
    ) -> impl std::future::Future<Output = Result<ReadResourceResponse, rmcp::ErrorData>> + rmcp::service::MaybeSendFuture + '_
    {
        std::future::ready(super::resources::read_resource(&request.uri).map(Into::into))
    }

    fn list_prompts(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: rmcp::service::RequestContext<rmcp::service::RoleServer>,
    ) -> impl std::future::Future<Output = Result<ListPromptsResult, rmcp::ErrorData>> + rmcp::service::MaybeSendFuture + '_
    {
        let prompts = self.prompt_router.list_all();
        std::future::ready(Ok(ListPromptsResult::with_all_items(prompts)))
    }

    fn get_prompt(
        &self,
        request: GetPromptRequestParams,
        context: rmcp::service::RequestContext<rmcp::service::RoleServer>,
    ) -> impl std::future::Future<Output = Result<GetPromptResponse, rmcp::ErrorData>> + rmcp::service::MaybeSendFuture + '_
    {
        let pr = self.prompt_router.clone();
        let pc = PromptContext::new(self, request.name, request.arguments, context);
        async move { pr.get_prompt(pc).await }
    }

    fn complete(
        &self,
        request: CompleteRequestParams,
        _context: rmcp::service::RequestContext<rmcp::service::RoleServer>,
    ) -> impl std::future::Future<Output = Result<CompleteResult, rmcp::ErrorData>> + rmcp::service::MaybeSendFuture + '_
    {
        std::future::ready(complete_impl(request))
    }
}

impl Default for XbergMcp {
    fn default() -> Self {
        Self::new().unwrap_or_else(|e| {
            #[cfg(feature = "api")]
            tracing::warn!("Failed to discover config, using default: {}", e);
            #[cfg(not(feature = "api"))]
            tracing::debug!("Warning: Failed to discover config, using default: {}", e);
            Self::with_config(ExtractionConfig::default())
        })
    }
}

/// Start the Xberg MCP server.
///
/// This function initializes and runs the MCP server using stdio transport.
/// It will block until the server is shut down.
///
/// # Errors
///
/// Returns an error if the server fails to start or encounters a fatal error.
///
/// # Example
///
/// ```rust,no_run
/// use xberg::mcp::start_mcp_server;
///
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
///     start_mcp_server().await?;
///     Ok(())
/// }
/// ```
#[cfg_attr(alef, alef(skip))]
pub async fn start_mcp_server() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let service = XbergMcp::new()?.serve(stdio()).await?;

    service.waiting().await?;
    Ok(())
}

/// Start MCP server with custom extraction config.
///
/// This variant allows specifying a custom extraction configuration
/// (e.g., loaded from a file) instead of using defaults.
#[cfg_attr(alef, alef(skip))]
pub async fn start_mcp_server_with_config(
    config: ExtractionConfig,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let service = XbergMcp::with_config(config).serve(stdio()).await?;

    service.waiting().await?;
    Ok(())
}

/// Wait for a shutdown signal: SIGTERM on Unix platforms or Ctrl-C on all platforms.
///
/// The future resolves as soon as the first signal arrives, allowing axum's
/// `with_graceful_shutdown` to drain in-flight connections before the process exits.
#[cfg(feature = "mcp-http")]
async fn mcp_shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};

        let mut sigterm = match signal(SignalKind::terminate()) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("Failed to install SIGTERM handler: {}", e);
                tokio::signal::ctrl_c()
                    .await
                    .unwrap_or_else(|e| tracing::warn!("Failed to listen for Ctrl-C: {}", e));
                tracing::info!("MCP server shutting down gracefully on signal...");
                return;
            }
        };

        tokio::select! {
            _ = sigterm.recv() => {
                tracing::info!("MCP server shutting down gracefully on signal...");
            }
            result = tokio::signal::ctrl_c() => {
                if let Err(e) = result {
                    tracing::warn!("Failed to listen for Ctrl-C: {}", e);
                }
                tracing::info!("MCP server shutting down gracefully on signal...");
            }
        }
    }

    #[cfg(not(unix))]
    {
        tokio::signal::ctrl_c()
            .await
            .unwrap_or_else(|e| tracing::warn!("Failed to listen for Ctrl-C: {}", e));
        tracing::info!("MCP server shutting down gracefully on signal...");
    }
}

/// Build the rmcp Streamable HTTP server config, extending (never replacing) the
/// built-in loopback-only `allowed_hosts` default with any caller-supplied hosts.
///
/// Extending rather than replacing keeps `localhost`/`127.0.0.1`/`::1` working even when
/// the server also needs to accept a reverse-proxy or ingress hostname in the `Host`
/// header. Entries are trimmed and de-duplicated against the existing list; blank hosts
/// are ignored. An empty `extra_allowed_hosts` leaves rmcp's default unchanged.
#[cfg(feature = "mcp-http")]
fn build_streamable_http_config(extra_allowed_hosts: &[String]) -> StreamableHttpServerConfig {
    let mut config = StreamableHttpServerConfig::default();
    for host in extra_allowed_hosts {
        let host = host.trim();
        if !host.is_empty() && !config.allowed_hosts.iter().any(|existing| existing == host) {
            config.allowed_hosts.push(host.to_string());
        }
    }
    config
}

/// Start MCP server with HTTP Stream transport.
///
/// Uses rmcp's built-in StreamableHttpService for HTTP/SSE support per MCP spec.
///
/// # Arguments
///
/// * `host` - Host to bind to (e.g., "127.0.0.1" or "0.0.0.0")
/// * `port` - Port number (e.g., 8001)
/// * `extra_allowed_hosts` - Additional `Host` header values to accept, on top of rmcp's
///   loopback-only default (`localhost`, `127.0.0.1`, `::1`). Needed when the server runs
///   behind a reverse proxy or ingress that forwards a different hostname. Pass an empty
///   slice to keep the default loopback-only behavior.
///
/// # Example
///
/// ```no_run
/// use xberg::mcp::start_mcp_server_http;
///
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
///     start_mcp_server_http("127.0.0.1", 8001, &[]).await?;
///     Ok(())
/// }
/// ```
#[cfg(feature = "mcp-http")]
#[cfg_attr(alef, alef(skip))]
pub async fn start_mcp_server_http(
    host: impl AsRef<str>,
    port: u16,
    extra_allowed_hosts: &[String],
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use axum::Router;
    use std::net::SocketAddr;

    let http_service = StreamableHttpService::new(
        || XbergMcp::new().map_err(|e| std::io::Error::other(e.to_string())),
        LocalSessionManager::default().into(),
        build_streamable_http_config(extra_allowed_hosts),
    );

    let router = Router::new().nest_service("/mcp", http_service);

    let addr: SocketAddr = format!("{}:{}", host.as_ref(), port)
        .parse()
        .map_err(|e| format!("Invalid address: {}", e))?;

    #[cfg(feature = "api")]
    tracing::info!("Starting MCP HTTP server on http://{}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, router)
        .with_graceful_shutdown(mcp_shutdown_signal())
        .await?;

    Ok(())
}

/// Start MCP HTTP server with custom extraction config.
///
/// This variant allows specifying a custom extraction configuration
/// while using HTTP Stream transport.
///
/// # Arguments
///
/// * `host` - Host to bind to (e.g., "127.0.0.1" or "0.0.0.0")
/// * `port` - Port number (e.g., 8001)
/// * `config` - Custom extraction configuration
/// * `extra_allowed_hosts` - Additional `Host` header values to accept, on top of rmcp's
///   loopback-only default (`localhost`, `127.0.0.1`, `::1`). Needed when the server runs
///   behind a reverse proxy or ingress that forwards a different hostname. Pass an empty
///   slice to keep the default loopback-only behavior.
///
/// # Example
///
/// ```no_run
/// use xberg::mcp::start_mcp_server_http_with_config;
/// use xberg::ExtractionConfig;
///
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
///     let config = ExtractionConfig::default();
///     start_mcp_server_http_with_config("127.0.0.1", 8001, config, &[]).await?;
///     Ok(())
/// }
/// ```
#[cfg(feature = "mcp-http")]
#[cfg_attr(alef, alef(skip))]
pub async fn start_mcp_server_http_with_config(
    host: impl AsRef<str>,
    port: u16,
    config: ExtractionConfig,
    extra_allowed_hosts: &[String],
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use axum::Router;
    use std::net::SocketAddr;

    let http_service = StreamableHttpService::new(
        move || Ok(XbergMcp::with_config(config.clone())),
        LocalSessionManager::default().into(),
        build_streamable_http_config(extra_allowed_hosts),
    );

    let router = Router::new().nest_service("/mcp", http_service);

    let addr: SocketAddr = format!("{}:{}", host.as_ref(), port)
        .parse()
        .map_err(|e| format!("Invalid address: {}", e))?;

    #[cfg(feature = "api")]
    tracing::info!("Starting MCP HTTP server on http://{}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, router)
        .with_graceful_shutdown(mcp_shutdown_signal())
        .await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_tool_router_has_routes() {
        let router = XbergMcp::tool_router();
        assert!(router.has_route("extract"));
        assert!(router.has_route("extract_batch"));
        assert!(router.has_route("detect_mime_type"));
        assert!(router.has_route("list_formats"));
        assert!(router.has_route("cache_stats"));
        assert!(router.has_route("cache_clear"));
        assert!(router.has_route("get_version"));
        assert!(router.has_route("cache_manifest"));
        assert!(router.has_route("cache_warm"));
    }

    #[test]
    fn test_server_info() {
        let server = XbergMcp::with_config(ExtractionConfig::default());
        let info = server.get_info();

        assert_eq!(info.server_info.name, "xberg-mcp");
        assert_eq!(info.server_info.version, env!("CARGO_PKG_VERSION"));
        assert!(info.capabilities.tools.is_some());
    }

    #[test]
    fn test_with_config_stores_provided_config() {
        let custom_config = ExtractionConfig {
            force_ocr: true,
            use_cache: false,
            ..Default::default()
        };

        let server = XbergMcp::with_config(custom_config);

        assert!(server.default_config.force_ocr);
        assert!(!server.default_config.use_cache);
    }

    #[test]
    fn test_new_creates_server_with_default_config() {
        let server = XbergMcp::new();
        assert!(server.is_ok());
    }

    #[test]
    fn test_default_creates_server_without_panic() {
        let server = XbergMcp::default();
        let info = server.get_info();
        assert_eq!(info.server_info.name, "xberg-mcp");
    }

    #[test]
    fn test_server_info_has_correct_fields() {
        let server = XbergMcp::with_config(ExtractionConfig::default());
        let info = server.get_info();

        assert_eq!(info.server_info.name, "xberg-mcp");
        assert_eq!(
            info.server_info.title,
            Some("Xberg Document Intelligence MCP Server".to_string())
        );
        assert_eq!(info.server_info.version, env!("CARGO_PKG_VERSION"));
        assert_eq!(info.server_info.website_url, Some("https://docs.xberg.io".to_string()));
        assert!(info.instructions.is_some());
        assert!(info.capabilities.tools.is_some());
    }

    #[test]
    fn test_mcp_server_info_protocol_version() {
        let server = XbergMcp::with_config(ExtractionConfig::default());
        let info = server.get_info();

        assert_eq!(info.protocol_version, ProtocolVersion::default());
    }

    #[test]
    fn test_mcp_server_info_has_all_required_fields() {
        let server = XbergMcp::with_config(ExtractionConfig::default());
        let info = server.get_info();

        assert!(!info.server_info.name.is_empty());
        assert!(!info.server_info.version.is_empty());

        assert!(info.server_info.title.is_some());
        assert!(info.server_info.website_url.is_some());
        assert!(info.instructions.is_some());
    }

    #[test]
    fn test_mcp_server_capabilities_declares_tools() {
        let server = XbergMcp::with_config(ExtractionConfig::default());
        let info = server.get_info();

        assert!(info.capabilities.tools.is_some());
    }

    #[test]
    fn test_mcp_server_name_follows_convention() {
        let server = XbergMcp::with_config(ExtractionConfig::default());
        let info = server.get_info();

        assert_eq!(info.server_info.name, "xberg-mcp");
        assert!(!info.server_info.name.contains('_'));
        assert!(!info.server_info.name.contains(' '));
    }

    #[test]
    fn test_mcp_version_matches_cargo_version() {
        let server = XbergMcp::with_config(ExtractionConfig::default());
        let info = server.get_info();

        assert_eq!(info.server_info.version, env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn test_mcp_instructions_are_helpful() {
        let server = XbergMcp::with_config(ExtractionConfig::default());
        let info = server.get_info();

        let instructions = info.instructions.expect("Instructions should be present");

        assert!(instructions.contains("extract") || instructions.contains("Extract"));
        assert!(instructions.contains("OCR") || instructions.contains("ocr"));
        assert!(instructions.contains("document"));
    }

    #[tokio::test]
    async fn test_all_tools_are_registered() {
        let router = XbergMcp::tool_router();

        let expected_tools = vec![
            "extract",
            "extract_batch",
            "detect_mime_type",
            "list_formats",
            "cache_stats",
            "cache_clear",
            "get_version",
            "cache_manifest",
            "cache_warm",
        ];

        for tool_name in expected_tools.iter() {
            assert!(router.has_route(tool_name), "Tool '{}' should be registered", tool_name);
        }
    }

    #[tokio::test]
    async fn test_tool_count_is_correct() {
        let router = XbergMcp::tool_router();
        let tools = router.list_all();

        assert_eq!(tools.len(), 9, "Expected 9 tools, found {}", tools.len());
    }

    #[tokio::test]
    async fn test_tools_have_descriptions() {
        let router = XbergMcp::tool_router();
        let tools = router.list_all();

        for tool in tools {
            assert!(
                tool.description.is_some(),
                "Tool '{}' should have a description",
                tool.name
            );
            let desc = tool.description.as_ref().unwrap();
            assert!(!desc.is_empty(), "Tool '{}' description should not be empty", tool.name);
        }
    }

    #[tokio::test]
    async fn test_tool_annotations_reflect_behavior() {
        let router = XbergMcp::tool_router();
        let tools = router.list_all();

        let annotations_for = |name: &str| {
            tools
                .iter()
                .find(|t| t.name == name)
                .unwrap_or_else(|| panic!("tool '{name}' should exist"))
                .annotations
                .clone()
                .unwrap_or_else(|| panic!("tool '{name}' should have annotations"))
        };

        for name in [
            "detect_mime_type",
            "cache_stats",
            "list_formats",
            "get_version",
            "cache_manifest",
        ] {
            let a = annotations_for(name);
            assert_eq!(a.read_only_hint, Some(true), "{name} should be read-only");
            assert_eq!(a.idempotent_hint, Some(true), "{name} should be idempotent");
            assert_ne!(a.open_world_hint, Some(true), "{name} should be closed-world");
        }

        for name in ["extract", "extract_batch"] {
            let a = annotations_for(name);
            assert_eq!(a.read_only_hint, Some(true), "{name} should be read-only");
            assert_eq!(a.idempotent_hint, Some(true), "{name} should be idempotent");
            assert_eq!(a.open_world_hint, Some(true), "{name} may fetch URLs");
        }

        let clear = annotations_for("cache_clear");
        assert_eq!(
            clear.read_only_hint,
            Some(false),
            "cache_clear modifies the environment"
        );
        assert_eq!(clear.destructive_hint, Some(true), "cache_clear is destructive");

        let warm = annotations_for("cache_warm");
        assert_eq!(warm.read_only_hint, Some(false), "cache_warm writes the cache");
        assert_eq!(
            warm.destructive_hint,
            Some(false),
            "cache_warm is additive, not destructive"
        );
        assert_eq!(warm.open_world_hint, Some(true), "cache_warm fetches from HuggingFace");
    }

    #[tokio::test]
    async fn test_extract_tool_has_correct_schema() {
        let router = XbergMcp::tool_router();
        let tools = router.list_all();

        let extract_tool = tools
            .iter()
            .find(|t| t.name == "extract")
            .expect("extract tool should exist");

        assert!(extract_tool.description.is_some());

        assert!(!extract_tool.input_schema.is_empty());
    }

    #[tokio::test]
    async fn test_all_tools_have_input_schemas() {
        let router = XbergMcp::tool_router();
        let tools = router.list_all();

        for tool in tools {
            assert!(
                !tool.input_schema.is_empty(),
                "Tool '{}' should have an input schema with fields",
                tool.name
            );
        }
    }

    #[test]
    fn test_server_creation_with_custom_config() {
        let custom_config = ExtractionConfig {
            force_ocr: true,
            use_cache: false,
            ocr: Some(crate::OcrConfig {
                backend: "tesseract".to_string(),
                language: vec!["spa".to_string()],
                ..Default::default()
            }),
            ..Default::default()
        };

        let server = XbergMcp::with_config(custom_config.clone());

        assert_eq!(server.default_config.force_ocr, custom_config.force_ocr);
        assert_eq!(server.default_config.use_cache, custom_config.use_cache);
    }

    #[test]
    fn test_server_clone_preserves_config() {
        let custom_config = ExtractionConfig {
            force_ocr: true,
            ..Default::default()
        };

        let server1 = XbergMcp::with_config(custom_config);
        let server2 = server1.clone();

        assert_eq!(server1.default_config.force_ocr, server2.default_config.force_ocr);
    }

    #[tokio::test]
    async fn test_server_is_thread_safe() {
        let server = XbergMcp::with_config(ExtractionConfig::default());

        let server1 = server.clone();
        let server2 = server.clone();

        let handle1 = tokio::spawn(async move { server1.get_info() });

        let handle2 = tokio::spawn(async move { server2.get_info() });

        let info1 = handle1.await.unwrap();
        let info2 = handle2.await.unwrap();

        assert_eq!(info1.server_info.name, info2.server_info.name);
    }

    #[test]
    fn test_get_version_returns_version() {
        let server = XbergMcp::with_config(ExtractionConfig::default());

        let result = server.get_version(rmcp::handler::server::wrapper::Parameters(
            crate::mcp::params::EmptyParams {},
        ));

        assert!(result.is_ok());
        let call_result = result.unwrap();
        if let Some(content) = call_result.content.first() {
            match content {
                ContentBlock::Text(text) => {
                    let parsed: serde_json::Value = serde_json::from_str(&text.text).expect("Should be valid JSON");
                    assert_eq!(parsed["version"], env!("CARGO_PKG_VERSION"));
                }
                _ => panic!("Expected text content"),
            }
        } else {
            panic!("Expected content in result");
        }
        assert!(
            call_result.structured_content.is_some(),
            "get_version should have structured_content"
        );
        let sc = call_result.structured_content.unwrap();
        assert_eq!(sc["version"], env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn test_cache_manifest_returns_json() {
        let server = XbergMcp::with_config(ExtractionConfig::default());

        let result = server.cache_manifest(rmcp::handler::server::wrapper::Parameters(
            crate::mcp::params::EmptyParams {},
        ));

        assert!(result.is_ok());
        let call_result = result.unwrap();
        if let Some(content) = call_result.content.first() {
            match content {
                ContentBlock::Text(text) => {
                    let parsed: serde_json::Value = serde_json::from_str(&text.text).expect("Should be valid JSON");
                    assert!(parsed.get("xberg_version").is_some());
                    assert!(parsed.get("model_count").is_some());
                    assert!(parsed.get("models").is_some());
                }
                _ => panic!("Expected text content"),
            }
        } else {
            panic!("Expected content in result");
        }
        assert!(
            call_result.structured_content.is_some(),
            "cache_manifest should have structured_content"
        );
    }

    #[tokio::test]
    async fn test_extract_batch_empty_inputs_returns_empty_envelope() {
        let server = XbergMcp::with_config(ExtractionConfig::default());
        let params = crate::mcp::params::ExtractBatchParams {
            inputs: vec![],
            config: None,
            pdf_password: None,
            response_format: None,
        };

        let result = server
            .extract_batch(rmcp::handler::server::wrapper::Parameters(params))
            .await;
        assert!(result.is_ok());
        let result = result.unwrap();
        let structured = result.structured_content.expect("structured content should exist");
        assert_eq!(structured["summary"]["inputs"], 0);
        assert_eq!(structured["summary"]["results"], 0);
        assert_eq!(structured["summary"]["errors"], 0);
    }

    #[test]
    fn test_capabilities_declare_resources_prompts_completions() {
        let server = XbergMcp::with_config(ExtractionConfig::default());
        let info = server.get_info();
        assert!(
            info.capabilities.resources.is_some(),
            "resources capability should be declared"
        );
        assert!(
            info.capabilities.prompts.is_some(),
            "prompts capability should be declared"
        );
        assert!(
            info.capabilities.completions.is_some(),
            "completions capability should be declared"
        );
        assert!(info.capabilities.tools.is_some(), "tools capability should be declared");
    }

    #[tokio::test]
    async fn test_output_schema_present_on_structured_tools() {
        let router = XbergMcp::tool_router();
        let tools = router.list_all();
        let structured_tools = [
            "extract",
            "extract_batch",
            "detect_mime_type",
            "get_version",
            "list_formats",
            "cache_stats",
            "cache_manifest",
            "cache_clear",
            "cache_warm",
        ];
        for name in structured_tools {
            let tool = tools
                .iter()
                .find(|t| t.name == name)
                .unwrap_or_else(|| panic!("tool '{}' not found", name));
            assert!(
                tool.output_schema.is_some(),
                "tool '{}' should have output_schema",
                name
            );
        }
    }

    /// Every registered MCP tool must declare a typed output schema (#250).
    ///
    /// The test above pins a hardcoded list, so a tool added later without an
    /// `output_schema` would pass it unnoticed. This one is exhaustive over
    /// whatever the router actually exposes, and pins the exact tool set so a
    /// silent addition or removal has to be acknowledged here.
    #[tokio::test]
    async fn should_declare_output_schema_on_every_registered_tool() {
        let router = XbergMcp::tool_router();
        let tools = router.list_all();

        let mut names: Vec<String> = tools.iter().map(|tool| tool.name.to_string()).collect();
        names.sort();
        assert_eq!(
            names,
            vec![
                "cache_clear",
                "cache_manifest",
                "cache_stats",
                "cache_warm",
                "detect_mime_type",
                "extract",
                "extract_batch",
                "get_version",
                "list_formats",
            ],
            "unexpected MCP tool set"
        );

        let missing: Vec<String> = tools
            .iter()
            .filter(|tool| tool.output_schema.is_none())
            .map(|tool| tool.name.to_string())
            .collect();
        assert_eq!(
            missing,
            Vec::<String>::new(),
            "every MCP tool must declare output_schema so clients can parse structured_content"
        );
    }

    #[test]
    fn test_record_warmed_model_reports_only_confirmed_cache_dispositions() {
        let mut available = Vec::new();
        let mut downloaded = Vec::new();
        let mut already_cached = Vec::new();

        record_warmed_model(
            &mut available,
            &mut downloaded,
            &mut already_cached,
            "availability-only".to_string(),
            CacheWarmDisposition::AvailabilityConfirmed,
        );
        record_warmed_model(
            &mut available,
            &mut downloaded,
            &mut already_cached,
            "downloaded".to_string(),
            CacheWarmDisposition::Downloaded,
        );
        record_warmed_model(
            &mut available,
            &mut downloaded,
            &mut already_cached,
            "cached".to_string(),
            CacheWarmDisposition::AlreadyCached,
        );

        assert_eq!(
            available,
            vec!["availability-only", "downloaded", "cached"],
            "every successful warm operation must be reported as available"
        );
        assert_eq!(
            downloaded,
            vec!["downloaded"],
            "only a confirmed download belongs in downloaded"
        );
        assert_eq!(
            already_cached,
            vec!["cached"],
            "only a confirmed cache hit belongs in already_cached"
        );
    }

    #[test]
    fn test_list_resources_returns_expected_uris() {
        let result = crate::mcp::resources::list_resources();
        let uris: Vec<&str> = result.resources.iter().map(|r| r.uri.as_str()).collect();
        assert!(uris.contains(&"xberg://formats"), "formats resource missing");
        assert!(uris.contains(&"xberg://models"), "models resource missing");
        assert!(
            uris.contains(&"xberg://languages/ocr"),
            "ocr languages resource missing"
        );
    }

    #[test]
    fn test_read_resource_formats_roundtrip() {
        let result =
            crate::mcp::resources::read_resource("xberg://formats").expect("formats resource should be readable");
        assert!(!result.contents.is_empty());
        if let ResourceContents::TextResourceContents { text, .. } = &result.contents[0] {
            let _: serde_json::Value = serde_json::from_str(text).expect("formats should be valid JSON");
        } else {
            panic!("Expected TextResourceContents");
        }
    }

    #[test]
    fn test_list_prompts_returns_workflows() {
        let server = XbergMcp::with_config(ExtractionConfig::default());
        let prompts = server.prompt_router.list_all();
        let names: Vec<&str> = prompts.iter().map(|p| p.name.as_str()).collect();
        assert!(names.contains(&"extract_document"), "extract_document prompt missing");
        assert!(names.contains(&"extract_with_ocr"), "extract_with_ocr prompt missing");
        assert!(names.contains(&"semantic_search"), "semantic_search prompt missing");
    }

    #[test]
    fn test_complete_ocr_language_by_prefix() {
        let candidates = complete_ocr_languages("en");
        assert!(!candidates.is_empty(), "should return candidates for prefix 'en'");
        assert!(
            candidates.iter().any(|c| c == "eng"),
            "eng should be in completions for prefix 'en'"
        );
    }

    #[test]
    fn test_complete_embedding_presets() {
        let candidates = complete_embedding_presets("b");
        assert_eq!(candidates, vec!["balanced"]);
    }

    #[test]
    fn test_complete_chunker_types_empty_prefix_returns_all() {
        let candidates = complete_chunker_types("");
        assert_eq!(candidates.len(), 4);
    }

    #[test]
    fn test_complete_output_formats() {
        let candidates = complete_output_formats("j");
        assert_eq!(candidates, vec!["json"]);
    }

    #[cfg(feature = "mcp-http")]
    #[test]
    fn test_build_streamable_http_config_empty_preserves_rmcp_default() {
        let config = build_streamable_http_config(&[]);
        let default_config = StreamableHttpServerConfig::default();
        assert_eq!(
            config.allowed_hosts, default_config.allowed_hosts,
            "empty extra hosts must leave rmcp's default untouched"
        );
    }

    #[cfg(feature = "mcp-http")]
    #[test]
    fn test_build_streamable_http_config_extends_default_without_replacing_it() {
        let default_hosts = StreamableHttpServerConfig::default().allowed_hosts;
        let config = build_streamable_http_config(&["proxy.example.com".to_string()]);

        for host in &default_hosts {
            assert!(
                config.allowed_hosts.contains(host),
                "loopback host '{host}' must still be present after extending"
            );
        }
        assert!(
            config.allowed_hosts.contains(&"proxy.example.com".to_string()),
            "supplied host must be added"
        );
    }

    #[cfg(feature = "mcp-http")]
    #[test]
    fn test_build_streamable_http_config_trims_and_deduplicates_hosts() {
        let config = build_streamable_http_config(&[" proxy.example.com ".to_string(), "localhost".to_string()]);

        let occurrences = config.allowed_hosts.iter().filter(|h| *h == "localhost").count();
        assert_eq!(
            occurrences, 1,
            "duplicate of an existing default host must not be added again"
        );
        assert!(config.allowed_hosts.contains(&"proxy.example.com".to_string()));
    }

    #[test]
    fn test_capabilities_declare_tasks_extension() {
        let server = XbergMcp::with_config(ExtractionConfig::default());
        let info = server.get_info();
        assert!(
            info.capabilities.supports_tasks(),
            "SEP-2663 tasks extension capability should be declared"
        );
    }

    #[test]
    fn test_spawn_task_for_tool_rejects_non_task_eligible_name() {
        let server = XbergMcp::with_config(ExtractionConfig::default());
        let request = CallToolRequestParams::new("get_version");
        let result = server.spawn_task_for_tool(request);
        assert!(
            result.is_err(),
            "non-task-eligible tool names must not materialize a task"
        );
    }

    #[test]
    fn test_spawn_task_for_tool_extract_rejects_invalid_params_synchronously() {
        let server = XbergMcp::with_config(ExtractionConfig::default());
        // No arguments at all: ExtractParams.input is required, so this must
        // fail during deserialization before any task is spawned.
        let request = CallToolRequestParams::new("extract");
        let result = server.spawn_task_for_tool(request);
        assert!(
            result.is_err(),
            "missing required 'input' field should be rejected before spawning a task"
        );
    }

    #[tokio::test]
    async fn test_spawn_task_for_tool_extract_task_reaches_terminal_state() {
        let server = XbergMcp::with_config(ExtractionConfig::default());
        let mut arguments = rmcp::model::JsonObject::new();
        arguments.insert(
            "input".to_string(),
            serde_json::json!({"kind": "uri", "uri": "/nonexistent/xberg-mcp-task-test.bin"}),
        );
        let request = CallToolRequestParams::new("extract").with_arguments(arguments);

        let response = server
            .spawn_task_for_tool(request)
            .expect("valid extract params should materialize a task");
        let task_id = match response {
            CallToolResponse::Task(create) => create.task.task_id,
            other => panic!("expected CreateTaskResult, got {other:?}"),
        };

        let mut detailed = server
            .tasks
            .get_task(&task_id)
            .expect("task should exist right after spawn");
        for _ in 0..200 {
            if detailed.status().is_terminal() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            detailed = server
                .tasks
                .get_task(&task_id)
                .expect("task should still be tracked while polling");
        }

        assert_eq!(
            detailed.status(),
            TaskStatus::Failed,
            "extraction from a nonexistent path should settle as a failed task, not hang"
        );
    }
}
