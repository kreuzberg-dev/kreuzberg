//! Cache command - Manage cache operations
//!
//! This module provides commands for cache management including statistics,
//! clearing, manifest generation, and model warming.

use anyhow::{Context, Result};
use serde_json::json;
use std::path::PathBuf;
use xberg::cache;

use crate::{WireFormat, style};

#[cfg(any(feature = "paddle-ocr", feature = "layout-detection", feature = "ner-onnx", feature = "formula-recognition"))]
#[derive(Debug, Clone, serde::Serialize)]
struct CacheManifestEntry {
    relative_path: String,
    sha256: String,
    size_bytes: u64,
    source_url: String,
}

#[cfg(any(feature = "paddle-ocr", feature = "layout-detection", feature = "ner-onnx", feature = "formula-recognition"))]
impl CacheManifestEntry {
    fn new(relative_path: String, sha256: String, size_bytes: u64, source_url: String) -> Self {
        Self {
            relative_path,
            sha256,
            size_bytes,
            source_url,
        }
    }
}

/// Execute cache stats command
#[expect(
    clippy::print_stdout,
    reason = "cache statistics are the command's stdout result output"
)]
pub fn stats_command(cache_dir: Option<PathBuf>, format: WireFormat) -> Result<()> {
    let default_cache_dir = std::env::current_dir()
        .context("Failed to get current directory")?
        .join(".xberg");

    let cache_path = cache_dir.unwrap_or(default_cache_dir);
    let cache_dir_str = cache_path.to_string_lossy();

    let stats = cache::get_cache_metadata(&cache_dir_str).with_context(|| {
        format!(
            "Failed to get cache statistics from directory '{}'. Ensure the directory exists and is readable.",
            cache_dir_str
        )
    })?;

    match format {
        WireFormat::Text => {
            println!("{}", style::header("Cache Statistics"));
            println!("{}", style::dim("================"));
            println!("{} {}", style::label("Directory:"), style::success(&cache_dir_str));
            println!("{} {}", style::label("Total files:"), stats.total_files);
            println!("{} {:.2} MB", style::label("Total size:"), stats.total_size_mb);
            println!(
                "{} {:.2} MB",
                style::label("Available space:"),
                stats.available_space_mb
            );
            println!(
                "{} {:.2} days",
                style::label("Oldest file age:"),
                stats.oldest_file_age_days
            );
            println!(
                "{} {:.2} days",
                style::label("Newest file age:"),
                stats.newest_file_age_days
            );
        }
        WireFormat::Json => {
            let output = json!({
                "directory": cache_dir_str,
                "total_files": stats.total_files,
                "total_size_mb": stats.total_size_mb,
                "available_space_mb": stats.available_space_mb,
                "oldest_file_age_days": stats.oldest_file_age_days,
                "newest_file_age_days": stats.newest_file_age_days,
            });
            println!(
                "{}",
                serde_json::to_string_pretty(&output).context("Failed to serialize cache statistics to JSON")?
            );
        }
        WireFormat::Toon => {
            let output = json!({
                "directory": cache_dir_str,
                "total_files": stats.total_files,
                "total_size_mb": stats.total_size_mb,
                "available_space_mb": stats.available_space_mb,
                "oldest_file_age_days": stats.oldest_file_age_days,
                "newest_file_age_days": stats.newest_file_age_days,
            });
            println!(
                "{}",
                serde_toon::to_string(&output).context("Failed to serialize cache statistics to TOON")?
            );
        }
    }

    Ok(())
}

