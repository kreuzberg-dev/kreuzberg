//! REST API server for Xberg document extraction.
//!
//! This module provides an Axum-based HTTP server for document extraction
//! with endpoints for single and batch extraction operations.
//!
//! # Endpoints
//!
//! - `POST /extract` - Extract text from uploaded files (multipart form data)
//! - `POST /detect` - Detect MIME type of an uploaded file (multipart form data)
//! - `GET /health` - Health check endpoint
//! - `GET /info` - Server information
//! - `GET /version` - Version information
//! - `GET /cache/stats` - Get cache statistics
//! - `GET /cache/manifest` - Get model manifest with checksums and sizes
//! - `POST /cache/warm` - Pre-download models to cache
//! - `DELETE /cache/clear` - Clear all cached files
//! - `PUT /process` - OpenWebUI "External" engine compatibility
//! - `POST /v1/convert/file` - OpenWebUI "Docling" engine compatibility (docling-serve drop-in)
//! - `GET /metrics` - Prometheus text-format extraction metrics (requires the `prometheus` feature)
//!
//! # Examples
//!
//! ## Starting the server
//!
//! ```no_run
//! use xberg::api::serve;
//!
//! #[tokio::main]
//! async fn main() -> xberg::Result<()> {
//!     // Local development
//!     serve("127.0.0.1", 8000).await?;
//!     Ok(())
//! }
//! ```
//!
//! ## Embedding the router in your app
//!
//! ```no_run
//! use xberg::{ExtractionConfig, api::create_router};
//! use axum::Router;
//!
//! #[tokio::main]
//! async fn main() -> xberg::Result<()> {
//!     // Load config (from file or use default)
//!     let config = ExtractionConfig::default();
//!     let xberg_router = create_router(config);
//!
//!     // Nest under /api prefix
//!     let app = Router::new().nest("/api", xberg_router);
//!
//!     // Add your own routes
//!     // ...
//!
//!     Ok(())
//! }
//! ```
//!
//! # cURL Examples
//!
//! ```bash
//! # Single file extraction
//! curl -F "files=@document.pdf" http://localhost:8000/extract
//!
//! # Multiple files with OCR config
//! curl -F "files=@doc1.pdf" -F "files=@doc2.jpg" \
//!      -F 'config={"ocr":{"language":"eng"}}' \
//!      http://localhost:8000/extract
//!
//! # Health check
//! curl http://localhost:8000/health
//!
//! # Server info
//! curl http://localhost:8000/info
//!
//! # Cache statistics
//! curl http://localhost:8000/cache/stats
//!
//! # Clear cache
//! curl -X DELETE http://localhost:8000/cache/clear
//!
//! ```

mod config;
mod error;
mod handlers;
#[cfg(feature = "api")]
pub(crate) mod jobs;
#[cfg(feature = "api")]
pub mod openapi;
mod openweb;
mod router;
mod startup;
mod types;

pub use error::ApiError;
#[allow(unused_imports)]
pub use router::{create_router, create_router_with_limits};
#[allow(unused_imports)]
pub use startup::{serve, serve_default, serve_with_config, serve_with_config_and_limits, serve_with_server_config};
pub use types::{
    ApiSizeLimits, ApiState, CacheClearResponse, CacheStatsResponse, DetectResponse, DoclingCompatDocument,
    DoclingCompatResponse, ErrorResponse, HealthResponse, InfoResponse, ManifestEntryResponse, ManifestResponse,
    OpenWebDocumentMetadata, OpenWebDocumentResponse, VersionResponse, WarmRequest, WarmResponse,
};
