//! OpenAPI 3.1 schema generation for Xberg API.
//!
//! This module generates OpenAPI documentation from Rust types using utoipa.
//! The schema is available at the `/openapi.json` endpoint.

#[cfg(feature = "api")]
use utoipa::OpenApi;

/// OpenAPI documentation structure.
///
/// Defines all endpoints, request/response schemas, and examples
/// for the Xberg document extraction API.
#[cfg(feature = "api")]
#[derive(OpenApi)]
#[openapi(
    info(
        title = "Xberg API",
        version = env!("CARGO_PKG_VERSION"),
        description = "High-performance document intelligence API for extracting text, metadata, and structured data from PDFs, Office documents, images, and 101 formats.",
        contact(
            name = "Xberg",
            url = "https://xberg.io"
        ),
        license(
            name = "Apache-2.0 OR MIT"
        )
    ),
    servers(
        (url = "http://localhost:8000", description = "Local development server"),
        (url = "https://api.xberg.io", description = "Production server (example)")
    ),
    paths(
        crate::api::handlers::health_handler,
        crate::api::handlers::info_handler,
        crate::api::handlers::extract_handler,
        crate::api::handlers::extract_async_handler,
        crate::api::handlers::job_status_handler,
        crate::api::handlers::cancel_job_handler,
        crate::api::handlers::detect_handler,
        crate::api::handlers::formats_handler,
        crate::api::handlers::cache_stats_handler,
        crate::api::handlers::cache_clear_handler,
        crate::api::handlers::cache_manifest_handler,
        crate::api::handlers::cache_warm_handler,
        crate::api::handlers::version_handler,
        crate::api::openweb::openweb_external_handler,
        crate::api::openweb::openweb_docling_handler,
    ),
    components(
        schemas(
            crate::api::types::HealthResponse,
            crate::api::types::PluginStatus,
            crate::api::types::InfoResponse,
            crate::api::types::ErrorResponse,
            crate::api::types::CacheStatsResponse,
            crate::api::types::CacheClearResponse,
            crate::api::types::VersionResponse,
            crate::api::types::DetectResponse,
            crate::api::types::AsyncJobResponse,
            crate::api::types::JobState,
            crate::api::types::JobStatus,
            crate::api::types::ManifestResponse,
            crate::api::types::ManifestEntryResponse,
            crate::api::types::WarmRequest,
            crate::api::types::WarmResponse,
            crate::core::mime::SupportedFormat,
            crate::core::config::ExtractionResult,
            crate::core::config::ExtractionSummary,
            crate::core::config::ExtractionErrorItem,
            crate::types::extraction::ExtractedDocument,
            crate::types::extraction::Chunk,
            crate::types::extraction::ChunkMetadata,
            crate::types::extraction::ExtractedImage,
            crate::types::extraction::Element,
            crate::types::extraction::ElementMetadata,
            crate::types::extraction::ElementId,
            crate::types::extraction::ElementType,
            crate::types::extraction::BoundingBox,
            crate::types::ocr_elements::OcrElement,
            crate::types::ocr_elements::OcrBoundingGeometry,
            crate::types::ocr_elements::OcrConfidence,
            crate::types::ocr_elements::OcrRotation,
            crate::types::ocr_elements::OcrElementLevel,
            crate::types::ocr_elements::OcrElementConfig,
            crate::types::metadata::Metadata,
            crate::types::tables::Table,
            crate::types::page::PageContent,
            crate::types::djot::DjotContent,
            // Nested schemas reachable from the types above. utoipa emits a `$ref`
            // for each of these but only defines a component for types listed here,
            // so omitting one produces a dangling pointer (#251).
            crate::types::extraction::ArchiveEntry,
            crate::types::extraction::DocumentCounts,
            crate::types::extraction::LanguageConfidence,
            crate::types::extraction::ExtractionMethod,
            crate::types::extraction::LlmUsage,
            crate::types::extraction::ProcessingWarning,
            crate::types::djot::DjotImage,
            crate::types::djot::DjotLink,
            crate::types::djot::Footnote,
            crate::types::djot::FormattedBlock,
            crate::types::annotations::PdfAnnotation,
            crate::types::classification::PageClassification,
            crate::types::document_structure::DocumentStructure,
            crate::types::entity::Entity,
            crate::types::form_field::PdfFormField,
            crate::types::formula::Formula,
            crate::types::redaction::RedactionReport,
            crate::types::revisions::DocumentRevision,
            crate::types::summary::DocumentSummary,
            crate::types::translation::Translation,
            crate::types::uri::ExtractedUri,
            // Transitive closure of the above. `NodeIndex` is deliberately absent:
            // it carries `schema(value_type = u32)`, so utoipa inlines it as a
            // primitive rather than emitting a `$ref`.
            crate::types::djot::BlockType,
            crate::types::djot::InlineElement,
            crate::types::djot::InlineType,
            crate::types::djot::Attributes,
            crate::types::document_structure::DocumentNode,
            crate::types::document_structure::DocumentRelationship,
            crate::types::document_structure::RelationshipKind,
            crate::types::document_structure::NodeContent,
            crate::types::document_structure::ContentLayer,
            crate::types::document_structure::TextAnnotation,
            crate::types::document_structure::AnnotationKind,
            crate::types::document_structure::TableGrid,
            crate::types::document_structure::GridCell,
            crate::api::types::OpenWebDocumentResponse,
            crate::api::types::OpenWebDocumentMetadata,
            crate::api::types::DoclingCompatResponse,
            crate::api::types::DoclingCompatDocument,
        )
    ),
    tags(
        (name = "health", description = "Health and status endpoints"),
        (name = "extraction", description = "Document extraction endpoints"),
        (name = "cache", description = "Cache management endpoints"),
        (name = "openweb", description = "OpenWebUI compatibility endpoints")
    )
)]
#[cfg_attr(alef, alef(skip))]
pub struct ApiDoc;

