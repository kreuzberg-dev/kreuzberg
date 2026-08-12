//! LaTeX recognition for rasterized formula regions.
//!
//! Wraps the RapidLaTeXOCR model set (MIT, derived from pix2tex/LaTeX-OCR):
//! an image resizer, a ViT encoder, and an autoregressive transformer decoder,
//! all ONNX. The decode loop runs in Rust: the decoder has no KV cache, so each
//! step feeds the full token prefix, and sampling at the reference temperature
//! of `1e-5` is equivalent to argmax, which is what this port does.
//!
//! Models download on demand from the upstream release and verify by SHA256,
//! like the tessdata and layout model managers.

use image::RgbImage;
use ndarray::{Array2, Array4};
use ort::inputs;
use ort::session::Session;
use ort::value::{Tensor, TensorRef};

use crate::core::config::AccelerationConfig;
use crate::layout::error::LayoutError;
#[cfg(not(paddle_ocr))]
use crate::layout::model_manager::ModelManifestEntry;
use crate::layout::session::build_session;
#[cfg(paddle_ocr)]
use crate::paddle_ocr::ModelManifestEntry;

/// Upstream release that hosts the model files (MIT licensed).
const RELEASE_BASE_URL: &str = "https://github.com/RapidAI/RapidLaTeXOCR/releases/download/v0.0.0";

/// Model files with their pinned checksums, verified after download.
const MODEL_FILES: [(&str, &str, u64); 4] = [
    (
        "image_resizer.onnx",
        "e0b075c39700f64d50400f39c8fc186bbb3b5d84d31864008313f376603aca9d",
        38_967_751,
    ),
    (
        "encoder.onnx",
        "01bf5dc25539ca0cd5b1bd29296ea495977a6ba5f629dc4178277809d26e5e7d",
        89_008_136,
    ),
    (
        "decoder.onnx",
        "bd695497bf1b22279b7626f5916c79226e1e244c84355f8da7edfd2d921d0072",
        50_952_726,
    ),
    (
        "tokenizer.json",
        "1dc27b18d6a518d0d5ff3f4bb7bd98521fe80ad39e5b2a246d4109f1bb9d5019",
        24_174,
    ),
];

/// Input geometry limits from the upstream configuration.
const MAX_WIDTH: u32 = 672;
const MAX_HEIGHT: u32 = 192;
const MIN_WIDTH: u32 = 32;
const MIN_HEIGHT: u32 = 32;
/// Pad dimension granularity; the resizer predicts widths in these buckets.
const DIVISOR: u32 = 32;

/// Decoder token contract. Ids 0..=3 are `<pad>`, `<s>`, `</s>`, `<unk>`.
const BOS_TOKEN: i64 = 1;
const EOS_TOKEN: i64 = 2;
const FIRST_CONTENT_TOKEN: i64 = 4;
const MAX_SEQ_LEN: usize = 512;
/// Break the decode when this many consecutive identical tokens appear:
/// the model has degenerated and further steps only repeat.
const REPETITION_CUTOFF: usize = 8;

/// Grayscale normalization from the upstream preprocessing, applied after
/// min-max contrast normalization.
const NORM_MEAN: f32 = 0.7931;
const NORM_STD: f32 = 0.1738;

/// White border added around the ink bounding box, like the reference crop.
const INK_BORDER: u32 = 8;

/// After one initialization failure, later calls fail fast for this long
/// instead of re-paying the download deadline per region.
const INIT_RETRY_COOLDOWN: std::time::Duration = std::time::Duration::from_secs(60);

/// Local filenames of the downloaded model set.
#[derive(Debug, Clone)]
#[cfg_attr(alef, alef(skip))]
pub struct FormulaModelPaths {
    pub resizer: std::path::PathBuf,
    pub encoder: std::path::PathBuf,
    pub decoder: std::path::PathBuf,
    pub tokenizer: std::path::PathBuf,
}

/// Default cache directory. The layout model manager uses the same root.
fn default_cache_dir() -> std::path::PathBuf {
    hf_hub::resolve_cache_dir().join("formula-recognition")
}

