//! Structured data extractor (JSON, JSONL, YAML, TOML).

use crate::Result;
use crate::core::config::ExtractionConfig;
use crate::extractors::security::SecurityBudget;
use crate::plugins::{InternalDocumentExtractor, Plugin};
use crate::types::internal::InternalDocument;
use crate::types::internal_builder::InternalDocumentBuilder;
use crate::types::metadata::Metadata;
use ahash::AHashMap;
use async_trait::async_trait;
use std::borrow::Cow;
#[cfg(feature = "tokio-runtime")]
use std::path::Path;

/// Build an `InternalDocument` from a structured data result.
///
/// For JSON objects: top-level keys become headings, nested objects become
/// sub-headings, arrays become lists. Falls back to a code block for other formats.
///
/// `budget` enforces hostile-input limits (nesting depth, iteration count, entity
/// length, cumulative content size). Any limit violation is converted into a
/// `XbergError::Security` via the `?` operator.
fn build_internal_document(
    result: &crate::extraction::structured::StructuredDataResult,
    mime_type: &str,
    budget: &mut SecurityBudget,
) -> Result<InternalDocument> {
    let source_format = match mime_type {
        "application/json" | "text/json" | "application/csl+json" => "json",
        "application/x-ndjson" | "application/jsonl" | "application/x-jsonlines" => "jsonl",
        "application/yaml" | "application/x-yaml" | "text/yaml" | "text/x-yaml" => "yaml",
        "application/toml" | "text/toml" => "toml",
        _ => "structured",
    };

    let language = match source_format {
        "json" | "jsonl" => Some("json"),
        "yaml" => Some("yaml"),
        "toml" => Some("toml"),
        _ => None,
    };

    let mut builder = InternalDocumentBuilder::new(source_format);

    // Render document structure (headings, sub-headings, lists) from the parsed value for
    // every structured format, not just JSON objects: YAML, TOML, and JSONL already parse
    // into the same `serde_json::Value` shape, and a top-level array (JSONL's natural shape,
    // and valid JSON on its own) gets per-item structure instead of an opaque code block
    // (xberg-io/xberg#155).
    if matches!(source_format, "json" | "jsonl" | "yaml" | "toml")
        && let Some(value) = result.value.as_ref()
    {
        match value {
            serde_json::Value::Object(_) => {
                build_json_internal_structure(value, &mut builder, 1, budget)?;
                return Ok(builder.build());
            }
            serde_json::Value::Array(items) if !items.is_empty() => {
                build_json_array(items, &mut builder, 1, budget)?;
                return Ok(builder.build());
            }
            _ => {}
        }
    }

    budget.account_text(result.content.len())?;
    builder.push_code(&result.content, language, None, None);
    Ok(builder.build())
}

/// Recursively build internal document structure from a JSON value.
///
/// `budget` enforces nesting depth, iteration count, entity length, and
/// cumulative text size limits against hostile input.
fn build_json_internal_structure(
    value: &serde_json::Value,
    builder: &mut InternalDocumentBuilder,
    depth: u8,
    budget: &mut SecurityBudget,
) -> Result<()> {
    let level = depth.min(6);
    match value {
        serde_json::Value::Object(map) => {
            budget.enter()?;
            for (key, val) in map {
                budget.step()?;
                budget.check_entity(key)?;
                match val {
                    serde_json::Value::Object(_) => {
                        builder.push_heading(level, key, None, None);
                        build_json_internal_structure(val, builder, depth + 1, budget)?;
                    }
                    serde_json::Value::Array(arr) => {
                        builder.push_heading(level, key, None, None);
                        build_json_array(arr, builder, depth + 1, budget)?;
                    }
                    serde_json::Value::String(s) => {
                        budget.check_entity(s)?;
                        let rendered = format!("{}: {}", key, s);
                        budget.account_text(rendered.len())?;
                        builder.push_paragraph(&rendered, vec![], None, None);
                    }
                    other => {
                        let rendered = format!("{}: {}", key, other);
                        budget.account_text(rendered.len())?;
                        builder.push_paragraph(&rendered, vec![], None, None);
                    }
                }
            }
            budget.leave();
        }
        serde_json::Value::Array(arr) => {
            build_json_array(arr, builder, depth, budget)?;
        }
        serde_json::Value::String(s) => {
            budget.check_entity(s)?;
            budget.account_text(s.len())?;
            builder.push_paragraph(s, vec![], None, None);
        }
        other => {
            let rendered = other.to_string();
            budget.account_text(rendered.len())?;
            builder.push_paragraph(&rendered, vec![], None, None);
        }
    }
    Ok(())
}

