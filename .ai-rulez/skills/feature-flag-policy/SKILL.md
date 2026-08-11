---
name: feature-flag-policy
description: Cargo feature flags for crates/xberg — ORT-incompatible targets (WASM, Android x86_64 emulator), type-only and tract inference companion features, WASM/Android-safe variants, PDF backend, mutually-exclusive ORT variants, platform-conditional deps, aggregate feature sets, and build profiles. Load when adding, wiring, or debugging a Cargo feature, or when reasoning about what compiles on WASM/Android/Windows/macOS-intel targets.
---

# Feature Flag Policy

All features in `crates/xberg/Cargo.toml`.

## ORT-Incompatible Targets (WASM, Android x86_64 emulator)

Only ORT-dependent paths are incompatible. The same paths block both WASM (no native ORT linkage at all) and the `x86_64-linux-android` emulator triple (no pyke prebuilt; `aarch64-linux-android` does ship a prebuilt and gets full ORT):

- `paddle-ocr` — ONNX Runtime + native C++ deps: not WASM-safe; no Android x86_64 prebuilt
- `layout-detection` — depends on ONNX Runtime layout models: not WASM-safe; no Android x86_64 prebuilt (RT-DETR + table classifier are available off-ORT via the `layout-tract` sibling, below)
- `embeddings` — depends on ONNX Runtime sentence-transformer models: not WASM-safe; no Android x86_64 prebuilt
- `auto-rotate` — depends on ONNX Runtime orientation classifier: not WASM-safe; no Android x86_64 prebuilt (available off-ORT via the `auto-rotate-tract` sibling, below)
- `formula-recognition` — LaTeX recognition for layout formula regions (RapidLaTeXOCR ONNX): ORT-dependent, opt-in, excluded from `full`/`formats`

Pure-Rust **type-only** companion features expose the public config/result types for the above without pulling in ORT:

- `layout-types` — `LayoutDetectionConfig`, `TableModel`, `BBox`, `DetectionResult`, `LayoutClass`, `LayoutDetection`, `RecognizedTable`. `layout-detection` implies `layout-types`.
- `auto-rotate-types` — `OrientationResult`. `auto-rotate` implies `auto-rotate-types`.
- `embedding-presets` — `EmbeddingPreset` (already existed; pure-Rust preset metadata).

Pure-Rust **tract inference** variants run select ONNX models where native ORT cannot link, loading the
same `.onnx` artifacts through the `tract` engine (CPU-only, no native library):

- `layout-detection` (ORT) → `layout-tract` (tract): RT-DETR layout detection + the PP-LCNet
  wired/wireless table classifier run on WASM and the Android x86_64 emulator. TATR, SLANeXT,
  PP-DocLayout-V3, and YOLO stay ONNX Runtime-only (tract 0.23.4 op-coverage gaps).
- `auto-rotate` (ORT) → `auto-rotate-tract` (tract): PP-LCNet document-orientation on WASM and the
  Android x86_64 emulator.

WASM/Android-safe variants:

- `ocr` (native) → `ocr-wasm` (uses `tesseract-wasm` + safe image deps) — Android keeps native `ocr`
- `excel` (native) → `excel-wasm` (drops `tokio-runtime`) — Android keeps native `excel`
- `tree-sitter` (native dlopen) → `tree-sitter-wasm` (statically-linked grammar pack). NOT in `wasm-target`: the 371-language grammar pack pushes the browser `.wasm` past jsDelivr's 50 MB per-file cap, breaking the CDN-hosted demo. WASM has no code intelligence; Android keeps native `tree-sitter`.
- `liter-llm` — works on WASM via the upstream `wasm-http` feature; included in `no-ort-target`
- `stopwords` — pure-Rust, included in `no-ort-target`
- `keywords` — pure-Rust YAKE/RAKE, included in `no-ort-target`

The `no-ort-target` aggregate is the shared no-ORT base used by both `wasm-target` and `android-target`. `wasm-target = no-ort-target + excel-wasm + ocr-wasm + layout-tract + auto-rotate-tract` (NO tree-sitter — see above). `android-target = no-ort-target + excel + tree-sitter + ocr + api + mcp + layout-tract + auto-rotate-tract`.

## PDF Backend

- `pdf` — the canonical PDF feature, backed by the pure-Rust `pdf_oxide` crate. This is the primary/default PDF extraction pipeline; there is no separate `pdf-oxide` feature. Included in `formats`, `full`, `no-ort-target`, and `wasm-target`.

## ORT Variants (Mutually Exclusive)

- `ort-bundled` — downloads official Microsoft ORT binaries; default when OCR/ML features active
- `ort-dynamic` — load ORT from system; only when system ORT is guaranteed present

## Platform-Conditional

- `xberg-paddle-ocr`, `hf-hub`, `pprof` — excluded on `wasm32`
- `ureq`: `rustls` on non-Windows; `native-tls` on Windows
- `xberg-ffi` and `xberg-dart` cargo dependencies are target-conditional: `cfg(target_os = "android")` (**both** ABIs — `aarch64` and `x86_64`) and `cfg(target_os = "ios")` select `android-target`, the pure-Rust tract ML surface with no ONNX Runtime; `cfg(target_os = "windows")` selects `windows-target` (full ORT ML via `ort-bundled`); `cfg(all(target_os = "macos", target_arch = "x86_64"))` selects `macos-intel-target`. All other targets (Linux, macOS arm64) get the full ORT-enabled feature set. Android and iOS run inference through `tract`, not ORT, on every ABI — there is no arch split that gives arm64 Android phones a native ORT build.

## Aggregate Sets

| Feature          | Description                                                                                        |
| ---------------- | -------------------------------------------------------------------------------------------------- |
| `formats`        | All document formats + api/mcp/otel/chunking; no OCR, no ML                                        |
| `full`           | `formats` + ocr + paddle-ocr + layout + embeddings + tree-sitter + liter-llm                        |
| `no-ort-target`  | Pure-Rust base: every capability that does not depend on ONNX Runtime                              |
| `wasm-target`    | `no-ort-target` + excel-wasm + ocr-wasm + layout-tract + auto-rotate-tract (no tree-sitter — grammar pack exceeds CDN 50 MB cap) |
| `android-target` | `no-ort-target` + excel + tree-sitter + ocr + api + mcp + layout-tract + auto-rotate-tract (for x86_64-linux-android emulator) |

## Build Profiles

- `release` — LTO thin, codegen-units=1, strip
- `profiling` — inherits release, retains debug info
- `xberg-wasm` override: `opt-level="z"` (size-optimized)
- `sevenz-rust2`, `zip` override: `opt-level=2` (prevents SIGBUS on macOS ARM64)
