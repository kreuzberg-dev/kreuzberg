use std::collections::HashMap;

use image::ImageBuffer;
use ort::session::builder::SessionBuilder;
use tracing;

use crate::{
    angle_net::AngleNet,
    base_net::BaseNet,
    crnn_net::CrnnNet,
    db_net::DbNet,
    ocr_error::OcrError,
    ocr_result::{OcrResult, Point, TextBlock, TextBox},
    ocr_utils::OcrUtils,
    scale_param::ScaleParam,
};

#[derive(Debug)]
pub struct OcrLite {
    db_net: DbNet,
    angle_net: AngleNet,
    crnn_net: CrnnNet,
}

unsafe impl Send for OcrLite {}
unsafe impl Sync for OcrLite {}

impl Default for OcrLite {
    fn default() -> Self {
        Self::new()
    }
}

impl OcrLite {
    pub fn new() -> Self {
        Self {
            db_net: DbNet::new(),
            angle_net: AngleNet::new(),
            crnn_net: CrnnNet::new(),
        }
    }

    pub fn init_models(
        &mut self,
        det_path: &str,
        cls_path: &str,
        rec_path: &str,
        num_thread: usize,
    ) -> Result<(), OcrError> {
        self.db_net.init_model(det_path, num_thread, None)?;
        self.angle_net.init_model(cls_path, num_thread, None)?;
        self.crnn_net.init_model(rec_path, num_thread, None)?;
        Ok(())
    }

    pub fn init_models_with_dict(
        &mut self,
        det_path: &str,
        cls_path: &str,
        rec_path: &str,
        dict_path: &str,
        num_thread: usize,
    ) -> Result<(), OcrError> {
        self.db_net.init_model(det_path, num_thread, None)?;
        self.angle_net.init_model(cls_path, num_thread, None)?;
        self.crnn_net
            .init_model_dict_file(rec_path, num_thread, None, dict_path)?;
        Ok(())
    }

    pub fn init_models_custom(
        &mut self,
        det_path: &str,
        cls_path: &str,
        rec_path: &str,
        builder_fn: fn(SessionBuilder) -> Result<SessionBuilder, ort::Error>,
    ) -> Result<(), OcrError> {
        self.db_net.init_model(det_path, 0, Some(builder_fn))?;
        self.angle_net.init_model(cls_path, 0, Some(builder_fn))?;
        self.crnn_net.init_model(rec_path, 0, Some(builder_fn))?;
        Ok(())
    }

    /// Initialize models with dictionary file and custom session builder.
    ///
    /// Combines `init_models_with_dict` and `init_models_custom`: loads the
    /// dictionary for the recognition model while applying a custom ORT
    /// session builder (e.g. for GPU execution providers).
    pub fn init_models_with_dict_custom(
        &mut self,
        det_path: &str,
        cls_path: &str,
        rec_path: &str,
        dict_path: &str,
        num_thread: usize,
        builder_fn: Option<fn(SessionBuilder) -> Result<SessionBuilder, ort::Error>>,
    ) -> Result<(), OcrError> {
        self.db_net.init_model(det_path, num_thread, builder_fn)?;
        self.angle_net.init_model(cls_path, num_thread, builder_fn)?;
        self.crnn_net
            .init_model_dict_file(rec_path, num_thread, builder_fn, dict_path)?;
        Ok(())
    }

    pub fn init_models_from_memory(
        &mut self,
        det_bytes: &[u8],
        cls_bytes: &[u8],
        rec_bytes: &[u8],
        num_thread: usize,
    ) -> Result<(), OcrError> {
        self.db_net.init_model_from_memory(det_bytes, num_thread, None)?;
        self.angle_net.init_model_from_memory(cls_bytes, num_thread, None)?;
        self.crnn_net.init_model_from_memory(rec_bytes, num_thread, None)?;
        Ok(())
    }

    pub fn init_models_from_memory_custom(
        &mut self,
        det_bytes: &[u8],
        cls_bytes: &[u8],
        rec_bytes: &[u8],
        builder_fn: fn(SessionBuilder) -> Result<SessionBuilder, ort::Error>,
    ) -> Result<(), OcrError> {
        self.db_net.init_model_from_memory(det_bytes, 0, Some(builder_fn))?;
        self.angle_net.init_model_from_memory(cls_bytes, 0, Some(builder_fn))?;
        self.crnn_net.init_model_from_memory(rec_bytes, 0, Some(builder_fn))?;
        Ok(())
    }