/// Render array scalars as list items and recursively expand structured items.
///
/// Lists are closed before an object or nested array is rendered so headings
/// and paragraphs do not become implicit children of the preceding list item.
fn build_json_array(
    values: &[serde_json::Value],
    builder: &mut InternalDocumentBuilder,
    depth: u8,
    budget: &mut SecurityBudget,
) -> Result<()> {
    const ARRAY_ITEM_LABEL: &str = "Item";

    budget.enter()?;
    let mut list_is_open = false;
    for (index, value) in values.iter().enumerate() {
        budget.step()?;
        match value {
            serde_json::Value::Object(_) | serde_json::Value::Array(_) => {
                if list_is_open {
                    builder.end_list();
                    list_is_open = false;
                }
                let label = format!("{ARRAY_ITEM_LABEL} {}", index + 1);
                budget.account_text(label.len())?;
                builder.push_heading(depth.min(6), &label, None, None);
                build_json_internal_structure(value, builder, depth + 1, budget)?;
            }
            serde_json::Value::String(text) => {
                budget.check_entity(text)?;
                budget.account_text(text.len())?;
                if !list_is_open {
                    builder.push_list(false);
                    list_is_open = true;
                }
                builder.push_list_item(text, false, vec![], None, None);
            }
            scalar => {
                let rendered = scalar.to_string();
                budget.account_text(rendered.len())?;
                if !list_is_open {
                    builder.push_list(false);
                    list_is_open = true;
                }
                builder.push_list_item(&rendered, false, vec![], None, None);
            }
        }
    }
    if list_is_open {
        builder.end_list();
    }
    budget.leave();
    Ok(())
}

/// Structured data extractor supporting JSON, JSONL/NDJSON, YAML, and TOML.
#[cfg_attr(alef, alef(skip))]
pub struct StructuredExtractor;

impl Default for StructuredExtractor {
    fn default() -> Self {
        Self::new()
    }
}

impl StructuredExtractor {
    pub(crate) fn new() -> Self {
        Self
    }
}

impl Plugin for StructuredExtractor {
    fn name(&self) -> &str {
        "structured-extractor"
    }

    fn version(&self) -> String {
        env!("CARGO_PKG_VERSION").to_string()
    }

    fn initialize(&self) -> Result<()> {
        Ok(())
    }

