//! API request handlers.

use axum::body::to_bytes;
use axum::extract::{FromRequest, Multipart, Request};
use axum::http::{HeaderMap, StatusCode, header};
use axum::{Json, extract::State, response::IntoResponse};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use bytes::Bytes;

use crate::cache;
use crate::core::config::{ExtractInput, ExtractInputKind, ExtractionResult};

use std::sync::Arc;

use super::{
    error::{ApiError, JsonApi, MultipartApi},
    types::{
        ApiState, AsyncJobResponse, CacheClearResponse, CacheStatsResponse, DetectResponse, HealthResponse,
        InfoResponse, JobStatusResponse, ManifestEntryResponse, ManifestResponse, VersionResponse, WarmRequest,
        WarmResponse,
    },
};

/// Unified extraction input accepted by `/extract` and `/extract-async`.
#[derive(Debug, Clone)]
enum ApiExtractInput {
    Bytes {
        data: Bytes,
        mime_type: String,
        file_name: Option<String>,
    },
    Uri {
        uri: String,
        mime_type: Option<String>,
    },
}

impl ApiExtractInput {
    fn into_core_input(self) -> ExtractInput {
        match self {
            Self::Bytes {
                data,
                mime_type,
                file_name,
            } => ExtractInput::from_bytes(data.to_vec(), mime_type, file_name),
            Self::Uri { uri, mime_type } => ExtractInput {
                kind: ExtractInputKind::Uri,
                uri: Some(uri),
                mime_type,
                ..Default::default()
            },
        }
    }
}

#[derive(Debug)]
pub(crate) struct UnifiedExtractRequest {
    inputs: Vec<ApiExtractInput>,
    config: Option<crate::core::config::ExtractionConfig>,
    output_format: Option<crate::core::config::OutputFormat>,
    pdf_passwords: Vec<String>,
    use_toon: bool,
}

#[derive(Debug, serde::Deserialize)]
struct JsonUnifiedExtractRequest {
    inputs: Vec<JsonExtractInput>,
    #[serde(default)]
    config: Option<crate::core::config::ExtractionConfig>,
    #[serde(default)]
    format: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(untagged)]
enum JsonExtractInput {
    Uri(String),
    Object(JsonExtractInputObject),
}

#[derive(Debug, serde::Deserialize)]
struct JsonExtractInputObject {
    #[serde(default)]
    kind: Option<String>,
    #[serde(default, rename = "type")]
    input_type: Option<String>,
    #[serde(default)]
    uri: Option<String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    data: Option<String>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    mime_type: Option<String>,
    #[serde(default)]
    filename: Option<String>,
}

impl<S> FromRequest<S> for UnifiedExtractRequest
where
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        let content_type = req
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("");

        if content_type.starts_with("multipart/form-data") {
            parse_multipart_extract_request(req, state).await
        } else if is_json_content_type(content_type) {
            parse_json_extract_request(req).await
        } else {
            Err(ApiError::new(
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                crate::error::XbergError::validation(
                    "Expected Content-Type application/json or multipart/form-data for extraction",
                ),
            ))
        }
    }
}

fn is_json_content_type(content_type: &str) -> bool {
    let lower = content_type.to_ascii_lowercase();
    lower.starts_with("application/json") || lower.contains("+json")
}

async fn parse_json_extract_request(req: Request) -> Result<UnifiedExtractRequest, ApiError> {
    let bytes = to_bytes(req.into_body(), usize::MAX).await.map_err(|_| {
        ApiError::new(
            StatusCode::BAD_REQUEST,
            crate::error::XbergError::Other("Failed to read request body".to_string()),
        )
    })?;
    let body: JsonUnifiedExtractRequest = serde_json::from_slice(&bytes).map_err(|e| {
        ApiError::new(
            StatusCode::BAD_REQUEST,
            crate::error::XbergError::validation(format!("Invalid extraction request JSON: {e}")),
        )
    })?;

    let inputs = body
        .inputs
        .into_iter()
        .map(json_input_to_api_input)
        .collect::<Result<Vec<_>, _>>()?;

    Ok(UnifiedExtractRequest {
        inputs,
        config: body.config,
        output_format: None,
        pdf_passwords: Vec::new(),
        use_toon: body
            .format
            .as_deref()
            .is_some_and(|format| format.eq_ignore_ascii_case("toon")),
    })
}

async fn parse_multipart_extract_request<S>(req: Request, state: &S) -> Result<UnifiedExtractRequest, ApiError>
where
    S: Send + Sync,
{
    let mut multipart = Multipart::from_request(req, state)
        .await
        .map_err(|rejection| ApiError {
            status: StatusCode::BAD_REQUEST,
            body: super::types::ErrorResponse {
                error_type: "MultipartError".to_string(),
                message: rejection.body_text(),
                traceback: None,
                status_code: StatusCode::BAD_REQUEST.as_u16(),
            },
        })?;

    let mut inputs = Vec::new();
    let mut config: Option<crate::core::config::ExtractionConfig> = None;
    let mut output_format = None;
    let mut pdf_passwords = Vec::new();
    let mut use_toon = false;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| ApiError::validation(crate::error::XbergError::validation(e.to_string())))?
    {
        let field_name = field.name().unwrap_or("").to_string();

        match field_name.as_str() {
            "file" | "files" => {
                let file_name = field.file_name().map(|s| s.to_string());
                let content_type = field.content_type().map(|s| s.to_string());
                let data = field
                    .bytes()
                    .await
                    .map_err(|e| ApiError::validation(crate::error::XbergError::validation(e.to_string())))?;
                let mime_type = resolve_multipart_mime(content_type, file_name.as_deref());

                inputs.push(ApiExtractInput::Bytes {
                    data,
                    mime_type,
                    file_name,
                });
            }
            "urls" => {
                let urls = field
                    .text()
                    .await
                    .map_err(|e| ApiError::validation(crate::error::XbergError::validation(e.to_string())))?;
                inputs.extend(parse_urls_field(&urls)?);
            }
            "inputs" => {
                let raw_inputs = field
                    .text()
                    .await
                    .map_err(|e| ApiError::validation(crate::error::XbergError::validation(e.to_string())))?;
                inputs.extend(parse_inputs_field(&raw_inputs)?);
            }
            "config" => {
                let config_str = field
                    .text()
                    .await
                    .map_err(|e| ApiError::validation(crate::error::XbergError::validation(e.to_string())))?;

                config = Some(serde_json::from_str(&config_str).map_err(|e| {
                    ApiError::validation(crate::error::XbergError::validation(format!(
                        "Invalid extraction configuration: {}",
                        e
                    )))
                })?);
            }
            "output_format" => {
                let format_str = field
                    .text()
                    .await
                    .map_err(|e| ApiError::validation(crate::error::XbergError::validation(e.to_string())))?;
                output_format = Some(parse_output_format(&format_str)?);
            }
            "pdf_password" => {
                let pwd = field
                    .text()
                    .await
                    .map_err(|e| ApiError::validation(crate::error::XbergError::validation(e.to_string())))?;
                pdf_passwords.push(pwd);
            }
            "format" => {
                let format_str = field
                    .text()
                    .await
                    .map_err(|e| ApiError::validation(crate::error::XbergError::validation(e.to_string())))?;
                if format_str.eq_ignore_ascii_case("toon") {
                    use_toon = true;
                }
            }
            _ => {}
        }
    }

    Ok(UnifiedExtractRequest {
        inputs,
        config,
        output_format,
        pdf_passwords,
        use_toon,
    })
}

