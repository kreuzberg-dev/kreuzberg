//! Layout probe fallback for builds without a layout inference engine.

use super::DoctorCheck;
use crate::core::config::ExtractionConfig;

pub(super) fn probe_layout(config: &ExtractionConfig) -> Vec<DoctorCheck> {
    // Without `layout-types` the config cannot even express layout detection.
    #[cfg(not(feature = "layout-types"))]
    {
        let _ = config;
        Vec::new()
    }
    #[cfg(feature = "layout-types")]
    {
        if config.layout.is_none() {
            return Vec::new();
        }
        #[cfg(target_arch = "wasm32")]
        let reason = "layout inference is not available on wasm32";
        #[cfg(all(not(target_arch = "wasm32"), not(layout_detection)))]
        let reason = "layout detection is not compiled in (enable `layout-detection` or `layout-tract`)";
        vec![DoctorCheck::skip("layout", reason)]
    }
}
