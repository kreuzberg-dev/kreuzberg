//! Core processor execution logic.
//!
//! This module handles the execution of post-processors and validators
//! in the correct order.

use crate::core::config::ExtractionConfig;
use crate::plugins::ProcessingStage;
use crate::types::{ExtractedDocument, ProcessingWarning};
use crate::{Result, XbergError};
use std::borrow::Cow;
use std::sync::Arc;
#[cfg(feature = "otel")]
use std::time::Instant;
#[cfg(feature = "otel")]
use tracing::Instrument;

type PostProcessorHandle = Arc<dyn crate::plugins::PostProcessor>;
type ProcessorStage = (ProcessingStage, Arc<Vec<PostProcessorHandle>>);

/// Execute registered post-processors for the supplied stages.
pub(super) async fn execute_processor_stages(
    result: &mut ExtractedDocument,
    config: &ExtractionConfig,
    pp_config: &Option<&crate::core::config::PostProcessorConfig>,
    stages: &[ProcessorStage],
) -> Result<()> {
    for (_stage, processors_arc) in stages {
        #[cfg(feature = "otel")]
        let stage_name = match _stage {
            ProcessingStage::Early => crate::telemetry::conventions::stages::POST_PROCESSING_EARLY,
            ProcessingStage::Middle => crate::telemetry::conventions::stages::POST_PROCESSING_MIDDLE,
            ProcessingStage::Late => crate::telemetry::conventions::stages::POST_PROCESSING_LATE,
        };
        #[cfg(feature = "otel")]
        let stage_span = crate::telemetry::spans::pipeline_stage_span(stage_name);
        #[cfg(feature = "otel")]
        let stage_start = Instant::now();
        #[cfg(feature = "otel")]
        let _stage_guard = stage_span.enter();

        for processor in processors_arc.iter() {
            let processor_name = processor.name();

            let should_run = should_processor_run(pp_config, processor_name);

            if should_run && processor.should_process(result, config) {
                #[cfg(feature = "otel")]
                let processor_span = crate::telemetry::spans::pipeline_processor_span(stage_name, processor_name);

                #[cfg(feature = "otel")]
                let process_result = processor.process(result, config).instrument(processor_span).await;
                #[cfg(not(feature = "otel"))]
                let process_result = processor.process(result, config).await;

                match process_result {
                    Ok(_) => {}
                    Err(err @ XbergError::Io(_))
                    | Err(err @ XbergError::LockPoisoned(_))
                    | Err(err @ XbergError::Plugin { .. }) => {
                        return Err(err);
                    }
                    Err(err) => {
                        let error_msg = err.to_string();
                        result.processing_warnings.push(ProcessingWarning {
                            source: Cow::Owned(processor_name.to_string()),
                            message: Cow::Owned(error_msg),
                        });
                    }
                }
            }
        }

        #[cfg(feature = "otel")]
        {
            let stage_ms = stage_start.elapsed().as_secs_f64() * 1000.0;
            crate::telemetry::metrics::get_metrics().pipeline_duration_ms.record(
                stage_ms,
                &[opentelemetry::KeyValue::new(
                    crate::telemetry::conventions::PIPELINE_STAGE,
                    stage_name.to_string(),
                )],
            );
            drop(_stage_guard);
            drop(stage_span);
        }
    }
    Ok(())
}

/// Determine if a processor should run based on configuration.
fn should_processor_run(pp_config: &Option<&crate::core::config::PostProcessorConfig>, processor_name: &str) -> bool {
    if let Some(config) = pp_config {
        if let Some(ref enabled_set) = config.enabled_set {
            enabled_set.contains(processor_name)
        } else if let Some(ref disabled_set) = config.disabled_set {
            !disabled_set.contains(processor_name)
        } else if let Some(ref enabled) = config.enabled_processors {
            enabled.iter().any(|name| name == processor_name)
        } else if let Some(ref disabled) = config.disabled_processors {
            !disabled.iter().any(|name| name == processor_name)
        } else {
            true
        }
    } else {
        true
    }
}

/// Execute all registered validators.
pub(super) async fn execute_validators(result: &ExtractedDocument, config: &ExtractionConfig) -> Result<()> {
    let validator_registry = crate::plugins::registry::get_validator_registry();
    let validators = {
        let registry = validator_registry.read();
        registry.get_all()
    };

    if validators.is_empty() {
        return Ok(());
    }

    // Only entered when at least one validator will actually run, so the stage span
    // never reports a validation pass that did not happen.
    let validate_all = async {
        for validator in validators {
            if validator.should_validate(result, config) {
                validator.validate(result, config).await?;
            }
        }
        Ok::<(), XbergError>(())
    };

    #[cfg(feature = "otel")]
    {
        validate_all
            .instrument(crate::telemetry::spans::pipeline_stage_span(
                crate::telemetry::conventions::stages::VALIDATION,
            ))
            .await
    }
    #[cfg(not(feature = "otel"))]
    {
        validate_all.await
    }
}
