//! Public LLM-driven image captioning API.
//!
//! This module exposes [`caption_image`], [`caption_image_file`], and [`caption_images`]
//! for generating captions on images using a configured VLM backend.
//!
//! # Example
//!
//! ```ignore
//! use xberg::captioning::caption_image;
//! use xberg::LlmConfig;
//!
//! # async fn example() -> xberg::Result<()> {
//! let image_bytes = std::fs::read("photo.jpg")?;
//! let config = LlmConfig {
//!     model: "openai/gpt-4o-mini".to_string(),
//!     ..Default::default()
//! };
//! let caption = caption_image(&image_bytes, &config, None).await?;
//! println!("Caption: {}", caption);
//! # Ok(())
//! # }
//! ```

use std::path::Path;

use crate::core::config::LlmConfig;

/// Caption a single image from bytes.
///
/// # Arguments
///
/// * `image_bytes` - The image data.
/// * `llm_config` - LLM configuration for the VLM call.
/// * `custom_prompt` - Optional custom caption prompt. Uses the default
///   `RegionKind::Caption` prompt when `None`.
///
/// # Returns
///
/// The generated caption text.
///
/// # Errors
///
/// Returns an error if the VLM call fails or if image format detection fails.
///
/// # Example
///
/// ```ignore
/// use xberg::captioning::caption_image;
/// use xberg::LlmConfig;
///
/// # async fn example() -> xberg::Result<()> {
/// let image_bytes = vec![0xFF, 0xD8]; // JPEG header
/// let config = LlmConfig {
///     model: "anthropic/claude-3-5-sonnet".to_string(),
///     ..Default::default()
/// };
/// let caption = caption_image(&image_bytes, &config, None).await?;
/// # Ok(())
/// # }
/// ```
#[cfg_attr(alef, alef(skip))]
pub async fn caption_image(
    image_bytes: &[u8],
    llm_config: &LlmConfig,
    custom_prompt: Option<&str>,
) -> crate::Result<String> {
    let mime = infer_mime_type(image_bytes);
    crate::llm::region_extractor::extract_region_with_vlm(
        image_bytes,
        mime,
        crate::llm::region_extractor::RegionKind::Caption,
        llm_config,
        custom_prompt,
    )
    .await
}

/// Same as [`caption_image`], but also returns the [`LlmUsage`](crate::types::LlmUsage)
/// captured from the VLM call.
///
/// [`caption_image`] returns a bare `String`, so every caller of it silently discards the
/// token / cost record for the call it just paid for. Callers that own an
/// [`ExtractedDocument`](crate::types::ExtractedDocument) must use this variant and append
/// the usage to
/// [`ExtractedDocument::llm_usage`](crate::types::ExtractedDocument::llm_usage), the way
/// the built-in captioning post-processor does (#263).
///
/// # Errors
///
/// Same as [`caption_image`].
pub(crate) async fn caption_image_with_usage(
    image_bytes: &[u8],
    llm_config: &LlmConfig,
    custom_prompt: Option<&str>,
) -> crate::Result<(String, Option<crate::types::LlmUsage>)> {
    let mime = infer_mime_type(image_bytes);
    crate::llm::region_extractor::extract_region_with_vlm_usage(
        image_bytes,
        mime,
        crate::llm::region_extractor::RegionKind::Caption,
        llm_config,
        custom_prompt,
    )
    .await
}

