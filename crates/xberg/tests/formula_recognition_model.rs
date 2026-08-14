//! Real-model integration test for formula recognition.
//!
//! Downloads the RapidLaTeXOCR model set (~180 MB) on first run, so it is
//! ignored by default. Run explicitly:
//! `cargo test -p xberg --features formula-recognition --test formula_recognition_model -- --ignored`

#![cfg(feature = "formula-recognition")]

use image::{Rgb, RgbImage};

/// Render a crude "E=mc2" into a white canvas without any font machinery:
/// the model only needs plausible dark-on-light glyph shapes to exercise the
/// full pipeline; exact recognition quality is asserted loosely.
fn render_test_formula() -> RgbImage {
    let mut img = RgbImage::from_pixel(220, 64, Rgb([255, 255, 255]));
    let black = Rgb([0, 0, 0]);
    // E
    for y in 16..48 {
        img.put_pixel(20, y, black);
        img.put_pixel(21, y, black);
    }
    for x in 20..40 {
        for y in [16u32, 31, 47] {
            img.put_pixel(x, y, black);
        }
    }
    // =
    for x in 52..76 {
        for y in [26u32, 27, 36, 37] {
            img.put_pixel(x, y, black);
        }
    }
    // m (two arches, simplified as three verticals + top bar)
    for y in 24..48 {
        for x in [88u32, 89, 100, 101, 112, 113] {
            img.put_pixel(x, y, black);
        }
    }
    for x in 88..114 {
        img.put_pixel(x, 24, black);
        img.put_pixel(x, 25, black);
    }
    // c
    for y in 26..46 {
        img.put_pixel(126, y, black);
        img.put_pixel(127, y, black);
    }
    for x in 126..144 {
        for y in [26u32, 27, 44, 45] {
            img.put_pixel(x, y, black);
        }
    }
    // 2 (superscript block)
    for x in 150..166 {
        for y in [12u32, 13, 22, 23, 32, 33] {
            img.put_pixel(x, y, black);
        }
    }
    for y in 13..23 {
        img.put_pixel(164, y, black);
    }
    for y in 22..33 {
        img.put_pixel(150, y, black);
    }
    img
}

/// Full pipeline: download, load, recognize. Asserts the machinery produces
/// SOME LaTeX for a formula-shaped crop, and that a blank crop produces none.
#[test]
#[ignore = "downloads ~180 MB of model weights"]
fn recognizes_a_formula_shaped_crop_end_to_end() {
    let img = render_test_formula();
    let result = xberg::formula_recognition::recognize_for_test(&img).expect("recognition pipeline must not fail");
    let latex = result.unwrap_or_default();
    assert!(
        !latex.is_empty(),
        "the model must produce LaTeX for a formula-shaped crop"
    );
    // The decoder caps at 512 TOKENS; one token can decode to several
    // characters, so the character bound is generous and only catches
    // runaway repetition.
    assert!(latex.len() < 4096, "runaway decode: {latex}");

    // The tokenizer file ships without a decoder; the loader must attach the
    // ByteLevel decoder so byte-level markers never reach the output.
    assert!(!latex.contains('Ġ'), "undecoded byte-level markers in output: {latex}");
    assert!(!latex.contains("[EOS]"), "special tokens in output: {latex}");

    let blank = RgbImage::from_pixel(96, 48, Rgb([255, 255, 255]));
    let blank_result = xberg::formula_recognition::recognize_for_test(&blank).expect("blank crop must not fail");
    assert!(
        blank_result.is_none(),
        "a blank crop must yield no formula: {blank_result:?}"
    );
}
