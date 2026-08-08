//! LLM-driven translation backend.
//!
//! Builds a Minijinja prompt per text segment (whole content, formatted
//! content, chunks) and calls
//! [`crate::llm::text_completion::complete_text`] for each. Empty segments are
//! skipped so we do not waste tokens on whitespace.

use crate::core::config::TranslationConfig;
use crate::types::translation::Translation;
use crate::types::{ExtractedDocument, LlmUsage};

/// Default Jinja2 template for LLM translation. Receives `target_lang`,
/// `source_lang` (may be `"auto"`), `preserve_markup`, and `text` variables.
pub const DEFAULT_TRANSLATION_TEMPLATE: &str = "\
You are a precise translation engine. Translate the text below {% if source_lang and source_lang != 'auto' %}from {{ source_lang }} {% endif %}\
into {{ target_lang }}.

Rules:
- Preserve the original meaning exactly.
- Do not add commentary, explanations, or surrounding quotes.
{% if preserve_markup %}- Preserve Markdown formatting (headings, lists, emphasis, links, code blocks) and HTML tags exactly as they appear.\
{% else %}- Return plain text only.{% endif %}
- If the text is already in {{ target_lang }}, return it unchanged.
- If the text is empty, return an empty string.

Text:
{{ text }}";

/// Render the prompt for a single text segment.
fn render_prompt(config: &TranslationConfig, text: &str, preserve_markup: bool) -> crate::Result<String> {
    let ctx = minijinja::context! {
        target_lang => &config.target_lang,
        source_lang => config.source_lang.as_deref().unwrap_or("auto"),
        preserve_markup => preserve_markup,
        text => text,
    };
    crate::llm::prompts::render_template(DEFAULT_TRANSLATION_TEMPLATE, &ctx)
}

/// Translate a single segment, collecting any usage entry produced.
///
/// `pub(super)` so the sibling [`super::fields`] module — which translates
/// every secondary text field (tables, pages, metadata, elements, document
/// structure) — can reuse it for fields that are naturally singular (page
/// content, table markdown) rather than duplicating this logic.
pub(super) async fn translate_segment(
    config: &TranslationConfig,
    text: &str,
    preserve_markup: bool,
    source_label: &str,
    usages: &mut Vec<LlmUsage>,
) -> crate::Result<String> {
    if text.trim().is_empty() {
        return Ok(text.to_string());
    }
    let prompt = render_prompt(config, text, preserve_markup)?;
    let (translated, usage) = crate::llm::text_completion::complete_text(&config.llm, &prompt, source_label).await?;
    if let Some(u) = usage {
        usages.push(u);
    }
    Ok(translated)
}

