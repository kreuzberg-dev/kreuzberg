//! Embed command implementation.

use anyhow::{Context, Result};

use crate::{WireFormat, style};

/// Execute the embed command: generate embeddings for input texts.
///
/// When `provider` is `"local"` (default), uses the ONNX preset model.
/// When `provider` is `"llm"`, uses liter-llm with the specified model and API key.
/// When `provider` is `"plugin"`, dispatches to a pre-registered in-process embedding backend.
#[expect(
    clippy::print_stdout,
    reason = "embedding vectors are the command's stdout result output"
)]
pub fn embed_command(
    texts: Vec<String>,
    preset: &str,
    provider: &str,
    llm_model: Option<String>,
    llm_api_key: Option<String>,
    plugin_name: Option<String>,
    format: WireFormat,
) -> Result<()> {
    if texts.is_empty() {
        anyhow::bail!("No texts provided for embedding. Provide --text or pipe text via stdin.");
    }

    for (i, t) in texts.iter().enumerate() {
        if t.is_empty() {
            anyhow::bail!("Text at position {} is empty. All texts must be non-empty.", i + 1);
        }
    }

    let (config, model_label) = match provider {
        "llm" => {
            let model = llm_model.as_deref().ok_or_else(|| {
                anyhow::anyhow!(
                    "--model is required when --provider is 'llm' (e.g., --model openai/text-embedding-3-small)"
                )
            })?;

            let llm_config = xberg::LlmConfig {
                model: model.to_string(),
                api_key: llm_api_key,
                ..Default::default()
            };

            let config = xberg::EmbeddingConfig {
                model: xberg::EmbeddingModelType::Llm {
                    llm: Box::new(llm_config),
                },
                show_download_progress: true,
                ..Default::default()
            };

            (config, model.to_string())
        }
        "local" | "" => {
            let _preset_info = xberg::get_embedding_preset(preset).with_context(|| {
                format!(
                    "Unknown embedding preset '{}'. Available: {:?}",
                    preset,
                    xberg::list_embedding_presets()
                )
            })?;

            let config = xberg::EmbeddingConfig {
                model: xberg::EmbeddingModelType::Preset {
                    name: preset.to_string(),
                },
                show_download_progress: true,
                ..Default::default()
            };

            (config, preset.to_string())
        }
        "plugin" => {
            let name = plugin_name.as_deref().ok_or_else(|| {
                anyhow::anyhow!(
                    "--plugin NAME is required when --provider is 'plugin'. Register a backend via xberg::plugins::register_embedding_backend first."
                )
            })?;
            if name.is_empty() {
                anyhow::bail!("--plugin NAME must not be empty.");
            }

            let available =
                xberg::plugins::list_embedding_backends().context("Failed to read embedding backend registry")?;
            if !available.iter().any(|n| n == name) {
                anyhow::bail!(
                    "Embedding backend '{}' is not registered. Available backends: {}",
                    name,
                    if available.is_empty() {
                        "(none registered)".to_string()
                    } else {
                        available.join(", ")
                    }
                );
            }

            let config = xberg::EmbeddingConfig {
                model: xberg::EmbeddingModelType::Plugin { name: name.to_string() },
                ..Default::default()
            };

            (config, name.to_string())
        }
        other => {
            anyhow::bail!(
                "Unknown embedding provider '{}'. Valid providers: 'local' (default, ONNX), 'llm' (liter-llm), or 'plugin' (in-process backend).",
                other
            );
        }
    };

    let embeddings = xberg::embed_texts(texts.clone(), &config).context("Failed to generate embeddings")?;

    let dimensions = embeddings.first().map(|e| e.len()).unwrap_or(0);

    match format {
        WireFormat::Json => {
            let output = serde_json::json!({
                "embeddings": embeddings,
                "model": model_label,
                "dimensions": dimensions,
                "count": embeddings.len(),
            });
            println!(
                "{}",
                serde_json::to_string_pretty(&output).context("Failed to serialize embeddings to JSON")?
            );
        }
        WireFormat::Toon => {
            let output = serde_json::json!({
                "embeddings": embeddings,
                "model": model_label,
                "dimensions": dimensions,
                "count": embeddings.len(),
            });
            println!(
                "{}",
                serde_toon::to_string(&output).context("Failed to serialize embeddings to TOON")?
            );
        }
        WireFormat::Text => {
            for (i, embedding) in embeddings.iter().enumerate() {
                if texts.len() > 1 {
                    println!("{}", style::dim(&format!("# text {}", i + 1)));
                }
                let values: Vec<String> = embedding.iter().map(|v| format!("{v}")).collect();
                println!("{}", values.join(","));
            }
        }
    }

    Ok(())
}
