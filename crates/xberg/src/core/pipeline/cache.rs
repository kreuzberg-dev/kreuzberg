//! Processor caching to reduce lock contention.
//!
//! This module manages the caching of post-processors by processing stage,
//! eliminating repeated registry lock acquisitions.

use crate::Result;
use crate::plugins::{PostProcessor, ProcessingStage};
use parking_lot::RwLock;
use std::sync::Arc;
use std::sync::LazyLock;

/// Cached post-processors for each stage to reduce lock contention.
///
/// This cache is populated once during the first pipeline run and reused
/// for all subsequent extractions, eliminating 3 of 4 registry lock acquisitions
/// per extraction.
pub(super) struct ProcessorCache {
    pub(super) early: Arc<Vec<Arc<dyn PostProcessor>>>,
    pub(super) middle: Arc<Vec<Arc<dyn PostProcessor>>>,
    pub(super) late: Arc<Vec<Arc<dyn PostProcessor>>>,
    /// The `PostProcessorRegistry` generation (see `plugins::registry::PostProcessorRegistry::generation`)
    /// this snapshot was built from. `initialize_processor_cache` (#215) compares
    /// this against the registry's live generation on every pipeline run and
    /// rebuilds the snapshot when they diverge, instead of trusting a cache
    /// populated once on the very first extraction forever.
    pub(super) generation: u64,
}

impl ProcessorCache {
    /// Create a new processor cache by fetching from the registry.
    pub(super) fn new() -> Result<Self> {
        let processor_registry = crate::plugins::registry::get_post_processor_registry();
        let registry = processor_registry.read();

        Ok(Self {
            early: Arc::new(registry.get_for_stage(ProcessingStage::Early)),
            middle: Arc::new(registry.get_for_stage(ProcessingStage::Middle)),
            late: Arc::new(registry.get_for_stage(ProcessingStage::Late)),
            generation: registry.generation(),
        })
    }
}

/// Lazy processor cache - initialized on first use, then cached.
pub(super) static PROCESSOR_CACHE: LazyLock<RwLock<Option<ProcessorCache>>> = LazyLock::new(|| RwLock::new(None));

/// Clear the processor cache (primarily for testing when registry changes).
#[cfg_attr(alef, alef(skip))]
pub fn clear_processor_cache() -> Result<()> {
    let mut cache = PROCESSOR_CACHE.write();
    *cache = None;
    Ok(())
}
