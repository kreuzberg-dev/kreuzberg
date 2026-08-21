//! OpenWebUI compatibility handlers.
//!
//! Provides endpoints compatible with OpenWebUI's Content Extraction Engine:
//!
//! - `PUT /process` — "External" engine: raw binary body, returns `{page_content, metadata}`
//! - `POST /v1/convert/file` — "Docling" engine: multipart form-data, returns `{document: {md_content}, status}`

use axum::{Json, body::Bytes, extract::State, http::HeaderMap};
use tower::Service;

use crate::service::ExtractionRequest;

use super::{
    error::{ApiError, MultipartApi},
    types::{ApiState, DoclingCompatDocument, DoclingCompatResponse, OpenWebDocumentMetadata, OpenWebDocumentResponse},
};

/// OpenWebUI "External" engine handler.
///
/// PUT /process
///
/// Accepts raw binary file content in the request body.
/// Uses `Content-Type` header for MIME type and `X-Filename` header for the filename.
///
/// Returns a JSON document matching OpenWebUI's external document loader contract.
#[utoipa::path(
    put,
    path = "/process",
    tag = "openweb",
    request_body(content_type = "application/octet-stream", content = Vec<u8>),
    responses(
        (status = 200, description = "Document extracted", body = OpenWebDocumentResponse),
        (status = 400, description = "Bad request", body = crate::api::types::ErrorResponse),
        (status = 500, description = "Internal server error", body = crate::api::types::ErrorResponse),
    )
)]
#[cfg_attr(
    feature = "otel",
    tracing::instrument(name = "api.openweb_process", skip(state, headers, body))
)]
pub(crate) async fn openweb_external_handler(
    State(state): State<ApiState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<OpenWebDocumentResponse>, ApiError> {
    if body.is_empty() {
        return Err(ApiError::validation(crate::error::XbergError::validation(
            "Empty request body — upload a file as the raw request body",
        )));
    }

    let mime_type = headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.split(';').next().unwrap_or(v).trim())
        .unwrap_or(crate::core::mime::OCTET_STREAM_MIME_TYPE)
        .to_string();

    let filename = headers
        .get("X-Filename")
        .and_then(|v| v.to_str().ok())
        .map(|v| urlencoding::decode(v).unwrap_or_else(|_| v.into()).into_owned())
        .unwrap_or_else(|| "unknown".to_string());

    let mime_type = if mime_type == crate::core::mime::OCTET_STREAM_MIME_TYPE {
        crate::core::mime::detect_mime_type(&filename, false).unwrap_or(mime_type)
    } else {
        mime_type
    };

    // Honor the server's user config as the base, then merge the per-request config
    // supplied via the `X-Config` header (JSON) — same capability as `/extract`.
    let config_json = headers.get("X-Config").and_then(|value| value.to_str().ok());
    let mut config = crate::core::config::merge::build_config_from_json(&state.default_config, config_json)
        .map_err(|e| ApiError::validation(crate::error::XbergError::validation(e)))?;
    // OpenWebUI's external loader consumes rendered content, so default to Markdown only
    // when neither the user's server config nor the request selects a format (`Plain` is
    // the struct default, i.e. "unspecified").
    if config.output_format == crate::core::config::OutputFormat::Plain {
        config.output_format = crate::core::config::OutputFormat::Markdown;
    }

    let request = ExtractionRequest::bytes(body.to_vec(), mime_type, config);
    let mut svc = state
        .extraction_service
        .lock()
        .expect("extraction service lock poisoned")
        .clone();
    let result = svc.call(request).await?;

    Ok(Json(OpenWebDocumentResponse {
        page_content: result.content,
        metadata: OpenWebDocumentMetadata { source: filename },
    }))
}

