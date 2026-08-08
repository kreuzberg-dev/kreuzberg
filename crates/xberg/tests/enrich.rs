//! Integration tests for the unified enrichment chokepoint.

use xberg::types::ExtractedDocument;
use xberg::{EnrichedResult, EnrichmentConfig, enrich};

fn bare_result(content: &str) -> ExtractedDocument {
    let mut result = ExtractedDocument::default();
    result.content = content.to_string();
    result
}

/// A default config is a no-op: the extraction passes through unchanged and
/// every enrichment field is `None`.
#[tokio::test]
async fn enrich_with_no_config_is_identity() {
    let extraction = bare_result("Some document text.");
    let config = EnrichmentConfig::default();
    let enriched: EnrichedResult = enrich(extraction.clone(), &config).await.unwrap();

    assert_eq!(enriched.extraction.content, extraction.content);

    #[cfg(feature = "ner")]
    assert!(enriched.entities.is_none());

    #[cfg(feature = "classification")]
    assert!(enriched.classification.is_none());

    #[cfg(feature = "captioning")]
    assert!(enriched.captions.is_none());
}

/// Captioning with an empty images list produces `Some(vec![])`, not `None`.
#[cfg(feature = "captioning")]
#[tokio::test]
async fn enrich_captioning_empty_images_yields_empty_vec() {
    use xberg::core::config::LlmConfig;
    use xberg::enrich::CaptioningEnrichmentConfig;

    let extraction = bare_result("no images here");
    let config = EnrichmentConfig {
        captioning: Some(CaptioningEnrichmentConfig {
            config: LlmConfig::default(),
            custom_prompt: None,
        }),
        ..Default::default()
    };
    let enriched = enrich(extraction, &config).await.unwrap();
    assert_eq!(enriched.captions, Some(vec![]));

    #[cfg(feature = "ner")]
    assert!(enriched.entities.is_none());
    #[cfg(feature = "classification")]
    assert!(enriched.classification.is_none());
}

/// Classification alone: entities and captions stay `None`.
#[cfg(feature = "classification")]
#[tokio::test]
async fn enrich_classification_leaves_other_stages_none() {
    use xberg::core::config::{LlmConfig, PageClassificationConfig};
    use xberg::enrich::ClassificationEnrichmentConfig;

    let extraction = bare_result("An invoice for $100.");
    let config = EnrichmentConfig {
        classification: Some(ClassificationEnrichmentConfig {
            config: PageClassificationConfig {
                labels: vec!["invoice".to_string(), "memo".to_string()],
                llm: LlmConfig::default(),
                prompt_template: None,
                multi_label: false,
            },
        }),
        ..Default::default()
    };

    if let Ok(enriched) = enrich(extraction, &config).await {
        #[cfg(feature = "ner")]
        assert!(enriched.entities.is_none());
        #[cfg(feature = "captioning")]
        assert!(enriched.captions.is_none());
    }
}

/// Stub NerBackend that returns two hardcoded entities.
#[cfg(feature = "ner")]
mod stub_ner {
    use async_trait::async_trait;
    use xberg::Result;
    use xberg::text::ner::NerBackend;
    use xberg::types::entity::{Entity, EntityCategory};

    pub struct StubBackend;

    #[async_trait]
    impl NerBackend for StubBackend {
        async fn detect(&self, _text: &str, _categories: &[EntityCategory]) -> Result<Vec<Entity>> {
            Ok(vec![
                Entity {
                    category: EntityCategory::Person,
                    text: "Alice".to_string(),
                    start: 0,
                    end: 5,
                    confidence: Some(0.99),
                },
                Entity {
                    category: EntityCategory::Organization,
                    text: "Acme".to_string(),
                    start: 16,
                    end: 20,
                    confidence: Some(0.95),
                },
            ])
        }
    }
}

/// NER with a stub backend: entities are populated, classification and captions stay None.
#[cfg(feature = "ner")]
#[tokio::test]
async fn enrich_ner_with_stub_backend_populates_entities() {
    use std::sync::Arc;

    use xberg::enrich::NerEnrichmentConfig;
    use xberg::types::entity::EntityCategory;

    use stub_ner::StubBackend;

    let extraction = bare_result("Alice works at Acme Corp.");
    // `EnrichmentConfig` gains fields under other features, so the update is
    // load-bearing there and only redundant in this narrow combination. ~keep
    #[allow(clippy::needless_update)]
    let config = EnrichmentConfig {
        ner: Some(NerEnrichmentConfig {
            backend: Arc::new(StubBackend),
            categories: vec![EntityCategory::Person, EntityCategory::Organization],
        }),
        ..Default::default()
    };

    let enriched = enrich(extraction, &config).await.unwrap();
    let entities = enriched.entities.expect("entities should be Some");
    assert_eq!(entities.len(), 2);
    assert_eq!(entities[0].text, "Alice");
    assert_eq!(entities[0].category, EntityCategory::Person);
    assert_eq!(entities[1].text, "Acme");
    assert_eq!(entities[1].category, EntityCategory::Organization);

    #[cfg(feature = "classification")]
    assert!(enriched.classification.is_none());
    #[cfg(feature = "captioning")]
    assert!(enriched.captions.is_none());
}

/// When `transcription` is `Some`, `enrich` must return an error (not yet implemented).
#[cfg(feature = "transcription-types")]
#[tokio::test]
async fn enrich_transcription_returns_not_implemented_error() {
    use xberg::core::config::TranscriptionConfig;

    let extraction = bare_result("audio transcript placeholder");
    let config = EnrichmentConfig {
        transcription: Some(TranscriptionConfig::default()),
        ..Default::default()
    };

    let result = enrich(extraction, &config).await;
    match result {
        Ok(_) => panic!("transcription must return an error until the backend lands"),
        Err(e) => {
            let msg = e.to_string();
            assert!(
                msg.contains("transcription"),
                "error message should mention transcription; got: {msg}"
            );
        }
    }
}
