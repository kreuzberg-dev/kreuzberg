//! Candle-based VLM OCR backends.
//!
//! Pure-Rust transformer OCR via the `xberg-candle-ocr` crate. This module
//! holds the `OcrBackend + Plugin` impls and the per-model configuration
//! plumbing; model code itself lives in `xberg-candle-ocr::models`.

mod config;

#[cfg(feature = "candle-trocr")]
pub mod trocr_backend;

#[cfg(feature = "candle-paddleocr-vl")]
pub mod paddleocr_vl_backend;

#[cfg(all(feature = "candle-glm-ocr", not(target_arch = "wasm32")))]
pub mod glm_ocr_backend;

#[cfg(feature = "candle-paddleocr-vl")]
pub(crate) mod model_stager;

#[cfg(all(feature = "candle-deepseek-ocr", not(target_arch = "wasm32")))]
pub mod deepseek_ocr_backend;

pub use config::{CandleModelId, CandleOcrConfig};

#[cfg(feature = "candle-trocr")]
pub use trocr_backend::TrocrBackend;

#[cfg(feature = "candle-paddleocr-vl")]
pub use paddleocr_vl_backend::PaddleOcrVlBackend;

#[cfg(all(feature = "candle-glm-ocr", not(target_arch = "wasm32")))]
pub use glm_ocr_backend::GlmOcrBackend;

#[cfg(all(feature = "candle-deepseek-ocr", not(target_arch = "wasm32")))]
pub use deepseek_ocr_backend::DeepseekOcrBackend;

#[cfg(any(
    feature = "candle-trocr",
    feature = "candle-paddleocr-vl",
    all(
        not(target_arch = "wasm32"),
        any(feature = "candle-glm-ocr", feature = "candle-deepseek-ocr")
    )
))]
use crate::core::config::{AccelerationConfig, ExecutionProviderType, OcrConfig};
#[cfg(any(
    feature = "candle-trocr",
    feature = "candle-paddleocr-vl",
    all(
        not(target_arch = "wasm32"),
        any(feature = "candle-glm-ocr", feature = "candle-deepseek-ocr")
    )
))]
use xberg_candle_ocr::DevicePreference;

/// Resolve a candle [`DevicePreference`] from the centralised acceleration
/// config plus the candle-specific `backend_options.device` override.
///
/// Precedence (highest first):
/// 1. `OcrConfig.backend_options.device` (when present) — an explicit
///    per-call override.
/// 2. `OcrConfig.acceleration.provider` — the central config that already
///    drives layout-detection and embeddings.
/// 3. `DevicePreference::Auto`.
///
/// The mapping from [`ExecutionProviderType`] (ORT-flavoured) to
/// [`DevicePreference`] (candle-flavoured) is:
/// - `Auto`     -> `DevicePreference::Auto`
/// - `Cpu`      -> `DevicePreference::Cpu`
/// - `Cuda`     -> `DevicePreference::Cuda`
/// - `CoreMl`   -> `DevicePreference::Metal` (Apple Neural Engine + GPU runs on Metal in candle)
/// - `TensorRt` -> `DevicePreference::Cuda` (TensorRT runs on CUDA hardware; candle has no separate TRT path)
#[cfg(any(
    feature = "candle-trocr",
    feature = "candle-paddleocr-vl",
    all(
        not(target_arch = "wasm32"),
        any(feature = "candle-glm-ocr", feature = "candle-deepseek-ocr")
    )
))]
pub(crate) fn resolve_device_preference(config: &OcrConfig) -> DevicePreference {
    if let Some(opts) = &config.backend_options
        && let Some(v) = opts.get("device").and_then(|v| v.as_str())
    {
        match v {
            "cpu" => return DevicePreference::Cpu,
            "cuda" => return DevicePreference::Cuda,
            "metal" => return DevicePreference::Metal,
            "auto" => return DevicePreference::Auto,
            _ => {}
        }
    }

    if let Some(accel) = &config.acceleration {
        return device_preference_from_acceleration(accel);
    }

    DevicePreference::Auto
}

/// Map an [`AccelerationConfig`] to the candle [`DevicePreference`] taxonomy.
///
/// Lifted out of `resolve_device_preference` so the mapping is independently
/// testable and reusable from future candle backends.
#[cfg(any(
    feature = "candle-trocr",
    feature = "candle-paddleocr-vl",
    all(
        not(target_arch = "wasm32"),
        any(feature = "candle-glm-ocr", feature = "candle-deepseek-ocr")
    )
))]
fn device_preference_from_acceleration(accel: &AccelerationConfig) -> DevicePreference {
    match accel.provider {
        ExecutionProviderType::Auto => DevicePreference::Auto,
        ExecutionProviderType::Cpu => DevicePreference::Cpu,
        ExecutionProviderType::Cuda | ExecutionProviderType::TensorRt => DevicePreference::Cuda,
        ExecutionProviderType::CoreMl => DevicePreference::Metal,
    }
}
