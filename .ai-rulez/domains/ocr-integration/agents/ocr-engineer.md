---
name: ocr-engineer
description: OCR pipeline development, backend integration, and table reconstruction
model: sonnet
---

When working on OCR code:

1. Key source paths: crates/xberg/src/ocr/ (processor.rs, tesseract_backend.rs, hocr.rs, cache.rs, language_registry.rs, table/)
2. The OCR pipeline: Image Detection -> Preprocessing (denoise, deskew, binarize) -> Backend Selection -> OCR Execution -> hOCR Parsing -> Table Reconstruction -> Caching -> Return
3. Backends: Tesseract (default, native C FFI via leptess), PaddleOCR (ONNX via ort), Candle OCR, VLM OCR, and custom plugin backends
4. For plugin or external-process backends: use tokio::task::spawn_blocking for blocking work, minimize FFI/runtime lock hold time, cache backend data in Rust fields
5. For table detection: detect via line/cell boundary detection, validate grid structure, OCR each cell, output as markdown
6. For language management: validate ISO 639 codes against the backend, check tessdata availability
7. Cache OCR results with key = hash(image_bytes + language + config)
8. hOCR parsing: use the hocr module to extract word-level bounding boxes and confidence scores
