//! Rust-only extraction engine.
//!
//! [`Engine`] owns the extraction internals that previously lived as free
//! functions in [`crate::core::extract`]. The crate-level [`crate::extract`]
//! and [`crate::extract_batch`] functions delegate to a process-global default
//! [`Engine`]. This is a pure refactor: behavior is identical to the previous
//! free-function implementation.
//!
//! This module is intentionally **not** part of the language-binding surface.
//! It is declared with a bare `pub mod engine;` in `lib.rs` and its files are
//! not listed in `alef.toml` `sources`, so the binding generator emits nothing
//! for it. The public types here are also listed in `alef.toml`
//! `[crates.exclude] types` as belt-and-suspenders.

use std::sync::Arc;

use crate::Result;
use crate::core::config::{ExtractInput, ExtractionConfig, ExtractionResult};

#[cfg(all(feature = "url-ingestion", feature = "tokio-runtime", not(target_arch = "wasm32")))]
mod crawl_handle;
mod extract_impl;
pub mod seams;

/// Generic, MIT-clean structured-extraction mechanism (rasterize, chunk, schema,
/// citations, prompts). Gated on `heuristics` because the mechanism speaks in
/// heuristics call-mode / confidence types; individual submodules carry further
/// gates for their own hard dependencies (`pdf`, `presets`). Not part of the
/// binding surface — see `alef.toml` `[crates.exclude]`.
#[cfg(feature = "heuristics")]
pub mod structured;

/// Single-operation parse memo (MIME, page count, shared bytes, lazily-rendered
/// pages). Depends on the structured rasterizer, so it carries that module's
/// `pdf` gate in addition to `heuristics`. Not part of the binding surface —
/// see `alef.toml` `[crates.exclude]`.
#[cfg(all(feature = "heuristics", feature = "pdf"))]
pub mod parsed;

use seams::{CacheBackend, NoopCache, NoopProgressSink, ProgressSink};
#[cfg(feature = "presets")]
use seams::{CorePresetResolver, PresetResolver};
#[cfg(feature = "layout-detection")]
use seams::{DefaultModelProvider, ModelProvider};
#[cfg(feature = "heuristics")]
use seams::{DefaultStructuredPolicy, StructuredPolicy};
#[cfg(feature = "liter-llm")]
use seams::{LiterLlmClient, LlmClient};

/// Internal engine state.
///
/// Holds the process-shared, fingerprinted crawl-engine memo so that multi-URL
/// batch extraction can reuse a single [`crawlberg::CrawlEngine`] (and its
/// shared middleware/cache/rate-limiter) across all URLs in a batch, plus the
/// six injected extension seams (each filled with its in-core default by
/// [`EngineBuilder::build`]). The single-URL `extract` path does not touch this
/// state.
struct EngineInner {
    /// Single-slot, fingerprinted memo of the last-built crawl engine. The slot
    /// is reused when the incoming [`crawlberg::CrawlConfig`] fingerprint
    /// matches, otherwise a fresh engine is built and stored.
    #[cfg(all(feature = "url-ingestion", feature = "tokio-runtime", not(target_arch = "wasm32")))]
    crawl: parking_lot::Mutex<Option<crawl_handle::CrawlHandleMemo>>,

    /// Content-addressed byte cache. Default: [`NoopCache`].
    cache: Arc<dyn CacheBackend>,
    /// Progress event sink. Default: [`NoopProgressSink`].
    progress: Arc<dyn ProgressSink>,

    /// Structured-extraction call-mode policy. Default: [`DefaultStructuredPolicy`].
    #[cfg(feature = "heuristics")]
    structured_policy: Arc<dyn StructuredPolicy>,
    /// Built-in preset resolver. Default: [`CorePresetResolver`].
    #[cfg(feature = "presets")]
    preset_resolver: Arc<dyn PresetResolver>,
    /// JSON-schema LLM client. Default: [`LiterLlmClient`].
    #[cfg(feature = "liter-llm")]
    llm_client: Arc<dyn LlmClient>,
    /// On-demand model-weight provider. Default: [`DefaultModelProvider`].
    #[cfg(feature = "layout-detection")]
    model_provider: Arc<dyn ModelProvider>,
}

