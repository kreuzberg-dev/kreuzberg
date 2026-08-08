//! Plugin registration and discovery.
//!
//! This module provides registries for managing plugins of different types.
//! Each plugin type (OcrBackend, DocumentExtractor, etc.) has its own registry
//! with type-safe registration and lookup.

mod embedding;
mod extractor;
mod ocr;
mod processor;
mod renderer;
mod reranker;
mod tokenizer;
mod validator;

pub use embedding::EmbeddingBackendRegistry;
pub use extractor::DocumentExtractorRegistry;
pub(crate) use extractor::RegisteredDocumentExtractor;
pub use ocr::OcrBackendRegistry;
pub use processor::PostProcessorRegistry;
pub use renderer::RendererRegistry;
pub use reranker::RerankerBackendRegistry;
pub use tokenizer::TokenizerBackendRegistry;
pub use validator::ValidatorRegistry;

use crate::{Result, XbergError};
use parking_lot::RwLock;
use std::sync::Arc;
use std::sync::LazyLock;

/// Validate a plugin name before registration.
///
/// # Rules
///
/// - Name cannot be empty
/// - Name cannot contain whitespace
/// - Name should follow kebab-case convention (lowercase with hyphens)
///
/// # Errors
///
/// Returns `ValidationError` if the name is invalid.
pub(super) fn validate_plugin_name(name: &str) -> Result<()> {
    if name.is_empty() {
        return Err(XbergError::Validation {
            message: "Plugin name cannot be empty".to_string(),
            source: None,
        });
    }

    if name.contains(char::is_whitespace) {
        return Err(XbergError::Validation {
            message: format!("Plugin name '{}' cannot contain whitespace", name),
            source: None,
        });
    }

    Ok(())
}

/// Global OCR backend registry singleton.
pub static OCR_BACKEND_REGISTRY: LazyLock<Arc<RwLock<OcrBackendRegistry>>> =
    LazyLock::new(|| Arc::new(RwLock::new(OcrBackendRegistry::new())));

/// Global embedding backend registry singleton.
pub static EMBEDDING_BACKEND_REGISTRY: LazyLock<Arc<RwLock<EmbeddingBackendRegistry>>> =
    LazyLock::new(|| Arc::new(RwLock::new(EmbeddingBackendRegistry::new())));

/// Global reranker backend registry singleton.
///
/// Since v5.0.0.
pub static RERANKER_BACKEND_REGISTRY: LazyLock<Arc<RwLock<RerankerBackendRegistry>>> =
    LazyLock::new(|| Arc::new(RwLock::new(RerankerBackendRegistry::new())));

/// Global tokenizer backend registry singleton.
pub static TOKENIZER_BACKEND_REGISTRY: LazyLock<Arc<RwLock<TokenizerBackendRegistry>>> =
    LazyLock::new(|| Arc::new(RwLock::new(TokenizerBackendRegistry::new())));

/// Global document extractor registry singleton.
pub static DOCUMENT_EXTRACTOR_REGISTRY: LazyLock<Arc<RwLock<DocumentExtractorRegistry>>> =
    LazyLock::new(|| Arc::new(RwLock::new(DocumentExtractorRegistry::new())));

/// Global post-processor registry singleton.
pub static POST_PROCESSOR_REGISTRY: LazyLock<Arc<RwLock<PostProcessorRegistry>>> =
    LazyLock::new(|| Arc::new(RwLock::new(PostProcessorRegistry::new())));

/// Global validator registry singleton.
pub static VALIDATOR_REGISTRY: LazyLock<Arc<RwLock<ValidatorRegistry>>> =
    LazyLock::new(|| Arc::new(RwLock::new(ValidatorRegistry::new())));

/// Global renderer registry singleton.
pub static RENDERER_REGISTRY: LazyLock<Arc<RwLock<RendererRegistry>>> =
    LazyLock::new(|| Arc::new(RwLock::new(RendererRegistry::new())));

