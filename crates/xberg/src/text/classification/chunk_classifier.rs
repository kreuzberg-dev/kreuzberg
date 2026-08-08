//! Per-chunk multi-label LLM classification driver.
//!
//! Iterates `ExtractedDocument::chunks`, groups them into bounded-size batches to
//! amortize the fixed cost of a (potentially large) label-definition prompt block,
//! and runs those batches with bounded concurrency against the configured LLM
//! using [`crate::llm::structured::complete_with_json_schema`].
//!
//! Unlike [`super::page_classifier`], every chunk always receives zero, one, or
//! many labels — there is no single-label mode.

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::{Value, json};

use crate::core::config::{ChunkClassificationConfig, ChunkClassificationDefinition};
use crate::types::classification::ClassificationLabel;
use crate::types::{ExtractedDocument, LlmUsage};

/// Stable, lowercase warning source for partial chunk-classification batch
/// failures (#264). See `core::diagnostics` for the convention.
const PARTIAL_FAILURE_WARNING_SOURCE: &str = "chunk_classification";

/// Default Jinja2 template used when `ChunkClassificationConfig::prompt_template`
/// is `None`. Variables: `definitions` (rendered label + description list),
/// `chunks` (numbered chunk texts in the current batch).
pub const DEFAULT_CHUNK_CLASSIFICATION_TEMPLATE: &str = "\
You are a precise document classification system operating on chunks of a larger \
document. Each chunk may match zero, one, or multiple of the following label \
definitions:

{{ definitions }}

For every numbered chunk below, return every label that applies. Order is not \
significant. If no label fits a chunk, return an empty list for it. Do not invent \
labels not in the list above.

Chunks:
{{ chunks }}

Respond as JSON that matches the provided schema, with one entry per chunk index.";

/// Render the definitions block: one `- label: description` line per entry.
fn render_definitions(definitions: &[ChunkClassificationDefinition]) -> String {
    definitions
        .iter()
        .map(|d| format!("- {}: {}", d.label, d.description))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Render the numbered chunk-text block for a single batch.
fn render_chunks_block(batch: &[(usize, String)]) -> String {
    batch
        .iter()
        .map(|(index, text)| format!("[{index}]\n{text}"))
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Build the JSON schema constraining the LLM's batch response.
///
/// Shape: `{"results": [{"index": int, "labels": [{"label": str, "confidence": float?}]}]}`.
fn build_schema(definitions: &[ChunkClassificationDefinition]) -> Value {
    let label_enum: Vec<Value> = definitions.iter().map(|d| Value::String(d.label.clone())).collect();
    let label_object = json!({
        "type": "object",
        "properties": {
            "label": { "type": "string", "enum": label_enum },
            "confidence": { "type": "number", "minimum": 0.0, "maximum": 1.0 },
        },
        "required": ["label"],
        "additionalProperties": false,
    });

    json!({
        "type": "object",
        "properties": {
            "results": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "index": { "type": "integer" },
                        "labels": { "type": "array", "items": label_object },
                    },
                    "required": ["index", "labels"],
                    "additionalProperties": false,
                },
            },
        },
        "required": ["results"],
        "additionalProperties": false,
    })
}

/// Parse the LLM's batch JSON response into a `chunk_index -> labels` map.
///
/// Entries whose `index` does not correspond to a chunk in the batch are kept as
/// keyed by whatever index the model reported; the caller only applies entries
/// that match a real chunk index, so a hallucinated index is silently dropped.
fn parse_batch_response(value: &Value) -> HashMap<usize, Vec<ClassificationLabel>> {
    let mut out = HashMap::new();
    let Some(results) = value.get("results").and_then(|v| v.as_array()) else {
        return out;
    };
    for entry in results {
        let Some(index) = entry.get("index").and_then(|v| v.as_u64()) else {
            continue;
        };
        let mut labels = Vec::new();
        if let Some(arr) = entry.get("labels").and_then(|v| v.as_array()) {
            for label_entry in arr {
                if let Some(label) = label_entry.get("label").and_then(|v| v.as_str()) {
                    labels.push(ClassificationLabel {
                        label: label.to_string(),
                        confidence: label_entry.get("confidence").and_then(|v| v.as_f64()).map(|f| f as f32),
                    });
                }
            }
        }
        out.insert(index as usize, labels);
    }
    out
}

/// Pre-rendered classification context shared across every batch.
struct ClassifyChunkContext {
    template: String,
    definitions_rendered: String,
    schema: Value,
}

