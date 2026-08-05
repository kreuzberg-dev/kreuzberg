//! OCR execution and result processing.
//!
//! This module handles the core OCR execution logic, including image processing,
//! text extraction, and result formatting.

use super::api_pool::TesseractApiPool;
use super::config::{apply_tesseract_variables, hash_config};
use super::validation::{
    resolve_all_installed_languages, resolve_tessdata_path, strip_control_characters, validate_language_and_traineddata,
};
use crate::core::config::ExtractionConfig;
use crate::image::normalize_image_dpi_owned;
use crate::ocr::cache::OcrCache;
use crate::ocr::conversion::{TsvRow, iterator_word_to_element, tsv_row_to_element};
use crate::ocr::error::OcrError;
use crate::ocr::hocr_parser::parse_hocr_to_internal_document;
#[cfg(feature = "pdf")]
use crate::ocr::table::post_process_table;
use crate::ocr::table::{extract_words_from_tsv, reconstruct_table, table_to_markdown};
#[cfg(test)]
use crate::ocr::types::BatchItemResult;
use crate::ocr::types::TesseractConfig;
use crate::types::internal::{ElementKind, InternalDocument};
use crate::types::{OcrExtractionResult, OcrTable, OcrTableBoundingBox};
use std::collections::HashMap;
use std::env;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use xberg_tesseract::{TessPageSegMode, TessPolyBlockType, TesseractAPI};

/// Process-global document-orientation classifier (ONNX PP-LCNet), shared by
/// every tesseract-backend auto-rotate call. Session initialization is lazy and
/// internally synchronized; `detect` is thread-safe.
#[cfg(auto_rotate)]
fn doc_orientation_detector() -> &'static crate::doc_orientation::DocOrientationDetector {
    static DETECTOR: std::sync::LazyLock<crate::doc_orientation::DocOrientationDetector> =
        std::sync::LazyLock::new(|| {
            crate::doc_orientation::DocOrientationDetector::with_acceleration(
                crate::doc_orientation::resolve_cache_dir(),
                None,
            )
        });
    &DETECTOR
}

use crate::types::OcrElement;

#[cfg(auto_rotate)]
/// Rotate raw RGB image data by the given degrees (0, 90, 180, 270).
///
/// Returns the rotated pixel data and the new (width, height).
/// For 90° and 270° rotations, width and height are swapped.
fn rotate_rgb_image_data(data: &[u8], width: u32, height: u32, degrees: i32) -> (Vec<u8>, u32, u32) {
    let bpp = 3usize;
    let w = width as usize;
    let h = height as usize;

    match degrees {
        0 => (data.to_vec(), width, height),
        90 => {
            let new_w = h;
            let new_h = w;
            let mut out = vec![0u8; new_w * new_h * bpp];
            for y in 0..h {
                for x in 0..w {
                    let src_idx = (y * w + x) * bpp;
                    let dst_x = h - 1 - y;
                    let dst_y = x;
                    let dst_idx = (dst_y * new_w + dst_x) * bpp;
                    out[dst_idx..dst_idx + bpp].copy_from_slice(&data[src_idx..src_idx + bpp]);
                }
            }
            (out, new_w as u32, new_h as u32)
        }
        180 => {
            let mut out = vec![0u8; w * h * bpp];
            for y in 0..h {
                for x in 0..w {
                    let src_idx = (y * w + x) * bpp;
                    let dst_x = w - 1 - x;
                    let dst_y = h - 1 - y;
                    let dst_idx = (dst_y * w + dst_x) * bpp;
                    out[dst_idx..dst_idx + bpp].copy_from_slice(&data[src_idx..src_idx + bpp]);
                }
            }
            (out, width, height)
        }
        270 => {
            let new_w = h;
            let new_h = w;
            let mut out = vec![0u8; new_w * new_h * bpp];
            for y in 0..h {
                for x in 0..w {
                    let src_idx = (y * w + x) * bpp;
                    let dst_x = y;
                    let dst_y = w - 1 - x;
                    let dst_idx = (dst_y * new_w + dst_x) * bpp;
                    out[dst_idx..dst_idx + bpp].copy_from_slice(&data[src_idx..src_idx + bpp]);
                }
            }
            (out, new_w as u32, new_h as u32)
        }
        _ => {
            tracing::warn!("Unsupported rotation angle: {}°, skipping rotation", degrees);
            (data.to_vec(), width, height)
        }
    }
}

/// Parse Tesseract TSV output into structured OcrElements.
///
/// TSV format columns: level, page_num, block_num, par_num, line_num, word_num, left, top, width, height, conf, text
///
/// # Arguments
///
/// * `tsv_data` - Raw TSV output from Tesseract
/// * `min_confidence` - Minimum confidence threshold (0-100 scale)
///
/// # Returns
///
/// Vector of OcrElements for word-level and line-level entries
fn parse_tsv_to_elements(tsv_data: &str, min_confidence: f64) -> Vec<OcrElement> {
    let mut elements = Vec::new();

    for line in tsv_data.lines().skip(1) {
        let fields: Vec<&str> = line.split('\t').collect();
        if fields.len() < 12 {
            continue;
        }

        let level = fields[0].parse::<i32>().unwrap_or(0);
        let page_num = fields[1].parse::<i32>().unwrap_or(1);
        let block_num = fields[2].parse::<i32>().unwrap_or(0);
        let par_num = fields[3].parse::<i32>().unwrap_or(0);
        let line_num = fields[4].parse::<i32>().unwrap_or(0);
        let word_num = fields[5].parse::<i32>().unwrap_or(0);
        let left = fields[6].parse::<u32>().unwrap_or(0);
        let top = fields[7].parse::<u32>().unwrap_or(0);
        let width = fields[8].parse::<u32>().unwrap_or(0);
        let height = fields[9].parse::<u32>().unwrap_or(0);
        let conf = fields[10].parse::<f64>().unwrap_or(-1.0);
        let text = fields[11].to_string();

        if conf < 0.0 || conf < min_confidence || text.trim().is_empty() {
            continue;
        }

        if level != 4 && level != 5 {
            continue;
        }

        let tsv_row = TsvRow {
            level,
            page_num,
            block_num,
            par_num,
            line_num,
            word_num,
            left,
            top,
            width,
            height,
            conf,
            text,
        };

        elements.push(tsv_row_to_element(&tsv_row));
    }

    elements
}

/// CI debug logging utility.
///
/// Logs debug messages when XBERG_CI_DEBUG environment variable is set.
fn log_ci_debug<F>(enabled: bool, stage: &str, details: F)
where
    F: FnOnce() -> String,
{
    if !enabled {
        return;
    }

    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0);

    tracing::debug!(stage, timestamp = format!("{timestamp:.3}"), "{}", details());
}

