// stdout is this binary's output contract, but per the org logging policy each
// result-output site opts in explicitly via `#[expect(clippy::print_stdout)]`;
// diagnostics must go through `tracing`, so both raw print macros are denied. ~keep
#![deny(clippy::print_stderr)]
#![deny(clippy::print_stdout)]
#![cfg_attr(test, allow(clippy::print_stderr, clippy::print_stdout))]
//! Xberg CLI - Command-line interface for document intelligence.
//!
//! This binary provides a command-line interface to the Xberg document intelligence
//! library, supporting document extraction, MIME type detection, caching, and batch operations.
//!
//! # Architecture
//!
//! The CLI is built using `clap` for argument parsing and provides five main commands:
//! - `extract`: Extract text/data from a single document
//! - `batch`: Process multiple documents in parallel
//! - `detect`: Identify MIME type of a file
//! - `cache`: Manage cache (clear, stats)
//! - `serve`: Start API server (requires `api` feature)
//! - `version`: Show version information
//!
//! # Configuration
//!
//! The CLI supports configuration files in TOML, YAML, or JSON formats:
//! - Explicit: `--config path/to/config.toml`
//! - Auto-discovery: Searches for `xberg.{toml,yaml,json}` in current and parent directories
//! - Inline JSON: `--config-json '{"ocr": {"backend": "tesseract"}}'`
//! - Command-line flags override config file settings
//!
//! Configuration precedence (highest to lowest):
//! 1. Individual CLI flags (--output-format, --ocr, etc.)
//! 2. Inline JSON config (--config-json or --config-json-base64)
//! 3. Config file (--config path.toml)
//! 4. Default values
//!
//! # Exit Codes
//!
//! - 0: Success
//! - Non-zero: Error (see stderr for details)
//!
//! # Examples
//!
//! ```bash
//! # Extract text from a PDF
//! xberg extract document.pdf
//!
//! # Extract with OCR enabled
//! xberg extract scanned.pdf --ocr true
//!
//! # Extract with inline JSON config
//! xberg extract doc.pdf --config-json '{"ocr":{"backend":"tesseract"}}'
//!
//! # Batch processing
//! xberg batch *.pdf --output-format json
//!
//! # Detect MIME type
//! xberg detect unknown-file.bin
//! ```

#![deny(unsafe_code)]

mod commands;
mod input;
mod logging;
mod output;
mod peak_memory;
mod style;

use anyhow::{Context, Result};
use clap::{CommandFactory, Parser, Subcommand};
#[cfg(feature = "embeddings")]
use commands::embed_command;
use commands::overrides::ExtractionOverrides;
#[cfg(feature = "api")]
use commands::serve_command;
#[cfg(any(
    feature = "embeddings",
    feature = "layout-detection",
    feature = "paddle-ocr",
    feature = "tree-sitter",
    feature = "ner-onnx"
))]
use commands::warm_command;
use commands::{
    BatchInputFormat, batch_command, clear_command, compiled_in_formats, doctor_command, extract_command, load_config,
    manifest_command, stats_command, validate_file_exists, validate_output_dir,
};
#[cfg(feature = "tree-sitter")]
use commands::{cache_dir_command, clean_command, download_command, list_command};
#[cfg(feature = "core-cli")]
use commands::{chunk_command, validate_chunk_params};
#[cfg(feature = "mcp")]
use commands::{mcp_command, resolve_mcp_allowed_hosts};
use input::{
    apply_json_overrides, resolve_batch_inputs, resolve_extract_input, validate_batch_input_uris,
    validate_extract_input,
};
use serde_json::json;
use std::path::PathBuf;
use std::time::Instant;
use xberg::{OutputFormat as ContentOutputFormat, detect_mime_type};

