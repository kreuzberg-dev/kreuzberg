//! JSON-level configuration merging.
//!
//! Provides a unified merge function for combining a base `ExtractionConfig` with
//! JSON overrides. Used by both the CLI (`--config-json`) and MCP server to apply
//! partial configuration overrides without losing unspecified fields.

use super::ExtractionConfig;

/// Merge extraction configuration using JSON-level field override.
///
/// Serializes the base config to JSON, merges each field from the override JSON
/// (top-level only), and deserializes back. This correctly handles boolean fields
/// explicitly set to their default values — the override always wins for any field
/// present in `override_json`.
///
/// Fields **not** present in `override_json` are preserved from `base`.
///
/// # Errors
///
/// Returns `Err` if the base config cannot be serialized, or if the merged JSON
/// cannot be deserialized back into `ExtractionConfig` (e.g., wrong field types).
///
/// # Examples
///
/// ```rust,ignore
/// use xberg::ExtractionConfig;
/// use serde_json::json;
///
/// let mut base = ExtractionConfig::default();
/// base.use_cache = true;
///
/// let overrides = r#"{"force_ocr": true}"#;
/// let merged = xberg::core::config::merge::merge_config_json(&base, overrides).unwrap();
/// assert!(merged.use_cache);   // preserved from base
/// assert!(merged.force_ocr);   // applied from override
/// ```
#[cfg_attr(alef, alef(skip))]
pub fn merge_config_json(base: &ExtractionConfig, override_json: &str) -> Result<ExtractionConfig, String> {
    let override_value: serde_json::Value =
        serde_json::from_str(override_json).map_err(|e| format!("Failed to parse override JSON: {e}"))?;

    let mut config_json =
        serde_json::to_value(base).map_err(|e| format!("Failed to serialize base config to JSON: {e}"))?;

    if let serde_json::Value::Object(json_obj) = override_value
        && let Some(config_obj) = config_json.as_object_mut()
    {
        for (key, value) in json_obj {
            config_obj.insert(key, value);
        }
    }

    let mut merged: ExtractionConfig =
        serde_json::from_value(config_json).map_err(|e| format!("Failed to deserialize merged config: {e}"))?;

    restore_skipped_fields(base, &mut merged);

    merged.validate().map_err(|e| e.to_string())?;

    Ok(merged)
}

/// Restore fields marked `#[serde(skip)]` that the JSON round-trip in
/// [`merge_config_json`] cannot carry through the override.
///
/// These fields are never present in `override_json` (they don't serialize at
/// all), so there is no override semantics to apply here — the caller's
/// runtime-injected values from `base` always survive the merge unchanged:
///
/// - [`ExtractionConfig::cancel_token`] and [`ExtractionConfig::source_name`]
/// - [`crate::core::config::OcrConfig::acceleration`] and
///   [`crate::core::config::OcrConfig::tessdata_bytes`] (only when the merged
///   config still has an `ocr` section — an override that removes it entirely
///   has nothing to restore onto)
/// - [`PostProcessorConfig::enabled_set`] and
///   [`PostProcessorConfig::disabled_set`] (same caveat for `postprocessor`)
fn restore_skipped_fields(base: &ExtractionConfig, merged: &mut ExtractionConfig) {
    merged.cancel_token = base.cancel_token.clone();
    merged.source_name = base.source_name.clone();

    if let (Some(base_ocr), Some(merged_ocr)) = (base.ocr.as_ref(), merged.ocr.as_mut()) {
        merged_ocr.acceleration = base_ocr.acceleration.clone();
        merged_ocr.tessdata_bytes = base_ocr.tessdata_bytes.clone();
    }

    if let (Some(base_pp), Some(merged_pp)) = (base.postprocessor.as_ref(), merged.postprocessor.as_mut()) {
        merged_pp.enabled_set = base_pp.enabled_set.clone();
        merged_pp.disabled_set = base_pp.disabled_set.clone();
    }
}

