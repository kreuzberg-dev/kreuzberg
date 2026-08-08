//! Redaction & anonymisation configuration.
//!
//! When `ExtractionConfig::redaction` is `Some`, the redaction post-processor runs
//! as the Late stage of the pipeline and rewrites `content`, `formatted_content`,
//! every chunk's text, and the textual fields of `entities` / `summary` /
//! `translation` / `page_classifications` using the configured strategy. The
//! original text never appears in the returned `ExtractedDocument`.

use crate::Result;
use crate::types::redaction::{PiiCategory, RedactionStrategy};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// Configuration for the redaction post-processor.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "api", derive(utoipa::ToSchema))]
#[cfg_attr(feature = "alef-meta", alef(since = "1.0.0"))]
pub struct RedactionConfig {
    /// Categories to redact. Empty means "every category supported by the engine."
    #[serde(default)]
    #[cfg_attr(feature = "api", schema(value_type = Vec<PiiCategory>))]
    pub categories: HashSet<PiiCategory>,
    /// Strategy applied to every match.
    #[serde(default)]
    pub strategy: RedactionStrategy,
    /// Optional NER backend — required to redact PERSON / ORGANIZATION / LOCATION
    /// categories (the pure-Rust pattern engine only covers regex-detectable PII).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ner: Option<super::ner::NerConfig>,
    /// When `true`, chunk byte ranges are kept consistent with the rewritten content by
    /// adjusting `byte_start` / `byte_end` after replacement. When `false`, chunk byte
    /// ranges still refer to the *original* content offsets — useful when downstream
    /// consumers want to map findings back to the original document.
    #[serde(default = "default_preserve_offsets")]
    pub preserve_offsets: bool,
    /// Arbitrary user-supplied literal terms to redact.
    ///
    /// Each term is treated as a regex hit against the document, surfacing as
    /// `PiiCategory::Custom(label)` in [`RedactionFinding`](crate::types::redaction::RedactionFinding)
    /// where `label` is the per-term label (defaulting to the literal value itself).
    /// Case-insensitive by default; set [`RedactionTerm::case_sensitive`] for exact match.
    ///
    /// Use this when you need to redact tenant-specific tokens (employee IDs,
    /// project codes, internal product names) without writing a custom plugin.
    #[serde(default)]
    pub custom_terms: Vec<RedactionTerm>,
    /// Arbitrary user-supplied regex patterns to redact.
    ///
    /// Same surfacing semantics as [`custom_terms`](Self::custom_terms): each
    /// hit becomes a `PiiCategory::Custom(label)` finding. Patterns are validated
    /// at config-construction time via [`RedactionConfig::validate`].
    #[serde(default)]
    pub custom_patterns: Vec<RedactionPattern>,
}

fn default_preserve_offsets() -> bool {
    true
}

fn default_case_sensitive() -> bool {
    false
}

/// One user-supplied literal term to redact.
///
/// Matched as a regex-escaped substring (so callers do not need to escape
/// metacharacters themselves). Case-insensitive by default — set
/// [`Self::case_sensitive`] to `true` for exact byte-match semantics.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "api", derive(utoipa::ToSchema))]
pub struct RedactionTerm {
    /// Custom category label surfaced in [`RedactionFinding::category`](crate::types::redaction::RedactionFinding::category).
    pub label: String,
    /// Literal value to match. Regex metacharacters are escaped automatically.
    pub value: String,
    /// When `true`, match the value as-is; otherwise match ASCII-case-insensitively.
    #[serde(default = "default_case_sensitive")]
    pub case_sensitive: bool,
}

impl RedactionTerm {
    /// Build a term whose label is the literal value itself (case-insensitive).
    pub fn literal(value: impl Into<String>) -> Self {
        let v = value.into();
        Self {
            label: v.clone(),
            value: v,
            case_sensitive: false,
        }
    }

    /// Build a term with a custom label.
    pub fn labeled(label: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            value: value.into(),
            case_sensitive: false,
        }
    }
}

/// One user-supplied regex pattern to redact.
///
/// The pattern is compiled with the Rust `regex` crate (no look-around). Case
/// sensitivity is encoded in the pattern via the `(?i)` inline flag when
/// [`Self::case_sensitive`] is `false`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "api", derive(utoipa::ToSchema))]
pub struct RedactionPattern {
    /// Custom category label surfaced in [`RedactionFinding::category`](crate::types::redaction::RedactionFinding::category).
    pub label: String,
    /// Regex pattern (Rust `regex` crate dialect — no look-around).
    pub pattern: String,
    /// When `true`, match case-sensitively; otherwise prepend `(?i)` to the regex.
    #[serde(default = "default_case_sensitive")]
    pub case_sensitive: bool,
}

impl RedactionPattern {
    /// Build a pattern with the given label (case-insensitive by default).
    pub fn labeled(label: impl Into<String>, pattern: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            pattern: pattern.into(),
            case_sensitive: false,
        }
    }
}

impl Default for RedactionConfig {
    fn default() -> Self {
        Self {
            categories: HashSet::new(),
            strategy: RedactionStrategy::default(),
            ner: None,
            preserve_offsets: true,
            custom_terms: Vec::new(),
            custom_patterns: Vec::new(),
        }
    }
}

impl RedactionConfig {
    /// Validate user-supplied terms and patterns at config-construction time.
    ///
    /// Compiles every [`RedactionPattern::pattern`] (with the case-insensitive
    /// inline flag where applicable) and returns the first compilation error so
    /// the caller can reject the config before the redaction pipeline runs.
    /// Pure terms (regex-escaped) cannot fail to compile, but the function
    /// still rejects empty values to avoid degenerate zero-length matches.
    pub fn validate(&self) -> Result<()> {
        for term in &self.custom_terms {
            if term.value.is_empty() {
                return Err(crate::XbergError::validation(format!(
                    "RedactionConfig.custom_terms[{}]: value is empty",
                    term.label
                )));
            }
        }
        for pattern in &self.custom_patterns {
            if pattern.pattern.is_empty() {
                return Err(crate::XbergError::validation(format!(
                    "RedactionConfig.custom_patterns[{}]: pattern is empty",
                    pattern.label
                )));
            }
            let compiled = if pattern.case_sensitive {
                regex::Regex::new(&pattern.pattern)
            } else {
                regex::Regex::new(&format!("(?i){}", pattern.pattern))
            };
            if let Err(err) = compiled {
                return Err(crate::XbergError::validation(format!(
                    "RedactionConfig.custom_patterns[{}]: invalid regex: {err}",
                    pattern.label
                )));
            }
        }
        Ok(())
    }
}
