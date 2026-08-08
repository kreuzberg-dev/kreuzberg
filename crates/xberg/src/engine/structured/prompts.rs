//! Build system + user prompts for the vision-LLM call.
//!
//! This is the generic prompt-assembly mechanism: it substitutes `{{var}}`
//! placeholders, fences untrusted content with a per-call nonce, truncates the
//! extracted-text excerpt at a caller-supplied limit, and optionally appends a
//! caller-supplied citation instruction. All policy text (system prompt, context
//! template, citation instruction) and all limits are **parameters** — no preset
//! type, no embedded instruction template, and no environment read live here.

use crate::heuristics::StructuredCallMode;

/// Build a per-call random nonce used to fence untrusted content (extracted
/// text + prior LLM JSON) inside the prompt. An attacker who can plant content
/// in the document cannot predict the nonce, so they cannot close the fence
/// and inject instructions at the same nesting level as the legitimate ones.
///
/// Randomness is drawn from `std`'s [`std::collections::hash_map::RandomState`],
/// which is seeded from OS entropy and randomized per instance, so the mechanism
/// stays dependency-light (no `uuid`/`rand`) while preserving unpredictability.
fn fence_nonce() -> String {
    use std::hash::{BuildHasher, Hasher};
    let hi = std::collections::hash_map::RandomState::new().build_hasher().finish();
    let lo = std::collections::hash_map::RandomState::new().build_hasher().finish();
    format!("{hi:016x}{lo:016x}")
}