/// Build extraction config by optionally merging JSON overrides into a base config.
///
/// If `override_json` is `None`, returns a clone of `base` after validating it. Otherwise
/// delegates to [`merge_config_json`], which validates the merged result.
#[cfg_attr(alef, alef(skip))]
pub fn build_config_from_json(
    base: &ExtractionConfig,
    override_json: Option<&str>,
) -> Result<ExtractionConfig, String> {
    match override_json {
        Some(json) => merge_config_json(base, json),
        None => {
            base.validate().map_err(|e| e.to_string())?;
            Ok(base.clone())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_merge_preserves_unspecified_fields() {
        let base = ExtractionConfig {
            use_cache: false,
            enable_quality_processing: true,
            force_ocr: false,
            ..Default::default()
        };

        let merged = merge_config_json(&base, r#"{"force_ocr": true}"#).unwrap();

        assert!(!merged.use_cache, "use_cache should be preserved from base");
        assert!(
            merged.enable_quality_processing,
            "enable_quality_processing should be preserved"
        );
        assert!(merged.force_ocr, "force_ocr should be overridden");
    }

    #[test]
    fn test_merge_override_to_default_value() {
        let base = ExtractionConfig {
            use_cache: false,
            ..Default::default()
        };

        let merged = merge_config_json(&base, r#"{"use_cache": true}"#).unwrap();
        assert!(
            merged.use_cache,
            "Should use explicit override even if it matches the struct default"
        );
    }

    #[test]
    fn test_merge_multiple_fields() {
        let base = ExtractionConfig {
            use_cache: true,
            force_ocr: true,
            ..Default::default()
        };

        let merged = merge_config_json(&base, r#"{"use_cache": false, "output_format": "markdown"}"#).unwrap();

        assert!(!merged.use_cache);
        assert!(merged.force_ocr, "force_ocr should be preserved");
        assert_eq!(
            merged.output_format,
            crate::core::config::formats::OutputFormat::Markdown,
        );
    }

    #[test]
    fn test_merge_invalid_field_type_returns_error() {
        let base = ExtractionConfig::default();
        let result = merge_config_json(&base, r#"{"use_cache": "not_a_boolean"}"#);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Failed to deserialize"));
    }

    #[test]
    fn test_build_config_from_json_none_returns_clone() {
        let base = ExtractionConfig {
            use_cache: false,
            ..Default::default()
        };
        let result = build_config_from_json(&base, None).unwrap();
        assert!(!result.use_cache);
    }

    #[test]
    fn test_build_config_from_json_some_merges() {
        let base = ExtractionConfig::default();
        let result = build_config_from_json(&base, Some(r#"{"force_ocr": true}"#)).unwrap();
        assert!(result.force_ocr);
    }

    /// Regression test for #218: the JSON round-trip in `merge_config_json`
    /// serializes `base` to `serde_json::Value`, which drops every
    /// `#[serde(skip)]` field before the override is even applied. Before the
    /// fix, `merged.cancel_token` / `source_name` were always `None`,
    /// `merged.ocr.{acceleration,tessdata_bytes}` were always `None`, and
    /// `merged.postprocessor.{enabled_set,disabled_set}` were always `None` —
    /// regardless of what `base` held — even when the override JSON never
    /// touched those sections at all.
    #[test]
    fn test_merge_preserves_serde_skip_fields() {
        use crate::cancellation::CancellationToken;
        use crate::core::config::{AccelerationConfig, ExecutionProviderType, OcrConfig, PostProcessorConfig};
        use ahash::AHashSet;

        let token = CancellationToken::new();

        let mut postprocessor = PostProcessorConfig {
            enabled_processors: Some(vec!["a".to_string()]),
            disabled_processors: Some(vec!["b".to_string()]),
            ..Default::default()
        };
        postprocessor.build_lookup_sets();

        let base = ExtractionConfig {
            cancel_token: Some(token.clone()),
            source_name: Some("secret/report.pdf".to_string()),
            ocr: Some(OcrConfig {
                acceleration: Some(AccelerationConfig {
                    provider: ExecutionProviderType::Cpu,
                    device_id: 2,
                }),
                tessdata_bytes: Some(std::collections::HashMap::from([("eng".to_string(), vec![1u8, 2, 3])])),
                ..Default::default()
            }),
            postprocessor: Some(postprocessor),
            ..Default::default()
        };

        // Override touches an unrelated top-level field only.
        let merged = merge_config_json(&base, r#"{"use_cache": false}"#).unwrap();
        assert!(!merged.use_cache, "override field should still apply");

        assert_eq!(
            merged.source_name,
            Some("secret/report.pdf".to_string()),
            "source_name must survive the merge"
        );

        // cancel_token must be the SAME token (a clone of the same Arc), not merely
        // present: cancelling the original must be observable through the merged copy.
        token.cancel();
        assert!(
            merged
                .cancel_token
                .expect("cancel_token must survive merge")
                .is_cancelled(),
            "merged cancel_token must be a clone of base's token"
        );

        let merged_ocr = merged.ocr.expect("ocr section must survive merge");
        assert_eq!(
            merged_ocr.acceleration.map(|a| (a.provider, a.device_id)),
            Some((ExecutionProviderType::Cpu, 2)),
            "ocr.acceleration must survive merge"
        );
        assert_eq!(
            merged_ocr.tessdata_bytes.and_then(|m| m.get("eng").cloned()),
            Some(vec![1u8, 2, 3]),
            "ocr.tessdata_bytes must survive merge"
        );

        let merged_pp = merged.postprocessor.expect("postprocessor section must survive merge");
        assert_eq!(
            merged_pp.enabled_set,
            Some(AHashSet::from_iter(["a".to_string()])),
            "postprocessor.enabled_set must survive merge"
        );
        assert_eq!(
            merged_pp.disabled_set,
            Some(AHashSet::from_iter(["b".to_string()])),
            "postprocessor.disabled_set must survive merge"
        );
    }
}