/// A reusable, cheaply-cloneable extraction engine.
///
/// Cloning an [`Engine`] shares the same underlying state via [`Arc`].
#[derive(Clone)]
pub struct Engine {
    inner: Arc<EngineInner>,
}

impl Engine {
    /// Start building an [`Engine`].
    pub fn builder() -> EngineBuilder {
        EngineBuilder::default()
    }

    /// Construct an [`Engine`] with default configuration.
    pub fn new_default() -> Self {
        EngineBuilder::default().build()
    }

    /// Extract content from a single bytes or URI input.
    ///
    /// Honours the injected [`CacheBackend`] and [`ProgressSink`] seams: a bytes
    /// input whose content-hash cache key already has an entry short-circuits
    /// straight to the cached [`ExtractionResult`], and every call emits coarse
    /// `ProgressEvent`s (start, then either completion or error, plus a
    /// cache-hit event when one occurs). Both seams default to no-ops
    /// ([`NoopCache`], [`NoopProgressSink`]), so callers who inject nothing see
    /// byte-identical behavior to before this wiring existed.
    pub async fn extract(&self, input: ExtractInput, config: &ExtractionConfig) -> Result<ExtractionResult> {
        extract_impl::extract(&self.inner, input, config).await
    }

    /// Extract content from multiple bytes or URI inputs.
    pub async fn extract_batch(
        &self,
        inputs: Vec<ExtractInput>,
        config: &ExtractionConfig,
    ) -> Result<ExtractionResult> {
        extract_impl::extract_batch(&self.inner, inputs, config).await
    }

    /// The injected [`CacheBackend`] seam (default: [`NoopCache`]).
    pub fn cache_backend(&self) -> &Arc<dyn CacheBackend> {
        &self.inner.cache
    }

    /// The injected [`ProgressSink`] seam (default: [`NoopProgressSink`]).
    pub fn progress_sink(&self) -> &Arc<dyn ProgressSink> {
        &self.inner.progress
    }

    /// The injected [`StructuredPolicy`] seam (default: [`DefaultStructuredPolicy`]).
    #[cfg(feature = "heuristics")]
    pub fn structured_policy(&self) -> &Arc<dyn StructuredPolicy> {
        &self.inner.structured_policy
    }

    /// The injected [`PresetResolver`] seam (default: [`CorePresetResolver`]).
    #[cfg(feature = "presets")]
    pub fn preset_resolver(&self) -> &Arc<dyn PresetResolver> {
        &self.inner.preset_resolver
    }

    /// The injected [`LlmClient`] seam (default: [`LiterLlmClient`]).
    #[cfg(feature = "liter-llm")]
    pub fn llm_client(&self) -> &Arc<dyn LlmClient> {
        &self.inner.llm_client
    }

    /// The injected [`ModelProvider`] seam (default: [`DefaultModelProvider`]).
    #[cfg(feature = "layout-detection")]
    pub fn model_provider(&self) -> &Arc<dyn ModelProvider> {
        &self.inner.model_provider
    }
}

/// Builder for [`Engine`].
///
/// Each extension seam left unset is filled with its in-core default by
/// [`build`](EngineBuilder::build), so [`Engine::new_default`] produces an
/// engine whose seams are exactly those defaults.
#[derive(Default)]
pub struct EngineBuilder {
    cache: Option<Arc<dyn CacheBackend>>,
    progress: Option<Arc<dyn ProgressSink>>,
    #[cfg(feature = "heuristics")]
    structured_policy: Option<Arc<dyn StructuredPolicy>>,
    #[cfg(feature = "presets")]
    preset_resolver: Option<Arc<dyn PresetResolver>>,
    #[cfg(feature = "liter-llm")]
    llm_client: Option<Arc<dyn LlmClient>>,
    #[cfg(feature = "layout-detection")]
    model_provider: Option<Arc<dyn ModelProvider>>,
}

impl EngineBuilder {
    /// Inject a [`CacheBackend`], overriding the [`NoopCache`] default.
    pub fn with_cache_backend(mut self, cache: Arc<dyn CacheBackend>) -> Self {
        self.cache = Some(cache);
        self
    }

