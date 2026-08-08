//! Configuration file loading.
//!
//! This module provides methods for loading extraction configuration from
//! TOML, YAML, and JSON files.
//!
//! Loading here is entirely generic (`toml`/`serde_yaml_ng`/`serde_json` deserializing
//! straight into `ExtractionConfig`) — it never names individual fields, including
//! nested ones like `ChunkingConfig::breadcrumb_target`. New `#[serde(default)]`
//! fields on any nested config (e.g. #337's `breadcrumb_target`) are picked up
//! automatically with no change required in this file.

use crate::{Result, XbergError};
use std::path::Path;

use super::core::ExtractionConfig;

impl ExtractionConfig {
    /// Load configuration from a TOML file.
    ///
    /// # Errors
    ///
    /// Returns `XbergError::Validation` if file doesn't exist or is invalid TOML.
    pub fn from_toml_file(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let content = std::fs::read_to_string(path)
            .map_err(|e| XbergError::validation(format!("Failed to read config file {}: {}", path.display(), e)))?;
        let config: Self = toml::from_str(&content)
            .map_err(|e| XbergError::validation(format!("Invalid TOML in {}: {}", path.display(), e)))?;
        config.validate()?;
        Ok(config)
    }

    /// Load configuration from a YAML file.
    pub fn from_yaml_file(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let content = std::fs::read_to_string(path)
            .map_err(|e| XbergError::validation(format!("Failed to read config file {}: {}", path.display(), e)))?;
        let config: Self = serde_yaml_ng::from_str(&content)
            .map_err(|e| XbergError::validation(format!("Invalid YAML in {}: {}", path.display(), e)))?;
        config.validate()?;
        Ok(config)
    }

    /// Load configuration from a JSON file.
    pub fn from_json_file(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let content = std::fs::read_to_string(path)
            .map_err(|e| XbergError::validation(format!("Failed to read config file {}: {}", path.display(), e)))?;
        let config: Self = serde_json::from_str(&content)
            .map_err(|e| XbergError::validation(format!("Invalid JSON in {}: {}", path.display(), e)))?;
        config.validate()?;
        Ok(config)
    }

    /// Load configuration from a file, auto-detecting format by extension.
    ///
    /// Supported formats: `.toml`, `.yaml`, `.yml`, `.json`.
    pub fn from_file(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let extension = path.extension().and_then(|ext| ext.to_str()).ok_or_else(|| {
            XbergError::validation(format!(
                "Cannot determine file format: no extension found in {}",
                path.display()
            ))
        })?;

        match extension.to_lowercase().as_str() {
            "toml" => Self::from_toml_file(path),
            "yaml" | "yml" => Self::from_yaml_file(path),
            "json" => Self::from_json_file(path),
            other => Err(XbergError::validation(format!(
                "Unsupported config file format: .{}. Supported formats: .toml, .yaml, .json",
                other
            ))),
        }
    }

    /// Discover configuration file.
    ///
    /// Searches for `xberg.toml` in the current directory and its parents. If no
    /// project-local config is found, falls back to a per-user global config in
    /// the platform config directory: `xberg/xberg.{toml,yaml,yml,json}` under
    /// `dirs::config_dir()` — i.e. `$XDG_CONFIG_HOME` (or `~/.config`) on Linux,
    /// `~/Library/Application Support` on macOS, `%APPDATA%` on Windows.
    pub fn discover() -> Result<Option<Self>> {
        let mut current = std::env::current_dir().map_err(crate::XbergError::from)?;

        loop {
            let xberg_toml = current.join("xberg.toml");
            if xberg_toml.exists() {
                return Ok(Some(Self::from_toml_file(xberg_toml)?));
            }

            if let Some(parent) = current.parent() {
                current = parent.to_path_buf();
            } else {
                break;
            }
        }

        if let Some(config_dir) = dirs::config_dir()
            && let Some(config) = Self::find_config_in_dir(&config_dir.join("xberg"))?
        {
            return Ok(Some(config));
        }

        Ok(None)
    }

    /// Load the first `xberg.{toml,yaml,yml,json}` present in `dir`, if any.
    ///
    /// Extensions are probed in a fixed order so discovery is deterministic when
    /// multiple config files coexist in the same directory.
    fn find_config_in_dir(dir: &Path) -> Result<Option<Self>> {
        const CONFIG_BASENAMES: [&str; 4] = ["xberg.toml", "xberg.yaml", "xberg.yml", "xberg.json"];

        for basename in CONFIG_BASENAMES {
            let candidate = dir.join(basename);
            if candidate.exists() {
                return Ok(Some(Self::from_file(candidate)?));
            }
        }

        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_config_in_dir_returns_none_when_absent() {
        let dir = tempfile::tempdir().unwrap();
        let found = ExtractionConfig::find_config_in_dir(dir.path()).unwrap();
        assert!(found.is_none(), "empty dir must yield no config");
    }

    #[test]
    fn find_config_in_dir_loads_yaml_and_json() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("xberg.json"), "{}").unwrap();
        assert!(
            ExtractionConfig::find_config_in_dir(dir.path()).unwrap().is_some(),
            "xberg.json must be discovered"
        );

        std::fs::remove_file(dir.path().join("xberg.json")).unwrap();
        std::fs::write(dir.path().join("xberg.yaml"), "use_cache: true\n").unwrap();
        assert!(
            ExtractionConfig::find_config_in_dir(dir.path()).unwrap().is_some(),
            "xberg.yaml must be discovered"
        );
    }

    #[test]
    fn find_config_in_dir_prefers_toml_over_other_formats() {
        let dir = tempfile::tempdir().unwrap();
        // A valid TOML file and a deliberately invalid JSON file coexist. TOML is
        // probed first, so discovery must succeed without touching the JSON. ~keep
        std::fs::write(dir.path().join("xberg.toml"), "use_cache = true\n").unwrap();
        std::fs::write(dir.path().join("xberg.json"), "not valid json").unwrap();

        let found = ExtractionConfig::find_config_in_dir(dir.path()).unwrap();
        assert!(found.is_some(), "xberg.toml must win over xberg.json");
    }
}
