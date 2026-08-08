//! Post-processor registry implementation.

use crate::Result;
use crate::plugins::{PostProcessor, ProcessingStage};
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

/// Registry for post-processor plugins.
///
/// Manages post-processors organized by processing stage.
#[cfg_attr(alef, alef(skip))]
pub struct PostProcessorRegistry {
    processors: HashMap<ProcessingStage, BTreeMap<i32, Vec<Arc<dyn PostProcessor>>>>,
    name_index: HashMap<String, (ProcessingStage, i32)>,
    /// Bumped on every structural change (register/remove). Consumers that cache
    /// a snapshot of this registry (see `core::pipeline::cache::ProcessorCache`,
    /// #215) record the generation their snapshot was built from and compare it
    /// against this value to detect a stale cache instead of trusting it forever.
    generation: u64,
}

impl PostProcessorRegistry {
    /// Create a new empty post-processor registry.
    pub fn new() -> Self {
        Self {
            processors: HashMap::new(),
            name_index: HashMap::new(),
            generation: 0,
        }
    }

    /// Current registration generation.
    ///
    /// Increments once per successful `register` and once per successful
    /// `remove` (including the removals `shutdown_all` performs). A cached
    /// snapshot is stale whenever this value has moved past the generation
    /// recorded when the snapshot was taken.
    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    /// Register a post-processor.
    ///
    /// Priority comes from the [`PostProcessor::priority`] trait method
    /// (default: 50; higher = runs first within stage).
    ///
    /// # Arguments
    ///
    /// * `processor` - The post-processor to register
    pub fn register(&mut self, processor: Arc<dyn PostProcessor>) -> Result<()> {
        let priority = processor.priority();
        let name = processor.name().to_string();
        let stage = processor.processing_stage();

        if let Err(e) = super::validate_plugin_name(&name) {
            tracing::warn!(
                "Failed to validate post-processor name '{}': {}. \
                 Registration aborted. Plugin names must be non-empty and contain only alphanumeric characters, hyphens, and underscores.",
                name,
                e
            );
            return Err(e);
        }

        if let Err(e) = processor.initialize() {
            tracing::error!(
                "Failed to initialize post-processor '{}' for processing stage {:?} with priority {}: {}. \
                 Post-processing step will not be executed.",
                name,
                stage,
                priority,
                e
            );
            return Err(e);
        }

        if self.name_index.contains_key(&name) {
            tracing::debug!(
                "Post-processor '{}' is already registered. Removing old instance and registering new one.",
                name
            );
            self.remove(&name)?;
        }

        self.processors
            .entry(stage)
            .or_default()
            .entry(priority)
            .or_default()
            .push(Arc::clone(&processor));

        self.name_index.insert(name.clone(), (stage, priority));
        self.generation = self.generation.wrapping_add(1);
        tracing::debug!(
            "Registered post-processor '{}' for stage {:?} with priority {}",
            name,
            stage,
            priority
        );

        Ok(())
    }

    /// Get all processors for a specific stage, in priority order.
    ///
    /// # Arguments
    ///
    /// * `stage` - The processing stage
    ///
    /// # Returns
    ///
    /// Vector of processors in priority order (highest first).
    pub fn get_for_stage(&self, stage: ProcessingStage) -> Vec<Arc<dyn PostProcessor>> {
        let mut result = Vec::new();

        if let Some(priority_map) = self.processors.get(&stage) {
            for (_priority, processors) in priority_map.iter().rev() {
                for processor in processors {
                    result.push(Arc::clone(processor));
                }
            }
        }

        result
    }

    /// List all registered processor names.
    pub fn list(&self) -> Vec<String> {
        self.name_index.keys().cloned().collect()
    }

