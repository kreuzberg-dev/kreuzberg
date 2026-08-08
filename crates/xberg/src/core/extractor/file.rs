//! File-based extraction operations.
//!
//! This module handles extraction from filesystem paths, including:
//! - MIME type detection and validation
//! - Legacy format conversion (DOC, PPT)
//! - File validation and reading
//! - Extraction pipeline orchestration

use crate::Result;
use crate::XbergError;
use crate::core::config::ExtractionConfig;
use crate::core::mime::{LEGACY_POWERPOINT_MIME_TYPE, LEGACY_WORD_MIME_TYPE};
use crate::plugins::InternalDocumentExtractor;
use crate::plugins::registry::RegisteredDocumentExtractor;
use crate::types::ExtractedDocument;
use std::path::Path;
#[cfg(feature = "otel")]
use tracing::Instrument;

use super::helpers::get_extractor;

/// Extract content from a file.
///
/// This is the main entry point for file-based extraction. It performs the following steps:
/// 1. Check cache for existing result (if caching enabled)
/// 2. Detect or validate MIME type
/// 3. Select appropriate extractor from registry
/// 4. Extract content
/// 5. Run post-processing pipeline
/// 6. Store result in cache (if caching enabled)
///
/// # Arguments
///
/// * `path` - Path to the file to extract
/// * `mime_type` - Optional MIME type override. If None, will be auto-detected
/// * `config` - Extraction configuration
///
/// # Returns
///
/// An `ExtractedDocument` containing the extracted content and metadata.
///
/// # Errors
///
/// Returns `XbergError::Io` if the file doesn't exist (NotFound) or for other file I/O errors.
/// Returns `XbergError::UnsupportedFormat` if MIME type is not supported.
///
/// # Example
///
/// This function is crate-internal; the public entry point that reaches it is
/// [`crate::extract`] with a URI input.
///
/// ```rust,no_run
/// use xberg::{ExtractInput, ExtractionConfig, extract};
///
/// # async fn example() -> xberg::Result<()> {
/// let config = ExtractionConfig::default();
/// let output = extract(ExtractInput::from_uri("document.pdf"), &config).await?;
/// println!("Content: {}", output.results[0].content);
/// # Ok(())
/// # }
/// ```
#[cfg_attr(feature = "otel", tracing::instrument(
    skip(config, path),
    fields(
        { crate::telemetry::conventions::OPERATION } = crate::telemetry::conventions::operations::EXTRACT_FILE,
        { crate::telemetry::conventions::DOCUMENT_FILENAME } = tracing::field::Empty,
        { crate::telemetry::conventions::OTEL_STATUS_CODE } = tracing::field::Empty,
        { crate::telemetry::conventions::ERROR_TYPE } = tracing::field::Empty,
        { crate::telemetry::conventions::ERROR_MESSAGE } = tracing::field::Empty,
    )
))]
pub(crate) async fn extract_file(
    path: impl AsRef<Path>,
    mime_type: Option<&str>,
    config: &ExtractionConfig,
) -> Result<ExtractedDocument> {
    use crate::core::{io, mime};

    let path = path.as_ref();

    #[cfg(feature = "otel")]
    {
        let span = tracing::Span::current();
        span.record(
            crate::telemetry::conventions::DOCUMENT_FILENAME,
            crate::telemetry::spans::sanitize_path(path),
        );
    }

    let extraction_future = Box::pin(async {
        io::validate_file_exists(path)?;

        if config.force_ocr && config.effective_disable_ocr() {
            return Err(crate::XbergError::Validation {
                message: "force_ocr and disable_ocr cannot both be true".to_string(),
                source: None,
            });
        }

        if matches!(
            config.ocr_strategy,
            crate::core::config::OcrStrategy::ScannedPages { .. }
        ) && config.effective_disable_ocr()
        {
            return Err(crate::XbergError::Validation {
                message: "ocr_strategy selects scanned pages for OCR, but disable_ocr is true".to_string(),
                source: None,
            });
        }

        let detected_mime = mime::detect_or_validate(path.to_str(), mime_type)?;

        #[cfg(not(feature = "office"))]
        match detected_mime.as_str() {
            LEGACY_WORD_MIME_TYPE => {
                return Err(XbergError::UnsupportedFormat(
                    "Legacy Word extraction requires the `office` feature".to_string(),
                ));
            }
            LEGACY_POWERPOINT_MIME_TYPE => {
                return Err(XbergError::UnsupportedFormat(
                    "Legacy PowerPoint extraction requires the `office` feature".to_string(),
                ));
            }
            _ => {}
        }

        #[cfg(feature = "office")]
        {
            let _ = LEGACY_WORD_MIME_TYPE;
            let _ = LEGACY_POWERPOINT_MIME_TYPE;
        }

        Box::pin(extract_file_with_extractor(path, &detected_mime, config)).await
    });

    // without a JS/WASI shim), which aborts the whole module with an uncatchable
    // `unreachable` trap. `tokio-runtime` can be enabled transitively on that target
    // (e.g. by `layout-tract` inside `wasm-target`), so gating on the feature alone is
    // not enough — explicitly exclude wasm32 here, matching `run_timed_extraction` in
    #[cfg(all(feature = "tokio-runtime", not(target_arch = "wasm32")))]
    let result = if let Some(secs) = config.extraction_timeout_secs {
        let start = std::time::Instant::now();
        match tokio::time::timeout(std::time::Duration::from_secs(secs), extraction_future).await {
            Ok(inner) => inner,
            Err(_elapsed) => {
                if let Some(ref token) = config.cancel_token {
                    token.cancel();
                }
                Err(crate::XbergError::Timeout {
                    elapsed_ms: start.elapsed().as_millis() as u64,
                    limit_ms: secs * 1000,
                })
            }
        }
    } else {
        extraction_future.await
    };

    #[cfg(any(not(feature = "tokio-runtime"), target_arch = "wasm32"))]
    let result = {
        // Without a usable tokio timer (no 'tokio-runtime' feature, or the WASM build,
        // where `std::time::Instant::now()` panics) there is no timer to enforce a
        // timeout, but the default ExtractionConfig sets extraction_timeout_secs, so
        // erroring here would reject every default call. Ignore the unenforceable
        // limit and run the extraction instead. ~keep
        if config.extraction_timeout_secs.is_some() {
            tracing::debug!(
                "extraction_timeout_secs is ignored on this target (no usable tokio timer); running without a timeout"
            );
        }
        extraction_future.await
    };

    #[cfg(feature = "otel")]
    if let Err(ref e) = result {
        crate::telemetry::spans::record_error_on_current_span(e);
    }

    result
}

