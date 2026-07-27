//! Layout detection configuration.

use std::fmt;

use serde::{Deserialize, Serialize};

/// Which table structure recognition model to use.
///
/// Controls the model used for table cell detection within layout-detected
/// table regions. Wire format is snake_case in all serializers (JSON, TOML,
/// YAML).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TableModel {
    /// TATR (Table Transformer) -- default, 30MB, DETR-based row/column detection.
    #[default]
    Tatr,
    /// SLANeXT wired variant -- 365MB, optimized for bordered tables.
    SlanetWired,
    /// SLANeXT wireless variant -- 365MB, optimized for borderless tables.
    SlanetWireless,
    /// SLANet-plus -- 7.78MB, lightweight general-purpose.
    SlanetPlus,
    /// Classifier-routed SLANeXT: auto-select wired/wireless per table.
    /// Uses PP-LCNet classifier (6.78MB) + both SLANeXT variants (730MB total).
    SlanetAuto,
    /// Disable table structure model inference entirely; use heuristic path only.
    Disabled,
}

impl std::str::FromStr for TableModel {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "tatr" => Ok(Self::Tatr),
            "slanet_wired" => Ok(Self::SlanetWired),
            "slanet_wireless" => Ok(Self::SlanetWireless),
            "slanet_plus" => Ok(Self::SlanetPlus),
            "slanet_auto" => Ok(Self::SlanetAuto),
            "disabled" => Ok(Self::Disabled),
            other => Err(format!(
                "unknown table model: '{other}'. Valid: tatr, slanet_wired, slanet_wireless, slanet_plus, slanet_auto, disabled"
            )),
        }
    }
}

impl fmt::Display for TableModel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TableModel::Tatr => write!(f, "tatr"),
            TableModel::SlanetWired => write!(f, "slanet_wired"),
            TableModel::SlanetWireless => write!(f, "slanet_wireless"),
            TableModel::SlanetPlus => write!(f, "slanet_plus"),
            TableModel::SlanetAuto => write!(f, "slanet_auto"),
            TableModel::Disabled => write!(f, "disabled"),
        }
    }
}

/// How to resolve overlapping native vs layout (TATR/SLANeXT) tables.
///
/// When both native oxide detection and the layout table model produce a table for
/// the same page region, one must be dropped. This controls which one wins. Wire
/// format is snake_case in all serializers (JSON, TOML, YAML).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TableOverlapPreference {
    /// Keep whichever table carries more content (cell count + markdown length).
    /// This is the historical default. TATR/SLANeXT tables usually recognize more
    /// cells and therefore win, which maximizes table-structure F1 but can lower
    /// text F1 when the recognized cell reflow diverges from the source reading order.
    #[default]
    Content,
    /// Prefer the native oxide table when it overlaps a layout table. Native tables
    /// preserve the source reading order, which scores higher on text F1 for
    /// documents where the layout model's cell reflow diverges from the ground truth.
    Native,
    /// Prefer the layout (TATR/SLANeXT) table when it overlaps a native table.
    Layout,
}

impl std::str::FromStr for TableOverlapPreference {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "content" => Ok(Self::Content),
            "native" => Ok(Self::Native),
            "layout" => Ok(Self::Layout),
            other => Err(format!(
                "unknown table overlap preference: '{other}'. Valid: content, native, layout"
            )),
        }
    }
}

impl fmt::Display for TableOverlapPreference {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TableOverlapPreference::Content => write!(f, "content"),
            TableOverlapPreference::Native => write!(f, "native"),
            TableOverlapPreference::Layout => write!(f, "layout"),
        }
    }
}

/// Which PDF pages the layout model runs on.
///
/// Layout detection renders each selected page to a raster and runs ONNX
/// inference on it, which dominates extraction cost. This controls page
/// selection; [`LayoutStrategy::Always`] preserves the historical behavior of
/// running on every page. Wire format is snake_case in all serializers
/// (JSON, TOML, YAML).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LayoutStrategy {
    /// Run layout detection unconditionally on every page.
    #[default]
    Always,
    /// Pre-screen each page with cheap geometry signals and run the model
    /// only on pages likely to benefit (multi-column, table-bearing,
    /// figure-heavy, form-like, or rotated pages).
    ///
    /// Pages the pre-screen skips are processed exactly like pages where the
    /// model ran and found no regions. On the OCR path only inference is
    /// skipped; page rasters are still produced because OCR consumes them.
    /// For non-PDF inputs `Auto` behaves as [`LayoutStrategy::Always`].
    Auto,
}

impl std::str::FromStr for LayoutStrategy {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "always" => Ok(Self::Always),
            "auto" => Ok(Self::Auto),
            other => Err(format!("unknown layout strategy: '{other}'. Valid: always, auto")),
        }
    }
}