/// OpenWebUI "Docling" engine handler (docling-serve compatible).
///
/// POST /v1/convert/file
///
/// Accepts multipart form-data with a `files` field containing the document.
/// Returns a JSON response matching docling-serve's `/v1/convert/file` contract.
///
/// OpenWebUI reads only `document.md_content` from the response.
#[utoipa::path(
    post,
    path = "/v1/convert/file",
    tag = "openweb",
    request_body(content_type = "multipart/form-data"),
    responses(
        (status = 200, description = "Document converted", body = DoclingCompatResponse),
        (status = 400, description = "Bad request", body = crate::api::types::ErrorResponse),
        (status = 500, description = "Internal server error", body = crate::api::types::ErrorResponse),
    )
)]
#[cfg_attr(
    feature = "otel",
    tracing::instrument(name = "api.openweb_docling", skip(state, multipart))
)]
pub(crate) async fn openweb_docling_handler(
    State(state): State<ApiState>,
    MultipartApi(mut multipart): MultipartApi,
) -> Result<Json<DoclingCompatResponse>, ApiError> {
    let mut file_data: Option<(Vec<u8>, String)> = None;
    let mut config_json: Option<String> = None;
    let mut flat_config = serde_json::Map::new();

    // OpenWebUI's Docling client sends one form field per parameter rather than a JSON blob,
    // so the keys of the serialized base config are what identify a field as configuration.
    // Matching on them keeps Docling's own knobs (`image_export_mode`,
    // `md_page_break_placeholder`) ignored instead of failing the whole request against
    // `ExtractionConfig`'s `deny_unknown_fields`.
    let config_keys = match serde_json::to_value(&*state.default_config) {
        Ok(serde_json::Value::Object(keys)) => keys,
        _ => serde_json::Map::new(),
    };

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| ApiError::validation(crate::error::XbergError::validation(e.to_string())))?
    {
        let field_name = field.name().unwrap_or("").to_string();

        match field_name.as_str() {
            "files" | "file" => {
                let file_name = field.file_name().map(|s| s.to_string());
                let content_type = field.content_type().map(|s| s.to_string());
                let data = field
                    .bytes()
                    .await
                    .map_err(|e| ApiError::validation(crate::error::XbergError::validation(e.to_string())))?;

                let mut mime_type =
                    content_type.unwrap_or_else(|| crate::core::mime::OCTET_STREAM_MIME_TYPE.to_string());

                if mime_type == crate::core::mime::OCTET_STREAM_MIME_TYPE
                    && let Some(ref name) = file_name
                    && let Ok(detected) = crate::core::mime::detect_mime_type(name, false)
                {
                    mime_type = detected;
                }

                file_data = Some((data.to_vec(), mime_type));
            }
            // A client that sends the whole config as one JSON blob: the /extract field name
            // "config", or the "parameters" label.
            "config" | "parameters" => {
                let text = field
                    .text()
                    .await
                    .map_err(|e| ApiError::validation(crate::error::XbergError::validation(e.to_string())))?;
                if !text.trim().is_empty() {
                    config_json = Some(text);
                }
            }
            name if config_keys.contains_key(name) => {
                let text = field
                    .text()
                    .await
                    .map_err(|e| ApiError::validation(crate::error::XbergError::validation(e.to_string())))?;
                flat_config.insert(name.to_string(), form_value_to_json(&text));
            }
            _ => {}
        }
    }

    if config_json.is_none() && !flat_config.is_empty() {
        config_json = Some(serde_json::Value::Object(flat_config).to_string());
    }

    let (data, mime_type) = file_data.ok_or_else(|| {
        ApiError::validation(crate::error::XbergError::validation(
            "No file provided. Upload a file with field name 'files'.",
        ))
    })?;

    // Honor the server's user config as the base, then merge the per-request config —
    // same capability as `/extract`.
    let mut config = crate::core::config::merge::build_config_from_json(&state.default_config, config_json.as_deref())
        .map_err(|e| ApiError::validation(crate::error::XbergError::validation(e)))?;
    if config.output_format == crate::core::config::OutputFormat::Plain {
        config.output_format = crate::core::config::OutputFormat::Markdown;
    }

    let request = ExtractionRequest::bytes(data, mime_type, config);
    let mut svc = state
        .extraction_service
        .lock()
        .expect("extraction service lock poisoned")
        .clone();
    let result = svc.call(request).await?;

    Ok(Json(DoclingCompatResponse {
        document: DoclingCompatDocument {
            md_content: result.content,
        },
        status: "success".to_string(),
    }))
}