pub(in crate::core::extractor) async fn extract_file_with_extractor(
    path: &Path,
    mime_type: &str,
    config: &ExtractionConfig,
) -> Result<ExtractedDocument> {
    let config = config.normalized();
    let config = config.as_ref();

    if !config.use_cache || config.cache_ttl_secs == Some(0) {
        return extract_file_uncached(path, mime_type, config).await;
    }

    let content_hash = crate::cache::blake3_hash_file(path)?;
    let config_hash = hash_extraction_config(config, mime_type);
    let cache_key = format!("{content_hash}_{config_hash}");

    let namespace = config.cache_namespace.as_deref();

    if let Some(cache) = get_extraction_cache()
        && let Ok(Some(data)) = cache.get(&cache_key, path.to_str(), namespace, config.cache_ttl_secs)
        && let Ok(result) = rmp_serde::from_slice::<ExtractedDocument>(&data)
    {
        tracing::debug!(cache_key = %cache_key, "Extraction cache hit");
        return Ok(result);
    }

    let result = Box::pin(extract_file_uncached(path, mime_type, config)).await?;

    if let Some(cache) = get_extraction_cache()
        && let Ok(data) = rmp_serde::to_vec(&result)
    {
        let _ = cache.set(&cache_key, data, path.to_str(), namespace, config.cache_ttl_secs);
    }

    Ok(result)
}

