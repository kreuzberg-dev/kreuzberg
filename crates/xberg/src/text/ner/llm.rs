//! liter-llm zero-shot NER backend.
//!
//! Uses a fixed JSON-schema prompt to coax any chat model into producing
//! `[{text, category, start, end}]` arrays. The output is reconciled with the
//! source text — entities whose `text` field is not actually a substring of
//! the input are dropped so the caller never gets phantom offsets, and every
//! occurrence of a mention is reported, not just the first.

use std::collections::HashSet;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::Result;
use crate::core::config::llm::LlmConfig;
use crate::types::entity::{Entity, EntityCategory};

use super::backend::NerBackend;
use super::offsets;

/// Word-token budget for one NER request.
///
/// A whole document in a single prompt either overruns the model's context
/// window or gets its tail quietly ignored; either way the entities in the tail
/// are never detected and the PII they cover is never redacted
/// (xberg-io/xberg#262). Long input is windowed instead.
const LLM_WINDOW_TOKENS: usize = 2_000;

/// Token overlap between adjacent request windows, so a mention sitting on a
/// window boundary is still seen whole by one of them.
const LLM_WINDOW_OVERLAP_TOKENS: usize = 128;

/// liter-llm-backed NER backend.
#[derive(Debug, Clone)]
#[cfg_attr(alef, alef(skip))]
pub struct LlmBackend {
    config: LlmConfig,
}

impl LlmBackend {
    /// Create a new LLM-backed NER backend with the given LLM configuration.
    pub fn new(config: LlmConfig) -> Self {
        Self { config }
    }
}

#[derive(Debug, Deserialize)]
struct EntityWire {
    text: String,
    category: String,
    #[serde(default)]
    confidence: Option<f32>,
}

#[derive(Debug, Deserialize)]
struct EntityListWire {
    entities: Vec<EntityWire>,
}

#[async_trait]
impl NerBackend for LlmBackend {
    async fn detect(&self, text: &str, categories: &[EntityCategory]) -> Result<Vec<Entity>> {
        self.detect_with_custom(text, categories, &[]).await
    }

    /// Detect entities, windowing long input and reporting every occurrence.
    ///
    /// Two fixes live in this loop:
    ///
    /// - Each mention the model reports is resolved against the **whole**
    ///   source, not just its window, and every occurrence becomes an entity.
    ///   Resolving only the first hit meant the second and third copy of the
    ///   same name stayed in the redacted output (xberg-io/xberg#200).
    /// - The source is windowed so a long document does not lose its tail
    ///   (xberg-io/xberg#262).
    ///
    /// A failed window aborts the whole call: a partial entity stream would
    /// under-redact silently, which is worse than a surfaced error.
    async fn detect_with_custom(
        &self,
        text: &str,
        categories: &[EntityCategory],
        custom_labels: &[String],
    ) -> Result<Vec<Entity>> {
        let windows = offsets::split_into_windows(text, LLM_WINDOW_TOKENS, LLM_WINDOW_OVERLAP_TOKENS);
        if windows.is_empty() {
            return Ok(Vec::new());
        }

        let label_lookup: std::collections::HashMap<String, String> = custom_labels
            .iter()
            .map(|l| (l.to_ascii_lowercase(), l.clone()))
            .collect();

        let mut resolved: HashSet<(EntityCategory, String)> = HashSet::new();
        let mut out: Vec<Entity> = Vec::new();

        for window in &windows {
            let (value, _usage) =
                complete_with_json_schema(&self.config, &window.text, categories, custom_labels).await?;

            let wire: EntityListWire = serde_json::from_value(value)
                .map_err(|e| crate::XbergError::parsing(format!("LLM NER backend returned malformed JSON: {e}")))?;

            for ent in wire.entities {
                let category = parse_category(&ent.category, &label_lookup);
                if !resolved.insert((category.clone(), ent.text.clone())) {
                    continue;
                }
                out.extend(offsets::entities_for_every_occurrence(
                    text,
                    &ent.text,
                    category,
                    ent.confidence,
                ));
            }
        }

        out.sort_by_key(|e| (e.start, e.end));
        out.dedup_by(|a, b| a.start == b.start && a.end == b.end && a.category == b.category);
        Ok(out)
    }
}