    /// Inject a [`ProgressSink`], overriding the [`NoopProgressSink`] default.
    pub fn with_progress_sink(mut self, progress: Arc<dyn ProgressSink>) -> Self {
        self.progress = Some(progress);
        self
    }

    /// Inject a [`StructuredPolicy`], overriding the [`DefaultStructuredPolicy`] default.
    #[cfg(feature = "heuristics")]
    pub fn with_structured_policy(mut self, policy: Arc<dyn StructuredPolicy>) -> Self {
        self.structured_policy = Some(policy);
        self
    }

    /// Inject a [`PresetResolver`], overriding the [`CorePresetResolver`] default.
    #[cfg(feature = "presets")]
    pub fn with_preset_resolver(mut self, resolver: Arc<dyn PresetResolver>) -> Self {
        self.preset_resolver = Some(resolver);
        self
    }

    /// Inject an [`LlmClient`], overriding the [`LiterLlmClient`] default.
    #[cfg(feature = "liter-llm")]
    pub fn with_llm_client(mut self, client: Arc<dyn LlmClient>) -> Self {
        self.llm_client = Some(client);
        self
    }

    /// Inject a [`ModelProvider`], overriding the [`DefaultModelProvider`] default.
    #[cfg(feature = "layout-detection")]
    pub fn with_model_provider(mut self, provider: Arc<dyn ModelProvider>) -> Self {
        self.model_provider = Some(provider);
        self
    }

    /// Finalize the builder into an [`Engine`], filling every unset seam with
    /// its in-core default.
    pub fn build(self) -> Engine {
        let inner = EngineInner {
            #[cfg(all(feature = "url-ingestion", feature = "tokio-runtime", not(target_arch = "wasm32")))]
            crawl: parking_lot::Mutex::new(None),
            cache: self.cache.unwrap_or_else(|| Arc::new(NoopCache)),
            progress: self.progress.unwrap_or_else(|| Arc::new(NoopProgressSink)),
            #[cfg(feature = "heuristics")]
            structured_policy: self
                .structured_policy
                .unwrap_or_else(|| Arc::new(DefaultStructuredPolicy::default())),
            #[cfg(feature = "presets")]
            preset_resolver: self.preset_resolver.unwrap_or_else(|| Arc::new(CorePresetResolver)),
            #[cfg(feature = "liter-llm")]
            llm_client: self.llm_client.unwrap_or_else(|| Arc::new(LiterLlmClient)),
            #[cfg(feature = "layout-detection")]
            model_provider: self
                .model_provider
                .unwrap_or_else(|| Arc::new(DefaultModelProvider::default())),
        };
        Engine { inner: Arc::new(inner) }
    }
}

#[cfg(all(test, feature = "tokio-runtime"))]
mod tests {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use super::*;
    use crate::types::ExtractedDocument;
    use seams::ProgressEvent;

    /// A [`ProgressSink`] that records the stage of every emitted event, in order.
    #[derive(Default)]
    struct RecordingProgressSink {
        stages: Mutex<Vec<String>>,
    }

    impl ProgressSink for RecordingProgressSink {
        fn emit(&self, event: ProgressEvent) {
            self.stages
                .lock()
                .expect("recording sink mutex poisoned")
                .push(event.stage);
        }
    }

    // Revert line: change `Engine::extract` back to
    // `extract_impl::extract(input, config).await` (dropping `&self.inner`) to make
    // this test fail — `RecordingProgressSink::emit` is then never called and
    // `stages` stays empty.
    #[tokio::test]
    async fn should_emit_start_then_complete_progress_events_for_a_successful_bytes_extraction() {
        let sink = Arc::new(RecordingProgressSink::default());
        let engine = Engine::builder().with_progress_sink(sink.clone()).build();

        let output = engine
            .extract(
                ExtractInput::from_bytes(b"hello progress".to_vec(), "text/plain", None),
                &ExtractionConfig::default(),
            )
            .await
            .unwrap();

        assert_eq!(output.results.len(), 1);
        assert_eq!(
            *sink.stages.lock().expect("recording sink mutex poisoned"),
            vec!["extract_start".to_string(), "extract_complete".to_string()],
            "expected exactly a start event followed by a complete event, in that order"
        );
    }

