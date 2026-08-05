fn main() {
    println!("cargo::rustc-check-cfg=cfg(coverage)");

    println!("cargo::rustc-check-cfg=cfg(inference_ort)");
    if std::env::var_os("CARGO_FEATURE_ORT_BUNDLED").is_some()
        || std::env::var_os("CARGO_FEATURE_ORT_DYNAMIC").is_some()
    {
        println!("cargo::rustc-cfg=inference_ort");
    }

    println!("cargo::rustc-check-cfg=cfg(auto_rotate)");
    if std::env::var_os("CARGO_FEATURE_AUTO_ROTATE").is_some()
        || std::env::var_os("CARGO_FEATURE_AUTO_ROTATE_TRACT").is_some()
    {
        println!("cargo::rustc-cfg=auto_rotate");
    }

    println!("cargo::rustc-check-cfg=cfg(sceptre_ocr)");
    if std::env::var_os("CARGO_FEATURE_SCEPTRE_OCR_ORT").is_some()
        || std::env::var_os("CARGO_FEATURE_SCEPTRE_OCR_TRACT").is_some()
    {
        println!("cargo::rustc-cfg=sceptre_ocr");
    }

    if std::env::var_os("CARGO_FEATURE_ORT_BUNDLED").is_some()
        && std::env::var_os("CARGO_FEATURE_ORT_DYNAMIC").is_some()
    {
        println!(
            "cargo::warning=features 'ort-bundled' and 'ort-dynamic' are both enabled; bundled ORT remains the default unless dynamic ORT is explicitly selected at runtime"
        );
    }

    println!("cargo::rustc-check-cfg=cfg(layout_detection)");
    if std::env::var_os("CARGO_FEATURE_LAYOUT_DETECTION").is_some()
        || std::env::var_os("CARGO_FEATURE_LAYOUT_TRACT").is_some()
    {
        println!("cargo::rustc-cfg=layout_detection");
    }
}