fn resolve_multipart_mime(content_type: Option<String>, file_name: Option<&str>) -> String {
    let mut mime_type = content_type.unwrap_or_else(|| crate::core::mime::OCTET_STREAM_MIME_TYPE.to_string());
    if mime_type == crate::core::mime::OCTET_STREAM_MIME_TYPE
        && let Some(name) = file_name
        && let Ok(detected) = crate::core::mime::detect_mime_type(name, false)
    {
        mime_type = detected;
    }
    mime_type
}

fn parse_urls_field(raw: &str) -> Result<Vec<ApiExtractInput>, ApiError> {
    let value: serde_json::Value = serde_json::from_str(raw).map_err(|e| {
        ApiError::validation(crate::error::XbergError::validation(format!(
            "Invalid urls field JSON: {e}"
        )))
    })?;

    match value {
        serde_json::Value::String(uri) => Ok(vec![ApiExtractInput::Uri { uri, mime_type: None }]),
        serde_json::Value::Array(values) => values
            .into_iter()
            .map(|value| match value {
                serde_json::Value::String(uri) => Ok(ApiExtractInput::Uri { uri, mime_type: None }),
                _ => Err(ApiError::validation(crate::error::XbergError::validation(
                    "urls field must be a JSON string or array of strings",
                ))),
            })
            .collect(),
        _ => Err(ApiError::validation(crate::error::XbergError::validation(
            "urls field must be a JSON string or array of strings",
        ))),
    }
}

fn parse_inputs_field(raw: &str) -> Result<Vec<ApiExtractInput>, ApiError> {
    let value: serde_json::Value = serde_json::from_str(raw).map_err(|e| {
        ApiError::validation(crate::error::XbergError::validation(format!(
            "Invalid inputs field JSON: {e}"
        )))
    })?;
    let inputs: Vec<JsonExtractInput> = serde_json::from_value(match value {
        serde_json::Value::Array(_) => value,
        other => serde_json::Value::Array(vec![other]),
    })
    .map_err(|e| {
        ApiError::validation(crate::error::XbergError::validation(format!(
            "Invalid inputs field shape: {e}"
        )))
    })?;

    inputs.into_iter().map(json_input_to_api_input).collect()
}

fn json_input_to_api_input(input: JsonExtractInput) -> Result<ApiExtractInput, ApiError> {
    match input {
        JsonExtractInput::Uri(uri) => Ok(ApiExtractInput::Uri { uri, mime_type: None }),
        JsonExtractInput::Object(object) => object_to_api_input(object),
    }
}

fn object_to_api_input(object: JsonExtractInputObject) -> Result<ApiExtractInput, ApiError> {
    let kind = object.kind.or(object.input_type).map(|kind| kind.to_ascii_lowercase());

    if matches!(kind.as_deref(), Some("bytes") | Some("base64")) || object.data.is_some() {
        let data = object.data.ok_or_else(|| {
            ApiError::validation(crate::error::XbergError::validation(
                "bytes input requires a base64 data field",
            ))
        })?;
        let decoded = STANDARD.decode(data).map_err(|e| {
            ApiError::validation(crate::error::XbergError::validation(format!(
                "Invalid base64 data field: {e}"
            )))
        })?;
        return Ok(ApiExtractInput::Bytes {
            data: Bytes::from(decoded),
            mime_type: object
                .mime_type
                .unwrap_or_else(|| crate::core::mime::OCTET_STREAM_MIME_TYPE.to_string()),
            file_name: object.filename,
        });
    }

    if matches!(kind.as_deref(), Some("text")) || object.text.is_some() {
        let text = object.text.ok_or_else(|| {
            ApiError::validation(crate::error::XbergError::validation("text input requires a text field"))
        })?;
        return Ok(ApiExtractInput::Bytes {
            data: Bytes::from(text),
            mime_type: object.mime_type.unwrap_or_else(|| "text/plain".to_string()),
            file_name: object.filename,
        });
    }

    if let Some(uri) = object.uri.or(object.url).or(object.path) {
        return Ok(ApiExtractInput::Uri {
            uri,
            mime_type: object.mime_type,
        });
    }

    Err(ApiError::validation(crate::error::XbergError::validation(
        "input must include one of uri, url, path, data, or text",
    )))
}

fn parse_output_format(format_str: &str) -> Result<crate::core::config::OutputFormat, ApiError> {
    let output_format = match format_str.to_lowercase().as_str() {
        "plain" => crate::core::config::OutputFormat::Plain,
        "markdown" => crate::core::config::OutputFormat::Markdown,
        "djot" => crate::core::config::OutputFormat::Djot,
        "html" => crate::core::config::OutputFormat::Html,
        _ => {
            return Err(ApiError::validation(crate::error::XbergError::validation(format!(
                "Invalid output_format: '{}'. Valid values: 'plain', 'markdown', 'djot', 'html'",
                format_str
            ))));
        }
    };
    Ok(output_format)
}

fn apply_multipart_config_fields(
    config: &mut crate::core::config::ExtractionConfig,
    output_format: Option<crate::core::config::OutputFormat>,
    pdf_passwords: Vec<String>,
) {
    if let Some(output_format) = output_format {
        config.output_format = output_format;
    }
    #[cfg(feature = "pdf")]
    {
        if !pdf_passwords.is_empty() {
            let pdf_opts = config.pdf_options.get_or_insert_with(Default::default);
            pdf_opts.passwords.get_or_insert_with(Vec::new).extend(pdf_passwords);
        }
    }
    #[cfg(not(feature = "pdf"))]
    let _ = pdf_passwords;
}

/// Health check endpoint handler.
///
/// GET /health
#[utoipa::path(
    get,
    path = "/health",
    tag = "health",
    responses(
        (status = 200, description = "Service is healthy", body = HealthResponse),
    )
)]
#[cfg_attr(feature = "otel", tracing::instrument(name = "api.health"))]
pub(crate) async fn health_handler() -> Json<HealthResponse> {
    let plugin_status = crate::plugins::startup_validation::PluginHealthStatus::check();

    Json(HealthResponse {
        status: "healthy".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        plugins: Some(super::types::PluginStatus {
            ocr_backends_count: plugin_status.ocr_backends_count,
            ocr_backends: plugin_status.ocr_backends,
            extractors_count: plugin_status.extractors_count,
            post_processors_count: plugin_status.post_processors_count,
        }),
    })
}

