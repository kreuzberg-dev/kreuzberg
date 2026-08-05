//! Document orientation detection implementation using PP-LCNet_x1_0_doc_ori.
//!
//! Detects page-level orientation (0°, 90°, 180°, 270°) for scanned documents
//! and images. Runs through the [`crate::inference`] seam, so it works on either
//! engine: the ORT-backed `auto-rotate` feature or the pure-Rust `auto-rotate-tract`
//! variant (no-ORT targets). The model is engine-neutral either way.
//!
//! Used by ALL OCR backends when `auto_rotate` is enabled in `OcrConfig`.
//! More reliable than Tesseract's `DetectOrientationScript` which crashes
//! on raw images without DPI metadata.

#[cfg(all(not(target_arch = "wasm32"), any(feature = "ocr", sceptre_ocr)))]
use std::path::PathBuf;

use image::RgbImage;

use crate::Result;
use crate::error::XbergError;
use crate::inference::{InferenceSession, InferenceTensor, default_backend};

use super::types::OrientationResult;

/// HuggingFace repository containing the model. Native-only: WASM resolves weights
/// from caller-supplied bytes (see [`DocOrientationDetector::from_bytes`]), so the
/// download coordinates are not compiled there.
///
/// Only read by `ensure_model`/`with_acceleration`, the native cache-dir resolution
/// path; dead on WASM, which receives model bytes from the caller. ~keep
#[cfg(all(not(target_arch = "wasm32"), any(feature = "ocr", sceptre_ocr)))]
const HF_REPO_ID: &str = "xberg-io/paddleocr-onnx-models";
#[cfg(all(not(target_arch = "wasm32"), any(feature = "ocr", sceptre_ocr)))]
const HF_REPO_REVISION: &str = "bfaf0b492cfc1dee0c73245fc5860bfdcf2c3443";
#[cfg(all(not(target_arch = "wasm32"), any(feature = "ocr", sceptre_ocr)))]
const REMOTE_FILENAME: &str = "v2/classifiers/PP-LCNet_x1_0_doc_ori.onnx";
#[cfg(all(not(target_arch = "wasm32"), any(feature = "ocr", sceptre_ocr)))]
const SHA256: &str = "6b742aebce6f0f7f71f747931ac7becfc7c96c51641e14943b291eeb334e7947";

const INPUT_SIZE: u32 = 224;
const RESIZE_SHORT: u32 = 256;
const RGB_MEAN: [f32; 3] = [0.485 * 255.0, 0.456 * 255.0, 0.406 * 255.0];
const RGB_NORM: [f32; 3] = [1.0 / (0.229 * 255.0), 1.0 / (0.224 * 255.0), 1.0 / (0.225 * 255.0)];

/// Output labels: index -> degrees.
const ORIENTATION_LABELS: [u32; 4] = [0, 90, 180, 270];

/// PP-LCNet doc_ori outputs ~45% confidence for correct class in a 4-class problem.
/// Uniform baseline is 25%. A threshold of 0.35 provides good discrimination.
pub const MIN_CONFIDENCE: f32 = 0.35;

/// Where a [`DocOrientationDetector`] resolves its ONNX model weights from.
///
/// `CacheDir` downloads (and verifies) the model from Hugging Face Hub via
/// `hf-hub`/`reqwest`, which are native-only — see the target-gated dependency
/// declarations in `Cargo.toml`. `Bytes` is the WASM path: the JS host fetches the
/// weights (never embedded in the `.wasm` binary, since RT-DETR-family assets can
/// run into the hundreds of MB) and hands the already-resolved bytes over.
enum ModelSource {
    // Only constructed by `with_acceleration`, the cache-dir entry point used by the
    // Native OCR backends resolve this model through the Hugging Face cache. WASM
    // callers provide model bytes instead because they do not have that cache. ~keep
    #[cfg(all(not(target_arch = "wasm32"), any(feature = "ocr", sceptre_ocr)))]
    CacheDir(PathBuf),
    Bytes(Vec<u8>),
}

/// Detects document page orientation using the PP-LCNet model.
///
/// Thread-safe: the model runs behind `&self` through the [`crate::inference`]
/// seam, which owns the session synchronization. On native targets the model is
/// downloaded from HuggingFace on first use and cached locally; on `wasm32` it is
/// constructed from bytes supplied by the caller (see [`Self::from_bytes`]).
#[cfg_attr(alef, alef(skip))]
pub struct DocOrientationDetector {
    session: once_cell::sync::OnceCell<Box<dyn InferenceSession>>,
    source: ModelSource,
    acceleration: Option<crate::core::config::acceleration::AccelerationConfig>,
}