/// Whether an extractor failure is eligible for the extractor fallback chain (#217).
///
/// Only failures indicating that *this* extractor could not handle the input in a
/// way another registered extractor for the same MIME type plausibly could are
/// eligible:
///
/// - [`XbergError::UnsupportedFormat`] — the extractor determined, past MIME-based
///   selection, that it does not actually support this content (e.g. a container
///   format that only handles some of its own sub-variants).
/// - [`XbergError::Plugin`] — a third-party extractor's own reported failure. That
///   is a property of the plugin, not necessarily of the document.
///
/// Every other variant is treated as a hard failure and is *not* retried with a
/// lower-priority extractor. In particular [`XbergError::Parsing`] — the variant
/// an encrypted file or a corrupt archive surfaces as — means the document itself
/// is the problem: every other extractor registered for the same MIME type would
/// almost certainly fail identically, so cascading through them would only add
/// latency before producing a confusing final error (e.g. a generic archive
/// extractor's error swallowing the specific "wrong password" message from the
/// primary one).
fn is_extractor_fallback_eligible(error: &XbergError) -> bool {
    matches!(error, XbergError::UnsupportedFormat(_) | XbergError::Plugin { .. })
}

/// Extract without caching logic.
///
/// Fetches extractor candidates for `mime_type` from the process-global
/// [`crate::plugins::registry::DocumentExtractorRegistry`] and delegates the
/// dispatch/fallback logic to [`extract_with_candidates`].
async fn extract_file_uncached(path: &Path, mime_type: &str, config: &ExtractionConfig) -> Result<ExtractedDocument> {
    let budget = crate::core::config::concurrency::resolve_thread_budget(config.concurrency.as_ref());
    crate::core::config::concurrency::init_thread_pools(budget);

    crate::extractors::ensure_initialized()?;

    let candidates = {
        let registry = crate::plugins::registry::get_document_extractor_registry();
        let registry_read = registry.read();
        registry_read.get_candidates(path, mime_type)
    };

    extract_with_candidates(path, mime_type, config, candidates).await
}

/// Tries every extractor `candidates` in order (highest priority first,
/// see [`crate::plugins::registry::DocumentExtractorRegistry::get_candidates`]),
/// falling back to the next candidate only when the failure is
/// [fallback-eligible](is_extractor_fallback_eligible) (#217). A
/// `ProcessingWarning` records which extractor ultimately ran and why whenever a
/// higher-priority extractor was tried and failed first.
///
/// Parameterized on `candidates` rather than fetching them itself so callers
/// (in particular tests) can supply candidates from a local registry instead
/// of the process-global one, without racing concurrently running extraction
/// paths that self-heal the global registry only when it is observed
/// completely empty (see `crate::extractors::ensure_initialized`).
pub(crate) async fn extract_with_candidates(
    path: &Path,
    mime_type: &str,
    config: &ExtractionConfig,
    candidates: Vec<RegisteredDocumentExtractor>,
) -> Result<ExtractedDocument> {
    if candidates.is_empty() {
        return Err(XbergError::UnsupportedFormat(mime_type.to_string()));
    }

    let candidate_count = candidates.len();
    let mut last_error = None;

    for (index, candidate) in candidates.into_iter().enumerate() {
        // The extraction stage span wraps only the extractor invocation — post-processing
        // is covered by `run_pipeline` below and must not be nested inside it.
        #[cfg(feature = "otel")]
        let extraction = {
            let stage_span = crate::telemetry::spans::extraction_stage_span(
                candidate.plugin().name(),
                candidate.plugin().priority(),
            );
            candidate
                .extract_path(path, mime_type, config)
                .instrument(stage_span)
                .await
        };
        #[cfg(not(feature = "otel"))]
        let extraction = candidate.extract_path(path, mime_type, config).await;

        match extraction {
            Ok(mut doc) => {
                if index > 0 {
                    let name = candidate.plugin().name();
                    crate::core::diagnostics::push_warning(
                        &mut doc.processing_warnings,
                        "extractor-fallback",
                        format!(
                            "extractor '{name}' handled this document for MIME type '{mime_type}' after \
                             {index} higher-priority extractor(s) failed"
                        ),
                    );
                }
                let result = Box::pin(crate::core::pipeline::run_pipeline(doc, config)).await?;
                return Ok(result);
            }
            Err(e) if index + 1 < candidate_count && is_extractor_fallback_eligible(&e) => {
                tracing::debug!(
                    "Extractor '{}' failed for MIME type '{}' with a fallback-eligible error, \
                     trying the next candidate: {}",
                    candidate.plugin().name(),
                    mime_type,
                    e
                );
                last_error = Some(e);
            }
            Err(e) => return Err(e),
        }
    }

    Err(last_error.unwrap_or_else(|| XbergError::UnsupportedFormat(mime_type.to_string())))
}