/// Translate the extraction result in place.
///
/// Populates `result.translation` with the translated `content`, optionally the
/// translated `formatted_content` (when `preserve_markup = true`), and rewrites
/// every chunk's `content` field. It also rewrites every other text-bearing
/// field — tables, pages, metadata, semantic elements, and the structured
/// document tree — via [`super::fields::translate_secondary_fields`]
/// (xberg-io/xberg#254). Every LLM call's usage is appended to
/// `result.llm_usage`.
pub async fn translate_result(result: &mut ExtractedDocument, config: &TranslationConfig) -> crate::Result<()> {
    if config.target_lang.trim().is_empty() {
        return Err(crate::XbergError::validation(
            "TranslationConfig.target_lang must not be empty",
        ));
    }

    let mut usages: Vec<LlmUsage> = Vec::new();

    let translated_content =
        translate_segment(config, &result.content, false, "translation_content", &mut usages).await?;

    let translated_formatted = if config.preserve_markup
        && let Some(formatted) = result.formatted_content.as_deref()
        && !formatted.trim().is_empty()
    {
        Some(translate_segment(config, formatted, true, "translation_formatted", &mut usages).await?)
    } else {
        None
    };

    if let Some(chunks) = result.chunks.as_mut() {
        for chunk in chunks.iter_mut() {
            let translated = translate_segment(config, &chunk.content, false, "translation_chunk", &mut usages).await?;
            chunk.content = translated;
        }
    }

    super::fields::translate_secondary_fields(result, config, &mut usages).await?;

    result.translation = Some(Translation {
        target_lang: config.target_lang.clone(),
        source_lang: config.source_lang.clone(),
        content: translated_content,
        formatted_content: translated_formatted,
    });

    if !usages.is_empty() {
        result.llm_usage.get_or_insert_with(Vec::new).extend(usages);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::config::LlmConfig;

    fn cfg() -> TranslationConfig {
        TranslationConfig {
            target_lang: "de".to_string(),
            source_lang: None,
            preserve_markup: false,
            llm: LlmConfig {
                model: "openai/gpt-4o-mini".to_string(),
                ..Default::default()
            },
        }
    }

    #[test]
    fn render_prompt_includes_target_lang() {
        let prompt = render_prompt(&cfg(), "Hello world", false).unwrap();
        assert!(prompt.contains("de"));
        assert!(prompt.contains("Hello world"));
    }

    #[test]
    fn render_prompt_includes_source_lang_when_set() {
        let mut c = cfg();
        c.source_lang = Some("en".to_string());
        let prompt = render_prompt(&c, "Hello", false).unwrap();
        assert!(prompt.contains("from en"));
    }

    #[test]
    fn render_prompt_preserves_markup_clause_when_enabled() {
        let prompt = render_prompt(&cfg(), "**hi**", true).unwrap();
        assert!(prompt.contains("Markdown"));
    }

    /// Regression test for xberg-io/xberg#254: `translate_result` used to
    /// rewrite only `content`, `formatted_content`, and chunk content,
    /// leaving every other text-bearing field (tables, pages, metadata, the
    /// structured document tree) in the source language. This drives
    /// `translate_result` end-to-end against a table cell — a field
    /// `translate_result` never touched before the fix — through a local
    /// loopback HTTP stub (no real network call), and asserts both the exact
    /// translated value and that exactly one LLM call was made for the one
    /// non-empty secondary field present, which is the call-volume contract
    /// documented in `super::fields`.
    #[cfg(feature = "api")]
    #[tokio::test]
    async fn translate_result_translates_table_cells_via_secondary_fields() {
        use crate::types::Table;
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let call_count = Arc::new(AtomicUsize::new(0));
        let call_count_handler = call_count.clone();

        let app = axum::Router::new().fallback(axum::routing::post(move || {
            let call_count = call_count_handler.clone();
            async move {
                call_count.fetch_add(1, Ordering::SeqCst);
                axum::response::Json(serde_json::json!({
                    "id": "test",
                    "object": "chat.completion",
                    "created": 0,
                    "model": "test",
                    "choices": [{
                        "index": 0,
                        "message": { "role": "assistant", "content": "[\"CELDA TRADUCIDA\"]" },
                        "finish_reason": "stop"
                    }]
                }))
            }
        }));

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let base_url = format!("http://{addr}/v1/");
        let config = TranslationConfig {
            target_lang: "es".to_string(),
            source_lang: None,
            preserve_markup: false,
            llm: LlmConfig {
                model: "openai/gpt-4o-mini".to_string(),
                api_key: Some("test-key".to_string()),
                base_url: Some(base_url),
                ..Default::default()
            },
        };

        // `content` is left empty so the only field this document has to
        // translate is the single table cell — isolating the assertion to
        // the field `translate_result` used to skip.
        let mut result = ExtractedDocument {
            content: String::new(),
            mime_type: std::borrow::Cow::Borrowed("text/plain"),
            tables: vec![Table {
                cells: vec![vec!["hola".to_string()]],
                markdown: String::new(),
                ..Table::default()
            }],
            ..Default::default()
        };

        translate_result(&mut result, &config).await.unwrap();

        assert_eq!(result.tables[0].cells[0][0], "CELDA TRADUCIDA");
        assert_eq!(
            call_count.load(Ordering::SeqCst),
            1,
            "expected exactly one batched LLM call for the one non-empty table cell, not one call per field"
        );
    }
}