/// The manifest for `cache manifest` / MCP model listings.
#[cfg_attr(alef, alef(skip))]
pub fn manifest() -> Vec<ModelManifestEntry> {
    MODEL_FILES
        .iter()
        .map(|(name, sha256, size)| ModelManifestEntry {
            relative_path: format!("formula-recognition/{name}"),
            sha256: (*sha256).to_string(),
            size_bytes: *size,
            source_url: format!("{RELEASE_BASE_URL}/{name}"),
        })
        .collect()
}

/// True when every model file is already cached in `dir` (default cache when
/// `None`).
#[cfg_attr(alef, alef(skip))]
pub fn models_cached_in(dir: Option<&std::path::Path>) -> bool {
    let dir = dir.map(std::path::Path::to_path_buf).unwrap_or_else(default_cache_dir);
    MODEL_FILES.iter().all(|(name, ..)| dir.join(name).is_file())
}

/// True when every model file is already cached in the default location.
#[cfg_attr(alef, alef(skip))]
pub fn models_cached() -> bool {
    models_cached_in(None)
}

/// Largest accepted model download; the encoder is ~89 MB.
const MAX_MODEL_BYTES: u64 = 256 * 1024 * 1024;

/// Download a file to a process-unique staging path. Publication (verify +
/// atomic rename under the in-process and cross-process locks, with the
/// Windows replace fallback) is the layout model manager's `atomic_publish`.
fn download_to_staging(url: &str, staging: &std::path::Path) -> Result<(), String> {
    let result = (|| {
        let response = ureq::get(url).call().map_err(|e| format!("download {url} failed: {e}"))?;
        if response.status() != 200 {
            return Err(format!("download {url} failed: HTTP {}", response.status()));
        }
        let bytes = response
            .into_body()
            .with_config()
            .limit(MAX_MODEL_BYTES)
            .read_to_vec()
            .map_err(|e| format!("download {url} read failed: {e}"))?;
        std::fs::write(staging, bytes).map_err(|e| format!("write {} failed: {e}", staging.display()))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(staging);
    }
    result
}

/// Download (if needed) and verify the model set in `dir` (default cache when
/// `None`), returning the local paths. Per-file downloads serialize through
/// the shared in-process download lock.
#[cfg_attr(alef, alef(skip))]
pub fn ensure_models_in(dir: Option<&std::path::Path>) -> Result<FormulaModelPaths, String> {
    let dir = dir.map(std::path::Path::to_path_buf).unwrap_or_else(default_cache_dir);
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create model cache dir {}: {e}", dir.display()))?;

    for (name, sha256, _) in MODEL_FILES {
        let target = dir.join(name);
        let lock = crate::model_download::download_lock(&format!("formula-recognition/{name}"));
        let _guard = lock.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if target.is_file() && crate::model_download::verify_sha256(&target, sha256, name).is_ok() {
            continue;
        }
        let url = format!("{RELEASE_BASE_URL}/{name}");
        let staging = dir.join(format!(".{name}.{}.tmp", std::process::id()));
        let dl_staging = staging.clone();
        crate::model_download::with_download_deadline(name, move || download_to_staging(&url, &dl_staging))?;
        let published = crate::layout::model_manager::atomic_publish(&staging, &target, &dir, sha256, name);
        let _ = std::fs::remove_file(&staging);
        published?;
    }

    Ok(FormulaModelPaths {
        resizer: dir.join("image_resizer.onnx"),
        encoder: dir.join("encoder.onnx"),
        decoder: dir.join("decoder.onnx"),
        tokenizer: dir.join("tokenizer.json"),
    })
}

/// Download (if needed) and verify the model set in the default location.
#[cfg_attr(alef, alef(skip))]
pub fn ensure_models() -> Result<FormulaModelPaths, String> {
    ensure_models_in(None)
}

/// Pool state: the loaded recognizer plus the acceleration it was built with,
/// so a changed acceleration config rebuilds the sessions, like the layout
/// engine's `matches_config`.
struct PooledRecognizer {
    recognizer: FormulaRecognizer,
    acceleration: Option<AccelerationConfig>,
}

/// Process-wide recognizer pool and the failure cooldown timestamp. Lock
/// poisoning recovers via `into_inner`: the recognizer holds no cross-call
/// state, so a panic mid-recognition leaves nothing inconsistent behind.
static RECOGNIZER: std::sync::Mutex<Option<PooledRecognizer>> = std::sync::Mutex::new(None);
static LAST_INIT_FAILURE: std::sync::Mutex<Option<std::time::Instant>> = std::sync::Mutex::new(None);

/// Recognize one region crop with the pooled recognizer, initializing (model
/// download included) or rebuilding it as needed. Errors are strings so
/// callers can degrade to plain OCR text with a warning.
///
/// The pool lock is held for the whole recognition; callers on an async
/// runtime must wrap this in `spawn_blocking`.
pub(crate) fn recognize_crop(crop: &RgbImage, accel: Option<&AccelerationConfig>) -> Result<Option<String>, String> {
    {
        let last = LAST_INIT_FAILURE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(at) = *last
            && at.elapsed() < INIT_RETRY_COOLDOWN
        {
            return Err("formula recognizer initialization failed recently; retry later".to_string());
        }
    }

    let mut pool = RECOGNIZER.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let rebuild = match pool.as_ref() {
        None => true,
        Some(pooled) => pooled.acceleration.as_ref() != accel,
    };
    if rebuild {
        let init = ensure_models().and_then(|paths| {
            FormulaRecognizer::load(&paths, accel).map_err(|e| format!("formula model load failed: {e}"))
        });
        match init {
            Ok(recognizer) => {
                *pool = Some(PooledRecognizer {
                    recognizer,
                    acceleration: accel.cloned(),
                });
                *LAST_INIT_FAILURE
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
            }
            Err(e) => {
                *LAST_INIT_FAILURE
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(std::time::Instant::now());
                return Err(e);
            }
        }
    }
    let pooled = pool.as_mut().expect("initialized above");
    pooled
        .recognizer
        .recognize(crop)
        .map_err(|e| format!("formula recognition failed: {e}"))
}

/// Recognize a crop off the async executor: the recognizer holds a
/// process-wide lock for the whole multi-step decode, so it must not park a
/// runtime worker. The inline arm is unreachable today (the feature implies
/// `tokio-runtime` and cannot be enabled on wasm32); it exists so the
/// function stays total if either implication ever changes.
pub(crate) async fn recognize_crop_blocking(
    crop: RgbImage,
    accel: Option<AccelerationConfig>,
) -> Result<Option<String>, String> {
    #[cfg(all(feature = "tokio-runtime", not(target_arch = "wasm32")))]
    {
        tokio::task::spawn_blocking(move || recognize_crop(&crop, accel.as_ref()))
            .await
            .map_err(|e| format!("formula recognition task failed: {e}"))?
    }
    #[cfg(any(not(feature = "tokio-runtime"), target_arch = "wasm32"))]
    {
        recognize_crop(&crop, accel.as_ref())
    }
}

/// Test-only public entry: the integration test exercises the full
/// download-load-recognize pipeline through this.
#[cfg_attr(alef, alef(skip))]
pub fn recognize_for_test(crop: &RgbImage) -> Result<Option<String>, String> {
    recognize_crop(crop, None)
}

/// A loaded recognizer: three ONNX sessions plus the BPE tokenizer.
pub(crate) struct FormulaRecognizer {
    resizer: Session,
    encoder: Session,
    decoder: Session,
    tokenizer: tokenizers::Tokenizer,
}

impl FormulaRecognizer {
    /// Load every session from `paths`.
    pub(crate) fn load(paths: &FormulaModelPaths, accel: Option<&AccelerationConfig>) -> Result<Self, LayoutError> {
        let threads = crate::core::config::concurrency::resolve_thread_budget(None);
        let resizer = build_session(&paths.resizer.to_string_lossy(), accel, threads)?;
        let encoder = build_session(&paths.encoder.to_string_lossy(), accel, threads)?;
        let decoder = build_session(&paths.decoder.to_string_lossy(), accel, threads)?;
        let tokenizer = tokenizers::Tokenizer::from_file(&paths.tokenizer)
            .map_err(|e| LayoutError::ModelDownload(format!("formula tokenizer failed to load: {e}")))?;
        Ok(Self {
            resizer,
            encoder,
            decoder,
            tokenizer,
        })
    }

    /// Recognize the LaTeX for one formula region crop.
    ///
    /// Returns `Ok(None)` when the crop carries no ink or the model produces
    /// no tokens.
    pub(crate) fn recognize(&mut self, crop: &RgbImage) -> Result<Option<String>, LayoutError> {
        let Some(gray) = preprocess_gray(crop) else {
            return Ok(None);
        };
        let sized = self.resize_to_model_width(&gray)?;
        let context = self.encode(&sized)?;
        let ids = self.greedy_decode(&context)?;
        if ids.is_empty() {
            return Ok(None);
        }
        let raw = self
            .tokenizer
            .decode(&ids.iter().map(|&i| i as u32).collect::<Vec<_>>(), false)
            .map_err(|e| LayoutError::InvalidOutput(format!("formula token decode failed: {e}")))?;
        let cleaned = post_process(&raw);
        Ok(if cleaned.is_empty() { None } else { Some(cleaned) })
    }

    /// The upstream adaptive-resize loop: the resizer model predicts the best
    /// model width bucket for the current render; iterate until stable.
    fn resize_to_model_width(&mut self, gray: &GrayCanvas) -> Result<Array4<f32>, LayoutError> {
        let mut width = gray.width.clamp(MIN_WIDTH, MAX_WIDTH);
        let mut height = gray.height.clamp(MIN_HEIGHT, MAX_HEIGHT);
        let mut tensor = gray.to_tensor(width, height);

        for _ in 0..10 {
            let input = Tensor::from_array(tensor.clone()).map_err(LayoutError::Ort)?;
            let outputs = self.resizer.run(inputs!["input" => input]).map_err(LayoutError::Ort)?;
            let (shape, data) = outputs[0].try_extract_tensor::<f32>().map_err(LayoutError::Ort)?;
            let argmax = argmax_last_row(shape, data)?;
            let predicted = ((argmax as u32) + 1) * DIVISOR;
            // The tensor's padded width is what the model judged.
            let current_padded = pad_up(width, DIVISOR);
            if predicted == current_padded {
                break;
            }
            let ratio = f64::from(predicted) / f64::from(current_padded);
            width = ((f64::from(width) * ratio).round().max(1.0) as u32).clamp(1, MAX_WIDTH);
            height = ((f64::from(height) * ratio).round().max(1.0) as u32).clamp(1, MAX_HEIGHT);
            tensor = gray.to_tensor(width, height);
        }
        Ok(tensor)
    }

    fn encode(&mut self, x: &Array4<f32>) -> Result<ndarray::Array3<f32>, LayoutError> {
        let input = Tensor::from_array(x.clone()).map_err(LayoutError::Ort)?;
        let outputs = self.encoder.run(inputs!["input" => input]).map_err(LayoutError::Ort)?;
        let (shape, data) = outputs[0].try_extract_tensor::<f32>().map_err(LayoutError::Ort)?;
        let dims: Vec<usize> = shape.iter().map(|&d| d as usize).collect();
        if dims.len() != 3 {
            return Err(LayoutError::InvalidOutput(format!(
                "formula encoder returned rank {} output, expected 3",
                dims.len()
            )));
        }
        ndarray::Array3::from_shape_vec((dims[0], dims[1], dims[2]), data.to_vec())
            .map_err(|e| LayoutError::InvalidOutput(format!("formula encoder output reshape failed: {e}")))
    }

    /// Greedy decode without KV cache: each step feeds the full prefix. The
    /// encoder context is passed as a zero-copy view each step.
    fn greedy_decode(&mut self, context: &ndarray::Array3<f32>) -> Result<Vec<i64>, LayoutError> {
        let mut out: Vec<i64> = vec![BOS_TOKEN];
        let mut repeats = 1usize;

        for _ in 0..MAX_SEQ_LEN {
            let window = &out[out.len().saturating_sub(MAX_SEQ_LEN)..];
            let len = window.len();
            let x = Array2::from_shape_vec((1, len), window.to_vec())
                .map_err(|e| LayoutError::InvalidOutput(format!("decoder input build failed: {e}")))?;
            let mask = Array2::from_elem((1, len), true);

            let x_t = Tensor::from_array(x).map_err(LayoutError::Ort)?;
            let mask_t = Tensor::from_array(mask).map_err(LayoutError::Ort)?;
            let ctx_t = TensorRef::from_array_view(context.view()).map_err(LayoutError::Ort)?;
            let outputs = self
                .decoder
                .run(inputs!["x" => x_t, "mask" => mask_t, "context" => ctx_t])
                .map_err(LayoutError::Ort)?;
            let (shape, data) = outputs[0].try_extract_tensor::<f32>().map_err(LayoutError::Ort)?;
            let next = argmax_last_row(shape, data)? as i64;
            if next == EOS_TOKEN {
                break;
            }
            repeats = if Some(&next) == out.last() { repeats + 1 } else { 1 };
            out.push(next);
            if repeats >= REPETITION_CUTOFF {
                // Degenerated output: drop the repeated tail and stop.
                let keep = out.len() - repeats;
                out.truncate(keep);
                break;
            }
        }

        Ok(out.into_iter().skip(1).filter(|&t| t >= FIRST_CONTENT_TOKEN).collect())
    }
}

/// A contrast-normalized, polarity-corrected, ink-cropped grayscale copy of
/// the source crop, renderable at any scale.
struct GrayCanvas {
    pixels: image::GrayImage,
    width: u32,
    height: u32,
}

impl GrayCanvas {
    /// Render at `(width, height)` content size, pad up to the divisor with
    /// white, normalize, and shape as `[1,1,H,W]`. Upscaling uses bilinear
    /// and downscaling Lanczos, like the reference.
    fn to_tensor(&self, width: u32, height: u32) -> Array4<f32> {
        let w = width.clamp(1, MAX_WIDTH);
        let h = height.clamp(1, MAX_HEIGHT);
        let filter = if w > self.width || h > self.height {
            image::imageops::FilterType::Triangle
        } else {
            image::imageops::FilterType::Lanczos3
        };
        let resized = image::imageops::resize(&self.pixels, w, h, filter);
        let padded_w = pad_up(w.max(MIN_WIDTH), DIVISOR);
        let padded_h = pad_up(h.max(MIN_HEIGHT), DIVISOR);

        let white = (1.0 - NORM_MEAN) / NORM_STD;
        let mut tensor = Array4::<f32>::from_elem((1, 1, padded_h as usize, padded_w as usize), white);
        for y in 0..h {
            for x in 0..w {
                let v = f32::from(resized.get_pixel(x, y).0[0]) / 255.0;
                tensor[[0, 0, y as usize, x as usize]] = (v - NORM_MEAN) / NORM_STD;
            }
        }
        tensor
    }
}

/// Round `v` up to the next multiple of `divisor`.
fn pad_up(v: u32, divisor: u32) -> u32 {
    v.div_ceil(divisor) * divisor
}

/// Argmax over the last `classes`-sized row of a flat logits buffer.
///
/// Errors instead of panicking when the buffer is empty or smaller than one
/// row: ONNX output shapes are model-controlled input.
fn argmax_last_row(shape: &[i64], data: &[f32]) -> Result<usize, LayoutError> {
    let classes = *shape.last().unwrap_or(&0) as usize;
    if classes == 0 || data.len() < classes {
        return Err(LayoutError::InvalidOutput(format!(
            "logits buffer of {} values cannot hold a row of {classes}",
            data.len()
        )));
    }
    let row = &data[data.len() - classes..];
    Ok(row
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(b.1))
        .map(|(i, _)| i)
        .unwrap_or(0))
}