/// Xberg document intelligence CLI
#[derive(Parser)]
#[command(name = "xberg")]
#[command(version, about, long_about = None)]
struct Cli {
    /// Set log level (trace, debug, info, warn, error). Overrides RUST_LOG env var.
    #[arg(long, global = true)]
    log_level: Option<String>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Extract text from a document
    Extract {
        /// URI to the document. Local paths and file:// URIs are supported in this checkout.
        #[cfg_attr(
            feature = "url-surface",
            arg(value_name = "URI", required_unless_present_any = ["url", "stdin"])
        )]
        #[cfg_attr(
            not(feature = "url-surface"),
            arg(value_name = "URI", required_unless_present = "stdin")
        )]
        uri: Option<String>,

        /// HTTP(S) URL to extract.
        #[cfg(feature = "url-surface")]
        #[arg(long, conflicts_with_all = ["uri", "stdin"])]
        url: Option<String>,

        /// Read document bytes from stdin.
        #[cfg_attr(
            feature = "url-surface",
            arg(long, conflicts_with_all = ["uri", "url"])
        )]
        #[cfg_attr(not(feature = "url-surface"), arg(long, conflicts_with = "uri"))]
        stdin: bool,

        /// Path to config file (TOML, YAML, or JSON). If not specified, searches for xberg.toml/yaml/json in current and parent directories.
        #[arg(short, long)]
        config: Option<PathBuf>,

        /// Skip project and user config discovery and start from built-in defaults.
        #[arg(long, conflicts_with = "config")]
        no_config_discovery: bool,

        /// Inline JSON configuration. Applied after config file but before individual flags.
        ///
        /// Example: --config-json '{"ocr":{"backend":"tesseract"},"chunking":{"max_chars":1000}}'
        #[arg(long)]
        config_json: Option<String>,

        /// Base64-encoded JSON configuration. Useful for shell environments where quotes are problematic.
        ///
        /// Example: --config-json-base64 eyJvY3IiOnsiYmFja2VuZCI6InRlc3NlcmFjdCJ9fQ==
        #[arg(long)]
        config_json_base64: Option<String>,

        /// MIME type hint (auto-detected if not provided)
        #[arg(short, long)]
        mime_type: Option<String>,

        /// Output format for CLI results (text or json).
        ///
        /// Controls how the CLI displays results, not the extraction content format.
        #[arg(short, long, default_value = "text")]
        format: WireFormat,

        /// Directory where extracted image files are written (text/toon output only).
        ///
        /// When `--extract-images true` is used with text or toon format, the markdown content
        /// references image files by name (e.g. `image_0.png`). Pass this flag to control where
        /// those files land. Defaults to the current working directory when not specified.
        /// Ignored for `--format json` because JSON embeds image bytes inline.
        /// The directory must already exist.
        #[arg(long)]
        output_dir: Option<PathBuf>,

        /// Extraction configuration overrides
        #[command(flatten)]
        overrides: ExtractionOverrides,
    },

    /// Batch extract from multiple documents
    Batch {
        /// Paths to documents
        paths: Vec<PathBuf>,

        /// JSON or JSONL manifest containing batch inputs.
        #[arg(long)]
        input: Option<PathBuf>,

        /// Format for --input.
        #[arg(long, value_enum)]
        input_format: Option<BatchInputFormat>,

        /// Path to config file (TOML, YAML, or JSON). If not specified, searches for xberg.toml/yaml/json in current and parent directories.
        #[arg(short, long)]
        config: Option<PathBuf>,

        /// Skip project and user config discovery and start from built-in defaults.
        #[arg(long, conflicts_with = "config")]
        no_config_discovery: bool,

        /// Inline JSON configuration. Applied after config file but before individual flags.
        ///
        /// Example: --config-json '{"ocr":{"backend":"tesseract"},"chunking":{"max_chars":1000}}'
        #[arg(long)]
        config_json: Option<String>,

        /// Base64-encoded JSON configuration. Useful for shell environments where quotes are problematic.
        ///
        /// Example: --config-json-base64 eyJvY3IiOnsiYmFja2VuZCI6InRlc3NlcmFjdCJ9fQ==
        #[arg(long)]
        config_json_base64: Option<String>,

        /// Output format for CLI results (text or json).
        ///
        /// Controls how the CLI displays results, not the extraction content format.
        #[arg(short, long, default_value = "json")]
        format: WireFormat,

        /// Directory where extracted image files are written (text/toon output only).
        ///
        /// When `--extract-images true` is used with text or toon format, the markdown content
        /// references image files by name (e.g. `image_0.png`). Pass this flag to control where
        /// those files land. Defaults to the current working directory when not specified.
        /// Ignored for `--format json` because JSON embeds image bytes inline.
        /// The directory must already exist.
        #[arg(long)]
        output_dir: Option<PathBuf>,

        /// Extraction configuration overrides
        #[command(flatten)]
        overrides: ExtractionOverrides,

        /// Path to a JSON file mapping file paths to per-file extraction config overrides.
        /// The JSON should be an object where keys are file paths and values are FileExtractionConfig objects.
        /// Example: {"doc1.pdf": {"force_ocr": true}, "doc2.pdf": {"output_format": "markdown"}}
        #[arg(long)]
        file_configs: Option<PathBuf>,
    },

    /// Detect MIME type of a file
    Detect {
        /// Path to the file
        path: PathBuf,

        /// Output format (text or json)
        #[arg(short, long, default_value = "text")]
        format: WireFormat,
    },

    /// List all supported document formats
    Formats {
        /// Output format (text or json)
        #[arg(short, long, default_value = "text")]
        format: WireFormat,
    },

    /// Show version information
    Version {
        /// Output format (text or json)
        #[arg(short, long, default_value = "text")]
        format: WireFormat,
    },

    /// Cache management operations
    Cache {
        #[command(subcommand)]
        command: CacheCommands,
    },

    /// Manage tree-sitter grammar parsers
    #[cfg(feature = "tree-sitter")]
    TreeSitter {
        #[command(subcommand)]
        command: TreeSitterCommands,
    },

    /// Probe configured backends and report what will actually execute on this host
    Doctor {
        /// Path to config file (TOML, YAML, or JSON). If not specified, searches for xberg.toml/yaml/json in current and parent directories.
        #[arg(short, long)]
        config: Option<PathBuf>,

        /// Skip project and user config discovery and start from built-in defaults.
        #[arg(long, conflicts_with = "config")]
        no_config_discovery: bool,

        /// Output format
        #[arg(short, long, default_value = "text")]
        format: WireFormat,

        /// Delete stray files reported in xberg-owned cache directories after probing
        #[arg(long)]
        clean: bool,
    },

    /// Start the API server
    ///
    /// Configuration is loaded with the following precedence (highest to lowest):
    /// 1. CLI arguments (--host, --port)
    /// 2. Environment variables (XBERG_HOST, XBERG_PORT)
    /// 3. Config file (TOML, YAML, or JSON)
    /// 4. Built-in defaults (127.0.0.1:8000)
    ///
    /// The config file can contain both extraction and server settings under `[server]` section.
    #[cfg(feature = "api")]
    Serve {
        /// Host to bind to (e.g., "127.0.0.1" or "0.0.0.0"). CLI arg overrides config file and env vars.
        #[arg(short = 'H', long)]
        host: Option<String>,

        /// Port to bind to. CLI arg overrides config file and env vars.
        #[arg(short, long)]
        port: Option<u16>,

        /// Path to config file (TOML, YAML, or JSON). If not specified, searches for xberg.toml/yaml/json in current and parent directories.
        #[arg(short, long)]
        config: Option<PathBuf>,
    },

    /// Start the MCP (Model Context Protocol) server
    #[cfg(feature = "mcp")]
    Mcp {
        /// Path to config file (TOML, YAML, or JSON). If not specified, searches for xberg.toml/yaml/json in current and parent directories.
        #[arg(short, long)]
        config: Option<PathBuf>,

        /// Transport mode: stdio (default) or http
        #[arg(long, default_value = "stdio")]
        transport: String,

        /// HTTP host (only for --transport http)
        #[arg(long, default_value = "127.0.0.1")]
        host: String,

        /// HTTP port (only for --transport http)
        #[arg(long, default_value = "8001")]
        port: u16,

        /// Additional Host header value to accept on the HTTP transport (repeatable; only
        /// for --transport http). Extends, but never replaces, rmcp's loopback-only
        /// allowlist (localhost, 127.0.0.1, ::1) — needed when running behind a reverse
        /// proxy or ingress that forwards a different hostname. Precedence: this flag >
        /// XBERG_MCP_ALLOWED_HOSTS env var (comma-separated) > `[mcp] allowed_hosts` in the
        /// config file (only when --config is given explicitly) > default (loopback only).
        #[arg(long = "allowed-host")]
        allowed_host: Vec<String>,
    },

    /// API utilities
    #[cfg(feature = "api")]
    Api {
        #[command(subcommand)]
        command: ApiCommands,
    },

    /// Generate embeddings for text
    ///
    /// Generates vector embeddings for one or more text inputs using a specified preset model
    /// or an LLM provider. Reads from --text flag or stdin if no text is provided.
    #[cfg(feature = "embeddings")]
    Embed {
        /// Text to embed. Can be specified multiple times for batch embedding.
        #[arg(long)]
        text: Vec<String>,

        /// Embedding preset (fast, balanced, quality, multilingual). Used with --provider local.
        #[arg(long, default_value = "balanced")]
        preset: String,

        /// Embedding provider: "local" (default, ONNX), "llm" (liter-llm), or "plugin" (registered in-process backend)
        #[arg(long, default_value = "local")]
        provider: String,

        /// LLM model for provider-hosted embeddings (e.g., "openai/text-embedding-3-small").
        /// Required when --provider is "llm".
        #[arg(long)]
        model: Option<String>,

        /// API key for the LLM provider
        #[arg(long)]
        api_key: Option<String>,

        /// Name of a pre-registered in-process embedding backend.
        /// Required when --provider is "plugin". The backend must have been
        /// registered via `xberg::plugins::register_embedding_backend`
        /// before this command runs.
        #[arg(long)]
        plugin: Option<String>,

        /// Output format (text or json)
        #[arg(short, long, default_value = "json")]
        format: WireFormat,
    },

    /// Chunk text for processing
    ///
    /// Splits text into chunks using configurable size and overlap.
    /// Reads from --text flag or stdin if no text is provided.
    #[cfg(feature = "core-cli")]
    Chunk {
        /// Text to chunk. If not provided, reads from stdin.
        #[arg(long)]
        text: Option<String>,

        /// Path to config file (TOML, YAML, or JSON)
        #[arg(short, long)]
        config: Option<PathBuf>,

        /// Chunk size in characters
        #[arg(long)]
        chunk_size: Option<usize>,

        /// Chunk overlap in characters
        #[arg(long)]
        chunk_overlap: Option<usize>,

        /// Chunker type: text, markdown, yaml, or semantic
        #[arg(long, default_value = "text")]
        chunker_type: String,

        /// Tokenizer model for token-based chunk sizing (e.g., "Xenova/gpt-4o").
        /// Requires the chunking-tokenizers feature.
        #[arg(long)]
        chunking_tokenizer: Option<String>,

        /// Topic threshold for semantic chunking (0.0-1.0, default: 0.75)
        #[arg(long)]
        topic_threshold: Option<f32>,

        /// Output format (text or json)
        #[arg(short, long, default_value = "json")]
        format: WireFormat,
    },

    /// Generate shell completions
    ///
    /// Outputs shell completion scripts for the specified shell.
    /// Install with: eval "$(xberg completions bash)"
    Completions {
        /// Shell to generate completions for
        #[arg(value_enum)]
        shell: clap_complete::Shell,
    },
}

