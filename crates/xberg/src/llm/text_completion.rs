//! Plain text completion helper for LLM-driven post-processors.
//!
//! Wraps a liter-llm chat call that takes a free-form prompt and returns the
//! assistant text. Used by translation and summarisation-abstractive
//! post-processors; not constrained by a JSON schema.

use crate::core::config::LlmConfig;
use crate::types::LlmUsage;

/// Send a single user prompt to the configured LLM and return the response text
/// along with the captured usage metadata.
///
/// The `source` argument labels the [`LlmUsage`] entry that is returned so
/// callers can aggregate per-feature spend (`"translation"`, `"summarisation"`,
/// etc.). The helper performs a single non-streaming chat completion request.
///
/// # Errors
///
/// Returns an error if the LLM client cannot be constructed, the request fails,
/// or the response does not contain assistant content.
#[allow(clippy::field_reassign_with_default)]
#[cfg_attr(alef, alef(skip))]
pub async fn complete_text(
    llm_config: &LlmConfig,
    prompt: &str,
    source: &str,
) -> crate::Result<(String, Option<LlmUsage>)> {
    use liter_llm::LlmClient;

    let client = super::client::create_client(llm_config)?;

    let mut request = liter_llm::ChatCompletionRequest::default();
    request.model = llm_config.model.clone();
    request.messages = vec![liter_llm::Message::User(liter_llm::UserMessage {
        content: liter_llm::UserContent::Text(prompt.to_string()),
        name: None,
    })];
    request.temperature = llm_config.temperature;
    request.max_tokens = llm_config.max_tokens;
    request.reasoning_effort = super::client::parse_reasoning_effort(llm_config)?;
    request.extra_body = llm_config.extra_body.clone();

    let response = client
        .chat(request)
        .await
        .map_err(|e| crate::XbergError::parsing(format!("LLM text completion request failed ({source}): {e}")))?;

    let usage = super::usage::extract_usage_from_chat(&response, source);

    let text = response
        .choices
        .first()
        .and_then(|c| c.message.content.as_ref().and_then(|m| m.as_text()))
        .map(|s| s.trim().to_string())
        .ok_or_else(|| {
            crate::XbergError::parsing(format!(
                "LLM text completion ({source}) returned no content (model={}, {} choices)",
                llm_config.model,
                response.choices.len()
            ))
        })?;

    Ok((text, usage))
}