/// Clear the Xberg-managed cache. Shared Hugging Face cache files are excluded.
#[expect(
    clippy::print_stdout,
    reason = "cache clear summary is the command's stdout result output"
)]
pub fn clear_command(cache_dir: Option<PathBuf>, format: WireFormat) -> Result<()> {
    let default_cache_dir = std::env::current_dir()
        .context("Failed to get current directory")?
        .join(".xberg");

    let cache_path = cache_dir.unwrap_or(default_cache_dir);
    let cache_dir_str = cache_path.to_string_lossy();

    let (removed_files, freed_mb) = cache::clear_cache_directory(&cache_dir_str).with_context(|| {
        format!(
            "Failed to clear cache directory '{}'. Ensure you have write permissions.",
            cache_dir_str
        )
    })?;

    match format {
        WireFormat::Text => {
            println!("{}", style::success("Xberg-managed cache cleared successfully"));
            println!("Shared Hugging Face Hub cache files were not removed.");
            println!("{} {}", style::label("Directory:"), style::success(&cache_dir_str));
            println!("{} {}", style::label("Removed files:"), removed_files);
            println!("{} {:.2} MB", style::label("Freed space:"), freed_mb);
        }
        WireFormat::Json => {
            let output = json!({
                "directory": cache_dir_str,
                "removed_files": removed_files,
                "freed_mb": freed_mb,
                "hugging_face_cache_cleared": false,
            });
            println!(
                "{}",
                serde_json::to_string_pretty(&output).context("Failed to serialize cache clear results to JSON")?
            );
        }
        WireFormat::Toon => {
            let output = json!({
                "directory": cache_dir_str,
                "removed_files": removed_files,
                "freed_mb": freed_mb,
                "hugging_face_cache_cleared": false,
            });
            println!(
                "{}",
                serde_toon::to_string(&output).context("Failed to serialize cache clear results to TOON")?
            );
        }
    }

    Ok(())
}

/// Execute cache manifest command - outputs expected model files with checksums.
pub fn manifest_command(format: WireFormat) -> Result<()> {
    // below is `#[cfg]`-stripped and `entries: Vec<_>` has no anchor for
    #[cfg(not(any(feature = "paddle-ocr", feature = "layout-detection", feature = "ner-onnx", feature = "formula-recognition")))]
    {
        let _ = format;
        anyhow::bail!(
            "manifest command unavailable: build xberg-cli with at least one of \
             --features \"paddle-ocr\", \"layout-detection\", or \"ner-onnx\""
        );
    }

    #[cfg(any(feature = "paddle-ocr", feature = "layout-detection", feature = "ner-onnx", feature = "formula-recognition"))]
    {
        manifest_command_inner(format)
    }
}

#[cfg(any(feature = "paddle-ocr", feature = "layout-detection", feature = "ner-onnx", feature = "formula-recognition"))]
#[expect(
    clippy::print_stdout,
    reason = "model manifest is the command's stdout result output"
)]
fn manifest_command_inner(format: WireFormat) -> Result<()> {
    let mut entries: Vec<CacheManifestEntry> = Vec::new();

    #[cfg(feature = "paddle-ocr")]
    {
        entries.extend(xberg::paddle_ocr::ModelManager::manifest().into_iter().map(|entry| {
            CacheManifestEntry::new(entry.relative_path, entry.sha256, entry.size_bytes, entry.source_url)
        }));
    }

    #[cfg(feature = "layout-detection")]
    {
        entries.extend(xberg::layout::LayoutModelManager::manifest().into_iter().map(|entry| {
            CacheManifestEntry::new(entry.relative_path, entry.sha256, entry.size_bytes, entry.source_url)
        }));
    }

    #[cfg(feature = "formula-recognition")]
    {
        entries.extend(xberg::formula_recognition::manifest().into_iter().map(|entry| {
            CacheManifestEntry::new(entry.relative_path, entry.sha256, entry.size_bytes, entry.source_url)
        }));
    }

    #[cfg(feature = "paddle-ocr")]
    {
        entries.extend(xberg::ocr::TessdataManager::manifest().into_iter().map(|entry| {
            CacheManifestEntry::new(entry.relative_path, entry.sha256, entry.size_bytes, entry.source_url)
        }));
    }

    #[cfg(feature = "ner-onnx")]
    {
        entries.extend(xberg::text::ner::manifest().into_iter().map(|entry| {
            CacheManifestEntry::new(entry.relative_path, entry.sha256, entry.size_bytes, entry.source_url)
        }));
    }

    let total_size_bytes: u64 = entries.iter().map(|e| e.size_bytes).sum();
    let version = env!("CARGO_PKG_VERSION");

    match format {
        WireFormat::Text => {
            println!(
                "{} {}",
                style::header("Model Manifest"),
                style::dim(&format!("(xberg {})", version))
            );
            println!("{}", style::dim("===================================="));
            println!(
                "{:<50} {:>12} {}",
                style::label("PATH"),
                style::label("SIZE"),
                style::label("SHA256")
            );
            println!("{}", style::dim(&format!("{:<50} {:>12} ------", "----", "----")));
            for entry in &entries {
                let size_str = if entry.size_bytes > 0 {
                    format!("{:.1} MB", entry.size_bytes as f64 / 1_048_576.0)
                } else {
                    "unknown".to_string()
                };
                let sha_display = if entry.sha256.len() >= 12 {
                    &entry.sha256[..12]
                } else if entry.sha256.is_empty() {
                    "-"
                } else {
                    &entry.sha256
                };
                println!(
                    "{:<50} {:>12} {}",
                    entry.relative_path,
                    size_str,
                    style::dim(sha_display)
                );
            }
            println!();
            println!(
                "{} {} files, {:.1} MB",
                style::label("Total:"),
                entries.len(),
                total_size_bytes as f64 / 1_048_576.0
            );
        }
        WireFormat::Json => {
            let output = json!({
                "xberg_version": version,
                "total_size_bytes": total_size_bytes,
                "model_count": entries.len(),
                "models": entries,
            });
            println!(
                "{}",
                serde_json::to_string_pretty(&output).context("Failed to serialize manifest to JSON")?
            );
        }
        WireFormat::Toon => {
            let output = json!({
                "xberg_version": version,
                "total_size_bytes": total_size_bytes,
                "model_count": entries.len(),
                "models": entries,
            });
            println!(
                "{}",
                serde_toon::to_string(&output).context("Failed to serialize manifest to TOON")?
            );
        }
    }

    Ok(())
}

