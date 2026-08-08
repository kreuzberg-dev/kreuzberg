//! Pure-Rust QR-code detection over the bytes of an [`crate::types::ExtractedImage`].
//!
//! Used by the QR post-processor
//! (`crates/xberg/src/plugins/processor/builtin/qr.rs`) to populate
//! [`ExtractedImage::qr_codes`](crate::types::ExtractedImage::qr_codes).
//!
//! Decoding flow:
//!
//! 1. Decode the image bytes into a dynamic [`image::DynamicImage`] (PNG, JPEG,
//!    WebP, BMP, TIFF, GIF, PNM — whatever the `image` crate's default codecs
//!    support).
//! 2. Convert to 8-bit grayscale (`image::GrayImage`) — `rqrr` works on
//!    luminance.
//! 3. Run `rqrr::PreparedImage::prepare`, iterate the detected grids, decode
//!    each one and collect the payload + pixel-space bounding box.
//!
//! Failures are tolerant: malformed image bytes, undetected grids, and
//! per-grid decode failures all yield an empty result rather than surfacing as
//! errors. The post-processor is responsible for the success/failure
//! distinction at the result level (a `Some(vec![])` means "ran but found
//! nothing").

use crate::types::qr::{QrBoundingBox, QrCode};

/// Detect QR codes in the bytes of an [`crate::types::ExtractedImage`].
///
/// `format_hint` is currently unused — the `image` crate auto-detects the
/// container format from magic bytes — but the parameter is retained so future
/// backends (e.g. a WebP-via-`webp-decoder` variant) can use it without an API
/// break.
///
/// Returns an empty vector on any of:
///
/// - Empty input.
/// - Image-decode failure.
/// - No QR grids detected.
/// - All detected grids fail to decode.
///
/// Successfully decoded QR codes carry their payload, a confidence of `1.0`
/// (rqrr does not expose per-grid confidence; a successful decode is treated
/// as high-confidence by convention), and the pixel-space bounding box derived
/// from the four corner points of the grid.
pub fn detect_qr_codes(image_bytes: &[u8], _format_hint: Option<&str>) -> Vec<QrCode> {
    if image_bytes.is_empty() {
        return Vec::new();
    }

    let dynamic = match image::load_from_memory(image_bytes) {
        Ok(img) => img,
        Err(error) => {
            tracing::warn!(error = %error, "qr: image decode failed; skipping");
            return Vec::new();
        }
    };

    let luma = dynamic.to_luma8();
    let (width, height) = luma.dimensions();
    let raw = luma.into_raw();
    let mut prepared = rqrr::PreparedImage::prepare_from_greyscale(width as usize, height as usize, |x, y| {
        raw[y * width as usize + x]
    });
    let grids = prepared.detect_grids();
    if grids.is_empty() {
        return Vec::new();
    }

    let mut results = Vec::with_capacity(grids.len());
    for grid in grids {
        let mut payload_bytes: Vec<u8> = Vec::new();
        match grid.decode_to(&mut payload_bytes) {
            Ok(_meta) => {
                let payload = match String::from_utf8(payload_bytes) {
                    Ok(s) => s,
                    Err(err) => {
                        tracing::debug!(
                            error = %err,
                            "qr: non-UTF-8 payload, falling back to lossy decode"
                        );
                        String::from_utf8_lossy(err.as_bytes()).into_owned()
                    }
                };
                let bbox = bounding_box_from_corners(&grid.bounds);
                results.push(QrCode {
                    payload,
                    confidence: Some(1.0),
                    bbox,
                });
            }
            Err(error) => {
                tracing::warn!(error = %error, "qr: grid decode failed; skipping grid");
            }
        }
    }
    results
}

/// Compute an axis-aligned [`QrBoundingBox`] from rqrr's four corner points.
///
/// `corners` is the array of four `Point` values that mark the corners of the
/// QR finder pattern. We pick the axis-aligned envelope so callers get a
/// rectangle in pixel coordinates regardless of rotation. Negative coordinates
/// are clamped to zero.
fn bounding_box_from_corners(corners: &[rqrr::Point; 4]) -> Option<QrBoundingBox> {
    let xs: [i32; 4] = [corners[0].x, corners[1].x, corners[2].x, corners[3].x];
    let ys: [i32; 4] = [corners[0].y, corners[1].y, corners[2].y, corners[3].y];

    let min_x = *xs.iter().min()?;
    let max_x = *xs.iter().max()?;
    let min_y = *ys.iter().min()?;
    let max_y = *ys.iter().max()?;

    let x = min_x.max(0) as u32;
    let y = min_y.max(0) as u32;
    let width = (max_x - min_x).max(0) as u32;
    let height = (max_y - min_y).max(0) as u32;

    Some(QrBoundingBox { x, y, width, height })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_bytes_returns_empty() {
        assert!(detect_qr_codes(&[], None).is_empty());
    }

    #[test]
    fn invalid_bytes_returns_empty() {
        assert!(detect_qr_codes(&[0, 1, 2, 3, 4], Some("image/png")).is_empty());
    }
}
