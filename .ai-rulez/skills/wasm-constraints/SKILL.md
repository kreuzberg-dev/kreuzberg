---
name: wasm-constraints
description: WASM build constraints for the crates/xberg-wasm crate — the wasm-target feature set, no-tokio sync-only internal APIs, the mandatory SyncExtractor trait for WASM-compatible extractors, the 2 MB HTML size limit, size-optimized build config (opt-level="z"), and the async-wrapper/sync-internal API pattern. Load when building for wasm32, adding or modifying a WASM-compatible extractor, or debugging WASM build/runtime failures.
---

# WASM Build Constraints

## Overview

WASM target in `crates/xberg-wasm/`. Uses wasm-bindgen with sync-only internal APIs.

## Feature Flags

```toml
[features]
# Aggregate: pure-Rust no-ORT base + excel + OCR + tract layout/orientation.
# RT-DETR layout detection and PP-LCNet document-orientation run in WASM through
# the pure-Rust `tract` engine (layout-tract + auto-rotate-tract); weights are
# streamed in via load_from_memory (detectLayout / detectOrientation). Deliberately
# NO tree-sitter — the 371-language grammar pack pushes the browser .wasm past
# jsDelivr's 50 MB per-file cap, so code intelligence is unavailable in WASM.
wasm-target = ["no-ort-target", "excel-wasm", "ocr-wasm", "layout-tract", "auto-rotate-tract"]
```

## Critical Constraints

### 1. No Tokio Runtime

All operations must be synchronous internally. Use `#[cfg(not(feature = "tokio-runtime"))]` paths.

### 2. Internal Sync Extractor Required

Every WASM-compatible built-in extractor MUST implement the internal `SyncExtractor` trait. This is not part of the public V1 extraction API; public callers still use unified `extract` / `extract_batch`.

```rust
impl SyncExtractor for MyExtractor {
    fn extract_sync(&self, content: &[u8], mime_type: &str, config: &ExtractionConfig)
        -> Result<InternalDocument> { /* sync implementation */ }
}

impl DocumentExtractor for MyExtractor {
    fn as_sync_extractor(&self) -> Option<&dyn SyncExtractor> {
        Some(self)  // MUST return Some for WASM
    }
}
```

### 3. HTML Size Limit

```rust
const MAX_HTML_SIZE: usize = 2 * 1024 * 1024;  // 2MB - stack constraint
```

## Build Config

```toml
[lib]
crate-type = ["cdylib", "rlib"]

[profile.release.package.xberg-wasm]
opt-level = "z"       # Size optimization
codegen-units = 1
```

## API Pattern

```rust
#[wasm_bindgen]
pub async fn extract_from_bytes(content: Vec<u8>, config: JsValue) -> Result<JsValue, JsValue> {
    let config: ExtractionConfig = serde_wasm_bindgen::from_value(config)?;
    let result = extract_bytes_sync(&content, mime_type, &config)?;
    Ok(serde_wasm_bindgen::to_value(&result)?)
}
```

Functions can be `async` for JS compatibility, but internal extraction is sync.

## Critical Rules

1. **No tokio** -- all operations synchronous
2. **Implement SyncExtractor** for all WASM-compatible extractors
3. **HTML limited to 2MB** due to stack constraints
4. **Size optimization** via `opt-level = "z"`
5. **Feature gate** with `#[cfg(target_arch = "wasm32")]`
