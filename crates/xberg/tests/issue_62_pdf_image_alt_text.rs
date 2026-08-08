#![cfg(feature = "pdf")]

mod helpers;
use helpers::extract_bytes_document_blocking;

use xberg::core::config::{ExtractionConfig, ImageExtractionConfig};

/// Issue #62: `test_documents/pdf/nougat_049.pdf` is a tagged PDF whose
/// page-1 `Figure` structure element carries `/Alt (officeArt object)`
/// (ISO 32000-1:2008 §14.9.3) for the image XObject painted there. Before
/// the fix, `ExtractedImage::description` was hardcoded to `None` for every
/// image, so this alt text never reached the public API (and markdown
/// output emitted an empty `![]()`).
#[test]
fn test_tagged_pdf_alt_text_reaches_extracted_image_description() {
    if !helpers::test_documents_available() {
        return;
    }
    let path = helpers::get_test_file_path("pdf/nougat_049.pdf");
    let bytes = std::fs::read(&path).expect("fixture must be readable");

    let config = ExtractionConfig {
        images: Some(ImageExtractionConfig {
            extract_images: true,
            ..Default::default()
        }),
        ..Default::default()
    };
    let result = extract_bytes_document_blocking(&bytes, "application/pdf", &config)
        .expect("fixture must extract without error");

    let images = result.images.expect("extract_images=true must populate result.images");
    let page_one_images: Vec<_> = images.iter().filter(|img| img.page_number == Some(1)).collect();
    assert!(
        !page_one_images.is_empty(),
        "fixture page 1 must contain at least one extracted image"
    );
    assert_eq!(
        page_one_images[0].description,
        Some("officeArt object".to_string()),
        "the first image on page 1 must carry its Figure element's /Alt text; got {:?}",
        page_one_images[0].description
    );
}
