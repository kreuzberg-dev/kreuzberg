//! MCP request parameter types.
//!
//! This module defines the parameter structures for all MCP tool calls.

use rmcp::schemars;
#[cfg_attr(alef, alef(skip))]
/// Request parameters for unified extraction.
#[derive(Debug, serde::Deserialize, serde::Serialize, schemars::JsonSchema)]
pub struct ExtractParams {
    /// Unified extraction input: bytes or URI.
    #[schemars(schema_with = "extract_input_schema")]
    pub input: serde_json::Value,
    /// Extraction configuration (JSON object)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config: Option<serde_json::Value>,
    /// Password for encrypted PDFs
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pdf_password: Option<String>,
    /// Wire format for the response: "json" (default) or "toon"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_format: Option<String>,
}
#[cfg_attr(alef, alef(skip))]
/// Request parameters for unified batch extraction.
#[derive(Debug, serde::Deserialize, serde::Serialize, schemars::JsonSchema)]
pub struct ExtractBatchParams {
    /// Unified extraction inputs: bytes and/or URIs.
    #[schemars(schema_with = "extract_inputs_schema")]
    pub inputs: Vec<serde_json::Value>,
    /// Extraction configuration (JSON object)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config: Option<serde_json::Value>,
    /// Password for encrypted PDFs
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pdf_password: Option<String>,
    /// Wire format for the response: "json" (default) or "toon"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_format: Option<String>,
}

/// Request parameters for MIME type detection.
#[cfg_attr(alef, alef(skip))]
#[derive(Debug, serde::Deserialize, serde::Serialize, schemars::JsonSchema)]
pub struct DetectMimeTypeParams {
    /// Path to the file
    pub path: String,
    /// Use content-based detection (default: true)
    #[serde(default = "default_use_content")]
    pub use_content: bool,
}

fn default_use_content() -> bool {
    true
}

/// Empty parameters for tools that take no arguments.
///
/// This generates `{"type": "object", "properties": {}}` which is required by
/// the MCP specification, unlike `()` which generates `{"const": null}`.
#[derive(Debug, serde::Deserialize, serde::Serialize, schemars::JsonSchema)]
pub struct EmptyParams {}

/// Request parameters for cache warm (model download).
#[cfg_attr(alef, alef(skip))]
#[derive(Debug, serde::Deserialize, serde::Serialize, schemars::JsonSchema)]
pub struct CacheWarmParams {
    /// Download all embedding model presets
    #[serde(default)]
    pub all_embeddings: bool,
    /// Specific embedding preset name to download (e.g. "balanced", "speed", "quality")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub embedding_model: Option<String>,
    /// Download the default GLiNER NER model into the standard Hugging Face cache
    #[serde(default)]
    pub ner: bool,
    /// Download every known GLiNER NER model into the standard Hugging Face cache
    #[serde(default)]
    pub all_ner_models: bool,
    /// Specific GLiNER NER model alias or catalog id to download into the standard Hugging Face cache
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ner_model: Option<String>,
}

fn extract_input_schema(_generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schemars::json_schema!({
        "type": "object",
        "required": ["kind"],
        "additionalProperties": false,
        "properties": {
            "kind": {
                "type": "string",
                "enum": ["bytes", "uri"],
                "description": "Input source kind."
            },
            "bytes": {
                "type": "array",
                "items": { "type": "integer", "minimum": 0, "maximum": 255 },
                "description": "Raw bytes for kind=bytes."
            },
            "uri": {
                "type": "string",
                "description": "Local path, file:// URI, or HTTP(S) URL for kind=uri."
            },
            "mime_type": {
                "type": "string",
                "description": "Optional MIME type hint."
            },
            "filename": {
                "type": "string",
                "description": "Optional filename hint for bytes inputs."
            },
            "config": {
                "type": "object",
                "description": "Optional per-input extraction overrides."
            }
        }
    })
}

