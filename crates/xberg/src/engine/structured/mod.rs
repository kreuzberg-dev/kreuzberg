//! Generic, MIT-clean structured-extraction **mechanism**.
//!
//! This module hosts the reusable building blocks for LLM-driven structured
//! extraction — page rasterization, token-aware batching, schema validation and
//! merging, citation fusion, and prompt assembly. It is deliberately free of any
//! product policy: there are no preset definitions, no environment-variable
//! reads, no hardcoded thresholds/DPI/token-budgets baked in as non-overridable
//! behavior, and no embedded instruction templates. Every such input is a
//! **parameter** the caller supplies (DPI, [`chunk::ChunkerConfig`], merge mode,
//! prompt/citation text, excerpt limits, …), so a downstream consumer layers its
//! own policy on top without forking the mechanism.
//!
//! Like the rest of [`crate::engine`], this is **not** part of the
//! language-binding surface: `engine` is a bare `pub mod engine;` in `lib.rs`
//! whose files are not listed in `alef.toml` `sources`, so the binding generator
//! emits nothing for it. The public types here are also listed in `alef.toml`
//! `[crates.exclude] types` as belt-and-suspenders.
//!
//! Submodules are feature-gated to their hard dependencies so default-feature
//! builds (and any feature subset) still compile:
//! * [`citations`] and [`prompts`] are pure (`serde_json` + heuristics types).
//! * [`rasterize`] and [`chunk`] require the `pdf` feature (PDF rendering + the
//!   `image` crate).
//! * [`schema`] and [`finalize`] require the `presets` feature (which activates
//!   `jsonschema`); [`finalize`] composes [`schema::validate_and_merge`] with
//!   [`citations::fuse`] into the schema-driven finalization step of the
//!   structured-extraction path.

pub mod citations;
pub mod prompts;

#[cfg(feature = "pdf")]
pub mod chunk;
#[cfg(feature = "presets")]
pub mod finalize;
#[cfg(feature = "pdf")]
pub mod rasterize;
#[cfg(feature = "presets")]
pub mod schema;
