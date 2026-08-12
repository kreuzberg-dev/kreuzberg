//! Unified OCR element types for structured output.
//!
//! This module provides a unified representation of OCR results that preserves
//! all spatial and confidence information from both Tesseract and PaddleOCR backends.
//!
//! # Design Goals
//!
//! - **Full fidelity preservation**: Keep all data from both backends (bounding boxes, confidence scores, rotation)
//! - **Unified API**: Same types work for both Tesseract and PaddleOCR
//! - **Format flexibility**: Support text, markdown, djot, and structured output formats
//! - **Table detection support**: Enable table reconstruction from element geometry

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Bounding geometry for an OCR element.
///
/// Supports both axis-aligned rectangles (from Tesseract) and 4-point quadrilaterals
/// (from PaddleOCR and rotated text detection).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
#[cfg_attr(feature = "api", derive(utoipa::ToSchema))]
pub enum OcrBoundingGeometry {
    /// Axis-aligned bounding box (typical for Tesseract output).
    Rectangle {
        /// Left x-coordinate in pixels
        left: u32,
        /// Top y-coordinate in pixels
        top: u32,
        /// Width in pixels
        width: u32,
        /// Height in pixels
        height: u32,
    },
    /// 4-point quadrilateral for rotated/skewed text (PaddleOCR).
    ///
    /// Points are in clockwise order starting from top-left:
    /// `[top_left, top_right, bottom_right, bottom_left]`
    Quadrilateral {
        /// Four corner points as `[[x, y], ...]` in clockwise order
        #[cfg_attr(feature = "api", schema(value_type = [[u32; 2]; 4]))]
        #[cfg_attr(alef, alef(skip))]
        points: [(u32, u32); 4],
    },
}

impl Default for OcrBoundingGeometry {
    fn default() -> Self {
        OcrBoundingGeometry::Rectangle {
            left: 0,
            top: 0,
            width: 0,
            height: 0,
        }
    }
}

impl OcrBoundingGeometry {
    /// Convert to axis-aligned bounding box (AABB).
    ///
    /// For rectangles, returns the exact bounds.
    /// For quadrilaterals, computes the minimal enclosing axis-aligned rectangle.
    ///
    /// # Returns
    ///
    /// Tuple of `(left, top, width, height)` in pixels.
    #[cfg(any(
        paddle_ocr,
        all(
            feature = "layout-detection",
            feature = "pdf",
            any(feature = "ocr", feature = "ocr-wasm")
        ),
        all(test, any(paddle_ocr, feature = "layout-detection", feature = "ocr"))
    ))]
    pub(crate) fn to_aabb(&self) -> (u32, u32, u32, u32) {
        match self {
            Self::Rectangle {
                left,
                top,
                width,
                height,
            } => (*left, *top, *width, *height),
            Self::Quadrilateral { points } => {
                let min_x = points.iter().map(|(x, _)| *x).min().unwrap_or(0);
                let max_x = points.iter().map(|(x, _)| *x).max().unwrap_or(0);
                let min_y = points.iter().map(|(_, y)| *y).min().unwrap_or(0);
                let max_y = points.iter().map(|(_, y)| *y).max().unwrap_or(0);
                (min_x, min_y, max_x.saturating_sub(min_x), max_y.saturating_sub(min_y))
            }
        }
    }

    /// Get the center point of the bounding geometry.
    #[cfg(any(
        all(
            feature = "layout-detection",
            feature = "pdf",
            any(feature = "ocr", feature = "ocr-wasm")
        ),
        all(test, feature = "layout-detection")
    ))]
    pub(crate) fn center(&self) -> (f64, f64) {
        let (left, top, width, height) = self.to_aabb();
        (left as f64 + width as f64 / 2.0, top as f64 + height as f64 / 2.0)
    }

    /// Check if this geometry overlaps with another.
    #[cfg(all(test, feature = "ocr"))]
    pub(crate) fn overlaps(&self, other: &Self) -> bool {
        let (l1, t1, w1, h1) = self.to_aabb();
        let (l2, t2, w2, h2) = other.to_aabb();

        let r1 = l1 + w1;
        let b1 = t1 + h1;
        let r2 = l2 + w2;
        let b2 = t2 + h2;

        l1 < r2 && r1 > l2 && t1 < b2 && b1 > t2
    }
}