/// Server info endpoint handler.
///
/// GET /info
#[utoipa::path(
    get,
    path = "/info",
    tag = "health",
    responses(
        (status = 200, description = "Server information", body = InfoResponse),
    )
)]
#[cfg_attr(feature = "otel", tracing::instrument(name = "api.info"))]
pub(crate) async fn info_handler() -> Json<InfoResponse> {
    Json(InfoResponse {
        version: env!("CARGO_PKG_VERSION").to_string(),
        rust_backend: true,
    })
}

/// Check whether TOON wire format was requested via the `Accept` header.
fn wants_toon(headers: &HeaderMap) -> bool {
    headers
        .get(axum::http::header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.contains("application/toon"))
}

/// Serialize extraction results as a TOON response.
fn toon_response(results: &ExtractionResult) -> Result<axum::response::Response<axum::body::Body>, ApiError> {
    let body = serde_toon::to_string(results).map_err(|e| {
        ApiError::internal(crate::error::XbergError::Other(format!(
            "Failed to serialize response to TOON: {}",
            e
        )))
    })?;
    Ok(axum::response::Response::builder()
        .header(axum::http::header::CONTENT_TYPE, "application/toon")
        .body(axum::body::Body::from(body))
        .expect("valid response"))
}

/// Extract endpoint handler.
///
/// POST /extract
///
/// Accepts multipart form data with:
/// - `files`: One or more files to extract
/// - `config` (optional): JSON extraction configuration (overrides server defaults)
/// - `format` (optional): Wire format for the response (`json` or `toon`, default: `json`).
///   Alternatively, set the `Accept: application/toon` header.
///
/// Returns an `ExtractionResult` envelope with extraction results and summary counts.
///
/// # Size Limits
///
/// Request body size limits are enforced at the router layer via `DefaultBodyLimit` and `RequestBodyLimitLayer`.
/// Default limits:
/// - Total request body: 100 MB (all files + form data combined)
/// - Individual multipart fields: 100 MB (controlled by Axum's `DefaultBodyLimit`)
///
/// Limits can be configured via environment variables or programmatically when creating the router.
/// If a request exceeds the size limit, it will be rejected with HTTP 413 (Payload Too Large).
///
/// The server's default config (loaded from xberg.toml/yaml/json via discovery)
/// is used as the base, and any per-request config overrides those defaults.
#[utoipa::path(
    post,
    path = "/extract",
    tag = "extraction",
    request_body(content_type = "multipart/form-data"),
    responses(
        (status = 200, description = "Extraction successful", body = crate::core::config::ExtractionResult),
        (status = 400, description = "Bad request", body = crate::api::types::ErrorResponse),
        (status = 413, description = "Payload too large", body = crate::api::types::ErrorResponse),
        (status = 500, description = "Internal server error", body = crate::api::types::ErrorResponse),
    )
)]
#[cfg_attr(
    feature = "otel",
    tracing::instrument(
        name = "api.extract",
        skip(state, headers, request),
        fields(files_count = tracing::field::Empty)
    )
)]
pub(crate) async fn extract_handler(
    State(state): State<ApiState>,
    headers: HeaderMap,
    request: UnifiedExtractRequest,
) -> Result<axum::response::Response<axum::body::Body>, ApiError> {
    let use_toon = wants_toon(&headers) || request.use_toon;

    #[cfg(feature = "otel")]
    tracing::Span::current().record("files_count", request.inputs.len());

    let mut final_config = request.config.unwrap_or_else(|| (*state.default_config).clone());
    apply_multipart_config_fields(&mut final_config, request.output_format, request.pdf_passwords);
    enforce_api_uri_policy(&request.inputs)?;
    apply_api_uri_policy_to_config(&mut final_config);
    let results = extract_unified_inputs(request.inputs, final_config).await?;

    if use_toon {
        toon_response(&results)
    } else {
        Ok(Json(results).into_response())
    }
}

async fn extract_unified_inputs(
    inputs: Vec<ApiExtractInput>,
    config: crate::core::config::ExtractionConfig,
) -> Result<ExtractionResult, ApiError> {
    if inputs.is_empty() {
        return Err(ApiError::validation(crate::error::XbergError::validation(
            "No inputs provided for extraction",
        )));
    }

    let inputs = inputs.into_iter().map(ApiExtractInput::into_core_input).collect();
    crate::extract_batch(inputs, &config).await.map_err(ApiError::from)
}

fn enforce_api_uri_policy(inputs: &[ApiExtractInput]) -> Result<(), ApiError> {
    if api_allows_local_uri_inputs() {
        return Ok(());
    }
    for input in inputs {
        if let ApiExtractInput::Uri { uri, .. } = input
            && !is_remote_uri(uri)
        {
            return Err(ApiError::validation(crate::error::XbergError::validation(
                "Local path and file:// URI extraction are disabled for the HTTP API. Set XBERG_API_ALLOW_LOCAL_URI_INPUTS=1 to enable server-side local URI access.",
            )));
        }
    }
    Ok(())
}

fn apply_api_uri_policy_to_config(config: &mut crate::core::config::ExtractionConfig) {
    if api_allows_local_uri_inputs() {
        return;
    }
    config.url.allow_local_file_inputs = false;
    config.url.allow_file_uris = false;
}

fn api_allows_local_uri_inputs() -> bool {
    std::env::var("XBERG_API_ALLOW_LOCAL_URI_INPUTS")
        .map(|value| matches!(value.to_ascii_lowercase().as_str(), "1" | "true" | "yes"))
        .unwrap_or(false)
}

fn is_remote_uri(uri: &str) -> bool {
    uri.starts_with("http://") || uri.starts_with("https://")
}

/// Formats endpoint handler.
///
/// GET /formats
///
/// Returns all supported file extensions and their corresponding MIME types.
#[utoipa::path(
    get,
    path = "/formats",
    tag = "health",
    responses(
        (status = 200, description = "Supported formats", body = Vec<crate::SupportedFormat>),
    )
)]
#[cfg_attr(feature = "otel", tracing::instrument(name = "api.formats"))]
pub(crate) async fn formats_handler() -> Json<Vec<crate::SupportedFormat>> {
    Json(crate::core::mime::list_supported_formats())
}

/// Cache stats endpoint handler.
///
/// GET /cache/stats
///
/// # Errors
///
/// Returns `ApiError::Internal` if:
/// - Current directory cannot be determined
/// - Cache directory path contains non-UTF8 characters
/// - Cache metadata retrieval fails
#[utoipa::path(
    get,
    path = "/cache/stats",
    tag = "cache",
    responses(
        (status = 200, description = "Cache statistics", body = CacheStatsResponse),
        (status = 500, description = "Internal server error", body = crate::api::types::ErrorResponse),
    )
)]
#[cfg_attr(feature = "otel", tracing::instrument(name = "api.cache_stats"))]
pub(crate) async fn cache_stats_handler() -> Result<Json<CacheStatsResponse>, ApiError> {
    let cache_dir = crate::cache_dir::resolve_cache_base();

    let cache_dir_str = cache_dir.to_str().ok_or_else(|| {
        ApiError::internal(crate::error::XbergError::Other(format!(
            "Cache directory path contains non-UTF8 characters: {}",
            cache_dir.display()
        )))
    })?;

    let stats = cache::get_cache_metadata(cache_dir_str).map_err(ApiError::internal)?;

    Ok(Json(CacheStatsResponse {
        directory: cache_dir.to_string_lossy().to_string(),
        total_files: stats.total_files,
        total_size_mb: stats.total_size_mb,
        available_space_mb: stats.available_space_mb,
        oldest_file_age_days: stats.oldest_file_age_days,
        newest_file_age_days: stats.newest_file_age_days,
    }))
}

