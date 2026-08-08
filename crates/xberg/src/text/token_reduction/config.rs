use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Intensity level for the token-reduction pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum ReductionLevel {
    /// No reduction applied; text is returned as-is.
    #[default]
    Off = 0,
    /// Remove only the most common stopwords.
    Light = 1,
    /// Balanced stopword removal and redundancy filtering.
    Moderate = 2,
    /// Aggressive filtering; may remove less common content words.
    Aggressive = 3,
    /// Maximum compression; prioritizes brevity over completeness.
    Maximum = 4,
}

impl From<&str> for ReductionLevel {
    fn from(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "off" => ReductionLevel::Off,
            "light" => ReductionLevel::Light,
            "moderate" => ReductionLevel::Moderate,
            "aggressive" => ReductionLevel::Aggressive,
            "maximum" => ReductionLevel::Maximum,
            _ => ReductionLevel::Moderate,
        }
    }
}

/// Configuration for the token-reduction pipeline.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TokenReductionConfig {
    /// Reduction intensity level.
    pub level: ReductionLevel,
    /// ISO 639-1 language code hint for stopword selection (e.g. `"en"`, `"de"`).
    pub language_hint: Option<String>,
    /// Preserve Markdown formatting tokens during reduction.
    pub preserve_markdown: bool,
    /// Preserve code block contents unchanged.
    pub preserve_code: bool,
    /// Cosine similarity threshold below which sentences are considered dissimilar.
    pub semantic_threshold: f32,
    /// Use Rayon parallel iterators for multi-core processing.
    pub enable_parallel: bool,
    /// Use SIMD-optimized text scanning where available.
    pub use_simd: bool,
    /// Per-language custom stopword lists (`language_code → stopword_list`).
    pub custom_stopwords: Option<HashMap<String, Vec<String>>>,
    /// Regex patterns whose matched text is always preserved unchanged.
    pub preserve_patterns: Vec<String>,
    /// Target fraction of text to retain (0.0–1.0); `None` = no fixed target.
    pub target_reduction: Option<f32>,
    /// Group semantically similar sentences and emit only one per cluster.
    pub enable_semantic_clustering: bool,
    /// Skip removal of words with "important" characteristics (all-caps
    /// acronyms, words containing digits, mixed-case identifiers, very long
    /// words) during the `Aggressive`/`Maximum` common-word removal pass.
    ///
    /// `true` (the default) protects those words even when they would
    /// otherwise be dropped as low-value filler; `false` lets the frequency/
    /// length heuristics apply uniformly to every word, including ones that
    /// look like acronyms or technical terms (#269).
    pub preserve_important_words: bool,
}

impl Default for TokenReductionConfig {
    fn default() -> Self {
        Self {
            level: ReductionLevel::Moderate,
            language_hint: None,
            preserve_markdown: false,
            preserve_code: true,
            semantic_threshold: 0.3,
            enable_parallel: true,
            use_simd: true,
            custom_stopwords: None,
            preserve_patterns: vec![],
            target_reduction: None,
            enable_semantic_clustering: false,
            preserve_important_words: true,
        }
    }
}