/// Hash ExtractionConfig fields that affect extraction output.
///
/// Excludes cache-control fields (use_cache, cache_namespace, cache_ttl_secs)
/// since they don't affect the extraction result. Uses a clone-and-normalize
/// approach to ensure determinism: cache fields are zeroed, then the struct
/// is serialized to canonical JSON via serde_json's sorted-keys representation.
fn hash_extraction_config(config: &ExtractionConfig, mime_type: &str) -> String {
    let mut normalized = config.clone();
    normalized.use_cache = true;
    normalized.cache_namespace = None;
    normalized.cache_ttl_secs = None;

    let mut hasher = blake3::Hasher::new();
    hasher.update(mime_type.as_bytes());
    if let Ok(bytes) = rmp_serde::to_vec(&normalized) {
        hasher.update(&bytes);
    }

    // `#[serde(skip)]` fields are absent from the MessagePack bytes above but DO
    hasher.update(b"\x00source_name\x00");
    if let Some(name) = normalized.source_name.as_deref() {
        hasher.update(name.as_bytes());
    }
    hasher.update(b"\x00tessdata\x00");
    if let Some(ocr) = normalized.ocr.as_ref()
        && let Some(tessdata) = ocr.tessdata_bytes.as_ref()
    {
        let mut keys: Vec<&String> = tessdata.keys().collect();
        keys.sort();
        for key in keys {
            hasher.update(key.as_bytes());
            hasher.update(&(tessdata[key].len() as u64).to_le_bytes());
            hasher.update(&tessdata[key]);
        }
    }

    let hash = hasher.finalize();
    hex::encode(&hash.as_bytes()[..16])
}

/// Get or initialize the global extraction cache.
fn get_extraction_cache() -> Option<&'static crate::cache::GenericCache> {
    use std::sync::OnceLock;
    static CACHE: OnceLock<Option<crate::cache::GenericCache>> = OnceLock::new();

    CACHE
        .get_or_init(|| crate::cache::GenericCache::new("extraction".to_string(), None, 30.0, 2000.0, 500.0).ok())
        .as_ref()
}

pub(in crate::core::extractor) async fn extract_bytes_with_extractor(
    content: &[u8],
    mime_type: &str,
    config: &ExtractionConfig,
) -> Result<ExtractedDocument> {
    let config = config.normalized();
    let config = config.as_ref();

    let budget = crate::core::config::concurrency::resolve_thread_budget(config.concurrency.as_ref());
    crate::core::config::concurrency::init_thread_pools(budget);

    crate::extractors::ensure_initialized()?;

    let extractor = get_extractor(mime_type)?;

    #[cfg(feature = "otel")]
    let doc = {
        let stage_span = crate::telemetry::spans::extraction_stage_span(extractor.name(), extractor.priority());
        Box::pin(extractor.extract_content(content, mime_type, config))
            .instrument(stage_span)
            .await?
    };
    #[cfg(not(feature = "otel"))]
    let doc = Box::pin(extractor.extract_content(content, mime_type, config)).await?;

    let result = Box::pin(crate::core::pipeline::run_pipeline(doc, config)).await?;
    Ok(result)
}