#[cfg(feature = "api")]
#[derive(Subcommand)]
enum ApiCommands {
    /// Output the OpenAPI schema (JSON)
    ///
    /// Prints the full OpenAPI 3.1 specification for the xberg REST API.
    /// Useful for code generation, documentation, and API client tooling.
    Schema,
}

#[derive(Subcommand)]
enum CacheCommands {
    /// Show cache statistics
    Stats {
        /// Cache directory (default: .xberg in current directory)
        #[arg(short, long)]
        cache_dir: Option<PathBuf>,

        /// Output format (text or json)
        #[arg(short, long, default_value = "text")]
        format: WireFormat,
    },

    /// Clear the Xberg-managed cache (does not clear shared Hugging Face files)
    Clear {
        /// Cache directory (default: .xberg in current directory)
        #[arg(short, long)]
        cache_dir: Option<PathBuf>,

        /// Output format (text or json)
        #[arg(short, long, default_value = "text")]
        format: WireFormat,
    },

    /// Output model manifest (expected model files, checksums, sizes)
    ///
    /// Outputs a JSON manifest of all model files required by xberg,
    /// including their relative paths, SHA256 checksums, and sizes.
    /// Used for pre-populating model caches in containerized deployments.
    Manifest {
        /// Output format (text or json)
        #[arg(short, long, default_value = "json")]
        format: WireFormat,
    },

