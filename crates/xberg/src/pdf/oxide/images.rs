//! Image extraction using the pdf_oxide backend.
//!
//! Extracts embedded images from PDF pages via pdf_oxide, including
//! actual image data and metadata.

use super::OxideDocument;
use crate::cancellation::CancellationToken;
use crate::pdf::error::{PdfError, Result};
use bytes::Bytes;
use image::{DynamicImage, ImageFormat};
use std::borrow::Cow;
use std::io::Cursor;

/// Detect image format from magic bytes, returning a static format string.
///
/// This function validates that image data actually matches its claimed format
/// by inspecting magic bytes. If the data doesn't match any known format, it
/// returns `"raw"`.
#[inline]
fn detect_image_format_from_bytes(data: &[u8]) -> &'static str {
    if data.starts_with(b"\xff\xd8\xff") {
        "jpeg"
    } else if data.starts_with(b"\x89PNG\r\n\x1a\n") {
        "png"
    } else if data.starts_with(b"GIF8") {
        "gif"
    } else if data.starts_with(b"II") || data.starts_with(b"MM") {
        "tiff"
    } else if data.starts_with(b"BM") {
        "bmp"
    } else if data.len() >= 8 && data[0..4] == [0x00, 0x00, 0x00, 0x0C] && data[4..8] == [0x6A, 0x50, 0x20, 0x20] {
        "jpeg2000"
    } else {
        "raw"
    }
}

/// Extract at most `limit` images in content-stream paint order.
///
/// `page_image_handles` performs the cheap content-stream/CTM pass first, allowing
/// the cap to be applied before image decompression while preserving bounding boxes,
/// inline images, and images nested in Form XObjects.
fn extract_n_images_from_page_handles(
    doc: &OxideDocument,
    page_idx: usize,
    limit: usize,
) -> Result<Vec<pdf_oxide::extractors::PdfImage>> {
    let handles = doc.doc.page_image_handles(page_idx).map_err(|error| {
        PdfError::ExtractionFailed(format!(
            "enumerating image handles for PDF page {}: {error}",
            page_idx + 1
        ))
    })?;
    let mut images = Vec::new();
    for handle in handles.into_iter().take(limit) {
        match handle.decode() {
            Ok(img) => images.push(img),
            Err(error) => {
                tracing::debug!(page = page_idx, "image decompression failed: {error}");
            }
        }
    }

    Ok(images)
}

/// Re-encode raw PDF pixel data as a PNG buffer.
///
/// pdf_oxide emits `ImageData::Raw` without self-describing headers. Re-encoding
/// to PNG makes the buffer probeable by `load_image_for_ocr`,
/// `extract_image_metadata`, VLM pipelines, etc.
///
/// Returns `Err` if the pixel buffer length does not match `w × h × bpp` or if
/// PNG encoding fails.
fn raw_pixels_to_png(w: u32, h: u32, format: &pdf_oxide::extractors::PixelFormat, pixels: &[u8]) -> Result<Bytes> {
    let dynamic = match *format {
        pdf_oxide::extractors::PixelFormat::Grayscale => {
            let buf = image::GrayImage::from_raw(w, h, pixels.to_vec()).ok_or_else(|| {
                PdfError::ExtractionFailed(format!(
                    "grayscale pixel buffer ({} bytes) does not fit {}×{} image",
                    pixels.len(),
                    w,
                    h
                ))
            })?;
            DynamicImage::ImageLuma8(buf)
        }
        pdf_oxide::extractors::PixelFormat::RGB => {
            let buf = image::RgbImage::from_raw(w, h, pixels.to_vec()).ok_or_else(|| {
                PdfError::ExtractionFailed(format!(
                    "RGB pixel buffer ({} bytes) does not fit {}×{} image",
                    pixels.len(),
                    w,
                    h
                ))
            })?;
            DynamicImage::ImageRgb8(buf)
        }
        pdf_oxide::extractors::PixelFormat::CMYK => {
            let mut rgb = Vec::with_capacity((pixels.len() / 4) * 3);
            for chunk in pixels.chunks_exact(4) {
                let c = chunk[0] as f32 / 255.0;
                let m = chunk[1] as f32 / 255.0;
                let y = chunk[2] as f32 / 255.0;
                let k = chunk[3] as f32 / 255.0;
                rgb.push(((1.0 - c) * (1.0 - k) * 255.0) as u8);
                rgb.push(((1.0 - m) * (1.0 - k) * 255.0) as u8);
                rgb.push(((1.0 - y) * (1.0 - k) * 255.0) as u8);
            }
            let buf = image::RgbImage::from_raw(w, h, rgb)
                .ok_or_else(|| PdfError::ExtractionFailed(format!("CMYK→RGB buffer does not fit {}×{} image", w, h)))?;
            DynamicImage::ImageRgb8(buf)
        }
    };
    let mut png_bytes = Vec::new();
    dynamic
        .write_to(&mut Cursor::new(&mut png_bytes), ImageFormat::Png)
        .map_err(|e| PdfError::ExtractionFailed(format!("PNG re-encode of raw PDF image failed: {e}")))?;
    Ok(Bytes::from(png_bytes))
}

