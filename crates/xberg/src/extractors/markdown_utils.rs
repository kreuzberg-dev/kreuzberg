//! Shared utilities for Markdown and MDX extractors.
//!
//! Provides the data-URI image decoding used by both the Markdown and MDX extractors while they
//! build their `InternalDocument`.

use crate::types::ExtractedImage;
use base64::Engine;
use bytes::Bytes;
use std::borrow::Cow;

/// Decode a data URI into an `ExtractedImage`.
///
/// Supports base64-encoded PNG, JPEG, GIF, WebP, and SVG data URIs.
/// Returns `None` for non-base64 encodings or unsupported formats.
///
/// # Arguments
///
/// * `uri` - The full `data:image/...;base64,...` URI string
/// * `index` - The zero-based image index within the document
pub(crate) fn decode_data_uri_image(uri: &str, index: u32) -> Option<ExtractedImage> {
    let after_data = uri.strip_prefix("data:")?;
    let (mime_and_encoding, data) = after_data.split_once(',')?;

    if !mime_and_encoding.contains("base64") {
        return None;
    }

    // `image/svg+xml` is recognized alongside the raster formats below (issue #145):
    // inline SVG data URIs used to be dropped from Markdown entirely, even though the HTML
    // extractor already handles inline SVG via `InlineImageFormat::Svg`.
    let format: &str = if mime_and_encoding.contains("image/png") {
        "png"
    } else if mime_and_encoding.contains("image/jpeg") {
        "jpeg"
    } else if mime_and_encoding.contains("image/gif") {
        "gif"
    } else if mime_and_encoding.contains("image/webp") {
        "webp"
    } else if mime_and_encoding.contains("image/svg+xml") {
        "svg"
    } else {
        return None;
    };

    let cleaned = data.replace(['\n', '\r'], "");
    let decoded = base64::engine::general_purpose::STANDARD.decode(&cleaned).ok()?;

    let (image_kind, kind_confidence) =
        crate::extraction::image_kind::classify(&decoded, format, None, None, None, None, false);

    Some(ExtractedImage {
        data: Bytes::from(decoded),
        format: Cow::Borrowed(format),
        image_index: index,
        page_number: None,
        width: None,
        height: None,
        colorspace: None,
        bits_per_component: None,
        is_mask: false,
        description: None,
        ocr_result: None,
        bounding_box: None,
        source_path: None,
        image_kind: Some(image_kind),
        kind_confidence: Some(kind_confidence),
        cluster_id: None,
        caption: None,
        qr_codes: None,
        data_base64: None,
    })
}
