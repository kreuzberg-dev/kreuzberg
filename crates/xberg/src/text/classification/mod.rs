//! Per-page LLM classification.
//!
//! Walks the rendered `content`, slices it on the page boundary metadata produced
//! during extraction, and asks the configured LLM to assign one or more labels
//! from a fixed vocabulary to each page. Results land on
//! [`ExtractedDocument::page_classifications`](crate::types::ExtractedDocument::page_classifications).
//!
//! Triggered by [`ExtractionConfig::page_classification`](crate::core::config::ExtractionConfig::page_classification);
//! invoked by the Middle-stage post-processor in
//! [`crate::plugins::processor::builtin::classification`].

pub mod chunk_classifier;
pub mod page_classifier;

pub use chunk_classifier::classify_chunks;
pub use page_classifier::{classify_pages, classify_text};

/// Classify a single document (as multiple pages or a single text block).
///
/// Aggregates classifications across all pages in the provided text, returning
/// a combined label set that represents the document as a whole.
///
/// # Arguments
///
/// * `pages` - Slice of page texts to classify. Each page is classified independently
///   using the configured LLM, and results are aggregated.
/// * `config` - Classification configuration including labels and LLM settings.
///
/// # Returns
///
/// A vector of `ClassificationLabel` entries representing the document's overall classification.
///
/// # Errors
///
/// Returns an error if `config.labels` is empty or if LLM calls fail.
///
/// # Example
///
/// ```rust,no_run
/// use xberg::text::classification::classify_document;
/// use xberg::core::config::PageClassificationConfig;
/// use xberg::core::config::LlmConfig;
///
/// # async fn example() -> xberg::Result<()> {
/// let config = PageClassificationConfig {
///     labels: vec!["invoice".to_string(), "memo".to_string()],
///     llm: LlmConfig::default(),
///     prompt_template: None,
///     multi_label: false,
/// };
///
/// let pages = vec!["Page 1 content", "Page 2 content"];
/// let labels = classify_document(&pages, &config).await?;
/// # Ok(())
/// # }
/// ```
pub async fn classify_document(
    pages: &[&str],
    config: &crate::core::config::PageClassificationConfig,
) -> crate::Result<Vec<crate::ClassificationLabel>> {
    if config.labels.is_empty() {
        return Err(crate::XbergError::validation(
            "PageClassificationConfig.labels must contain at least one entry",
        ));
    }

    if pages.is_empty() {
        return Ok(Vec::new());
    }

    let ctx = page_classifier::ClassifyContext::new(config);
    let mut per_page_labels: Vec<Vec<crate::ClassificationLabel>> = Vec::new();

    for page_text in pages {
        if page_text.is_empty() {
            continue;
        }
        // `classify_document` returns a bare `Vec<ClassificationLabel>` with no slot to
        // carry usage, so this entry point still drops it. Callers that own an
        // `ExtractedDocument` must use `classify_document_onto`, which records both the
        // per-page labels and every call's `LlmUsage` on the document (#263).
        let (labels, _usage) = page_classifier::classify_one(page_text, &ctx, config).await?;
        per_page_labels.push(labels);
    }

    Ok(finalize_document_labels(&per_page_labels, config.multi_label))
}

/// Classify `result`'s pages and record everything the call produced **on the document**.
///
/// This is the write-back counterpart of [`classify_document`]. Besides returning the
/// aggregated document-level label set it also populates
/// [`ExtractedDocument::page_classifications`](crate::types::ExtractedDocument::page_classifications)
/// with the per-page detail and appends every LLM call's
/// [`LlmUsage`](crate::types::LlmUsage) to
/// [`ExtractedDocument::llm_usage`](crate::types::ExtractedDocument::llm_usage).
///
/// `classify_document` can do neither: it takes borrowed page slices and returns a bare
/// label vector, so its caller's token/cost accounting and per-page labels were silently
/// discarded — the enrichment chokepoint's classification stage was the only caller and
/// therefore produced results that never reached the serialized document (#263).
///
/// Pages are the per-page `content` entries when the document has them, otherwise the
/// whole `content` as a single page. Empty pages are skipped and consume no LLM call, but
/// still keep their 1-indexed page number for the pages that follow.
///
/// # Errors
///
/// Returns [`crate::XbergError::Validation`] when `config.labels` is empty, or any error
/// raised by prompt rendering or the underlying LLM call.
pub(crate) async fn classify_document_onto(
    result: &mut crate::types::ExtractedDocument,
    config: &crate::core::config::PageClassificationConfig,
) -> crate::Result<Vec<crate::ClassificationLabel>> {
    if config.labels.is_empty() {
        return Err(crate::XbergError::validation(
            "PageClassificationConfig.labels must contain at least one entry",
        ));
    }

    let pages: Vec<String> = match result.pages.as_deref() {
        Some(pages) if !pages.is_empty() => pages.iter().map(|page| page.content.clone()).collect(),
        _ => vec![result.content.clone()],
    };

    let ctx = page_classifier::ClassifyContext::new(config);
    let mut per_page_labels: Vec<Vec<crate::ClassificationLabel>> = Vec::new();
    let mut classifications: Vec<crate::types::classification::PageClassification> = Vec::new();
    let mut usages: Vec<crate::types::LlmUsage> = Vec::new();

    for (index, page_text) in pages.iter().enumerate() {
        if page_text.is_empty() {
            continue;
        }
        let (labels, usage) = page_classifier::classify_one(page_text, &ctx, config).await?;
        if let Some(usage) = usage {
            usages.push(usage);
        }
        classifications.push(crate::types::classification::PageClassification {
            page_number: index as u32 + 1,
            labels: labels.clone(),
        });
        per_page_labels.push(labels);
    }

    if !classifications.is_empty() {
        result.page_classifications = Some(classifications);
    }
    if !usages.is_empty() {
        result.llm_usage.get_or_insert_with(Vec::new).extend(usages);
    }

    Ok(finalize_document_labels(&per_page_labels, config.multi_label))
}

