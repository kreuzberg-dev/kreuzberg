//! TATR (Table Transformer) table structure recognition model.
//!
//! Takes a cropped table image and outputs detected rows, columns, headers,
//! and spanning cells as bounding boxes for cell grid reconstruction.
//!
//! TATR is a DETR-based non-autoregressive object detection model that
//! simultaneously predicts all table structure elements in a single forward pass.
//!
//! Model: TATR (Table Transformer, DETR-based)
//! Input: `pixel_values` shape `[batch, 3, H, W]` f32 (variable size, DETR preprocessing)
//! Output 0: `logits`     shape `[batch, 125, 7]` f32 — class logits (7 classes)
//! Output 1: `pred_boxes` shape `[batch, 125, 4]` f32 — normalized (cx, cy, w, h)
//!
//! Classes (7): Table=0, Column=1, Row=2, ColumnHeader=3, ProjectedRowHeader=4,
//!              SpanningCell=5, NoObject=6

use image::RgbImage;
use ndarray::Array4;
use ort::{inputs, session::Session, value::Tensor};

use crate::layout::error::LayoutError;
use crate::layout::session::build_session;

/// DETR standard shortest-edge target.
const DETR_SHORT_EDGE: u32 = 800;

/// DETR standard longest-edge cap.
const DETR_LONG_EDGE: u32 = 1000;

/// ImageNet normalization mean (RGB channel order).
const IMAGENET_MEAN_RGB: [f32; 3] = [0.485, 0.456, 0.406];

/// ImageNet normalization std (RGB channel order).
const IMAGENET_STD_RGB: [f32; 3] = [0.229, 0.224, 0.225];

/// Number of TATR output classes (including NoObject).
const NUM_CLASSES: usize = 7;

/// Confidence threshold for row and column detections.
const CONF_THRESHOLD_ROW_COL: f32 = 0.3;

/// Confidence threshold for spanning cell detections.
const CONF_THRESHOLD_SPANNING: f32 = 0.5;

/// IoB threshold for NMS on **row** detections during cell grid construction.
///
/// A candidate detection is suppressed if more than this fraction of its area
/// overlaps with any already-kept detection.  The previous value of 0.1 was
/// too aggressive: TATR row predictions frequently overlap by a few pixels,
/// causing valid rows to be suppressed and merging their content.
/// 0.5 means "suppress only when the majority of the candidate is already
/// covered" — enough to remove true duplicates while preserving close but
/// distinct rows.
const NMS_IOB_THRESHOLD_ROWS: f32 = 0.5;

/// IoB threshold for NMS on **column** detections during cell grid construction.
///
/// Columns need a lower threshold than rows because narrow adjacent columns
/// (e.g. Q1, Q2 quarter headers) can have significant mutual IoB overlap
/// relative to their small width, causing valid columns to be suppressed and
/// merged. 0.3 preserves narrow adjacent columns while still removing true
/// duplicates.
const NMS_IOB_THRESHOLD_COLS: f32 = 0.3;

/// Minimum column width as a fraction of table width.
///
/// After NMS, columns narrower than this fraction of the total table width
/// are removed as noise. Prevents spurious thin column detections from
/// splitting the grid incorrectly.
const MIN_COL_WIDTH_FRAC: f32 = 0.01;

#[cfg_attr(alef, alef(skip))]
/// TATR object detection class labels.
///
/// The 7 classes output by the Table Transformer model. `NoObject` (class 6)
/// is the background/padding class and is filtered out during post-processing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TatrClass {
    /// Full table bounding box (class 0).
    Table,
    /// Table column (class 1).
    Column,
    /// Table row (class 2).
    Row,
    /// Column header row (class 3).
    ColumnHeader,
    /// Projected row header column (class 4).
    ProjectedRowHeader,
    /// Spanning cell covering multiple rows/columns (class 5).
    SpanningCell,
}

impl TatrClass {
    /// Map a raw class index (0..6) to a `TatrClass`.
    ///
    /// Returns `None` for class 6 (NoObject) and any out-of-range index.
    fn from_index(idx: usize) -> Option<Self> {
        match idx {
            0 => Some(Self::Table),
            1 => Some(Self::Column),
            2 => Some(Self::Row),
            3 => Some(Self::ColumnHeader),
            4 => Some(Self::ProjectedRowHeader),
            5 => Some(Self::SpanningCell),
            _ => None,
        }
    }
}
#[cfg_attr(alef, alef(skip))]
/// A single TATR detection result.
#[derive(Debug, Clone)]
pub struct TatrDetection {
    /// Bounding box in crop-pixel coordinates: `[x1, y1, x2, y2]`.
    pub bbox: [f32; 4],
    /// Detection confidence score (0.0..1.0).
    pub confidence: f32,
    /// Detected class.
    pub class_name: TatrClass,
}
#[cfg_attr(alef, alef(skip))]
/// Aggregated TATR recognition result with detections separated by class.
#[derive(Debug, Clone)]
pub struct TatrResult {
    /// Detected rows, sorted top-to-bottom by `y2`.
    pub rows: Vec<TatrDetection>,
    /// Detected columns, sorted left-to-right by `x2`.
    pub columns: Vec<TatrDetection>,
    /// Detected headers (ColumnHeader and ProjectedRowHeader).
    pub headers: Vec<TatrDetection>,
    /// Detected spanning cells.
    pub spanning: Vec<TatrDetection>,
    /// The model's own detected `Table` bbox (highest-confidence one), in
    /// crop-pixel coordinates. `None` if the model produced no `Table`
    /// detection above threshold (xberg-io/xberg#193).
    pub table_bbox: Option<[f32; 4]>,
}
/// A cell bounding box within the reconstructed table grid (pixel coordinates in the crop).
#[cfg_attr(alef, alef(skip))]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CellBBox {
    /// Left edge in crop-pixel coordinates.
    pub x1: f32,
    /// Top edge in crop-pixel coordinates.
    pub y1: f32,
    /// Right edge in crop-pixel coordinates.
    pub x2: f32,
    /// Bottom edge in crop-pixel coordinates.
    pub y2: f32,
}