/// Clear the Xberg-managed cache. Shared Hugging Face Hub cache files are excluded.
///
/// DELETE /cache/clear
///
/// # Errors
///
/// Returns `ApiError::Internal` if:
/// - Current directory cannot be determined
/// - Cache directory path contains non-UTF8 characters
/// - Cache clearing operation fails
#[utoipa::path(
    delete,
    path = "/cache/clear",
    tag = "cache",
    responses(
        (status = 200, description = "Xberg-managed cache cleared; shared Hugging Face cache excluded", body = CacheClearResponse),
        (status = 500, description = "Internal server error", body = crate::api::types::ErrorResponse),
    )
)]
#[cfg_attr(feature = "otel", tracing::instrument(name = "api.cache_clear"))]
pub(crate) async fn cache_clear_handler() -> Result<Json<CacheClearResponse>, ApiError> {
    let cache_dir = crate::cache_dir::resolve_cache_base();

    let cache_dir_str = cache_dir.to_str().ok_or_else(|| {
        ApiError::internal(crate::error::XbergError::Other(format!(
            "Cache directory path contains non-UTF8 characters: {}",
            cache_dir.display()
        )))
    })?;

    let (removed_files, freed_mb) = cache::clear_cache_directory(cache_dir_str).map_err(ApiError::internal)?;

    Ok(Json(CacheClearResponse {
        directory: cache_dir.to_string_lossy().to_string(),
        removed_files,
        freed_mb,
    }))
}

/// Version endpoint handler.
///
/// GET /version
///
/// Returns the current xberg version.
#[utoipa::path(
    get,
    path = "/version",
    tag = "health",
    responses(
        (status = 200, description = "Version information", body = VersionResponse),
    )
)]
#[cfg_attr(feature = "otel", tracing::instrument(name = "api.version"))]
pub(crate) async fn version_handler() -> Json<VersionResponse> {
    Json(VersionResponse {
        version: env!("CARGO_PKG_VERSION").to_string(),
    })
}

/// MIME type detection endpoint handler.
///
/// POST /detect
///
/// Accepts multipart form data with a single file and returns its detected MIME type.
///
/// # Errors
///
/// Returns `ApiError::Validation` if no file is provided.
/// Returns `ApiError::Internal` if MIME type detection fails.
#[utoipa::path(
    post,
    path = "/detect",
    tag = "extraction",
    request_body(content_type = "multipart/form-data"),
    responses(
        (status = 200, description = "MIME type detected", body = DetectResponse),
        (status = 400, description = "Bad request - no file provided", body = crate::api::types::ErrorResponse),
        (status = 500, description = "Internal server error", body = crate::api::types::ErrorResponse),
    )
)]
#[cfg_attr(feature = "otel", tracing::instrument(name = "api.detect", skip(multipart)))]
pub(crate) async fn detect_handler(
    MultipartApi(mut multipart): MultipartApi,
) -> Result<Json<DetectResponse>, ApiError> {
    let mut file_data: Option<(Vec<u8>, Option<String>)> = None;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| ApiError::validation(crate::error::XbergError::validation(e.to_string())))?
    {
        let field_name = field.name().unwrap_or("").to_string();

        if field_name == "file" || field_name == "files" {
            let file_name = field.file_name().map(|s| s.to_string());
            let data = field
                .bytes()
                .await
                .map_err(|e| ApiError::validation(crate::error::XbergError::validation(e.to_string())))?;
            file_data = Some((data.to_vec(), file_name));
            break;
        }
    }

    let (data, file_name) = file_data.ok_or_else(|| {
        ApiError::validation(crate::error::XbergError::validation(
            "No file provided for MIME type detection. Upload a file with field name 'file' or 'files'.",
        ))
    })?;

    let mime_type = crate::core::mime::detect_mime_type_from_bytes(&data).or_else(|_| {
        if let Some(ref name) = file_name {
            crate::core::mime::detect_mime_type(name, false)
        } else {
            Err(crate::error::XbergError::Other(
                "Could not detect MIME type from file content or filename".to_string(),
            ))
        }
    })?;

    Ok(Json(DetectResponse {
        mime_type,
        filename: file_name,
    }))
}

/// Model manifest endpoint handler.
///
/// GET /cache/manifest
///
/// Returns the expected model files with checksums and sizes.
#[utoipa::path(
    get,
    path = "/cache/manifest",
    tag = "cache",
    responses(
        (status = 200, description = "Model manifest", body = ManifestResponse),
    )
)]
#[cfg_attr(feature = "otel", tracing::instrument(name = "api.cache_manifest"))]
pub(crate) async fn cache_manifest_handler() -> Json<ManifestResponse> {
    #[allow(unused_mut)]
    let mut models: Vec<ManifestEntryResponse> = Vec::new();

    #[cfg(feature = "paddle-ocr")]
    {
        models.extend(
            crate::paddle_ocr::ModelManager::manifest()
                .into_iter()
                .map(|e| ManifestEntryResponse {
                    relative_path: e.relative_path,
                    sha256: e.sha256,
                    size_bytes: e.size_bytes,
                    source_url: e.source_url,
                }),
        );
    }

    #[cfg(feature = "layout-detection")]
    {
        models.extend(
            crate::layout::LayoutModelManager::manifest()
                .into_iter()
                .map(|e| ManifestEntryResponse {
                    relative_path: e.relative_path,
                    sha256: e.sha256,
                    size_bytes: e.size_bytes,
                    source_url: e.source_url,
                }),
        );
    }

    #[cfg(feature = "ner-onnx")]
    {
        models.extend(crate::text::ner::manifest().into_iter().map(|e| ManifestEntryResponse {
            relative_path: e.relative_path,
            sha256: e.sha256,
            size_bytes: e.size_bytes,
            source_url: e.source_url,
        }));
    }

    let total_size_bytes: u64 = models.iter().map(|e| e.size_bytes).sum();
    let model_count = models.len();

    Json(ManifestResponse {
        xberg_version: env!("CARGO_PKG_VERSION").to_string(),
        total_size_bytes,
        model_count,
        models,
    })
}

