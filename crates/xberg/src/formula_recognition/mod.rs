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
use ort::value::Tensor;

use crate::core::config::AccelerationConfig;
use crate::layout::error::LayoutError;
use crate::layout::model_manager::ModelManifestEntry;
use crate::layout::session::build_session;

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

/// Decoder token contract.
const BOS_TOKEN: i64 = 1;
const EOS_TOKEN: i64 = 2;
const MAX_SEQ_LEN: usize = 512;

/// Grayscale normalization from the upstream preprocessing.
const NORM_MEAN: f32 = 0.7931;
const NORM_STD: f32 = 0.1738;

/// Local filenames of the downloaded model set.
#[derive(Debug, Clone)]
pub(crate) struct FormulaModelPaths {
    pub resizer: std::path::PathBuf,
    pub encoder: std::path::PathBuf,
    pub decoder: std::path::PathBuf,
    pub tokenizer: std::path::PathBuf,
}

/// Cache directory for the formula recognition models.
fn cache_dir() -> std::path::PathBuf {
    crate::model_download::resolve_cache_dir("formula-recognition")
}

/// The manifest for `cache manifest` / MCP model listings.
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

/// True when every model file is already cached.
pub fn models_cached() -> bool {
    let dir = cache_dir();
    MODEL_FILES.iter().all(|(name, ..)| dir.join(name).is_file())
}

/// Largest accepted model download; the encoder is ~89 MB.
const MAX_MODEL_BYTES: u64 = 256 * 1024 * 1024;

/// Download a file and write it atomically next to its final path.
fn download_file(url: &str, target: &std::path::Path) -> Result<(), String> {
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
    let tmp = target.with_extension("partial");
    std::fs::write(&tmp, bytes).map_err(|e| format!("write {} failed: {e}", tmp.display()))?;
    std::fs::rename(&tmp, target).map_err(|e| format!("rename to {} failed: {e}", target.display()))?;
    Ok(())
}

/// Download (if needed) and verify the model set, returning the local paths.
pub fn ensure_models() -> Result<FormulaModelPaths, String> {
    let dir = cache_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create model cache dir {}: {e}", dir.display()))?;

    for (name, sha256, _) in MODEL_FILES {
        let target = dir.join(name);
        if target.is_file() && crate::model_download::verify_sha256(&target, sha256, name).is_ok() {
            continue;
        }
        let url = format!("{RELEASE_BASE_URL}/{name}");
        crate::model_download::with_download_deadline(name, || download_file(&url, &target))?;
        crate::model_download::verify_sha256(&target, sha256, name)?;
    }

    Ok(FormulaModelPaths {
        resizer: dir.join("image_resizer.onnx"),
        encoder: dir.join("encoder.onnx"),
        decoder: dir.join("decoder.onnx"),
        tokenizer: dir.join("tokenizer.json"),
    })
}

/// Process-wide recognizer pool: sessions load once and are reused.
static RECOGNIZER: std::sync::OnceLock<std::sync::Mutex<FormulaRecognizer>> = std::sync::OnceLock::new();