/// TATR (Table Transformer) table structure recognition model.
///
/// Wraps an ORT session for the TATR ONNX model and provides preprocessing,
/// inference, and post-processing in a single `recognize` call.
#[cfg_attr(alef, alef(skip))]
pub struct TatrModel {
    session: Session,
    input_name: String,
}

impl TatrModel {
    /// Load a TATR ONNX model from a file path.
    ///
    /// Uses the default execution provider selection from `build_session`
    /// with a CPU-only fallback if the platform EP fails.
    pub(crate) fn from_file(
        path: &str,
        accel: Option<&crate::core::config::acceleration::AccelerationConfig>,
        thread_budget: usize,
    ) -> Result<Self, LayoutError> {
        let thread_budget = thread_budget.max(1);
        let session = match build_session(path, accel, thread_budget) {
            Ok(s) => s,
            Err(first_err) => {
                tracing::warn!("TATR: platform EP failed ({first_err}), retrying with CPU-only");
                match Self::build_cpu_session(path, thread_budget) {
                    Ok(s) => s,
                    Err(cpu_err) => {
                        tracing::warn!("TATR: CPU-only also failed: {cpu_err}");
                        return Err(cpu_err);
                    }
                }
            }
        };
        let input_name = session.inputs()[0].name().to_string();

        Ok(Self { session, input_name })
    }

    /// Build a CPU-only ORT session (no CoreML/CUDA).
    fn build_cpu_session(path: &str, thread_budget: usize) -> Result<Session, LayoutError> {
        use ort::session::builder::GraphOptimizationLevel;
        let mut builder = Session::builder()?
            .with_optimization_level(GraphOptimizationLevel::All)
            .map_err(|e| LayoutError::Ort(ort::Error::new(e.message())))?
            .with_intra_threads(thread_budget)
            .map_err(|e| LayoutError::Ort(ort::Error::new(e.message())))?
            .with_inter_threads(1)
            .map_err(|e| LayoutError::Ort(ort::Error::new(e.message())))?;
        Ok(builder.commit_from_file(path)?)
    }

    /// Recognize table structure from a cropped table image.
    ///
    /// Returns a [`TatrResult`] with detected rows, columns, headers, and
    /// spanning cells in the input image's pixel coordinate space.
    pub(crate) fn recognize(&mut self, table_img: &RgbImage) -> Result<TatrResult, LayoutError> {
        let img_w = table_img.width() as f32;
        let img_h = table_img.height() as f32;

        let (input_tensor, resized_w, resized_h) = preprocess_detr(table_img);
        let tensor = Tensor::from_array(input_tensor)?;

        let outputs = self.session.run(inputs![
            self.input_name.as_str() => tensor
        ])?;

        let mut float_outputs: Vec<(Vec<usize>, Vec<f32>)> = Vec::new();
        for (_name, value) in outputs.iter() {
            if let Ok(view) = value.try_extract_tensor::<f32>() {
                let shape: Vec<usize> = view.0.iter().map(|&d| d as usize).collect();
                let data: Vec<f32> = view.1.to_vec();
                float_outputs.push((shape, data));
            }
        }

        if float_outputs.len() < 2 {
            return Err(LayoutError::InvalidOutput(format!(
                "TATR expected 2 float outputs, got {}",
                float_outputs.len()
            )));
        }

        let (logits_shape, logits_data, boxes_shape, boxes_data) = if float_outputs[0].0.last() == Some(&NUM_CLASSES) {
            let (ls, ld) = float_outputs.remove(0);
            let (bs, bd) = float_outputs.remove(0);
            (ls, ld, bs, bd)
        } else {
            let (bs, bd) = float_outputs.remove(0);
            let (ls, ld) = float_outputs.remove(0);
            (ls, ld, bs, bd)
        };

        let num_queries = logits_shape.get(1).copied().unwrap_or(0);
        let num_classes = logits_shape.last().copied().unwrap_or(0);
        let box_dim = boxes_shape.last().copied().unwrap_or(0);

        if num_queries == 0 || num_classes < NUM_CLASSES || box_dim < 4 {
            return Ok(TatrResult {
                rows: Vec::new(),
                columns: Vec::new(),
                headers: Vec::new(),
                spanning: Vec::new(),
                table_bbox: None,
            });
        }

        let mut rows = Vec::new();
        let mut columns = Vec::new();
        let mut headers = Vec::new();
        let mut spanning = Vec::new();
        let mut table_bbox: Option<[f32; 4]> = None;
        let mut table_confidence = 0.0f32;

        for q in 0..num_queries {
            let logit_offset = q * num_classes;
            let logits_slice = &logits_data[logit_offset..logit_offset + num_classes];

            let (class_idx, confidence) = softmax_argmax(logits_slice);

            let class = match TatrClass::from_index(class_idx) {
                Some(c) => c,
                None => continue,
            };

            let threshold = match class {
                TatrClass::SpanningCell => CONF_THRESHOLD_SPANNING,
                TatrClass::Table => CONF_THRESHOLD_ROW_COL,
                _ => CONF_THRESHOLD_ROW_COL,
            };
            if confidence < threshold {
                continue;
            }

            let box_offset = q * box_dim;
            let cx = boxes_data[box_offset];
            let cy = boxes_data[box_offset + 1];
            let w = boxes_data[box_offset + 2];
            let h = boxes_data[box_offset + 3];

            let bbox = cxcywh_to_xyxy(cx, cy, w, h, resized_w as f32, resized_h as f32);

            let scale_x = img_w / resized_w as f32;
            let scale_y = img_h / resized_h as f32;
            let bbox = [
                (bbox[0] * scale_x).clamp(0.0, img_w),
                (bbox[1] * scale_y).clamp(0.0, img_h),
                (bbox[2] * scale_x).clamp(0.0, img_w),
                (bbox[3] * scale_y).clamp(0.0, img_h),
            ];

            let detection = TatrDetection {
                bbox,
                confidence,
                class_name: class,
            };

            match class {
                TatrClass::Row => rows.push(detection),
                TatrClass::Column => columns.push(detection),
                TatrClass::ColumnHeader | TatrClass::ProjectedRowHeader => {
                    headers.push(detection);
                }
                TatrClass::SpanningCell => spanning.push(detection),
                // Previously dropped: the model's own Table bbox is a more
                // precise localization of the table within the crop than the
                // full crop extent, and callers had no way to widen columns
                // to it (xberg-io/xberg#193). Keep the highest-confidence one.
                TatrClass::Table => {
                    if detection.confidence > table_confidence {
                        table_confidence = detection.confidence;
                        table_bbox = Some(detection.bbox);
                    }
                }
            }
        }

        rows.sort_by(|a, b| a.bbox[3].total_cmp(&b.bbox[3]));
        columns.sort_by(|a, b| a.bbox[2].total_cmp(&b.bbox[2]));

        Ok(TatrResult {
            rows,
            columns,
            headers,
            spanning,
            table_bbox,
        })
    }
}