/// Confidence scores for an OCR element.
///
/// Separates detection confidence (how confident that text exists at this location)
/// from recognition confidence (how confident about the actual text content).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "api", derive(utoipa::ToSchema))]
pub struct OcrConfidence {
    /// Detection confidence: how confident the OCR engine is that text exists here.
    ///
    /// PaddleOCR provides this as `box_score`, Tesseract doesn't have a direct equivalent.
    /// Range: 0.0 to 1.0 (or None if not available).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub detection: Option<f64>,

    /// Recognition confidence: how confident about the text content.
    ///
    /// Range: 0.0 to 1.0.
    pub recognition: f64,
}

impl OcrConfidence {
    /// Create confidence from Tesseract's single confidence value.
    ///
    /// Tesseract provides confidence as 0-100, which we normalize to 0.0-1.0.
    #[cfg(feature = "ocr")]
    pub(crate) fn from_tesseract(confidence: f64) -> Self {
        Self {
            detection: None,
            recognition: (confidence / 100.0).clamp(0.0, 1.0),
        }
    }

    /// Create confidence from PaddleOCR scores.
    ///
    /// Both scores should be in 0.0-1.0 range, but PaddleOCR may occasionally return
    /// values slightly above 1.0 due to model calibration. This method clamps both
    /// values to ensure they stay within the valid 0.0-1.0 range.
    #[cfg(paddle_ocr)]
    pub(crate) fn from_paddle(box_score: f32, text_score: f32) -> Self {
        Self {
            detection: Some((box_score as f64).clamp(0.0, 1.0)),
            recognition: (text_score as f64).clamp(0.0, 1.0),
        }
    }
}

/// Rotation information for an OCR element.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "api", derive(utoipa::ToSchema))]
pub struct OcrRotation {
    /// Rotation angle in degrees (0, 90, 180, 270 for PaddleOCR).
    pub angle_degrees: f64,

    /// Confidence score for the rotation detection.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f64>,
}

impl OcrRotation {
    /// Create rotation from PaddleOCR angle classification.
    ///
    /// PaddleOCR uses angle_index (0-3) representing 0, 90, 180, 270 degrees.
    ///
    /// # Arguments
    ///
    /// * `angle_index` - Must be in range 0..=3; invalid values return an error
    /// * `angle_score` - Confidence score for rotation detection
    ///
    /// # Errors
    ///
    /// Returns an error if `angle_index` is not in the valid range (0-3).
    #[cfg(paddle_ocr)]
    pub(crate) fn from_paddle(angle_index: i32, angle_score: f32) -> std::result::Result<Self, String> {
        if !(0..=3).contains(&angle_index) {
            return Err(format!(
                "Invalid angle_index: {}. Must be 0-3 (representing 0°, 90°, 180°, 270°)",
                angle_index
            ));
        }

        Ok(Self {
            angle_degrees: match angle_index {
                0 => 0.0,
                1 => 180.0,
                2 => 90.0,
                3 => 270.0,
                _ => unreachable!("angle_index validated to 0..=3 above"),
            },
            confidence: Some((angle_score as f64).clamp(0.0, 1.0)),
        })
    }
}

/// Hierarchical level of an OCR element.
///
/// Maps to Tesseract's page segmentation hierarchy and provides
/// equivalent semantics for PaddleOCR.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "api", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum OcrElementLevel {
    /// Individual word
    Word,
    /// Line of text (default for PaddleOCR)
    #[default]
    Line,
    /// Paragraph or text block
    Block,
    /// Page-level element
    Page,
}

impl OcrElementLevel {
    /// Convert from Tesseract's numeric level (1-5).
    ///
    /// Tesseract levels: 1=Page, 2=Block, 3=Paragraph, 4=Line, 5=Word
    #[cfg(feature = "ocr")]
    pub(crate) fn from_tesseract_level(level: i32) -> Self {
        match level {
            1 => Self::Page,
            2 => Self::Block,
            3 => Self::Block,
            4 => Self::Line,
            5 => Self::Word,
            _ => Self::Line,
        }
    }
}

/// A unified OCR element representing detected text with full metadata.
///
/// This is the primary type for structured OCR output, preserving all information
/// from both Tesseract and PaddleOCR backends.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "api", derive(utoipa::ToSchema))]
pub struct OcrElement {
    /// The recognized text content.
    pub text: String,

    /// Bounding geometry (rectangle or quadrilateral).
    pub geometry: OcrBoundingGeometry,

    /// Confidence scores for detection and recognition.
    pub confidence: OcrConfidence,