/// Get the global OCR backend registry.
#[cfg_attr(alef, alef(skip))]
pub fn get_ocr_backend_registry() -> Arc<RwLock<OcrBackendRegistry>> {
    OCR_BACKEND_REGISTRY.clone()
}

/// Get the global embedding backend registry.
#[cfg_attr(alef, alef(skip))]
pub fn get_embedding_backend_registry() -> Arc<RwLock<EmbeddingBackendRegistry>> {
    EMBEDDING_BACKEND_REGISTRY.clone()
}

/// Get the global reranker backend registry.
///
/// Since v5.0.0.
#[cfg_attr(alef, alef(skip))]
pub fn get_reranker_backend_registry() -> Arc<RwLock<RerankerBackendRegistry>> {
    RERANKER_BACKEND_REGISTRY.clone()
}

/// Get the global tokenizer backend registry.
#[cfg_attr(alef, alef(skip))]
pub fn get_tokenizer_backend_registry() -> Arc<RwLock<TokenizerBackendRegistry>> {
    TOKENIZER_BACKEND_REGISTRY.clone()
}

/// Get the global document extractor registry.
#[cfg_attr(alef, alef(skip))]
pub fn get_document_extractor_registry() -> Arc<RwLock<DocumentExtractorRegistry>> {
    DOCUMENT_EXTRACTOR_REGISTRY.clone()
}

/// Get the global post-processor registry.
#[cfg_attr(alef, alef(skip))]
pub fn get_post_processor_registry() -> Arc<RwLock<PostProcessorRegistry>> {
    POST_PROCESSOR_REGISTRY.clone()
}

/// Get the global validator registry.
#[cfg_attr(alef, alef(skip))]
pub fn get_validator_registry() -> Arc<RwLock<ValidatorRegistry>> {
    VALIDATOR_REGISTRY.clone()
}

/// Get the global renderer registry.
#[cfg_attr(alef, alef(skip))]
pub fn get_renderer_registry() -> Arc<RwLock<RendererRegistry>> {
    RENDERER_REGISTRY.clone()
}