    /// Download model artifacts eagerly
    ///
    /// Downloads model artifacts for offline/container use. Unlike normal
    /// operation which downloads lazily on first use, this ensures selected
    /// models are available for offline use. Hugging Face models remain in the
    /// standard HF cache; Xberg does not copy them into its own cache.
    ///
    /// Use --all-embeddings to also download all 4 embedding model presets,
    /// or `--embedding-model <preset>` to download a specific one.
    ///
    /// By default, only the core layout models (rtdetr + tatr) are downloaded.
    /// Use --all-table-models to also download SLANeXT variants (~730MB).
    ///
    /// Use --ner to download the default GLiNER NER model, --ner-model <MODEL>
    /// for a specific GLiNER alias/catalog id, or --all-ner-models for every
    /// known GLiNER NER model.
    #[cfg(any(
        feature = "embeddings",
        feature = "layout-detection",
        feature = "paddle-ocr",
        feature = "tree-sitter",
        feature = "ner-onnx"
    ))]
    Warm {
        /// Xberg cache directory; for HF models, an explicit HF cache root
        ///
        /// Without this option, HF models follow HF_HUB_CACHE, HF_HOME, and
        /// platform defaults instead of the Xberg cache directory.
        #[arg(short, long)]
        cache_dir: Option<PathBuf>,

        /// Output format (text or json)
        #[arg(short, long, default_value = "text")]
        format: WireFormat,

        /// Download all embedding model presets (fast, balanced, quality, multilingual)
        #[cfg(feature = "embeddings")]
        #[arg(long)]
        all_embeddings: bool,

        /// Download a specific embedding model preset
        #[cfg(feature = "embeddings")]
        #[arg(long, value_name = "PRESET")]
        embedding_model: Option<String>,

        /// Download all table structure models including SLANeXT variants (~730MB)
        #[cfg(feature = "layout-detection")]
        #[arg(
            long,
            help = "Download all table structure models including SLANeXT variants (~730MB)"
        )]
        all_table_models: bool,

        /// Download all tree-sitter grammar parsers
        #[cfg(feature = "tree-sitter")]
        #[arg(long)]
        all_grammars: bool,

        /// Download specific tree-sitter grammar groups (comma-separated: web,systems,scripting,data,jvm,functional)
        #[cfg(feature = "tree-sitter")]
        #[arg(long, value_name = "GROUPS", value_delimiter = ',')]
        grammar_groups: Option<Vec<String>>,

        /// Download specific tree-sitter grammars by language name (comma-separated)
        #[cfg(feature = "tree-sitter")]
        #[arg(long, value_name = "LANGUAGES", value_delimiter = ',')]
        grammars: Option<Vec<String>>,

        /// Download the default xberg GLiNER NER model alias
        #[cfg(feature = "ner-onnx")]
        #[arg(long)]
        ner: bool,

        /// Download a specific xberg GLiNER NER model alias or catalog id
        #[cfg(feature = "ner-onnx")]
        #[arg(long, value_name = "MODEL")]
        ner_model: Option<String>,

        /// Download every GLiNER NER model variant xberg knows about
        #[cfg(feature = "ner-onnx")]
        #[arg(long)]
        all_ner_models: bool,
    },
}

#[cfg(feature = "tree-sitter")]
#[derive(Subcommand)]
enum TreeSitterCommands {
    /// Download tree-sitter grammar parsers
    ///
    /// Downloads specific languages by name, all available languages (--all),
    /// language groups (--groups), or resolves cache_dir/languages/groups from
    /// the auto-discovered xberg config's [tree_sitter] section (--from-config).
    Download {
        /// Language names to download (e.g., python rust go)
        languages: Vec<String>,

        /// Download all available languages
        #[arg(long)]
        all: bool,

        /// Download specific language groups (comma-separated: web,systems,scripting,data,jvm,functional)
        #[arg(long, value_name = "GROUPS", value_delimiter = ',')]
        groups: Option<Vec<String>>,

        /// Grammar cache directory. CLI arg overrides the config file's tree_sitter.cache_dir.
        #[arg(long)]
        cache_dir: Option<PathBuf>,

        /// Resolve cache_dir/languages/groups from the auto-discovered xberg config's
        /// [tree_sitter] section. Explicit CLI args (--cache-dir, languages, --groups) still win.
        #[arg(long)]
        from_config: bool,

        /// Output format (text or json)
        #[arg(short, long, default_value = "text")]
        format: WireFormat,
    },

    /// List available or downloaded tree-sitter languages
    List {
        /// Only list already-downloaded languages
        #[arg(long)]
        downloaded: bool,

        /// Filter languages by name substring
        #[arg(long)]
        filter: Option<String>,

        /// Output format (text or json)
        #[arg(short, long, default_value = "text")]
        format: WireFormat,
    },

    /// Show the effective tree-sitter grammar cache directory
    CacheDir {
        /// Output format (text or json)
        #[arg(short, long, default_value = "text")]
        format: WireFormat,
    },

    /// Clear all cached tree-sitter grammar parser shared libraries
    Clean {
        /// Output format (text or json)
        #[arg(short, long, default_value = "text")]
        format: WireFormat,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WireFormat {
    Text,
    Json,
    Toon,
}

impl std::str::FromStr for WireFormat {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "text" => Ok(WireFormat::Text),
            "json" => Ok(WireFormat::Json),
            "toon" => Ok(WireFormat::Toon),
            _ => Err(format!("Invalid format: {}. Use 'text', 'json', or 'toon'", s)),
        }
    }
}

/// Content output format for extraction results.
///
/// Controls the format of the extracted content (not the CLI output format).
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
enum ContentOutputFormatArg {
    /// Plain text (default)
    Plain,
    /// Markdown format
    Markdown,
    /// Djot markup format
    Djot,
    /// HTML format
    Html,
    /// JSON tree format with heading-driven sections
    Json,
}