impl DocOrientationDetector {
    /// Creates a new detector with the given cache directory and acceleration config.
    ///
    /// Not available on `wasm32` — see [`Self::from_bytes`] for the WASM constructor.
    /// Used by native OCR backends that support whole-image automatic rotation. ~keep
    #[cfg(all(not(target_arch = "wasm32"), any(feature = "ocr", sceptre_ocr)))]
    pub(crate) fn with_acceleration(
        cache_dir: PathBuf,
        accel: Option<crate::core::config::acceleration::AccelerationConfig>,
    ) -> Self {
        Self {
            session: once_cell::sync::OnceCell::new(),
            source: ModelSource::CacheDir(cache_dir),
            acceleration: accel,
        }
    }

    /// Creates a new detector from already-resolved ONNX model bytes.
    ///
    /// The byte-buffer entry point (used on WASM, where there is no filesystem
    /// cache or HTTP download): the caller fetches the weights and hands over the
    /// bytes, which flow straight through to the [`crate::inference`] seam's
    /// `load_from_memory` — no filesystem path or HTTP download involved.
    pub fn from_bytes(
        model_bytes: Vec<u8>,
        accel: Option<crate::core::config::acceleration::AccelerationConfig>,
    ) -> Self {
        Self {
            session: once_cell::sync::OnceCell::new(),
            source: ModelSource::Bytes(model_bytes),
            acceleration: accel,
        }
    }

    /// Decode `image_bytes` and detect the document page orientation.
    ///
    /// A convenience wrapper over [`Self::detect`] for callers that hold encoded
    /// image bytes (PNG/JPEG/…) rather than a decoded [`RgbImage`] — notably the
    /// WASM bridge, which receives image bytes from JS.
    pub fn detect_image_bytes(&self, image_bytes: &[u8]) -> Result<OrientationResult> {
        let image = image::load_from_memory(image_bytes)
            .map_err(|e| XbergError::Ocr {
                message: format!("Failed to decode image for orientation detection: {e}"),
                source: None,
            })?
            .to_rgb8();
        self.detect(&image)
    }

    /// Detect document page orientation.
    ///
    /// Returns the detected orientation (0°, 90°, 180°, 270°) and confidence.
    /// Thread-safe: can be called concurrently from multiple pages.
    pub(crate) fn detect(&self, image: &RgbImage) -> Result<OrientationResult> {
        let session = self.get_or_init_session()?;
        let [center, start, end] = preprocess_crops(image);
        let mut best_result = infer_orientation(session, &center)?;
        if best_result.confidence >= MIN_CONFIDENCE {
            return Ok(best_result);
        }

        for edge_crop in [&start, &end] {
            let result = infer_orientation(session, edge_crop)?;
            if result.confidence > best_result.confidence {
                best_result = result;
            }
        }

        Ok(best_result)
    }

    /// Resolve the verified ONNX model directly from the Hugging Face cache.
    ///
    /// Not available on `wasm32` — see [`ModelSource::Bytes`]. Only called from the
    /// `ModelSource::CacheDir` arm of `get_or_init_session`, gated the same way. ~keep
    #[cfg(all(not(target_arch = "wasm32"), any(feature = "ocr", sceptre_ocr)))]
    fn ensure_model(cache_dir: &std::path::Path) -> Result<PathBuf> {
        crate::model_download::hf_resolve_file(
            HF_REPO_ID,
            REMOTE_FILENAME,
            Some(HF_REPO_REVISION),
            Some(cache_dir),
            Some(SHA256),
        )
        .map_err(|e| XbergError::Plugin {
            message: e,
            plugin_name: "auto-rotate".to_string(),
        })
    }

    /// Get or initialize the inference session (lazy, thread-safe via OnceCell).
    ///
    /// The session (optimization level, thread budget, execution-provider
    /// selection, and CPU fallback) is built by the [`crate::inference`] seam.
    fn get_or_init_session(&self) -> Result<&dyn InferenceSession> {
        let session = self
            .session
            .get_or_try_init(|| -> crate::Result<Box<dyn InferenceSession>> {
                let session = match &self.source {
                    #[cfg(all(not(target_arch = "wasm32"), any(feature = "ocr", sceptre_ocr)))]
                    ModelSource::CacheDir(cache_dir) => {
                        let model_path = Self::ensure_model(cache_dir)?;
                        default_backend()
                            .load(&model_path, self.acceleration.as_ref())
                            .map_err(|e| XbergError::Ocr {
                                message: format!("Failed to load doc_ori model: {e}"),
                                source: None,
                            })?
                    }
                    ModelSource::Bytes(model_bytes) => default_backend()
                        .load_from_memory(model_bytes, self.acceleration.as_ref())
                        .map_err(|e| XbergError::Ocr {
                            message: format!("Failed to load doc_ori model: {e}"),
                            source: None,
                        })?,
                };

                tracing::info!("Doc orientation model loaded");
                Ok(session)
            })?;
        Ok(session.as_ref())
    }
}