/// Recognize one region crop with the pooled recognizer, initializing it on
/// first use (model download included). Errors are strings so callers can
/// degrade to plain OCR text with a warning.
pub(crate) fn recognize_crop(crop: &RgbImage, accel: Option<&AccelerationConfig>) -> Result<Option<String>, String> {
    if RECOGNIZER.get().is_none() {
        let paths = ensure_models()?;
        let loaded = FormulaRecognizer::load(&paths, accel).map_err(|e| format!("formula model load failed: {e}"))?;
        let _ = RECOGNIZER.set(std::sync::Mutex::new(loaded));
    }
    let cell = RECOGNIZER.get().expect("set above");
    let mut recognizer = cell.lock().map_err(|_| "formula recognizer lock poisoned".to_string())?;
    recognizer
        .recognize(crop)
        .map_err(|e| format!("formula recognition failed: {e}"))
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
            .map_err(|e| LayoutError::Config(format!("formula tokenizer failed to load: {e}")))?;
        Ok(Self {
            resizer,
            encoder,
            decoder,
            tokenizer,
        })
    }

    /// Recognize the LaTeX for one formula region crop.
    ///
    /// Returns `Ok(None)` when the model produces no tokens.
    pub(crate) fn recognize(&mut self, crop: &RgbImage) -> Result<Option<String>, LayoutError> {
        let gray = preprocess_gray(crop);
        let sized = self.resize_to_model_width(&gray)?;
        let context = self.encode(&sized)?;
        let ids = self.greedy_decode(&context)?;
        if ids.is_empty() {
            return Ok(None);
        }
        let raw = self
            .tokenizer
            .decode(&ids.iter().map(|&i| i as u32).collect::<Vec<_>>(), false)
            .map_err(|e| LayoutError::Config(format!("formula token decode failed: {e}")))?;
        let cleaned = post_process(&raw);
        Ok(if cleaned.is_empty() { None } else { Some(cleaned) })
    }

    /// The upstream adaptive-resize loop: the resizer model predicts the best
    /// model width bucket for the current render; iterate until stable.
    fn resize_to_model_width(&mut self, gray: &GrayCanvas) -> Result<Array4<f32>, LayoutError> {
        let mut ratio = 1.0f32;
        let mut width = gray.width;
        let mut height = gray.height;
        let mut tensor = gray.to_tensor(width, height);

        for _ in 0..10 {
            let input = Tensor::from_array(tensor.clone()).map_err(LayoutError::Ort)?;
            let outputs = self
                .resizer
                .run(inputs!["input" => input])
                .map_err(LayoutError::Ort)?;
            let (shape, data) = outputs[0]
                .try_extract_tensor::<f32>()
                .map_err(LayoutError::Ort)?;
            let classes = *shape.last().unwrap_or(&1) as usize;
            let flat = &data[data.len() - classes..];
            let argmax = flat
                .iter()
                .enumerate()
                .max_by(|a, b| a.1.total_cmp(b.1))
                .map(|(i, _)| i)
                .unwrap_or(0);
            let predicted = ((argmax as u32) + 1) * DIVISOR;
            let padded_width = pad_up(width.min(MAX_WIDTH).max(MIN_WIDTH), DIVISOR);
            if predicted == padded_width {
                break;
            }
            ratio = predicted as f32 / padded_width as f32;
            width = ((width as f32) * ratio).round().max(1.0) as u32;
            height = ((height as f32) * ratio).round().max(1.0) as u32;
            tensor = gray.to_tensor(width, height);
        }
        Ok(tensor)
    }

    fn encode(&mut self, x: &Array4<f32>) -> Result<ndarray::Array3<f32>, LayoutError> {
        let input = Tensor::from_array(x.clone()).map_err(LayoutError::Ort)?;
        let outputs = self
            .encoder
            .run(inputs!["input" => input])
            .map_err(LayoutError::Ort)?;
        let (shape, data) = outputs[0]
            .try_extract_tensor::<f32>()
            .map_err(LayoutError::Ort)?;
        let dims: Vec<usize> = shape.iter().map(|&d| d as usize).collect();
        if dims.len() != 3 {
            return Err(LayoutError::Config(format!(
                "formula encoder returned rank {} output, expected 3",
                dims.len()
            )));
        }
        ndarray::Array3::from_shape_vec((dims[0], dims[1], dims[2]), data.to_vec())
            .map_err(|e| LayoutError::Config(format!("formula encoder output reshape failed: {e}")))
    }

    /// Greedy decode without KV cache: each step feeds the full prefix.
    fn greedy_decode(&mut self, context: &ndarray::Array3<f32>) -> Result<Vec<i64>, LayoutError> {
        let mut out: Vec<i64> = vec![BOS_TOKEN];

        for _ in 0..MAX_SEQ_LEN {
            let window = &out[out.len().saturating_sub(MAX_SEQ_LEN)..];
            let len = window.len();
            let x = Array2::from_shape_vec((1, len), window.to_vec())
                .map_err(|e| LayoutError::Config(format!("decoder input build failed: {e}")))?;
            let mask = Array2::from_elem((1, len), true);

            let x_t = Tensor::from_array(x).map_err(LayoutError::Ort)?;
            let mask_t = Tensor::from_array(mask).map_err(LayoutError::Ort)?;
            let ctx_t = Tensor::from_array(context.clone()).map_err(LayoutError::Ort)?;
            let outputs = self
                .decoder
                .run(inputs!["x" => x_t, "mask" => mask_t, "context" => ctx_t])
                .map_err(LayoutError::Ort)?;
            let (shape, data) = outputs[0]
                .try_extract_tensor::<f32>()
                .map_err(LayoutError::Ort)?;
            let vocab = *shape.last().unwrap_or(&1) as usize;
            let last = &data[data.len() - vocab..];
            let next = last
                .iter()
                .enumerate()
                .max_by(|a, b| a.1.total_cmp(b.1))
                .map(|(i, _)| i as i64)
                .unwrap_or(EOS_TOKEN);
            if next == EOS_TOKEN {
                break;
            }
            out.push(next);
        }

        Ok(out.into_iter().skip(1).filter(|&t| t != BOS_TOKEN && t != EOS_TOKEN).collect())
    }
}