impl From<ContentOutputFormatArg> for ContentOutputFormat {
    fn from(arg: ContentOutputFormatArg) -> Self {
        match arg {
            ContentOutputFormatArg::Plain => ContentOutputFormat::Plain,
            ContentOutputFormatArg::Markdown => ContentOutputFormat::Markdown,
            ContentOutputFormatArg::Djot => ContentOutputFormat::Djot,
            ContentOutputFormatArg::Html => ContentOutputFormat::Html,
            ContentOutputFormatArg::Json => ContentOutputFormat::Json,
        }
    }
}

#[expect(
    clippy::print_stdout,
    reason = "detect/formats/version/api-schema results are the CLI's stdout output contract"
)]
fn main() -> Result<()> {
    // Captured as early as feasible for the optional per-stage cold-start timing breakdown (see
    // `commands::extract::stage_timing_requested`). Gated on the env var so the timing path is
    // fully zero-cost (no `Instant::now()` call, no state) when stage timing isn't requested. ~keep
    let process_start = commands::extract::stage_timing_requested().then(Instant::now);

    let cli = Cli::parse();

    let env_filter = logging::build_env_filter(cli.log_level.as_deref());

    let _ = tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .with_writer(std::io::stderr)
        .try_init();

    match cli.command {
        Commands::Extract {
            uri,
            #[cfg(feature = "url-surface")]
            url,
            stdin,
            config: config_path,
            no_config_discovery,
            config_json,
            config_json_base64,
            mime_type,
            format,
            output_dir,
            overrides,
        } => {
            let input = resolve_extract_input(
                uri,
                #[cfg(feature = "url-surface")]
                url,
                stdin,
            )?;
            validate_extract_input(&input)?;
            if let Some(ref dir) = output_dir {
                validate_output_dir(dir)?;
            }
            overrides.validate()?;

            let mut config = load_config(config_path, !no_config_discovery)?;
            apply_json_overrides(&mut config, config_json, config_json_base64)?;
            overrides.apply(&mut config);

            extract_command(input, config, mime_type, format, output_dir, process_start)?;
        }

        Commands::Batch {
            paths,
            input,
            input_format,
            config: config_path,
            no_config_discovery,
            config_json,
            config_json_base64,
            format,
            output_dir,
            overrides,
            file_configs,
        } => {
            let input_uris = resolve_batch_inputs(paths, input, input_format)?;
            validate_batch_input_uris(&input_uris)?;
            if let Some(ref dir) = output_dir {
                validate_output_dir(dir)?;
            }
            overrides.validate()?;

            let mut config = load_config(config_path, !no_config_discovery)?;
            apply_json_overrides(&mut config, config_json, config_json_base64)?;
            overrides.apply(&mut config);

            let file_configs_map = if let Some(file_configs_path) = file_configs {
                let file_configs_json = std::fs::read_to_string(&file_configs_path)
                    .with_context(|| format!("Failed to read file configs from '{}'", file_configs_path.display()))?;
                let map: std::collections::HashMap<String, serde_json::Value> =
                    serde_json::from_str(&file_configs_json).with_context(|| {
                        format!(
                            "Failed to parse file configs JSON from '{}'",
                            file_configs_path.display()
                        )
                    })?;
                Some(map)
            } else {
                None
            };
            batch_command(input_uris, file_configs_map, config, format, output_dir)?;
        }

        Commands::Detect { path, format } => {
            validate_file_exists(&path)?;

            let path_str = path.to_string_lossy().to_string();
            let mime_type = detect_mime_type(path_str.clone(), true).with_context(|| {
                format!(
                    "Failed to detect MIME type for file '{}'. Ensure the file is readable.",
                    path.display()
                )
            })?;

            match format {
                WireFormat::Text => {
                    println!("{}", style::success(&mime_type));
                }
                WireFormat::Json => {
                    let output = json!({
                        "path": path_str,
                        "mime_type": mime_type,
                    });
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&output)
                            .context("Failed to serialize MIME type detection result to JSON")?
                    );
                }
                WireFormat::Toon => {
                    let output = json!({
                        "path": path_str,
                        "mime_type": mime_type,
                    });
                    println!(
                        "{}",
                        serde_toon::to_string(&output)
                            .context("Failed to serialize MIME type detection result to TOON")?
                    );
                }
            }
        }

        Commands::Formats { format } => {
            // Resolved against the extractor registry rather than the core's static catalogue,
            // so this reports what the binary can actually extract. See `commands::formats`.
            let formats = compiled_in_formats()?;
            match format {
                WireFormat::Text => {
                    println!("{:<15} {}", style::label("EXTENSION"), style::label("MIME TYPE"));
                    println!("{}", style::dim(&format!("{:<15} ---------", "---------")));
                    for f in &formats {
                        println!("{:<15} {}", style::success(&format!(".{}", f.extension)), f.mime_type);
                    }
                }
                WireFormat::Json => {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&formats).context("Failed to serialize formats to JSON")?
                    );
                }
                WireFormat::Toon => {
                    println!(
                        "{}",
                        serde_toon::to_string(&formats).context("Failed to serialize formats to TOON")?
                    );
                }
            }
        }

        Commands::Version { format } => {
            let version = env!("CARGO_PKG_VERSION");
            let name = env!("CARGO_PKG_NAME");

            match format {
                WireFormat::Text => {
                    println!("{} {}", style::label(name), style::success(version));
                }
                WireFormat::Json => {
                    let output = json!({
                        "name": name,
                        "version": version,
                    });
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&output)
                            .context("Failed to serialize version information to JSON")?
                    );
                }
                WireFormat::Toon => {
                    let output = json!({
                        "name": name,
                        "version": version,
                    });
                    println!(
                        "{}",
                        serde_toon::to_string(&output).context("Failed to serialize version information to TOON")?
                    );
                }
            }
        }

        #[cfg(feature = "api")]
        Commands::Serve {
            host: cli_host,
            port: cli_port,
            config: config_path,
        } => {
            let mut extraction_config = load_config(config_path.clone(), true)?;
            extraction_config.apply_env_overrides()?;
            serve_command(cli_host, cli_port, extraction_config, config_path)?;
        }

        #[cfg(feature = "mcp")]
        Commands::Mcp {
            config: config_path,
            transport,
            #[cfg(feature = "mcp-http")]
            host,
            #[cfg(feature = "mcp-http")]
            port,
            #[cfg(not(feature = "mcp-http"))]
            host,
            #[cfg(not(feature = "mcp-http"))]
            port,
            allowed_host,
        } => {
            let allowed_hosts = resolve_mcp_allowed_hosts(&allowed_host, config_path.as_deref())?;
            let mut config = load_config(config_path, true)?;
            config.apply_env_overrides()?;
            mcp_command(config, transport, host, port, allowed_hosts)?;
        }

        Commands::Cache { command } => match command {
            CacheCommands::Stats { cache_dir, format } => {
                stats_command(cache_dir, format)?;
            }
            CacheCommands::Clear { cache_dir, format } => {
                clear_command(cache_dir, format)?;
            }
            CacheCommands::Manifest { format } => {
                manifest_command(format)?;
            }
            #[cfg(any(
                feature = "embeddings",
                feature = "layout-detection",
                feature = "paddle-ocr",
                feature = "tree-sitter",
                feature = "ner-onnx"
            ))]
            CacheCommands::Warm {
                cache_dir,
                format,
                #[cfg(feature = "embeddings")]
                all_embeddings,
                #[cfg(feature = "embeddings")]
                embedding_model,
                #[cfg(feature = "layout-detection")]
                all_table_models,
                #[cfg(feature = "tree-sitter")]
                all_grammars,
                #[cfg(feature = "tree-sitter")]
                grammar_groups,
                #[cfg(feature = "tree-sitter")]
                grammars,
                #[cfg(feature = "ner-onnx")]
                ner,
                #[cfg(feature = "ner-onnx")]
                ner_model,
                #[cfg(feature = "ner-onnx")]
                all_ner_models,
            } => {
                warm_command(
                    cache_dir.clone(),
                    format,
                    #[cfg(feature = "embeddings")]
                    all_embeddings,
                    #[cfg(feature = "embeddings")]
                    embedding_model,
                    #[cfg(feature = "layout-detection")]
                    all_table_models,
                    #[cfg(feature = "tree-sitter")]
                    all_grammars,
                    #[cfg(feature = "tree-sitter")]
                    grammar_groups,
                    #[cfg(feature = "tree-sitter")]
                    grammars,
                    #[cfg(feature = "ner-onnx")]
                    ner,
                    #[cfg(feature = "ner-onnx")]
                    ner_model,
                    #[cfg(feature = "ner-onnx")]
                    all_ner_models,
                )?;
            }
        },

        #[cfg(feature = "tree-sitter")]
        Commands::TreeSitter { command } => match command {
            TreeSitterCommands::Download {
                languages,
                all,
                groups,
                cache_dir,
                from_config,
                format,
            } => {
                download_command(languages, all, groups, cache_dir, from_config, format)?;
            }
            TreeSitterCommands::List {
                downloaded,
                filter,
                format,
            } => {
                list_command(downloaded, filter, format)?;
            }
            TreeSitterCommands::CacheDir { format } => {
                cache_dir_command(format)?;
            }
            TreeSitterCommands::Clean { format } => {
                clean_command(format)?;
            }
        },

        Commands::Doctor {
            config,
            no_config_discovery,
            format,
            clean,
        } => {
            doctor_command(config, no_config_discovery, format, clean)?;
        }

        #[cfg(feature = "api")]
        Commands::Api { command } => match command {
            ApiCommands::Schema => {
                println!("{}", xberg::api::openapi::openapi_json());
            }
        },

        #[cfg(feature = "embeddings")]
        Commands::Embed {
            text,
            preset,
            provider,
            model,
            api_key,
            plugin,
            format,
        } => {
            let texts = if text.is_empty() {
                vec![commands::read_stdin()?]
            } else {
                text
            };
            embed_command(texts, &preset, &provider, model, api_key, plugin, format)?;
        }

        #[cfg(feature = "core-cli")]
        Commands::Chunk {
            text,
            config: config_path,
            chunk_size,
            chunk_overlap,
            chunker_type,
            chunking_tokenizer,
            topic_threshold,
            format,
        } => {
            let input = match text {
                Some(t) => t,
                None => commands::read_stdin().context("No --text provided and failed to read from stdin")?,
            };

            validate_chunk_params(chunk_size, chunk_overlap)?;

            let base_config = load_config(config_path, true)?;
            let mut chunking_config = base_config.chunking.unwrap_or_default();

            if let Some(size) = chunk_size {
                chunking_config.max_characters = size;
                if chunk_overlap.is_none() && chunking_config.overlap >= size {
                    chunking_config.overlap = size / 4;
                }
            }
            if let Some(overlap) = chunk_overlap {
                chunking_config.overlap = overlap;
            }
            match chunker_type.as_str() {
                "markdown" => chunking_config.chunker_type = xberg::ChunkerType::Markdown,
                "yaml" => chunking_config.chunker_type = xberg::ChunkerType::Yaml,
                "semantic" => chunking_config.chunker_type = xberg::ChunkerType::Semantic,
                _ => chunking_config.chunker_type = xberg::ChunkerType::Text,
            }
            #[cfg(feature = "chunking-tokenizers")]
            if let Some(ref tokenizer) = chunking_tokenizer {
                chunking_config.sizing = xberg::ChunkSizing::Tokenizer {
                    model: tokenizer.clone(),
                    cache_dir: None,
                };
            }
            #[cfg(not(feature = "chunking-tokenizers"))]
            if chunking_tokenizer.is_some() {
                anyhow::bail!("--chunking-tokenizer requires the chunking-tokenizers feature");
            }
            if topic_threshold.is_some() {
                chunking_config.topic_threshold = topic_threshold;
            }

            chunk_command(input, chunking_config, format)?;
        }

        Commands::Completions { shell } => {
            let mut cmd = Cli::command();
            clap_complete::generate(shell, &mut cmd, "xberg", &mut std::io::stdout());
        }
    }

    Ok(())
}

