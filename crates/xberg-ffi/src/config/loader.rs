//! Configuration loading from files
//!
//! Handles loading ExtractionConfig from TOML/JSON/YAML files and discovery.

use crate::helpers::set_last_error;
use std::path::Path;
use xberg::XbergError;
use xberg::core::config::ExtractionConfig;

/// Load an ExtractionConfig from a file (returns JSON string).
///
/// # Arguments
///
/// * `file_path` - Path to the configuration file
///
/// # Returns
///
/// JSON string representation of the config, or error message.
pub fn load_config_as_json(file_path: &str) -> Result<String, String> {
    match ExtractionConfig::from_file(file_path) {
        Ok(config) => match serde_json::to_string(&config) {
            Ok(json) => Ok(json),
            Err(e) => Err(format!("Failed to serialize config to JSON: {}", e)),
        },
        Err(e) => Err(e.to_string()),
    }
}

/// Load an ExtractionConfig from a file (returns config struct).
///
/// # Arguments
///
/// * `path` - Path to the configuration file
///
/// # Returns
///
/// ExtractionConfig on success, or error message.
pub fn load_config_from_file(path: &Path) -> Result<ExtractionConfig, String> {
    match ExtractionConfig::from_file(path) {
        Ok(config) => Ok(config),
        Err(e) => match &e {
            XbergError::Io(io_err) => Err(format!("IO error loading config: {}", io_err)),
            _ => Err(format!("Failed to load config from file: {}", e)),
        },
    }
}

/// Discover and load an ExtractionConfig (returns JSON string).
///
/// Searches the current directory and all parent directories for:
/// - `xberg.toml`
/// - `xberg.json`
///
/// # Returns
///
/// JSON string of the first config file found, or None if not found.
pub fn discover_config_as_json() -> Option<String> {
    match ExtractionConfig::discover() {
        Ok(Some(config)) => match serde_json::to_string(&config) {
            Ok(json) => Some(json),
            Err(e) => {
                set_last_error(format!("Failed to serialize config: {}", e));
                None
            }
        },
        Ok(None) => None,
        Err(e) => {
            match &e {
                XbergError::Io(io_err) => {
                    set_last_error(format!("IO error discovering config: {}", io_err));
                }
                _ => {
                    set_last_error(format!("Failed to discover config: {}", e));
                }
            }
            None
        }
    }
}

/// List available embedding preset names.
///
/// # Returns
///
/// JSON array of preset names, or error message.
#[cfg(feature = "embeddings")]
pub fn list_embedding_presets() -> Result<String, String> {
    let presets = xberg::embeddings::list_presets();
    match serde_json::to_string(&presets) {
        Ok(json) => Ok(json),
        Err(e) => Err(format!("Failed to serialize presets: {}", e)),
    }
}

/// Get a specific embedding preset by name.
///
/// # Arguments
///
/// * `preset_name` - Name of the preset to retrieve
///
/// # Returns
///
/// JSON representation of the preset, or error message.
#[cfg(feature = "embeddings")]
pub fn get_embedding_preset(preset_name: &str) -> Result<String, String> {
    let preset = match xberg::embeddings::get_preset(preset_name) {
        Some(preset) => preset,
        None => {
            return Err(format!("Unknown embedding preset: {}", preset_name));
        }
    };

    let model_name = preset.model_repo.to_string();
    let serializable = super::serialize::SerializableEmbeddingPreset {
        name: preset.name,
        chunk_size: preset.chunk_size,
        overlap: preset.overlap,
        model_name,
        dimensions: preset.dimensions,
        description: preset.description,
    };

    match serde_json::to_string(&serializable) {
        Ok(json) => Ok(json),
        Err(e) => Err(format!("Failed to serialize embedding preset: {}", e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "embeddings")]
    #[test]
    fn test_list_embedding_presets() {
        let result = list_embedding_presets();
        assert!(result.is_ok());
        let json = result.unwrap();
        assert!(json.starts_with('['));
        assert!(json.ends_with(']'));
    }

    #[cfg(feature = "embeddings")]
    #[test]
    fn test_get_embedding_preset_unknown() {
        let result = get_embedding_preset("nonexistent_preset");
        assert!(result.is_err());
    }

    #[cfg(feature = "embeddings")]
    #[test]
    fn test_get_embedding_preset_valid() {
        let result = get_embedding_preset("fast");
        assert!(result.is_ok());
        let json = result.unwrap();
        assert!(json.contains("name"));
        assert!(json.contains("chunk_size"));
    }
}
