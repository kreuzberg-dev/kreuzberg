//! Tree-sitter language pack configuration.
//!
//! This module contains configuration types for the tree-sitter integration,
//! including grammar download settings and code analysis processing options.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Content rendering mode for code extraction.
///
/// Controls how extracted code content is represented in the `content` field
/// of `ExtractedDocument`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodeContentMode {
    /// Use TSLP semantic chunks as content (default).
    #[default]
    Chunks,
    /// Use raw source code as content.
    Raw,
    /// Emit function/class headings + docstrings (no code bodies).
    Structure,
}

/// Configuration for tree-sitter language pack integration.
///
/// Controls grammar download behavior and code analysis options.
///
/// # Example (TOML)
///
/// ```toml
/// [tree_sitter]
/// languages = ["python", "rust"]
/// groups = ["web"]
///
/// [tree_sitter.process]
/// structure = true
/// comments = true
/// docstrings = true
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TreeSitterConfig {
    /// Enable code intelligence processing (default: true).
    ///
    /// When `false`, tree-sitter analysis is completely skipped even if
    /// the config section is present.
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Custom cache directory for downloaded grammars.
    ///
    /// When `None`, uses the default: `~/.cache/tree-sitter-language-pack/v{version}/libs/`.
    ///
    /// Consumed both by the CLI (`tree-sitter download --from-config`,
    /// `cache warm`) and by [`crate::extractors::code::CodeExtractor`] at
    /// extraction time, so that a configured cache directory is honoured
    /// wherever grammars are looked up or downloaded, not only during an
    /// explicit CLI download.
    #[serde(default)]
    pub cache_dir: Option<PathBuf>,

    /// Languages to pre-download on init (e.g., `["python", "rust"]`).
    ///
    /// Consumed only by the CLI's `tree-sitter download --from-config` and
    /// `cache warm` commands as a pre-download hint. Extraction itself does
    /// not read this field: a given source file always processes with a
    /// single, already auto-detected language, so there is nothing for a
    /// language allowlist to gate at extraction time.
    #[serde(default)]
    pub languages: Option<Vec<String>>,

    /// Language groups to pre-download (e.g., `["web", "systems", "scripting"]`).
    ///
    /// Consumed only by the CLI's `tree-sitter download --from-config` and
    /// `cache warm` commands, for the same reason as `languages` above.
    #[serde(default)]
    pub groups: Option<Vec<String>>,

    /// Processing options for code analysis.
    #[serde(default)]
    pub process: TreeSitterProcessConfig,
}

/// Processing options for tree-sitter code analysis.
///
/// Controls which analysis features are enabled when extracting code files.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TreeSitterProcessConfig {
    /// Extract structural items (functions, classes, structs, etc.). Default: true.
    #[serde(default = "default_true")]
    pub structure: bool,

    /// Extract import statements. Default: true.
    #[serde(default = "default_true")]
    pub imports: bool,

    /// Extract export statements. Default: true.
    #[serde(default = "default_true")]
    pub exports: bool,

    /// Extract comments. Default: false.
    #[serde(default)]
    pub comments: bool,

    /// Extract docstrings. Default: false.
    #[serde(default)]
    pub docstrings: bool,

    /// Extract symbol definitions. Default: false.
    #[serde(default)]
    pub symbols: bool,

    /// Include parse diagnostics. Default: false.
    #[serde(default)]
    pub diagnostics: bool,

    /// Extract a hierarchical key/value data tree from data-format files
    /// (JSON, YAML, TOML, XML, CSV, etc.). Default: false.
    #[serde(default)]
    pub data_extraction: bool,

    /// Maximum chunk size in bytes. `None` disables chunking.
    #[serde(default)]
    pub chunk_max_size: Option<usize>,

    /// Content rendering mode for code extraction.
    #[serde(default)]
    pub content_mode: CodeContentMode,
}

impl Default for TreeSitterConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            cache_dir: None,
            languages: None,
            groups: None,
            process: TreeSitterProcessConfig::default(),
        }
    }
}

impl Default for TreeSitterProcessConfig {
    fn default() -> Self {
        Self {
            structure: true,
            imports: true,
            exports: true,
            comments: false,
            docstrings: false,
            symbols: false,
            diagnostics: false,
            data_extraction: false,
            chunk_max_size: None,
            content_mode: CodeContentMode::default(),
        }
    }
}

fn default_true() -> bool {
    true
}

/// Convert xberg's process config to TSLP's `ProcessConfig`.
///
/// The language field is left empty — callers must set it before use.
impl From<&TreeSitterProcessConfig> for tree_sitter_language_pack::ProcessConfig {
    fn from(p: &TreeSitterProcessConfig) -> Self {
        Self {
            language: std::borrow::Cow::Borrowed(""),
            structure: p.structure,
            imports: p.imports,
            exports: p.exports,
            comments: p.comments,
            docstrings: p.docstrings,
            symbols: p.symbols,
            diagnostics: p.diagnostics,
            data_extraction: p.data_extraction,
            chunk_max_size: p.chunk_max_size,
        }
    }
}