/// Build content with OCR tables inlined at their correct vertical positions.
///
/// Parses TSV word positions to separate table words from non-table words,
/// groups non-table words into lines and paragraphs, then interleaves
/// paragraphs and table markdown sorted by Y-position.
fn build_content_with_inline_tables(tsv_data: &str, tables: &[OcrTable], min_confidence: f64) -> String {
    let words = match extract_words_from_tsv(tsv_data, min_confidence) {
        Ok(w) => w,
        Err(_) => return String::new(),
    };

    if words.is_empty() {
        return String::new();
    }

    let table_bboxes: Vec<_> = tables.iter().filter_map(|t| t.bounding_box.as_ref()).collect();

    let mut non_table_words = Vec::new();
    for word in &words {
        let in_table = table_bboxes.iter().any(|bbox| {
            let word_cx = word.left + word.width / 2;
            let word_cy = word.top + word.height / 2;
            word_cx >= bbox.left && word_cx <= bbox.right && word_cy >= bbox.top && word_cy <= bbox.bottom
        });
        if !in_table {
            non_table_words.push(word);
        }
    }

    if non_table_words.is_empty() && tables.is_empty() {
        return String::new();
    }

    let mut sorted_words = non_table_words;
    sorted_words.sort_by(|a, b| a.top.cmp(&b.top).then(a.left.cmp(&b.left)));

    let avg_height = if sorted_words.is_empty() {
        20
    } else {
        let total_h: u32 = sorted_words.iter().map(|w| w.height).sum();
        (total_h / sorted_words.len() as u32).max(1)
    };
    let line_threshold = avg_height / 2;

    struct TextLine {
        y_center: u32,
        text: String,
    }

    let mut lines: Vec<TextLine> = Vec::new();
    for word in &sorted_words {
        let word_y = word.top + word.height / 2;
        if let Some(last_line) = lines.last_mut()
            && word_y.abs_diff(last_line.y_center) <= line_threshold
        {
            last_line.text.push(' ');
            last_line.text.push_str(&word.text);
            continue;
        }
        lines.push(TextLine {
            y_center: word_y,
            text: word.text.clone(),
        });
    }

    let paragraph_gap = avg_height * 2;

    struct Paragraph {
        y_start: u32,
        text: String,
    }

    let mut paragraphs: Vec<Paragraph> = Vec::new();
    for line in &lines {
        if let Some(last_para) = paragraphs.last_mut() {
            let last_y = last_para.y_start;
            if line.y_center.saturating_sub(last_y) <= paragraph_gap {
                last_para.text.push('\n');
                last_para.text.push_str(&line.text);
                last_para.y_start = line.y_center;
                continue;
            }
        }
        paragraphs.push(Paragraph {
            y_start: line.y_center,
            text: line.text.clone(),
        });
    }

    enum ContentElement<'a> {
        Paragraph { y: u32, text: String },
        Table { y: u32, markdown: &'a str },
    }

    let mut elements: Vec<ContentElement> = Vec::new();

    {
        let mut para_idx = 0;
        let mut line_idx = 0;
        for para in &paragraphs {
            let line_count = para.text.matches('\n').count() + 1;
            let first_y = if line_idx < lines.len() {
                lines[line_idx].y_center
            } else {
                para.y_start
            };
            elements.push(ContentElement::Paragraph {
                y: first_y,
                text: para.text.clone(),
            });
            line_idx += line_count;
            para_idx += 1;
        }
        let _ = para_idx;
    }

    for table in tables {
        if let Some(ref bbox) = table.bounding_box {
            elements.push(ContentElement::Table {
                y: bbox.top,
                markdown: &table.markdown,
            });
        } else {
            elements.push(ContentElement::Table {
                y: u32::MAX,
                markdown: &table.markdown,
            });
        }
    }

    elements.sort_by_key(|e| match e {
        ContentElement::Paragraph { y, .. } => *y,
        ContentElement::Table { y, .. } => *y,
    });

    let mut output = String::new();
    for elem in &elements {
        let text = match elem {
            ContentElement::Paragraph { text, .. } => text.as_str(),
            ContentElement::Table { markdown, .. } => markdown,
        };
        let trimmed = text.trim();
        if trimmed.is_empty() {
            continue;
        }
        if !output.is_empty() {
            output.push_str("\n\n");
        }
        output.push_str(trimmed);
    }

    output
}

/// Minimum confidence for accepting orientation detection results.
///
/// Keep in sync with `doc_orientation::MIN_CONFIDENCE` (module is feature-gated,
/// this const is not): the PP-LCNet classifier reports correct 90°/270° rotations
/// on real documents at ~0.45, so a 0.5 cutoff rejects valid detections while
/// 0° false positives above 0.35 are rare.
const MIN_ORIENTATION_CONFIDENCE: f32 = 0.35;

#[cfg(auto_rotate)]
const _: () = assert!(MIN_ORIENTATION_CONFIDENCE == crate::doc_orientation::MIN_CONFIDENCE);

/// Grayscale mean (0–255) below which the image is considered background-
/// dark rather than background-light.
///
/// Scanned documents are background-dominated (background typically covers
/// 85–95% of pixel area), so the grayscale mean tracks background tone far
/// more than foreground text tone. A mean below this value means most of the
/// image is dark, i.e. a plausible dark background.
const DARK_BACKGROUND_MEAN_THRESHOLD: f64 = 100.0;

/// Grayscale value (0–255) at or above which a pixel counts as "light" when
/// computing the light-pixel fraction used to guard polarity auto-detection.
const LIGHT_PIXEL_VALUE_THRESHOLD: u8 = 180;

/// Minimum fraction of light pixels required, alongside a dark mean, before
/// auto-detection treats an image as light-text-on-dark-background.
///
/// Without this guard, a naive `mean < threshold` check would also fire on
/// uniformly dark, non-text images (e.g. a dark photograph or a heavily
/// shadowed scan) where inverting would corrupt the image rather than fix
/// it. Real text/foreground content typically covers at least a small
/// fraction of a page, so requiring *some* light pixels alongside the dark
/// mean distinguishes "light text on a dark background" from "uniformly
/// dark image".
const MIN_LIGHT_PIXEL_FRACTION_FOR_INVERT: f64 = 0.01;

/// Sampling grid spacing (pixels) used when computing polarity statistics.
/// Keeps polarity detection cheap on large scans while remaining
/// representative.
const POLARITY_SAMPLE_STRIDE: i32 = 4;

/// Source resolution used when OCR receives the original image unchanged.
const RAW_IMAGE_SOURCE_DPI: i32 = 72;
/// Source resolution retained when explicit DPI normalization fails.
const PREPROCESSING_FALLBACK_SOURCE_DPI: i32 = 300;
/// Pixel stride used to classify whether an image resembles a clean document page.
const DEFAULT_PREPROCESSING_SAMPLE_STRIDE: usize = 4;
/// Minimum normalized mean luminance for automatic document-page preprocessing.
const CLEAN_PAGE_MEAN_LUMINANCE_THRESHOLD: f64 = 0.90;
/// Normalized luminance at which a sampled pixel counts as near-white.
const CLEAN_PAGE_LIGHT_PIXEL_THRESHOLD: f64 = 0.90;
/// Minimum near-white sample fraction for automatic document-page preprocessing.
const CLEAN_PAGE_LIGHT_PIXEL_FRACTION_THRESHOLD: f64 = 0.80;
/// Maximum value of an 8-bit RGB channel.
const RGB_CHANNEL_MAX: f64 = u8::MAX as f64;
/// Number of channels in an RGB pixel.
const RGB_CHANNEL_COUNT: usize = 3;

struct PreparedOcrImage {
    data: Vec<u8>,
    width: u32,
    height: u32,
    source_dpi: i32,
    apply_pix_preprocessing: bool,
}

fn prepare_ocr_image(
    rgb_data: Vec<u8>,
    width: u32,
    height: u32,
    preprocessing: Option<&crate::types::ImagePreprocessingConfig>,
    ci_debug_enabled: bool,
) -> PreparedOcrImage {
    let Some(preprocessing) = preprocessing else {
        if should_apply_default_preprocessing(&rgb_data) {
            return prepare_preprocessed_ocr_image(
                rgb_data,
                width,
                height,
                &crate::types::ImagePreprocessingConfig::default(),
                ci_debug_enabled,
            );
        }
        return PreparedOcrImage {
            data: rgb_data,
            width,
            height,
            source_dpi: RAW_IMAGE_SOURCE_DPI,
            apply_pix_preprocessing: false,
        };
    };

    prepare_preprocessed_ocr_image(rgb_data, width, height, preprocessing, ci_debug_enabled)
}