/// Convert one multipart form value into JSON for the config merge.
///
/// Form values are always strings, and OpenWebUI's Python client renders booleans as
/// `True`/`False`, which no JSON parser accepts.
fn form_value_to_json(raw: &str) -> serde_json::Value {
    match raw.trim() {
        "True" => serde_json::Value::Bool(true),
        "False" => serde_json::Value::Bool(false),
        trimmed => serde_json::from_str(trimmed).unwrap_or_else(|_| serde_json::Value::String(raw.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use axum::{
        Router,
        body::Body,
        http::{Request, StatusCode},
        routing::{post, put},
    };
    use tower::ServiceExt;

    use super::*;

    /// Tiny real fixture (39 bytes) from the shared `test_documents` corpus: plain text,
    /// no OCR/network required, extracts to non-empty Markdown quickly.
    fn fixture_bytes() -> Vec<u8> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test_documents/text/plain.txt");
        std::fs::read(&path).unwrap_or_else(|e| panic!("failed to read fixture {}: {e}", path.display()))
    }

    fn test_router() -> Router {
        test_router_with_config(crate::ExtractionConfig::default())
    }

    fn test_router_with_config(config: crate::ExtractionConfig) -> Router {
        let extraction_service = crate::service::ExtractionServiceBuilder::new().build();
        let state = ApiState {
            default_config: std::sync::Arc::new(config),
            extraction_service: std::sync::Arc::new(std::sync::Mutex::new(extraction_service)),
            #[cfg(feature = "api")]
            job_store: std::sync::Arc::new(crate::api::jobs::JobStore::new()),
            #[cfg(feature = "prometheus")]
            prometheus_registry: crate::telemetry::init_prometheus(),
        };

        Router::new()
            .route("/process", put(openweb_external_handler))
            .route("/v1/convert/file", post(openweb_docling_handler))
            .with_state(state)
    }

    fn docling_body(boundary: &str, config: Option<&str>) -> Vec<u8> {
        let mut body = Vec::new();
        body.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"files\"; filename=\"plain.txt\"\r\nContent-Type: text/plain\r\n\r\n"
            )
            .as_bytes(),
        );
        body.extend_from_slice(&fixture_bytes());
        body.extend_from_slice(b"\r\n");
        if let Some(cfg) = config {
            body.extend_from_slice(
                format!("--{boundary}\r\nContent-Disposition: form-data; name=\"config\"\r\n\r\n{cfg}\r\n").as_bytes(),
            );
        }
        body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
        body
    }

    /// A body in the shape OpenWebUI's `DoclingLoader` actually sends: the file plus one
    /// form field per parameter, rather than a single JSON blob.
    fn docling_body_with_fields(boundary: &str, fields: &[(&str, &str)]) -> Vec<u8> {
        let mut body = Vec::new();
        body.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"files\"; filename=\"plain.txt\"\r\nContent-Type: text/plain\r\n\r\n"
            )
            .as_bytes(),
        );
        body.extend_from_slice(&fixture_bytes());
        body.extend_from_slice(b"\r\n");
        for (name, value) in fields {
            body.extend_from_slice(
                format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n")
                    .as_bytes(),
            );
        }
        body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
        body
    }

    async fn docling_flat_response(fields: &[(&str, &str)]) -> (StatusCode, String) {
        let app = test_router();
        let boundary = "flatboundary";
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/convert/file")
                    .header("content-type", format!("multipart/form-data; boundary={boundary}"))
                    .body(Body::from(docling_body_with_fields(boundary, fields)))
                    .expect("valid request"),
            )
            .await
            .expect("handler responded");
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body bytes readable");
        let content = serde_json::from_slice::<DoclingCompatResponse>(&bytes)
            .map(|parsed| parsed.document.md_content)
            .unwrap_or_default();
        (status, content)
    }

    async fn docling_md_content(config: Option<&str>) -> String {
        let app = test_router();
        let boundary = "cfgboundary";
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/convert/file")
                    .header("content-type", format!("multipart/form-data; boundary={boundary}"))
                    .body(Body::from(docling_body(boundary, config)))
                    .expect("valid request"),
            )
            .await
            .expect("handler responded");
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body bytes readable");
        serde_json::from_slice::<DoclingCompatResponse>(&bytes)
            .expect("response parses")
            .document
            .md_content
    }

    #[tokio::test]
    async fn openweb_process_returns_markdown_and_source() {
        let app = test_router();

        let response = app
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/process")
                    .header("content-type", "text/plain")
                    .header("X-Filename", "plain.txt")
                    .body(Body::from(fixture_bytes()))
                    .expect("valid request"),
            )
            .await
            .expect("handler responded");

        assert_eq!(response.status(), StatusCode::OK);

        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body bytes readable");
        let parsed: OpenWebDocumentResponse =
            serde_json::from_slice(&bytes).expect("response parses as OpenWebDocumentResponse");
        assert!(!parsed.page_content.is_empty(), "page_content must be non-empty");
        assert_eq!(parsed.metadata.source, "plain.txt");
    }

    #[tokio::test]
    async fn openweb_process_rejects_empty_body() {
        let app = test_router();

        let response = app
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/process")
                    .header("content-type", "text/plain")
                    .header("X-Filename", "empty.txt")
                    .body(Body::empty())
                    .expect("valid request"),
            )
            .await
            .expect("handler responded");

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn openweb_process_url_decodes_filename_header() {
        let app = test_router();

        let response = app
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/process")
                    .header("content-type", "text/plain")
                    .header("X-Filename", "my%20file.txt")
                    .body(Body::from(fixture_bytes()))
                    .expect("valid request"),
            )
            .await
            .expect("handler responded");

        assert_eq!(response.status(), StatusCode::OK);

        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body bytes readable");
        let parsed: OpenWebDocumentResponse =
            serde_json::from_slice(&bytes).expect("response parses as OpenWebDocumentResponse");
        assert_eq!(parsed.metadata.source, "my file.txt");
    }

    #[tokio::test]
    async fn openweb_docling_convert_returns_md_content() {
        let app = test_router();
        let boundary = "testboundary123";

        let mut body = Vec::new();
        body.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"files\"; filename=\"plain.txt\"\r\nContent-Type: text/plain\r\n\r\n"
            )
            .as_bytes(),
        );
        body.extend_from_slice(&fixture_bytes());
        body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/convert/file")
                    .header("content-type", format!("multipart/form-data; boundary={boundary}"))
                    .body(Body::from(body))
                    .expect("valid request"),
            )
            .await
            .expect("handler responded");

        assert_eq!(response.status(), StatusCode::OK);

        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body bytes readable");
        let parsed: DoclingCompatResponse =
            serde_json::from_slice(&bytes).expect("response parses as DoclingCompatResponse");
        assert!(!parsed.document.md_content.is_empty(), "md_content must be non-empty");
        assert_eq!(parsed.status, "success");
    }

    #[tokio::test]
    async fn openweb_docling_rejects_missing_file() {
        let app = test_router();
        let boundary = "testboundary";
        let body = format!("--{boundary}--\r\n");

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/convert/file")
                    .header("content-type", format!("multipart/form-data; boundary={boundary}"))
                    .body(Body::from(body))
                    .expect("valid request"),
            )
            .await
            .expect("handler responded");

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    /// A `config` field is now parsed and merged: an invalid config must fail the request
    /// rather than being silently ignored (regression for the OpenWebUI params-ignored bug).
    #[tokio::test]
    async fn openweb_docling_rejects_invalid_config() {
        let app = test_router();
        let boundary = "badcfg";
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/convert/file")
                    .header("content-type", format!("multipart/form-data; boundary={boundary}"))
                    .body(Body::from(docling_body(
                        boundary,
                        Some(r#"{"use_cache":"not_a_bool"}"#),
                    )))
                    .expect("valid request"),
            )
            .await
            .expect("handler responded");

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    /// A per-request `config` field changes the rendered output — proof the config now
    /// reaches the pipeline instead of being dropped.
    #[tokio::test]
    async fn openweb_docling_config_field_changes_output() {
        let markdown = docling_md_content(None).await;
        let json = docling_md_content(Some(r#"{"output_format":"json"}"#)).await;
        assert_ne!(
            markdown, json,
            "config output_format should change the rendered content"
        );
    }

    /// OpenWebUI flattens its parameters into one form field per key, so a flattened field
    /// has to reach the config instead of being dropped.
    #[tokio::test]
    async fn openweb_docling_flat_form_field_changes_output() {
        let markdown = docling_md_content(None).await;
        let (status, json) = docling_flat_response(&[("output_format", "json")]).await;

        assert_eq!(status, StatusCode::OK);
        assert_ne!(markdown, json, "a flattened output_format field must reach the config");
    }

    /// `image_export_mode` and `md_page_break_placeholder` are Docling's own parameters and
    /// are sent on every OpenWebUI request. They have no xberg equivalent, so they must stay
    /// ignored rather than failing the request.
    #[tokio::test]
    async fn openweb_docling_ignores_docling_only_form_fields() {
        let markdown = docling_md_content(None).await;
        let (status, json) = docling_flat_response(&[
            ("image_export_mode", "placeholder"),
            ("md_page_break_placeholder", "\u{c}"),
            ("output_format", "json"),
        ])
        .await;

        assert_eq!(status, StatusCode::OK);
        assert_ne!(markdown, json);
    }

    /// The parameter set from the report, in the encoding OpenWebUI's Python client produces:
    /// every value a string, booleans rendered `True`/`False`. `output_format` carries the
    /// assertion because the reporter's own values all render identically to the defaults,
    /// which is why the bug was invisible from the response alone.
    #[tokio::test]
    async fn openweb_docling_accepts_python_rendered_form_values() {
        let markdown = docling_md_content(None).await;
        let (status, json) = docling_flat_response(&[
            ("image_export_mode", "placeholder"),
            ("force_ocr", "False"),
            ("output_format", "json"),
            ("extraction_timeout_secs", "7200"),
            ("disable_ocr", "True"),
        ])
        .await;

        assert_eq!(status, StatusCode::OK, "Python-rendered values must coerce, not 400");
        assert_ne!(markdown, json, "the flattened parameters must reach the config");
    }

    /// An explicit JSON blob keeps precedence over flattened fields.
    #[tokio::test]
    async fn openweb_docling_config_blob_wins_over_flat_fields() {
        let app = test_router();
        let boundary = "bothboundary";
        let mut body = docling_body_with_fields(boundary, &[("output_format", "json")]);
        let closing = format!("--{boundary}--\r\n");
        body.truncate(body.len() - closing.len());
        body.extend_from_slice(
            format!("--{boundary}\r\nContent-Disposition: form-data; name=\"config\"\r\n\r\n{{\"output_format\":\"markdown\"}}\r\n")
                .as_bytes(),
        );
        body.extend_from_slice(closing.as_bytes());

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/convert/file")
                    .header("content-type", format!("multipart/form-data; boundary={boundary}"))
                    .body(Body::from(body))
                    .expect("valid request"),
            )
            .await
            .expect("handler responded");
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body bytes readable");
        let content = serde_json::from_slice::<DoclingCompatResponse>(&bytes)
            .expect("response parses")
            .document
            .md_content;

        assert_eq!(content, docling_md_content(None).await, "the blob must win");
    }

    #[test]
    fn form_values_coerce_python_literals_and_numbers() {
        assert_eq!(form_value_to_json("True"), serde_json::json!(true));
        assert_eq!(form_value_to_json("False"), serde_json::json!(false));
        assert_eq!(form_value_to_json("7200"), serde_json::json!(7200));
        assert_eq!(form_value_to_json("markdown"), serde_json::json!("markdown"));
    }

    /// The Docling endpoint honors the server's user config as the base when no per-request
    /// config is sent (no more unconditional force-to-Markdown).
    #[tokio::test]
    async fn openweb_docling_honors_default_config_format() {
        let cfg = crate::ExtractionConfig {
            output_format: crate::core::config::OutputFormat::Json,
            ..Default::default()
        };
        let app = test_router_with_config(cfg);
        let boundary = "defcfg";
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/convert/file")
                    .header("content-type", format!("multipart/form-data; boundary={boundary}"))
                    .body(Body::from(docling_body(boundary, None)))
                    .expect("valid request"),
            )
            .await
            .expect("handler responded");
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body bytes readable");
        let from_user_config = serde_json::from_slice::<DoclingCompatResponse>(&bytes)
            .expect("response parses")
            .document
            .md_content;
        let markdown_default = docling_md_content(None).await;
        assert_ne!(
            from_user_config, markdown_default,
            "server-configured output_format must be honored, not overridden by Markdown"
        );
    }

    /// The External endpoint accepts a per-request config via the `X-Config` header and
    /// rejects an invalid one.
    #[tokio::test]
    async fn openweb_external_rejects_invalid_x_config() {
        let app = test_router();
        let response = app
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/process")
                    .header("content-type", "text/plain")
                    .header("X-Filename", "plain.txt")
                    .header("X-Config", r#"{"use_cache":"not_a_bool"}"#)
                    .body(Body::from(fixture_bytes()))
                    .expect("valid request"),
            )
            .await
            .expect("handler responded");

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    /// A valid `X-Config` header changes the External endpoint's rendered output.
    #[tokio::test]
    async fn openweb_external_x_config_changes_output() {
        async fn page_content(x_config: Option<&str>) -> String {
            let app = test_router();
            let mut builder = Request::builder()
                .method("PUT")
                .uri("/process")
                .header("content-type", "text/plain")
                .header("X-Filename", "plain.txt");
            if let Some(cfg) = x_config {
                builder = builder.header("X-Config", cfg);
            }
            let response = app
                .oneshot(builder.body(Body::from(fixture_bytes())).expect("valid request"))
                .await
                .expect("handler responded");
            assert_eq!(response.status(), StatusCode::OK);
            let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("body bytes readable");
            serde_json::from_slice::<OpenWebDocumentResponse>(&bytes)
                .expect("response parses")
                .page_content
        }

        let markdown = page_content(None).await;
        let json = page_content(Some(r#"{"output_format":"json"}"#)).await;
        assert_ne!(markdown, json, "X-Config output_format should change rendered content");
    }
}
