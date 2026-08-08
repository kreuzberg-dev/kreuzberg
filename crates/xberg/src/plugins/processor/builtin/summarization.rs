//! Summarisation post-processor.
//!
//! Runs at [`ProcessingStage::Middle`]. Triggers when
//! [`ExtractionConfig::summarization`] is `Some`. Dispatches on
//! [`SummaryStrategy`]:
//!
//! - [`SummaryStrategy::Extractive`] — pure-Rust TextRank via
//!   [`crate::text::summarization::textrank::summarize`]. Deterministic and
//!   available in every build.
//! - [`SummaryStrategy::Abstractive`] — LLM-driven via
//!   [`crate::text::summarization::llm::summarize_with_llm`]. Requires both
//!   the `summarization-llm` cargo feature and an `llm` config slot on
//!   [`crate::core::config::SummarizationConfig`]. When the feature is absent
//!   or `llm` is `None`, the processor returns a validation error so callers
//!   notice the misconfiguration.

use std::sync::Arc;

use crate::Result;
use crate::core::config::ExtractionConfig;
use crate::plugins::{Plugin, PostProcessor, ProcessingStage, register_post_processor};
use crate::types::ExtractedDocument;
use crate::types::summary::{DocumentSummary, SummaryStrategy};
use async_trait::async_trait;

/// Post-processor that produces a [`DocumentSummary`] for the extraction result.
#[cfg_attr(alef, alef(skip))]
#[derive(Debug, Clone, Copy, Default)]
pub struct SummarizationProcessor;