/// Classify bright, page-like RGB images that benefit from the default OCR preprocessing path.
fn should_apply_default_preprocessing(rgb_data: &[u8]) -> bool {
    let mut luminance_sum = 0.0;
    let mut light_pixels = 0usize;
    let mut sample_count = 0usize;

    for pixel in rgb_data
        .chunks_exact(RGB_CHANNEL_COUNT)
        .step_by(DEFAULT_PREPROCESSING_SAMPLE_STRIDE)
    {
        let luminance =
            pixel.iter().map(|channel| f64::from(*channel)).sum::<f64>() / (RGB_CHANNEL_MAX * RGB_CHANNEL_COUNT as f64);
        luminance_sum += luminance;
        light_pixels += usize::from(luminance >= CLEAN_PAGE_LIGHT_PIXEL_THRESHOLD);
        sample_count += 1;
    }

    if sample_count == 0 {
        return false;
    }
    let sample_count = sample_count as f64;
    luminance_sum / sample_count >= CLEAN_PAGE_MEAN_LUMINANCE_THRESHOLD
        && light_pixels as f64 / sample_count >= CLEAN_PAGE_LIGHT_PIXEL_FRACTION_THRESHOLD
}

fn prepare_preprocessed_ocr_image(
    rgb_data: Vec<u8>,
    width: u32,
    height: u32,
    preprocessing: &crate::types::ImagePreprocessingConfig,
    ci_debug_enabled: bool,
) -> PreparedOcrImage {
    let dpi_config = crate::types::ImageDpiConfig {
        target_dpi: preprocessing.target_dpi,
        ..Default::default()
    };
    match normalize_image_dpi_owned(rgb_data, width as usize, height as usize, &dpi_config, None) {
        Ok(result) => {
            let normalized_width = result.dimensions.0 as u32;
            let normalized_height = result.dimensions.1 as u32;
            let source_dpi = result.metadata.final_dpi;
            log_ci_debug(ci_debug_enabled, "dpi_normalization", || {
                format!(
                    "original={}x{} normalized={}x{} target_dpi={} final_dpi={} resized={}",
                    width,
                    height,
                    normalized_width,
                    normalized_height,
                    result.metadata.target_dpi,
                    source_dpi,
                    !result.metadata.skipped_resize
                )
            });
            PreparedOcrImage {
                data: result.rgb_data,
                width: normalized_width,
                height: normalized_height,
                source_dpi,
                apply_pix_preprocessing: true,
            }
        }
        Err((error, data)) => {
            tracing::warn!("DPI normalization failed, using original image: {}", error);
            PreparedOcrImage {
                data,
                width,
                height,
                source_dpi: PREPROCESSING_FALLBACK_SOURCE_DPI,
                apply_pix_preprocessing: true,
            }
        }
    }
}

/// Decide whether an image should be inverted before OCR.
///
/// `force_invert` mirrors `ImagePreprocessingConfig.invert_colors`: `true`
/// unconditionally forces inversion (an explicit override for cases where
/// auto-detection disagrees); `false` (the default) falls back to
/// auto-detection from `mean_gray` and `light_fraction` — see
/// `DARK_BACKGROUND_MEAN_THRESHOLD` and `MIN_LIGHT_PIXEL_FRACTION_FOR_INVERT`.
///
/// This is a pure function so the polarity decision can be unit-tested
/// without linking native Tesseract/Leptonica.
fn should_invert_for_polarity(mean_gray: f64, light_fraction: f64, force_invert: bool) -> bool {
    force_invert
        || (mean_gray < DARK_BACKGROUND_MEAN_THRESHOLD && light_fraction >= MIN_LIGHT_PIXEL_FRACTION_FOR_INVERT)
}

/// Preprocess a raw Pix before handing it to Tesseract.
///
/// Order matters: polarity (light-on-dark vs. dark-on-light) is detected and
/// corrected *first*, because `background_normalize` assumes a light
/// background and Tesseract's own binarizer assumes dark text on a light
/// background. `force_invert` mirrors `ImagePreprocessingConfig.invert_colors`
/// (see [`should_invert_for_polarity`]).
fn preprocess_pix(pix: xberg_tesseract::Pix, force_invert: bool) -> xberg_tesseract::Result<xberg_tesseract::Pix> {
    let polarity_stats = pix
        .to_grayscale()
        .and_then(|gray| gray.grayscale_stats(LIGHT_PIXEL_VALUE_THRESHOLD, POLARITY_SAMPLE_STRIDE))
        .ok();

    let invert = match polarity_stats {
        Some((mean_gray, light_fraction)) => should_invert_for_polarity(mean_gray, light_fraction, force_invert),
        None => force_invert,
    };

    let source = if invert {
        let inverted = pix.invert()?;
        drop(pix);
        inverted
    } else {
        pix
    };

    let normalized = source.background_normalize()?;
    drop(source);

    let sharpened = normalized.unsharp_mask(3, 0.5)?;
    drop(normalized);

    let grayscale = sharpened.to_grayscale()?;
    drop(sharpened);
    Ok(grayscale)
}

/// Check whether a center point (x, y) lies within a bounding box.
fn point_in_bbox(x: i32, y: i32, left: i32, top: i32, right: i32, bottom: i32) -> bool {
    x >= left && x <= right && y >= top && y <= bottom
}

/// Extract OcrElements via Tesseract's iterator APIs with rich metadata.
///
/// Uses ResultIterator for word-level text, bounding boxes, confidence, and font
/// attributes, plus PageIterator for block type and paragraph info. This replaces
/// TSV-based extraction with significantly richer metadata.
fn extract_elements_via_iterator(
    api: &TesseractAPI,
    page_number: u32,
    min_confidence: f64,
) -> Result<Vec<OcrElement>, OcrError> {
    let page_iter = match api.get_page_iterator() {
        Ok(iter) => iter,
        Err(_) => return Ok(Vec::new()),
    };

    let blocks = match page_iter.extract_all_blocks() {
        Ok(b) => b,
        Err(_) => return Ok(Vec::new()),
    };

    let paragraphs = match page_iter.extract_all_paragraphs() {
        Ok(p) => p,
        Err(_) => return Ok(Vec::new()),
    };

    let result_iter = match api.get_iterator() {
        Ok(iter) => iter,
        Err(_) => return Ok(Vec::new()),
    };

    let words = match result_iter.extract_all_words() {
        Ok(w) => w,
        Err(_) => return Ok(Vec::new()),
    };

    let skip_block_types = [
        TessPolyBlockType::PT_NOISE,
        TessPolyBlockType::PT_FLOWING_IMAGE,
        TessPolyBlockType::PT_HEADING_IMAGE,
        TessPolyBlockType::PT_PULLOUT_IMAGE,
        TessPolyBlockType::PT_HORZ_LINE,
        TessPolyBlockType::PT_VERT_LINE,
    ];

    let mut elements = Vec::new();

    for word in &words {
        if (word.confidence as f64) < min_confidence {
            continue;
        }

        if word.text.trim().is_empty() {
            continue;
        }

        let cx = (word.left + word.right) / 2;
        let cy = (word.top + word.bottom) / 2;

        let parent_block = blocks
            .iter()
            .find(|b| point_in_bbox(cx, cy, b.left, b.top, b.right, b.bottom));

        let block_type = parent_block.map(|b| b.block_type);

        if let Some(bt) = block_type
            && skip_block_types.contains(&bt)
        {
            continue;
        }

        let para_info = paragraphs
            .iter()
            .find(|p| point_in_bbox(cx, cy, p.left, p.top, p.right, p.bottom));

        let element = iterator_word_to_element(word, block_type, para_info, page_number);
        elements.push(element);
    }

    Ok(elements)
}

