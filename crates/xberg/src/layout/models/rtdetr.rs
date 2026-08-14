use std::time::Instant;

use image::RgbImage;
use ndarray::{Array, Array2, Array4};

use crate::core::config::acceleration::AccelerationConfig;
#[cfg(target_os = "macos")]
use crate::core::config::acceleration::ExecutionProviderType;
use crate::inference::{InferenceSession, InferenceTensor, default_backend};
use crate::layout::error::LayoutError;
use crate::layout::models::LayoutModel;
use crate::layout::preprocessing;
use crate::layout::types::{BBox, LayoutClass, LayoutDetection};

/// Default confidence threshold for RT-DETR detections.
const DEFAULT_THRESHOLD: f32 = 0.3;

/// RT-DETR input resolution.
const INPUT_SIZE: u32 = 640;

pub(crate) fn effective_acceleration(accel: Option<&AccelerationConfig>) -> Option<AccelerationConfig> {
    // The current RT-DETR export fails under CoreML; keep auto reliable while preserving explicit CoreML.
    #[cfg(target_os = "macos")]
    if accel.is_none_or(|config| config.provider == ExecutionProviderType::Auto) {
        return Some(AccelerationConfig {
            provider: ExecutionProviderType::Cpu,
            device_id: accel.map_or(0, |config| config.device_id),
        });
    }

    accel.cloned()
}

/// Docling RT-DETR v2 layout detection model.
///
/// This model is NMS-free (transformer-based end-to-end detection).
///
/// Input tensors:
///   - `images`:            f32 [batch, 3, 640, 640]  (preprocessed pixel data)
///   - `orig_target_sizes`: i64 [batch, 2]            ([height, width] of source image)
///
/// Output tensors:
///   - `labels`: i64 [batch, num_queries]   (class IDs, 0-16)
///   - `boxes`:  f32 [batch, num_queries, 4] (bounding boxes in original image coordinates)
///   - `scores`: f32 [batch, num_queries]   (confidence scores)
#[cfg_attr(alef, alef(skip))]
pub struct RtDetrModel {
    session: Box<dyn InferenceSession>,
    input_names: Vec<String>,
}

impl RtDetrModel {
    /// Load a Docling RT-DETR ONNX model from a file.
    ///
    /// The session (optimization level, thread budget, execution-provider
    /// selection, and CPU-only fallback) is built by the [`crate::inference`]
    /// seam's default backend, so the model is engine-neutral.
    ///
    /// Native-only: WASM has no filesystem model path and uses [`Self::from_bytes`].
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn from_file(
        path: &str,
        accel: Option<&AccelerationConfig>,
        thread_budget: usize,
    ) -> Result<Self, LayoutError> {
        let accel = effective_acceleration(accel);
        let session = default_backend()
            .load_with_thread_budget(std::path::Path::new(path), accel.as_ref(), thread_budget)
            .map_err(|e| LayoutError::Inference(e.to_string()))?;
        let input_names: Vec<String> = session.input_names().to_vec();
        Ok(Self { session, input_names })
    }

    /// Load a Docling RT-DETR ONNX model from an in-memory byte buffer.
    ///
    /// Used where there is no filesystem path to read from, e.g. WASM builds where
    /// the JS host fetches and hands over the model weights directly. Uses the same
    /// engine-neutral [`crate::inference`] seam as [`Self::from_file`].
    pub(crate) fn from_bytes(model_bytes: &[u8], accel: Option<&AccelerationConfig>) -> Result<Self, LayoutError> {
        let accel = effective_acceleration(accel);
        let session = default_backend()
            .load_from_memory(model_bytes, accel.as_ref())
            .map_err(|e| LayoutError::Inference(e.to_string()))?;
        let input_names: Vec<String> = session.input_names().to_vec();
        Ok(Self { session, input_names })
    }

    /// The two input names (`images`, `orig_target_sizes`) the RT-DETR graph declares,
    /// cloned for the `session.run` call.
    ///
    /// A model handed to [`Self::from_bytes`] by a caller (e.g. the WASM `detectLayout`
    /// bridge) could declare fewer than two inputs; return an error rather than panicking
    /// on an out-of-range index into `input_names`.
    fn input_names_pair(&self) -> Result<(String, String), LayoutError> {
        match (self.input_names.first(), self.input_names.get(1)) {
            (Some(images), Some(sizes)) => Ok((images.clone(), sizes.clone())),
            _ => Err(LayoutError::Inference(format!(
                "RT-DETR model must declare 2 inputs (images, orig_target_sizes), found {}",
                self.input_names.len()
            ))),
        }
    }