#[cfg(test)]
mod feature_profile_tests {
    use super::*;

    fn command_arg_ids(command: &str) -> Vec<String> {
        Cli::command()
            .find_subcommand(command)
            .expect("command should exist")
            .get_arguments()
            .map(|arg| arg.get_id().as_str().to_owned())
            .collect()
    }

    #[cfg(not(feature = "core-cli"))]
    #[test]
    fn lean_profile_omits_chunk_command() {
        assert!(Cli::command().find_subcommand("chunk").is_none());
    }

    #[cfg(feature = "core-cli")]
    #[test]
    fn core_cli_exposes_chunk_command() {
        assert!(Cli::command().find_subcommand("chunk").is_some());
    }

    #[cfg(not(feature = "url-surface"))]
    #[test]
    fn lean_profile_omits_url_flag() {
        assert!(!command_arg_ids("extract").iter().any(|id| id == "url"));
    }

    #[test]
    fn doctor_command_is_always_exposed() {
        let args = command_arg_ids("doctor");
        for required in ["config", "no_config_discovery", "format", "clean"] {
            assert!(args.iter().any(|id| id == required), "missing doctor arg {required}");
        }
    }

    #[cfg(feature = "url-surface")]
    #[test]
    fn url_ingestion_exposes_url_flag() {
        assert!(command_arg_ids("extract").iter().any(|id| id == "url"));
    }