/// Preprocess an image using DETR-standard preprocessing.
///
/// Pipeline:
/// 1. Resize: scale shortest edge to 800px, cap longest edge at 1000px (aspect-preserving)
/// 2. Normalize: ImageNet mean/std in RGB channel order
/// 3. Layout: NCHW `[1, 3, H, W]` f32
///
/// Returns `(tensor, resized_width, resized_height)`.
fn preprocess_detr(img: &RgbImage) -> (Array4<f32>, u32, u32) {
    let (orig_w, orig_h) = (img.width(), img.height());
    let (new_w, new_h) = compute_detr_resize(orig_w, orig_h);

    let resized = image::imageops::resize(img, new_w, new_h, image::imageops::FilterType::Triangle);

    let w = new_w as usize;
    let h = new_h as usize;
    let hw = h * w;

    let inv_std_r = 1.0 / IMAGENET_STD_RGB[0];
    let inv_std_g = 1.0 / IMAGENET_STD_RGB[1];
    let inv_std_b = 1.0 / IMAGENET_STD_RGB[2];

    let mut data = vec![0.0f32; 3 * hw];
    let pixels = resized.as_raw();

    for y in 0..h {
        for x in 0..w {
            let src_idx = (y * w + x) * 3;
            let dst_idx = y * w + x;
            let r = pixels[src_idx] as f32 * (1.0 / 255.0);
            let g = pixels[src_idx + 1] as f32 * (1.0 / 255.0);
            let b = pixels[src_idx + 2] as f32 * (1.0 / 255.0);
            data[dst_idx] = (r - IMAGENET_MEAN_RGB[0]) * inv_std_r;
            data[hw + dst_idx] = (g - IMAGENET_MEAN_RGB[1]) * inv_std_g;
            data[2 * hw + dst_idx] = (b - IMAGENET_MEAN_RGB[2]) * inv_std_b;
        }
    }

    let tensor = Array4::from_shape_vec((1, 3, h, w), data).expect("shape mismatch in preprocess_detr");

    (tensor, new_w, new_h)
}

/// Compute DETR resize dimensions.
///
/// Scales shortest edge to [`DETR_SHORT_EDGE`] (800), then caps longest edge
/// at [`DETR_LONG_EDGE`] (1000), maintaining aspect ratio.
fn compute_detr_resize(orig_w: u32, orig_h: u32) -> (u32, u32) {
    let short = u64::from(orig_w.min(orig_h));
    let long = u64::from(orig_w.max(orig_h));
    if short == 0 {
        return (orig_w.max(1), orig_h.max(1));
    }

    // Match Hugging Face's `get_resize_output_image_size` exactly: compute and
    // truncate the tentative long edge first, then use that truncated value
    // when applying the long-edge cap. Collapsing this into one ratio causes
    // one-pixel drift for some dimensions. ~keep
    let requested_short = u64::from(DETR_SHORT_EDGE);
    let requested_long = requested_short * long / short;
    let (new_short, new_long) = if requested_long > u64::from(DETR_LONG_EDGE) {
        (
            u64::from(DETR_LONG_EDGE) * requested_short / requested_long,
            u64::from(DETR_LONG_EDGE),
        )
    } else {
        (requested_short, requested_long)
    };

    if orig_w <= orig_h {
        (new_short.max(1) as u32, new_long.max(1) as u32)
    } else {
        (new_long.max(1) as u32, new_short.max(1) as u32)
    }
}

/// Softmax over a slice, returning `(argmax_index, max_probability)`.
fn softmax_argmax(logits: &[f32]) -> (usize, f32) {
    let max_val = logits.iter().copied().fold(f32::NEG_INFINITY, f32::max);

    let mut sum = 0.0f32;
    let mut probs = Vec::with_capacity(logits.len());
    for &v in logits {
        let e = (v - max_val).exp();
        probs.push(e);
        sum += e;
    }

    let inv_sum = 1.0 / sum;
    let mut best_idx = 0;
    let mut best_prob = 0.0f32;
    for (i, p) in probs.iter().enumerate() {
        let prob = p * inv_sum;
        if prob > best_prob {
            best_prob = prob;
            best_idx = i;
        }
    }

    (best_idx, best_prob)
}

/// Convert normalized center-format box `(cx, cy, w, h)` to pixel `[x1, y1, x2, y2]`.
///
/// The input values are in `[0, 1]` normalized space; they are scaled by
/// `img_w` and `img_h` to produce pixel coordinates.
fn cxcywh_to_xyxy(cx: f32, cy: f32, w: f32, h: f32, img_w: f32, img_h: f32) -> [f32; 4] {
    let px_cx = cx * img_w;
    let px_cy = cy * img_h;
    let px_w = w * img_w;
    let px_h = h * img_h;

    let x1 = px_cx - px_w / 2.0;
    let y1 = px_cy - px_h / 2.0;
    let x2 = px_cx + px_w / 2.0;
    let y2 = px_cy + px_h / 2.0;

    [x1.max(0.0), y1.max(0.0), x2.max(0.0), y2.max(0.0)]
}