    /// Run inference and extract detections from raw outputs.
    ///
    /// Uses the original official export contract: exact 640x640 bilinear
    /// resize, /255 rescaling, and no ImageNet normalization. The model uses
    /// `orig_target_sizes` to return boxes in source-image coordinates.
    fn run_inference(&mut self, img: &RgbImage, threshold: f32) -> Result<Vec<LayoutDetection>, LayoutError> {
        #[cfg(feature = "otel")]
        let inference_span = crate::telemetry::spans::model_inference_span("rtdetr-layout");
        #[cfg(feature = "otel")]
        let _inference_guard = inference_span.enter();
        #[cfg(feature = "otel")]
        let inference_start = Instant::now();

        let orig_width = img.width();
        let orig_height = img.height();

        let preprocess_start = Instant::now();

        let input_tensor = preprocessing::preprocess_rescale(img, INPUT_SIZE);

        // The official RT-DETR postprocessor multiplies xyxy boxes by
        // `orig_target_sizes.repeat(1, 2)`, so the tensor order is (w, h).
        // Passing (h, w) inflates x by H/W and compresses y by W/H on
        // portrait pages, which misplaces every region crop.
        let sizes = Array::from_shape_vec((1, 2), vec![orig_width as i64, orig_height as i64])
            .map_err(|e| LayoutError::InvalidOutput(format!("Failed to create sizes tensor: {e}")))?;

        let preprocess_ms = preprocess_start.elapsed().as_secs_f64() * 1000.0;
        tracing::debug!(preprocess_ms, "RT-DETR preprocessing complete");

        let onnx_start = Instant::now();

        let (images_name, sizes_name) = self.input_names_pair()?;
        let outputs = self
            .session
            .run(vec![
                (images_name, InferenceTensor::F32(input_tensor.into_dyn())),
                (sizes_name, InferenceTensor::I64(sizes.into_dyn())),
            ])
            .map_err(|e| LayoutError::Inference(e.to_string()))?;

        let onnx_ms = onnx_start.elapsed().as_secs_f64() * 1000.0;
        tracing::debug!(onnx_ms, "RT-DETR ONNX session.run() complete");

        let mut float_data: Vec<Vec<f32>> = Vec::new();
        let mut float_shapes: Vec<Vec<usize>> = Vec::new();
        let mut label_data: Vec<i64> = Vec::new();

        for (_name, value) in outputs {
            match value {
                InferenceTensor::I64(array) => {
                    label_data = array.into_raw_vec_and_offset().0;
                }
                InferenceTensor::F32(array) => {
                    float_shapes.push(array.shape().to_vec());
                    float_data.push(array.into_raw_vec_and_offset().0);
                }
                _ => {}
            }
        }

        if label_data.is_empty() && float_data.len() >= 3 {
            label_data = float_data.last().unwrap().iter().map(|&v| v as i64).collect();
            float_data.pop();
            float_shapes.pop();
        }

        if float_data.len() < 2 {
            return Err(LayoutError::InvalidOutput(format!(
                "Expected at least 2 float output tensors, got {}",
                float_data.len()
            )));
        }

        let boxes = &float_data[0];
        let scores = &float_data[1];
        let box_shape = &float_shapes[0];
        let num_detections = if box_shape.len() == 3 {
            box_shape[1]
        } else {
            box_shape[0]
        };

        if scores.len() < num_detections || label_data.len() < num_detections || boxes.len() < num_detections * 4 {
            return Err(LayoutError::InvalidOutput(format!(
                "RT-DETR output shape mismatch: num_detections={num_detections} \
                 but scores.len()={}, labels.len()={}, boxes.len()={}",
                scores.len(),
                label_data.len(),
                boxes.len()
            )));
        }

        let mut detections = Vec::new();
        for i in 0..num_detections {
            let score = scores[i];
            if score < threshold {
                continue;
            }

            let label_id = label_data[i];
            let class = match LayoutClass::from_docling_id(label_id) {
                Some(c) => c,
                None => continue,
            };

            let bbox = clamp_output_box(
                [boxes[i * 4], boxes[i * 4 + 1], boxes[i * 4 + 2], boxes[i * 4 + 3]],
                orig_width,
                orig_height,
            );

            detections.push(LayoutDetection::new(class, score, bbox));
        }

        detections = LayoutDetection::sort_by_confidence_desc(detections);

        crate::layout::inference_timings::set(preprocess_ms, onnx_ms);

        tracing::debug!(
            preprocess_ms,
            onnx_ms,
            detections = detections.len(),
            "RT-DETR inference breakdown"
        );

        #[cfg(feature = "otel")]
        {
            let total_inference_ms = inference_start.elapsed().as_secs_f64() * 1000.0;
            tracing::Span::current().record(crate::telemetry::conventions::MODEL_INFERENCE_MS, total_inference_ms);
        }

        Ok(detections)
    }