/// Perform OCR on an image using Tesseract.
///
/// This function handles the complete OCR pipeline:
/// 1. Image loading and preprocessing
/// 2. Tesseract initialization and configuration
/// 3. Text recognition
/// 4. Output formatting (text, markdown, hOCR, or TSV)
/// 5. Optional table detection
///
/// # Arguments
///
/// * `image_bytes` - Raw image data
/// * `config` - OCR configuration
/// * `extraction_config` - Optional extraction config for output format (markdown vs djot)
///
/// # Returns
///
/// OCR extraction result containing text and optional tables
pub(super) fn perform_ocr(
    image_bytes: &[u8],
    config: &TesseractConfig,
    api_pool: &Arc<TesseractApiPool>,
    extraction_config: Option<&ExtractionConfig>,
) -> Result<OcrExtractionResult, OcrError> {
    let ci_debug_enabled = env::var_os("XBERG_CI_DEBUG").is_some();
    log_ci_debug(ci_debug_enabled, "perform_ocr:start", || {
        format!(
            "bytes={} language={} output={} use_cache={}",
            image_bytes.len(),
            config.language,
            config.output_format,
            config.use_cache
        )
    });

    let rgb_image = {
        let img = crate::extraction::image::load_image_for_ocr(image_bytes)
            .map_err(|e| OcrError::ImageProcessingFailed(e.to_string()))?;
        img.into_rgb8()
    };
    let (orig_width, orig_height) = rgb_image.dimensions();
    let rgb_data = rgb_image.into_raw();

    log_ci_debug(ci_debug_enabled, "image", || {
        format!("dimensions={}x{} color_type=RGB8", orig_width, orig_height)
    });

    let prepared_image = prepare_ocr_image(
        rgb_data,
        orig_width,
        orig_height,
        config.preprocessing.as_ref(),
        ci_debug_enabled,
    );
    let image_data = prepared_image.data;
    let width = prepared_image.width;
    let height = prepared_image.height;
    let source_dpi = prepared_image.source_dpi;
    #[cfg_attr(not(auto_rotate), allow(unused_mut))]
    let mut ocr_image_width = width;
    #[cfg_attr(not(auto_rotate), allow(unused_mut))]
    let mut ocr_image_height = height;

    let bytes_per_pixel: u32 = 3;
    let bytes_per_line = width * bytes_per_pixel;

    let languages: Vec<String> = config.language.split('+').map(|lang| lang.trim().to_string()).collect();
    let tessdata_path = resolve_tessdata_path(&languages, config.tessdata_path.as_deref())?;

    log_ci_debug(ci_debug_enabled, "tessdata", || {
        let path_preview = env::var_os("PATH").map(|paths| {
            env::split_paths(&paths)
                .take(6)
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        });
        let resolved_exists = !tessdata_path.is_empty() && std::path::Path::new(&tessdata_path).exists();

        format!(
            "env={:?} resolved={} exists={} path_preview={:?}",
            env::var("TESSDATA_PREFIX").ok(),
            if tessdata_path.is_empty() {
                "unset"
            } else {
                &tessdata_path
            },
            resolved_exists,
            path_preview
        )
    });

    log_ci_debug(ci_debug_enabled, "tesseract_version", || {
        format!("version={}", TesseractAPI::version())
    });

    validate_language_and_traineddata(&config.language, &tessdata_path)?;

    let api = api_pool.checkout(&tessdata_path, &config.language)?;

    log_ci_debug(ci_debug_enabled, "init", || {
        format!("language={} datapath='{}'", config.language, tessdata_path)
    });

    if ci_debug_enabled {
        match api.get_available_languages() {
            Ok(languages) => {
                log_ci_debug(ci_debug_enabled, "available_languages", move || {
                    let preview = languages.iter().take(10).cloned().collect::<Vec<_>>();
                    format!("count={} preview={:?}", languages.len(), preview)
                });
            }
            Err(err) => {
                log_ci_debug(ci_debug_enabled, "available_languages_error", move || {
                    format!("error={:?}", err)
                });
            }
        }
    }

    let psm_mode = TessPageSegMode::from_int(config.psm as i32);
    let psm_result = api.set_page_seg_mode(psm_mode);
    log_ci_debug(ci_debug_enabled, "set_psm", || match &psm_result {
        Ok(_) => format!("mode={}", config.psm),
        Err(err) => format!("error={:?}", err),
    });
    psm_result.map_err(|e| OcrError::InvalidConfiguration(format!("Failed to set PSM mode: {}", e)))?;

    apply_tesseract_variables(&api, config)?;

    let force_invert_colors = config.preprocessing.as_ref().map(|p| p.invert_colors).unwrap_or(false);

    let processed_pix: Option<xberg_tesseract::Pix> = if prepared_image.apply_pix_preprocessing {
        match xberg_tesseract::Pix::from_raw_rgb(&image_data, width, height) {
            Ok(mut pix) => {
                if let Ok((xres, yres)) = pix.get_resolution()
                    && (xres == 0 || yres == 0)
                {
                    let _ = pix.set_resolution(72, 72);
                }

                let processed = preprocess_pix(pix, force_invert_colors);
                match processed {
                    Ok(p) => Some(p),
                    Err(e) => {
                        tracing::debug!("Leptonica preprocessing failed, using raw image: {}", e);
                        None
                    }
                }
            }
            Err(e) => {
                tracing::debug!("Leptonica Pix creation failed, using raw image: {}", e);
                None
            }
        }
    } else {
        None
    };

    if let Some(ref pix) = processed_pix {
        api.set_image_2(pix.as_ptr())
            .map_err(|e| OcrError::ProcessingFailed(format!("Failed to set preprocessed image: {}", e)))?;
    } else {
        api.set_image(
            &image_data,
            width as i32,
            height as i32,
            bytes_per_pixel as i32,
            bytes_per_line as i32,
        )
        .map_err(|e| OcrError::ProcessingFailed(format!("Failed to set image: {}", e)))?;
    }
    drop(processed_pix);

    let source_dpi = source_dpi.max(70);
    api.set_source_resolution(source_dpi)
        .map_err(|e| OcrError::ProcessingFailed(format!("Failed to set source resolution: {}", e)))?;

    log_ci_debug(ci_debug_enabled, "set_image", || {
        format!(
            "width={} height={} bytes_per_pixel={} bytes_per_line={} source_dpi={}",
            width, height, bytes_per_pixel, bytes_per_line, source_dpi
        )
    });

    #[cfg_attr(not(auto_rotate), allow(unused_mut))]
    let mut detected_orientation: Option<(i32, f32, String, f32)> = None;
    let auto_rotate_enabled =
        config.preprocessing.as_ref().map(|p| p.auto_rotate).unwrap_or(false) || config.auto_rotate;

    #[cfg(not(auto_rotate))]
    if auto_rotate_enabled {
        tracing::warn!(
            "auto_rotate requested but the `auto-rotate` feature is not compiled in; skipping orientation detection"
        );
    }

    #[cfg(auto_rotate)]
    if auto_rotate_enabled {
        let orientation_result = image::RgbImage::from_raw(width, height, image_data.clone())
            .ok_or_else(|| crate::error::XbergError::Ocr {
                message: "auto_rotate: image buffer does not match dimensions".to_string(),
                source: None,
            })
            .and_then(|img| doc_orientation_detector().detect(&img));

        match orientation_result {
            Err(e) => {
                tracing::warn!("Orientation detection failed, proceeding without rotation: {}", e);
            }
            Ok(orientation) => {
                let orient_deg = orientation.degrees as i32;
                let orient_conf = orientation.confidence;
                log_ci_debug(ci_debug_enabled, "orientation_detection", || {
                    format!("orientation={}° confidence={:.2}", orient_deg, orient_conf)
                });
                detected_orientation = Some((orient_deg, orient_conf, String::new(), 0.0));

                if orient_deg != 0 && orient_conf > MIN_ORIENTATION_CONFIDENCE {
                    tracing::info!(
                        "Auto-rotating image by {} degrees (confidence: {:.2})",
                        orient_deg,
                        orient_conf
                    );

                    let correction_deg = (360 - orient_deg).rem_euclid(360);
                    let (rotated_data, new_width, new_height) =
                        rotate_rgb_image_data(&image_data, width, height, correction_deg);
                    let new_bytes_per_line = new_width * bytes_per_pixel;

                    let rotated_pix = if prepared_image.apply_pix_preprocessing {
                        xberg_tesseract::Pix::from_raw_rgb(&rotated_data, new_width, new_height)
                            .ok()
                            .and_then(|pix| preprocess_pix(pix, force_invert_colors).ok())
                    } else {
                        None
                    };

                    if let Some(ref pix) = rotated_pix {
                        api.set_image_2(pix.as_ptr()).map_err(|e| {
                            OcrError::ProcessingFailed(format!("Failed to set rotated preprocessed image: {}", e))
                        })?;
                    } else {
                        api.set_image(
                            &rotated_data,
                            new_width as i32,
                            new_height as i32,
                            bytes_per_pixel as i32,
                            new_bytes_per_line as i32,
                        )
                        .map_err(|e| OcrError::ProcessingFailed(format!("Failed to set rotated image: {}", e)))?;
                    }

                    drop(rotated_pix);

                    api.set_source_resolution(source_dpi).map_err(|e| {
                        OcrError::ProcessingFailed(format!("Failed to set source resolution after rotation: {}", e))
                    })?;
                    ocr_image_width = new_width;
                    ocr_image_height = new_height;

                    log_ci_debug(ci_debug_enabled, "auto_rotate", || {
                        format!("rotated={}° new_dimensions={}x{}", orient_deg, new_width, new_height)
                    });
                } else {
                    tracing::debug!(
                        degrees = orient_deg,
                        confidence = orient_conf,
                        threshold = MIN_ORIENTATION_CONFIDENCE,
                        "auto_rotate: keeping original orientation"
                    );
                }
            }
        }
    }

    drop(image_data);

    api.recognize()
        .map_err(|e| OcrError::ProcessingFailed(format!("Failed to recognize text: {}", e)))?;

    let mean_text_conf = api.mean_text_conf().unwrap_or(-1);

    log_ci_debug(ci_debug_enabled, "recognize", || {
        format!("completed mean_text_conf={}", mean_text_conf)
    });

    let word_confidence_stats = match api.all_word_confidences() {
        Ok(confidences) if !confidences.is_empty() => {
            let word_count = confidences.len();
            let low_conf_word_count = confidences.iter().filter(|&&c| c < 50).count();

            let mut sorted = confidences.clone();
            sorted.sort_unstable();
            let median_word_conf = if word_count % 2 == 0 {
                (sorted[word_count / 2 - 1] + sorted[word_count / 2]) / 2
            } else {
                sorted[word_count / 2]
            };

            let p10_idx = ((word_count as f64 - 1.0) * 0.1).floor() as usize;
            let p10_word_conf = sorted[p10_idx.min(word_count - 1)];

            Some((median_word_conf, p10_word_conf, word_count, low_conf_word_count))
        }
        Ok(_) => match api.mean_text_conf() {
            Ok(mean_conf) => Some((mean_conf, mean_conf, 0usize, 0usize)),
            Err(_) => None,
        },
        Err(_) => match api.mean_text_conf() {
            Ok(mean_conf) => Some((mean_conf, mean_conf, 0usize, 0usize)),
            Err(_) => None,
        },
    };

    let tsv_data_for_tables = if config.enable_table_detection || config.output_format == "tsv" {
        Some(
            api.get_tsv_text(0)
                .map_err(|e| OcrError::ProcessingFailed(format!("Failed to extract TSV: {}", e)))?,
        )
    } else {
        None
    };

    let mut hocr_document: Option<InternalDocument> = None;

    let (raw_content, mime_type) = match config.output_format.as_str() {
        "text" => {
            let text = api
                .get_utf8_text()
                .map_err(|e| OcrError::ProcessingFailed(format!("Failed to extract text: {}", e)))?;
            (text, "text/plain".to_string())
        }
        "markdown" => {
            let hocr = api
                .get_hocr_text(0)
                .map_err(|e| OcrError::ProcessingFailed(format!("Failed to extract hOCR: {}", e)))?;

            let internal_doc = parse_hocr_to_internal_document(&hocr);
            let content = internal_doc
                .elements
                .iter()
                .filter_map(|e| match e.kind {
                    ElementKind::PageBreak => None,
                    _ if !e.text.is_empty() => Some(e.text.clone()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n\n");
            hocr_document = Some(internal_doc);

            let mime_type = extraction_config
                .map(|c| match c.output_format {
                    crate::core::config::OutputFormat::Djot => "text/djot",
                    _ => "text/markdown",
                })
                .unwrap_or("text/markdown");

            (content, mime_type.to_string())
        }
        "hocr" => {
            let hocr = api
                .get_hocr_text(0)
                .map_err(|e| OcrError::ProcessingFailed(format!("Failed to extract hOCR: {}", e)))?;
            (hocr, "text/html".to_string())
        }
        "tsv" => {
            let tsv = tsv_data_for_tables
                .as_ref()
                .ok_or_else(|| OcrError::ProcessingFailed("TSV data not available".to_string()))?
                .clone();
            (tsv, "text/plain".to_string())
        }
        _ => {
            return Err(OcrError::InvalidConfiguration(format!(
                "Unsupported output format: {}",
                config.output_format
            )));
        }
    };

    let mut metadata = HashMap::new();
    metadata.insert(
        crate::ocr::OCR_PROCESSED_IMAGE_WIDTH_METADATA_KEY.to_string(),
        serde_json::Value::Number(ocr_image_width.into()),
    );
    metadata.insert(
        crate::ocr::OCR_PROCESSED_IMAGE_HEIGHT_METADATA_KEY.to_string(),
        serde_json::Value::Number(ocr_image_height.into()),
    );
    metadata.insert(
        "language".to_string(),
        serde_json::Value::String(config.language.clone()),
    );
    metadata.insert("psm".to_string(), serde_json::Value::String(config.psm.to_string()));
    metadata.insert("table_count".to_string(), serde_json::Value::Number(0.into()));
    metadata.insert("tables_detected".to_string(), serde_json::Value::Number(0.into()));
    if config.output_format == "markdown" {
        metadata.insert(
            "source_format".to_string(),
            serde_json::Value::String("hocr".to_string()),
        );
    }

    if mean_text_conf >= 0 {
        metadata.insert(
            "mean_text_conf".to_string(),
            serde_json::Value::Number(serde_json::Number::from(mean_text_conf)),
        );
    }

    if let Some((median_conf, p10_conf, word_count, low_conf_count)) = word_confidence_stats {
        metadata.insert(
            "median_word_conf".to_string(),
            serde_json::Value::Number(serde_json::Number::from(median_conf)),
        );
        metadata.insert(
            "p10_word_conf".to_string(),
            serde_json::Value::Number(serde_json::Number::from(p10_conf)),
        );
        metadata.insert(
            "word_count".to_string(),
            serde_json::Value::Number(serde_json::Number::from(word_count)),
        );
        metadata.insert(
            "low_conf_word_count".to_string(),
            serde_json::Value::Number(serde_json::Number::from(low_conf_count)),
        );
    }

    if let Some((orient_deg, orient_conf, ref script_name, script_conf)) = detected_orientation {
        metadata.insert(
            crate::ocr::OCR_ORIENTATION_DEGREES_METADATA_KEY.to_string(),
            serde_json::Value::Number(serde_json::Number::from(orient_deg)),
        );
        metadata.insert(
            "orientation_confidence".to_string(),
            serde_json::Value::Number(
                serde_json::Number::from_f64(orient_conf as f64).unwrap_or(serde_json::Number::from(0)),
            ),
        );
        metadata.insert(
            "script_name".to_string(),
            serde_json::Value::String(script_name.clone()),
        );
        metadata.insert(
            "script_confidence".to_string(),
            serde_json::Value::Number(
                serde_json::Number::from_f64(script_conf as f64).unwrap_or(serde_json::Number::from(0)),
            ),
        );
        if orient_deg != 0 && orient_conf > MIN_ORIENTATION_CONFIDENCE {
            metadata.insert(
                crate::ocr::OCR_AUTO_ROTATED_METADATA_KEY.to_string(),
                serde_json::Value::Bool(true),
            );
        }
    }

    let mut tables = Vec::new();
    let mut ocr_elements = None;

    if config.enable_table_detection {
        let tsv_data = tsv_data_for_tables.as_ref().unwrap();

        let words = extract_words_from_tsv(tsv_data, config.table_min_confidence)?;

        if words.len() >= 6 {
            let table = reconstruct_table(&words, config.table_column_threshold, config.table_row_threshold_ratio);
            if !table.is_empty() && !table[0].is_empty() {
                #[cfg(feature = "pdf")]
                let cleaned = post_process_table(table, false, false);
                #[cfg(not(feature = "pdf"))]
                let cleaned = Some(table);
                if let Some(cleaned) = cleaned {
                    metadata.insert("table_count".to_string(), serde_json::Value::Number(1.into()));
                    metadata.insert("tables_detected".to_string(), serde_json::Value::Number(1.into()));
                    metadata.insert(
                        "table_rows".to_string(),
                        serde_json::Value::Number(cleaned.len().into()),
                    );
                    metadata.insert(
                        "table_cols".to_string(),
                        serde_json::Value::Number(cleaned[0].len().into()),
                    );

                    let markdown_table = table_to_markdown(&cleaned);

                    let bbox = if !words.is_empty() {
                        let left = words.iter().map(|w| w.left).min().unwrap_or(0);
                        let top = words.iter().map(|w| w.top).min().unwrap_or(0);
                        let right = words.iter().map(|w| w.left + w.width).max().unwrap_or(0);
                        let bottom = words.iter().map(|w| w.top + w.height).max().unwrap_or(0);
                        Some(OcrTableBoundingBox {
                            left,
                            top,
                            right,
                            bottom,
                        })
                    } else {
                        None
                    };

                    tables.push(OcrTable {
                        cells: cleaned,
                        markdown: markdown_table,
                        page_number: 1,
                        bounding_box: bbox,
                    });
                }
            }
        }
    }

    let iterator_elements = extract_elements_via_iterator(&api, 1, config.min_confidence);
    match iterator_elements {
        Ok(elements) if !elements.is_empty() => {
            ocr_elements = Some(elements);
        }
        _ => {
            if let Some(ref tsv_data) = tsv_data_for_tables {
                let elements = parse_tsv_to_elements(tsv_data, config.min_confidence);
                if !elements.is_empty() {
                    ocr_elements = Some(elements);
                }
            }
        }
    }

    let mut content = strip_control_characters(&raw_content).into_owned();

    let is_markdown_output = extraction_config
        .map(|c| c.output_format == crate::core::config::OutputFormat::Markdown)
        .unwrap_or(config.output_format == "markdown");

    if !tables.is_empty()
        && is_markdown_output
        && let Some(ref tsv_data) = tsv_data_for_tables
    {
        let rebuilt = build_content_with_inline_tables(tsv_data, &tables, config.table_min_confidence);
        if !rebuilt.is_empty() {
            content = rebuilt;
            metadata.insert(
                "pre_formatted".to_string(),
                serde_json::Value::String("markdown".to_string()),
            );
        }
    }

    drop(api);

    Ok(OcrExtractionResult {
        content,
        mime_type,
        metadata,
        tables,
        ocr_elements,
        internal_document: hocr_document,
    })
}

/// Process an image file and return OCR results.
///
/// # Arguments
///
/// * `file_path` - Path to image file
/// * `config` - OCR configuration
/// * `cache` - Cache instance
/// * `output_format` - Optional output format (Plain, Markdown, Djot) for proper mime_type handling
///
/// # Returns
///
/// OCR extraction result
pub(super) fn process_image_file_with_cache(
    file_path: &str,
    config: &TesseractConfig,
    cache: &OcrCache,
    api_pool: &Arc<TesseractApiPool>,
    output_format: Option<crate::core::config::OutputFormat>,
) -> Result<OcrExtractionResult, OcrError> {
    let image_bytes = std::fs::read(file_path)
        .map_err(|e| OcrError::IOError(format!("Failed to read file '{}': {}", file_path, e)))?;
    process_image_with_cache(&image_bytes, config, cache, api_pool, output_format)
}

/// Check if a language value is the "all" wildcard (case-insensitive).
fn is_all_languages(lang: &str) -> bool {
    let lower = lang.to_ascii_lowercase();
    lower == "all" || lower == "*"
}

/// Resolve the "all"/"*" wildcard in a config's language field.
///
/// If the language is a wildcard, scans the tessdata directory for installed
/// languages and returns a new config with the resolved language string.
/// Otherwise returns `None`, indicating the original config should be used as-is.
fn resolve_config_language(config: &TesseractConfig) -> Result<Option<TesseractConfig>, OcrError> {
    if is_all_languages(&config.language) {
        let bootstrap_langs = vec!["eng".to_string()];
        let tessdata_path = resolve_tessdata_path(&bootstrap_langs, config.tessdata_path.as_deref())?;
        let resolved = resolve_all_installed_languages(&tessdata_path)?;
        let mut resolved_config = config.clone();
        resolved_config.language = resolved;
        Ok(Some(resolved_config))
    } else {
        Ok(None)
    }
}

/// Process an image and return OCR results, using cache if enabled.
///
/// Resolves the `"all"` / `"*"` language wildcard, then delegates to
/// [`process_image_resolved`] for caching and OCR execution.
///
/// # Arguments
///
/// * `image_bytes` - Raw image data
/// * `config` - OCR configuration
/// * `cache` - Cache instance
/// * `output_format` - Optional output format (Plain, Markdown, Djot) for proper mime_type handling
///
/// # Returns
///
/// OCR extraction result
pub(super) fn process_image_with_cache(
    image_bytes: &[u8],
    config: &TesseractConfig,
    cache: &OcrCache,
    api_pool: &Arc<TesseractApiPool>,
    output_format: Option<crate::core::config::OutputFormat>,
) -> Result<OcrExtractionResult, OcrError> {
    config.validate().map_err(OcrError::InvalidConfiguration)?;

    let resolved = resolve_config_language(config)?;
    let config = resolved.as_ref().unwrap_or(config);

    process_image_resolved(image_bytes, config, cache, api_pool, output_format)
}

/// Inner implementation operating on an already-resolved config.
///
/// Handles cache lookup, OCR execution, and cache storage. Callers are
/// responsible for validating and resolving wildcards in the config before
/// calling this function.
fn process_image_resolved(
    image_bytes: &[u8],
    config: &TesseractConfig,
    cache: &OcrCache,
    api_pool: &Arc<TesseractApiPool>,
    output_format: Option<crate::core::config::OutputFormat>,
) -> Result<OcrExtractionResult, OcrError> {
    let image_hash = crate::cache::blake3_hash_bytes(image_bytes);

    let config_str = hash_config(config);

    if config.use_cache
        && let Some(cached_result) = cache.get_cached_result(&image_hash, "tesseract", &config_str)?
    {
        #[cfg(feature = "otel")]
        tracing::Span::current().record("cache.hit", true);
        return Ok(cached_result);
    }

    #[cfg(feature = "otel")]
    tracing::Span::current().record("cache.hit", false);

    let extraction_config = output_format.map(|fmt| ExtractionConfig {
        output_format: fmt,
        ..Default::default()
    });

    let result = perform_ocr(image_bytes, config, api_pool, extraction_config.as_ref())?;

    if config.use_cache {
        let _ = cache.set_cached_result(&image_hash, "tesseract", &config_str, &result);
    }

    Ok(result)
}

/// Process multiple image files in parallel using Rayon.
///
/// Validates and resolves the language wildcard once, then processes all files
/// in parallel using [`process_image_resolved`] directly (skipping redundant
/// per-image resolution).
///
/// Results are returned in the same order as the input file paths.
#[cfg(test)]
pub(super) fn process_image_files_batch(
    file_paths: Vec<String>,
    config: &TesseractConfig,
    cache: &OcrCache,
    api_pool: &Arc<TesseractApiPool>,
) -> Vec<BatchItemResult> {
    #[cfg(not(target_arch = "wasm32"))]
    use rayon::prelude::*;

    if let Err(e) = config.validate().map_err(OcrError::InvalidConfiguration) {
        return file_paths
            .into_iter()
            .map(|path| BatchItemResult {
                file_path: path,
                success: false,
                result: None,
                error: Some(e.to_string()),
            })
            .collect();
    }

    let resolved = match resolve_config_language(config) {
        Ok(r) => r,
        Err(e) => {
            return file_paths
                .into_iter()
                .map(|path| BatchItemResult {
                    file_path: path,
                    success: false,
                    result: None,
                    error: Some(e.to_string()),
                })
                .collect();
        }
    };
    let config = resolved.as_ref().unwrap_or(config);

    #[cfg(not(target_arch = "wasm32"))]
    {
        file_paths
            .par_iter()
            .map(|path| {
                let image_bytes = match std::fs::read(path) {
                    Ok(b) => b,
                    Err(e) => {
                        return BatchItemResult {
                            file_path: path.clone(),
                            success: false,
                            result: None,
                            error: Some(
                                OcrError::IOError(format!("Failed to read file '{}': {}", path, e)).to_string(),
                            ),
                        };
                    }
                };
                match process_image_resolved(&image_bytes, config, cache, api_pool, None) {
                    Ok(result) => BatchItemResult {
                        file_path: path.clone(),
                        success: true,
                        result: Some(result),
                        error: None,
                    },
                    Err(e) => BatchItemResult {
                        file_path: path.clone(),
                        success: false,
                        result: None,
                        error: Some(e.to_string()),
                    },
                }
            })
            .collect()
    }
    #[cfg(target_arch = "wasm32")]
    {
        file_paths
            .iter()
            .map(|path| {
                let image_bytes = match std::fs::read(path) {
                    Ok(b) => b,
                    Err(e) => {
                        return BatchItemResult {
                            file_path: path.clone(),
                            success: false,
                            result: None,
                            error: Some(
                                OcrError::IOError(format!("Failed to read file '{}': {}", path, e)).to_string(),
                            ),
                        };
                    }
                };
                match process_image_resolved(&image_bytes, config, cache, api_pool, None) {
                    Ok(result) => BatchItemResult {
                        file_path: path.clone(),
                        success: true,
                        result: Some(result),
                        error: None,
                    },
                    Err(e) => BatchItemResult {
                        file_path: path.clone(),
                        success: false,
                        result: None,
                        error: Some(e.to_string()),
                    },
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_is_all_languages() {
        assert!(is_all_languages("all"));
        assert!(is_all_languages("ALL"));
        assert!(is_all_languages("All"));
        assert!(is_all_languages("*"));
        assert!(!is_all_languages("eng"));
        assert!(!is_all_languages("eng+fra"));
        assert!(!is_all_languages(""));
    }

    #[test]
    fn test_resolve_config_language_passthrough() {
        let config = TesseractConfig {
            language: "eng".to_string(),
            ..TesseractConfig::default()
        };
        let resolved = resolve_config_language(&config).unwrap();
        assert!(resolved.is_none(), "non-wildcard should return None (no clone)");
    }

    #[test]
    fn test_compute_image_hash_deterministic() {
        let image_bytes = vec![1, 2, 3, 4, 5];

        let hash1 = crate::cache::blake3_hash_bytes(&image_bytes);
        let hash2 = crate::cache::blake3_hash_bytes(&image_bytes);

        assert_eq!(hash1, hash2);
        assert_eq!(hash1.len(), 32);
    }

    #[test]
    fn test_compute_image_hash_different_data() {
        let image_bytes1 = vec![1, 2, 3, 4, 5];
        let image_bytes2 = vec![5, 4, 3, 2, 1];

        let hash1 = crate::cache::blake3_hash_bytes(&image_bytes1);
        let hash2 = crate::cache::blake3_hash_bytes(&image_bytes2);

        assert_ne!(hash1, hash2);
    }

    #[test]
    fn test_log_ci_debug_disabled() {
        log_ci_debug(false, "test_stage", || "test message".to_string());
    }

    #[test]
    fn test_log_ci_debug_enabled() {
        log_ci_debug(true, "test_stage", || "test message".to_string());
    }

    #[test]
    fn test_process_image_file_nonexistent() {
        let temp_dir = tempdir().unwrap();
        let cache = OcrCache::new(Some(temp_dir.path().to_path_buf())).unwrap();
        let config = TesseractConfig {
            output_format: "text".to_string(),
            enable_table_detection: false,
            use_cache: false,
            ..TesseractConfig::default()
        };

        let api_pool = TesseractApiPool::new();
        let result = process_image_file_with_cache("/nonexistent/file.png", &config, &cache, &api_pool, None);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Failed to read file"));
    }

    #[test]
    fn test_process_image_invalid_image_data() {
        let temp_dir = tempdir().unwrap();
        let cache = OcrCache::new(Some(temp_dir.path().to_path_buf())).unwrap();
        let config = TesseractConfig {
            output_format: "text".to_string(),
            enable_table_detection: false,
            use_cache: false,
            ..TesseractConfig::default()
        };

        let invalid_data = vec![0, 1, 2, 3, 4];
        let api_pool = TesseractApiPool::new();
        let result = process_image_with_cache(&invalid_data, &config, &cache, &api_pool, None);

        assert!(result.is_err());
    }

    #[test]
    fn test_preprocess_pix_produces_grayscale_with_valid_resolution() {
        let width = 32;
        let height = 32;
        let mut rgb_data = Vec::with_capacity((width * height * 3) as usize);
        for y in 0..height {
            for x in 0..width {
                let value = if (x + y) % 2 == 0 { 32 } else { 224 };
                rgb_data.extend_from_slice(&[value, value, value]);
            }
        }
        let mut pix = xberg_tesseract::Pix::from_raw_rgb(&rgb_data, width, height).unwrap();
        pix.set_resolution(300, 300).unwrap();

        let processed = preprocess_pix(pix, false).unwrap();

        assert_eq!(processed.width(), width as i32);
        assert_eq!(processed.height(), height as i32);
        assert_eq!(processed.depth(), 8);
        assert_eq!(processed.get_resolution().unwrap(), (72, 72));
    }

    #[test]
    fn test_prepare_ocr_image_without_config_preserves_shadowed_rgb() {
        let rgb_data = vec![0, 1, 2, 3, 4, 5];

        let prepared = prepare_ocr_image(rgb_data.clone(), 2, 1, None, false);

        assert_eq!(prepared.data, rgb_data);
        assert_eq!(prepared.width, 2);
        assert_eq!(prepared.height, 1);
        assert_eq!(prepared.source_dpi, RAW_IMAGE_SOURCE_DPI);
        assert!(!prepared.apply_pix_preprocessing);
    }

    #[test]
    fn test_clean_white_rgb_selects_default_preprocessing() {
        const SAMPLE_PIXEL_COUNT: usize = 16;
        let rgb_data = vec![u8::MAX; SAMPLE_PIXEL_COUNT * RGB_CHANNEL_COUNT];

        assert!(should_apply_default_preprocessing(&rgb_data));
    }

    #[test]
    fn test_prepare_ocr_image_without_config_preprocesses_clean_white_rgb() {
        const WIDTH: u32 = 4;
        const HEIGHT: u32 = 4;
        let rgb_data = vec![u8::MAX; WIDTH as usize * HEIGHT as usize * RGB_CHANNEL_COUNT];

        let prepared = prepare_ocr_image(rgb_data, WIDTH, HEIGHT, None, false);

        assert!(prepared.apply_pix_preprocessing);
    }

    #[test]
    fn test_shadowed_rgb_skips_default_preprocessing() {
        const SAMPLE_PIXEL_COUNT: usize = 16;
        const SHADOWED_CHANNEL_VALUE: u8 = 128;
        let rgb_data = vec![SHADOWED_CHANNEL_VALUE; SAMPLE_PIXEL_COUNT * RGB_CHANNEL_COUNT];

        assert!(!should_apply_default_preprocessing(&rgb_data));
    }

    #[test]
    fn test_prepare_ocr_image_with_config_keeps_preprocessing_enabled() {
        let rgb_data = vec![255; 12];
        let preprocessing = crate::types::ImagePreprocessingConfig {
            target_dpi: 72,
            ..Default::default()
        };

        let prepared = prepare_ocr_image(rgb_data, 2, 2, Some(&preprocessing), false);

        assert!(prepared.apply_pix_preprocessing);
    }

    #[test]
    fn test_should_invert_for_polarity_dark_background_with_text() {
        // Dark background (mean well below threshold) with a clear light-text
        // fraction should trigger auto-detected inversion.
        assert!(should_invert_for_polarity(40.0, 0.05, false));
    }

    #[test]
    fn test_should_invert_for_polarity_light_background_not_inverted() {
        // Typical dark-text-on-light-paper scan: high mean, no auto-invert.
        assert!(!should_invert_for_polarity(220.0, 0.9, false));
    }

    #[test]
    fn test_should_invert_for_polarity_uniformly_dark_image_not_inverted() {
        // Dark mean but essentially no light pixels: a uniformly dark, non-text
        // image (e.g. a dark photo) should NOT be auto-inverted.
        assert!(!should_invert_for_polarity(20.0, 0.0, false));
    }

    #[test]
    fn test_should_invert_for_polarity_force_true_overrides_light_background() {
        // Explicit invert_colors=true forces inversion even on a light-mean image.
        assert!(should_invert_for_polarity(220.0, 0.9, true));
    }

    #[test]
    fn test_should_invert_for_polarity_force_false_still_auto_detects() {
        assert!(should_invert_for_polarity(10.0, 0.5, false));
    }

    #[test]
    fn test_preprocess_pix_inverts_light_on_dark_image() {
        // Synthetic light-text-on-dark-background image: dark background (20)
        // with a bright "text" region covering ~6% of the image.
        let width = 40u32;
        let height = 40u32;
        let mut rgb_data = Vec::with_capacity((width * height * 3) as usize);
        for y in 0..height {
            for x in 0..width {
                let is_text = (10..13).contains(&y) && (5..25).contains(&x);
                let value = if is_text { 230u8 } else { 20u8 };
                rgb_data.extend_from_slice(&[value, value, value]);
            }
        }

        let pix = xberg_tesseract::Pix::from_raw_rgb(&rgb_data, width, height).unwrap();
        let original_gray = pix.to_grayscale().unwrap();
        let (original_mean, _) = original_gray
            .grayscale_stats(LIGHT_PIXEL_VALUE_THRESHOLD, POLARITY_SAMPLE_STRIDE)
            .unwrap();
        drop(original_gray);

        // Auto-detection with no explicit override should invert: the result's
        // background (post background_normalize + grayscale) should be light,
        // i.e. brighter on average than the raw dark-background source.
        let processed = preprocess_pix(pix, false).unwrap();
        let (processed_mean, _) = processed
            .grayscale_stats(LIGHT_PIXEL_VALUE_THRESHOLD, POLARITY_SAMPLE_STRIDE)
            .unwrap();

        assert!(
            processed_mean > original_mean,
            "expected inverted+normalized output to be brighter than the dark-background \
             source (original_mean={original_mean}, processed_mean={processed_mean})"
        );
    }

    #[test]
    fn test_preprocess_pix_does_not_invert_dark_on_light_image() {
        // A normal dark-text-on-light-paper image should not be inverted:
        // processing should keep (or increase) brightness, not flip polarity.
        let width = 40u32;
        let height = 40u32;
        let mut rgb_data = Vec::with_capacity((width * height * 3) as usize);
        for y in 0..height {
            for x in 0..width {
                let is_text = (10..13).contains(&y) && (5..25).contains(&x);
                let value = if is_text { 20u8 } else { 230u8 };
                rgb_data.extend_from_slice(&[value, value, value]);
            }
        }

        let pix = xberg_tesseract::Pix::from_raw_rgb(&rgb_data, width, height).unwrap();
        let processed = preprocess_pix(pix, false).unwrap();
        let (processed_mean, _) = processed
            .grayscale_stats(LIGHT_PIXEL_VALUE_THRESHOLD, POLARITY_SAMPLE_STRIDE)
            .unwrap();

        assert!(
            processed_mean > DARK_BACKGROUND_MEAN_THRESHOLD,
            "expected light-background image to stay light after preprocessing (mean={processed_mean})"
        );
    }

    #[cfg(auto_rotate)]
    #[test]
    #[cfg(auto_rotate)]
    fn test_rotate_rgb_image_data_identity() {
        let data: Vec<u8> = (0..18).collect();
        let (out, w, h) = rotate_rgb_image_data(&data, 2, 3, 0);
        assert_eq!(out, data);
        assert_eq!(w, 2);
        assert_eq!(h, 3);
    }

    #[cfg(auto_rotate)]
    #[test]
    #[cfg(auto_rotate)]
    fn test_rotate_rgb_image_data_180() {
        let data = vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12];
        let (out, w, h) = rotate_rgb_image_data(&data, 2, 2, 180);
        assert_eq!(w, 2);
        assert_eq!(h, 2);
        assert_eq!(out, vec![10, 11, 12, 7, 8, 9, 4, 5, 6, 1, 2, 3]);
    }

    #[cfg(auto_rotate)]
    #[test]
    #[cfg(auto_rotate)]
    fn test_rotate_rgb_image_data_90_swaps_dimensions() {
        let data: Vec<u8> = (0..18).collect();
        let (_, w, h) = rotate_rgb_image_data(&data, 2, 3, 90);
        assert_eq!(w, 3);
        assert_eq!(h, 2);
    }

    #[cfg(auto_rotate)]
    #[test]
    #[cfg(auto_rotate)]
    fn test_rotate_rgb_image_data_270_swaps_dimensions() {
        let data: Vec<u8> = (0..18).collect();
        let (_, w, h) = rotate_rgb_image_data(&data, 2, 3, 270);
        assert_eq!(w, 3);
        assert_eq!(h, 2);
    }

    #[cfg(auto_rotate)]
    #[test]
    #[cfg(auto_rotate)]
    fn test_rotate_rgb_image_data_90_then_270_is_identity() {
        let data: Vec<u8> = (0..18).collect();
        let (rotated_90, w1, h1) = rotate_rgb_image_data(&data, 2, 3, 90);
        let (back, w2, h2) = rotate_rgb_image_data(&rotated_90, w1, h1, 270);
        assert_eq!(w2, 2);
        assert_eq!(h2, 3);
        assert_eq!(back, data);
    }

    #[cfg(auto_rotate)]
    #[test]
    #[cfg(auto_rotate)]
    fn test_rotate_rgb_image_data_unsupported_angle() {
        let data: Vec<u8> = (0..12).collect();
        let (out, w, h) = rotate_rgb_image_data(&data, 2, 2, 45);
        assert_eq!(out, data);
        assert_eq!(w, 2);
        assert_eq!(h, 2);
    }
}
