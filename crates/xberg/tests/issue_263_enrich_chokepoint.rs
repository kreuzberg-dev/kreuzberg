//! Regression tests for #263: the enrichment chokepoint must write its results onto the
//! document, not only into the side struct it returns.
//!
//! `enrich` used to return every stage's output solely in `EnrichedResult` and hand back
//! `EnrichedResult::extraction` untouched. `ExtractedDocument` is the only thing downstream
//! consumers ever see — it is what serializes to JSON, what the REST schema and the
//! language bindings expose, and what the post-processors and `split_and_extract` operate
//! on — so detected entities, page classifications and image captions produced by `enrich`
//! were invisible everywhere, and the `LlmUsage` for the classification and captioning
//! calls was discarded outright.

#[cfg(feature = "ner")]
mod stub_ner {
    use async_trait::async_trait;
    use xberg::Result;
    use xberg::text::ner::NerBackend;
    use xberg::types::entity::{Entity, EntityCategory};

    /// Deterministic backend: two fixed entities, no network, no model download.
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
                    start: 15,
                    end: 19,
                    confidence: Some(0.95),
                },
            ])
        }
    }
}

#[cfg(feature = "ner")]
fn ner_config() -> xberg::EnrichmentConfig {
    use std::sync::Arc;

    use xberg::EnrichmentConfig;
    use xberg::enrich::NerEnrichmentConfig;
    use xberg::types::entity::EntityCategory;

    // `EnrichmentConfig` gains fields under other features, so the update is
    // load-bearing there and only redundant in this narrow combination. ~keep
    #[allow(clippy::needless_update)]
    EnrichmentConfig {
        ner: Some(NerEnrichmentConfig {
            backend: Arc::new(stub_ner::StubBackend),
            categories: vec![EntityCategory::Person, EntityCategory::Organization],
        }),
        ..Default::default()
    }
}

#[cfg(feature = "ner")]
fn document(content: &str) -> xberg::types::ExtractedDocument {
    let mut result = xberg::types::ExtractedDocument::default();
    result.content = content.to_string();
    result
}

/// The NER stage must populate `ExtractedDocument::entities`, the canonical home for
/// detected entities, and not only `EnrichedResult::entities`.
#[cfg(feature = "ner")]
#[tokio::test]
async fn enrich_ner_writes_detected_entities_onto_the_extracted_document() {
    use xberg::enrich;
    use xberg::types::entity::EntityCategory;

    let enriched = enrich(document("Alice works at Acme Corp."), &ner_config())
        .await
        .expect("enrichment succeeds");

    let on_document = enriched
        .extraction
        .entities
        .as_ref()
        .expect("NER results must be written onto ExtractedDocument::entities");

    assert_eq!(on_document.len(), 2);
    assert_eq!(on_document[0].text, "Alice");
    assert_eq!(on_document[0].category, EntityCategory::Person);
    assert_eq!(on_document[1].text, "Acme");
    assert_eq!(on_document[1].category, EntityCategory::Organization);

    assert_eq!(
        on_document,
        enriched
            .entities
            .as_ref()
            .expect("side-channel entities stay populated"),
        "the document field and the returned field must carry the same entities"
    );
}

/// The consumer-visible proof of the defect: everything downstream reads the *serialized*
/// document, so entities that never land on it are invisible no matter what `enrich`
/// returned.
#[cfg(feature = "ner")]
#[tokio::test]
async fn enriched_document_serializes_the_detected_entities() {
    use xberg::enrich;

    let enriched = enrich(document("Alice works at Acme Corp."), &ner_config())
        .await
        .expect("enrichment succeeds");

    let json = serde_json::to_value(&enriched.extraction).expect("document serializes");

    assert_eq!(json["entities"][0]["text"], "Alice");
    assert_eq!(json["entities"][0]["category"], "person");
    assert_eq!(json["entities"][1]["text"], "Acme");
}

/// A no-op config must leave the document exactly as it arrived — the write-backs are
/// strictly opt-in per stage.
#[cfg(feature = "ner")]
#[tokio::test]
async fn enrich_without_ner_config_leaves_document_entities_unset() {
    use xberg::{EnrichmentConfig, enrich};

    let enriched = enrich(document("Alice works at Acme Corp."), &EnrichmentConfig::default())
        .await
        .expect("enrichment succeeds");

    assert!(enriched.extraction.entities.is_none());
}

/// The captioning stage now takes the image list out of the document to record usage, so it
/// must always put it back. Images with empty `data` are skipped (no VLM call), which keeps
/// this test hermetic.
#[cfg(feature = "captioning")]
#[tokio::test]
async fn enrich_captioning_preserves_the_documents_images() {
    use xberg::core::config::LlmConfig;
    use xberg::enrich::CaptioningEnrichmentConfig;
    use xberg::types::{ExtractedDocument, ExtractedImage};
    use xberg::{EnrichmentConfig, enrich};

    let mut extraction = ExtractedDocument::default();
    extraction.content = "one reference-only image".to_string();
    extraction.images = Some(vec![ExtractedImage::default()]);

    let config = EnrichmentConfig {
        captioning: Some(CaptioningEnrichmentConfig {
            config: LlmConfig::default(),
            custom_prompt: None,
        }),
        ..Default::default()
    };

    let enriched = enrich(extraction, &config).await.expect("enrichment succeeds");

    assert_eq!(enriched.captions, Some(vec![String::new()]));

    let images = enriched
        .extraction
        .images
        .as_ref()
        .expect("the image list must be restored on the document");
    assert_eq!(images.len(), 1);
    assert_eq!(images[0].caption, None, "an empty-data image costs no VLM call");
    assert!(
        enriched.extraction.llm_usage.is_none(),
        "no VLM call means no usage record"
    );
}