    /// Run batched inference over multiple images in a single ONNX call.
    ///
    /// Stacks per-image tensors into `[N, 3, 640, 640]` and `[N, 2]` inputs,
    /// executes a single `session.run()`, then splits outputs by batch index.
    ///
    /// Returns one `Vec<LayoutDetection>` per input image, in the same order.
    pub(crate) fn run_batch_inference(
        &mut self,
        images: &[&RgbImage],
        threshold: f32,
    ) -> Result<Vec<Vec<LayoutDetection>>, LayoutError> {
        #[cfg(feature = "otel")]
        let inference_span = crate::telemetry::spans::model_inference_span("rtdetr-layout");
        #[cfg(feature = "otel")]
        let _inference_guard = inference_span.enter();
        #[cfg(feature = "otel")]
        let inference_start = Instant::now();

        if images.is_empty() {
            return Ok(Vec::new());
        }
        let batch = images.len();

        let ts = INPUT_SIZE as usize;
        let hw = ts * ts;

        let preprocess_start = Instant::now();

        let mut all_pixel_data: Vec<f32> = Vec::with_capacity(batch * 3 * hw);
        let mut metas: Vec<(u32, u32)> = Vec::with_capacity(batch);

        for img in images {
            let tensor = preprocessing::preprocess_rescale(img, INPUT_SIZE);
            let slice = tensor
                .as_slice()
                .ok_or_else(|| LayoutError::InvalidOutput("preprocessed image tensor is not contiguous".to_string()))?;
            all_pixel_data.extend_from_slice(slice);
            metas.push((img.width(), img.height()));
        }

        let images_array = Array4::from_shape_vec((batch, 3, ts, ts), all_pixel_data)
            .map_err(|e| LayoutError::InvalidOutput(format!("Failed to build batch images tensor: {e}")))?;

        // (w, h) per image; see the single-image path for the contract.
        let sizes_flat: Vec<i64> = images
            .iter()
            .flat_map(|img| [img.width() as i64, img.height() as i64])
            .collect();
        let sizes_array = Array2::from_shape_vec((batch, 2), sizes_flat)
            .map_err(|e| LayoutError::InvalidOutput(format!("Failed to build batch sizes tensor: {e}")))?;

        let preprocess_ms = preprocess_start.elapsed().as_secs_f64() * 1000.0;
        tracing::debug!(preprocess_ms, batch, "RT-DETR batch preprocessing complete");

        let onnx_start = Instant::now();

        let (images_name, sizes_name) = self.input_names_pair()?;
        let outputs = self
            .session
            .run(vec![
                (images_name, InferenceTensor::F32(images_array.into_dyn())),
                (sizes_name, InferenceTensor::I64(sizes_array.into_dyn())),
            ])
            .map_err(|e| LayoutError::Inference(e.to_string()))?;

        let onnx_ms = onnx_start.elapsed().as_secs_f64() * 1000.0;
        tracing::debug!(onnx_ms, batch, "RT-DETR batch ONNX session.run() complete");

        let mut float_data: Vec<Vec<f32>> = Vec::new();
        let mut float_shapes: Vec<Vec<usize>> = Vec::new();
        let mut label_data: Vec<i64> = Vec::new();

        for (_name, value) in outputs {
            match value {
                InferenceTensor::I64(array) => {
                    label_data = array.into_raw_vec_and_offset().0;
                }
                InferenceTensor::F32(array) => {
                    float_shapes.push(array.shape().to_vec());
                    float_data.push(array.into_raw_vec_and_offset().0);
                }
                _ => {}
            }
        }

        if label_data.is_empty() && float_data.len() >= 3 {
            label_data = float_data.last().unwrap().iter().map(|&v| v as i64).collect();
            float_data.pop();
            float_shapes.pop();
        }

        if float_data.len() < 2 {
            return Err(LayoutError::InvalidOutput(format!(
                "Expected at least 2 float output tensors, got {}",
                float_data.len()
            )));
        }

        let boxes = &float_data[0];
        let scores = &float_data[1];
        let box_shape = &float_shapes[0];

        let num_queries = if box_shape.len() == 3 {
            box_shape[1]
        } else {
            box_shape[0]
        };

        crate::layout::inference_timings::set(preprocess_ms / batch as f64, onnx_ms / batch as f64);

        let expected_flat = batch * num_queries;
        if scores.len() < expected_flat || label_data.len() < expected_flat || boxes.len() < expected_flat * 4 {
            return Err(LayoutError::InvalidOutput(format!(
                "RT-DETR batch output shape mismatch: batch={batch} num_queries={num_queries} \
                 but scores.len()={}, labels.len()={}, boxes.len()={}",
                scores.len(),
                label_data.len(),
                boxes.len()
            )));
        }

        let mut results: Vec<Vec<LayoutDetection>> = Vec::with_capacity(batch);

        for (b, &(orig_width, orig_height)) in metas.iter().enumerate() {
            let mut detections = Vec::new();
            for i in 0..num_queries {
                let flat_i = b * num_queries + i;
                let score = scores[flat_i];
                if score < threshold {
                    continue;
                }

                let label_id = label_data[flat_i];
                let class = match LayoutClass::from_docling_id(label_id) {
                    Some(c) => c,
                    None => continue,
                };

                let box_base = flat_i * 4;
                let bbox = clamp_output_box(
                    [
                        boxes[box_base],
                        boxes[box_base + 1],
                        boxes[box_base + 2],
                        boxes[box_base + 3],
                    ],
                    orig_width,
                    orig_height,
                );

                detections.push(LayoutDetection::new(class, score, bbox));
            }

            detections = LayoutDetection::sort_by_confidence_desc(detections);

            tracing::debug!(
                batch_index = b,
                detections = detections.len(),
                "RT-DETR batch inference: per-image detections"
            );

            results.push(detections);
        }

        tracing::debug!(preprocess_ms, onnx_ms, batch, "RT-DETR batch inference breakdown");

        #[cfg(feature = "otel")]
        {
            let total_inference_ms = inference_start.elapsed().as_secs_f64() * 1000.0;
            tracing::Span::current().record(crate::telemetry::conventions::MODEL_INFERENCE_MS, total_inference_ms);
        }

        Ok(results)
    }
}