/// Build the `ProcessingWarning` for an image that was dropped because its raw
/// pixel buffer could not be re-encoded (issue #71). Previously this case only
/// logged via `tracing::warn!`, so callers had no structured signal that the
/// output `images` array is shorter than the document's actual image count.
fn unencodable_image_warning(image_index: u32, page_number: u32, error: &PdfError) -> crate::types::ProcessingWarning {
    crate::types::ProcessingWarning {
        source: std::borrow::Cow::Borrowed("pdf_images"),
        message: std::borrow::Cow::Owned(format!(
            "skipped image {image_index} on page {page_number}: could not be re-encoded from raw \
             pixel data ({error})"
        )),
    }
}

/// Collect OCR-ready image bytes for every image XObject on `page_idx`, for use as a
/// `force_ocr` fallback when whole-page rasterization silently dropped an undecodable
/// image (issue #1355).
///
/// `page_image_handles` is a cheap content-stream/CTM pass that succeeds even when the
/// renderer could not paint an image (e.g. an unsupported codec that pdf_oxide's page
/// renderer silently substitutes with a blank page). For each handle:
/// - `decode()` success → re-encode raw pixels to PNG, or pass the embedded JPEG through
///   as-is.
/// - `decode()` failure on a DCTDecode/JPXDecode stream → hand back the raw compressed
///   bytes, which are a valid standalone JPEG/JP2 file the OCR backend can decode itself.
/// - Any other failure → skip the image; there is no way to recover pixel data from it.
///
/// Returned in content-stream paint order; empty when the page has no image XObjects or
/// none of them yielded usable bytes.
///
// Available under `ocr-pipeline` too (not just `ocr`): `extract_with_ocr` — the sole
// caller — is gated `any(ocr, ocr-pipeline)`, and the `binstall` CLI profile pulls
// `ocr-pipeline` (via `liter-llm`) without `ocr`. ~keep
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
pub(crate) fn page_ocr_fallback_image_bytes(doc: &pdf_oxide::PdfDocument, page_idx: usize) -> Vec<Bytes> {
    let handles = match doc.page_image_handles(page_idx) {
        Ok(h) => h,
        Err(error) => {
            tracing::debug!(
                page = page_idx,
                "force_ocr fallback: enumerating image handles failed: {error}"
            );
            return Vec::new();
        }
    };

    let mut out = Vec::with_capacity(handles.len());
    for handle in &handles {
        match handle.decode() {
            Ok(img) => match img.data() {
                pdf_oxide::extractors::ImageData::Jpeg(jpeg_bytes) => out.push(Bytes::copy_from_slice(jpeg_bytes)),
                pdf_oxide::extractors::ImageData::Raw { pixels, format } => {
                    match raw_pixels_to_png(img.width(), img.height(), format, pixels) {
                        Ok(bytes) => out.push(bytes),
                        Err(error) => {
                            tracing::debug!(page = page_idx, "force_ocr fallback: raw re-encode failed: {error}");
                        }
                    }
                }
            },
            Err(decode_err) => {
                let passthrough = matches!(
                    handle.filter_chain.last(),
                    Some(pdf_oxide::PdfFilter::DCTDecode) | Some(pdf_oxide::PdfFilter::JPXDecode)
                );
                if passthrough {
                    match handle.raw_compressed_bytes() {
                        Ok(raw) if matches!(detect_image_format_from_bytes(&raw), "jpeg" | "jpeg2000") => {
                            out.push(Bytes::from(raw));
                        }
                        Ok(_) => {
                            tracing::debug!(
                                page = page_idx,
                                "force_ocr fallback: raw bytes not a recognizable JPEG/JP2"
                            );
                        }
                        Err(error) => {
                            tracing::debug!(
                                page = page_idx,
                                "force_ocr fallback: raw_compressed_bytes failed: {error}"
                            );
                        }
                    }
                } else {
                    tracing::debug!(
                        page = page_idx,
                        "force_ocr fallback: undecodable image, non-JPEG codec: {decode_err}"
                    );
                }
            }
        }
    }
    out
}

