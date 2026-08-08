//! Unified enrichment chokepoint composing captioning, NER, classification,
//! and (future) transcription on top of an [`ExtractedDocument`].
//!
//! # Design
//!
//! Each enrichment stage is independently optional and feature-gated. Passing
//! `None` for a stage's config field skips that stage entirely. Stages run
//! sequentially so that later stages can see prior results without complex
//! dependency tracking.
//!
//! ## Stage order
//!
//! 1. Classification — operates on the per-page `content` (or the whole document when it
//!    has no pages); writes `page_classifications` and appends `llm_usage`
//! 2. Chunk classification — multi-labels each entry of `ExtractedDocument::chunks`
//!    in place; a no-op when the document has no chunks
//! 3. NER — operates on the full document text (`content`); writes `entities`
//! 4. Captioning — operates on images extracted into `ExtractedDocument::images`; writes
//!    each image's `caption` / `description` and appends `llm_usage`
//!
//! ## Results live on the document
//!
//! Every stage writes its output onto the [`ExtractedDocument`] field that already exists
//! for it, not only into [`EnrichedResult`]. The document is the only thing downstream
//! consumers see — it is what serializes to JSON, what the REST schema and the language
//! bindings expose, and what `split_and_extract` and the post-processors operate on — so
//! results returned solely in the side struct were invisible to all of them, and the LLM
//! token / cost records for the classification and captioning calls were discarded
//! outright (#263).
//!
//! Transcription is reserved for a future backend and is kept present in the
//! config surface so callers can wire it today; any attempt to activate it
//! returns an explicit not-yet-implemented error.
//!
//! # Example
//!
//! ```ignore
//! use xberg::{ExtractInput, ExtractionConfig, extract, enrich, EnrichmentConfig};
//!
//! # async fn run() -> xberg::Result<()> {
//! let output = extract(ExtractInput::from_uri("document.pdf"), &ExtractionConfig::default()).await?;
//! let extraction = output.results.into_iter().next().expect("one input yields one result");
//! let config = EnrichmentConfig::default();
//! let enriched = enrich(extraction, &config).await?;
//! assert!(enriched.entities.is_none()); // no NER config supplied
//! # Ok(())
//! # }
//! ```

use crate::types::ExtractedDocument;

#[cfg(feature = "ner")]
use std::sync::Arc;

#[cfg(feature = "classification")]
use crate::ClassificationLabel;

#[cfg(feature = "ner")]
use crate::types::entity::{Entity, EntityCategory};

/// NER enrichment knob: which backend to use and which categories to request.
#[cfg(feature = "ner")]
pub struct NerEnrichmentConfig {
    /// The NER backend implementation. Wrap a concrete backend in `Arc` and
    /// assign it here:
    ///
    /// ```ignore
    /// use std::sync::Arc;
    /// use xberg::{LlmBackend, LlmConfig, enrich::NerEnrichmentConfig};
    ///
    /// let config = NerEnrichmentConfig {
    ///     backend: Arc::new(LlmBackend::new(LlmConfig::default())),
    ///     categories: vec![],
    /// };
    /// ```
    pub backend: Arc<dyn crate::text::ner::NerBackend>,
    /// Entity categories to detect. An empty slice tells the backend to return
    /// every category it recognises.
    pub categories: Vec<EntityCategory>,
}

/// Classification enrichment knob: how to label the document.
#[cfg(feature = "classification")]
pub struct ClassificationEnrichmentConfig {
    /// Label set and LLM settings for the classification stage.
    pub config: crate::core::config::PageClassificationConfig,
}

/// Chunk-classification enrichment knob: how to multi-label individual chunks.
///
/// Operates on `ExtractedDocument::chunks` in place — the caller must have
/// already produced chunks (e.g. via `ExtractionConfig::chunking`) for this
/// stage to have any effect; a document with no chunks is a no-op.
#[cfg(feature = "classification")]
pub struct ChunkClassificationEnrichmentConfig {
    /// Label-definition set and LLM/batching settings for the chunk-classification stage.
    pub config: crate::core::config::ChunkClassificationConfig,
}

/// Captioning enrichment knob: which LLM to use for image captions.
///
/// The enrichment stage calls [`crate::captioning::caption_image`] for every
/// image in `ExtractedDocument::images` that has non-empty `data`. Images with
/// empty byte data (e.g. reference-only images populated via `source_path`) are
/// skipped rather than forwarded to the VLM.
#[cfg(feature = "captioning")]
pub struct CaptioningEnrichmentConfig {
    /// LLM / VLM configuration forwarded verbatim to each `caption_image` call.
    pub config: crate::core::config::LlmConfig,
    /// Optional custom prompt override forwarded to every `caption_image` call.
    /// `None` uses the default `RegionKind::Caption` prompt.
    pub custom_prompt: Option<String>,
}

