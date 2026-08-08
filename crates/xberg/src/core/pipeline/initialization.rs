//! Pipeline initialization and setup logic.
//!
//! This module handles the initialization of features and processor cache
//! required for pipeline execution.

use crate::Result;
use std::sync::{OnceLock, RwLock};

use super::cache::{PROCESSOR_CACHE, ProcessorCache};

/// Records the outcome of the one-time built-in post-processor registration
/// (#271), so callers can learn *that* registration was incomplete instead of
/// the failure being dropped on the floor by `OnceLock::get_or_init`, whose
/// closure cannot return a value here without changing every call site.
static BUILTIN_REGISTRATION_ERROR: RwLock<Option<String>> = RwLock::new(None);

/// The error message from the built-in post-processor registration pass, if any
/// processor failed to register. `None` means either registration has not run
/// yet or every enabled processor registered successfully.
///
/// Called from `pipeline::mod::run_pipeline` (#271), which pushes a
/// `ProcessingWarning` naming this message whenever it is `Some(_)` — the
/// aggregate counterpart to the captioning-only "processor missing" warning at
/// the call site of `run_captioning_prepass`.
pub(crate) fn builtin_registration_error() -> Option<String> {
    BUILTIN_REGISTRATION_ERROR
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
}

/// Test-only access to `BUILTIN_REGISTRATION_ERROR` for exercising the
/// `builtin_registration_error()` call site inside `pipeline::mod` without
/// re-triggering (or racing) the real one-time `BUILTIN_INIT` `OnceLock` pass.
#[cfg(test)]
pub(crate) mod test_support {
    use super::BUILTIN_REGISTRATION_ERROR;

    /// Set (or clear, with `None`) the recorded registration error. The static is
    /// process-global, so callers must restore it (typically to `None`) when done.
    pub(crate) fn set_registration_error(value: Option<String>) {
        let mut slot = BUILTIN_REGISTRATION_ERROR
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *slot = value;
    }
}

/// Type alias for processor stages tuple (Early, Middle, Late).
type ProcessorStages = (
    std::sync::Arc<Vec<std::sync::Arc<dyn crate::plugins::PostProcessor>>>,
    std::sync::Arc<Vec<std::sync::Arc<dyn crate::plugins::PostProcessor>>>,
    std::sync::Arc<Vec<std::sync::Arc<dyn crate::plugins::PostProcessor>>>,
);

/// Initialize feature-specific systems that may be needed during pipeline execution.
pub(super) fn initialize_features() {
    #[cfg(any(feature = "keywords-yake", feature = "keywords-rake"))]
    {
        let _ = crate::keywords::ensure_initialized();
    }

    #[cfg(feature = "quality")]
    {
        static QUALITY_INIT: OnceLock<()> = OnceLock::new();
        QUALITY_INIT.get_or_init(|| {
            let registry = crate::plugins::registry::get_post_processor_registry();
            let mut reg = registry.write();
            if let Err(e) = reg.register(std::sync::Arc::new(crate::text::QualityProcessor)) {
                tracing::error!("Failed to register built-in quality post-processor: {e}");
            }
        });
    }

    static BUILTIN_INIT: OnceLock<()> = OnceLock::new();
    BUILTIN_INIT.get_or_init(|| {
        if let Err(e) = crate::plugins::processor::builtin::register_builtin() {
            tracing::error!("Built-in post-processor registration was incomplete: {e}");
            let mut slot = BUILTIN_REGISTRATION_ERROR
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            *slot = Some(e.to_string());
        }
    });
}

/// Initialize the processor cache if not already initialized, or rebuild it if
/// the post-processor registry has changed since it was last built.
///
/// #215: the cache used to populate once on the first pipeline run and never
/// again, so a post-processor registered (or removed) after that first run was
/// silently invisible unless a caller happened to know about and call
/// `clear_processor_cache()`. Comparing the registry's live generation against
/// the generation recorded in the cached snapshot makes this self-correcting.
pub(super) fn initialize_processor_cache() -> Result<()> {
    let current_generation = crate::plugins::registry::get_post_processor_registry()
        .read()
        .generation();

    let mut cache_lock = PROCESSOR_CACHE.write();
    let is_stale = cache_lock
        .as_ref()
        .is_some_and(|cache| cache.generation != current_generation);

    if cache_lock.is_none() || is_stale {
        *cache_lock = Some(ProcessorCache::new()?);
    }
    Ok(())
}