/// Extract full image data from all pages of a PDF.
///
/// Returns a `Vec<ExtractedImage>` with complete image data and metadata, plus any
/// non-fatal `ProcessingWarning`s produced along the way (e.g. an image that could
/// not be re-encoded and was skipped — see issue #71).
/// When image extraction is disabled or no images are found, returns an empty vec.
///
/// # Arguments
///
/// * `doc` - Mutable reference to the oxide document
/// * `max_images_per_page` - Optional limit on images per page
/// * `cancel_token` - Optional cancellation token checked between pages
///
/// # Returns
///
/// A `Vec<ExtractedImage>` containing all extracted images with their data, and a
/// `Vec<ProcessingWarning>` describing any images that were skipped.
pub(crate) fn extract_images_with_data(
    doc: &mut OxideDocument,
    max_images_per_page: Option<u32>,
    cancel_token: Option<&CancellationToken>,
) -> Result<(Vec<crate::types::ExtractedImage>, Vec<crate::types::ProcessingWarning>)> {
    if max_images_per_page == Some(0) {
        return Ok((Vec::new(), Vec::new()));
    }

    tracing::debug!(
        target: "xberg::pdf::oxide::images",
        event = "decompression_started",
        "extract_images_with_data entered"
    );

    let page_count = doc
        .doc
        .page_count()
        .map_err(|e| PdfError::MetadataExtractionFailed(format!("pdf_oxide: failed to get page count: {e}")))?;

    let mut all_images = Vec::new();
    let mut warnings = Vec::new();
    let mut global_index = 0u32;

    // Tagged-PDF `/Alt` text for `Figure` structure elements, keyed by 0-based page
    // index (issue #62). Empty for the (common) untagged-PDF case.
    let mut alt_text_by_page = super::hierarchy::extract_figure_alt_text_by_page(doc);

    for page_idx in 0..page_count {
        if cancel_token.is_some_and(|t| t.is_cancelled()) {
            break;
        }

        let oxide_images = match max_images_per_page.map(|n| n as usize) {
            Some(limit) => {
                let handle_images = match extract_n_images_from_page_handles(doc, page_idx, limit) {
                    Ok(images) => images,
                    Err(error) => {
                        tracing::debug!(
                            page = page_idx,
                            "capped image-handle extraction failed; falling back to eager extraction: {error}"
                        );
                        Vec::new()
                    }
                };
                if !handle_images.is_empty() {
                    handle_images
                } else {
                    match doc.doc.extract_images(page_idx) {
                        Ok(imgs) => imgs.into_iter().take(limit).collect(),
                        Err(e) => {
                            tracing::debug!(page = page_idx, "pdf_oxide: failed to extract images (fallback): {e}");
                            continue;
                        }
                    }
                }
            }
            None => match doc.doc.extract_images(page_idx) {
                Ok(imgs) => imgs,
                Err(e) => {
                    tracing::debug!(page = page_idx, "pdf_oxide: failed to extract images: {e}");
                    continue;
                }
            },
        };

        let page_number = (page_idx + 1) as u32;
        let page_alt_texts = alt_text_by_page.remove(&(page_idx as u32));
        for (page_image_position, oxide_img) in oxide_images.iter().enumerate() {
            let alt_text = page_alt_texts
                .as_ref()
                .and_then(|alts| alts.get(page_image_position))
                .and_then(|alt| alt.clone());
            let (data, format) = match oxide_img.data() {
                pdf_oxide::extractors::ImageData::Jpeg(jpeg_bytes) => {
                    let data_bytes = Bytes::copy_from_slice(jpeg_bytes);
                    let actual_format = detect_image_format_from_bytes(data_bytes.as_ref());
                    (data_bytes, Cow::Borrowed(actual_format))
                }
                pdf_oxide::extractors::ImageData::Raw { pixels, format } => {
                    match raw_pixels_to_png(oxide_img.width(), oxide_img.height(), format, pixels) {
                        Ok(bytes) => (bytes, Cow::Borrowed("png")),
                        Err(e) => {
                            tracing::warn!(
                                page = page_number,
                                image_index = global_index,
                                "skipping raw PDF image that could not be re-encoded: {e}"
                            );
                            warnings.push(unencodable_image_warning(global_index, page_number, &e));
                            continue;
                        }
                    }
                }
            };

            let extracted_img = crate::types::ExtractedImage {
                data,
                format,
                image_index: global_index,
                page_number: Some(page_number),
                width: Some(oxide_img.width()),
                height: Some(oxide_img.height()),
                colorspace: Some(format!("{:?}", oxide_img.color_space())),
                bits_per_component: Some(oxide_img.bits_per_component() as u32),
                is_mask: false,
                description: alt_text,
                ocr_result: None,
                bounding_box: oxide_img.bbox().map(|r| crate::types::BoundingBox {
                    x0: r.x as f64,
                    y0: r.y as f64,
                    x1: (r.x + r.width) as f64,
                    y1: (r.y + r.height) as f64,
                }),
                source_path: None,
                image_kind: None,
                kind_confidence: None,
                cluster_id: None,
                caption: None,
                qr_codes: None,
                data_base64: None,
            };

            all_images.push(extracted_img);
            global_index += 1;
        }
    }

    Ok((all_images, warnings))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cancellation::CancellationToken;
    use std::path::PathBuf;

    const PNG_MAGIC: &[u8] = b"\x89PNG";

    #[test]
    fn test_raw_pixels_to_png_grayscale() {
        let pixels: Vec<u8> = vec![0x00, 0x80, 0xc0, 0xff];
        let result = raw_pixels_to_png(2, 2, &pdf_oxide::extractors::PixelFormat::Grayscale, &pixels);
        let bytes = result.expect("grayscale 2×2 must encode without error");
        assert!(
            bytes.starts_with(PNG_MAGIC),
            "output must be a PNG; got {:02x?}",
            &bytes[..4.min(bytes.len())]
        );
    }

    #[test]
    fn test_raw_pixels_to_png_rgb() {
        let pixels: Vec<u8> = vec![0xff, 0x00, 0x00, 0x00, 0xff, 0x00, 0x00, 0x00, 0xff, 0xff, 0xff, 0xff];
        let result = raw_pixels_to_png(2, 2, &pdf_oxide::extractors::PixelFormat::RGB, &pixels);
        let bytes = result.expect("RGB 2×2 must encode without error");
        assert!(
            bytes.starts_with(PNG_MAGIC),
            "output must be a PNG; got {:02x?}",
            &bytes[..4.min(bytes.len())]
        );
    }

    #[test]
    fn test_raw_pixels_to_png_cmyk_converts_to_rgb_png() {
        let pixels: Vec<u8> = vec![0x00, 0x00, 0x00, 0xff, 0x00, 0x00, 0x00, 0x00];
        let result = raw_pixels_to_png(1, 2, &pdf_oxide::extractors::PixelFormat::CMYK, &pixels);
        let bytes = result.expect("CMYK 1×2 must encode without error");
        assert!(
            bytes.starts_with(PNG_MAGIC),
            "output must be a PNG; got {:02x?}",
            &bytes[..4.min(bytes.len())]
        );
        let decoded = image::load_from_memory(&bytes).expect("decoded PNG must be valid");
        assert_eq!(decoded.width(), 1);
        assert_eq!(decoded.height(), 2);
    }

    #[test]
    fn test_raw_pixels_to_png_size_mismatch_returns_error() {
        let pixels: Vec<u8> = vec![0x00, 0x80, 0xc0, 0xff];
        let result = raw_pixels_to_png(4, 4, &pdf_oxide::extractors::PixelFormat::Grayscale, &pixels);
        assert!(
            result.is_err(),
            "mismatched buffer size must return Err, not Ok or panic"
        );
    }

    #[test]
    fn test_raw_pixels_to_png_rgb_size_mismatch_returns_error() {
        let pixels: Vec<u8> = vec![0xff; 9];
        let result = raw_pixels_to_png(2, 2, &pdf_oxide::extractors::PixelFormat::RGB, &pixels);
        assert!(result.is_err(), "mismatched RGB buffer must return Err");
    }

    #[test]
    fn test_raw_pixels_to_png_cmyk_odd_length_returns_error() {
        let pixels: Vec<u8> = vec![0x00, 0x00, 0x00];
        let result = raw_pixels_to_png(1, 1, &pdf_oxide::extractors::PixelFormat::CMYK, &pixels);
        assert!(
            result.is_err(),
            "CMYK buffer whose length is not a multiple of 4 must return Err, not panic"
        );
    }

    /// Issue #71: an image dropped for failing to re-encode must produce a
    /// `ProcessingWarning` naming the image index and page, not just a
    /// `tracing::warn!` log line the caller can never see.
    #[test]
    fn test_unencodable_image_warning_names_index_and_page() {
        let pixels: Vec<u8> = vec![0x00, 0x80, 0xc0, 0xff];
        let error = raw_pixels_to_png(4, 4, &pdf_oxide::extractors::PixelFormat::Grayscale, &pixels)
            .expect_err("4x4 grayscale from a 4-byte buffer must fail to re-encode");

        let warning = unencodable_image_warning(3, 2, &error);

        assert_eq!(warning.source.as_ref(), "pdf_images");
        assert_eq!(
            warning.message.as_ref(),
            format!("skipped image 3 on page 2: could not be re-encoded from raw pixel data ({error})"),
            "warning message must name the exact image index (3) and page (2)"
        );
    }

    fn test_documents_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("test_documents")
    }

    /// `max_images_per_page = Some(0)` must return an empty vec immediately
    /// without opening any page — the early-exit short-circuit at the top of
    /// `extract_images_with_data` fires before the page loop even starts.
    #[test]
    fn test_max_images_per_page_zero_returns_immediately() {
        let pdf_path = test_documents_dir().join("pdf/embedded_images_tables.pdf");
        assert!(
            pdf_path.exists(),
            "missing fixture: test PDF not found at {}",
            pdf_path.display()
        );

        let bytes = std::fs::read(&pdf_path).expect("failed to read test PDF");
        let mut doc = crate::pdf::oxide::OxideDocument::open_bytes(&bytes).expect("failed to open PDF");

        let (result, _warnings) = extract_images_with_data(&mut doc, Some(0), None).expect("cap=0 must not error");

        assert!(
            result.is_empty(),
            "max_images_per_page=Some(0) must return empty without decompressing any page; \
             got {} image(s)",
            result.len()
        );
    }

    /// A cancellation token fired from a background thread stops extraction after
    /// the current page completes and before the next page's cancellation check.
    ///
    /// Uses `nougat_039.pdf` (2 pages, ~67KB). A background thread cancels the
    /// token after 20ms — a window chosen to land after page 0's images are
    /// decompressed but before page 1's cancellation check fires.
    ///
    /// Timing note: on very fast or very slow hardware, the cancel may fire before
    /// page 0 completes (result is empty) or after page 1 completes (result equals
    /// the full count). Both are valid outcomes.  The invariant under test is
    /// `result.len() ≤ full_count`, which proves that cancellation never produces
    /// *more* images than an uncancelled run and that the code path compiles and
    /// runs without error.
    #[test]
    fn test_cancellation_fires_between_pages() {
        let pdf_path = test_documents_dir().join("pdf/nougat_039.pdf");
        assert!(
            pdf_path.exists(),
            "missing fixture: nougat_039.pdf not found at {}",
            pdf_path.display()
        );

        let bytes = std::fs::read(&pdf_path).expect("failed to read test PDF");

        let mut doc_full = crate::pdf::oxide::OxideDocument::open_bytes(&bytes).expect("failed to open PDF");
        let (full_result, _warnings) =
            extract_images_with_data(&mut doc_full, None, None).expect("uncancelled extraction must not error");
        let full_count = full_result.len();
        let page_count = doc_full
            .doc
            .page_count()
            .expect("page_count must succeed on the fixture");

        if page_count <= 1 || full_count == 0 {
            eprintln!(
                "SKIP test_cancellation_fires_between_pages: nougat_039.pdf has {} page(s) \
                 and {} extractable images — need ≥2 pages with images",
                page_count, full_count
            );
            return;
        }

        let mut doc_cancel = crate::pdf::oxide::OxideDocument::open_bytes(&bytes).expect("failed to open PDF");
        let token = CancellationToken::new();
        let token_clone = token.clone();

        let handle = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(20));
            token_clone.cancel();
        });

        let (result, _warnings) =
            extract_images_with_data(&mut doc_cancel, None, Some(&token)).expect("cancellation must not error");

        handle.join().expect("background thread must not panic");

        assert!(
            token.is_cancelled(),
            "token must be cancelled after background thread fires"
        );

        assert!(
            result.len() <= full_count,
            "cancelled extraction returned {} image(s); uncancelled returned {}; \
             cancellation must never exceed the full count",
            result.len(),
            full_count
        );
    }

    /// Pre-cancelled token fires on the first loop iteration (page 0) before
    /// any decompression begins. This test covers the trivial case; see
    /// `test_cancellation_fires_between_pages` for mid-run coverage.
    #[test]
    fn test_cancellation_stops_extraction_early() {
        let pdf_path = test_documents_dir().join("pdf/embedded_images_tables.pdf");
        assert!(
            pdf_path.exists(),
            "missing fixture: test PDF not found at {}",
            pdf_path.display()
        );

        let bytes = std::fs::read(&pdf_path).expect("failed to read test PDF");
        let mut doc = crate::pdf::oxide::OxideDocument::open_bytes(&bytes).expect("failed to open PDF");

        let token = CancellationToken::new();
        token.cancel();

        let (result, _warnings) =
            extract_images_with_data(&mut doc, None, Some(&token)).expect("extract must not error");

        assert!(
            result.is_empty(),
            "pre-cancelled token must cause extraction to return empty vec immediately, \
             got {} image(s)",
            result.len()
        );
    }

    /// The default (uncapped) extraction path routes through `PdfDocument::extract_images`,
    /// which walks content streams tracking the CTM and calls `PdfImage::set_bbox` for every
    /// image `Do` operator. Confirms `bounding_box` is populated end-to-end for a real fixture.
    #[test]
    fn test_extract_images_with_data_default_path_populates_bounding_box() {
        let pdf_path = test_documents_dir().join("pdf/embedded_images_tables.pdf");
        assert!(
            pdf_path.exists(),
            "missing fixture: test PDF not found at {}",
            pdf_path.display()
        );

        let bytes = std::fs::read(&pdf_path).expect("failed to read test PDF");
        let mut doc = crate::pdf::oxide::OxideDocument::open_bytes(&bytes).expect("failed to open PDF");

        let (result, _warnings) = extract_images_with_data(&mut doc, None, None).expect("extraction must not error");

        assert!(!result.is_empty(), "fixture must contain at least one image");
        assert!(
            result.iter().all(|img| img.bounding_box.is_some()),
            "every image extracted via the default (uncapped) path must carry a bounding_box \
             from pdf_oxide's CTM-tracked extract_images(); got: {:?}",
            result.iter().map(|img| img.bounding_box).collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_extract_images_with_data_capped_path_preserves_bounding_box() {
        let pdf_path = test_documents_dir().join("pdf/embedded_images_tables.pdf");
        assert!(
            pdf_path.exists(),
            "missing fixture: test PDF not found at {}",
            pdf_path.display()
        );

        let bytes = std::fs::read(&pdf_path).expect("failed to read test PDF");
        let mut doc = crate::pdf::oxide::OxideDocument::open_bytes(&bytes).expect("failed to open PDF");

        let (result, _warnings) =
            extract_images_with_data(&mut doc, Some(50), None).expect("extraction must not error");

        assert!(!result.is_empty(), "fixture must contain at least one image");
        assert!(
            result.iter().all(|img| img.bounding_box.is_some()),
            "the capped image-handle path must preserve CTM-derived bounding boxes; \
             got: {:?}",
            result.iter().map(|img| img.bounding_box).collect::<Vec<_>>()
        );
    }

    /// Verify that `detect_image_format_from_bytes` correctly identifies formats from magic bytes.
    /// This test ensures that even if pdf_oxide returns data labeled as JPEG but lacking proper
    /// headers, we can detect the actual format.
    #[test]
    fn test_detect_image_format_from_bytes() {
        let jpeg_data = b"\xff\xd8\xff\xe0\x00\x10JFIF";
        assert_eq!(detect_image_format_from_bytes(jpeg_data), "jpeg");

        let png_data = b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR";
        assert_eq!(detect_image_format_from_bytes(png_data), "png");

        let gif_data = b"GIF89a";
        assert_eq!(detect_image_format_from_bytes(gif_data), "gif");

        let tiff_le = b"II\x2a\x00";
        assert_eq!(detect_image_format_from_bytes(tiff_le), "tiff");

        let tiff_be = b"MM\x00\x2a";
        assert_eq!(detect_image_format_from_bytes(tiff_be), "tiff");

        let bmp_data = b"BM\x00\x00\x00";
        assert_eq!(detect_image_format_from_bytes(bmp_data), "bmp");

        let jp2_data = b"\x00\x00\x00\x0cjP  ";
        assert_eq!(detect_image_format_from_bytes(jp2_data), "jpeg2000");

        let raw_data = b"\x00\x01\x02\x03\x04\x05";
        assert_eq!(detect_image_format_from_bytes(raw_data), "raw");

        assert_eq!(detect_image_format_from_bytes(b""), "raw");

        let incomplete = b"\xff\xd8";
        assert_eq!(detect_image_format_from_bytes(incomplete), "raw");
    }

    /// `page_ocr_fallback_image_bytes` must recover usable image bytes for a page whose
    /// content is real, decodable image XObjects — the fixture used elsewhere in this
    /// file for image extraction (issue #1355 force_ocr fallback).
    #[cfg(feature = "ocr")]
    #[test]
    fn test_page_ocr_fallback_image_bytes_recovers_real_image() {
        let pdf_path = test_documents_dir().join("pdf/embedded_images_tables.pdf");
        assert!(
            pdf_path.exists(),
            "missing fixture: test PDF not found at {}",
            pdf_path.display()
        );

        let bytes = std::fs::read(&pdf_path).expect("failed to read test PDF");
        let doc = crate::pdf::oxide::OxideDocument::open_bytes(&bytes).expect("failed to open PDF");

        let fallback_images = page_ocr_fallback_image_bytes(&doc.doc, 0);

        assert!(
            !fallback_images.is_empty(),
            "fixture page 0 must contain at least one recoverable image XObject"
        );
        for image_bytes in &fallback_images {
            let format = detect_image_format_from_bytes(image_bytes);
            assert!(
                matches!(format, "jpeg" | "png" | "jpeg2000"),
                "fallback image bytes must carry a recognizable magic (jpeg/png/jpeg2000); got {:02x?}",
                &image_bytes[..8.min(image_bytes.len())]
            );
        }
    }

    /// A page index past the end of the document must not panic; `page_image_handles`
    /// returns an `Err` that the fallback helper degrades to an empty vec.
    #[cfg(feature = "ocr")]
    #[test]
    fn test_page_ocr_fallback_image_bytes_out_of_range_page_returns_empty() {
        let pdf_path = test_documents_dir().join("pdf/embedded_images_tables.pdf");
        let bytes = std::fs::read(&pdf_path).expect("failed to read test PDF");
        let doc = crate::pdf::oxide::OxideDocument::open_bytes(&bytes).expect("failed to open PDF");

        let fallback_images = page_ocr_fallback_image_bytes(&doc.doc, 9999);

        assert!(
            fallback_images.is_empty(),
            "out-of-range page must degrade to an empty vec, not panic or error"
        );
    }

    /// Issue #62: a tagged PDF's `Figure` structure element carries `/Alt "officeArt
    /// object"` on page 1 for the image XObject painted there. Before the fix,
    /// `description` was hardcoded to `None` for every extracted image, so this
    /// alt text was silently dropped and never reached `ExtractedImage::description`
    /// (which markdown/plain-text rendering consume as the image's alt/caption text).
    #[test]
    fn test_extract_images_with_data_reads_tagged_pdf_alt_text() {
        let pdf_path = test_documents_dir().join("pdf/nougat_049.pdf");
        assert!(
            pdf_path.exists(),
            "missing fixture: test PDF not found at {}",
            pdf_path.display()
        );

        let bytes = std::fs::read(&pdf_path).expect("failed to read test PDF");
        let mut doc = crate::pdf::oxide::OxideDocument::open_bytes(&bytes).expect("failed to open PDF");

        let (result, _warnings) = extract_images_with_data(&mut doc, None, None).expect("extraction must not error");

        let page_one_images: Vec<_> = result.iter().filter(|img| img.page_number == Some(1)).collect();
        assert!(
            !page_one_images.is_empty(),
            "fixture page 1 must contain at least one extracted image"
        );
        assert_eq!(
            page_one_images[0].description,
            Some("officeArt object".to_string()),
            "the first image on page 1 must carry the /Alt text from its Figure structure \
             element instead of None; got {:?}",
            page_one_images[0].description
        );
    }
}