impl ClassifyChunkContext {
    fn new(config: &ChunkClassificationConfig) -> Self {
        let template = config
            .prompt_template
            .clone()
            .unwrap_or_else(|| DEFAULT_CHUNK_CLASSIFICATION_TEMPLATE.to_string());
        Self {
            template,
            definitions_rendered: render_definitions(&config.definitions),
            schema: build_schema(&config.definitions),
        }
    }
}

/// Classify a single batch of `(chunk_index, chunk_text)` pairs.
///
/// Returns the parsed `chunk_index -> labels` map (only for chunks the model
/// actually reported) plus the LLM call's usage record, if any.
async fn classify_batch(
    batch: &[(usize, String)],
    ctx: &ClassifyChunkContext,
    llm_config: &crate::core::config::LlmConfig,
) -> crate::Result<(HashMap<usize, Vec<ClassificationLabel>>, Option<LlmUsage>)> {
    let chunks_rendered = render_chunks_block(batch);
    let render_ctx = minijinja::context! {
        definitions => &ctx.definitions_rendered,
        chunks => &chunks_rendered,
    };
    let prompt = crate::llm::prompts::render_template(&ctx.template, &render_ctx)?;

    let (value, usage) = crate::llm::structured::complete_with_json_schema(
        llm_config,
        &prompt,
        "chunk_classification_multi",
        &ctx.schema,
        "chunk_classification",
    )
    .await?;

    Ok((parse_batch_response(&value), usage))
}

/// Run chunk classification against an extraction result.
///
/// Mutates `ChunkMetadata::classifications` on every chunk in
/// `result.chunks` and appends every LLM call's usage to `result.llm_usage`.
/// A chunk whose classification batch call fails (or that the model omitted
/// from its response) is simply left with an empty `classifications` vector for
/// that chunk, unless the failure is a validation error (empty config) or every
/// batch task fails, in which case the first error is returned.
///
/// # Errors
///
/// Returns [`crate::XbergError::Validation`] when `config.definitions` is empty.
/// Returns the first batch error encountered when rendering the prompt or
/// calling the LLM fails for every batch; partial failures on a subset of
/// batches are recorded here as a `ProcessingWarning` on `result` instead of
/// aborting the whole run.
pub async fn classify_chunks(result: &mut ExtractedDocument, config: &ChunkClassificationConfig) -> crate::Result<()> {
    if config.definitions.is_empty() {
        return Err(crate::XbergError::validation(
            "ChunkClassificationConfig.definitions must contain at least one entry",
        ));
    }

    let Some(chunks) = result.chunks.as_ref() else {
        return Ok(());
    };
    if chunks.is_empty() {
        return Ok(());
    }

    let batch_size = config.batch_size.max(1);
    let max_concurrency = config.max_concurrency.max(1);

    let batches: Vec<Vec<(usize, String)>> = chunks
        .iter()
        .enumerate()
        .filter(|(_, chunk)| !chunk.content.is_empty())
        .map(|(index, chunk)| (index, chunk.content.clone()))
        .collect::<Vec<_>>()
        .chunks(batch_size)
        .map(<[(usize, String)]>::to_vec)
        .collect();

    if batches.is_empty() {
        return Ok(());
    }
    let total_batches = batches.len();

    let ctx = Arc::new(ClassifyChunkContext::new(config));
    let llm_config = Arc::new(config.llm.clone());
    let semaphore = Arc::new(tokio::sync::Semaphore::new(max_concurrency));

    let mut join_set = tokio::task::JoinSet::new();
    for batch in batches {
        let ctx = Arc::clone(&ctx);
        let llm_config = Arc::clone(&llm_config);
        let semaphore = Arc::clone(&semaphore);
        join_set.spawn(async move {
            let _permit = semaphore.acquire_owned().await.map_err(|_| {
                crate::XbergError::Other("chunk-classification concurrency semaphore closed unexpectedly".to_string())
            })?;
            classify_batch(&batch, &ctx, &llm_config).await
        });
    }

    let mut per_chunk: HashMap<usize, Vec<ClassificationLabel>> = HashMap::new();
    let mut usages: Vec<LlmUsage> = Vec::new();
    let mut first_error: Option<crate::XbergError> = None;
    let mut failed_batches = 0usize;
    let mut any_success = false;

    while let Some(joined) = join_set.join_next().await {
        let batch_result = match joined {
            Ok(inner) => inner,
            Err(join_err) => Err(crate::XbergError::Other(format!(
                "chunk classification batch task failed to complete: {join_err}"
            ))),
        };
        match batch_result {
            Ok((labels_by_index, usage)) => {
                any_success = true;
                per_chunk.extend(labels_by_index);
                if let Some(u) = usage {
                    usages.push(u);
                }
            }
            Err(err) => {
                failed_batches += 1;
                if first_error.is_none() {
                    first_error = Some(err);
                }
            }
        }
    }

    if !any_success && let Some(err) = first_error {
        return Err(err);
    }

    if let Some(err) = &first_error {
        let message = partial_batch_failure_message(failed_batches, total_batches, err);
        crate::core::diagnostics::push_warning(
            &mut result.processing_warnings,
            PARTIAL_FAILURE_WARNING_SOURCE,
            message,
        );
    }

    if let Some(chunks_mut) = result.chunks.as_mut() {
        apply_batch_results(chunks_mut, per_chunk);
    }

    if !usages.is_empty() {
        result.llm_usage.get_or_insert_with(Vec::new).extend(usages);
    }

    Ok(())
}