fn extract_inputs_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
    let input_schema = extract_input_schema(generator);
    schemars::json_schema!({
        "type": "array",
        "items": input_schema
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_params_defaults() {
        let json = r#"{"input": {"kind": "uri", "uri": "/test.pdf"}}"#;
        let params: ExtractParams = serde_json::from_str(json).unwrap();

        assert_eq!(params.input["uri"], "/test.pdf");
        assert_eq!(params.config, None);
    }

    #[test]
    fn test_extract_params_accepts_bytes_input() {
        let json = r#"{"input": {"kind": "bytes", "bytes": [72, 105], "mime_type": "text/plain"}}"#;
        let params: ExtractParams = serde_json::from_str(json).unwrap();

        assert_eq!(params.input["kind"], "bytes");
        assert_eq!(params.config, None);
    }

    #[test]
    fn test_extract_batch_params_defaults() {
        let json = r#"{"inputs": [{"kind": "uri", "uri": "/a.pdf"}, {"kind": "uri", "uri": "/b.pdf"}]}"#;
        let params: ExtractBatchParams = serde_json::from_str(json).unwrap();

        assert_eq!(params.inputs.len(), 2);
        assert_eq!(params.config, None);
    }

    #[test]
    fn test_detect_mime_type_params_defaults() {
        let json = r#"{"path": "/test.pdf"}"#;
        let params: DetectMimeTypeParams = serde_json::from_str(json).unwrap();

        assert_eq!(params.path, "/test.pdf");
        assert!(params.use_content);
    }

    #[test]
    fn test_detect_mime_type_params_use_content_false() {
        let json = r#"{"path": "/test.pdf", "use_content": false}"#;
        let params: DetectMimeTypeParams = serde_json::from_str(json).unwrap();

        assert!(!params.use_content);
    }

    #[test]
    fn test_extract_params_with_config() {
        let json = r#"{"input": {"kind": "uri", "uri": "/test.pdf"}, "config": {"use_cache": false}}"#;
        let params: ExtractParams = serde_json::from_str(json).unwrap();

        assert_eq!(params.input["uri"], "/test.pdf");
        assert!(params.config.is_some());
    }

    #[test]
    fn test_extract_params_serialization() {
        let params = ExtractParams {
            input: serde_json::json!({
                "kind": "uri",
                "uri": "/test.pdf",
                "mime_type": "application/pdf"
            }),
            config: Some(serde_json::json!({"use_cache": false})),
            pdf_password: None,
            response_format: None,
        };

        let json = serde_json::to_string(&params).unwrap();
        let deserialized: ExtractParams = serde_json::from_str(&json).unwrap();

        assert_eq!(params.input, deserialized.input);
        assert_eq!(params.config, deserialized.config);
    }

    #[test]
    fn test_extract_batch_params_serialization() {
        let params = ExtractBatchParams {
            inputs: vec![
                serde_json::json!({"kind": "uri", "uri": "/a.pdf"}),
                serde_json::json!({"kind": "uri", "uri": "/b.pdf"}),
            ],
            config: Some(serde_json::json!({"use_cache": true})),
            pdf_password: None,
            response_format: None,
        };

        let json = serde_json::to_string(&params).unwrap();
        let deserialized: ExtractBatchParams = serde_json::from_str(&json).unwrap();

        assert_eq!(params.inputs, deserialized.inputs);
        assert_eq!(params.config, deserialized.config);
    }

    #[test]
    fn test_detect_mime_type_params_serialization() {
        let params = DetectMimeTypeParams {
            path: "/test.pdf".to_string(),
            use_content: false,
        };

        let json = serde_json::to_string(&params).unwrap();
        let deserialized: DetectMimeTypeParams = serde_json::from_str(&json).unwrap();

        assert_eq!(params.path, deserialized.path);
        assert_eq!(params.use_content, deserialized.use_content);
    }

    #[test]
    fn test_empty_params_schema_has_type_object() {
        let schema = schemars::schema_for!(EmptyParams);
        let json = serde_json::to_value(&schema).unwrap();
        assert_eq!(json["type"], "object");
    }

    #[test]
    fn test_empty_params_deserializes_from_empty_object() {
        let params: EmptyParams = serde_json::from_str("{}").unwrap();
        let _ = params;
    }

    #[test]
    fn test_cache_warm_params_defaults() {
        let json = r#"{}"#;
        let params: CacheWarmParams = serde_json::from_str(json).unwrap();
        assert!(!params.all_embeddings);
        assert!(params.embedding_model.is_none());
        assert!(!params.ner);
        assert!(!params.all_ner_models);
        assert!(params.ner_model.is_none());
    }

    #[test]
    fn test_cache_warm_params_with_values() {
        let json = r#"{
            "all_embeddings": true,
            "embedding_model": "balanced",
            "ner": true,
            "all_ner_models": true,
            "ner_model": "gliner_small-v2.5"
        }"#;
        let params: CacheWarmParams = serde_json::from_str(json).unwrap();
        assert!(params.all_embeddings);
        assert_eq!(params.embedding_model.as_deref(), Some("balanced"));
        assert!(params.ner);
        assert!(params.all_ner_models);
        assert_eq!(params.ner_model.as_deref(), Some("gliner_small-v2.5"));
    }
}