    /// Hierarchical level (word, line, block, page).
    #[serde(default)]
    pub level: OcrElementLevel,

    /// Rotation information (if detected).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rotation: Option<OcrRotation>,

    /// Page number (1-indexed).
    #[serde(default = "default_page_number")]
    pub page_number: u32,

    /// Parent element ID for hierarchical relationships.
    ///
    /// Only used for Tesseract output which has word -> line -> block hierarchy.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,

    /// Backend-specific metadata that doesn't fit the unified schema.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub backend_metadata: HashMap<String, serde_json::Value>,
}

fn default_page_number() -> u32 {
    1
}

#[cfg(feature = "ocr")]
impl OcrElement {
    /// Create a new OCR element with minimal required fields.
    pub(crate) fn new(text: impl Into<String>, geometry: OcrBoundingGeometry, confidence: OcrConfidence) -> Self {
        Self {
            text: text.into(),
            geometry,
            confidence,
            level: OcrElementLevel::default(),
            rotation: None,
            page_number: 1,
            parent_id: None,
            backend_metadata: HashMap::new(),
        }
    }

    /// Set the hierarchical level.
    pub(crate) fn with_level(mut self, level: OcrElementLevel) -> Self {
        self.level = level;
        self
    }

    /// Set rotation information.
    #[cfg(all(test, paddle_ocr))]
    pub(crate) fn with_rotation(mut self, rotation: OcrRotation) -> Self {
        self.rotation = Some(rotation);
        self
    }

    /// Set page number.
    pub(crate) fn with_page_number(mut self, page_number: u32) -> Self {
        self.page_number = page_number;
        self
    }

    /// Set parent element ID.
    pub(crate) fn with_parent_id(mut self, parent_id: impl Into<String>) -> Self {
        self.parent_id = Some(parent_id.into());
        self
    }

    /// Add backend-specific metadata.
    pub(crate) fn with_metadata(mut self, key: impl Into<String>, value: serde_json::Value) -> Self {
        self.backend_metadata.insert(key.into(), value);
        self
    }
}

/// Configuration for OCR element extraction.
///
/// Controls how OCR elements are extracted and filtered.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "api", derive(utoipa::ToSchema))]
pub struct OcrElementConfig {
    /// Whether to include OCR elements in the extraction result.
    ///
    /// When true, the `ocr_elements` field in `ExtractedDocument` will be populated.
    #[serde(default)]
    pub include_elements: bool,

    /// Minimum hierarchical level to include.
    ///
    /// Elements below this level (e.g., words when min_level is Line) will be excluded.
    #[serde(default)]
    pub min_level: OcrElementLevel,

    /// Minimum recognition confidence threshold (0.0-1.0).
    ///
    /// Elements with confidence below this threshold will be filtered out.
    #[serde(default)]
    pub min_confidence: f64,

    /// Whether to build hierarchical relationships between elements.
    ///
    /// When true, `parent_id` fields will be populated based on spatial containment.
    /// Only meaningful for Tesseract output.
    #[serde(default)]
    pub build_hierarchy: bool,
}

/// Geometry-only tests that do not require the `ocr` feature.
#[cfg(all(test, any(paddle_ocr, feature = "layout-detection")))]
mod geometry_tests {
    use super::*;

    #[test]
    fn test_rectangle_to_aabb() {
        let geom = OcrBoundingGeometry::Rectangle {
            left: 10,
            top: 20,
            width: 100,
            height: 50,
        };
        assert_eq!(geom.to_aabb(), (10, 20, 100, 50));
    }

    #[test]
    fn test_quadrilateral_to_aabb() {
        let geom = OcrBoundingGeometry::Quadrilateral {
            points: [(10, 22), (108, 20), (110, 72), (12, 74)],
        };
        let (left, top, width, height) = geom.to_aabb();
        assert_eq!(left, 10);
        assert_eq!(top, 20);
        assert_eq!(width, 100);
        assert_eq!(height, 54);
    }

    #[cfg(feature = "layout-detection")]
    #[test]
    fn test_geometry_center() {
        let geom = OcrBoundingGeometry::Rectangle {
            left: 0,
            top: 0,
            width: 100,
            height: 50,
        };
        let (cx, cy) = geom.center();
        assert!((cx - 50.0).abs() < 0.001);
        assert!((cy - 25.0).abs() < 0.001);
    }
}

#[cfg(all(test, feature = "ocr"))]
mod tests {
    use super::*;