    fn detect_base(
        &self,
        img_src: &image::RgbImage,
        padding: u32,
        max_side_len: u32,
        box_score_thresh: f32,
        box_thresh: f32,
        un_clip_ratio: f32,
        do_angle: bool,
        most_angle: bool,
        angle_rollback: bool,
        angle_rollback_threshold: f32,
        cls_thresh: f32,
        thresh: f32,
        rec_batch_size: u32,
    ) -> Result<OcrResult, OcrError> {
        tracing::debug!(
            width = img_src.width(),
            height = img_src.height(),
            padding = padding,
            max_side_len = max_side_len,
            do_angle = do_angle,
            angle_rollback = angle_rollback,
            "PaddleOCR: starting detection"
        );

        let origin_max_side = img_src.width().max(img_src.height());
        let mut resize;
        if max_side_len == 0 || max_side_len > origin_max_side {
            resize = origin_max_side;
        } else {
            resize = max_side_len;
        }
        resize += 2 * padding;

        let padding_src = OcrUtils::make_padding(img_src, padding)?;

        let scale = ScaleParam::get_scale_param(&padding_src, resize);
        tracing::debug!(
            resize_width = scale.dst_width,
            resize_height = scale.dst_height,
            "PaddleOCR: image resized"
        );

        self.detect_once(
            &padding_src,
            &scale,
            padding,
            box_score_thresh,
            box_thresh,
            un_clip_ratio,
            do_angle,
            most_angle,
            angle_rollback,
            angle_rollback_threshold,
            cls_thresh,
            thresh,
            rec_batch_size,
        )
    }

    /// Detect text in image
    ///
    /// # Arguments
    ///
    /// - `img_src` - Input image
    /// - `padding` - Padding width added during image transformation (improves detection)
    /// - `max_side_len` - Maximum side length after transformation (larger images will be scaled down)
    /// - `box_score_thresh` - Score threshold for text region detection
    /// - `box_thresh` - Box threshold
    /// - `un_clip_ratio` - Unclip ratio
    /// - `do_angle` - Whether to perform angle detection
    /// - `most_angle` - Use most common angle for all text regions
    const DEFAULT_CLS_THRESH: f32 = 0.9;
    const DEFAULT_THRESH: f32 = 0.3;
    const DEFAULT_REC_BATCH_SIZE: u32 = 6;

    pub fn detect(
        &self,
        img_src: &image::RgbImage,
        padding: u32,
        max_side_len: u32,
        box_score_thresh: f32,
        box_thresh: f32,
        un_clip_ratio: f32,
        do_angle: bool,
        most_angle: bool,
    ) -> Result<OcrResult, OcrError> {
        self.detect_base(
            img_src,
            padding,
            max_side_len,
            box_score_thresh,
            box_thresh,
            un_clip_ratio,
            do_angle,
            most_angle,
            false,
            0.0,
            Self::DEFAULT_CLS_THRESH,
            Self::DEFAULT_THRESH,
            Self::DEFAULT_REC_BATCH_SIZE,
        )
    }

    /// Detect text with a configurable recognition batch size.
    #[allow(clippy::too_many_arguments)]
    pub fn detect_with_rec_batch_size(
        &self,
        img_src: &image::RgbImage,
        padding: u32,
        max_side_len: u32,
        box_score_thresh: f32,
        box_thresh: f32,
        un_clip_ratio: f32,
        do_angle: bool,
        most_angle: bool,
        rec_batch_size: u32,
    ) -> Result<OcrResult, OcrError> {
        self.detect_base(
            img_src,
            padding,
            max_side_len,
            box_score_thresh,
            box_thresh,
            un_clip_ratio,
            do_angle,
            most_angle,
            false,
            0.0,
            Self::DEFAULT_CLS_THRESH,
            Self::DEFAULT_THRESH,
            rec_batch_size,
        )
    }