    /// Remove a processor from the registry.
    pub fn remove(&mut self, name: &str) -> Result<()> {
        let (stage, priority) = match self.name_index.remove(name) {
            Some(location) => location,
            None => {
                tracing::debug!(
                    "Post-processor '{}' not found in registry (already removed or never registered)",
                    name
                );
                return Ok(());
            }
        };
        self.generation = self.generation.wrapping_add(1);

        let processor_to_shutdown = if let Some(priority_map) = self.processors.get_mut(&stage) {
            let processor = priority_map.get_mut(&priority).and_then(|processors| {
                processors
                    .iter()
                    .position(|p| p.name() == name)
                    .map(|pos| processors.remove(pos))
            });

            if let Some(processors) = priority_map.get(&priority)
                && processors.is_empty()
            {
                priority_map.remove(&priority);
            }

            if priority_map.is_empty() {
                self.processors.remove(&stage);
            }
            processor
        } else {
            None
        };

        if let Some(processor) = processor_to_shutdown {
            if let Err(e) = processor.shutdown() {
                tracing::warn!(
                    "Failed to shutdown post-processor '{}': {}. \
                     Resources may not have been properly released.",
                    name,
                    e
                );
                return Err(e);
            }
            tracing::debug!("Successfully removed and shut down post-processor '{}'", name);
        }

        Ok(())
    }

    /// Shutdown all processors and clear the registry.
    pub fn shutdown_all(&mut self) -> Result<()> {
        let names = self.list();
        let count = names.len();

        if count > 0 {
            tracing::debug!("Shutting down {} post-processors", count);
        }

        for name in names {
            self.remove(&name)?;
        }

        if count > 0 {
            tracing::debug!("Successfully shut down all {} post-processors", count);
        }
        Ok(())
    }

    /// Drain the registry. Alias for `shutdown_all` used by alef trait-bridge codegen.
    pub fn clear(&mut self) -> Result<()> {
        self.shutdown_all()
    }
}