/// Get processors from the cache, organized by stage.
pub(super) fn get_processors_from_cache() -> Result<ProcessorStages> {
    let cache_lock = PROCESSOR_CACHE.read();
    let cache = cache_lock
        .as_ref()
        .ok_or_else(|| crate::XbergError::Other("Processor cache not initialized".to_string()))?;
    Ok((
        std::sync::Arc::clone(&cache.early),
        std::sync::Arc::clone(&cache.middle),
        std::sync::Arc::clone(&cache.late),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #271: `builtin_registration_error` must round-trip whatever the
    /// registration pass recorded, so pipeline code has somewhere to look
    /// instead of the failure being dropped by `let _ = ...`.
    #[test]
    fn builtin_registration_error_reports_a_recorded_failure() {
        {
            let mut slot = BUILTIN_REGISTRATION_ERROR
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            *slot = Some("1 of 3 built-in post-processor(s) failed to register: ner: boom".to_string());
        }
        assert_eq!(
            builtin_registration_error(),
            Some("1 of 3 built-in post-processor(s) failed to register: ner: boom".to_string())
        );

        // Reset: this static is process-global and shared with other tests in this binary.
        let mut slot = BUILTIN_REGISTRATION_ERROR
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *slot = None;
    }

    /// #215: a post-processor registered after the cache was already populated
    /// must become visible on the *next* extraction, not stay invisible until
    /// something remembers to call `clear_processor_cache()`.
    ///
    /// `#[serial]`: this test clears the global post-processor registry (via
    /// `PostProcessorRegistryGuard`) and mutates the process-global
    /// `PROCESSOR_CACHE` directly — there is no local/injectable variant of either,
    /// so any other test that runs the real pipeline concurrently and expects the
    /// built-in post-processors (or a previously cached, non-empty processor set)
    /// to be present would race this one. Serializing against the crate's other
    /// `#[serial]` tests (see `core::extractor::mod::test_concurrent_extractions_different_mimes`)
    /// is the same pattern used there for the same reason.
    ///
    /// The assertions are deliberately about **containment of this test's own
    /// processor**, never about the registry being empty or about an exact count.
    /// `#[serial]` only excludes the crate's other `#[serial]` tests, while *any*
    /// non-serial test that runs a real extraction calls `initialize_features()` and
    /// registers the built-in post-processors into the same global registry. An
    /// "assert the cache starts empty" check therefore failed roughly one run in four
    /// — a property of the test harness, not of the code under test. Emptiness was
    /// only ever scaffolding; the #215 invariant is that a *late* registration becomes
    /// visible, which containment states exactly and races nothing.
    #[serial_test::serial]
    #[test]
    fn processor_cache_rebuilds_when_registry_changes_after_first_use() {
        use crate::plugins::registry::test_support::PostProcessorRegistryGuard;
        use crate::plugins::{Plugin, PostProcessor, ProcessingStage};
        use crate::types::ExtractedDocument;
        use async_trait::async_trait;
        use std::sync::Arc;

        let _guard = PostProcessorRegistryGuard::acquire();

        #[derive(Debug)]
        struct LateAddedProcessor;

        impl Plugin for LateAddedProcessor {
            fn name(&self) -> &str {
                "late-added-215"
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

        #[async_trait]
        impl PostProcessor for LateAddedProcessor {
            async fn process(
                &self,
                _result: &mut ExtractedDocument,
                _config: &crate::core::config::ExtractionConfig,
            ) -> Result<()> {
                Ok(())
            }
            fn processing_stage(&self) -> ProcessingStage {
                ProcessingStage::Middle
            }
        }

        *PROCESSOR_CACHE.write() = None;

        let has_late_added =
            |processors: &[std::sync::Arc<dyn PostProcessor>]| processors.iter().any(|p| p.name() == "late-added-215");

        // Populate the cache, exactly as the first pipeline run of a process would.
        initialize_processor_cache().unwrap();
        let (_, middle, _) = get_processors_from_cache().unwrap();
        assert!(
            !has_late_added(&middle),
            "the processor under test must not be in the cache before it is registered"
        );

        // Register a processor *after* the cache already holds a snapshot.
        crate::plugins::register_post_processor(Arc::new(LateAddedProcessor)).unwrap();

        // Without the #215 fix, `initialize_processor_cache` is a no-op once the
        // cache is `Some(_)`, so the newly registered processor would never appear.
        initialize_processor_cache().unwrap();
        let (_, middle, _) = get_processors_from_cache().unwrap();
        assert!(
            has_late_added(&middle),
            "the cache must pick up the post-registration processor"
        );

        *PROCESSOR_CACHE.write() = None;
    }
}
