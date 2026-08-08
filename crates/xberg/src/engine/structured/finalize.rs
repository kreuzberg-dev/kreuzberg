//! Compose validate-and-merge with citation fusion into a single schema-driven
//! finalization step for a structured-extraction batch.
//!
//! This ties together [`schema::validate_and_merge`] and [`citations::fuse`] —
//! validating and merging per-batch vision-LLM responses against the schema,
//! then fusing the merged output with extracted OCR/metadata context — in the
//! same order structured-extraction orchestrators built on this mechanism run
//! them: merge first, citations second, unconditionally (citation fusion always
//! runs on the post-merge value — including a `null` merge on a failed
//! outcome — it is never skipped or reordered ahead of merging).
//!
//! All policy (merge mode, citation emission, match threshold, fused
//! confidence) remains a caller-supplied parameter, matching the rest of this
//! mechanism: there is no default merge mode, no default citation policy, and
//! no hardcoded threshold here.

use serde_json::Value;

use super::citations::{self, CitationOutput};
use super::schema::{self, Outcome};
use crate::core::config::MergeMode;

/// Final result of validating, merging, and citation-fusing a
/// structured-extraction batch.
pub struct FinalizedOutput {
    /// Citation-wrapped merged JSON output, or the plain merged value when
    /// `emit_citations` is `false`.
    pub structured_output: Value,
    /// Values-only flattened view of `structured_output`.
    pub structured_output_flat: Value,
    /// Aggregate validate-and-merge outcome.
    pub outcome: Outcome,
    /// Top-level error message, when the whole operation failed.
    pub error_message: Option<String>,
    /// Per-batch validation/parse errors, surfaced by the caller's event payload.
    pub per_batch_errors: Vec<String>,
}

/// Validate and merge `raw_responses` against `schema_value`, then fuse the
/// merged output with `ocr_elements` / `element_metadata`.
///
/// # Arguments
/// * `raw_responses` - Vision-LLM JSON responses (already parsed as `serde_json::Value`)
/// * `schema_value` - JSON Schema (Draft 2020-12) for validation
/// * `merge_mode` - Merge strategy applied across validated batches
/// * `ocr_elements` - Extracted OCR elements used for citation fusion
/// * `element_metadata` - Extracted element metadata used for citation fusion
/// * `emit_citations` - Whether to wrap fields in citation envelopes
/// * `match_threshold` - Minimum text-similarity score (`0.0`-`1.0`) for an OCR
///   element to be accepted as the source of a field value
/// * `fused_confidence` - Confidence recorded on a successfully fused field
#[allow(clippy::too_many_arguments)]
pub fn merge_and_cite(
    raw_responses: Vec<Value>,
    schema_value: &Value,
    merge_mode: MergeMode,
    ocr_elements: &[Value],
    element_metadata: &[Value],
    emit_citations: bool,
    match_threshold: f64,
    fused_confidence: f64,
) -> FinalizedOutput {
    let merged = schema::validate_and_merge(raw_responses, schema_value, merge_mode);

    let CitationOutput {
        structured_output,
        structured_output_flat,
    } = citations::fuse(
        merged.merged,
        ocr_elements,
        element_metadata,
        emit_citations,
        match_threshold,
        fused_confidence,
    );

    FinalizedOutput {
        structured_output,
        structured_output_flat,
        outcome: merged.outcome,
        error_message: merged.error_message,
        per_batch_errors: merged.per_batch_errors,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Conventional citation-fusion tuning used across these tests: a `0.8`
    /// match threshold and `0.95` fused confidence (mirrors the values a
    /// downstream policy layer supplies as parameters, not defaults owned here).
    const MATCH_THRESHOLD: f64 = 0.8;
    const FUSED_CONFIDENCE: f64 = 0.95;

    #[test]
    fn should_merge_then_cite_when_all_batches_are_valid_and_citations_are_on() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "name": { "type": "string" },
                "age": { "type": "number" }
            }
        });

        let batch1 = serde_json::json!({"name": "Alice"});
        let batch2 = serde_json::json!({"age": 30});
        let ocr = serde_json::json!({
            "text": "Alice",
            "page_number": 1,
            "bbox": [10.0, 20.0, 100.0, 30.0]
        });

        let result = merge_and_cite(
            vec![batch1, batch2],
            &schema,
            MergeMode::ObjectMerge,
            &[ocr],
            &[],
            true,
            MATCH_THRESHOLD,
            FUSED_CONFIDENCE,
        );

        assert_eq!(result.outcome, Outcome::Success);
        assert!(result.per_batch_errors.is_empty());

        // The merged "name" field matched OCR text exactly, so it is fused.
        let name_field = result.structured_output.get("name").expect("name field present");
        let cited: citations::CitedField =
            serde_json::from_value(name_field.clone()).expect("citation envelope deserializes");
        assert_eq!(cited.source, citations::CitationSource::Fused);
        assert_eq!(cited.page, Some(1));
        assert_eq!(cited.confidence, Some(FUSED_CONFIDENCE));

        // The flattened view drops the envelope and exposes plain values.
        assert_eq!(
            result.structured_output_flat.get("name").and_then(|v| v.as_str()),
            Some("Alice")
        );
        assert_eq!(
            result.structured_output_flat.get("age").and_then(|v| v.as_u64()),
            Some(30)
        );
    }

    #[test]
    fn should_report_partial_success_and_pass_through_when_citations_are_off() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "name": { "type": "string" }
            }
        });

        let batch1 = serde_json::json!({"name": "Alice"});
        let batch2 = serde_json::json!({"name": 123});

        let result = merge_and_cite(
            vec![batch1, batch2],
            &schema,
            MergeMode::ObjectMerge,
            &[],
            &[],
            false,
            MATCH_THRESHOLD,
            FUSED_CONFIDENCE,
        );

        assert_eq!(result.outcome, Outcome::PartialSuccess);
        assert_eq!(result.per_batch_errors.len(), 1);
        assert!(result.per_batch_errors[0].contains("batch 1"));

        // emit_citations=false: structured_output is the plain merged value, not
        // wrapped in a citation envelope.
        assert_eq!(
            result.structured_output,
            serde_json::json!({"name": "Alice"}),
            "citations disabled means structured_output is the merged value verbatim"
        );
        assert_eq!(result.structured_output, result.structured_output_flat);
    }

    #[test]
    fn should_fuse_null_merge_when_every_batch_fails_schema_validation() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "count": { "type": "number" }
            },
            "required": ["count"]
        });

        let batch1 = serde_json::json!({"count": "not a number"});

        let result = merge_and_cite(
            vec![batch1],
            &schema,
            MergeMode::ObjectMerge,
            &[],
            &[],
            true,
            MATCH_THRESHOLD,
            FUSED_CONFIDENCE,
        );

        assert_eq!(result.outcome, Outcome::SchemaInvalid);
        assert_eq!(
            result.error_message,
            Some("all batches failed schema validation".to_string())
        );
        // Citation fusion still runs on the null merge (never skipped ahead of
        // merging), producing a citation envelope around a null leaf value.
        let cited: citations::CitedField =
            serde_json::from_value(result.structured_output.clone()).expect("null leaf still gets an envelope");
        assert_eq!(cited.source, citations::CitationSource::None);
        assert_eq!(cited.value, Value::Null);
    }
}
