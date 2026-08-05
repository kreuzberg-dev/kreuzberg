//! Dispatch to the configured OCR backend's `probe()` via the registry.

use super::DoctorCheck;
use crate::core::config::{ExtractionConfig, OcrConfig};

pub(super) fn probe_ocr(config: &ExtractionConfig) -> Vec<DoctorCheck> {
    match &config.ocr {
        Some(ocr) if ocr.enabled => probe_backend(ocr),
        Some(_) => Vec::new(),
        // force_ocr without an explicit OCR config runs the default backend at
        // runtime (extractors build OcrConfig::default()), so probe that.
        None if config.force_ocr || config.force_ocr_pages.is_some() => probe_backend(&OcrConfig::default()),
        None => Vec::new(),
    }
}

#[cfg(any(feature = "ocr", feature = "ocr-wasm", feature = "ocr-pipeline"))]
fn probe_backend(ocr: &OcrConfig) -> Vec<DoctorCheck> {
    #[cfg(not(target_arch = "wasm32"))]
    crate::plugins::ensure_ocr_backends_initialized();
    // Resolve under the read lock, probe after releasing it: a probe that
    // touches the registry (e.g. a custom backend) must not deadlock.
    let backend = {
        let registry = crate::plugins::registry::get_ocr_backend_registry();
        let guard = registry.read();
        guard.get(&ocr.backend)
    };

    match backend {
        Ok(backend) => vec![backend.probe(ocr)],
        Err(e) => vec![DoctorCheck::fail(
            format!("ocr.{}", ocr.backend),
            format!("backend not available: {e}"),
        )],
    }
}

#[cfg(not(any(feature = "ocr", feature = "ocr-wasm", feature = "ocr-pipeline")))]
fn probe_backend(ocr: &OcrConfig) -> Vec<DoctorCheck> {
    vec![DoctorCheck::fail(
        format!("ocr.{}", ocr.backend),
        "no OCR backend support compiled in (enable an OCR feature)",
    )]
}