/// Reduce per-page labels to the document-level label set: every label in multi-label
/// mode (sorted by name), or the single highest-confidence label otherwise.
fn finalize_document_labels(
    per_page_labels: &[Vec<crate::ClassificationLabel>],
    multi_label: bool,
) -> Vec<crate::ClassificationLabel> {
    let aggregated = aggregate_page_labels(per_page_labels);

    if multi_label {
        let mut labels = aggregated;
        labels.sort_by(|a, b| a.label.cmp(&b.label));
        labels
    } else {
        let best = aggregated.into_iter().max_by(|a, b| {
            let a_score = a.confidence.unwrap_or(0.0);
            let b_score = b.confidence.unwrap_or(0.0);
            a_score.partial_cmp(&b_score).unwrap_or(std::cmp::Ordering::Equal)
        });
        best.into_iter().collect()
    }
}

/// Aggregate per-page classification labels into one label set, averaging
/// confidence across every page that reported the same label instead of
/// keeping an arbitrary single page's score (#265).
///
/// A label's confidence is the mean of the confidences reported for it by
/// pages that included one; if no page reported a confidence for a label, the
/// aggregated confidence is `None`. Labels are returned in first-seen order
/// across `per_page_labels`.
fn aggregate_page_labels(per_page_labels: &[Vec<crate::ClassificationLabel>]) -> Vec<crate::ClassificationLabel> {
    let mut order: Vec<String> = Vec::new();
    let mut counts: std::collections::HashMap<String, (f32, u32)> = std::collections::HashMap::new();

    for labels in per_page_labels {
        for label in labels {
            let entry = counts.entry(label.label.clone()).or_insert_with(|| {
                order.push(label.label.clone());
                (0.0, 0)
            });
            if let Some(conf) = label.confidence {
                entry.0 += conf;
                entry.1 += 1;
            }
        }
    }

    order
        .into_iter()
        .map(|label| {
            let (sum, count) = counts[&label];
            let confidence = if count > 0 { Some(sum / count as f32) } else { None };
            crate::ClassificationLabel { label, confidence }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ClassificationLabel;

    fn label(name: &str, confidence: Option<f32>) -> ClassificationLabel {
        ClassificationLabel {
            label: name.to_string(),
            confidence,
        }
    }

    #[test]
    fn should_average_confidence_across_pages_reporting_the_same_label() {
        let per_page = vec![
            vec![label("invoice", Some(0.6))],
            vec![label("invoice", Some(0.8)), label("memo", Some(0.2))],
        ];

        let aggregated = aggregate_page_labels(&per_page);

        assert_eq!(aggregated.len(), 2);
        assert_eq!(aggregated[0].label, "invoice");
        // (0.6_f32 + 0.8_f32) / 2.0 — exact f32 arithmetic result, not the
        // mathematically rounded 0.7 (f32 cannot represent it exactly).
        assert_eq!(aggregated[0].confidence, Some(0.700_000_05));
        assert_eq!(aggregated[1].label, "memo");
        assert_eq!(aggregated[1].confidence, Some(0.2));
    }

    #[test]
    fn should_return_none_confidence_when_no_page_reported_one() {
        let per_page = vec![vec![label("invoice", None)], vec![label("invoice", None)]];

        let aggregated = aggregate_page_labels(&per_page);

        assert_eq!(aggregated.len(), 1);
        assert_eq!(aggregated[0].label, "invoice");
        assert_eq!(aggregated[0].confidence, None);
    }

    #[test]
    fn should_average_only_over_pages_that_reported_a_confidence() {
        let per_page = vec![vec![label("invoice", Some(0.9))], vec![label("invoice", None)]];

        let aggregated = aggregate_page_labels(&per_page);

        assert_eq!(aggregated.len(), 1);
        assert_eq!(
            aggregated[0].confidence,
            Some(0.9),
            "the None entry must not dilute the average"
        );
    }

    #[test]
    fn should_preserve_first_seen_order() {
        let per_page = vec![vec![label("zeta", Some(0.5)), label("alpha", Some(0.5))]];

        let aggregated = aggregate_page_labels(&per_page);

        let names: Vec<&str> = aggregated.iter().map(|l| l.label.as_str()).collect();
        assert_eq!(names, vec!["zeta", "alpha"]);
    }
}
