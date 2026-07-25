//! API router setup and configuration.

use std::sync::Arc;

use axum::{
    Router,
    extract::DefaultBodyLimit,
    routing::{delete, get, post, put},
};
use tower_http::{
    catch_panic::CatchPanicLayer,
    compression::CompressionLayer,
    cors::{AllowOrigin, Any, CorsLayer},
    limit::RequestBodyLimitLayer,
    request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer},
    sensitive_headers::SetSensitiveHeadersLayer,
    trace::{DefaultMakeSpan, DefaultOnFailure, DefaultOnRequest, DefaultOnResponse, TraceLayer},
};

use crate::{ExtractionConfig, core::ServerConfig, service::ExtractionServiceBuilder};

use super::{
    handlers::{
        cache_clear_handler, cache_manifest_handler, cache_stats_handler, cache_warm_handler, cancel_job_handler,
        detect_handler, extract_async_handler, extract_handler, formats_handler, health_handler, info_handler,
        job_status_handler, not_found_handler, version_handler,
    },
    openweb::{openweb_docling_handler, openweb_external_handler},
    types::{ApiSizeLimits, ApiState},
};

/// Create the API router with all routes configured.
///
/// This is public to allow users to embed the router in their own applications.
///
/// # Arguments
///
/// * `config` - Default extraction configuration. Per-request configs override these defaults.
///
/// # Examples
///
/// ```no_run
/// use xberg::{ExtractionConfig, api::create_router};
///
/// # #[tokio::main]
/// # async fn main() {
/// // Create router with default config and size limits
/// let config = ExtractionConfig::default();
/// let router = create_router(config);
/// # }
/// ```
#[cfg_attr(alef, alef(skip))]
pub fn create_router(config: ExtractionConfig) -> Router {
    create_router_with_limits(config, ApiSizeLimits::default())
}

/// Create the API router with custom size limits.
///
/// This allows fine-grained control over request body and multipart field size limits.
///
/// # Arguments
///
/// * `config` - Default extraction configuration. Per-request configs override these defaults.
/// * `limits` - Size limits for request bodies and multipart uploads.
///
/// # Examples
///
/// ```ignore
/// use xberg::{ExtractionConfig, api::{create_router_with_limits, ApiSizeLimits}};
///
/// # #[tokio::main]
/// # async fn main() {
/// // Create router with 50 MB limits
/// let config = ExtractionConfig::default();
/// let limits = ApiSizeLimits::from_mb(50, 50);
/// let router = create_router_with_limits(config, limits);
/// # }
/// ```
///
/// ```ignore
/// use xberg::{ExtractionConfig, api::{create_router_with_limits, ApiSizeLimits}};
/// use tower_http::limit::RequestBodyLimitLayer;
///
/// # #[tokio::main]
/// # async fn main() {
/// // Custom limits for very large documents (500 MB)
/// let config = ExtractionConfig::default();
/// let limits = ApiSizeLimits::from_mb(500, 500);
/// let router = create_router_with_limits(config, limits);
/// # }
/// ```
#[cfg_attr(alef, alef(skip))]
pub fn create_router_with_limits(config: ExtractionConfig, limits: ApiSizeLimits) -> Router {
    create_router_with_limits_and_server_config(config, limits, ServerConfig::default())
}