    fn shutdown(&self) -> Result<()> {
        Ok(())
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
impl InternalDocumentExtractor for StructuredExtractor {
    #[cfg_attr(feature = "otel", tracing::instrument(
        skip(self, content, config),
        fields(
            extractor.name = self.name(),
            content.size_bytes = content.len(),
        )
    ))]
    async fn extract_content(
        &self,
        content: &[u8],
        mime_type: &str,
        config: &ExtractionConfig,
    ) -> Result<InternalDocument> {
        let structured_result = match mime_type {
            "application/json" | "text/json" | "application/csl+json" => {
                crate::extraction::structured::parse_json(content, None)?
            }
            "application/x-ndjson" | "application/jsonl" | "application/x-jsonlines" => {
                crate::extraction::structured::parse_jsonl(content, None)?
            }
            "application/yaml" | "application/x-yaml" | "text/yaml" | "text/x-yaml" => {
                crate::extraction::structured::parse_yaml(content)?
            }
            "application/toml" | "text/toml" => crate::extraction::structured::parse_toml(content)?,
            _ => return Err(crate::XbergError::UnsupportedFormat(mime_type.to_string())),
        };

        let mut additional = AHashMap::new();
        additional.insert(
            Cow::Borrowed("field_count"),
            serde_json::json!(structured_result.text_fields.len()),
        );
        additional.insert(
            Cow::Borrowed("data_format"),
            serde_json::json!(structured_result.format),
        );
        // Surface the full flattened `path: value` view instead of discarding it
        // (xberg-io/xberg#166): the structured renderer above only emits headings/lists for
        // a subset of shapes, so this is the one place a consumer can always get every leaf
        // field as text, regardless of source format or nesting.
        if !structured_result.flattened.is_empty() {
            additional.insert(
                Cow::Borrowed("flattened_fields"),
                serde_json::json!(structured_result.flattened),
            );
        }

        for (key, value) in &structured_result.metadata {
            additional.insert(Cow::Owned(key.clone()), serde_json::json!(value));
        }

        let mut budget = SecurityBudget::from_config(config);
        let mut doc = build_internal_document(&structured_result, mime_type, &mut budget)?;
        doc.mime_type = mime_type.to_string();

        doc.metadata = Metadata {
            additional,
            ..Default::default()
        };

        Ok(doc)
    }

    #[cfg(feature = "tokio-runtime")]
    #[cfg_attr(feature = "otel", tracing::instrument(
        skip(self, path, config),
        fields(
            extractor.name = self.name(),
        )
    ))]
    async fn extract_path(&self, path: &Path, mime_type: &str, config: &ExtractionConfig) -> Result<InternalDocument> {
        let bytes = crate::core::io::read_file_async(path).await?;
        self.extract_content(&bytes, mime_type, config).await
    }

    fn supported_mime_types(&self) -> &[&str] {
        &[
            "application/json",
            "text/json",
            "application/csl+json",
            "application/x-ndjson",
            "application/jsonl",
            "application/x-jsonlines",
            "application/yaml",
            "application/x-yaml",
            "text/yaml",
            "text/x-yaml",
            "application/toml",
            "text/toml",
        ]
    }

    fn priority(&self) -> i32 {
        50
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_json_array_objects_render_as_nested_markdown() {
        let value = serde_json::json!({
            "people": [
                {
                    "name": "Ada",
                    "details": {
                        "role": "Engineer",
                        "active": true
                    }
                },
                {
                    "name": "Grace",
                    "skills": ["compilers", "mathematics"]
                }
            ]
        });
        let mut builder = InternalDocumentBuilder::new("json");
        let mut budget = SecurityBudget::from_config(&ExtractionConfig::default());

        build_json_internal_structure(&value, &mut builder, 1, &mut budget).unwrap();
        let markdown = crate::rendering::render_markdown(&builder.build());

        assert!(markdown.contains("# people"), "missing array heading: {markdown}");
        assert!(markdown.contains("## Item 1"), "missing first item heading: {markdown}");
        assert!(markdown.contains("name: Ada"), "missing first nested value: {markdown}");
        assert!(
            markdown.contains("### details"),
            "missing nested object heading: {markdown}"
        );
        assert!(
            markdown.contains("role: Engineer"),
            "missing deeply nested value: {markdown}"
        );
        assert!(markdown.contains("active: true"), "missing boolean value: {markdown}");
        assert!(
            markdown.contains("## Item 2"),
            "missing second item heading: {markdown}"
        );
        assert!(
            markdown.contains("name: Grace"),
            "missing second nested value: {markdown}"
        );
        assert!(
            markdown.contains("- compilers"),
            "missing nested array value: {markdown}"
        );
        assert!(
            markdown.contains("- mathematics"),
            "missing nested array value: {markdown}"
        );
        assert!(
            !markdown.contains(r#"{\"name\":\"Ada\""#),
            "object remained compact JSON: {markdown}"
        );
    }

    #[test]
    fn test_structured_extractor_plugin_interface() {
        let extractor = StructuredExtractor::new();
        assert_eq!(extractor.name(), "structured-extractor");
        assert!(extractor.initialize().is_ok());
        assert!(extractor.shutdown().is_ok());
    }

    #[test]
    fn test_structured_extractor_supported_mime_types() {
        let extractor = StructuredExtractor::new();
        let mime_types = extractor.supported_mime_types();
        assert_eq!(mime_types.len(), 12);
        assert!(mime_types.contains(&"application/json"));
        assert!(mime_types.contains(&"application/x-ndjson"));
        assert!(mime_types.contains(&"application/jsonl"));
        assert!(mime_types.contains(&"application/x-jsonlines"));
        assert!(mime_types.contains(&"application/x-yaml"));
        assert!(mime_types.contains(&"application/toml"));
        assert!(mime_types.contains(&"application/csl+json"));
    }
}