    #[tokio::test]
    async fn should_emit_start_then_error_progress_events_for_a_failed_extraction() {
        let sink = Arc::new(RecordingProgressSink::default());
        let engine = Engine::builder().with_progress_sink(sink.clone()).build();

        let error = engine
            .extract(
                ExtractInput::from_uri("s3://bucket/file.txt"),
                &ExtractionConfig::default(),
            )
            .await
            .unwrap_err();

        assert!(error.to_string().contains("unsupported URI scheme"));
        assert_eq!(
            *sink.stages.lock().expect("recording sink mutex poisoned"),
            vec!["extract_start".to_string(), "extract_error".to_string()],
            "expected exactly a start event followed by an error event, in that order"
        );
    }

    /// A [`CacheBackend`] that always serves one fixed payload and counts lookups,
    /// so a test can prove a hit was actually consulted (not just that the result
    /// happens to match).
    struct StubCacheBackend {
        cached_payload: Vec<u8>,
        gets: AtomicUsize,
        puts: AtomicUsize,
    }

    #[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
    #[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
    impl CacheBackend for StubCacheBackend {
        async fn get(&self, _key: &str) -> Option<Vec<u8>> {
            self.gets.fetch_add(1, Ordering::SeqCst);
            Some(self.cached_payload.clone())
        }

        async fn put(&self, _key: &str, _value: Vec<u8>, _ttl: Option<Duration>) {
            self.puts.fetch_add(1, Ordering::SeqCst);
        }
    }

    // Revert line: change `content_cache_key` in `extract_impl.rs` to always
    // `return None;` to make this test fail — with no cache key, `extract` never
    // calls `inner.cache.get`, `gets` stays 0, and the real (uncached) extraction
    // result ("this is not the cached content") is returned instead.
    #[tokio::test]
    async fn should_return_cached_result_and_skip_extraction_on_cache_hit() {
        let cached_result = ExtractionResult::single(ExtractedDocument {
            content: "CACHED-RESULT-NOT-REEXTRACTED".to_string(),
            ..Default::default()
        });
        let cache = Arc::new(StubCacheBackend {
            cached_payload: serde_json::to_vec(&cached_result).unwrap(),
            gets: AtomicUsize::new(0),
            puts: AtomicUsize::new(0),
        });
        let engine = Engine::builder().with_cache_backend(cache.clone()).build();

        let output = engine
            .extract(
                ExtractInput::from_bytes(b"this is not the cached content".to_vec(), "text/plain", None),
                &ExtractionConfig::default(),
            )
            .await
            .unwrap();

        assert_eq!(output.results.len(), 1);
        assert_eq!(output.results[0].content, "CACHED-RESULT-NOT-REEXTRACTED");
        assert_eq!(
            cache.gets.load(Ordering::SeqCst),
            1,
            "the cache backend must be consulted exactly once"
        );
        assert_eq!(
            cache.puts.load(Ordering::SeqCst),
            0,
            "a hit must not also write back to the cache"
        );
    }

    /// A [`CacheBackend`] backed by a real `HashMap`, so hit/miss behavior reflects
    /// actual key derivation (content hash + config) rather than a scripted response.
    #[derive(Default)]
    struct InMemoryCacheBackend {
        store: Mutex<std::collections::HashMap<String, Vec<u8>>>,
        gets: AtomicUsize,
        puts: AtomicUsize,
    }

    #[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
    #[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
    impl CacheBackend for InMemoryCacheBackend {
        async fn get(&self, key: &str) -> Option<Vec<u8>> {
            self.gets.fetch_add(1, Ordering::SeqCst);
            self.store
                .lock()
                .expect("in-memory cache mutex poisoned")
                .get(key)
                .cloned()
        }

        async fn put(&self, key: &str, value: Vec<u8>, _ttl: Option<Duration>) {
            self.puts.fetch_add(1, Ordering::SeqCst);
            self.store
                .lock()
                .expect("in-memory cache mutex poisoned")
                .insert(key.to_string(), value);
        }
    }