/// Setup/teardown support for tests that exercise a process-global plugin registry.
///
/// Each global registry is shared mutable state for the whole test binary, and every plugin type
/// exposes a `clear_*` function that wipes all of it. Tests for one registry live in several
/// modules, so unique per-test plugin names are not enough on their own: a `clear_*` call can
/// land between another test's registration and the assertion that reads the registry back, and a
/// test that panics mid-way leaves its plugin registered for whatever runs next. Both failure
/// modes are order- and timing-dependent, so they surface as flakes rather than reproducible
/// breaks.
#[cfg(test)]
pub(crate) mod test_support {
    /// Defines a guard type that serializes access to one global registry and leaves it empty on
    /// both entry and exit.
    ///
    /// Each registry gets its own lock, so this only serializes tests that explicitly acquire
    /// the *same* guard type against each other; tests for a different plugin type acquire a
    /// different lock and are unaffected. Critically, this lock is *not* held by, and does not
    /// serialize against, code paths that read the same global registry without acquiring this
    /// guard at all — for example a self-healing consumer that repopulates a registry only when
    /// it observes the registry as completely empty (see `extractors::ensure_initialized`), or
    /// one that checks for its own specific entry by name instead of emptiness (see
    /// `keywords::ensure_initialized`, #317: a `OnceCell`-guarded registrar that never re-runs
    /// once initialized would otherwise leave its plugin permanently missing after any later
    /// `PostProcessorRegistryGuard` cycle). A by-name check is more robust than an
    /// emptiness check when the registry is shared by unrelated registrants (post-processors
    /// include built-ins, the keyword extractor, etc.): a guard holding only foreign entries
    /// still reads as "my entry is missing" and self-heals correctly. An emptiness check does
    /// not have this property — while a guard is held with only mock/foreign entries
    /// registered, the registry is non-empty, so such a self-heal is skipped; any concurrently
    /// running, non-guarded consumer that expects the real registrations to be present can then
    /// fail. Either way, guard holders must therefore either register everything a concurrent
    /// unguarded consumer could need, or (better) avoid mutating the global registry at all and
    /// use a local `DocumentExtractorRegistry::new()` (or equivalent) instead.
    macro_rules! registry_guard {
        ($guard:ident, $lock:ident, $clear:path, $what:literal) => {
            /// Holds this registry's lock for the lifetime of a test and leaves the registry
            /// empty both on entry and on exit, so every test sees a known-empty registry no
            /// matter what ran before it — and a failing assertion cannot leak a registration.
            pub(crate) struct $guard(#[allow(dead_code)] std::sync::MutexGuard<'static, ()>);

            impl $guard {
                pub(crate) fn acquire() -> Self {
                    static $lock: std::sync::Mutex<()> = std::sync::Mutex::new(());
                    // A test that panics while holding the lock poisons it. The guarded data is
                    // `()`, so there is no inconsistent state to protect against and recovering
                    // is correct.
                    let lock = $lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                    $clear().expect(concat!($what, " registry setup must succeed"));
                    Self(lock)
                }
            }

            impl Drop for $guard {
                fn drop(&mut self) {
                    // Runs before the inner `MutexGuard` field is dropped, so teardown still
                    // holds the lock and cannot race the next test's setup.
                    let cleared = $clear();
                    // Panicking inside `drop` while the thread is already unwinding aborts the
                    // process and hides the assertion failure that caused it, so only surface a
                    // teardown error when the test itself passed.
                    if !std::thread::panicking() {
                        cleared.expect(concat!($what, " registry teardown must succeed"));
                    }
                }
            }
        };
    }

    registry_guard!(
        RerankerRegistryGuard,
        RERANKER_REGISTRY_LOCK,
        crate::plugins::clear_reranker_backends,
        "reranker"
    );
    registry_guard!(
        EmbeddingRegistryGuard,
        EMBEDDING_REGISTRY_LOCK,
        crate::plugins::clear_embedding_backends,
        "embedding"
    );
    registry_guard!(
        TokenizerRegistryGuard,
        TOKENIZER_REGISTRY_LOCK,
        crate::plugins::clear_tokenizer_backends,
        "tokenizer"
    );
    // The renderer registry is the one global registry seeded with built-ins, so its guard
    // restores those defaults instead of leaving it empty — an empty renderer registry would
    // break every later test that renders through the global registry.
    registry_guard!(
        RendererRegistryGuard,
        RENDERER_REGISTRY_LOCK,
        reset_renderers_to_defaults,
        "renderer"
    );

    fn reset_renderers_to_defaults() -> crate::Result<()> {
        let registry = super::get_renderer_registry();
        let mut registry = registry.write();
        registry.reset_to_defaults()
    }
    // `DocumentExtractorRegistryGuard` (which serialized tests via
    // `crate::plugins::clear_document_extractors`) was removed: it had no remaining callers
    // after the document-extractor tests were rewritten to use local
    // `DocumentExtractorRegistry` instances instead of mutating the global registry (see
    // `core::extractor::file::issue_217_fallback_tests` and `plugins::extractor::tests`).
    registry_guard!(
        PostProcessorRegistryGuard,
        POST_PROCESSOR_REGISTRY_LOCK,
        crate::plugins::clear_post_processors,
        "post-processor"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_global_registry_access() {
        let ocr_registry = get_ocr_backend_registry();
        let _ = ocr_registry.read().list();

        let extractor_registry = get_document_extractor_registry();
        let _ = extractor_registry.read().list();

        let processor_registry = get_post_processor_registry();
        let _ = processor_registry.read().list();

        let validator_registry = get_validator_registry();
        let _ = validator_registry.read().list();

        let renderer_registry = get_renderer_registry();
        let _ = renderer_registry.read().list();
    }
}