    /// Detect text with angle rollback support
    ///
    /// When `do_angle` is true, if the image was angle-corrected but recognition
    /// result is poor, the angle correction will be reverted.
    ///
    /// # Arguments
    ///
    /// - `img_src` - Input image
    /// - `padding` - Padding width added during image transformation
    /// - `max_side_len` - Maximum side length after transformation
    /// - `box_score_thresh` - Score threshold for text region detection
    /// - `box_thresh` - Box threshold
    /// - `un_clip_ratio` - Unclip ratio
    /// - `do_angle` - Whether to perform angle detection
    /// - `most_angle` - Use most common angle
    /// - `angle_rollback_threshold` - If text score is below this value (or NaN), angle correction is reverted
    pub fn detect_angle_rollback(
        &self,
        img_src: &image::RgbImage,
        padding: u32,
        max_side_len: u32,
        box_score_thresh: f32,
        box_thresh: f32,
        un_clip_ratio: f32,
        do_angle: bool,
        most_angle: bool,
        angle_rollback_threshold: f32,
    ) -> Result<OcrResult, OcrError> {
        self.detect_base(
            img_src,
            padding,
            max_side_len,
            box_score_thresh,
            box_thresh,
            un_clip_ratio,
            do_angle,
            most_angle,
            true,
            angle_rollback_threshold,
            Self::DEFAULT_CLS_THRESH,
            Self::DEFAULT_THRESH,
            Self::DEFAULT_REC_BATCH_SIZE,
        )
    }

    pub fn detect_from_path(
        &self,
        img_path: &str,
        padding: u32,
        max_side_len: u32,
        box_score_thresh: f32,
        box_thresh: f32,
        un_clip_ratio: f32,
        do_angle: bool,
        most_angle: bool,
    ) -> Result<OcrResult, OcrError> {
        let img_src = image::open(img_path)?.to_rgb8();

        self.detect(
            &img_src,
            padding,
            max_side_len,
            box_score_thresh,
            box_thresh,
            un_clip_ratio,
            do_angle,
            most_angle,
        )
    }

    /// Sort text boxes in reading order: top-to-bottom, left-to-right.
    ///
    /// Sorts by top-left Y coordinate first, then by top-left X coordinate within
    /// the same Y. Matches PaddleOCR Python's `sorted_boxes` primary ordering.
    fn sort_text_boxes(text_boxes: &mut [TextBox]) {
        text_boxes.sort_by(|a, b| {
            let ay = a.points.first().map_or(0, |p| p.y);
            let ax = a.points.first().map_or(0, |p| p.x);
            let by = b.points.first().map_or(0, |p| p.y);
            let bx = b.points.first().map_or(0, |p| p.x);
            (ay, ax).cmp(&(by, bx))
        });
    }

