//! Document translation post-processor implementation.
//!
//! Translates `ExtractedDocument::content`, optionally `formatted_content`,
//! each `Chunk::content`, and every other text-bearing field (tables, pages,
//! metadata, semantic elements, and the structured document tree) into the
//! language requested by
//! [`TranslationConfig::target_lang`](crate::core::config::TranslationConfig).
//!
//! Triggered by [`ExtractionConfig::translation`](crate::core::config::ExtractionConfig::translation);
//! invoked by the Middle-stage post-processor in
//! [`crate::plugins::processor::builtin::translation`].

mod fields;
pub mod llm;

pub use llm::translate_result;