/// Intersection-over-Box: `intersection_area(a, b) / area(a)`.
///
/// Measures what fraction of box `a` is covered by box `b`.
/// Returns 0.0 if `a` has zero area.
fn iob(a: [f32; 4], b: [f32; 4]) -> f32 {
    let area_a = (a[2] - a[0]).max(0.0) * (a[3] - a[1]).max(0.0);
    if area_a <= 0.0 {
        return 0.0;
    }

    let ix1 = a[0].max(b[0]);
    let iy1 = a[1].max(b[1]);
    let ix2 = a[2].min(b[2]);
    let iy2 = a[3].min(b[3]);
    let inter = (ix2 - ix1).max(0.0) * (iy2 - iy1).max(0.0);

    inter / area_a
}

/// Build a 2D cell grid from TATR detections.
///
/// The grid is `[num_rows][num_cols]` where each cell is the intersection
/// of a row bounding box and a column bounding box.
///
/// Processing steps:
/// 1. Widen all rows to span the full table width (min x1 to max x2 across rows)
/// 2. Apply NMS using IoB: sort by confidence descending, remove detections
///    whose IoB with any higher-confidence detection exceeds [`NMS_IOB_THRESHOLD`]
/// 3. For each (row, column) pair, compute the intersection rectangle
///
/// If `table_bbox` is provided, it is used to clip the row widening bounds.
pub(crate) fn build_cell_grid(result: &TatrResult, table_bbox: Option<[f32; 4]>) -> Vec<Vec<CellBBox>> {
    build_cell_grid_with_structure(result, table_bbox).0
}

/// Header-row and merged-span metadata for a cell grid, derived from TATR's
/// `headers`/`spanning` detections (xberg-io/xberg#176).
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct TableStructure {
    /// Number of leading grid rows that overlap a TATR header detection
    /// (`ColumnHeader` or `ProjectedRowHeader`). `0` means TATR detected no
    /// header rows at all; callers should fall back to their own default.
    pub(crate) header_row_count: usize,
    /// Grid-index ranges `(row_start, row_end_exclusive, col_start,
    /// col_end_exclusive)` covered by a single TATR spanning-cell detection.
    pub(crate) spans: Vec<(usize, usize, usize, usize)>,
}

/// IoB threshold for associating a grid row/column with a header or spanning
/// detection: more than half the row/column must be covered.
const STRUCTURE_IOB_THRESHOLD: f32 = 0.5;

/// Same as [`build_cell_grid`], but also returns the [`TableStructure`]
/// (header rows + merged spans) derived from `result.headers` and
/// `result.spanning` against the same row/column bands used to build the
/// grid.
pub(crate) fn build_cell_grid_with_structure(
    result: &TatrResult,
    table_bbox: Option<[f32; 4]>,
) -> (Vec<Vec<CellBBox>>, TableStructure) {
    if result.rows.is_empty() || result.columns.is_empty() {
        return (Vec::new(), TableStructure::default());
    }

    let (table_x1, table_x2) = if let Some(tb) = table_bbox {
        (tb[0], tb[2])
    } else {
        let min_x1 = result.rows.iter().map(|r| r.bbox[0]).fold(f32::INFINITY, f32::min);
        let max_x2 = result.rows.iter().map(|r| r.bbox[2]).fold(f32::NEG_INFINITY, f32::max);
        (min_x1, max_x2)
    };

    let widened_rows: Vec<[f32; 4]> = result
        .rows
        .iter()
        .map(|r| [table_x1, r.bbox[1], table_x2, r.bbox[3]])
        .collect();

    let mut nms_rows = nms_by_iob(&result.rows, &widened_rows, NMS_IOB_THRESHOLD_ROWS);
    nms_rows.sort_by(|a, b| a[1].total_cmp(&b[1]));

    let col_bboxes: Vec<[f32; 4]> = result.columns.iter().map(|c| c.bbox).collect();
    let mut nms_cols = nms_by_iob(&result.columns, &col_bboxes, NMS_IOB_THRESHOLD_COLS);

    let table_width = table_x2 - table_x1;
    if table_width > 0.0 {
        let min_col_width = table_width * MIN_COL_WIDTH_FRAC;
        nms_cols.retain(|col| (col[2] - col[0]) >= min_col_width);
    }

    nms_cols.sort_by(|a, b| a[0].total_cmp(&b[0]));

    let mut grid = Vec::with_capacity(nms_rows.len());
    for row_bbox in &nms_rows {
        let mut row_cells = Vec::with_capacity(nms_cols.len());
        for col_bbox in &nms_cols {
            let cell = intersect_boxes(*row_bbox, *col_bbox);
            row_cells.push(cell);
        }
        grid.push(row_cells);
    }

    let structure = TableStructure {
        header_row_count: compute_header_row_count(&result.headers, &nms_rows),
        spans: compute_spans(&result.spanning, &nms_rows, &nms_cols),
    };

    (grid, structure)
}

/// Fraction of the `[lo, hi]` extent covered by `[other_lo, other_hi]`.
///
/// Row bands are widened to the full table width and column bands to the
/// full table height, but the perpendicular axis is *not* normalized that
/// way — a header or spanning-cell detection typically covers only part of
/// a column's height (or a row's width). Projecting onto the single
/// perpendicular axis of each band, rather than computing 2D IoB against the
/// detection's whole box, avoids that axis's mismatch swamping the overlap
/// fraction.
fn axis_overlap_fraction(lo: f32, hi: f32, other_lo: f32, other_hi: f32) -> f32 {
    let extent = hi - lo;
    if extent <= 0.0 {
        return 0.0;
    }
    let overlap = (hi.min(other_hi) - lo.max(other_lo)).max(0.0);
    overlap / extent
}

/// Count the leading grid rows that overlap a header detection, stopping at
/// the first row that does not — TATR's header rows are always the topmost
/// rows of the table, so a single leading run is the correct interpretation
/// even for multi-row headers.
fn compute_header_row_count(headers: &[TatrDetection], rows: &[[f32; 4]]) -> usize {
    let mut count = 0;
    for row in rows {
        let is_header = headers
            .iter()
            .any(|h| axis_overlap_fraction(row[1], row[3], h.bbox[1], h.bbox[3]) > STRUCTURE_IOB_THRESHOLD);
        if !is_header {
            break;
        }
        count += 1;
    }
    count
}