/// Truncate `text` to at most `max_bytes` bytes, backing off to the nearest
/// UTF-8 char boundary so the slice never panics on multi-byte input.
///
/// `max_bytes` is a byte budget (it bounds the excerpt's serialized size); when
/// the cut point lands inside a multi-byte character the excerpt is shortened to
/// the preceding boundary rather than panicking.
fn truncate_to_char_boundary(text: &str, max_bytes: usize) -> &str {
    if text.len() <= max_bytes {
        return text;
    }
    let mut end = max_bytes;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// Built prompt components ready to send to the vision model.
#[derive(Debug, Clone)]
pub struct BuiltPrompt {
    /// System instruction primer.
    pub system: String,
    /// User content (text + images context). None for VisionOnly / Skip modes.
    pub user_text: Option<String>,
}

/// Build system + user prompts from caller-supplied text and context.
///
/// - System prompt is `system_prompt` with `{{var}}` substitution from `user_context`.
/// - `citation_instruction` is appended to the system prompt when `Some`.
/// - User text includes `context_template` (if present) + the extracted excerpt,
///   truncated at `max_excerpt_bytes`.
/// - VisionOnly / Skip modes set `user_text` to None.
pub fn build_prompt(
    system_prompt: &str,
    context_template: Option<&str>,
    extracted_text_excerpt: &str,
    user_context: Option<&serde_json::Map<String, serde_json::Value>>,
    call_mode: StructuredCallMode,
    citation_instruction: Option<&str>,
    max_excerpt_bytes: usize,
) -> BuiltPrompt {
    let mut system = substitute_vars(system_prompt, user_context);

    if let Some(instruction) = citation_instruction {
        system.push_str(instruction);
    }

    let user_text = match call_mode {
        StructuredCallMode::VisionOnly => None,
        StructuredCallMode::Skip => {
            tracing::warn!("build_prompt called with Skip mode; treating as no content");
            None
        }
        StructuredCallMode::TextOnly
        | StructuredCallMode::TextPlusVision
        | StructuredCallMode::TextOnlyWithVisionFallback => {
            let mut content = String::new();

            if let Some(template) = context_template {
                let rendered = substitute_vars(template, user_context);
                content.push_str(&rendered);
            }

            let excerpt = truncate_to_char_boundary(extracted_text_excerpt, max_excerpt_bytes);

            if !content.is_empty() && !excerpt.is_empty() {
                content.push_str("\n\n---\n\n");
            }
            content.push_str(excerpt);

            Some(content)
        }
    };

    BuiltPrompt { system, user_text }
}

/// Build a vision-fallback prompt when text-only extraction was low confidence.
///
/// Composes the system prompt (with optional citation instruction), the context
/// template, the original text excerpt (fenced, truncated at `max_excerpt_bytes`),
/// the prior text-only JSON output, a confidence breakdown, and a fallback
/// instruction.
///
/// Returns the composed `user_text` (text only — images come separately in the request).
#[allow(clippy::too_many_arguments)]
pub fn build_vision_fallback_prompt(
    system_prompt: &str,
    context_template: Option<&str>,
    extracted_text_excerpt: &str,
    user_context: Option<&serde_json::Map<String, serde_json::Value>>,
    prior_json: &serde_json::Value,
    confidence: &crate::heuristics::confidence::ExtractionConfidence,
    citation_instruction: Option<&str>,
    max_excerpt_bytes: usize,
) -> BuiltPrompt {
    let mut system = substitute_vars(system_prompt, user_context);

    if let Some(instruction) = citation_instruction {
        system.push_str(instruction);
    }

    let mut content = String::new();

    if let Some(template) = context_template {
        let rendered = substitute_vars(template, user_context);
        content.push_str(&rendered);
    }

    let excerpt = truncate_to_char_boundary(extracted_text_excerpt, max_excerpt_bytes);

    let nonce = fence_nonce();

    if !excerpt.is_empty() {
        if !content.is_empty() {
            content.push_str("\n\n");
        }
        content.push_str(&format!("--- BEGIN EXTRACTED_TEXT_{nonce} ---\n"));
        content.push_str(excerpt);
        content.push_str(&format!("\n--- END EXTRACTED_TEXT_{nonce} ---\n"));
    }

    content.push_str("\nPrior text-only extraction (low confidence):\n");
    content.push_str(&format!("--- BEGIN PRIOR_JSON_{nonce} ---\n"));
    match serde_json::to_string_pretty(prior_json) {
        Ok(pretty) => content.push_str(&pretty),
        Err(_) => {
            let fallback = serde_json::json!({"error": "prior extraction unavailable"});
            if let Ok(s) = serde_json::to_string_pretty(&fallback) {
                content.push_str(&s);
            }
        }
    }
    content.push_str(&format!("\n--- END PRIOR_JSON_{nonce} ---\n\n"));

    content.push_str("Confidence breakdown of prior extraction:\n");
    content.push_str(&format!(
        "  - Text coverage: {:.2}%\n",
        confidence.text_coverage * 100.0
    ));
    if let Some(ocr) = confidence.ocr_aggregate {
        content.push_str(&format!("  - OCR confidence: {:.2}%\n", ocr * 100.0));
    }
    content.push_str(&format!("  - Schema compliance: {:?}\n", confidence.schema_compliance));
    content.push_str(&format!("  - Combined score: {:.2}%\n", confidence.combined * 100.0));

    content.push_str(
        "\nPlease review the source pages directly and produce a corrected/completed JSON output matching the schema. Focus on details the text extraction may have missed.",
    );

    BuiltPrompt {
        system,
        user_text: Some(content),
    }
}

/// Decide whether a text-only pass's confidence warrants escalating to a
/// vision-fallback call, building the fallback prompt via
/// [`build_vision_fallback_prompt`] when it does.
///
/// Returns `Some` when `confidence.combined` falls strictly below
/// `fallback_threshold`; `None` when the text-only result is confident enough
/// and no escalation is needed. Mirrors the fallback decision made by
/// structured-extraction orchestrators built on this mechanism, but
/// `fallback_threshold` is a caller-supplied parameter — this mechanism bakes
/// in no default threshold.
#[allow(clippy::too_many_arguments)]
pub fn escalate_if_below_threshold(
    system_prompt: &str,
    context_template: Option<&str>,
    extracted_text_excerpt: &str,
    user_context: Option<&serde_json::Map<String, serde_json::Value>>,
    prior_json: &serde_json::Value,
    confidence: &crate::heuristics::confidence::ExtractionConfidence,
    fallback_threshold: f32,
    citation_instruction: Option<&str>,
    max_excerpt_bytes: usize,
) -> Option<BuiltPrompt> {
    if confidence.combined >= fallback_threshold {
        return None;
    }

    Some(build_vision_fallback_prompt(
        system_prompt,
        context_template,
        extracted_text_excerpt,
        user_context,
        prior_json,
        confidence,
        citation_instruction,
        max_excerpt_bytes,
    ))
}

/// Substitute {{var}} placeholders from the provided context map.
/// Missing vars leave the placeholder intact (do not error).
fn substitute_vars(template: &str, context: Option<&serde_json::Map<String, serde_json::Value>>) -> String {
    let Some(ctx) = context else {
        return template.to_string();
    };

    let mut result = String::new();
    let mut chars = template.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '{' && chars.peek() == Some(&'{') {
            chars.next();

            let mut var_name = String::new();
            let mut found_close = false;

            while let Some(next_ch) = chars.next() {
                if next_ch == '}' && chars.peek() == Some(&'}') {
                    chars.next();
                    found_close = true;
                    break;
                }
                var_name.push(next_ch);
            }

            if found_close {
                if let Some(value) = ctx.get(&var_name) {
                    match value {
                        serde_json::Value::Null => {
                            result.push_str("{{");
                            result.push_str(&var_name);
                            result.push_str("}}");
                        }
                        serde_json::Value::Bool(b) => result.push_str(&b.to_string()),
                        serde_json::Value::Number(n) => result.push_str(&n.to_string()),
                        serde_json::Value::String(s) => result.push_str(s),
                        _ => {
                            if let Ok(s) = serde_json::to_string(value) {
                                result.push_str(&s);
                            } else {
                                result.push_str("{{");
                                result.push_str(&var_name);
                                result.push_str("}}");
                            }
                        }
                    }
                } else {
                    result.push_str("{{");
                    result.push_str(&var_name);
                    result.push_str("}}");
                }
            } else {
                result.push_str("{{");
                result.push_str(&var_name);
            }
        } else {
            result.push(ch);
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::heuristics::confidence::{ExtractionConfidence, SchemaCompliance};

    /// Caller-supplied excerpt cap used across the assembly tests (a
    /// conventional 200k limit, supplied as a plain parameter).
    const TEST_MAX_EXCERPT: usize = 200_000;

    /// A literal citation instruction, supplied as a parameter — no preset, and
    /// not the worker's embedded `CITATION_INSTRUCTION` text.
    const TEST_CITATION_INSTRUCTION: &str = "\n\n---\n\nFORMAT EACH FIELD WITH value/page/bbox/confidence.\n";

    #[test]
    fn context_substitution_all_vars_present() {
        let mut context = serde_json::Map::new();
        context.insert("entity_type".to_string(), serde_json::json!("invoice"));
        context.insert("document_kind".to_string(), serde_json::json!("receipt"));

        let prompt = build_prompt(
            "Extract {{entity_type}} from the {{document_kind}}.",
            Some("Context: {{entity_type}}"),
            "text",
            Some(&context),
            StructuredCallMode::TextOnly,
            None,
            TEST_MAX_EXCERPT,
        );

        assert!(prompt.system.contains("Extract invoice from the receipt."));
        assert!(prompt.user_text.unwrap().contains("Context: invoice"));
    }

    #[test]
    fn context_substitution_missing_var_leaves_placeholder() {
        let mut context = serde_json::Map::new();
        context.insert("entity_type".to_string(), serde_json::json!("invoice"));

        let prompt = build_prompt(
            "Extract {{entity_type}} from {{missing_var}}.",
            None,
            "text",
            Some(&context),
            StructuredCallMode::TextOnly,
            None,
            TEST_MAX_EXCERPT,
        );

        assert!(prompt.system.contains("Extract invoice from {{missing_var}}."));
    }

    #[test]
    fn context_substitution_with_nested_json() {
        let mut context = serde_json::Map::new();
        context.insert("config".to_string(), serde_json::json!({"nested": "value"}));

        let prompt = build_prompt(
            "Use {{config}}",
            None,
            "text",
            Some(&context),
            StructuredCallMode::TextOnly,
            None,
            TEST_MAX_EXCERPT,
        );

        assert!(prompt.system.contains(r#"{"nested":"value"}"#));
    }

    #[test]
    fn context_substitution_with_number() {
        let mut context = serde_json::Map::new();
        context.insert("threshold".to_string(), serde_json::json!(0.95));

        let prompt = build_prompt(
            "Precision: {{threshold}}",
            None,
            "text",
            Some(&context),
            StructuredCallMode::TextOnly,
            None,
            TEST_MAX_EXCERPT,
        );

        assert!(prompt.system.contains("Precision: 0.95"));
    }

    #[test]
    fn context_substitution_with_bool() {
        let mut context = serde_json::Map::new();
        context.insert("strict".to_string(), serde_json::json!(true));

        let prompt = build_prompt(
            "Strict mode: {{strict}}",
            None,
            "text",
            Some(&context),
            StructuredCallMode::TextOnly,
            None,
            TEST_MAX_EXCERPT,
        );

        assert!(prompt.system.contains("Strict mode: true"));
    }

    #[test]
    fn text_only_includes_excerpt() {
        let prompt = build_prompt(
            "You are a helpful extractor.",
            None,
            "Sample extracted text",
            None,
            StructuredCallMode::TextOnly,
            None,
            TEST_MAX_EXCERPT,
        );

        assert_eq!(prompt.system, "You are a helpful extractor.");
        assert!(prompt.user_text.is_some());
        assert!(prompt.user_text.unwrap().contains("Sample extracted text"));
    }

    #[test]
    fn vision_only_has_no_user_text() {
        let prompt = build_prompt(
            "You are a helpful extractor.",
            None,
            "Sample extracted text",
            None,
            StructuredCallMode::VisionOnly,
            None,
            TEST_MAX_EXCERPT,
        );

        assert_eq!(prompt.system, "You are a helpful extractor.");
        assert!(prompt.user_text.is_none());
    }

    #[test]
    fn skip_mode_returns_none_with_warning() {
        let prompt = build_prompt(
            "You are a helpful extractor.",
            None,
            "text",
            None,
            StructuredCallMode::Skip,
            None,
            TEST_MAX_EXCERPT,
        );

        assert!(prompt.user_text.is_none());
        assert_eq!(prompt.system, "You are a helpful extractor.");
    }

    #[test]
    fn text_plus_vision_includes_excerpt() {
        let prompt = build_prompt(
            "You are a helpful extractor.",
            None,
            "Sample text with images",
            None,
            StructuredCallMode::TextPlusVision,
            None,
            TEST_MAX_EXCERPT,
        );

        assert!(prompt.user_text.is_some());
        assert!(prompt.user_text.unwrap().contains("Sample text with images"));
    }

    #[test]
    fn citation_instruction_appended_when_passed() {
        let prompt = build_prompt(
            "You are a helpful extractor.",
            None,
            "text",
            None,
            StructuredCallMode::TextOnly,
            Some(TEST_CITATION_INSTRUCTION),
            TEST_MAX_EXCERPT,
        );

        assert!(prompt.system.contains(TEST_CITATION_INSTRUCTION));
    }

    #[test]
    fn citation_instruction_not_appended_when_none() {
        let prompt = build_prompt(
            "You are a helpful extractor.",
            None,
            "text",
            None,
            StructuredCallMode::TextOnly,
            None,
            TEST_MAX_EXCERPT,
        );

        assert_eq!(prompt.system, "You are a helpful extractor.");
    }

    #[test]
    fn excerpt_truncated_at_max_excerpt_bytes() {
        let long_excerpt = "a".repeat(300_000);

        let prompt = build_prompt(
            "You are a helpful extractor.",
            None,
            &long_excerpt,
            None,
            StructuredCallMode::TextOnly,
            None,
            TEST_MAX_EXCERPT,
        );

        let user_text = prompt.user_text.unwrap();
        assert!(user_text.len() < 300_000);
        assert!(user_text.contains(&"a".repeat(1000)));
    }

    #[test]
    fn context_template_merged_with_excerpt() {
        let prompt = build_prompt(
            "You are a helpful extractor.",
            Some("Document type: invoice"),
            "Invoice number: 12345",
            None,
            StructuredCallMode::TextOnly,
            None,
            TEST_MAX_EXCERPT,
        );

        let user_text = prompt.user_text.unwrap();
        assert!(user_text.contains("Document type: invoice"));
        assert!(user_text.contains("Invoice number: 12345"));
        assert!(user_text.contains("---"));
    }

    #[test]
    fn vision_fallback_includes_excerpt_prior_and_confidence() {
        let confidence = ExtractionConfidence {
            text_coverage: 0.5,
            ocr_aggregate: Some(0.8),
            schema_compliance: SchemaCompliance::PartialValid,
            combined: 0.6,
        };
        let prior = serde_json::json!({"name": "Alice"});

        let prompt = build_vision_fallback_prompt(
            "You are a helpful extractor.",
            None,
            "Sample extracted text",
            None,
            &prior,
            &confidence,
            Some(TEST_CITATION_INSTRUCTION),
            TEST_MAX_EXCERPT,
        );

        assert!(prompt.system.contains(TEST_CITATION_INSTRUCTION));
        let user_text = prompt.user_text.unwrap();
        assert!(user_text.contains("Sample extracted text"));
        assert!(user_text.contains("Prior text-only extraction"));
        assert!(user_text.contains("\"name\": \"Alice\""));
        assert!(user_text.contains("Text coverage: 50.00%"));
        assert!(user_text.contains("OCR confidence: 80.00%"));
        assert!(user_text.contains("Combined score: 60.00%"));
    }

    #[test]
    fn vision_fallback_truncates_excerpt() {
        let confidence = ExtractionConfidence {
            text_coverage: 1.0,
            ocr_aggregate: None,
            schema_compliance: SchemaCompliance::AllValid,
            combined: 1.0,
        };
        let prior = serde_json::json!({});
        let long_excerpt = "b".repeat(300_000);

        let prompt = build_vision_fallback_prompt(
            "system",
            None,
            &long_excerpt,
            None,
            &prior,
            &confidence,
            None,
            TEST_MAX_EXCERPT,
        );

        let user_text = prompt.user_text.unwrap();
        assert!(!user_text.contains(&"b".repeat(300_000)));
        assert!(user_text.contains(&"b".repeat(1000)));
    }

    #[test]
    fn should_escalate_when_confidence_is_below_threshold() {
        let confidence = ExtractionConfidence {
            text_coverage: 0.5,
            ocr_aggregate: Some(0.4),
            schema_compliance: SchemaCompliance::PartialValid,
            combined: 0.4,
        };
        let prior = serde_json::json!({"name": "Alice"});

        let decision = escalate_if_below_threshold(
            "You are a helpful extractor.",
            None,
            "Sample extracted text",
            None,
            &prior,
            &confidence,
            0.6,
            None,
            TEST_MAX_EXCERPT,
        );

        let prompt = decision.expect("confidence 0.4 < threshold 0.6 must escalate");
        let user_text = prompt.user_text.expect("fallback prompt carries user text");
        assert!(user_text.contains("Prior text-only extraction"));
        assert!(user_text.contains("\"name\": \"Alice\""));
    }

    #[test]
    fn should_not_escalate_when_confidence_meets_threshold() {
        let confidence = ExtractionConfidence {
            text_coverage: 0.9,
            ocr_aggregate: None,
            schema_compliance: SchemaCompliance::AllValid,
            combined: 0.6,
        };
        let prior = serde_json::json!({"name": "Alice"});

        let decision = escalate_if_below_threshold(
            "You are a helpful extractor.",
            None,
            "Sample extracted text",
            None,
            &prior,
            &confidence,
            0.6,
            None,
            TEST_MAX_EXCERPT,
        );

        assert!(
            decision.is_none(),
            "confidence 0.6 meeting threshold 0.6 must not escalate"
        );
    }

    #[test]
    fn multibyte_excerpt_truncation_does_not_panic() {
        let multibyte = "世".repeat(200);
        let max_bytes = 100usize;

        let prompt = build_prompt(
            "system",
            None,
            &multibyte,
            None,
            StructuredCallMode::TextOnly,
            None,
            max_bytes,
        );
        let user_text = prompt.user_text.expect("text-only mode yields user text");
        assert!(user_text.len() <= max_bytes);
        assert!(user_text.chars().all(|c| c == '世'));

        let confidence = ExtractionConfidence {
            text_coverage: 1.0,
            ocr_aggregate: None,
            schema_compliance: SchemaCompliance::AllValid,
            combined: 1.0,
        };
        let fallback = build_vision_fallback_prompt(
            "system",
            None,
            &multibyte,
            None,
            &serde_json::json!({}),
            &confidence,
            None,
            max_bytes,
        );
        assert!(fallback.user_text.expect("fallback user text").contains('世'));
    }
}