// Schemas for types that only exist behind an optional feature.
//
// utoipa's `components(schemas(..))` list is a single attribute argument list and
// cannot carry `#[cfg]` on individual entries, so a type gated behind a feature
// cannot simply be added to `ApiDoc` above — the reference would fail to compile
// in any build without that feature. Each group therefore gets its own gated
// `OpenApi` document, merged into the spec by `openapi_json`.
//
// Every type below IS reachable from `ExtractedDocument` when its feature is on,
// so leaving it unregistered emits a `$ref` with no matching component — a
// dangling pointer that makes the published spec unusable for codegen (#251).

#[cfg(all(feature = "api", feature = "tree-sitter"))]
#[derive(OpenApi)]
#[openapi(components(schemas(crate::types::metadata::CodeDataAttribute, crate::types::metadata::CodeDataNodeKind)))]
#[cfg_attr(alef, alef(skip))]
struct TreeSitterSchemas;

#[cfg(all(feature = "api", feature = "heuristics"))]
#[derive(OpenApi)]
#[openapi(components(schemas(crate::heuristics::confidence::ExtractionConfidence)))]
#[cfg_attr(alef, alef(skip))]
struct HeuristicsSchemas;

#[cfg(all(feature = "api", any(feature = "keywords-yake", feature = "keywords-rake")))]
#[derive(OpenApi)]
#[openapi(components(schemas(crate::keywords::types::Keyword)))]
#[cfg_attr(alef, alef(skip))]
struct KeywordSchemas;

// `/metrics` is only routed when the `prometheus` feature is enabled (it requires both
// `api` and `otel`; see the `prometheus` feature definition), so — same as the schema
// groups above — its path lives in its own gated `OpenApi` document rather than in
// `ApiDoc::paths(..)` directly, which would fail to compile without the feature.
#[cfg(all(feature = "api", feature = "prometheus"))]
#[derive(OpenApi)]
#[openapi(paths(crate::api::handlers::metrics_handler))]
#[cfg_attr(alef, alef(skip))]
struct PrometheusPaths;