fn clamp_output_box(coords: [f32; 4], image_width: u32, image_height: u32) -> BBox {
    BBox::new(
        coords[0].clamp(0.0, image_width as f32),
        coords[1].clamp(0.0, image_height as f32),
        coords[2].clamp(0.0, image_width as f32),
        coords[3].clamp(0.0, image_height as f32),
    )
}

impl LayoutModel for RtDetrModel {
    fn detect(&mut self, img: &RgbImage) -> Result<Vec<LayoutDetection>, LayoutError> {
        self.run_inference(img, DEFAULT_THRESHOLD)
    }

    fn detect_with_threshold(&mut self, img: &RgbImage, threshold: f32) -> Result<Vec<LayoutDetection>, LayoutError> {
        self.run_inference(img, threshold)
    }

    fn detect_batch(
        &mut self,
        images: &[&RgbImage],
        threshold: Option<f32>,
    ) -> Result<Vec<Vec<LayoutDetection>>, LayoutError> {
        if images.is_empty() {
            return Ok(Vec::new());
        }
        let t = threshold.unwrap_or(DEFAULT_THRESHOLD);
        if images.len() == 1 {
            return self.run_inference(images[0], t).map(|d| vec![d]);
        }
        self.run_batch_inference(images, t)
    }

    fn name(&self) -> &str {
        "Docling RT-DETR v2"
    }
}

#[cfg(test)]
mod tests {
    use super::clamp_output_box;
    #[cfg(target_os = "macos")]
    use super::effective_acceleration;
    #[cfg(target_os = "macos")]
    use crate::core::config::acceleration::{AccelerationConfig, ExecutionProviderType};

    #[test]
    fn output_boxes_are_already_in_portrait_source_coordinates() {
        let bbox = clamp_output_box([32.0, 64.0, 288.0, 576.0], 320, 640);

        assert_eq!([bbox.x1, bbox.y1, bbox.x2, bbox.y2], [32.0, 64.0, 288.0, 576.0]);
    }

    #[test]
    fn output_boxes_are_already_in_landscape_source_coordinates() {
        let bbox = clamp_output_box([-5.0, 32.0, 650.0, 340.0], 640, 320);

        assert_eq!([bbox.x1, bbox.y1, bbox.x2, bbox.y2], [0.0, 32.0, 640.0, 320.0]);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn rtdetr_auto_uses_cpu_but_explicit_coreml_is_preserved() {
        assert_eq!(
            effective_acceleration(None).expect("auto must resolve").provider,
            ExecutionProviderType::Cpu
        );

        let automatic = AccelerationConfig {
            provider: ExecutionProviderType::Auto,
            device_id: 2,
        };
        let automatic = effective_acceleration(Some(&automatic)).expect("auto must resolve");
        assert_eq!(automatic.provider, ExecutionProviderType::Cpu);
        assert_eq!(automatic.device_id, 2);

        let coreml = AccelerationConfig {
            provider: ExecutionProviderType::CoreMl,
            device_id: 0,
        };
        assert_eq!(
            effective_acceleration(Some(&coreml)).expect("explicit CoreML must be preserved"),
            coreml
        );
    }
}