/// Port of the reference `pad()` preprocessing: min-max contrast
/// normalization, polarity correction on the normalized mean, and a crop to
/// the ink bounding box with a small white border. Returns `None` for a
/// flat (ink-less) crop.
fn preprocess_gray(crop: &RgbImage) -> Option<GrayCanvas> {
    let gray = image::imageops::grayscale(crop);
    let (min, max) = gray
        .pixels()
        .fold((u8::MAX, u8::MIN), |(lo, hi), p| (lo.min(p.0[0]), hi.max(p.0[0])));
    if max <= min {
        return None; // flat crop: nothing to recognize
    }

    // Min-max normalize to the full range, then correct polarity so ink is
    // dark on light: the reference keeps the image when the normalized mean
    // is light and inverts otherwise.
    let range = f32::from(max - min);
    let mut normalized = image::GrayImage::new(gray.width(), gray.height());
    let mut sum: u64 = 0;
    for (src, dst) in gray.pixels().zip(normalized.pixels_mut()) {
        let v = ((f32::from(src.0[0] - min) / range) * 255.0).round() as u8;
        dst.0[0] = v;
        sum += u64::from(v);
    }
    let mean = sum / (normalized.len() as u64).max(1);
    if mean <= 128 {
        image::imageops::invert(&mut normalized);
    }

    // Crop to the ink bounding box plus a white border.
    let mut min_x = u32::MAX;
    let mut min_y = u32::MAX;
    let mut max_x = 0u32;
    let mut max_y = 0u32;
    for (x, y, p) in normalized.enumerate_pixels() {
        if p.0[0] < 250 {
            min_x = min_x.min(x);
            min_y = min_y.min(y);
            max_x = max_x.max(x);
            max_y = max_y.max(y);
        }
    }
    if min_x > max_x {
        return None; // normalization left pure white: no ink
    }
    let x0 = min_x.saturating_sub(INK_BORDER);
    let y0 = min_y.saturating_sub(INK_BORDER);
    let x1 = (max_x + 1 + INK_BORDER).min(normalized.width());
    let y1 = (max_y + 1 + INK_BORDER).min(normalized.height());
    let cropped = image::imageops::crop_imm(&normalized, x0, y0, x1 - x0, y1 - y0).to_image();

    let (width, height) = cropped.dimensions();
    Some(GrayCanvas {
        pixels: cropped,
        width,
        height,
    })
}