impl Default for PostProcessorRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::XbergError;
    use crate::core::config::ExtractionConfig;
    use crate::plugins::Plugin;
    use crate::types::ExtractedDocument;
    use async_trait::async_trait;

    struct MockPostProcessor {
        name: String,
        stage: ProcessingStage,
        priority: i32,
    }

    #[allow(dead_code)]
    impl MockPostProcessor {
        fn new(name: impl Into<String>, stage: ProcessingStage, priority: i32) -> Self {
            Self {
                name: name.into(),
                stage,
                priority,
            }
        }
    }

    impl Plugin for MockPostProcessor {
        fn name(&self) -> &str {
            &self.name
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
    impl PostProcessor for MockPostProcessor {
        async fn process(&self, _result: &mut ExtractedDocument, _: &ExtractionConfig) -> Result<()> {
            Ok(())
        }

        fn processing_stage(&self) -> ProcessingStage {
            self.stage
        }

        fn priority(&self) -> i32 {
            self.priority
        }
    }

    #[test]
    fn test_post_processor_registry() {
        let mut registry = PostProcessorRegistry::new();

        let early = Arc::new(MockPostProcessor {
            name: "early-processor".to_string(),
            stage: ProcessingStage::Early,
            priority: 100,
        });

        let middle = Arc::new(MockPostProcessor {
            name: "middle-processor".to_string(),
            stage: ProcessingStage::Middle,
            priority: 50,
        });

        registry.register(early).unwrap();
        registry.register(middle).unwrap();

        let early_processors = registry.get_for_stage(ProcessingStage::Early);
        assert_eq!(early_processors.len(), 1);
        assert_eq!(early_processors[0].name(), "early-processor");

        let middle_processors = registry.get_for_stage(ProcessingStage::Middle);
        assert_eq!(middle_processors.len(), 1);

        let names = registry.list();
        assert_eq!(names.len(), 2);
    }

    #[test]
    fn test_post_processor_registry_remove() {
        let mut registry = PostProcessorRegistry::new();

        let processor = Arc::new(MockPostProcessor {
            name: "test-processor".to_string(),
            stage: ProcessingStage::Early,
            priority: 50,
        });

        registry.register(processor).unwrap();
        assert_eq!(registry.get_for_stage(ProcessingStage::Early).len(), 1);

        registry.remove("test-processor").unwrap();
        assert_eq!(registry.get_for_stage(ProcessingStage::Early).len(), 0);
    }

    #[test]
    fn test_post_processor_registry_default() {
        let registry = PostProcessorRegistry::default();
        assert_eq!(registry.list().len(), 0);
    }

    #[test]
    fn test_post_processor_registry_invalid_name_empty() {
        let mut registry = PostProcessorRegistry::new();

        let processor = Arc::new(MockPostProcessor {
            name: "".to_string(),
            stage: ProcessingStage::Early,
            priority: 50,
        });

        let result = registry.register(processor);
        assert!(matches!(result, Err(XbergError::Validation { .. })));
    }

    #[test]
    fn test_post_processor_registry_invalid_name_whitespace() {
        let mut registry = PostProcessorRegistry::new();

        let processor = Arc::new(MockPostProcessor {
            name: "my processor".to_string(),
            stage: ProcessingStage::Early,
            priority: 50,
        });

        let result = registry.register(processor);
        assert!(matches!(result, Err(XbergError::Validation { .. })));
    }

    #[test]
    fn test_post_processor_registry_shutdown_all() {
        let mut registry = PostProcessorRegistry::new();

        let early = Arc::new(MockPostProcessor {
            name: "early".to_string(),
            stage: ProcessingStage::Early,
            priority: 100,
        });

        let late = Arc::new(MockPostProcessor {
            name: "late".to_string(),
            stage: ProcessingStage::Late,
            priority: 50,
        });

        registry.register(early).unwrap();
        registry.register(late).unwrap();

        assert_eq!(registry.list().len(), 2);

        registry.shutdown_all().unwrap();
        assert_eq!(registry.list().len(), 0);
    }

    #[test]
    fn test_post_processor_registry_priority_order() {
        let mut registry = PostProcessorRegistry::new();

        let low = Arc::new(MockPostProcessor {
            name: "low-priority".to_string(),
            stage: ProcessingStage::Early,
            priority: 10,
        });

        let high = Arc::new(MockPostProcessor {
            name: "high-priority".to_string(),
            stage: ProcessingStage::Early,
            priority: 100,
        });

        registry.register(low).unwrap();
        registry.register(high).unwrap();

        let processors = registry.get_for_stage(ProcessingStage::Early);
        assert_eq!(processors.len(), 2);
        assert_eq!(processors[0].name(), "high-priority");
        assert_eq!(processors[1].name(), "low-priority");
    }

    #[test]
    fn test_post_processor_registry_empty_stage() {
        let registry = PostProcessorRegistry::new();

        let processors = registry.get_for_stage(ProcessingStage::Late);
        assert_eq!(processors.len(), 0);
    }

    struct FailingPostProcessor {
        name: String,
        stage: ProcessingStage,
        fail_on_init: bool,
    }

    impl Plugin for FailingPostProcessor {
        fn name(&self) -> &str {
            &self.name
        }
        fn version(&self) -> String {
            "1.0.0".to_string()
        }
        fn initialize(&self) -> Result<()> {
            if self.fail_on_init {
                Err(XbergError::Plugin {
                    message: "Processor initialization failed".to_string(),
                    plugin_name: self.name.clone(),
                })
            } else {
                Ok(())
            }
        }
        fn shutdown(&self) -> Result<()> {
            Ok(())
        }
    }

    #[async_trait]
    impl PostProcessor for FailingPostProcessor {
        async fn process(&self, _result: &mut ExtractedDocument, _: &ExtractionConfig) -> Result<()> {
            Ok(())
        }

        fn processing_stage(&self) -> ProcessingStage {
            self.stage
        }
    }

    #[test]
    fn test_post_processor_initialization_failure_logs_error() {
        let mut registry = PostProcessorRegistry::new();

        let processor = Arc::new(FailingPostProcessor {
            name: "failing-processor".to_string(),
            stage: ProcessingStage::Early,
            fail_on_init: true,
        });

        let result = registry.register(processor);
        assert!(result.is_err());
        assert_eq!(registry.list().len(), 0);
    }

    #[test]
    fn test_post_processor_invalid_name_empty_logs_warning() {
        let mut registry = PostProcessorRegistry::new();

        let processor = Arc::new(MockPostProcessor {
            name: "".to_string(),
            stage: ProcessingStage::Early,
            priority: 50,
        });

        let result = registry.register(processor);
        assert!(matches!(result, Err(XbergError::Validation { .. })));
    }

    #[test]
    fn test_post_processor_invalid_name_with_spaces_logs_warning() {
        let mut registry = PostProcessorRegistry::new();

        let processor = Arc::new(MockPostProcessor {
            name: "invalid processor".to_string(),
            stage: ProcessingStage::Early,
            priority: 50,
        });

        let result = registry.register(processor);
        assert!(matches!(result, Err(XbergError::Validation { .. })));
    }

    #[test]
    fn test_post_processor_successful_registration_logs_debug() {
        let mut registry = PostProcessorRegistry::new();

        let processor = Arc::new(MockPostProcessor {
            name: "valid-processor".to_string(),
            stage: ProcessingStage::Early,
            priority: 50,
        });

        let result = registry.register(processor);
        assert!(result.is_ok());
        assert_eq!(registry.list().len(), 1);
    }

    #[test]
    fn test_post_processor_remove_nonexistent_logs_debug() {
        let mut registry = PostProcessorRegistry::new();

        let result = registry.remove("nonexistent-processor");
        assert!(result.is_ok());
        assert_eq!(registry.list().len(), 0);
    }

    #[test]
    fn test_post_processor_register_same_name_twice() {
        let mut registry = PostProcessorRegistry::new();

        let processor = Arc::new(MockPostProcessor {
            name: "duplicate-processor".to_string(),
            stage: ProcessingStage::Early,
            priority: 50,
        });

        registry.register(processor.clone()).unwrap();
        assert_eq!(registry.list().len(), 1);

        registry.register(processor).unwrap();
        assert_eq!(registry.list().len(), 1);
    }

    #[test]
    fn test_post_processor_multiple_stages() {
        let mut registry = PostProcessorRegistry::new();

        let early_processor = Arc::new(MockPostProcessor {
            name: "early-proc".to_string(),
            stage: ProcessingStage::Early,
            priority: 100,
        });

        let middle_processor = Arc::new(MockPostProcessor {
            name: "middle-proc".to_string(),
            stage: ProcessingStage::Middle,
            priority: 50,
        });

        let late_processor = Arc::new(MockPostProcessor {
            name: "late-proc".to_string(),
            stage: ProcessingStage::Late,
            priority: 25,
        });

        registry.register(early_processor).unwrap();
        registry.register(middle_processor).unwrap();
        registry.register(late_processor).unwrap();

        assert_eq!(registry.get_for_stage(ProcessingStage::Early).len(), 1);
        assert_eq!(registry.get_for_stage(ProcessingStage::Middle).len(), 1);
        assert_eq!(registry.get_for_stage(ProcessingStage::Late).len(), 1);
    }

    /// #215: the registry must expose a generation counter that advances on
    /// every structural change, so a cache built from a snapshot can detect
    /// that it is stale instead of caching forever.
    #[test]
    fn test_post_processor_registry_generation_advances_on_register_and_remove() {
        let mut registry = PostProcessorRegistry::new();
        assert_eq!(registry.generation(), 0);

        let processor = Arc::new(MockPostProcessor {
            name: "gen-processor".to_string(),
            stage: ProcessingStage::Early,
            priority: 50,
        });
        registry.register(processor).unwrap();
        assert_eq!(registry.generation(), 1, "register must advance the generation");

        registry.remove("gen-processor").unwrap();
        assert_eq!(registry.generation(), 2, "remove must advance the generation");

        // A no-op remove (nothing to remove) must not advance the generation —
        // nothing structurally changed.
        registry.remove("does-not-exist").unwrap();
        assert_eq!(registry.generation(), 2);
    }

    #[test]
    fn test_post_processor_shutdown_empty_registry() {
        let mut registry = PostProcessorRegistry::new();
        let result = registry.shutdown_all();
        assert!(result.is_ok());
        assert_eq!(registry.list().len(), 0);
    }
}