/// Cache warm endpoint handler.
///
/// POST /cache/warm
///
/// Eagerly downloads required models. Hugging Face artifacts remain in the
/// standard HF cache selected by `HF_HUB_CACHE`, `HF_HOME`, or platform defaults.
/// Optionally downloads embedding models when the `embeddings` feature is enabled.
///
/// # Errors
///
/// Returns `ApiError::Internal` if model downloading fails.
/// Returns `ApiError::Validation` if an unknown embedding preset is requested
/// or a requested model-warming feature is not enabled.
#[utoipa::path(
    post,
    path = "/cache/warm",
    tag = "cache",
    request_body = WarmRequest,
    responses(
        (status = 200, description = "Models warmed", body = WarmResponse),
        (status = 400, description = "Bad request - unknown or empty model name, or requested warmer feature is unavailable", body = crate::api::types::ErrorResponse),
        (status = 422, description = "Unprocessable entity - invalid JSON body", body = crate::api::types::ErrorResponse),
        (status = 500, description = "Internal server error", body = crate::api::types::ErrorResponse),
        (status = 502, description = "Bad gateway - upstream model download failed", body = crate::api::types::ErrorResponse),
    )
)]
#[cfg_attr(feature = "otel", tracing::instrument(name = "api.cache_warm", skip(request)))]
pub(crate) async fn cache_warm_handler(JsonApi(request): JsonApi<WarmRequest>) -> Result<Json<WarmResponse>, ApiError> {
    if let Some(ref name) = request.embedding_model
        && name.trim().is_empty()
    {
        return Err(ApiError::validation(crate::error::XbergError::validation(
            "Field 'embedding_model' must not be empty. Omit the field or provide a valid preset name.",
        )));
    }
    if let Some(ref name) = request.ner_model
        && name.trim().is_empty()
    {
        return Err(ApiError::validation(crate::error::XbergError::validation(
            "Field 'ner_model' must not be empty. Omit the field or provide a valid model name.",
        )));
    }

    let cache_base = resolve_cache_base();

    #[allow(unused_mut)]
    let mut downloaded: Vec<String> = Vec::new();
    #[allow(unused_mut)]
    let mut already_cached: Vec<String> = Vec::new();

    #[cfg(feature = "paddle-ocr")]
    {
        let paddle_dir = cache_base.join("paddle-ocr");
        let manager = crate::paddle_ocr::ModelManager::new(paddle_dir);

        manager.ensure_all_models().map_err(ApiError::bad_gateway)?;
        downloaded.push("paddle-ocr v2 (server+mobile det, cls, doc_ori, unified+per-script rec)".to_string());
    }

    #[cfg(feature = "layout-detection")]
    {
        let layout_dir = cache_base.join("layout");
        let manager = crate::layout::LayoutModelManager::new(Some(layout_dir));

        let was_cached = manager.is_rtdetr_cached() && manager.is_tatr_cached();

        if was_cached {
            already_cached.push("layout (rtdetr, tatr)".to_string());
        } else {
            manager.ensure_all_models().map_err(|e| {
                ApiError::bad_gateway(crate::error::XbergError::Other(format!(
                    "Failed to download layout models: {}",
                    e
                )))
            })?;
            downloaded.push("layout (rtdetr, tatr)".to_string());
        }
    }

    #[cfg(feature = "embeddings")]
    {
        let embeddings_dir = cache_base.join("embeddings");
        let presets_to_warm: Vec<crate::EmbeddingPreset> = if request.all_embeddings {
            crate::embeddings::EMBEDDING_PRESETS.clone()
        } else if let Some(ref name) = request.embedding_model {
            match crate::embeddings::get_preset(name) {
                Some(preset) => vec![preset],
                None => {
                    let available: Vec<String> = crate::embeddings::list_presets();
                    return Err(ApiError::validation(crate::error::XbergError::validation(format!(
                        "Unknown embedding preset '{}'. Available: {}",
                        name,
                        available.join(", ")
                    ))));
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
                ApiError::bad_gateway(crate::error::XbergError::Other(format!(
                    "Failed to download embedding model '{}': {}",
                    preset.name, e
                )))
            })?;
            downloaded.push(label);
        }
    }

    #[cfg(not(feature = "embeddings"))]
    {
        if request.all_embeddings || request.embedding_model.is_some() {
            return Err(ApiError::validation(crate::error::XbergError::validation(
                "Embedding model warming requires the 'embeddings' feature to be enabled",
            )));
        }
    }

    #[cfg(feature = "ner-onnx")]
    {
        if request.ner || request.all_ner_models || request.ner_model.is_some() {
            let models_to_warm: Vec<String> = if request.all_ner_models {
                crate::text::ner::known_models().iter().map(|s| s.to_string()).collect()
            } else if let Some(ref name) = request.ner_model {
                vec![name.clone()]
            } else {
                vec![crate::text::ner::default_model_name().to_string()]
            };

            for model in &models_to_warm {
                let path = crate::text::ner::download_model(model, None).map_err(|e| {
                    ApiError::bad_gateway(crate::error::XbergError::Other(format!(
                        "Failed to download NER model '{}': {}",
                        model, e
                    )))
                })?;
                downloaded.push(format!(
                    "ner gliner ({model}) -> {} (Hugging Face cache)",
                    path.display()
                ));
            }
        }
    }

    #[cfg(not(feature = "ner-onnx"))]
    {
        if request.ner || request.all_ner_models || request.ner_model.is_some() {
            return Err(ApiError::validation(crate::error::XbergError::MissingDependency(
                "NER model warming requires the 'ner-onnx' feature to be enabled".to_string(),
            )));
        }
    }

    Ok(Json(WarmResponse {
        cache_dir: cache_base.to_string_lossy().to_string(),
        downloaded,
        already_cached,
    }))
}

/// Resolve the cache base directory.
fn resolve_cache_base() -> std::path::PathBuf {
    crate::cache_dir::resolve_cache_base()
}