fn infer_orientation(session: &dyn InferenceSession, image: &RgbImage) -> Result<OrientationResult> {
    let input_tensor = normalize(image);

    let input_name = session
        .input_names()
        .first()
        .cloned()
        .unwrap_or_else(|| "x".to_string());
    let outputs = session
        .run(vec![(input_name, InferenceTensor::F32(input_tensor.into_dyn()))])
        .map_err(|e| XbergError::Ocr {
            message: format!("Doc orientation inference failed: {e}"),
            source: None,
        })?;

    let (_, output_value) = outputs.first().ok_or_else(|| XbergError::Ocr {
        message: "No output from doc orientation model".to_string(),
        source: None,
    })?;

    let probabilities: Vec<f32> = output_value
        .as_f32()
        .ok_or_else(|| XbergError::Ocr {
            message: "doc orientation output is not an f32 tensor".to_string(),
            source: None,
        })?
        .iter()
        .copied()
        .collect();

    orientation_from_probabilities(&probabilities)
}

/// Resolve the standard Hugging Face cache directory for the auto-rotate model.
///
/// Not available on `wasm32` — there is no Hugging Face cache on that target.
/// Used by native OCR backends that support whole-image automatic rotation. ~keep
#[cfg(all(not(target_arch = "wasm32"), any(feature = "ocr", sceptre_ocr)))]
pub(crate) fn resolve_cache_dir() -> PathBuf {
    hf_hub::resolve_cache_dir()
}

/// Apply PP-LCNet's resize-short transform and return center/start/end crops.
///
/// The center crop is the model's official preprocessing path. Edge crops are low-confidence
/// fallbacks for sparse pages whose text lies outside the center crop. ~keep
fn preprocess_crops(image: &RgbImage) -> [RgbImage; 3] {
    let (width, height) = image.dimensions();
    let (resized_width, resized_height) = if width < height {
        let scale = RESIZE_SHORT as f32 / width as f32;
        (RESIZE_SHORT, (height as f32 * scale).round() as u32)
    } else {
        let scale = RESIZE_SHORT as f32 / height as f32;
        ((width as f32 * scale).round() as u32, RESIZE_SHORT)
    };
    let resized = image::imageops::resize(
        image,
        resized_width,
        resized_height,
        image::imageops::FilterType::Triangle,
    );

    if resized_width >= resized_height {
        let cross_axis_offset = (resized_height - INPUT_SIZE) / 2;
        let end_offset = resized_width - INPUT_SIZE;
        let center_offset = end_offset / 2;
        [center_offset, 0, end_offset]
            .map(|x| image::imageops::crop_imm(&resized, x, cross_axis_offset, INPUT_SIZE, INPUT_SIZE).to_image())
    } else {
        let cross_axis_offset = (resized_width - INPUT_SIZE) / 2;
        let end_offset = resized_height - INPUT_SIZE;
        let center_offset = end_offset / 2;
        [center_offset, 0, end_offset]
            .map(|y| image::imageops::crop_imm(&resized, cross_axis_offset, y, INPUT_SIZE, INPUT_SIZE).to_image())
    }
}

/// Normalize RGB pixels to a `[1, 3, H, W]` tensor with ImageNet mean/std.
fn normalize(image: &RgbImage) -> ndarray::Array4<f32> {
    let (w, h) = (image.width() as usize, image.height() as usize);
    let mut tensor = ndarray::Array4::<f32>::zeros((1, 3, h, w));

    for y in 0..h {
        for x in 0..w {
            let pixel = image.get_pixel(x as u32, y as u32);
            for channel in 0..3 {
                tensor[[0, channel, y, x]] = (pixel[channel] as f32 - RGB_MEAN[channel]) * RGB_NORM[channel];
            }
        }
    }

    tensor
}