    // Revert line: remove the `inner.cache.put(...)` call in the `Ok(output)` arm of
    // `extract_impl::extract` to make this test fail -- `puts` stays 0 after the first
    // (miss) extraction, and the second identical extract also misses (still 0 entries
    // in the store), so `puts` never reaches 1 either.
    #[tokio::test]
    async fn should_populate_cache_on_miss_and_skip_reextraction_on_identical_second_call() {
        let cache = Arc::new(InMemoryCacheBackend::default());
        let engine = Engine::builder().with_cache_backend(cache.clone()).build();
        let config = ExtractionConfig::default();
        let bytes = b"identical bytes for cache".to_vec();

        let first = engine
            .extract(ExtractInput::from_bytes(bytes.clone(), "text/plain", None), &config)
            .await
            .unwrap();

        assert_eq!(
            cache.gets.load(Ordering::SeqCst),
            1,
            "the first extract must consult the cache exactly once (a miss)"
        );
        assert_eq!(
            cache.puts.load(Ordering::SeqCst),
            1,
            "a successful cache-miss extraction must populate the cache exactly once"
        );

        let second = engine
            .extract(ExtractInput::from_bytes(bytes, "text/plain", None), &config)
            .await
            .unwrap();

        assert_eq!(
            cache.gets.load(Ordering::SeqCst),
            2,
            "the second identical extract must also consult the cache"
        );
        assert_eq!(
            cache.puts.load(Ordering::SeqCst),
            1,
            "a cache hit must short-circuit extraction and must not write back to the cache again"
        );
        assert_eq!(
            first.results[0].content, second.results[0].content,
            "the cache-hit result must equal the originally-extracted content"
        );
    }

    // Revert line: change `content_cache_key` in `extract_impl.rs` to drop
    // `config_json` from the hash to make this test fail -- both configs would then
    // derive the same key, the second extract would hit the first extract's cache
    // entry, and `puts` would stay at 1 instead of reaching 2.
    #[tokio::test]
    async fn should_miss_cache_when_extraction_config_changes_for_identical_bytes() {
        let cache = Arc::new(InMemoryCacheBackend::default());
        let engine = Engine::builder().with_cache_backend(cache.clone()).build();
        let bytes = b"same bytes different config".to_vec();

        let config_a = ExtractionConfig::default();
        engine
            .extract(ExtractInput::from_bytes(bytes.clone(), "text/plain", None), &config_a)
            .await
            .unwrap();

        let config_b = ExtractionConfig {
            enable_quality_processing: !config_a.enable_quality_processing,
            ..ExtractionConfig::default()
        };
        engine
            .extract(ExtractInput::from_bytes(bytes, "text/plain", None), &config_b)
            .await
            .unwrap();

        assert_eq!(
            cache.gets.load(Ordering::SeqCst),
            2,
            "both extracts must consult the cache once each"
        );
        assert_eq!(
            cache.puts.load(Ordering::SeqCst),
            2,
            "a config change must derive a different cache key, forcing a second miss and a second write"
        );
    }

    // Revert line: move the `PROGRESS_STAGE_CACHE_HIT` emit in `extract_impl.rs` so it
    // no longer runs before the early `return Ok(cached_result)` to make this test fail
    // -- `stages` would then read only `["extract_start"]` instead of including the
    // cache-hit stage.
    #[tokio::test]
    async fn should_emit_start_then_cache_hit_progress_events_on_a_cache_hit() {
        let sink = Arc::new(RecordingProgressSink::default());
        let cached_result = ExtractionResult::single(ExtractedDocument {
            content: "CACHED".to_string(),
            ..Default::default()
        });
        let cache = Arc::new(StubCacheBackend {
            cached_payload: serde_json::to_vec(&cached_result).unwrap(),
            gets: AtomicUsize::new(0),
            puts: AtomicUsize::new(0),
        });
        let engine = Engine::builder()
            .with_progress_sink(sink.clone())
            .with_cache_backend(cache)
            .build();

        engine
            .extract(
                ExtractInput::from_bytes(b"progress on cache hit".to_vec(), "text/plain", None),
                &ExtractionConfig::default(),
            )
            .await
            .unwrap();

        assert_eq!(
            *sink.stages.lock().expect("recording sink mutex poisoned"),
            vec!["extract_start".to_string(), "extract_cache_hit".to_string()],
            "a cache hit must emit start then cache_hit, not complete"
        );
    }
}