/// Submit an async extraction job.
///
/// POST /extract-async
///
/// Accepts multipart form data with:
/// - `files`: One or more files to extract
/// - `config` (optional): JSON extraction configuration
///
/// Returns immediately with a job ID. Poll `GET /jobs/{job_id}` for status.
///
/// # Size Limits
///
/// Request body size limits are enforced at the router layer via `DefaultBodyLimit` and `RequestBodyLimitLayer`.
/// Default limits:
/// - Total request body: 100 MB (all files + form data combined)
/// - Individual multipart fields: 100 MB (controlled by Axum's `DefaultBodyLimit`)
///
/// Limits can be configured via environment variables or programmatically when creating the router.
/// If a request exceeds the size limit, it will be rejected with HTTP 413 (Payload Too Large).
#[cfg(feature = "api")]
#[utoipa::path(
    post,
    path = "/extract-async",
    tag = "extraction",
    request_body(content_type = "multipart/form-data"),
    responses(
        (status = 202, description = "Job accepted", body = AsyncJobResponse),
        (status = 400, description = "Bad request", body = crate::api::types::ErrorResponse),
        (status = 413, description = "Payload too large", body = crate::api::types::ErrorResponse),
    )
)]
pub(crate) async fn extract_async_handler(
    State(state): State<ApiState>,
    request: UnifiedExtractRequest,
) -> Result<axum::response::Response, ApiError> {
    if request.inputs.is_empty() {
        return Err(ApiError::validation(crate::error::XbergError::validation(
            "No inputs provided",
        )));
    }

    if state.job_store.active_count() >= super::jobs::MAX_ACTIVE_JOBS {
        return Err(ApiError::new(
            axum::http::StatusCode::TOO_MANY_REQUESTS,
            crate::error::XbergError::Other("too many concurrent jobs; try again later".into()),
        ));
    }

    let job_id = state.job_store.create_job();
    let mut effective_config = request.config.unwrap_or_else(|| (*state.default_config).clone());
    apply_multipart_config_fields(&mut effective_config, request.output_format, request.pdf_passwords);
    enforce_api_uri_policy(&request.inputs)?;
    effective_config.cancel_token = state.job_store.cancellation_token(&job_id);
    let inputs = request.inputs;

    let job_store = Arc::clone(&state.job_store);
    let job_id_bg = job_id.clone();

    tokio::spawn(async move {
        let store = job_store;
        let jid = job_id_bg;

        store.set_running(&jid, super::jobs::now_rfc3339());

        let timeout_secs = effective_config.extraction_timeout_secs.unwrap_or(300);
        let timeout_dur = std::time::Duration::from_secs(timeout_secs);

        let extraction_fut = async {
            let results = extract_unified_inputs(inputs, effective_config)
                .await
                .map_err(|e| e.body.message)?;
            serde_json::to_value(&results).map_err(|e| format!("failed to serialize results: {e}"))
        };

        match tokio::time::timeout(timeout_dur, extraction_fut).await {
            Ok(Ok(value)) => store.complete(&jid, value, super::jobs::now_rfc3339()),
            Ok(Err(e)) => store.fail(&jid, e, super::jobs::now_rfc3339()),
            Err(_elapsed) => store.fail(
                &jid,
                format!("extraction timed out after {}s", timeout_secs),
                super::jobs::now_rfc3339(),
            ),
        }
    });

    Ok((
        axum::http::StatusCode::ACCEPTED,
        axum::Json(AsyncJobResponse { job_id }),
    )
        .into_response())
}

/// Poll the status of an async extraction job.
///
/// GET /jobs/{job_id}
///
/// Returns the current `JobStatus`. Once `state == completed` the `result`
/// field is populated; once `state == failed` the `error` field is populated.
/// Jobs expire after 5 minutes and return 404 once evicted.
#[cfg(feature = "api")]
#[utoipa::path(
    get,
    path = "/jobs/{job_id}",
    tag = "extraction",
    params(
        ("job_id" = String, Path, description = "Job ID returned by POST /extract-async"),
    ),
    responses(
        (status = 200, description = "Job status", body = crate::api::types::JobStatus),
        (status = 404, description = "Job not found or expired", body = crate::api::types::ErrorResponse),
    )
)]
pub(crate) async fn job_status_handler(
    State(state): State<ApiState>,
    axum::extract::Path(job_id): axum::extract::Path<String>,
) -> Result<axum::Json<JobStatusResponse>, ApiError> {
    match state.job_store.get(&job_id) {
        Some(status) => Ok(axum::Json(status)),
        None => Err(ApiError {
            status: axum::http::StatusCode::NOT_FOUND,
            body: super::types::ErrorResponse {
                error_type: "NotFoundError".to_string(),
                message: format!("Job '{}' not found or expired", job_id),
                traceback: None,
                status_code: axum::http::StatusCode::NOT_FOUND.as_u16(),
            },
        }),
    }
}

/// Cancel a pending or running async extraction job.
///
/// DELETE /jobs/{job_id}
///
/// A pending job is removed from the queue; a running job's extraction is
/// signalled to stop cooperatively at its next checkpoint. Both transition to
/// `cancelled` and return the updated `JobStatus`. A job that already reached
/// `completed`, `failed`, or `cancelled` cannot be cancelled again and returns
/// 409. Jobs expire after 5 minutes and return 404 once evicted, as with
/// `GET /jobs/{job_id}`.
#[cfg(feature = "api")]
#[utoipa::path(
    delete,
    path = "/jobs/{job_id}",
    tag = "extraction",
    params(
        ("job_id" = String, Path, description = "Job ID returned by POST /extract-async"),
    ),
    responses(
        (status = 200, description = "Job cancelled", body = crate::api::types::JobStatus),
        (status = 404, description = "Job not found or expired", body = crate::api::types::ErrorResponse),
        (status = 409, description = "Job already reached a terminal state", body = crate::api::types::ErrorResponse),
    )
)]
#[cfg_attr(
    feature = "otel",
    tracing::instrument(
        name = "api.cancel_job",
        skip(state),
        fields(job_id = %job_id, outcome = tracing::field::Empty)
    )
)]
pub(crate) async fn cancel_job_handler(
    State(state): State<ApiState>,
    axum::extract::Path(job_id): axum::extract::Path<String>,
) -> Result<axum::Json<JobStatusResponse>, ApiError> {
    let outcome = state.job_store.cancel(&job_id, super::jobs::now_rfc3339());

    #[cfg(feature = "otel")]
    tracing::Span::current().record(
        "outcome",
        match &outcome {
            super::jobs::CancelOutcome::Cancelled(_) => "cancelled",
            super::jobs::CancelOutcome::Conflict(_) => "conflict",
            super::jobs::CancelOutcome::NotFound => "not_found",
        },
    );

    match outcome {
        super::jobs::CancelOutcome::Cancelled(status) => Ok(axum::Json(status)),
        super::jobs::CancelOutcome::Conflict(status) => Err(ApiError {
            status: axum::http::StatusCode::CONFLICT,
            body: super::types::ErrorResponse {
                error_type: "ConflictError".to_string(),
                message: format!(
                    "Job '{}' already reached state '{:?}' and cannot be cancelled",
                    job_id, status.state
                ),
                traceback: None,
                status_code: axum::http::StatusCode::CONFLICT.as_u16(),
            },
        }),
        super::jobs::CancelOutcome::NotFound => Err(ApiError {
            status: axum::http::StatusCode::NOT_FOUND,
            body: super::types::ErrorResponse {
                error_type: "NotFoundError".to_string(),
                message: format!("Job '{}' not found or expired", job_id),
                traceback: None,
                status_code: axum::http::StatusCode::NOT_FOUND.as_u16(),
            },
        }),
    }
}