/// Generate OpenAPI JSON schema.
///
/// Returns the complete OpenAPI 3.1 specification as a JSON string.
///
/// # Examples
///
/// ```no_run
/// use xberg::api::openapi::openapi_json;
///
/// let schema = openapi_json();
/// println!("{}", schema);
/// ```
#[cfg(feature = "api")]
#[cfg_attr(alef, alef(skip))]
pub fn openapi_json() -> String {
    #[allow(unused_mut)]
    let mut document = ApiDoc::openapi();

    #[cfg(feature = "tree-sitter")]
    document.merge(TreeSitterSchemas::openapi());
    #[cfg(feature = "heuristics")]
    document.merge(HeuristicsSchemas::openapi());
    #[cfg(any(feature = "keywords-yake", feature = "keywords-rake"))]
    document.merge(KeywordSchemas::openapi());
    #[cfg(feature = "prometheus")]
    document.merge(PrometheusPaths::openapi());

    document.to_pretty_json().unwrap_or_else(|_| "{}".to_string())
}

#[cfg(not(feature = "api"))]
#[cfg_attr(alef, alef(skip))]
pub(crate) fn openapi_json() -> String {
    r#"{"error": "API feature not enabled"}"#.to_string()
}

/// Recursively collect every `$ref` target string in a JSON value.
///
/// A `$ref` key is never descended into, so a schema that legitimately contains
/// a property literally named `$ref` cannot smuggle in a false positive.
#[cfg(all(test, feature = "api"))]
fn collect_refs(value: &serde_json::Value, found: &mut Vec<String>) {
    match value {
        serde_json::Value::Object(map) => {
            for (key, child) in map {
                if key == "$ref" {
                    if let Some(reference) = child.as_str() {
                        found.push(reference.to_string());
                    }
                } else {
                    collect_refs(child, found);
                }
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                collect_refs(item, found);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    #[cfg(feature = "api")]
    use super::*;

    #[test]
    #[cfg(feature = "api")]
    fn test_openapi_schema_generation() {
        let schema = openapi_json();
        assert!(!schema.is_empty());
        assert!(schema.contains("Xberg API"));
        assert!(schema.contains("/health"));
        assert!(schema.contains("/extract"));
    }

    #[test]
    #[cfg(feature = "api")]
    fn test_openapi_schema_valid_json() {
        let schema = openapi_json();
        let parsed: serde_json::Value = serde_json::from_str(&schema).expect("Invalid JSON");
        assert!(parsed.is_object());
        assert!(parsed["openapi"].is_string());
    }

    #[test]
    #[cfg(feature = "api")]
    fn test_openapi_includes_all_endpoints() {
        let schema = openapi_json();
        assert!(schema.contains("/health"));
        assert!(schema.contains("/info"));
        assert!(schema.contains("/version"));
        assert!(schema.contains("/extract"));
        assert!(schema.contains("/detect"));
        assert!(schema.contains("/cache/stats"));
        assert!(schema.contains("/cache/clear"));
        assert!(schema.contains("/cache/manifest"));
        assert!(schema.contains("/cache/warm"));
    }

    /// The async job endpoints are routed, so they must also be documented.
    ///
    /// `/extract-async` and `/jobs/{job_id}` are live routes in `create_router`,
    /// but they were absent from `ApiDoc`'s `paths(..)` list, making the entire
    /// async workflow invisible to `/openapi.json` and to every generated client
    /// (#249).
    #[test]
    #[cfg(feature = "api")]
    fn should_document_async_job_endpoints_in_openapi() {
        let document: serde_json::Value =
            serde_json::from_str(&openapi_json()).expect("OpenAPI document must be valid JSON");
        assert!(document["paths"].is_object(), "document must have a paths object");
        // Index the `Value`, not the inner `Map`: a missing key yields `Null`
        // (a clean assertion failure) instead of panicking inside the indexer.
        let paths = &document["paths"];

        let async_operations: Vec<&str> = ["get", "post", "put", "delete"]
            .into_iter()
            .filter(|method| paths["/extract-async"].get(*method).is_some())
            .collect();
        assert_eq!(
            async_operations,
            vec!["post"],
            "/extract-async must expose exactly POST"
        );

        let job_operations: Vec<&str> = ["get", "post", "put", "delete"]
            .into_iter()
            .filter(|method| paths["/jobs/{job_id}"].get(*method).is_some())
            .collect();
        assert_eq!(
            job_operations,
            vec!["get", "delete"],
            "/jobs/{{job_id}} must expose exactly GET and DELETE"
        );

        // Collect refs from the 202 subtree rather than asserting one fixed JSON
        // path: utoipa may emit the reference bare or wrapped (e.g. in `allOf`),
        // and the property under test is which schema it points at, not the shape.
        let mut accepted_refs = Vec::new();
        collect_refs(&paths["/extract-async"]["post"]["responses"]["202"], &mut accepted_refs);
        assert_eq!(
            accepted_refs,
            vec!["#/components/schemas/AsyncJobResponse"],
            "the 202 response must reference exactly AsyncJobResponse"
        );

        let schemas = document["components"]["schemas"]
            .as_object()
            .expect("document must have component schemas");
        for name in ["AsyncJobResponse", "JobStatus", "JobState"] {
            assert!(schemas.contains_key(name), "component schema '{name}' must be defined");
        }
    }

    /// `/extract` and `/extract-async` both reject an unsupported request `Content-Type`
    /// with HTTP 415 (see `UnifiedExtractRequest::from_request`), but the OpenAPI document
    /// did not declare 415 as a possible response for either operation — an undocumented
    /// status code that fails contract testing (schemathesis) even though the handler
    /// behavior is correct.
    #[test]
    #[cfg(feature = "api")]
    fn should_declare_415_for_unsupported_content_type_on_extract_operations() {
        let document: serde_json::Value =
            serde_json::from_str(&openapi_json()).expect("OpenAPI document must be valid JSON");
        let paths = &document["paths"];

        for (path, method) in [("/extract", "post"), ("/extract-async", "post")] {
            let responses = &paths[path][method]["responses"];
            assert_eq!(
                responses["415"]["description"].as_str(),
                Some("Unsupported Content-Type"),
                "{path} {method} must declare a 415 response"
            );

            let mut refs = Vec::new();
            collect_refs(&responses["415"], &mut refs);
            assert_eq!(
                refs,
                vec!["#/components/schemas/ErrorResponse"],
                "{path} {method}'s 415 response must reference exactly ErrorResponse"
            );
        }
    }

    /// Every `$ref` in the document must resolve to a node that actually exists.
    ///
    /// utoipa only emits a component schema for a type listed in `components(schemas(..))`.
    /// A response or nested field that references an unlisted type still emits a
    /// `$ref` to it, producing a document that parses as JSON but breaks every
    /// client generator that follows the pointer. Nothing else in the suite
    /// dereferences these pointers, so this test is the only thing standing
    /// between us and a shipped-but-unusable spec (#251).
    #[test]
    #[cfg(feature = "api")]
    fn should_resolve_every_openapi_ref_to_a_defined_component() {
        let document: serde_json::Value =
            serde_json::from_str(&openapi_json()).expect("OpenAPI document must be valid JSON");

        let mut references = Vec::new();
        collect_refs(&document, &mut references);
        references.sort();
        references.dedup();

        assert!(
            !references.is_empty(),
            "document contains no $ref entries at all — the walker is broken or the spec is empty"
        );

        let dangling: Vec<String> = references
            .iter()
            .filter(|reference| {
                // Only local pointers (`#/...`) are resolvable in-document; an
                // external/remote $ref is unresolvable here and counts as dangling.
                reference
                    .strip_prefix('#')
                    .and_then(|pointer| document.pointer(pointer))
                    .is_none()
            })
            .cloned()
            .collect();

        assert_eq!(
            dangling,
            Vec::<String>::new(),
            "these $ref targets are missing from the OpenAPI document; add the type to components(schemas(..))"
        );
    }

    #[test]
    #[cfg(feature = "api")]
    fn test_openapi_includes_schemas() {
        let schema = openapi_json();
        assert!(schema.contains("HealthResponse"));
        assert!(schema.contains("ErrorResponse"));
        assert!(schema.contains("VersionResponse"));
        assert!(schema.contains("DetectResponse"));
        assert!(schema.contains("ManifestResponse"));
        assert!(schema.contains("WarmRequest"));
        assert!(schema.contains("WarmResponse"));
    }
}
