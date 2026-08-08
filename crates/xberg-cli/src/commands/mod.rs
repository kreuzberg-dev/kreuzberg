//! Command modules for Xberg CLI
//!
//! This module organizes the CLI commands into focused submodules:
//! - `extract` - Document extraction commands
//! - `cache` - Cache management operations
//! - `server` - API and MCP server commands
//! - `config` - Configuration loading and discovery
//! - `embed` - Embedding generation commands
//! - `chunk` - Text chunking commands

use anyhow::{Context, Result};
#[cfg(any(feature = "core-cli", feature = "embeddings"))]
use std::io::Read;

pub mod cache;
#[cfg(feature = "core-cli")]
pub mod chunk;
pub mod config;
pub mod doctor;
#[cfg(feature = "embeddings")]
pub mod embed;
pub mod extract;
pub mod formats;
#[cfg(feature = "ner-onnx")]
pub mod ner;
pub mod overrides;
#[cfg(any(feature = "api", feature = "mcp"))]
pub mod server;
#[cfg(feature = "tree-sitter")]
pub mod tree_sitter;

#[cfg(any(
    feature = "embeddings",
    feature = "layout-detection",
    feature = "paddle-ocr",
    feature = "tree-sitter",
    feature = "ner-onnx"
))]
pub use cache::warm_command;
pub use cache::{clear_command, manifest_command, stats_command};
#[cfg(feature = "core-cli")]
pub use chunk::chunk_command;
pub use config::load_config;
pub use doctor::doctor_command;
#[cfg(feature = "embeddings")]
pub use embed::embed_command;
pub use extract::{
    BatchInputFormat, ExtractInputSource, batch_command, extract_command, load_batch_input_manifest, uri_to_local_path,
};
pub use formats::compiled_in_formats;
#[cfg(feature = "api")]
pub use server::serve_command;
#[cfg(feature = "mcp")]
pub use server::{mcp_command, resolve_mcp_allowed_hosts};
#[cfg(feature = "tree-sitter")]
pub use tree_sitter::{cache_dir_command, clean_command, download_command, list_command};

/// Validates that a directory exists and is accessible.
///
/// # Errors
///
/// Returns an error if the path does not exist or is not a directory.
pub fn validate_output_dir(path: &std::path::Path) -> Result<()> {
    if !path.exists() {
        anyhow::bail!(
            "Output directory not found: '{}'. Create the directory before running.",
            path.display()
        );
    }
    if !path.is_dir() {
        anyhow::bail!("Output path is not a directory: '{}'.", path.display());
    }
    Ok(())
}

/// Validates that a file exists and is accessible.
pub fn validate_file_exists(path: &std::path::Path) -> Result<()> {
    if !path.exists() {
        anyhow::bail!(
            "File not found: '{}'. Please check that the file exists and is accessible.",
            path.display()
        );
    }
    if !path.is_file() {
        anyhow::bail!(
            "Path is not a file: '{}'. Please provide a path to a regular file.",
            path.display()
        );
    }
    Ok(())
}

/// Validates chunking parameters for correctness.
#[cfg(feature = "core-cli")]
pub fn validate_chunk_params(chunk_size: Option<usize>, chunk_overlap: Option<usize>) -> Result<()> {
    if let Some(size) = chunk_size {
        if size == 0 {
            anyhow::bail!("Invalid chunk size: {}. Chunk size must be greater than 0.", size);
        }
        if size > 1_000_000 {
            anyhow::bail!(
                "Invalid chunk size: {}. Chunk size must be less than 1,000,000 characters to avoid excessive memory usage.",
                size
            );
        }
    }
    if let Some(overlap) = chunk_overlap
        && let Some(size) = chunk_size
        && overlap >= size
    {
        anyhow::bail!(
            "Invalid chunk overlap: {}. Overlap ({}) must be less than chunk size ({}).",
            overlap,
            overlap,
            size
        );
    }
    Ok(())
}

/// Validates batch extraction paths for correctness.
pub fn validate_batch_paths(paths: &[std::path::PathBuf]) -> Result<()> {
    if paths.is_empty() {
        anyhow::bail!("No files provided for batch extraction. Please provide at least one file path.");
    }
    for (i, path) in paths.iter().enumerate() {
        validate_file_exists(path).with_context(|| format!("Invalid file at position {}", i + 1))?;
    }
    Ok(())
}

/// Read text from stdin, trimming whitespace.
#[cfg(any(feature = "core-cli", feature = "embeddings"))]
pub fn read_stdin() -> Result<String> {
    let mut input = String::new();
    std::io::stdin()
        .read_to_string(&mut input)
        .context("Failed to read from stdin")?;
    let trimmed = input.trim().to_string();
    if trimmed.is_empty() {
        anyhow::bail!("No input received from stdin. Provide text via --text or pipe it to stdin.");
    }
    Ok(trimmed)
}