/// Caption a single image from a file path.
///
/// # Arguments
///
/// * `path` - Path to the image file.
/// * `llm_config` - LLM configuration for the VLM call.
/// * `custom_prompt` - Optional custom caption prompt. Uses the default
///   `RegionKind::Caption` prompt when `None`.
///
/// # Returns
///
/// The generated caption text.
///
/// # Errors
///
/// Returns an error if the file cannot be read, if image format detection fails,
/// or if the VLM call fails.
///
/// # Example
///
/// ```ignore
/// use xberg::captioning::caption_image_file;
/// use xberg::LlmConfig;
///
/// # async fn example() -> xberg::Result<()> {
/// let config = LlmConfig {
///     model: "openai/gpt-4o-mini".to_string(),
///     ..Default::default()
/// };
/// let caption = caption_image_file("document_page_001.png", &config, None).await?;
/// # Ok(())
/// # }
/// ```
#[cfg_attr(alef, alef(skip))]
pub async fn caption_image_file(
    path: impl AsRef<Path>,
    llm_config: &LlmConfig,
    custom_prompt: Option<&str>,
) -> crate::Result<String> {
    let image_bytes = std::fs::read(path)?;
    caption_image(&image_bytes, llm_config, custom_prompt).await
}

/// Caption multiple images in a single batch.
///
/// Processes images sequentially (not in parallel). Returns one caption per input image
/// in the same order. If a caption fails, the error is returned immediately without
/// processing remaining images.
///
/// # Arguments
///
/// * `images` - Slice of image byte references to caption.
/// * `llm_config` - LLM configuration for the VLM calls.
/// * `custom_prompt` - Optional custom caption prompt. Uses the default
///   `RegionKind::Caption` prompt when `None`.
///
/// # Returns
///
/// A vector of captions, one per input image, in the same order.
///
/// # Errors
///
/// Returns an error if any VLM call fails.
///
/// # Example
///
/// ```ignore
/// use xberg::captioning::caption_images;
/// use xberg::LlmConfig;
///
/// # async fn example() -> xberg::Result<()> {
/// let image1 = std::fs::read("photo1.jpg")?;
/// let image2 = std::fs::read("photo2.jpg")?;
/// let images = vec![image1.as_ref(), image2.as_ref()];
/// let config = LlmConfig {
///     model: "openai/gpt-4o-mini".to_string(),
///     ..Default::default()
/// };
/// let captions = caption_images(&images, &config, None).await?;
/// assert_eq!(captions.len(), 2);
/// # Ok(())
/// # }
/// ```
#[cfg_attr(alef, alef(skip))]
pub async fn caption_images(
    images: &[&[u8]],
    llm_config: &LlmConfig,
    custom_prompt: Option<&str>,
) -> crate::Result<Vec<String>> {
    let mut captions = Vec::with_capacity(images.len());
    for image_bytes in images {
        let caption = caption_image(image_bytes, llm_config, custom_prompt).await?;
        captions.push(caption);
    }
    Ok(captions)
}

/// Infer MIME type from image byte signature.
///
/// Attempts to detect the image format using the `infer` crate. Falls back to
/// "image/png" if detection fails.
fn infer_mime_type(bytes: &[u8]) -> &'static str {
    match infer::get(bytes) {
        Some(ftype) => match ftype.mime_type() {
            "image/jpeg" => "image/jpeg",
            "image/png" => "image/png",
            "image/webp" => "image/webp",
            "image/gif" => "image/gif",
            "image/bmp" => "image/bmp",
            "image/tiff" => "image/tiff",
            _ => "image/png",
        },
        None => "image/png",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn infer_jpeg_mime() {
        let jpeg_header = vec![0xFF, 0xD8, 0xFF, 0xE0];
        assert_eq!(infer_mime_type(&jpeg_header), "image/jpeg");
    }

    #[test]
    fn infer_png_mime() {
        let png_header = vec![0x89, 0x50, 0x4E, 0x47];
        assert_eq!(infer_mime_type(&png_header), "image/png");
    }

    #[test]
    fn fallback_on_unknown() {
        let unknown = vec![0x00, 0x00, 0x00];
        assert_eq!(infer_mime_type(&unknown), "image/png");
    }

    #[test]
    fn fallback_on_empty() {
        let empty: Vec<u8> = vec![];
        assert_eq!(infer_mime_type(&empty), "image/png");
    }
}