#[cfg(test)]
mod cache_key_tests {
    use super::hash_extraction_config;
    use crate::core::config::ExtractionConfig;

    #[test]
    fn source_name_changes_the_cache_key() {
        let a = ExtractionConfig {
            source_name: Some("snippet.py".to_string()),
            ..Default::default()
        };
        let b = ExtractionConfig {
            source_name: Some("snippet.rb".to_string()),
            ..Default::default()
        };
        assert_ne!(
            hash_extraction_config(&a, "text/x-source-code"),
            hash_extraction_config(&b, "text/x-source-code"),
            "source_name (serde-skipped) must be part of the cache key"
        );
    }

    #[test]
    #[cfg(feature = "ocr")]
    fn tessdata_bytes_changes_the_cache_key() {
        use crate::core::config::OcrConfig;
        use std::collections::HashMap;

        let mut eng = HashMap::new();
        eng.insert("eng".to_string(), vec![1u8, 2, 3]);
        let mut deu = HashMap::new();
        deu.insert("eng".to_string(), vec![9u8, 9, 9]);

        let a = ExtractionConfig {
            ocr: Some(OcrConfig {
                tessdata_bytes: Some(eng),
                ..OcrConfig::default()
            }),
            ..Default::default()
        };
        let b = ExtractionConfig {
            ocr: Some(OcrConfig {
                tessdata_bytes: Some(deu),
                ..OcrConfig::default()
            }),
            ..Default::default()
        };
        assert_ne!(
            hash_extraction_config(&a, "image/png"),
            hash_extraction_config(&b, "image/png"),
            "tessdata_bytes (serde-skipped) must be part of the cache key"
        );
    }

    #[test]
    fn ocr_strategy_changes_the_cache_key() {
        use crate::core::config::OcrStrategy;

        let auto = ExtractionConfig::default();
        let scanned = ExtractionConfig {
            ocr_strategy: OcrStrategy::ScannedPages { min_confidence: 0.7 },
            ..Default::default()
        };
        assert_ne!(
            hash_extraction_config(&auto, "application/pdf"),
            hash_extraction_config(&scanned, "application/pdf"),
            "ocr_strategy selects different pages for OCR and must be part of the cache key"
        );
    }

    #[test]
    fn scanned_pages_min_confidence_changes_the_cache_key() {
        use crate::core::config::OcrStrategy;

        let lenient = ExtractionConfig {
            ocr_strategy: OcrStrategy::ScannedPages { min_confidence: 0.6 },
            ..Default::default()
        };
        let strict = ExtractionConfig {
            ocr_strategy: OcrStrategy::ScannedPages { min_confidence: 0.9 },
            ..Default::default()
        };
        assert_ne!(
            hash_extraction_config(&lenient, "application/pdf"),
            hash_extraction_config(&strict, "application/pdf"),
            "min_confidence selects different pages for OCR and must be part of the cache key"
        );
    }
}

/// #217: extractor fallback chain behavior.
#[cfg(all(test, feature = "tokio-runtime", not(target_arch = "wasm32")))]
mod issue_217_fallback_tests {
    use super::*;
    use crate::core::config::{ExtractInput, ExtractionConfig};
    use crate::plugins::registry::DocumentExtractorRegistry;
    use crate::plugins::{DocumentExtractor, Plugin};
    use crate::types::ExtractedDocument;
    use std::borrow::Cow;
    use std::sync::Arc;
    use tempfile::tempdir;

    const FALLBACK_MIME: &str = "application/x-issue-217-fallback";

    /// A `DocumentExtractor` whose `extract` outcome is a plain function pointer,
    /// so each test can script a distinct sequence of successes/failures without
    /// a new type per scenario.
    struct ScriptedExtractor {
        name: &'static str,
        priority: i32,
        outcome: fn() -> Result<ExtractedDocument>,
    }

    impl Plugin for ScriptedExtractor {
        fn name(&self) -> &str {
            self.name
        }
        fn version(&self) -> String {
            "1.0.0".to_string()
        }
        fn initialize(&self) -> Result<()> {
            Ok(())
        }
        fn shutdown(&self) -> Result<()> {
            Ok(())
        }
    }