/// The exported ONNX graph ends in `Softmax`, so its values are already probabilities. ~keep
fn orientation_from_probabilities(probabilities: &[f32]) -> Result<OrientationResult> {
    if probabilities.len() != ORIENTATION_LABELS.len() {
        return Err(XbergError::Ocr {
            message: format!(
                "Doc orientation model returned {} probabilities; expected {}",
                probabilities.len(),
                ORIENTATION_LABELS.len()
            ),
            source: None,
        });
    }
    if probabilities
        .iter()
        .any(|probability| !probability.is_finite() || !(0.0..=1.0).contains(probability))
    {
        return Err(XbergError::Ocr {
            message: "Doc orientation model returned invalid probabilities".to_string(),
            source: None,
        });
    }
    let probability_sum: f32 = probabilities.iter().sum();
    if (probability_sum - 1.0).abs() > 0.01 {
        return Err(XbergError::Ocr {
            message: format!(
                "Doc orientation model probabilities sum to {probability_sum:.4}; expected approximately 1.0"
            ),
            source: None,
        });
    }

    let (best_idx, &best_probability) = probabilities
        .iter()
        .enumerate()
        .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
        .unwrap_or((0, &0.0));

    Ok(OrientationResult {
        degrees: ORIENTATION_LABELS.get(best_idx).copied().unwrap_or(0),
        confidence: best_probability,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_offer_edge_crops_for_sparse_portrait_page() {
        let mut image = RgbImage::from_pixel(20, 100, image::Rgb([0, 255, 0]));
        for y in 0..10 {
            for x in 0..image.width() {
                image.put_pixel(x, y, image::Rgb([255, 0, 0]));
                image.put_pixel(x, image.height() - 1 - y, image::Rgb([0, 0, 255]));
            }
        }

        let [center, start, end] = preprocess_crops(&image);

        assert_eq!(center.dimensions(), (INPUT_SIZE, INPUT_SIZE));
        assert_eq!(
            *center.get_pixel(INPUT_SIZE / 2, INPUT_SIZE / 2),
            image::Rgb([0, 255, 0])
        );
        assert_eq!(*start.get_pixel(INPUT_SIZE / 2, 0), image::Rgb([255, 0, 0]));
        assert_eq!(*end.get_pixel(INPUT_SIZE / 2, INPUT_SIZE - 1), image::Rgb([0, 0, 255]));
    }

    #[test]
    fn should_normalize_channels_in_rgb_order() {
        let image = RgbImage::from_pixel(1, 1, image::Rgb([255, 0, 128]));

        let tensor = normalize(&image);

        let expected_red = 2.248_908_3;
        let expected_green = -2.035_714_4;
        let expected_blue = 0.426_492_36;
        const NORMALIZATION_TEST_TOLERANCE: f32 = 1e-6;
        assert!((tensor[[0, 0, 0, 0]] - expected_red).abs() < NORMALIZATION_TEST_TOLERANCE);
        assert!((tensor[[0, 1, 0, 0]] - expected_green).abs() < NORMALIZATION_TEST_TOLERANCE);
        assert!((tensor[[0, 2, 0, 0]] - expected_blue).abs() < NORMALIZATION_TEST_TOLERANCE);
    }

    #[test]
    fn should_use_model_probability_as_confidence() {
        let result = orientation_from_probabilities(&[0.02, 0.03, 0.92, 0.03])
            .expect("valid model probabilities must be accepted");

        assert_eq!(result.degrees, 180);
        assert_eq!(result.confidence, 0.92);
    }

    #[test]
    fn should_reject_invalid_model_probabilities() {
        for probabilities in [vec![0.5, 0.5], vec![f32::NAN, 0.0, 0.0, 1.0], vec![0.5, 0.5, 0.5, 0.5]] {
            assert!(orientation_from_probabilities(&probabilities).is_err());
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    #[ignore = "requires the document-orientation ONNX model"]
    fn should_detect_each_quarter_turn_of_portrait_document() {
        let model_path = std::env::var_os("XBERG_DOC_ORIENTATION_MODEL")
            .expect("XBERG_DOC_ORIENTATION_MODEL must point to the ONNX model");
        let model_bytes = std::fs::read(model_path).expect("orientation model must be readable");
        let fixture_path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test_documents/images/ocr_test_original.png");
        let original = image::open(fixture_path)
            .expect("orientation fixture must exist")
            .to_rgb8();
        let detector = DocOrientationDetector::from_bytes(model_bytes, None);
        let cases = [
            (0, original.clone()),
            (90, image::imageops::rotate90(&original)),
            (180, image::imageops::rotate180(&original)),
            (270, image::imageops::rotate270(&original)),
        ];

        for (expected_degrees, image) in cases {
            let result = detector.detect(&image).expect("orientation inference must succeed");
            assert_eq!(result.degrees, expected_degrees);
            assert!(
                result.confidence > MIN_CONFIDENCE,
                "expected confidence above {MIN_CONFIDENCE}, got {} for {expected_degrees}°",
                result.confidence
            );
        }
    }
}