/// The upstream whitespace cleanup: spaces between non-letter tokens are
/// artifacts of BPE decoding, not LaTeX content. The explicit-space command
/// `\ ` is protected before the collapse and restored after, standing in for
/// the reference's negative lookahead.
fn post_process(s: &str) -> String {
    use std::sync::OnceLock;
    static TEXT_RE: OnceLock<regex::Regex> = OnceLock::new();
    static PAIR_RES: OnceLock<[regex::Regex; 3]> = OnceLock::new();

    let text_re = TEXT_RE.get_or_init(|| {
        regex::Regex::new(r"(\\(operatorname|mathrm|text|mathbf)\s?\*?\s?\{.*?\})").expect("static regex")
    });
    let pair_res = PAIR_RES.get_or_init(|| {
        let letter = "[a-zA-Z]";
        let noletter = r"[\W_^\d]";
        [
            regex::Regex::new(&format!(r"(?P<a>{noletter})\s+(?P<b>{noletter})")).expect("static regex"),
            regex::Regex::new(&format!(r"(?P<a>{noletter})\s+(?P<b>{letter})")).expect("static regex"),
            regex::Regex::new(&format!(r"(?P<a>{letter})\s+(?P<b>{noletter})")).expect("static regex"),
        ]
    });

    const SPACE_SENTINEL: &str = "\u{E000}";

    // Protect \operatorname{...}-style groups and the `\ ` command from the
    // pair collapse.
    let mut out = text_re
        .replace_all(s, |caps: &regex::Captures<'_>| caps[0].replace(' ', ""))
        .into_owned();
    out = out.replace("\\ ", SPACE_SENTINEL);

    loop {
        let mut next = out.clone();
        for re in pair_res.iter() {
            next = re.replace_all(&next, "$a$b").into_owned();
        }
        if next == out {
            break;
        }
        out = next;
    }
    out.replace(SPACE_SENTINEL, "\\ ").trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pad_up_rounds_to_divisor() {
        assert_eq!(pad_up(1, 32), 32);
        assert_eq!(pad_up(32, 32), 32);
        assert_eq!(pad_up(33, 32), 64);
    }

    #[test]
    fn argmax_picks_the_last_row_maximum() {
        // Two rows of three classes; the last row's max is index 1.
        let data = [9.0, 0.0, 0.0, 0.1, 5.0, 0.2];
        assert_eq!(argmax_last_row(&[2, 3], &data).unwrap(), 1);
    }

    #[test]
    fn argmax_on_empty_output_errors_instead_of_panicking() {
        assert!(argmax_last_row(&[0], &[]).is_err());
        assert!(argmax_last_row(&[1, 4], &[0.0]).is_err());
    }

    #[test]
    fn post_process_collapses_bpe_spaces() {
        assert_eq!(post_process("E = m c ^ { 2 }"), "E=m c^{2}");
    }

    #[test]
    fn post_process_keeps_operatorname_groups() {
        let s = r"\operatorname* { l i m }";
        let out = post_process(s);
        assert!(out.starts_with(r"\operatorname*"), "got: {out}");
        assert!(!out.contains("{ l"), "inner spaces collapse: {out}");
    }

    #[test]
    fn post_process_preserves_explicit_space_command() {
        assert_eq!(post_process(r"a \ b"), r"a\ b");
    }

    #[test]
    fn models_cached_in_requires_every_file() {
        let dir = std::env::temp_dir().join(format!("xberg-formula-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert!(!models_cached_in(Some(&dir)), "empty dir is not cached");
        for (name, ..) in MODEL_FILES {
            std::fs::write(dir.join(name), b"stub").unwrap();
        }
        assert!(models_cached_in(Some(&dir)), "all files present counts as cached");
        std::fs::remove_file(dir.join(MODEL_FILES[0].0)).unwrap();
        assert!(!models_cached_in(Some(&dir)), "one missing file breaks the cache");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn manifest_lists_every_model_file() {
        let m = manifest();
        assert_eq!(m.len(), 4);
        assert!(m.iter().all(|e| e.source_url.starts_with(RELEASE_BASE_URL)));
        assert!(m.iter().all(|e| e.sha256.len() == 64));
    }

    #[test]
    fn gray_canvas_tensor_is_padded_and_normalized() {
        let mut img = RgbImage::from_pixel(100, 40, image::Rgb([255, 255, 255]));
        for x in 30..70 {
            img.put_pixel(x, 20, image::Rgb([0, 0, 0]));
        }
        let canvas = preprocess_gray(&img).expect("inked crop");
        let t = canvas.to_tensor(canvas.width, canvas.height);
        let shape = t.shape();
        assert_eq!(shape[0], 1);
        assert_eq!(shape[1], 1);
        assert_eq!(shape[2] % 32, 0);
        assert_eq!(shape[3] % 32, 0);
        let white = (1.0 - NORM_MEAN) / NORM_STD;
        assert!((t[[0, 0, 0, 0]] - white).abs() < 0.2, "border stays white-ish");
    }

    #[test]
    fn blank_crops_yield_no_canvas() {
        let blank = RgbImage::from_pixel(96, 48, image::Rgb([255, 255, 255]));
        assert!(preprocess_gray(&blank).is_none());
        let gray_flat = RgbImage::from_pixel(96, 48, image::Rgb([180, 180, 180]));
        assert!(preprocess_gray(&gray_flat).is_none());
    }

    #[test]
    fn low_contrast_sparse_ink_survives_normalization() {
        // A thin, low-contrast stroke: min-max normalization must amplify it
        // into recognizable ink instead of dropping the crop.
        let mut img = RgbImage::from_pixel(300, 120, image::Rgb([230, 230, 230]));
        for x in 40..260 {
            img.put_pixel(x, 60, image::Rgb([180, 180, 180]));
        }
        let canvas = preprocess_gray(&img).expect("sparse ink must survive");
        // The ink crop shrinks the canvas to the stroke plus border.
        assert!(canvas.height <= 1 + 2 * INK_BORDER);
    }

    #[test]
    fn dark_background_inverts() {
        let mut img = RgbImage::from_pixel(64, 32, image::Rgb([10, 10, 10]));
        for x in 20..44 {
            img.put_pixel(x, 16, image::Rgb([240, 240, 240]));
        }
        let canvas = preprocess_gray(&img).expect("inked");
        // After polarity correction the majority background is light.
        let light = canvas.pixels.pixels().filter(|p| p.0[0] > 128).count();
        assert!(light * 2 > canvas.pixels.len());
    }
}