/// Aggregated enrichment configuration.
///
/// Each field is feature-gated and independently optional. Set a field to
/// `Some(...)` to activate the corresponding stage; leave it `None` to skip.
///
/// `EnrichmentConfig::default()` produces a no-op config: all stages skipped,
/// and `enrich` returns an `EnrichedResult` with all enrichment fields `None`.
#[derive(Default)]
pub struct EnrichmentConfig {
    /// NER stage.  `None` skips entity detection.
    #[cfg(feature = "ner")]
    pub ner: Option<NerEnrichmentConfig>,

    /// Document-classification stage.  `None` skips classification.
    #[cfg(feature = "classification")]
    pub classification: Option<ClassificationEnrichmentConfig>,

    /// Chunk-classification stage.  `None` skips per-chunk multi-label classification.
    #[cfg(feature = "classification")]
    pub chunk_classification: Option<ChunkClassificationEnrichmentConfig>,

    /// Image-captioning stage.  `None` skips captioning.
    #[cfg(feature = "captioning")]
    pub captioning: Option<CaptioningEnrichmentConfig>,

    /// Transcription stage (reserved — not yet implemented).
    ///
    /// Any `Some(...)` value causes `enrich` to return
    /// `Err(XbergError::Other("transcription backend not yet implemented"))`.
    /// Include config here now so call-sites compile and activate the stage once
    /// the backend lands.
    #[cfg(feature = "transcription-types")]
    pub transcription: Option<crate::core::config::TranscriptionConfig>,
}

/// Extraction result with optional enrichment layers applied.
///
/// `extraction` is the enriched [`ExtractedDocument`]: every stage writes its output onto
/// the canonical field that already exists for it, so enrichment survives serialization,
/// the REST/API schema, and the language bindings — all of which see the document and
/// nothing else. The fields below mirror those writes for callers that want the stage
/// output directly; they are `None` when the stage was not configured or the feature was
/// compiled out.
///
/// | Stage | Written onto `extraction` |
/// |---|---|
/// | Classification | `page_classifications`, `llm_usage` |
/// | Chunk classification | `chunks[].classification` (in place) |
/// | NER | `entities` |
/// | Captioning | `images[].caption`, `images[].description`, `llm_usage` |
pub struct EnrichedResult {
    /// The extraction result with every configured stage's output applied.
    pub extraction: ExtractedDocument,

    /// Detected named entities (populated by the NER stage).
    ///
    /// Mirrors [`ExtractedDocument::entities`](crate::types::ExtractedDocument::entities).
    #[cfg(feature = "ner")]
    pub entities: Option<Vec<Entity>>,

    /// Document-level classification labels (populated by the classification stage).
    ///
    /// `classify_document` aggregates across all pages; the result here is the
    /// post-aggregation label set (one entry for single-label mode, any subset
    /// of the configured labels for multi-label mode).
    #[cfg(feature = "classification")]
    pub classification: Option<Vec<ClassificationLabel>>,

    /// Per-image captions indexed parallel to `extraction.images`
    /// (populated by the captioning stage).
    ///
    /// `captions[i]` is the caption for `extraction.images.as_deref().unwrap()[i]`.
    /// Images whose `data` bytes were empty produce an empty string rather than a
    /// VLM call.
    #[cfg(feature = "captioning")]
    pub captions: Option<Vec<String>>,
}