impl Plugin for SummarizationProcessor {
    fn name(&self) -> &str {
        "summarization"
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
impl PostProcessor for SummarizationProcessor {
    async fn process(&self, result: &mut ExtractedDocument, config: &ExtractionConfig) -> Result<()> {
        let summarization_config = match &config.summarization {
            Some(cfg) => cfg,
            None => return Ok(()),
        };

        if result.content.trim().is_empty() {
            return Ok(());
        }

        match summarization_config.strategy {
            SummaryStrategy::Extractive => run_extractive(result, summarization_config.max_tokens),
            SummaryStrategy::Abstractive => run_abstractive(result, summarization_config).await,
        }
    }

    fn processing_stage(&self) -> ProcessingStage {
        ProcessingStage::Middle
    }

    fn should_process(&self, _result: &ExtractedDocument, config: &ExtractionConfig) -> bool {
        config.summarization.is_some()
    }

    fn estimated_duration_ms(&self, result: &ExtractedDocument) -> u64 {
        let word_count = result.content.split_whitespace().count() as u64;
        word_count / 200 + 50
    }
}

fn run_extractive(result: &mut ExtractedDocument, max_tokens: Option<u32>) -> Result<()> {
    let language = result
        .detected_languages
        .as_ref()
        .and_then(|langs| langs.first())
        .map(String::as_str);

    let summary_text = crate::text::summarization::textrank::summarize(&result.content, language, max_tokens);
    apply_extractive_summary(result, summary_text);
    Ok(())
}

/// Apply the outcome of TextRank extractive summarisation to `result`.
///
/// `summary_text` is `None` (or, defensively, `Some("")`) when the summariser
/// could not produce output for this document. Previously this was masked by
/// `.unwrap_or_default()` and silently left `result.summary` unset with no
/// signal to the caller, unlike the abstractive branch which pushes a
/// `ProcessingWarning` on failure (#257). Mirror that behaviour here so the
/// caller can distinguish "no summary produced" from "summariser errored".
fn apply_extractive_summary(result: &mut ExtractedDocument, summary_text: Option<String>) {
    let summary_text = match summary_text {
        Some(text) if !text.is_empty() => text,
        _ => {
            crate::core::diagnostics::push_warning(
                &mut result.processing_warnings,
                "summarization_extractive",
                "Extractive summarisation produced no output for this document",
            );
            return;
        }
    };

    let token_count = crate::text::summarization::textrank::token_count(&summary_text);
    result.summary = Some(DocumentSummary {
        text: summary_text,
        strategy: SummaryStrategy::Extractive,
        token_count: Some(token_count),
    });
}

#[cfg(feature = "summarization-llm")]
async fn run_abstractive(
    result: &mut ExtractedDocument,
    summarization_config: &crate::core::config::SummarizationConfig,
) -> Result<()> {
    use crate::types::ProcessingWarning;
    use std::borrow::Cow;

    let llm_config = summarization_config.llm.as_ref().ok_or_else(|| {
        crate::XbergError::validation(
            "Abstractive summarisation requires `SummarizationConfig.llm` to be set".to_string(),
        )
    })?;

    let outcome = crate::text::summarization::llm::summarize_with_llm(
        &result.content,
        llm_config,
        summarization_config.max_tokens,
    )
    .await;

    let (text, usage) = match outcome {
        Ok(pair) => pair,
        Err(e) => {
            result.processing_warnings.push(ProcessingWarning {
                source: Cow::Borrowed("summarization_abstractive"),
                message: Cow::Owned(format!("LLM summarisation failed: {e}")),
            });
            return Ok(());
        }
    };

    if text.is_empty() {
        return Ok(());
    }

    let token_count = crate::text::summarization::textrank::token_count(&text);
    result.summary = Some(DocumentSummary {
        text,
        strategy: SummaryStrategy::Abstractive,
        token_count: Some(token_count),
    });

    if let Some(u) = usage {
        result.llm_usage.get_or_insert_with(Vec::new).push(u);
    }

    Ok(())
}

#[cfg(not(feature = "summarization-llm"))]
async fn run_abstractive(
    _result: &mut ExtractedDocument,
    _summarization_config: &crate::core::config::SummarizationConfig,
) -> Result<()> {
    Err(crate::XbergError::validation(
        "Abstractive summarisation requires the `summarization-llm` cargo feature".to_string(),
    ))
}

/// Register the default summarisation post-processor with the global registry.
#[cfg_attr(alef, alef(skip))]
pub fn register() -> Result<()> {
    register_post_processor(Arc::new(SummarizationProcessor))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::config::SummarizationConfig;
    use std::borrow::Cow;

    const PARAGRAPH: &str = "Machine learning is a branch of artificial intelligence. \
        It focuses on building systems that learn from data. \
        Deep learning is a subset of machine learning. \
        Neural networks are inspired by the human brain.";

    #[tokio::test]
    async fn extractive_processor_populates_summary() {
        let processor = SummarizationProcessor;
        let config = ExtractionConfig {
            summarization: Some(SummarizationConfig {
                strategy: SummaryStrategy::Extractive,
                max_tokens: Some(40),
                llm: None,
            }),
            ..Default::default()
        };
        let mut result = ExtractedDocument {
            content: PARAGRAPH.to_string(),
            mime_type: Cow::Borrowed("text/plain"),
            detected_languages: Some(vec!["en".to_string()]),
            ..Default::default()
        };

        processor.process(&mut result, &config).await.unwrap();
        let summary = result.summary.as_ref().expect("summary populated");
        assert_eq!(summary.strategy, SummaryStrategy::Extractive);
        assert!(!summary.text.is_empty());
    }

    #[tokio::test]
    async fn no_config_means_no_summary() {
        let processor = SummarizationProcessor;
        let config = ExtractionConfig::default();
        let mut result = ExtractedDocument {
            content: PARAGRAPH.to_string(),
            mime_type: Cow::Borrowed("text/plain"),
            ..Default::default()
        };
        processor.process(&mut result, &config).await.unwrap();
        assert!(result.summary.is_none());
    }

    #[tokio::test]
    async fn empty_content_skips() {
        let processor = SummarizationProcessor;
        let config = ExtractionConfig {
            summarization: Some(SummarizationConfig::default()),
            ..Default::default()
        };
        let mut result = ExtractedDocument {
            content: String::new(),
            mime_type: Cow::Borrowed("text/plain"),
            ..Default::default()
        };
        processor.process(&mut result, &config).await.unwrap();
        assert!(result.summary.is_none());
    }

    #[test]
    fn stage_is_middle() {
        assert_eq!(SummarizationProcessor.processing_stage(), ProcessingStage::Middle);
    }

    #[test]
    fn plugin_metadata() {
        let p = SummarizationProcessor;
        assert_eq!(p.name(), "summarization");
        assert!(!p.version().is_empty());
    }

    #[test]
    fn should_warn_when_extractive_summariser_produces_no_output() {
        let mut result = ExtractedDocument {
            content: PARAGRAPH.to_string(),
            mime_type: Cow::Borrowed("text/plain"),
            ..Default::default()
        };

        apply_extractive_summary(&mut result, None);

        assert!(result.summary.is_none(), "no summary should be set on failure");
        assert_eq!(result.processing_warnings.len(), 1);
        assert_eq!(result.processing_warnings[0].source, "summarization_extractive");
        assert_eq!(
            result.processing_warnings[0].message,
            "Extractive summarisation produced no output for this document"
        );
    }

    #[test]
    fn should_warn_when_extractive_summariser_returns_empty_string() {
        let mut result = ExtractedDocument {
            content: PARAGRAPH.to_string(),
            mime_type: Cow::Borrowed("text/plain"),
            ..Default::default()
        };

        apply_extractive_summary(&mut result, Some(String::new()));

        assert!(result.summary.is_none());
        assert_eq!(result.processing_warnings.len(), 1);
        assert_eq!(result.processing_warnings[0].source, "summarization_extractive");
    }

    #[test]
    fn should_set_summary_and_emit_no_warning_when_summariser_succeeds() {
        let mut result = ExtractedDocument {
            content: PARAGRAPH.to_string(),
            mime_type: Cow::Borrowed("text/plain"),
            ..Default::default()
        };

        apply_extractive_summary(&mut result, Some("Machine learning is great.".to_string()));

        assert!(result.processing_warnings.is_empty());
        let summary = result.summary.as_ref().expect("summary populated");
        assert_eq!(summary.text, "Machine learning is great.");
        assert_eq!(summary.strategy, SummaryStrategy::Extractive);
        assert_eq!(summary.token_count, Some(4));
    }
}