fn parse_category(raw: &str, custom_lookup: &std::collections::HashMap<String, String>) -> EntityCategory {
    let lower = raw.to_ascii_lowercase();
    match lower.as_str() {
        "person" => EntityCategory::Person,
        "organization" | "org" => EntityCategory::Organization,
        "location" | "place" => EntityCategory::Location,
        "date" => EntityCategory::Date,
        "time" => EntityCategory::Time,
        "money" => EntityCategory::Money,
        "percent" => EntityCategory::Percent,
        "email" => EntityCategory::Email,
        "phone" => EntityCategory::Phone,
        "url" => EntityCategory::Url,
        _ => {
            if let Some(label) = custom_lookup.get(&lower) {
                EntityCategory::Custom(label.clone())
            } else {
                EntityCategory::Custom(raw.to_string())
            }
        }
    }
}

/// Inline structured-output helper.
///
/// Stream A owns the canonical `crate::llm::structured` helper; until that
/// signature stabilises we ship our own thin wrapper here so the NER backend
/// is self-contained and can be feature-tested independently.
async fn complete_with_json_schema(
    config: &LlmConfig,
    text: &str,
    categories: &[EntityCategory],
    custom_labels: &[String],
) -> Result<(Value, Option<crate::types::LlmUsage>)> {
    use liter_llm::LlmClient;

    let client = crate::llm::client::create_client(config)?;

    let mut category_strings: Vec<String> = if categories.is_empty() && custom_labels.is_empty() {
        ["person", "organization", "location", "date", "email", "phone", "url"]
            .iter()
            .map(|s| s.to_string())
            .collect()
    } else {
        categories.iter().map(category_label).collect()
    };
    for label in custom_labels {
        if !category_strings.iter().any(|c| c.eq_ignore_ascii_case(label)) {
            category_strings.push(label.clone());
        }
    }

    let category_list = category_strings.join(", ");
    let prompt = format!(
        "Identify named entities in the text. For each entity, return:\n\
         - text: the exact mention as it appears in the input\n\
         - category: one of {category_list}\n\
         - confidence: a number between 0 and 1\n\n\
         Return JSON only, conforming to the supplied schema.\n\n\
         TEXT:\n{text}"
    );

    let schema = json!({
        "type": "object",
        "properties": {
            "entities": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "text": { "type": "string" },
                        "category": { "type": "string" },
                        "confidence": { "type": "number" }
                    },
                    "required": ["text", "category"]
                }
            }
        },
        "required": ["entities"]
    });

    let request = liter_llm::ChatCompletionRequest {
        model: config.model.clone(),
        messages: vec![liter_llm::Message::User(liter_llm::UserMessage {
            content: liter_llm::UserContent::Text(prompt),
            name: None,
        })],
        temperature: config.temperature,
        max_tokens: config.max_tokens,
        reasoning_effort: crate::llm::client::parse_reasoning_effort(config)?,
        extra_body: config.extra_body.clone(),
        response_format: Some(liter_llm::ResponseFormat::JsonSchema {
            json_schema: liter_llm::JsonSchemaFormat {
                name: "entity_list".to_string(),
                description: Some("Named entity recognition output.".to_string()),
                schema,
                strict: Some(true),
            },
        }),
        ..Default::default()
    };

    let response = client
        .chat(request)
        .await
        .map_err(|e| crate::XbergError::parsing(format!("LLM NER request failed: {e}")))?;

    let usage = crate::llm::usage::extract_usage_from_chat(&response, "ner");

    let raw = response
        .choices
        .first()
        .and_then(|c| c.message.content.as_ref().and_then(|m| m.as_text()))
        .ok_or_else(|| crate::XbergError::parsing("LLM NER returned no content".to_string()))?;

    let cleaned = raw
        .trim()
        .strip_prefix("```json")
        .or_else(|| raw.trim().strip_prefix("```"))
        .and_then(|s| s.strip_suffix("```"))
        .map(|s| s.trim())
        .unwrap_or(raw.trim());

    let value: Value = serde_json::from_str(cleaned)
        .map_err(|e| crate::XbergError::parsing(format!("LLM NER returned invalid JSON: {e}")))?;

    Ok((value, usage))
}

fn category_label(category: &EntityCategory) -> String {
    match category {
        EntityCategory::Person => "person".into(),
        EntityCategory::Organization => "organization".into(),
        EntityCategory::Location => "location".into(),
        EntityCategory::Date => "date".into(),
        EntityCategory::Time => "time".into(),
        EntityCategory::Money => "money".into(),
        EntityCategory::Percent => "percent".into(),
        EntityCategory::Email => "email".into(),
        EntityCategory::Phone => "phone".into(),
        EntityCategory::Url => "url".into(),
        EntityCategory::Custom(label) => label.clone(),
    }
}