    fn detect_once(
        &self,
        img_src: &image::RgbImage,
        scale: &ScaleParam,
        padding: u32,
        box_score_thresh: f32,
        box_thresh: f32,
        un_clip_ratio: f32,
        do_angle: bool,
        most_angle: bool,
        angle_rollback: bool,
        angle_rollback_threshold: f32,
        cls_thresh: f32,
        thresh: f32,
        rec_batch_size: u32,
    ) -> Result<OcrResult, OcrError> {
        tracing::debug!("PaddleOCR: running DB-net text detection");
        let mut text_boxes =
            self.db_net
                .get_text_boxes(img_src, scale, box_score_thresh, box_thresh, un_clip_ratio, thresh)?;

        tracing::debug!(
            num_boxes = text_boxes.len(),
            min_score = text_boxes.iter().map(|b| b.score).fold(f32::INFINITY, f32::min),
            max_score = text_boxes.iter().map(|b| b.score).fold(f32::NEG_INFINITY, f32::max),
            "PaddleOCR: detection complete"
        );

        Self::sort_text_boxes(&mut text_boxes);

        let part_images = OcrUtils::get_part_images(img_src, &text_boxes);

        if do_angle {
            tracing::debug!(num_regions = part_images.len(), "PaddleOCR: running angle classifier");
        }

        let angles = self
            .angle_net
            .get_angles(&part_images, do_angle, most_angle, cls_thresh)?;

        if do_angle {
            let rotated_count = angles.iter().filter(|a| a.index != 0).count();
            tracing::debug!(
                rotated_regions = rotated_count,
                "PaddleOCR: angle classification complete"
            );
        }

        let mut rotated_images: Vec<image::RgbImage> = Vec::with_capacity(part_images.len());

        let mut angle_rollback_records = HashMap::<usize, ImageBuffer<image::Rgb<u8>, Vec<u8>>>::new();

        for (index, (angle, mut part_image)) in angles.iter().zip(part_images).enumerate() {
            if angle.index == 1 {
                if angle_rollback {
                    angle_rollback_records.insert(index, part_image.clone());
                }

                OcrUtils::mat_rotate_clock_wise_180(&mut part_image);
            }
            rotated_images.push(part_image);
        }

        tracing::debug!(
            num_regions = rotated_images.len(),
            "PaddleOCR: running CRNN text recognition"
        );
        let text_lines = self.crnn_net.get_text_lines(
            &rotated_images,
            &angle_rollback_records,
            angle_rollback_threshold,
            rec_batch_size,
        )?;

        if text_lines.is_empty() {
            tracing::warn!("PaddleOCR: no text recognized");
        } else {
            let avg_score = text_lines.iter().map(|l| l.text_score).sum::<f32>() / text_lines.len() as f32;
            let empty_count = text_lines.iter().filter(|l| l.text.trim().is_empty()).count();
            tracing::debug!(
                num_lines = text_lines.len(),
                avg_score = avg_score,
                empty_lines = empty_count,
                "PaddleOCR: recognition complete"
            );
        }

        let mut text_blocks = Vec::with_capacity(text_lines.len());
        for (i, text_line) in text_lines.into_iter().enumerate() {
            text_blocks.push(TextBlock {
                box_points: text_boxes[i]
                    .points
                    .iter()
                    .map(|p| Point {
                        x: ((p.x as f32) - padding as f32) as u32,
                        y: ((p.y as f32) - padding as f32) as u32,
                    })
                    .collect(),
                box_score: text_boxes[i].score,
                angle_index: angles[i].index,
                angle_score: angles[i].score,
                text: text_line.text,
                text_score: text_line.text_score,
            });
        }

        Ok(OcrResult { text_blocks })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ocr_result::TextBox;

    fn make_box(x: u32, y: u32) -> TextBox {
        TextBox {
            points: vec![
                Point { x, y },
                Point { x: x + 100, y },
                Point { x: x + 100, y: y + 20 },
                Point { x, y: y + 20 },
            ],
            score: 0.9,
        }
    }

    #[test]
    fn test_sort_text_boxes_top_to_bottom() {
        let mut boxes = vec![make_box(10, 100), make_box(10, 50), make_box(10, 10)];
        OcrLite::sort_text_boxes(&mut boxes);
        assert_eq!(boxes[0].points[0].y, 10);
        assert_eq!(boxes[1].points[0].y, 50);
        assert_eq!(boxes[2].points[0].y, 100);
    }

    #[test]
    fn test_sort_text_boxes_same_line_left_to_right() {
        let mut boxes = vec![make_box(200, 10), make_box(100, 10), make_box(50, 10)];
        OcrLite::sort_text_boxes(&mut boxes);
        assert_eq!(boxes[0].points[0].x, 50);
        assert_eq!(boxes[1].points[0].x, 100);
        assert_eq!(boxes[2].points[0].x, 200);
    }

    #[test]
    fn test_sort_text_boxes_multi_line() {
        let mut boxes = vec![
            make_box(300, 50),
            make_box(100, 100),
            make_box(50, 50),
            make_box(200, 100),
        ];
        OcrLite::sort_text_boxes(&mut boxes);

        assert_eq!(boxes[0].points[0].x, 50);
        assert_eq!(boxes[1].points[0].x, 300);
        assert_eq!(boxes[2].points[0].x, 100);
        assert_eq!(boxes[3].points[0].x, 200);
    }

    #[test]
    fn test_sort_text_boxes_empty() {
        let mut boxes: Vec<TextBox> = vec![];
        OcrLite::sort_text_boxes(&mut boxes);
        assert!(boxes.is_empty());
    }

    #[test]
    fn test_sort_text_boxes_single() {
        let mut boxes = vec![make_box(10, 20)];
        OcrLite::sort_text_boxes(&mut boxes);
        assert_eq!(boxes.len(), 1);
    }
}