/// Execute cache warm command - eagerly downloads all models.
#[cfg(any(
    feature = "embeddings",
    feature = "layout-detection",
    feature = "paddle-ocr",
    feature = "tree-sitter",
    feature = "ner-onnx"
))]
#[allow(clippy::too_many_arguments)]
#[expect(
    clippy::print_stdout,
    reason = "cache warm download summary is the command's stdout result output"
)]
pub fn warm_command(
    cache_dir: Option<PathBuf>,
    format: WireFormat,
    #[cfg(feature = "embeddings")] all_embeddings: bool,
    #[cfg(feature = "embeddings")] embedding_model: Option<String>,
    #[cfg(feature = "layout-detection")] all_table_models: bool,
    #[cfg(feature = "tree-sitter")] all_grammars: bool,
    #[cfg(feature = "tree-sitter")] grammar_groups: Option<Vec<String>>,
    #[cfg(feature = "tree-sitter")] grammars: Option<Vec<String>>,
    #[cfg(feature = "ner-onnx")] ner: bool,
    #[cfg(feature = "ner-onnx")] ner_model: Option<String>,
    #[cfg(feature = "ner-onnx")] all_ner_models: bool,
) -> Result<()> {
    #[cfg(any(feature = "embeddings", feature = "ner-onnx", feature = "paddle-ocr"))]
    let hf_cache_dir = cache_dir.clone();
    #[cfg(any(feature = "embeddings", feature = "ner-onnx", feature = "paddle-ocr"))]
    let uses_hf_cache = cfg!(feature = "paddle-ocr")
        || {
            #[cfg(feature = "embeddings")]
            {
                all_embeddings || embedding_model.is_some()
            }
            #[cfg(not(feature = "embeddings"))]
            {
                false
            }
        }
        || {
            #[cfg(feature = "ner-onnx")]
            {
                ner || ner_model.is_some() || all_ner_models
            }
            #[cfg(not(feature = "ner-onnx"))]
            {
                false
            }
        };
    #[cfg(any(feature = "embeddings", feature = "ner-onnx", feature = "paddle-ocr"))]
    let hf_cache_label = uses_hf_cache.then(|| {
        hf_cache_dir
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "HF_HUB_CACHE/HF_HOME/platform default".to_string())
    });
    #[cfg(not(any(feature = "embeddings", feature = "ner-onnx", feature = "paddle-ocr")))]
    let hf_cache_label: Option<String> = None;
    let cache_base = resolve_cache_base(cache_dir);

    let mut downloaded: Vec<String> = Vec::new();
    #[cfg(any(feature = "paddle-ocr", feature = "layout-detection", feature = "tree-sitter", feature = "formula-recognition"))]
    let mut already_cached: Vec<String> = Vec::new();
    #[cfg(not(any(feature = "paddle-ocr", feature = "layout-detection", feature = "tree-sitter", feature = "formula-recognition")))]
    let already_cached: Vec<String> = Vec::new();

    #[cfg(feature = "paddle-ocr")]
    {
        let manager = hf_cache_dir
            .clone()
            .map(xberg::paddle_ocr::ModelManager::new)
            .unwrap_or_default();

        manager
            .ensure_all_models()
            .context("Failed to download PaddleOCR v2 models")?;
        downloaded.push("paddle-ocr v2 (server+mobile det, cls, doc_ori, unified+per-script rec)".to_string());
    }

    #[cfg(feature = "formula-recognition")]
    {
        let formula_dir = cache_base.join("formula-recognition");
        if xberg::formula_recognition::models_cached_in(Some(&formula_dir)) {
            already_cached.push("formula-recognition (latex-ocr)".to_string());
        } else {
            xberg::formula_recognition::ensure_models_in(Some(&formula_dir))
                .map_err(|e| anyhow::anyhow!(e))
                .context("Failed to download formula recognition models")?;
            downloaded.push("formula-recognition (latex-ocr)".to_string());
        }
    }

    #[cfg(feature = "layout-detection")]
    {
        let layout_dir = cache_base.join("layout");
        let manager = xberg::layout::LayoutModelManager::new(Some(layout_dir));

        if all_table_models {
            let was_cached = manager.is_rtdetr_cached() && manager.is_tatr_cached();
            if was_cached {
                already_cached.push("layout (rtdetr, tatr, slanet variants)".to_string());
            } else {
                manager
                    .ensure_all_models()
                    .context("Failed to download layout models")?;
                downloaded.push("layout (rtdetr, tatr, slanet variants)".to_string());
            }
        } else {
            let was_cached = manager.is_rtdetr_cached() && manager.is_tatr_cached();
            if was_cached {
                already_cached.push("layout (rtdetr, tatr)".to_string());
            } else {
                manager
                    .ensure_default_models()
                    .context("Failed to download layout models")?;
                downloaded.push("layout (rtdetr, tatr)".to_string());
            }
        }
    }

    #[cfg(feature = "paddle-ocr")]
    {
        let tessdata_dir = cache_base.join("tessdata");
        let manager = xberg::ocr::TessdataManager::new(Some(tessdata_dir));

        let newly_downloaded = manager
            .ensure_all_languages()
            .context("Failed to download tessdata files")?;

        if newly_downloaded > 0 {
            downloaded.push(format!("tessdata ({newly_downloaded} languages)"));
        } else {
            already_cached.push("tessdata (all languages)".to_string());
        }
    }

    #[cfg(feature = "embeddings")]
    {
        let presets_to_warm: Vec<xberg::EmbeddingPreset> = if all_embeddings {
            xberg::list_embedding_presets()
                .into_iter()
                .filter_map(|name| xberg::get_embedding_preset(&name))
                .collect()
        } else if let Some(ref name) = embedding_model {
            match xberg::get_embedding_preset(name) {
                Some(preset) => vec![preset],
                None => {
                    let available = xberg::list_embedding_presets();
                    anyhow::bail!(
                        "Unknown embedding preset '{}'. Available: {}",
                        name,
                        available.join(", ")
                    );
                }
            }
        } else {
            vec![]
        };

        for preset in &presets_to_warm {
            let label = format!("embedding ({})", preset.name);
            xberg::embeddings::warm_model(
                &xberg::core::config::EmbeddingModelType::Preset {
                    name: preset.name.clone(),
                },
                hf_cache_dir.clone(),
            )
            .map_err(|e| anyhow::anyhow!("Failed to download embedding model '{}': {}", preset.name, e))?;
            downloaded.push(label);
        }
    }

    #[cfg(feature = "tree-sitter")]
    {
        if all_grammars {
            let count =
                tree_sitter_language_pack::download_all().context("Failed to download all tree-sitter grammars")?;
            if count > 0 {
                downloaded.push(format!("tree-sitter grammars ({count} languages)"));
            } else {
                already_cached.push("tree-sitter grammars (all)".to_string());
            }
        } else if let Some(ref groups) = grammar_groups {
            let config = tree_sitter_language_pack::PackConfig {
                cache_dir: None,
                languages: None,
                groups: Some(groups.clone()),
            };
            tree_sitter_language_pack::init(&config).context("Failed to download tree-sitter grammar groups")?;
            downloaded.push(format!("tree-sitter grammars (groups: {})", groups.join(", ")));
        } else if let Some(ref langs) = grammars {
            let refs: Vec<&str> = langs.iter().map(String::as_str).collect();
            let count =
                tree_sitter_language_pack::download(&refs).context("Failed to download tree-sitter grammars")?;
            if count > 0 {
                downloaded.push(format!("tree-sitter grammars ({count} languages)"));
            } else {
                already_cached.push(format!("tree-sitter grammars ({})", langs.join(", ")));
            }
        }
    }

    #[cfg(feature = "ner-onnx")]
    {
        let ner_models: Vec<String> = ner_model.into_iter().collect();
        if ner || !ner_models.is_empty() || all_ner_models {
            let to_download = crate::commands::ner::select_models(ner, ner_models, all_ner_models)?;
            downloaded.extend(
                crate::commands::ner::download_models(&to_download, hf_cache_dir)
                    .context("Failed to download GLiNER NER models")?
                    .into_iter()
                    .map(|entry| format!("ner gliner ({entry})")),
            );
        }
    }

    match format {
        WireFormat::Text => {
            if !downloaded.is_empty() {
                println!("{}", style::label("Downloaded:"));
                for d in &downloaded {
                    println!("  {}", style::success(d));
                }
            }
            if !already_cached.is_empty() {
                println!("{}", style::label("Already cached:"));
                for c in &already_cached {
                    println!("  {}", style::dim(c));
                }
            }
            println!(
                "Xberg-managed cache: {}",
                style::success(&cache_base.display().to_string())
            );
            if let Some(ref hf_cache) = hf_cache_label {
                println!("Hugging Face cache: {}", style::success(hf_cache));
            }
        }
        WireFormat::Json => {
            let output = json!({
                "cache_dir": cache_base.to_string_lossy(),
                "xberg_cache_dir": cache_base.to_string_lossy(),
                "hugging_face_cache_dir": hf_cache_label,
                "downloaded": downloaded,
                "already_cached": already_cached,
            });
            println!(
                "{}",
                serde_json::to_string_pretty(&output).context("Failed to serialize warm results to JSON")?
            );
        }
        WireFormat::Toon => {
            let output = json!({
                "cache_dir": cache_base.to_string_lossy(),
                "xberg_cache_dir": cache_base.to_string_lossy(),
                "hugging_face_cache_dir": hf_cache_label,
                "downloaded": downloaded,
                "already_cached": already_cached,
            });
            println!(
                "{}",
                serde_toon::to_string(&output).context("Failed to serialize warm results to TOON")?
            );
        }
    }

    Ok(())
}

/// Resolve the cache base directory.
#[cfg(any(
    feature = "embeddings",
    feature = "layout-detection",
    feature = "paddle-ocr",
    feature = "tree-sitter",
    feature = "ner-onnx"
))]
fn resolve_cache_base(cache_dir: Option<PathBuf>) -> PathBuf {
    if let Some(dir) = cache_dir {
        return dir;
    }
    if let Ok(env_path) = std::env::var("XBERG_CACHE_DIR") {
        return PathBuf::from(env_path);
    }
    std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join(".xberg")
}