    #[async_trait::async_trait]
    impl DocumentExtractor for ScriptedExtractor {
        async fn extract(&self, _input: ExtractInput, _config: &ExtractionConfig) -> Result<ExtractedDocument> {
            (self.outcome)()
        }

        fn supported_mime_types(&self) -> &[&str] {
            &[FALLBACK_MIME]
        }

        fn priority(&self) -> i32 {
            self.priority
        }
    }

    fn ok_result() -> Result<ExtractedDocument> {
        Ok(ExtractedDocument {
            content: "fallback succeeded".to_string(),
            mime_type: Cow::Borrowed(FALLBACK_MIME),
            ..Default::default()
        })
    }

    fn unsupported_format_error() -> Result<ExtractedDocument> {
        Err(XbergError::UnsupportedFormat(
            "this extractor declines this specific variant".to_string(),
        ))
    }

    fn parsing_error() -> Result<ExtractedDocument> {
        Err(XbergError::Parsing {
            message: "corrupt or encrypted content".to_string(),
            source: None,
        })
    }

    fn write_temp_file() -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("doc.bin");
        std::fs::write(&file_path, b"irrelevant bytes").unwrap();
        (dir, file_path)
    }

    /// A [`XbergError::UnsupportedFormat`] failure from the highest-priority
    /// extractor must fall through to the next-priority candidate, and the
    /// success must be recorded in `processing_warnings` naming which extractor
    /// ran and why.
    #[tokio::test]
    async fn fallback_eligible_error_tries_next_extractor_and_warns() {
        let mut registry = DocumentExtractorRegistry::new();
        registry
            .register(Arc::new(ScriptedExtractor {
                name: "picky-217",
                priority: 100,
                outcome: unsupported_format_error,
            }))
            .unwrap();
        registry
            .register(Arc::new(ScriptedExtractor {
                name: "fallback-217",
                priority: 50,
                outcome: ok_result,
            }))
            .unwrap();

        let (_dir, file_path) = write_temp_file();
        let config = ExtractionConfig::default();
        let candidates = registry.get_candidates(&file_path, FALLBACK_MIME);
        let result = extract_with_candidates(&file_path, FALLBACK_MIME, &config, candidates)
            .await
            .expect("the lower-priority extractor must still succeed");

        assert_eq!(result.content, "fallback succeeded");
        assert_eq!(result.processing_warnings.len(), 1);
        assert_eq!(result.processing_warnings[0].source, "extractor-fallback");
        assert!(
            result.processing_warnings[0].message.contains("fallback-217"),
            "warning must name the extractor that actually ran: {}",
            result.processing_warnings[0].message
        );
    }

    /// A hard failure ([`XbergError::Parsing`] — the shape an encrypted file or a
    /// corrupt archive surfaces as) must NOT cascade to a lower-priority
    /// extractor: the document itself is the problem, so retrying would only add
    /// latency before producing a confusing error.
    #[tokio::test]
    async fn non_eligible_error_does_not_cascade_to_lower_priority_extractor() {
        let mut registry = DocumentExtractorRegistry::new();
        registry
            .register(Arc::new(ScriptedExtractor {
                name: "hard-failure-217",
                priority: 100,
                outcome: parsing_error,
            }))
            .unwrap();
        registry
            .register(Arc::new(ScriptedExtractor {
                name: "never-reached-217",
                priority: 50,
                outcome: ok_result,
            }))
            .unwrap();

        let (_dir, file_path) = write_temp_file();
        let config = ExtractionConfig::default();
        let candidates = registry.get_candidates(&file_path, FALLBACK_MIME);
        let result = extract_with_candidates(&file_path, FALLBACK_MIME, &config, candidates).await;

        match result {
            Err(XbergError::Parsing { message, .. }) => {
                assert_eq!(message, "corrupt or encrypted content");
            }
            other => panic!("a hard failure must propagate directly, not cascade: {other:?}"),
        }
    }
}
