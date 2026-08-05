//! Document orientation detection using PP-LCNet_x1_0_doc_ori.
//!
//! The `types` submodule is always available under the `auto-rotate-types` feature
//! (pure-Rust, no ORT dependency). The full detection implementation requires the
//! document-orientation capability (`auto_rotate` cfg) — provided by the ORT-backed
//! `auto-rotate` feature or the pure-Rust `auto-rotate-tract` variant.

pub mod types;
pub use types::OrientationResult;

#[cfg(auto_rotate)]
pub(crate) mod detector;
#[cfg(all(auto_rotate, not(target_arch = "wasm32"), any(feature = "ocr", sceptre_ocr)))]
pub(crate) use detector::resolve_cache_dir;
#[cfg(auto_rotate)]
pub use detector::{DocOrientationDetector, MIN_CONFIDENCE};