impl fmt::Display for LayoutStrategy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LayoutStrategy::Always => write!(f, "always"),
            LayoutStrategy::Auto => write!(f, "auto"),
        }
    }
}

/// Layout detection configuration.
///
/// Controls layout detection behavior in the extraction pipeline.
/// When set on [`ExtractionConfig`](super::ExtractionConfig), layout detection
/// is enabled for PDF extraction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LayoutDetectionConfig {
    /// Which pages the layout model runs on.
    ///
    /// Defaults to [`LayoutStrategy::Always`], the historical behavior:
    /// every page is rendered and inferred. [`LayoutStrategy::Auto`]
    /// pre-screens pages with cheap signals and skips the model where it
    /// cannot help.
    #[serde(default)]
    pub strategy: LayoutStrategy,

    /// Confidence threshold override (None = use model default).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence_threshold: Option<f32>,

    /// Whether to apply postprocessing heuristics (default: true).
    #[serde(default = "default_true")]
    pub apply_heuristics: bool,

    /// Table structure recognition model.
    ///
    /// Controls which model is used for table cell detection within layout-detected
    /// table regions. Defaults to [`TableModel::Tatr`].
    #[serde(default)]
    pub table_model: TableModel,

    /// How to resolve overlapping native vs layout tables.
    ///
    /// When a native oxide table and a layout (TATR/SLANeXT) table overlap on the
    /// same region, this controls which one is kept. Defaults to
    /// [`TableOverlapPreference::Content`] (historical behavior: keep the table with
    /// more content). Set to [`TableOverlapPreference::Native`] to favor source
    /// reading order (higher text F1) over the model's cell reflow.
    #[serde(default)]
    pub table_overlap_preference: TableOverlapPreference,

    /// Hardware acceleration for ONNX models (layout detection + table structure).
    ///
    /// When set, controls which execution provider (CPU, CUDA, CoreML, TensorRT)
    /// is used for inference. Defaults to `None` (auto-select per platform).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acceleration: Option<super::acceleration::AccelerationConfig>,

    /// Route regions classified as charts to the chart-understanding OCR task.
    ///
    /// When `true`, layout regions detected as charts are sent to the VLM
    /// chart task (data-series/axis recovery) instead of being treated as
    /// generic image regions. Defaults to `false` — chart understanding is
    /// opt-in and has no effect on standard text/table extraction scores.
    #[serde(default)]
    pub enable_chart_understanding: bool,
}

impl Default for LayoutDetectionConfig {
    fn default() -> Self {
        Self {
            strategy: LayoutStrategy::default(),
            confidence_threshold: None,
            apply_heuristics: true,
            table_model: TableModel::default(),
            table_overlap_preference: TableOverlapPreference::default(),
            acceleration: None,
            enable_chart_understanding: false,
        }
    }
}

fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = LayoutDetectionConfig::default();
        assert_eq!(config.strategy, LayoutStrategy::Always);
        assert_eq!(config.table_model, TableModel::Tatr);
        assert!(config.apply_heuristics);
        assert!(config.confidence_threshold.is_none());
    }

    #[test]
    fn layout_strategy_defaults_to_always_when_field_absent() {
        let config: LayoutDetectionConfig = serde_json::from_str("{}").expect("empty config must deserialize");
        assert_eq!(config.strategy, LayoutStrategy::Always);
    }

    #[test]
    fn layout_strategy_serde_roundtrip_is_snake_case() {
        let auto: LayoutStrategy = serde_json::from_str(r#""auto""#).expect("auto must deserialize");
        assert_eq!(auto, LayoutStrategy::Auto);
        assert_eq!(serde_json::to_string(&auto).expect("auto must serialize"), r#""auto""#);

        let always: LayoutStrategy = serde_json::from_str(r#""always""#).expect("always must deserialize");
        assert_eq!(always, LayoutStrategy::Always);
        assert_eq!(
            serde_json::to_string(&always).expect("always must serialize"),
            r#""always""#
        );
    }

    #[test]
    fn layout_strategy_from_str_accepts_wire_names_and_rejects_unknown() {
        assert_eq!("always".parse::<LayoutStrategy>(), Ok(LayoutStrategy::Always));
        assert_eq!("auto".parse::<LayoutStrategy>(), Ok(LayoutStrategy::Auto));

        let error = "adaptive".parse::<LayoutStrategy>().expect_err("unknown must fail");
        assert!(error.contains("unknown layout strategy: 'adaptive'"));
        assert!(error.contains("always, auto"));
    }

    #[test]
    fn layout_strategy_display_matches_wire_format() {
        assert_eq!(LayoutStrategy::Always.to_string(), "always");
        assert_eq!(LayoutStrategy::Auto.to_string(), "auto");
    }

    #[test]
    fn test_table_model_deserialize() {
        let json = r#""tatr""#;
        let model: TableModel = serde_json::from_str(json).unwrap();
        assert_eq!(model, TableModel::Tatr);

        let json = r#""slanet_auto""#;
        let model: TableModel = serde_json::from_str(json).unwrap();
        assert_eq!(model, TableModel::SlanetAuto);

        let json = r#""disabled""#;
        let model: TableModel = serde_json::from_str(json).unwrap();
        assert_eq!(model, TableModel::Disabled);
    }

    #[test]
    fn test_table_model_serialize() {
        let json = serde_json::to_string(&TableModel::SlanetWired).unwrap();
        assert_eq!(json, r#""slanet_wired""#);
    }

    #[test]
    fn test_table_model_round_trip() {
        for model in [
            TableModel::Tatr,
            TableModel::SlanetWired,
            TableModel::SlanetWireless,
            TableModel::SlanetPlus,
            TableModel::SlanetAuto,
            TableModel::Disabled,
        ] {
            let serialized = serde_json::to_string(&model).unwrap();
            let parsed: TableModel = serde_json::from_str(&serialized).unwrap();
            assert_eq!(parsed, model, "round-trip failed for {model:?}");
        }
    }

    #[test]
    fn test_backward_compat_unknown_fields_ignored() {
        let json = r#"{"preset": "accurate", "apply_heuristics": true}"#;
        let config: LayoutDetectionConfig = serde_json::from_str(json).unwrap();
        assert!(config.apply_heuristics);
        assert_eq!(config.table_model, TableModel::Tatr);
    }

    #[test]
    fn test_backward_compat_old_table_model_field() {
        let json = r#"{"table_model": "slanet_wired"}"#;
        let config: LayoutDetectionConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.table_model, TableModel::SlanetWired);
    }

    #[test]
    fn test_table_model_display() {
        assert_eq!(TableModel::Tatr.to_string(), "tatr");
        assert_eq!(TableModel::SlanetWired.to_string(), "slanet_wired");
        assert_eq!(TableModel::Disabled.to_string(), "disabled");
    }

    #[test]
    fn layout_detection_config_omitting_enable_chart_understanding_defaults_to_false() {
        // enable_chart_understanding uses `#[serde(default)]`.
        let json = r#"{"apply_heuristics": true, "table_model": "tatr"}"#;
        let config: LayoutDetectionConfig = serde_json::from_str(json).unwrap();
        assert!(
            !config.enable_chart_understanding,
            "omitted enable_chart_understanding must default to false"
        );
    }

    #[test]
    fn table_overlap_preference_defaults_to_content() {
        let config = LayoutDetectionConfig::default();
        assert_eq!(config.table_overlap_preference, TableOverlapPreference::Content);
    }

    #[test]
    fn table_overlap_preference_omitted_defaults_to_content() {
        let json = r#"{"apply_heuristics": true, "table_model": "tatr"}"#;
        let config: LayoutDetectionConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.table_overlap_preference, TableOverlapPreference::Content);
    }

    #[test]
    fn table_overlap_preference_serde_snake_case() {
        let config = LayoutDetectionConfig {
            table_overlap_preference: TableOverlapPreference::Native,
            ..LayoutDetectionConfig::default()
        };
        let json = serde_json::to_string(&config).unwrap();
        assert!(json.contains(r#""table_overlap_preference":"native""#), "got: {json}");
        let parsed: LayoutDetectionConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.table_overlap_preference, TableOverlapPreference::Native);
    }

    #[test]
    fn table_overlap_preference_from_str_and_display_round_trip() {
        for pref in [
            TableOverlapPreference::Content,
            TableOverlapPreference::Native,
            TableOverlapPreference::Layout,
        ] {
            let s = pref.to_string();
            let parsed: TableOverlapPreference = s.parse().unwrap();
            assert_eq!(parsed, pref, "round-trip failed for {pref:?}");
        }
        assert!("bogus".parse::<TableOverlapPreference>().is_err());
    }

    #[test]
    fn layout_detection_config_enable_chart_understanding_round_trip() {
        let config = LayoutDetectionConfig {
            enable_chart_understanding: true,
            ..LayoutDetectionConfig::default()
        };
        let json = serde_json::to_string(&config).unwrap();
        let deserialized: LayoutDetectionConfig = serde_json::from_str(&json).unwrap();
        assert!(deserialized.enable_chart_understanding);
    }
}