/// Map each TATR spanning-cell detection onto the grid-index rectangle it
/// covers, based on axis overlap with the row/column bands used to build the
/// grid. Detections that resolve to a single cell (no actual merge) are
/// skipped.
fn compute_spans(
    spanning: &[TatrDetection],
    rows: &[[f32; 4]],
    cols: &[[f32; 4]],
) -> Vec<(usize, usize, usize, usize)> {
    let mut spans = Vec::new();
    for span in spanning {
        let row_indices: Vec<usize> = rows
            .iter()
            .enumerate()
            .filter(|(_, r)| axis_overlap_fraction(r[1], r[3], span.bbox[1], span.bbox[3]) > STRUCTURE_IOB_THRESHOLD)
            .map(|(i, _)| i)
            .collect();
        let col_indices: Vec<usize> = cols
            .iter()
            .enumerate()
            .filter(|(_, c)| axis_overlap_fraction(c[0], c[2], span.bbox[0], span.bbox[2]) > STRUCTURE_IOB_THRESHOLD)
            .map(|(i, _)| i)
            .collect();

        if row_indices.len() * col_indices.len() <= 1 {
            continue;
        }

        let (Some(&row_start), Some(&row_last)) = (row_indices.first(), row_indices.last()) else {
            continue;
        };
        let (Some(&col_start), Some(&col_last)) = (col_indices.first(), col_indices.last()) else {
            continue;
        };
        spans.push((row_start, row_last + 1, col_start, col_last + 1));
    }
    spans
}

/// Apply NMS using IoB (Intersection over Box) metric.
///
/// Sort detections by confidence descending, then greedily keep detections
/// whose IoB with all previously kept detections is below the threshold.
///
/// `bboxes` are the (possibly widened) bounding boxes corresponding 1:1
/// with `detections`. `threshold` is the IoB suppression threshold.
fn nms_by_iob(detections: &[TatrDetection], bboxes: &[[f32; 4]], threshold: f32) -> Vec<[f32; 4]> {
    let mut indices: Vec<usize> = (0..detections.len()).collect();
    indices.sort_by(|&a, &b| detections[b].confidence.total_cmp(&detections[a].confidence));

    let mut kept: Vec<[f32; 4]> = Vec::new();

    for &idx in &indices {
        let candidate = bboxes[idx];
        let suppressed = kept.iter().any(|&kept_box| iob(candidate, kept_box) > threshold);
        if !suppressed {
            kept.push(candidate);
        }
    }

    kept
}