/// Create the API router with custom size limits and server configuration.
///
/// This function provides full control over request limits, CORS, and server settings via ServerConfig.
///
/// # Arguments
///
/// * `config` - Default extraction configuration. Per-request configs override these defaults.
/// * `limits` - Size limits for request bodies and multipart uploads.
/// * `server_config` - Server configuration including host, port, and CORS settings.
///
/// # Examples
///
/// ```ignore
/// use xberg::{ExtractionConfig, api::{create_router_with_limits_and_server_config, ApiSizeLimits}, core::ServerConfig};
///
/// # #[tokio::main]
/// # async fn main() -> xberg::Result<()> {
/// let extraction_config = ExtractionConfig::default();
/// let mut server_config = ServerConfig::default();
/// server_config.cors_origins = vec!["https://example.com".to_string()];
/// let router = create_router_with_limits_and_server_config(
///     extraction_config,
///     ApiSizeLimits::default(),
///     server_config
/// );
/// # Ok(())
/// # }
/// ```
pub(crate) fn create_router_with_limits_and_server_config(
    config: ExtractionConfig,
    limits: ApiSizeLimits,
    server_config: ServerConfig,
) -> Router {
    let extraction_service = ExtractionServiceBuilder::new().with_tracing().with_metrics().build();

    let state = ApiState {
        default_config: Arc::new(config),
        extraction_service: Arc::new(std::sync::Mutex::new(extraction_service)),
        #[cfg(feature = "api")]
        job_store: Arc::new(super::jobs::JobStore::new()),
    };

    let cors_layer = if server_config.cors_allows_all() {
        tracing::warn!(
            "CORS configured to allow all origins (default). This permits CSRF attacks. \
             For production, set XBERG_CORS_ORIGINS environment variable to comma-separated \
             list of allowed origins (e.g., 'https://app.example.com,https://api.example.com')"
        );
        CorsLayer::new().allow_origin(Any).allow_methods(Any).allow_headers(Any)
    } else {
        let origins: Vec<_> = server_config
            .cors_origins
            .iter()
            .filter_map(|s| s.trim().parse::<axum::http::HeaderValue>().ok())
            .collect();

        if !origins.is_empty() {
            tracing::info!("CORS configured with {} explicit allowed origin(s)", origins.len());
            CorsLayer::new()
                .allow_origin(AllowOrigin::list(origins))
                .allow_methods(Any)
                .allow_headers(Any)
        } else {
            tracing::warn!(
                "CORS origins configured but empty/invalid - falling back to permissive CORS. \
                 This allows CSRF attacks. Set explicit origins for production."
            );
            CorsLayer::new().allow_origin(Any).allow_methods(Any).allow_headers(Any)
        }
    };

    let mut router = Router::new()
        .route("/extract", post(extract_handler))
        .route("/extract-async", post(extract_async_handler))
        .route("/jobs/{job_id}", get(job_status_handler).delete(cancel_job_handler))
        .route("/detect", post(detect_handler))
        .route("/formats", get(formats_handler))
        .route("/health", get(health_handler))
        .route("/info", get(info_handler))
        .route("/version", get(version_handler))
        .route("/cache/stats", get(cache_stats_handler))
        .route("/cache/clear", delete(cache_clear_handler))
        .route("/cache/manifest", get(cache_manifest_handler))
        .route("/cache/warm", post(cache_warm_handler))
        .route("/process", put(openweb_external_handler))
        .route("/v1/convert/file", post(openweb_docling_handler))
        .fallback(not_found_handler);

    #[cfg(feature = "api")]
    {
        router = router.route("/openapi.json", get(openapi_schema_handler));
    }

    router
        .layer(DefaultBodyLimit::max(limits.max_request_body_bytes))
        .layer(RequestBodyLimitLayer::new(limits.max_request_body_bytes))
        .layer(cors_layer)
        .layer(SetRequestIdLayer::x_request_id(MakeRequestUuid))
        .layer(PropagateRequestIdLayer::x_request_id())
        .layer(CompressionLayer::new())
        .layer(CatchPanicLayer::new())
        .layer(SetSensitiveHeadersLayer::new([axum::http::header::AUTHORIZATION]))
        // Per-request and per-response events are demoted to DEBUG so the default
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(DefaultMakeSpan::new().level(tracing::Level::DEBUG))
                .on_request(DefaultOnRequest::new().level(tracing::Level::DEBUG))
                .on_response(DefaultOnResponse::new().level(tracing::Level::DEBUG))
                .on_failure(DefaultOnFailure::new().level(tracing::Level::WARN)),
        )
        .with_state(state)
}

/// OpenAPI schema handler.
///
/// Returns the OpenAPI 3.1 JSON schema for all documented endpoints.
#[cfg(feature = "api")]
async fn openapi_schema_handler() -> axum::Json<serde_json::Value> {
    use crate::api::openapi::openapi_json;

    let schema_str = openapi_json();
    let schema: serde_json::Value = serde_json::from_str(&schema_str)
        .unwrap_or_else(|_| serde_json::json!({"error": "Failed to generate OpenAPI schema"}));

    axum::Json(schema)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_router() {
        let config = ExtractionConfig::default();
        let _router = create_router(config);
    }

    #[test]
    fn test_router_has_routes() {
        use std::mem::size_of_val;
        let config = ExtractionConfig::default();
        let router = create_router(config);
        assert!(size_of_val(&router) > 0);
    }

    #[test]
    fn test_create_router_with_limits() {
        let config = ExtractionConfig::default();
        let limits = ApiSizeLimits::from_mb(50, 50);
        let _router = create_router_with_limits(config, limits);
    }

    #[test]
    fn test_create_router_with_server_config() {
        let extraction_config = ExtractionConfig::default();
        let limits = ApiSizeLimits::from_mb(100, 100);
        let server_config = ServerConfig::default();
        let _router = create_router_with_limits_and_server_config(extraction_config, limits, server_config);
    }

    #[test]
    fn test_server_config_cors_handling() {
        let extraction_config = ExtractionConfig::default();
        let limits = ApiSizeLimits::default();
        let server_config = ServerConfig {
            cors_origins: vec!["https://example.com".to_string()],
            ..Default::default()
        };
        let _router = create_router_with_limits_and_server_config(extraction_config, limits, server_config);
    }
}