/// Handler for 404 Not Found errors.
///
/// Returns a JSON error response instead of the default plain text.
pub async fn not_found_handler() -> ApiError {
    ApiError::new(
        axum::http::StatusCode::NOT_FOUND,
        crate::error::XbergError::validation("The requested resource was not found"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Router,
        body::Body,
        http::{Request, StatusCode},
        routing::{get, post},
    };
    use tower::ServiceExt;

    fn test_router() -> Router {
        let extraction_service = crate::service::ExtractionServiceBuilder::new().build();
        let state = ApiState {
            default_config: std::sync::Arc::new(crate::ExtractionConfig::default()),
            extraction_service: std::sync::Arc::new(std::sync::Mutex::new(extraction_service)),
            #[cfg(feature = "api")]
            job_store: std::sync::Arc::new(crate::api::jobs::JobStore::new()),
        };
        #[allow(unused_mut)]
        let mut router = Router::new()
            .route("/version", get(version_handler))
            .route("/detect", post(detect_handler))
            .route("/cache/manifest", get(cache_manifest_handler))
            .route("/cache/warm", post(cache_warm_handler));

        #[cfg(feature = "api")]
        let router = router
            .route("/extract-async", post(extract_async_handler))
            .route("/jobs/{job_id}", get(job_status_handler).delete(cancel_job_handler));

        router.with_state(state)
    }

    #[tokio::test]
    async fn test_version_handler_returns_200() {
        let app = test_router();
        let response = app
            .oneshot(Request::builder().uri("/version").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(json["version"].is_string());
        assert!(!json["version"].as_str().unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_cache_manifest_handler_returns_200() {
        let app = test_router();
        let response = app
            .oneshot(Request::builder().uri("/cache/manifest").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(json["xberg_version"].is_string());
        assert!(json["total_size_bytes"].is_number());
        assert!(json["model_count"].is_number());
        assert!(json["models"].is_array());
    }

    #[tokio::test]
    async fn test_detect_handler_no_file_returns_400() {
        let app = test_router();

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/detect")
                    .header("content-type", "multipart/form-data; boundary=testboundary")
                    .body(Body::from("--testboundary--\r\n"))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_cache_warm_handler_empty_request_returns_200() {
        let app = test_router();
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/cache/warm")
                    .header("content-type", "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(json["cache_dir"].is_string());
        assert!(json["downloaded"].is_array());
        assert!(json["already_cached"].is_array());
    }

    #[tokio::test]
    async fn test_cache_warm_handler_empty_embedding_model_returns_400() {
        let app = test_router();
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/cache/warm")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"embedding_model": ""}"#))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let error_msg = json["message"].as_str().unwrap_or("");
        assert!(
            error_msg.contains("must not be empty"),
            "Expected empty embedding_model validation error, got: {}",
            error_msg
        );
    }

    #[tokio::test]
    async fn test_cache_warm_handler_whitespace_embedding_model_returns_400() {
        let app = test_router();
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/cache/warm")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"embedding_model": "   "}"#))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_cache_warm_handler_empty_ner_model_returns_400() {
        let app = test_router();
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/cache/warm")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"ner_model": ""}"#))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let error_msg = json["message"].as_str().unwrap_or("");
        assert!(
            error_msg.contains("ner_model") && error_msg.contains("must not be empty"),
            "Expected empty ner_model validation error, got: {}",
            error_msg
        );
    }

    #[cfg(not(feature = "ner-onnx"))]
    #[tokio::test]
    async fn test_cache_warm_handler_ner_request_without_feature_returns_400() {
        let app = test_router();
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/cache/warm")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"ner": true}"#))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let error_msg = json["message"].as_str().unwrap_or("");
        assert!(
            error_msg.contains("ner-onnx"),
            "Expected missing ner-onnx validation error, got: {}",
            error_msg
        );
    }

    #[cfg(feature = "api")]
    #[tokio::test]
    async fn test_extract_async_returns_job_id() {
        let app = test_router();
        let boundary = "testboundary123";
        let body = format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"files\"; filename=\"test.txt\"\r\nContent-Type: text/plain\r\n\r\nhello world\r\n--{boundary}--\r\n",
            boundary = boundary
        );

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/extract-async")
                    .header("content-type", format!("multipart/form-data; boundary={}", boundary))
                    .body(Body::from(body))
                    .expect("valid request"),
            )
            .await
            .expect("handler responded");

        assert_eq!(
            response.status(),
            StatusCode::ACCEPTED,
            "expected HTTP 202 Accepted from POST /extract-async"
        );

        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body bytes readable");
        let resp: AsyncJobResponse = serde_json::from_slice(&bytes).expect("response parses as AsyncJobResponse");
        assert!(!resp.job_id.is_empty(), "job_id must be non-empty");
    }

    #[cfg(feature = "api")]
    #[tokio::test]
    async fn test_job_status_not_found() {
        let app = test_router();

        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/jobs/does-not-exist")
                    .body(Body::empty())
                    .expect("valid request"),
            )
            .await
            .expect("handler responded");

        assert_eq!(
            response.status(),
            StatusCode::NOT_FOUND,
            "expected HTTP 404 for unknown job ID"
        );
    }

    #[cfg(feature = "api")]
    #[tokio::test]
    async fn test_cancel_job_not_found() {
        let app = test_router();

        let response = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/jobs/does-not-exist")
                    .body(Body::empty())
                    .expect("valid request"),
            )
            .await
            .expect("handler responded");

        assert_eq!(
            response.status(),
            StatusCode::NOT_FOUND,
            "expected HTTP 404 for unknown job ID"
        );
    }

    #[cfg(feature = "api")]
    #[tokio::test]
    async fn test_cancel_job_immediately_after_submission() {
        use crate::api::types::{JobState, JobStatus};
        use tower::Service;

        let mut app = test_router();
        let boundary = "cancelboundary000";
        let body = format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"files\"; filename=\"test.txt\"\r\nContent-Type: text/plain\r\n\r\nhello world\r\n--{boundary}--\r\n",
            boundary = boundary
        );

        let post_req: Request<Body> = Request::builder()
            .method("POST")
            .uri("/extract-async")
            .header("content-type", format!("multipart/form-data; boundary={}", boundary))
            .body(Body::from(body))
            .expect("valid request");
        let post_response = tower::ServiceExt::<Request<Body>>::ready(&mut app)
            .await
            .expect("service ready")
            .call(post_req)
            .await
            .expect("POST handler responded");
        let post_bytes = axum::body::to_bytes(post_response.into_body(), usize::MAX)
            .await
            .expect("POST body bytes readable");
        let async_resp: AsyncJobResponse =
            serde_json::from_slice(&post_bytes).expect("POST response parses as AsyncJobResponse");
        let job_id = async_resp.job_id;

        let delete_req: Request<Body> = Request::builder()
            .method("DELETE")
            .uri(format!("/jobs/{}", job_id))
            .body(Body::empty())
            .expect("valid request");
        let delete_response = tower::ServiceExt::<Request<Body>>::ready(&mut app)
            .await
            .expect("service ready")
            .call(delete_req)
            .await
            .expect("DELETE handler responded");

        assert_eq!(
            delete_response.status(),
            StatusCode::OK,
            "expected HTTP 200 when cancelling a job that has not yet reached a terminal state"
        );
        let delete_bytes = axum::body::to_bytes(delete_response.into_body(), usize::MAX)
            .await
            .expect("DELETE body bytes readable");
        let status: JobStatus = serde_json::from_slice(&delete_bytes).expect("response is JobStatus");
        assert_eq!(status.job_id, job_id);
        assert_eq!(status.state, JobState::Cancelled);
    }

    #[cfg(feature = "api")]
    #[tokio::test]
    async fn test_cancel_job_conflict_after_completion() {
        use crate::api::types::{JobState, JobStatus};
        use tower::Service;

        let mut app = test_router();
        let boundary = "conflictboundary111";
        let body = format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"files\"; filename=\"test.txt\"\r\nContent-Type: text/plain\r\n\r\nhello world\r\n--{boundary}--\r\n",
            boundary = boundary
        );

        let post_req: Request<Body> = Request::builder()
            .method("POST")
            .uri("/extract-async")
            .header("content-type", format!("multipart/form-data; boundary={}", boundary))
            .body(Body::from(body))
            .expect("valid request");
        let post_response = tower::ServiceExt::<Request<Body>>::ready(&mut app)
            .await
            .expect("service ready")
            .call(post_req)
            .await
            .expect("POST handler responded");
        let post_bytes = axum::body::to_bytes(post_response.into_body(), usize::MAX)
            .await
            .expect("POST body bytes readable");
        let async_resp: AsyncJobResponse =
            serde_json::from_slice(&post_bytes).expect("POST response parses as AsyncJobResponse");
        let job_id = async_resp.job_id;

        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            let poll_req: Request<Body> = Request::builder()
                .method("GET")
                .uri(format!("/jobs/{}", job_id))
                .body(Body::empty())
                .expect("valid request");
            let resp = tower::ServiceExt::<Request<Body>>::ready(&mut app)
                .await
                .expect("service ready")
                .call(poll_req)
                .await
                .expect("GET responded");
            let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
                .await
                .expect("body readable");
            let status: JobStatus = serde_json::from_slice(&bytes).expect("response is JobStatus");
            if matches!(status.state, JobState::Completed | JobState::Failed) {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "job did not reach terminal state within 2s"
            );
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }

        let delete_req: Request<Body> = Request::builder()
            .method("DELETE")
            .uri(format!("/jobs/{}", job_id))
            .body(Body::empty())
            .expect("valid request");
        let delete_response = tower::ServiceExt::<Request<Body>>::ready(&mut app)
            .await
            .expect("service ready")
            .call(delete_req)
            .await
            .expect("DELETE handler responded");

        assert_eq!(
            delete_response.status(),
            StatusCode::CONFLICT,
            "expected HTTP 409 when cancelling a job that already completed"
        );
    }

    #[cfg(feature = "api")]
    #[tokio::test]
    async fn test_extract_async_then_poll_job_id() {
        use crate::api::types::{JobState, JobStatus};
        use tower::Service;

        let mut app = test_router();
        let boundary = "pollboundary456";
        let body = format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"files\"; filename=\"hello.txt\"\r\nContent-Type: text/plain\r\n\r\nhello world\r\n--{boundary}--\r\n",
            boundary = boundary
        );

        let post_req: Request<Body> = Request::builder()
            .method("POST")
            .uri("/extract-async")
            .header("content-type", format!("multipart/form-data; boundary={}", boundary))
            .body(Body::from(body))
            .expect("valid request");
        let post_response = tower::ServiceExt::<Request<Body>>::ready(&mut app)
            .await
            .expect("service ready")
            .call(post_req)
            .await
            .expect("POST handler responded");

        assert_eq!(
            post_response.status(),
            StatusCode::ACCEPTED,
            "expected HTTP 202 from POST /extract-async"
        );

        let post_bytes = axum::body::to_bytes(post_response.into_body(), usize::MAX)
            .await
            .expect("POST body bytes readable");
        let async_resp: AsyncJobResponse =
            serde_json::from_slice(&post_bytes).expect("POST response parses as AsyncJobResponse");
        let job_id = async_resp.job_id;
        assert!(!job_id.is_empty(), "job_id from POST must be non-empty");

        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
        let final_status = loop {
            let poll_req: Request<Body> = Request::builder()
                .method("GET")
                .uri(format!("/jobs/{}", job_id))
                .body(Body::empty())
                .expect("valid request");
            let resp = tower::ServiceExt::<Request<Body>>::ready(&mut app)
                .await
                .expect("service ready")
                .call(poll_req)
                .await
                .expect("GET responded");
            assert_eq!(
                resp.status(),
                StatusCode::OK,
                "expected HTTP 200 from GET /jobs/{{job_id}}"
            );
            let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
                .await
                .expect("body readable");
            let status: JobStatus = serde_json::from_slice(&bytes).expect("response is JobStatus");
            if matches!(status.state, JobState::Completed | JobState::Failed) {
                break status;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "job did not reach terminal state within 2s"
            );
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        };

        assert_eq!(
            final_status.job_id, job_id,
            "JobStatus.job_id must match the submitted job_id"
        );
        assert_eq!(
            final_status.state,
            JobState::Completed,
            "job must complete successfully"
        );
        assert!(
            final_status.result.is_some(),
            "completed job must carry an extraction result"
        );
    }

    #[cfg(feature = "api")]
    #[tokio::test]
    async fn test_extract_async_bad_file_fails() {
        use crate::api::types::{JobState, JobStatus};
        use tower::Service;

        let mut app = test_router();
        let boundary = "badboundary789";
        let body = format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"files\"; filename=\"bad.xyz\"\r\nContent-Type: application/x-unsupported-format\r\n\r\ngarbage\r\n--{boundary}--\r\n",
            boundary = boundary
        );

        let post_req: Request<Body> = Request::builder()
            .method("POST")
            .uri("/extract-async")
            .header("content-type", format!("multipart/form-data; boundary={}", boundary))
            .body(Body::from(body))
            .expect("valid request");
        let post_response = tower::ServiceExt::<Request<Body>>::ready(&mut app)
            .await
            .expect("service ready")
            .call(post_req)
            .await
            .expect("POST handler responded");

        assert_eq!(post_response.status(), StatusCode::ACCEPTED);

        let post_bytes = axum::body::to_bytes(post_response.into_body(), usize::MAX)
            .await
            .expect("body readable");
        let async_resp: AsyncJobResponse = serde_json::from_slice(&post_bytes).expect("parses as AsyncJobResponse");
        let job_id = async_resp.job_id;

        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
        let final_status = loop {
            let poll_req: Request<Body> = Request::builder()
                .method("GET")
                .uri(format!("/jobs/{}", job_id))
                .body(Body::empty())
                .expect("valid request");
            let resp = tower::ServiceExt::<Request<Body>>::ready(&mut app)
                .await
                .expect("service ready")
                .call(poll_req)
                .await
                .expect("GET responded");
            assert_eq!(resp.status(), StatusCode::OK);
            let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
                .await
                .expect("body readable");
            let status: JobStatus = serde_json::from_slice(&bytes).expect("response is JobStatus");
            if matches!(status.state, JobState::Completed | JobState::Failed) {
                break status;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "job did not reach terminal state within 2s"
            );
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        };

        assert_eq!(
            final_status.state,
            JobState::Completed,
            "unsupported-format input is reported in the result envelope, not as a job failure"
        );
        let result = final_status
            .result
            .expect("completed job must carry an extraction result");
        let errors = result
            .get("errors")
            .and_then(|value| value.as_array())
            .expect("result envelope must contain an errors array");
        assert!(
            !errors.is_empty(),
            "unsupported-format input must be reported as a per-input error"
        );
    }
}