/// Compute the intersection rectangle of two axis-aligned bounding boxes.
///
/// If the boxes do not overlap, the resulting `CellBBox` will have
/// `x1 >= x2` or `y1 >= y2` (zero-area cell).
fn intersect_boxes(a: [f32; 4], b: [f32; 4]) -> CellBBox {
    CellBBox {
        x1: a[0].max(b[0]),
        y1: a[1].max(b[1]),
        x2: a[2].min(b[2]),
        y2: a[3].min(b[3]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compute_detr_resize_landscape() {
        let (w, h) = compute_detr_resize(1600, 1200);
        assert_eq!((w, h), (1000, 750));
    }

    #[test]
    fn test_compute_detr_resize_portrait() {
        let (w, h) = compute_detr_resize(600, 1000);
        assert_eq!((w, h), (600, 1000));
    }

    #[test]
    fn test_compute_detr_resize_very_elongated() {
        let (w, h) = compute_detr_resize(100, 3000);
        assert_eq!((w, h), (33, 1000));
    }

    #[test]
    fn test_compute_detr_resize_square() {
        let (w, h) = compute_detr_resize(800, 800);
        assert_eq!(w, 800);
        assert_eq!(h, 800);
    }

    #[test]
    fn test_compute_detr_resize_truncates_like_hugging_face() {
        assert_eq!(compute_detr_resize(102, 101), (807, 800));
        assert_eq!(compute_detr_resize(6, 17), (353, 1000));
    }

    #[test]
    fn test_compute_detr_resize_small() {
        let (w, h) = compute_detr_resize(200, 300);
        assert_eq!((w, h), (666, 1000));
    }

    #[test]
    fn test_cxcywh_to_xyxy_center() {
        let bbox = cxcywh_to_xyxy(0.5, 0.5, 0.5, 0.5, 100.0, 100.0);
        assert!((bbox[0] - 25.0).abs() < 1e-5, "x1={}", bbox[0]);
        assert!((bbox[1] - 25.0).abs() < 1e-5, "y1={}", bbox[1]);
        assert!((bbox[2] - 75.0).abs() < 1e-5, "x2={}", bbox[2]);
        assert!((bbox[3] - 75.0).abs() < 1e-5, "y2={}", bbox[3]);
    }

    #[test]
    fn test_cxcywh_to_xyxy_top_left() {
        let bbox = cxcywh_to_xyxy(0.5, 0.5, 1.0, 1.0, 200.0, 100.0);
        assert!((bbox[0] - 0.0).abs() < 1e-5);
        assert!((bbox[1] - 0.0).abs() < 1e-5);
        assert!((bbox[2] - 200.0).abs() < 1e-5);
        assert!((bbox[3] - 100.0).abs() < 1e-5);
    }

    #[test]
    fn test_cxcywh_to_xyxy_clamps_negative() {
        let bbox = cxcywh_to_xyxy(0.0, 0.0, 0.5, 0.5, 100.0, 100.0);
        assert_eq!(bbox[0], 0.0, "x1 should be clamped to 0");
        assert_eq!(bbox[1], 0.0, "y1 should be clamped to 0");
    }

    #[test]
    fn test_softmax_argmax_clear_winner() {
        let logits = [0.0, 0.0, 10.0, 0.0, 0.0, 0.0, 0.0];
        let (idx, prob) = softmax_argmax(&logits);
        assert_eq!(idx, 2);
        assert!(prob > 0.99, "confidence should be ~1.0, got {prob}");
    }

    #[test]
    fn test_softmax_argmax_uniform() {
        let logits = [1.0; 7];
        let (_, prob) = softmax_argmax(&logits);
        assert!(
            (prob - 1.0 / 7.0).abs() < 1e-5,
            "uniform logits should give ~1/7 confidence, got {prob}"
        );
    }

    #[test]
    fn test_softmax_argmax_negative() {
        let logits = [-10.0, -5.0, -1.0, -20.0, -30.0, -2.0, -100.0];
        let (idx, _) = softmax_argmax(&logits);
        assert_eq!(idx, 2, "should pick the least negative");
    }

    #[test]
    fn test_iob_full_containment() {
        let a = [10.0, 10.0, 20.0, 20.0];
        let b = [0.0, 0.0, 100.0, 100.0];
        let result = iob(a, b);
        assert!((result - 1.0).abs() < 1e-5, "fully contained → IoB=1.0, got {result}");
    }

    #[test]
    fn test_iob_no_overlap() {
        let a = [0.0, 0.0, 10.0, 10.0];
        let b = [20.0, 20.0, 30.0, 30.0];
        let result = iob(a, b);
        assert_eq!(result, 0.0);
    }

    #[test]
    fn test_iob_partial_overlap() {
        let a = [0.0, 0.0, 10.0, 10.0];
        let b = [5.0, 0.0, 15.0, 10.0];
        let result = iob(a, b);
        assert!((result - 0.5).abs() < 1e-5, "expected 0.5, got {result}");
    }

    #[test]
    fn test_iob_zero_area() {
        let a = [5.0, 5.0, 5.0, 5.0];
        let b = [0.0, 0.0, 10.0, 10.0];
        let result = iob(a, b);
        assert_eq!(result, 0.0, "zero-area box should return 0.0");
    }

    #[test]
    fn test_nms_suppresses_overlapping() {
        let detections = vec![
            TatrDetection {
                bbox: [0.0, 0.0, 100.0, 20.0],
                confidence: 0.9,
                class_name: TatrClass::Row,
            },
            TatrDetection {
                bbox: [0.0, 2.0, 100.0, 22.0],
                confidence: 0.7,
                class_name: TatrClass::Row,
            },
        ];
        let bboxes: Vec<[f32; 4]> = detections.iter().map(|d| d.bbox).collect();
        let kept = nms_by_iob(&detections, &bboxes, NMS_IOB_THRESHOLD_ROWS);
        assert_eq!(kept.len(), 1, "overlapping detection should be suppressed");
        assert_eq!(kept[0], [0.0, 0.0, 100.0, 20.0]);
    }

    #[test]
    fn test_nms_keeps_non_overlapping() {
        let detections = vec![
            TatrDetection {
                bbox: [0.0, 0.0, 100.0, 20.0],
                confidence: 0.9,
                class_name: TatrClass::Row,
            },
            TatrDetection {
                bbox: [0.0, 50.0, 100.0, 70.0],
                confidence: 0.8,
                class_name: TatrClass::Row,
            },
        ];
        let bboxes: Vec<[f32; 4]> = detections.iter().map(|d| d.bbox).collect();
        let kept = nms_by_iob(&detections, &bboxes, NMS_IOB_THRESHOLD_ROWS);
        assert_eq!(kept.len(), 2, "non-overlapping detections should both be kept");
    }

    #[test]
    fn test_nms_keeps_adjacent_rows_with_minor_overlap() {
        let detections = vec![
            TatrDetection {
                bbox: [0.0, 0.0, 100.0, 20.0],
                confidence: 0.9,
                class_name: TatrClass::Row,
            },
            TatrDetection {
                bbox: [0.0, 18.0, 100.0, 38.0],
                confidence: 0.8,
                class_name: TatrClass::Row,
            },
        ];
        let bboxes: Vec<[f32; 4]> = detections.iter().map(|d| d.bbox).collect();
        let kept = nms_by_iob(&detections, &bboxes, NMS_IOB_THRESHOLD_ROWS);
        assert_eq!(kept.len(), 2, "adjacent rows with minor overlap should both be kept");
    }

    #[test]
    fn test_build_cell_grid_2x2() {
        let result = TatrResult {
            rows: vec![
                TatrDetection {
                    bbox: [0.0, 0.0, 100.0, 20.0],
                    confidence: 0.9,
                    class_name: TatrClass::Row,
                },
                TatrDetection {
                    bbox: [0.0, 20.0, 100.0, 40.0],
                    confidence: 0.85,
                    class_name: TatrClass::Row,
                },
            ],
            columns: vec![
                TatrDetection {
                    bbox: [0.0, 0.0, 50.0, 40.0],
                    confidence: 0.9,
                    class_name: TatrClass::Column,
                },
                TatrDetection {
                    bbox: [50.0, 0.0, 100.0, 40.0],
                    confidence: 0.85,
                    class_name: TatrClass::Column,
                },
            ],
            headers: Vec::new(),
            spanning: Vec::new(),
            table_bbox: None,
        };

        let grid = build_cell_grid(&result, None);
        assert_eq!(grid.len(), 2, "should have 2 rows");
        assert_eq!(grid[0].len(), 2, "should have 2 columns per row");

        let tl = &grid[0][0];
        assert!((tl.x1 - 0.0).abs() < 1e-5);
        assert!((tl.y1 - 0.0).abs() < 1e-5);
        assert!((tl.x2 - 50.0).abs() < 1e-5);
        assert!((tl.y2 - 20.0).abs() < 1e-5);

        let br = &grid[1][1];
        assert!((br.x1 - 50.0).abs() < 1e-5);
        assert!((br.y1 - 20.0).abs() < 1e-5);
        assert!((br.x2 - 100.0).abs() < 1e-5);
        assert!((br.y2 - 40.0).abs() < 1e-5);
    }

    #[test]
    fn test_build_cell_grid_empty() {
        let result = TatrResult {
            rows: Vec::new(),
            columns: Vec::new(),
            headers: Vec::new(),
            spanning: Vec::new(),
            table_bbox: None,
        };
        let grid = build_cell_grid(&result, None);
        assert!(grid.is_empty());
    }

    #[test]
    fn test_build_cell_grid_with_table_bbox() {
        let result = TatrResult {
            rows: vec![TatrDetection {
                bbox: [10.0, 5.0, 90.0, 25.0],
                confidence: 0.9,
                class_name: TatrClass::Row,
            }],
            columns: vec![TatrDetection {
                bbox: [0.0, 0.0, 50.0, 30.0],
                confidence: 0.9,
                class_name: TatrClass::Column,
            }],
            headers: Vec::new(),
            spanning: Vec::new(),
            table_bbox: None,
        };

        let grid = build_cell_grid(&result, Some([0.0, 0.0, 100.0, 30.0]));
        assert_eq!(grid.len(), 1);
        assert_eq!(grid[0].len(), 1);
        let cell = &grid[0][0];
        assert!((cell.x1 - 0.0).abs() < 1e-5, "x1={}", cell.x1);
        assert!((cell.x2 - 50.0).abs() < 1e-5, "x2={}", cell.x2);
    }

    #[test]
    fn test_tatr_class_from_index() {
        assert_eq!(TatrClass::from_index(0), Some(TatrClass::Table));
        assert_eq!(TatrClass::from_index(1), Some(TatrClass::Column));
        assert_eq!(TatrClass::from_index(2), Some(TatrClass::Row));
        assert_eq!(TatrClass::from_index(3), Some(TatrClass::ColumnHeader));
        assert_eq!(TatrClass::from_index(4), Some(TatrClass::ProjectedRowHeader));
        assert_eq!(TatrClass::from_index(5), Some(TatrClass::SpanningCell));
        assert_eq!(TatrClass::from_index(6), None);
        assert_eq!(TatrClass::from_index(7), None);
    }

    #[test]
    fn test_build_cell_grid_rows_sorted_spatially() {
        let result = TatrResult {
            rows: vec![
                TatrDetection {
                    bbox: [0.0, 30.0, 100.0, 50.0],
                    confidence: 0.95,
                    class_name: TatrClass::Row,
                },
                TatrDetection {
                    bbox: [0.0, 0.0, 100.0, 20.0],
                    confidence: 0.80,
                    class_name: TatrClass::Row,
                },
            ],
            columns: vec![TatrDetection {
                bbox: [0.0, 0.0, 100.0, 50.0],
                confidence: 0.9,
                class_name: TatrClass::Column,
            }],
            headers: Vec::new(),
            spanning: Vec::new(),
            table_bbox: None,
        };

        let grid = build_cell_grid(&result, None);
        assert_eq!(grid.len(), 2, "should have 2 rows");
        assert!(
            grid[0][0].y1 < grid[1][0].y1,
            "grid rows should be sorted top-to-bottom: row0.y1={} row1.y1={}",
            grid[0][0].y1,
            grid[1][0].y1,
        );
    }

    #[test]
    fn test_build_cell_grid_columns_sorted_spatially() {
        let result = TatrResult {
            rows: vec![TatrDetection {
                bbox: [0.0, 0.0, 100.0, 20.0],
                confidence: 0.9,
                class_name: TatrClass::Row,
            }],
            columns: vec![
                TatrDetection {
                    bbox: [60.0, 0.0, 100.0, 20.0],
                    confidence: 0.95,
                    class_name: TatrClass::Column,
                },
                TatrDetection {
                    bbox: [0.0, 0.0, 50.0, 20.0],
                    confidence: 0.80,
                    class_name: TatrClass::Column,
                },
            ],
            headers: Vec::new(),
            spanning: Vec::new(),
            table_bbox: None,
        };

        let grid = build_cell_grid(&result, None);
        assert_eq!(grid[0].len(), 2, "should have 2 columns");
        assert!(
            grid[0][0].x1 < grid[0][1].x1,
            "grid columns should be sorted left-to-right: col0.x1={} col1.x1={}",
            grid[0][0].x1,
            grid[0][1].x1,
        );
    }

    #[test]
    fn test_preprocess_detr_output_shape() {
        let img = RgbImage::new(640, 480);
        let (tensor, rw, rh) = preprocess_detr(&img);
        let shape = tensor.shape();
        assert_eq!(shape[0], 1, "batch dim");
        assert_eq!(shape[1], 3, "channel dim");
        assert_eq!(shape[2], rh as usize, "height dim");
        assert_eq!(shape[3], rw as usize, "width dim");
        assert_eq!(rh, 750);
        assert_eq!(rw, 1000);
    }

    #[test]
    fn test_nms_col_threshold_preserves_narrow_adjacent_columns() {
        let col_width = 20.0;
        let overlap = 7.0;
        let detections = vec![
            TatrDetection {
                bbox: [0.0, 0.0, col_width, 100.0],
                confidence: 0.9,
                class_name: TatrClass::Column,
            },
            TatrDetection {
                bbox: [col_width - overlap, 0.0, 2.0 * col_width - overlap, 100.0],
                confidence: 0.85,
                class_name: TatrClass::Column,
            },
        ];
        let bboxes: Vec<[f32; 4]> = detections.iter().map(|d| d.bbox).collect();

        let kept_row = nms_by_iob(&detections, &bboxes, NMS_IOB_THRESHOLD_ROWS);
        assert_eq!(kept_row.len(), 2, "row threshold should keep both");

        let kept_col = nms_by_iob(&detections, &bboxes, NMS_IOB_THRESHOLD_COLS);
        assert_eq!(
            kept_col.len(),
            1,
            "column threshold should suppress heavily overlapping column"
        );
    }

    #[test]
    fn test_nms_col_threshold_keeps_well_separated_columns() {
        let detections = vec![
            TatrDetection {
                bbox: [0.0, 0.0, 20.0, 100.0],
                confidence: 0.9,
                class_name: TatrClass::Column,
            },
            TatrDetection {
                bbox: [17.0, 0.0, 37.0, 100.0],
                confidence: 0.85,
                class_name: TatrClass::Column,
            },
        ];
        let bboxes: Vec<[f32; 4]> = detections.iter().map(|d| d.bbox).collect();

        let kept = nms_by_iob(&detections, &bboxes, NMS_IOB_THRESHOLD_COLS);
        assert_eq!(kept.len(), 2, "well-separated columns should both be kept");
    }

    #[test]
    fn test_min_col_width_filter_removes_noise_columns() {
        let result = TatrResult {
            rows: vec![TatrDetection {
                bbox: [0.0, 0.0, 100.0, 20.0],
                confidence: 0.9,
                class_name: TatrClass::Row,
            }],
            columns: vec![
                TatrDetection {
                    bbox: [0.0, 0.0, 50.0, 20.0],
                    confidence: 0.9,
                    class_name: TatrClass::Column,
                },
                TatrDetection {
                    bbox: [60.0, 0.0, 60.5, 20.0],
                    confidence: 0.5,
                    class_name: TatrClass::Column,
                },
                TatrDetection {
                    bbox: [70.0, 0.0, 100.0, 20.0],
                    confidence: 0.85,
                    class_name: TatrClass::Column,
                },
            ],
            headers: Vec::new(),
            spanning: Vec::new(),
            table_bbox: None,
        };

        let grid = build_cell_grid(&result, Some([0.0, 0.0, 100.0, 20.0]));
        assert_eq!(
            grid[0].len(),
            2,
            "noise column should be filtered, leaving 2 real columns"
        );
    }

    #[test]
    fn test_build_cell_grid_uses_per_class_nms() {
        let result = TatrResult {
            rows: vec![
                TatrDetection {
                    bbox: [0.0, 0.0, 100.0, 25.0],
                    confidence: 0.9,
                    class_name: TatrClass::Row,
                },
                TatrDetection {
                    bbox: [0.0, 15.0, 100.0, 40.0],
                    confidence: 0.85,
                    class_name: TatrClass::Row,
                },
            ],
            columns: vec![TatrDetection {
                bbox: [0.0, 0.0, 50.0, 40.0],
                confidence: 0.9,
                class_name: TatrClass::Column,
            }],
            headers: Vec::new(),
            spanning: Vec::new(),
            table_bbox: None,
        };

        let grid = build_cell_grid(&result, None);
        assert_eq!(
            grid.len(),
            2,
            "rows with 0.4 IoB should both survive row NMS (threshold 0.5)"
        );
    }

    fn header_detection(bbox: [f32; 4]) -> TatrDetection {
        TatrDetection {
            bbox,
            confidence: 0.9,
            class_name: TatrClass::ColumnHeader,
        }
    }

    fn spanning_detection(bbox: [f32; 4]) -> TatrDetection {
        TatrDetection {
            bbox,
            confidence: 0.9,
            class_name: TatrClass::SpanningCell,
        }
    }

    #[test]
    fn test_compute_header_row_count_single_header_row() {
        let rows = [[0.0, 0.0, 100.0, 20.0], [0.0, 20.0, 100.0, 40.0]];
        let headers = [header_detection([0.0, 0.0, 100.0, 20.0])];
        assert_eq!(compute_header_row_count(&headers, &rows), 1);
    }

    #[test]
    fn test_compute_header_row_count_multi_row_header() {
        let rows = [
            [0.0, 0.0, 100.0, 20.0],
            [0.0, 20.0, 100.0, 40.0],
            [0.0, 40.0, 100.0, 60.0],
        ];
        let headers = [header_detection([0.0, 0.0, 100.0, 40.0])];
        assert_eq!(
            compute_header_row_count(&headers, &rows),
            2,
            "a header detection spanning two row bands must mark both as header rows"
        );
    }

    #[test]
    fn test_compute_header_row_count_no_headers_returns_zero() {
        let rows = [[0.0, 0.0, 100.0, 20.0]];
        assert_eq!(
            compute_header_row_count(&[], &rows),
            0,
            "absence of TATR header detections must not be conflated with an explicit single header row"
        );
    }

    #[test]
    fn test_compute_spans_merges_two_columns() {
        let rows = [[0.0, 0.0, 100.0, 20.0]];
        let cols = [[0.0, 0.0, 50.0, 20.0], [50.0, 0.0, 100.0, 20.0]];
        let spanning = [spanning_detection([0.0, 0.0, 100.0, 20.0])];
        let spans = compute_spans(&spanning, &rows, &cols);
        assert_eq!(spans, vec![(0, 1, 0, 2)]);
    }

    #[test]
    fn test_compute_spans_skips_single_cell_overlap() {
        let rows = [[0.0, 0.0, 100.0, 20.0]];
        let cols = [[0.0, 0.0, 50.0, 20.0], [50.0, 0.0, 100.0, 20.0]];
        // Covers only the first column: not an actual merge.
        let spanning = [spanning_detection([0.0, 0.0, 50.0, 20.0])];
        let spans = compute_spans(&spanning, &rows, &cols);
        assert!(spans.is_empty(), "a detection covering a single cell is not a span");
    }

    #[test]
    fn test_build_cell_grid_with_structure_reports_header_and_span() {
        let result = TatrResult {
            rows: vec![
                TatrDetection {
                    bbox: [0.0, 0.0, 100.0, 20.0],
                    confidence: 0.9,
                    class_name: TatrClass::Row,
                },
                TatrDetection {
                    bbox: [0.0, 20.0, 100.0, 40.0],
                    confidence: 0.9,
                    class_name: TatrClass::Row,
                },
            ],
            columns: vec![
                TatrDetection {
                    bbox: [0.0, 0.0, 50.0, 40.0],
                    confidence: 0.9,
                    class_name: TatrClass::Column,
                },
                TatrDetection {
                    bbox: [50.0, 0.0, 100.0, 40.0],
                    confidence: 0.9,
                    class_name: TatrClass::Column,
                },
            ],
            headers: vec![header_detection([0.0, 0.0, 100.0, 20.0])],
            spanning: vec![spanning_detection([0.0, 0.0, 100.0, 20.0])],
            table_bbox: Some([0.0, 0.0, 100.0, 40.0]),
        };

        let (grid, structure) = build_cell_grid_with_structure(&result, result.table_bbox);
        assert_eq!(grid.len(), 2, "grid shape must be unaffected by structure metadata");
        assert_eq!(grid[0].len(), 2);
        assert_eq!(structure.header_row_count, 1);
        assert_eq!(structure.spans, vec![(0, 1, 0, 2)]);
    }
}