    #[cfg(not(feature = "ocr-surface"))]
    #[test]
    fn lean_profile_omits_ocr_overrides() {
        let args = command_arg_ids("extract");
        for unavailable in ["ocr", "ocr_backend", "force_ocr"] {
            assert!(!args.iter().any(|id| id == unavailable));
        }
    }

    #[cfg(feature = "ocr-surface")]
    #[test]
    fn ocr_capable_profile_exposes_ocr_overrides() {
        let args = command_arg_ids("extract");
        for required in ["ocr", "ocr_backend", "force_ocr"] {
            assert!(args.iter().any(|id| id == required));
        }
    }

    #[cfg(not(feature = "pdf-surface"))]
    #[test]
    fn non_pdf_profile_omits_pdf_overrides() {
        let args = command_arg_ids("extract");
        for unavailable in [
            "pdf_password",
            "pdf_extract_images",
            "pdf_extract_tables",
            "pdf_ocr_inline_images",
            "pdf_extract_metadata",
            "pdf_backend",
        ] {
            assert!(!args.iter().any(|id| id == unavailable));
        }
    }

    #[cfg(feature = "pdf-surface")]
    #[test]
    fn pdf_profile_exposes_pdf_overrides() {
        let args = command_arg_ids("extract");
        for required in [
            "pdf_password",
            "pdf_extract_images",
            "pdf_extract_tables",
            "pdf_extract_metadata",
            "pdf_backend",
        ] {
            assert!(args.iter().any(|id| id == required));
        }
        assert_eq!(
            args.iter().any(|id| id == "pdf_ocr_inline_images"),
            cfg!(feature = "ocr-surface")
        );
    }

    #[cfg(not(feature = "analysis"))]
    #[test]
    fn lean_profile_omits_analysis_overrides() {
        let args = command_arg_ids("extract");
        for unavailable in ["quality", "detect_language", "token_reduction"] {
            assert!(!args.iter().any(|id| id == unavailable));
        }
    }

    #[cfg(not(any(feature = "core-cli", feature = "analysis")))]
    #[test]
    fn lean_profile_omits_chunking_overrides() {
        let args = command_arg_ids("extract");
        for unavailable in ["chunk", "chunk_size", "chunk_overlap", "chunking_tokenizer"] {
            assert!(!args.iter().any(|id| id == unavailable));
        }
    }

    #[cfg(not(any(
        feature = "embeddings",
        feature = "layout-detection",
        feature = "paddle-ocr",
        feature = "tree-sitter",
        feature = "ner-onnx"
    )))]
    #[test]
    fn lean_profile_omits_warm_command() {
        let command = Cli::command();
        let cache = command.find_subcommand("cache").expect("cache command should exist");
        assert!(cache.find_subcommand("warm").is_none());
    }

    #[cfg(any(
        feature = "embeddings",
        feature = "layout-detection",
        feature = "paddle-ocr",
        feature = "tree-sitter",
        feature = "ner-onnx"
    ))]
    #[test]
    fn warm_capable_profile_exposes_warm_command() {
        let command = Cli::command();
        let cache = command.find_subcommand("cache").expect("cache command should exist");
        assert!(cache.find_subcommand("warm").is_some());
    }
}

/// Regression coverage for issue #280: `commands/tree_sitter.rs` implemented
/// `download_command`/`list_command`/`cache_dir_command`/`clean_command` (and the
/// `--from-config` cascade in `resolve_pack_config`), but no `Commands` variant ever
/// invoked them, so the flag was unreachable from the command line. These tests parse
/// argv through clap directly (never shelling out to the built binary) to prove the
/// `tree-sitter` subcommand is registered and its arguments actually reach the enum
/// variants that main() matches on.
#[cfg(all(test, feature = "tree-sitter"))]
mod tree_sitter_cli_tests {
    use super::*;