/// Apply enrichment stages to an extraction result.
///
/// Stages run sequentially: classification → NER → captioning.
/// On any error the partial result is dropped and the error is returned
/// immediately.
///
/// # Example
///
/// ```ignore
/// use xberg::{ExtractInput, ExtractionConfig, extract, enrich, EnrichmentConfig};
///
/// # async fn run() -> xberg::Result<()> {
/// let output = extract(ExtractInput::from_uri("report.pdf"), &ExtractionConfig::default()).await?;
/// let extraction = output.results.into_iter().next().expect("one input yields one result");
///
/// // Skip all stages — identity pass.
/// let enriched = enrich(extraction, &EnrichmentConfig::default()).await?;
/// assert!(enriched.entities.is_none());
/// assert!(enriched.classification.is_none());
/// assert!(enriched.captions.is_none());
/// # Ok(())
/// # }
/// ```
///
/// # Errors
///
/// - [`crate::XbergError::Validation`] when the classification config has an
///   empty label set (propagated from `classify_document_onto`).
/// - [`crate::XbergError::Other`] when the NER or captioning backends fail. A captioning
///   failure still leaves `extraction.images` intact and records the usage for the calls
///   that already succeeded before the error propagates.
/// - [`crate::XbergError::Other`] when `config.transcription` is `Some`:
///   the transcription backend is not yet implemented.
#[cfg_attr(alef, alef(skip))]
#[cfg_attr(not(feature = "classification"), allow(unused_mut))]
pub async fn enrich(mut extraction: ExtractedDocument, config: &EnrichmentConfig) -> crate::Result<EnrichedResult> {
    // read inside `#[cfg(...)]` branches that are all compiled out — silence
    #[cfg(not(any(
        feature = "transcription-types",
        feature = "classification",
        feature = "ner",
        feature = "captioning",
    )))]
    let _ = config;

    #[cfg(feature = "transcription-types")]
    if config.transcription.is_some() {
        return Err(crate::XbergError::Other(
            "transcription backend not yet implemented; set config.transcription = None to skip".into(),
        ));
    }

    #[cfg(feature = "classification")]
    let classification = if let Some(ref cfg) = config.classification {
        // `classify_document_onto` also writes `page_classifications` and appends the
        // per-page `LlmUsage`; the plain `classify_document` dropped both (#263).
        Some(crate::text::classification::classify_document_onto(&mut extraction, &cfg.config).await?)
    } else {
        None
    };

    #[cfg(feature = "classification")]
    if let Some(ref cfg) = config.chunk_classification {
        crate::text::classification::classify_chunks(&mut extraction, &cfg.config).await?;
    }

    #[cfg(feature = "ner")]
    let entities = if let Some(ref cfg) = config.ner {
        let detected =
            crate::text::ner::detect_entities(&extraction.content, cfg.backend.as_ref(), &cfg.categories).await?;
        // `ExtractedDocument::entities` is the field the API schema, the bindings, and every
        // serializing consumer read. Without this write-back the detected entities existed
        // only in `EnrichedResult` and were invisible downstream (#263).
        extraction.entities = Some(detected.clone());
        Some(detected)
    } else {
        None
    };

    #[cfg(feature = "captioning")]
    let captions = if let Some(ref cfg) = config.captioning {
        Some(caption_images_onto(&mut extraction, cfg).await?)
    } else {
        None
    };

    Ok(EnrichedResult {
        extraction,
        #[cfg(feature = "ner")]
        entities,
        #[cfg(feature = "classification")]
        classification,
        #[cfg(feature = "captioning")]
        captions,
    })
}

/// Caption every image in `extraction` that carries bytes, writing each caption onto the
/// image and every VLM call's [`crate::types::LlmUsage`] onto the document.
///
/// Returns the captions positionally parallel to `extraction.images`. Images with empty
/// `data` (reference-only images populated via `source_path`) are skipped, cost no VLM
/// call, and yield an empty string — matching the documented contract of
/// [`CaptioningEnrichmentConfig`].
///
/// Captions land on `ExtractedImage::caption`, and additionally on
/// `ExtractedImage::description` when that is unset, because renderers emit `description`
/// at the image placeholder and never read `caption` — the same mirroring the built-in
/// captioning post-processor does. Without this the caption existed only in
/// `EnrichedResult::captions` and never reached the serialized document (#263).
///
/// On a VLM failure the images taken out of `extraction` are put back, and the usage for
/// the calls that did succeed is recorded, before the error propagates; otherwise a failure
/// on image *n* would silently strip the document's entire image list.
#[cfg(feature = "captioning")]
async fn caption_images_onto(
    extraction: &mut ExtractedDocument,
    cfg: &CaptioningEnrichmentConfig,
) -> crate::Result<Vec<String>> {
    /// Put the borrowed image list back and record the usage accumulated so far.
    fn commit(
        extraction: &mut ExtractedDocument,
        images: Vec<crate::types::ExtractedImage>,
        usages: Vec<crate::types::LlmUsage>,
    ) {
        extraction.images = Some(images);
        if !usages.is_empty() {
            extraction.llm_usage.get_or_insert_with(Vec::new).extend(usages);
        }
    }

    // Taken out so `extraction.llm_usage` can be written inside the loop without holding a
    // second mutable borrow of `extraction`.
    let Some(mut images) = extraction.images.take() else {
        return Ok(Vec::new());
    };

    let mut captions = Vec::with_capacity(images.len());
    let mut usages: Vec<crate::types::LlmUsage> = Vec::new();

    for index in 0..images.len() {
        let data = images[index].data.clone();
        if data.is_empty() {
            captions.push(String::new());
            continue;
        }

        match crate::captioning::caption_image_with_usage(&data, &cfg.config, cfg.custom_prompt.as_deref()).await {
            Ok((caption, usage)) => {
                let caption = caption.trim().to_string();
                if !caption.is_empty() {
                    if images[index].description.is_none() {
                        images[index].description = Some(caption.clone());
                    }
                    images[index].caption = Some(caption.clone());
                }
                if let Some(mut usage) = usage {
                    if usage.source.is_empty() || usage.source == "vlm_ocr" {
                        usage.source = "captioning".to_string();
                    }
                    usages.push(usage);
                }
                captions.push(caption);
            }
            Err(error) => {
                commit(extraction, images, usages);
                return Err(error);
            }
        }
    }

    commit(extraction, images, usages);
    Ok(captions)
}
