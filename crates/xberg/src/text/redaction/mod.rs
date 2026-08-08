//! Redaction & anonymisation engine.
//!
//! The engine is invoked from the Late-stage post-processor at
//! [`crate::plugins::processor::builtin::redaction`]. It runs the pure-Rust
//! pattern engine (and optionally a NER backend for PERSON / ORGANIZATION /
//! LOCATION / caller-supplied custom labels) over
//! [`ExtractedDocument::content`](crate::ExtractedDocument::content) and
//! rewrites every textual field in place. Detected mentions become literal
//! matchers so they are redacted at *every* occurrence in *every* field, not
//! only at the first byte span in `content`. The original text is dropped at
//! the end of the pipeline; the audit trail lives in
//! [`ExtractedDocument::redaction_report`](crate::ExtractedDocument::redaction_report)
//! and records only replacements that were actually applied.

pub mod engine;
pub mod patterns;
#[cfg(feature = "redaction-rehydrate")]
pub mod rehydration;
pub mod strategy;

#[cfg(feature = "redaction-rehydrate")]
pub use engine::redact_capturing_rehydration_map;
pub use engine::{redact, redact_with_entities};
#[cfg(feature = "redaction-rehydrate")]
pub use rehydration::{RehydrationMap, SubjectMatch, decrypt_map, encrypt_map, find_subject, forget_subject};