    #[test]
    fn test_confidence_from_tesseract() {
        let conf = OcrConfidence::from_tesseract(85.0);
        assert!(conf.detection.is_none());
        assert!((conf.recognition - 0.85).abs() < 0.001);
    }

    #[cfg(paddle_ocr)]
    #[test]
    fn test_confidence_from_paddle() {
        let conf = OcrConfidence::from_paddle(0.95, 0.88);
        assert!(conf.detection.is_some());
        assert!((conf.detection.unwrap() - 0.95).abs() < 0.001);
        assert!((conf.recognition - 0.88).abs() < 0.001);
    }

    #[cfg(paddle_ocr)]
    #[test]
    fn test_rotation_from_paddle() {
        let rot = OcrRotation::from_paddle(1, 0.92).expect("Valid angle_index");
        assert_eq!(rot.angle_degrees, 180.0);
        assert!(rot.confidence.is_some());
        assert!((rot.confidence.unwrap() - 0.92).abs() < 0.001);
    }

    #[cfg(paddle_ocr)]
    #[test]
    fn test_rotation_from_paddle_invalid_angle_index() {
        assert!(OcrRotation::from_paddle(-1, 0.92).is_err());
        assert!(OcrRotation::from_paddle(4, 0.92).is_err());
        assert!(OcrRotation::from_paddle(100, 0.92).is_err());

        assert!(OcrRotation::from_paddle(0, 0.92).is_ok());
        assert!(OcrRotation::from_paddle(1, 0.92).is_ok());
        assert!(OcrRotation::from_paddle(2, 0.92).is_ok());
        assert!(OcrRotation::from_paddle(3, 0.92).is_ok());
    }

    #[test]
    fn test_element_level_from_tesseract() {
        assert_eq!(OcrElementLevel::from_tesseract_level(1), OcrElementLevel::Page);
        assert_eq!(OcrElementLevel::from_tesseract_level(2), OcrElementLevel::Block);
        assert_eq!(OcrElementLevel::from_tesseract_level(3), OcrElementLevel::Block);
        assert_eq!(OcrElementLevel::from_tesseract_level(4), OcrElementLevel::Line);
        assert_eq!(OcrElementLevel::from_tesseract_level(5), OcrElementLevel::Word);
    }

    #[test]
    fn test_ocr_element_builder() {
        let geom = OcrBoundingGeometry::Rectangle {
            left: 0,
            top: 0,
            width: 100,
            height: 20,
        };
        let conf = OcrConfidence::from_tesseract(90.0);

        let element = OcrElement::new("Hello", geom, conf)
            .with_level(OcrElementLevel::Word)
            .with_page_number(2)
            .with_metadata("backend", serde_json::json!("tesseract"));

        assert_eq!(element.text, "Hello");
        assert_eq!(element.level, OcrElementLevel::Word);
        assert_eq!(element.page_number, 2);
        assert!(element.backend_metadata.contains_key("backend"));
    }

    #[test]
    fn test_geometry_overlaps() {
        let geom1 = OcrBoundingGeometry::Rectangle {
            left: 0,
            top: 0,
            width: 100,
            height: 50,
        };
        let geom2 = OcrBoundingGeometry::Rectangle {
            left: 50,
            top: 25,
            width: 100,
            height: 50,
        };
        let geom3 = OcrBoundingGeometry::Rectangle {
            left: 200,
            top: 0,
            width: 50,
            height: 50,
        };

        assert!(geom1.overlaps(&geom2));
        assert!(!geom1.overlaps(&geom3));
    }

    #[cfg(paddle_ocr)]
    #[test]
    fn test_serialization_roundtrip() {
        let geom = OcrBoundingGeometry::Quadrilateral {
            points: [(10, 20), (100, 22), (98, 70), (8, 68)],
        };
        let conf = OcrConfidence::from_paddle(0.95, 0.88);
        let rot = OcrRotation::from_paddle(0, 0.99).expect("Valid angle_index");

        let element = OcrElement::new("Test text", geom, conf)
            .with_rotation(rot)
            .with_level(OcrElementLevel::Line);

        let json = serde_json::to_string(&element).expect("Failed to serialize");
        let deserialized: OcrElement = serde_json::from_str(&json).expect("Failed to deserialize");

        assert_eq!(deserialized.text, element.text);
        assert_eq!(deserialized.level, element.level);
        assert!(deserialized.rotation.is_some());
    }
}