/// Build the warning message for a chunk-classification run where some
/// batches failed but at least one succeeded. The chunks belonging to the
/// failed batches are left with an empty `classifications` vector, which is
/// otherwise indistinguishable from "classified, no label matched" (#264).
fn partial_batch_failure_message(
    failed_batches: usize,
    total_batches: usize,
    last_error: &crate::XbergError,
) -> String {
    format!(
        "{failed_batches} of {total_batches} chunk-classification batch{} failed; \
         chunks in the failed batch{} were left without classifications (last error: {last_error})",
        if total_batches == 1 { "" } else { "es" },
        if failed_batches == 1 { "" } else { "es" },
    )
}

/// Write each batch's parsed labels onto the matching chunk's
/// `ChunkMetadata::classifications`. Chunks with no entry in `per_chunk`
/// (skipped as empty, or dropped by a failed batch) are left untouched, so
/// their `classifications` stays whatever it already was (empty, by
/// construction, before this stage runs).
fn apply_batch_results(chunks: &mut [crate::types::Chunk], mut per_chunk: HashMap<usize, Vec<ClassificationLabel>>) {
    for (index, chunk) in chunks.iter_mut().enumerate() {
        if let Some(labels) = per_chunk.remove(&index) {
            chunk.metadata.classifications = labels;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn definitions() -> Vec<ChunkClassificationDefinition> {
        vec![
            ChunkClassificationDefinition {
                label: "director_appointment".to_string(),
                description: "A person is appointed, elected, or designated as a director.".to_string(),
            },
            ChunkClassificationDefinition {
                label: "director_resignation".to_string(),
                description: "A director resigned, retired, or was removed.".to_string(),
            },
            ChunkClassificationDefinition {
                label: "registered_office_change".to_string(),
                description: "The registered office or legal address of an entity changes.".to_string(),
            },
        ]
    }

    #[test]
    fn build_schema_uses_labels_array_with_enum() {
        let schema = build_schema(&definitions());
        assert_eq!(schema["properties"]["results"]["type"], "array");
        let label_enum =
            &schema["properties"]["results"]["items"]["properties"]["labels"]["items"]["properties"]["label"]["enum"];
        assert_eq!(label_enum.as_array().unwrap().len(), 3);
    }

    #[test]
    fn render_definitions_joins_label_and_description() {
        let rendered = render_definitions(&definitions());
        assert_eq!(
            rendered,
            "- director_appointment: A person is appointed, elected, or designated as a director.\n\
             - director_resignation: A director resigned, retired, or was removed.\n\
             - registered_office_change: The registered office or legal address of an entity changes."
        );
    }

    #[test]
    fn render_chunks_block_numbers_each_chunk() {
        let batch = vec![(0usize, "alpha text".to_string()), (3usize, "beta text".to_string())];
        let rendered = render_chunks_block(&batch);
        assert_eq!(rendered, "[0]\nalpha text\n\n[3]\nbeta text");
    }

    #[test]
    fn parse_batch_response_maps_multiple_chunks_to_their_labels() {
        let payload = json!({
            "results": [
                {
                    "index": 0,
                    "labels": [
                        {"label": "director_appointment", "confidence": 0.93},
                        {"label": "registered_office_change", "confidence": 0.81},
                    ]
                },
                {
                    "index": 1,
                    "labels": []
                },
            ]
        });
        let parsed = parse_batch_response(&payload);
        assert_eq!(parsed.len(), 2);
        let chunk0 = &parsed[&0];
        assert_eq!(chunk0.len(), 2);
        assert_eq!(chunk0[0].label, "director_appointment");
        assert_eq!(chunk0[0].confidence, Some(0.93));
        assert_eq!(chunk0[1].label, "registered_office_change");
        assert_eq!(chunk0[1].confidence, Some(0.81));
        assert!(parsed[&1].is_empty());
    }

    #[test]
    fn parse_batch_response_drops_entries_missing_an_index() {
        let payload = json!({
            "results": [
                {"labels": [{"label": "director_appointment"}]},
                {"index": 2, "labels": [{"label": "director_resignation"}]},
            ]
        });
        let parsed = parse_batch_response(&payload);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[&2][0].label, "director_resignation");
    }

    #[test]
    fn parse_batch_response_empty_results_yields_empty_map() {
        let parsed = parse_batch_response(&json!({"results": []}));
        assert!(parsed.is_empty());
    }

    fn empty_chunk(index: usize) -> crate::types::Chunk {
        crate::types::Chunk {
            content: format!("chunk {index}"),
            chunk_type: Default::default(),
            embedding: None,
            sparse_embedding: None,
            late_interaction: None,
            metadata: crate::types::ChunkMetadata {
                byte_start: 0,
                byte_end: 0,
                token_count: None,
                chunk_index: index,
                total_chunks: 3,
                first_page: None,
                last_page: None,
                heading_context: None,
                heading_path: Vec::new(),
                image_indices: Vec::new(),
                node_ids: Vec::new(),
                page_spans: Vec::new(),
                classifications: Vec::new(),
            },
        }
    }

    #[test]
    fn apply_batch_results_writes_multi_label_results_onto_matching_chunks() {
        let mut chunks = vec![empty_chunk(0), empty_chunk(1), empty_chunk(2)];
        let mut per_chunk = HashMap::new();
        per_chunk.insert(
            0,
            vec![
                ClassificationLabel {
                    label: "director_appointment".to_string(),
                    confidence: Some(0.93),
                },
                ClassificationLabel {
                    label: "registered_office_change".to_string(),
                    confidence: Some(0.81),
                },
            ],
        );
        per_chunk.insert(2, vec![]);

        apply_batch_results(&mut chunks, per_chunk);

        assert_eq!(chunks[0].metadata.classifications.len(), 2);
        assert_eq!(chunks[0].metadata.classifications[0].label, "director_appointment");
        assert_eq!(chunks[0].metadata.classifications[1].label, "registered_office_change");
        assert!(chunks[1].metadata.classifications.is_empty());
        assert!(chunks[2].metadata.classifications.is_empty());
    }

    #[test]
    fn partial_batch_failure_message_names_counts_and_last_error() {
        let err = crate::XbergError::Other("model timed out".to_string());
        let message = partial_batch_failure_message(1, 3, &err);
        assert_eq!(
            message,
            "1 of 3 chunk-classification batches failed; chunks in the failed batch \
             were left without classifications (last error: model timed out)"
        );
    }

    #[test]
    fn partial_batch_failure_message_pluralizes_singular_counts() {
        let err = crate::XbergError::Other("boom".to_string());
        let message = partial_batch_failure_message(1, 1, &err);
        assert_eq!(
            message,
            "1 of 1 chunk-classification batch failed; chunks in the failed batch \
             were left without classifications (last error: boom)"
        );
    }

    #[tokio::test]
    async fn classify_chunks_returns_validation_error_when_definitions_empty() {
        let config = ChunkClassificationConfig {
            prompt_template: None,
            definitions: Vec::new(),
            llm: crate::core::config::LlmConfig {
                model: "openai/gpt-4o-mini".to_string(),
                ..Default::default()
            },
            batch_size: 10,
            max_concurrency: 4,
        };
        let mut result = ExtractedDocument {
            content: "x".to_string(),
            mime_type: std::borrow::Cow::Borrowed("text/plain"),
            ..Default::default()
        };
        let err = classify_chunks(&mut result, &config).await.unwrap_err();
        assert!(err.to_string().contains("definitions must contain at least one entry"));
    }

    #[tokio::test]
    async fn classify_chunks_is_noop_when_no_chunks_present() {
        let config = ChunkClassificationConfig {
            prompt_template: None,
            definitions: definitions(),
            llm: crate::core::config::LlmConfig {
                model: "openai/gpt-4o-mini".to_string(),
                ..Default::default()
            },
            batch_size: 10,
            max_concurrency: 4,
        };
        let mut result = ExtractedDocument {
            content: "x".to_string(),
            mime_type: std::borrow::Cow::Borrowed("text/plain"),
            chunks: None,
            ..Default::default()
        };
        classify_chunks(&mut result, &config).await.unwrap();
        assert!(result.chunks.is_none());
    }
}
