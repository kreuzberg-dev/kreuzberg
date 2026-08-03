//! Layout probe: verify the RT-DETR model is cached and can actually run an
//! inference with the configured execution provider.
//!
//! This is the probe that catches the "CoreML cannot execute the model, layout
//! silently degrades to no-layout output" class (#1344): a cache/SHA check alone
//! cannot see inference-time failures, so the probe runs one real detection on a
//! synthetic page. No download: an uncached model reports `Skip` instead.

use super::DoctorCheck;
use crate::core::config::{AccelerationConfig, ExtractionConfig};
use crate::layout::engine::{LayoutEngine, LayoutEngineConfig, ModelBackend};
use crate::layout::model_manager::LayoutModelManager;

pub(super) fn probe_layout(config: &ExtractionConfig) -> Vec<DoctorCheck> {
    let Some(layout) = &config.layout else {
        return Vec::new();
    };

    let manager = LayoutModelManager::new(None);
    if !manager.is_rtdetr_cached() {
        return vec![DoctorCheck::skip(
            "layout.rtdetr",
            "model not cached locally (will download on first use)",
        )];
    }

    match run_inference(layout, layout.acceleration.clone()) {
        Ok(detections) => vec![DoctorCheck::pass(
            "layout.rtdetr",
            format!("inference ok ({detections} detections on synthetic page)"),
        )],
        Err(e) if runtime_retries_on_cpu(layout) => match run_inference(
            layout,
            Some(AccelerationConfig {
                provider: crate::core::config::ExecutionProviderType::Cpu,
                ..Default::default()
            }),
        ) {
            // Mirrors the layout runner's automatic-provider CPU retry: the
            // run succeeds degraded, with a processing warning, not an error.
            Ok(_) => vec![DoctorCheck::warn(
                "layout.rtdetr",
                format!(
                    "automatic execution provider failed, CPU works; runtime retries on CPU with a processing warning ({e})"
                ),
            )],
            Err(cpu_e) => vec![DoctorCheck::fail(
                "layout.rtdetr",
                format!("inference failed (automatic provider: {e}; CPU retry: {cpu_e})"),
            )],
        },
        Err(e) => vec![DoctorCheck::fail(
            "layout.rtdetr",
            format!("inference failed with configured execution provider: {e}"),
        )],
    }
}

fn run_inference(
    layout: &crate::core::config::LayoutDetectionConfig,
    acceleration: Option<AccelerationConfig>,
) -> Result<usize, crate::layout::LayoutError> {
    let engine_config = LayoutEngineConfig {
        backend: ModelBackend::RtDetr,
        confidence_threshold: layout.confidence_threshold,
        apply_heuristics: layout.apply_heuristics,
        cache_dir: None,
        acceleration,
    };
    let mut engine = LayoutEngine::from_config(engine_config)?;
    Ok(engine.detect(&synthetic_page())?.detections.len())
}

/// The layout runner retries inference once on CPU when the provider was
/// automatic and no `XBERG_ORT_EP` override is active (ORT builds only).
fn runtime_retries_on_cpu(layout: &crate::core::config::LayoutDetectionConfig) -> bool {
    #[cfg(feature = "layout-detection")]
    {
        crate::ort_discovery::execution_provider_override().is_none()
            && layout
                .acceleration
                .as_ref()
                .is_none_or(|a| a.provider == crate::core::config::ExecutionProviderType::Auto)
    }
    #[cfg(not(feature = "layout-detection"))]
    {
        let _ = layout;
        false
    }
}

/// A small page-like image: light background with dark bars approximating text
/// lines and a second block, so the detector sees realistic input statistics.
fn synthetic_page() -> image::RgbImage {
    let mut img = image::RgbImage::from_pixel(640, 480, image::Rgb([245, 245, 245]));
    let dark = image::Rgb([30, 30, 30]);
    for (top, left, width, height) in [
        (40u32, 60u32, 520u32, 14u32),
        (70, 60, 480, 14),
        (100, 60, 500, 14),
        (160, 60, 240, 120),
        (160, 340, 240, 120),
        (320, 60, 520, 14),
        (350, 60, 440, 14),
    ] {
        for y in top..(top + height) {
            for x in left..(left + width) {
                img.put_pixel(x, y, dark);
            }
        }
    }
    img
}