    /// `Cli::command().debug_assert()` walks the entire clap command graph (including
    /// the newly-added `tree-sitter` subcommand) and panics on any structural error
    /// (duplicate ids, conflicting arg configuration, etc.). This is clap's own
    /// self-check and is the cheapest possible proof that `TreeSitterCommands` is
    /// wired into `Commands` without a definition error.
    #[test]
    fn cli_command_graph_is_structurally_valid() {
        Cli::command().debug_assert();
    }

    /// The regression this whole task exists to prevent: `tree_sitter.rs` was never
    /// declared in `commands/mod.rs`, and even after that fix, no subcommand invoked
    /// it, so `xberg tree-sitter --help` would have failed with clap's "unrecognized
    /// subcommand" error. Asserting the subcommand (and each of its four children) is
    /// discoverable via `find_subcommand` is the check that would have caught the
    /// original bug.
    #[test]
    fn tree_sitter_subcommand_and_its_children_are_registered() {
        let command = Cli::command();
        let tree_sitter = command
            .find_subcommand("tree-sitter")
            .expect("tree-sitter subcommand should be registered");
        for child in ["download", "list", "cache-dir", "clean"] {
            assert!(
                tree_sitter.find_subcommand(child).is_some(),
                "tree-sitter subcommand missing child: {child}"
            );
        }
    }

    /// `xberg tree-sitter download --from-config` must parse `from_config` as `true`
    /// while leaving CLI overrides such as `--cache-dir` intact alongside it, proving
    /// the parsed args are the exact values `Commands::TreeSitter`'s match arm forwards
    /// into `download_command` (which threads them into `resolve_pack_config`).
    #[test]
    fn should_parse_download_from_config_flag_with_explicit_cache_dir_override() {
        let cli = Cli::try_parse_from([
            "xberg",
            "tree-sitter",
            "download",
            "--from-config",
            "--cache-dir",
            "/tmp/xberg-grammars",
        ])
        .expect("clap should parse tree-sitter download --from-config --cache-dir");

        let Commands::TreeSitter { command } = cli.command else {
            panic!("expected Commands::TreeSitter");
        };
        let TreeSitterCommands::Download {
            languages,
            all,
            groups,
            cache_dir,
            from_config,
            format,
        } = command
        else {
            panic!("expected TreeSitterCommands::Download");
        };

        assert!(from_config);
        assert_eq!(cache_dir, Some(PathBuf::from("/tmp/xberg-grammars")));
        assert!(languages.is_empty());
        assert!(!all);
        assert_eq!(groups, None);
        assert_eq!(format, WireFormat::Text);
    }

    /// `xberg tree-sitter download go zig --groups web,systems` must parse the
    /// positional language names and the comma-delimited `--groups` list exactly,
    /// with `from_config` defaulting to `false` when the flag is absent.
    #[test]
    fn should_parse_download_languages_and_groups_without_from_config() {
        let cli = Cli::try_parse_from([
            "xberg",
            "tree-sitter",
            "download",
            "go",
            "zig",
            "--groups",
            "web,systems",
        ])
        .expect("clap should parse tree-sitter download with languages and --groups");

        let Commands::TreeSitter { command } = cli.command else {
            panic!("expected Commands::TreeSitter");
        };
        let TreeSitterCommands::Download {
            languages,
            groups,
            from_config,
            ..
        } = command
        else {
            panic!("expected TreeSitterCommands::Download");
        };

        assert_eq!(languages, vec!["go".to_string(), "zig".to_string()]);
        assert_eq!(groups, Some(vec!["web".to_string(), "systems".to_string()]));
        assert!(!from_config);
    }

    /// `xberg tree-sitter list --downloaded --filter py --format json` must parse
    /// exactly into `TreeSitterCommands::List`'s three fields.
    #[test]
    fn should_parse_list_subcommand_with_downloaded_and_filter() {
        let cli = Cli::try_parse_from([
            "xberg",
            "tree-sitter",
            "list",
            "--downloaded",
            "--filter",
            "py",
            "--format",
            "json",
        ])
        .expect("clap should parse tree-sitter list --downloaded --filter py --format json");

        let Commands::TreeSitter { command } = cli.command else {
            panic!("expected Commands::TreeSitter");
        };
        let TreeSitterCommands::List {
            downloaded,
            filter,
            format,
        } = command
        else {
            panic!("expected TreeSitterCommands::List");
        };

        assert!(downloaded);
        assert_eq!(filter, Some("py".to_string()));
        assert_eq!(format, WireFormat::Json);
    }

    /// `xberg tree-sitter cache-dir` must parse into `TreeSitterCommands::CacheDir`
    /// with the default text format.
    #[test]
    fn should_parse_cache_dir_subcommand() {
        let cli = Cli::try_parse_from(["xberg", "tree-sitter", "cache-dir"])
            .expect("clap should parse tree-sitter cache-dir");

        let Commands::TreeSitter { command } = cli.command else {
            panic!("expected Commands::TreeSitter");
        };
        assert!(matches!(
            command,
            TreeSitterCommands::CacheDir {
                format: WireFormat::Text
            }
        ));
    }

    /// `xberg tree-sitter clean --format toon` must parse into
    /// `TreeSitterCommands::Clean` carrying the requested format.
    #[test]
    fn should_parse_clean_subcommand_with_format_override() {
        let cli = Cli::try_parse_from(["xberg", "tree-sitter", "clean", "--format", "toon"])
            .expect("clap should parse tree-sitter clean --format toon");

        let Commands::TreeSitter { command } = cli.command else {
            panic!("expected Commands::TreeSitter");
        };
        assert!(matches!(
            command,
            TreeSitterCommands::Clean {
                format: WireFormat::Toon
            }
        ));
    }
}