/// A grayscale, background-normalized copy of the source crop plus its
/// content dimensions, renderable at any scale.
struct GrayCanvas {
    pixels: image::GrayImage,
    width: u32,
    height: u32,
}

impl GrayCanvas {
    /// Render at `(width, height)` content size, clamp into the model limits,
    /// pad up to the divisor with white, normalize, and shape as `[1,1,H,W]`.
    fn to_tensor(&self, width: u32, height: u32) -> Array4<f32> {
        let (mut w, mut h) = (width.max(1), height.max(1));
        // Clamp into the model's min/max box, keeping aspect.
        let scale = (MAX_WIDTH as f32 / w as f32)
            .min(MAX_HEIGHT as f32 / h as f32)
            .min(1.0);
        w = ((w as f32) * scale).round().max(1.0) as u32;
        h = ((h as f32) * scale).round().max(1.0) as u32;
        w = w.max(MIN_WIDTH.min(MAX_WIDTH));
        h = h.max(MIN_HEIGHT.min(MAX_HEIGHT));

        let resized = image::imageops::resize(&self.pixels, w, h, image::imageops::FilterType::Lanczos3);
        let padded_w = pad_up(w, DIVISOR);
        let padded_h = pad_up(h, DIVISOR);

        let mut tensor = Array4::<f32>::from_elem((1, 1, padded_h as usize, padded_w as usize), (1.0 - NORM_MEAN) / NORM_STD);
        for y in 0..h {
            for x in 0..w {
                let v = resized.get_pixel(x, y).0[0] as f32 / 255.0;
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

/// Grayscale the crop and invert when the background is dark, per the
/// upstream `pad()` preprocessing.
fn preprocess_gray(crop: &RgbImage) -> GrayCanvas {
    let mut gray = image::imageops::grayscale(crop);
    let mean: u64 = gray.pixels().map(|p| p.0[0] as u64).sum::<u64>() / (gray.len().max(1) as u64);
    if mean < 128 {
        for p in gray.pixels_mut() {
            p.0[0] = 255 - p.0[0];
        }
    }
    let (width, height) = gray.dimensions();
    GrayCanvas {
        pixels: gray,
        width,
        height,
    }
}

/// The upstream whitespace cleanup: spaces between non-letter tokens are
/// artifacts of BPE decoding, not LaTeX content.
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

    // Protect \operatorname{...}-style groups by collapsing their inner spaces
    // first, so the pair rules cannot touch them.
    let mut out = text_re
        .replace_all(s, |caps: &regex::Captures<'_>| caps[0].replace(' ', ""))
        .into_owned();

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
    out.trim().to_string()
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
    fn manifest_lists_every_model_file() {
        let m = manifest();
        assert_eq!(m.len(), 4);
        assert!(m.iter().all(|e| e.source_url.starts_with(RELEASE_BASE_URL)));
        assert!(m.iter().all(|e| e.sha256.len() == 64));
    }

    #[test]
    fn gray_canvas_tensor_is_padded_and_normalized() {
        let img = RgbImage::from_pixel(100, 40, image::Rgb([255, 255, 255]));
        let canvas = preprocess_gray(&img);
        let t = canvas.to_tensor(100, 40);
        let shape = t.shape();
        assert_eq!(shape[0], 1);
        assert_eq!(shape[1], 1);
        assert_eq!(shape[2] % 32, 0);
        assert_eq!(shape[3] % 32, 0);
        // White background normalizes to (1 - mean) / std everywhere.
        let expected = (1.0 - NORM_MEAN) / NORM_STD;
        assert!((t[[0, 0, 0, 0]] - expected).abs() < 1e-4);
    }

    #[test]
    fn dark_background_inverts() {
        let img = RgbImage::from_pixel(64, 32, image::Rgb([10, 10, 10]));
        let canvas = preprocess_gray(&img);
        assert!(canvas.pixels.get_pixel(0, 0).0[0] > 200);
    }
}
