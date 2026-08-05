//! Static configuration diagnostics: misconfigurations detectable without
//! touching the environment.
//!
//! Backend-name resolution itself is reported by the OCR probe dispatch (the
//! registry is the live source of truth; the `config_validation` backend list
//! predates the candle backends and is not widened to runtime).

use super::DoctorCheck;
use crate::core::config::ExtractionConfig;
use crate::core::config::ocr::VlmFallbackPolicy;

pub(super) fn lint_config(config: &ExtractionConfig) -> Vec<DoctorCheck> {
    let mut checks = Vec::new();

    let Some(ocr) = &config.ocr else { return checks };
    if !ocr.enabled {
        return checks;
    }

    if ocr.pipeline.is_none() && ocr.vlm_fallback != VlmFallbackPolicy::Disabled && ocr.vlm_config.is_none() {
        checks.push(DoctorCheck::fail(
            "config.ocr.vlm_fallback",
            "vlm_fallback is enabled but vlm_config is missing; provide an LlmConfig with model and API key",
        ));
    }

    #[cfg(feature = "paddle-ocr")]
    if ocr.backend == "paddle-ocr" || ocr.backend == "paddleocr" {
        let (_model, warnings) = crate::paddle_ocr::select_paddle_language(&ocr.effective_languages());
        for warning in warnings {
            checks.push(DoctorCheck::fail("config.ocr.languages", warning.message.into_owned()));
        }
    }

    checks
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::config::OcrConfig;

    #[test]
    fn vlm_fallback_without_vlm_config_fails() {
        let ocr = OcrConfig {
            vlm_fallback: VlmFallbackPolicy::OnLowQuality { quality_threshold: 0.5 },
            ..OcrConfig::default()
        };
        let config = ExtractionConfig {
            ocr: Some(ocr),
            ..ExtractionConfig::default()
        };
        let checks = lint_config(&config);
        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].status, crate::doctor::ProbeStatus::Fail);
        assert_eq!(checks[0].name, "config.ocr.vlm_fallback");
    }

    #[test]
    fn default_config_has_no_lint_failures() {
        let config = ExtractionConfig {
            ocr: Some(OcrConfig::default()),
            ..ExtractionConfig::default()
        };
        let checks = lint_config(&config);
        #[cfg(not(feature = "paddle-ocr"))]
        assert!(checks.is_empty());
        #[cfg(feature = "paddle-ocr")]
        assert!(checks.iter().all(|c| c.status != crate::doctor::ProbeStatus::Fail));
    }
}
