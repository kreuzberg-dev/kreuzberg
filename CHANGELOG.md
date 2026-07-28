# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

---

## [Unreleased]

### Fixed

- **#1323**: RTF hex byte escapes now honor `\ansicpgNNNN` via the shared Windows-codepage table, so
  CP1251 Cyrillic and other non-1252 ANSI byte runs decode as readable text instead of Windows-1252
  mojibake; adjacent escapes decode as one multi-byte run, surviving line wraps, and formatting spans
  stay aligned with the decoded text.
### Added

- `LayoutStrategy` enum on `LayoutDetectionConfig` (`strategy` field, default `always`). `auto` pre-screens each PDF page with cheap geometry signals and runs the layout model only on pages likely to benefit; existing configs keep the every-page behavior bit-for-bit. On the OCR path only inference is skipped, since OCR consumes the layout pass's rasters. Skipped pages are auditable via `metadata.format.layout_gated_pages` and `layout_gate_reasons`, and the CLI gains `--layout-strategy` ([#1322](https://github.com/xberg-io/xberg/issues/1322)).

## [1.0.0] - 2026-07-27

xberg 1.0.0 is the first stable release of the document-intelligence engine previously developed as
**Kreuzberg**. It is the direct successor to Kreuzberg v4.9 and carries the same Rust core and
extraction-API lineage forward under the xberg name. The Kreuzberg v4 line continues as LTS at
[kreuzberg-dev/kreuzberg-lts](https://github.com/kreuzberg-dev/kreuzberg-lts). This entry summarizes
everything that changed relative to Kreuzberg v4.9.

Beyond the rename, 1.0.0 is a large release: the PDF stack moved to a pure-Rust backend, the OCR story
grew from a single engine to a family of classical and vision-language models, and whole new
capabilities landed — audio/video transcription, named-entity recognition, structured LLM extraction,
sparse/late-interaction retrieval, and four new language bindings.

For a step-by-step upgrade, see the [migration guide](/migration/from-kreuzberg-v4/).

### Migration from Kreuzberg v4

- Packages are renamed `kreuzberg` → `xberg` across every ecosystem (crates.io, PyPI, npm, Maven,
  NuGet, Composer, RubyGems, Hex, Go).
- The Rust error type `KreuzbergError` is now `XbergError`.
- Environment variables are re-prefixed `KREUZBERG_*` → `XBERG_*`, and config files are discovered as
  `xberg.{toml,yaml,yml,json}`.
- **Breaking API changes:** extracted URIs are returned as `ExtractedUri` (formerly `Uri`); document
  metadata drops the untyped `additional`/serde-flatten bag in favour of typed fields plus a `custom`
  residual map.
- The **R binding**, the **EasyOCR** backend, and the bundled **pdfium** fork are removed (see Removed).
  Existing Kreuzberg v4 installs keep working under their original names.

The full identifier mapping is in the [migration guide](/migration/from-kreuzberg-v4/).

### Added

- **A family of OCR backends.** Alongside Tesseract, 1.0.0 adds a native **PaddleOCR** backend
  (PP-OCRv6, with `medium`/`small`/`tiny` tiers) and a pure-Rust **Candle** OCR/VLM stack — **TrOCR**,
  **GLM-OCR**, **GOT-OCR**, **DeepSeek-OCR**, and **PaddleOCR-VL** — that runs without ONNX Runtime or
  native Tesseract. Model weights are self-hosted on the `xberg-io` Hugging Face org.
- **A second, ONNX-Runtime-free inference path (tract).** CNN classifiers, layout detection (RT-DETR),
  and auto-rotation run through a pure-Rust `tract` backend on targets without ONNX Runtime — this is
  what makes in-browser (WASM) and mobile inference possible.
- **Structured (LLM) extraction.** `extract_structured` and `split_and_extract` drive a vision-LLM
  client with rasterization, chunking, citations, caching, and configurable `CallMode` / `MergeMode` /
  VLM-fallback policies.
- **Audio and video transcription.** A Whisper ONNX encoder/decoder engine extracts text from `.mp3`,
  `.wav`, `.m4a`, `.mp4`, and `.webm`.
- **Named-entity recognition.** GLiNER2-based entity extraction, including an in-browser WASM
  `NerModel` that detects entities locally with no server round-trip.
- **Retrieval building blocks.** Sparse embeddings (SPLADE), ColBERT late-interaction retrieval, and a
  cross-encoder reranking / semantic-search stage alongside dense embeddings, with self-hosted model
  presets pinned by sha256 manifests.
- **Text intelligence.** Redaction with reversible rehydration and per-entity erasure, summarization,
  translation, VLM image captioning, QR-code detection, document diffing (`revisions` on
  `ExtractionResult`), and page/chunk classification.
- **URL and web ingestion.** `map_url` discovers URLs from sitemaps and a shared crawl engine batches
  multi-URL extraction, over a URI-based `ExtractInput` / `ExtractionOutput` envelope.
- **New document formats (98 total).** WordPerfect `.wpd`/`.wp`/`.wp5` (via a vendored `xberg-libwpd`),
  HEIC/HEIF/AVIF images (via a vendored libheif), OpenDocument Presentation `.odp`, Quarto/R Markdown,
  configurable Jupyter cell rendering, and the audio/video formats above.
- **Four new language bindings.** Dart/Flutter, Swift, Kotlin/Android, and Zig — for 15 language
  bindings over one engine, with Android/iOS cross-compilation.
- **First-party integrations, consolidated into the monorepo.** LangChain.js, LlamaIndex, an n8n
  community node, CrewAI, and a Spring AI document reader.
- **Richer chunking and API surface.** Caller-supplied tokenizers, `TableChunkingMode::RepeatHeader`,
  RAG chunking with heading-path breadcrumbs, multi-label chunk classification, per-page spans with
  bounding boxes, a `list_supported_formats()` call in every binding, cheap `pdf_page_count`, and a
  `DELETE /jobs/{job_id}` cancellation endpoint on the API server.
- **Wider code intelligence.** tree-sitter coverage grows from 248 to 306 programming languages.

### Changed

- **PDF backend replaced.** pdfium is gone; `pdf_oxide`, a pure-Rust engine, is now the sole PDF
  backend — no native pdfium dependency.
- **Layout-aware PDF pipeline.** Reading order is reconstructed with ONNX layout detection
  (PP-DocLayoutV3 / RT-DETR) and Docling-style predecessor-graph reordering; scanned PDFs are detected
  and OCR'd selectively per page; AcroForm/XFA form fields and outline-based headings are extracted.
- **Public API stabilized** and frozen for 1.0, with a Rust-only `Engine` and extension seams.
- **Renamed from Kreuzberg to xberg** across packages, namespaces, and the `KreuzbergError` →
  `XbergError` type (see Migration).
- **Environment variables** use the `XBERG_` prefix; new layout, OCR model-tier, CoreML, and ORT
  execution-provider variables are available.
- **Config discovery** now also accepts the `.yml` extension (`xberg.{toml,yaml,yml,json}`) with an XDG
  config-directory fallback.
- **Models and cache** live under the `xberg` cache segment and the `xberg-io` Hugging Face org; the
  project domain is `xberg.io`.
- **Python support** widens to 3.10–3.14; the SurrealDB connector moves to v3 (dropping `mem://`); the
  default `extraction_timeout_secs` is 60s.
- **License.** Relative to the Kreuzberg 4.8/4.9 line (Elastic License 2.0), xberg 1.0.0 is **MIT**.

### Fixed

More than 150 bugs were resolved during the 1.0 cycle. Highlights by area:

- **PDF text fidelity:** text inside Marked-Content (MCID) blocks is no longer dropped from
  markdown/HTML output (#917); ligature glyphs no longer map to control characters (#1135);
  glyph-spaced text no longer extracts one character per line (#962); spurious intra-word spaces in
  native extraction are fixed (#1291, #1222); JPEG 2000 images no longer render blank and silently
  break OCR (#1158); XML entity references (`&amp;`/`&lt;`/`&gt;`) are preserved (#1242).
- **Tables:** bordered / graphical-line tables are detected reliably instead of silently skipped (#964,
  #1097, #1213); rotated full-page tables no longer extract as word salad (#1220, #1221); duplicate
  table emission and double-counting are fixed (#1288); physically fragmented per-row tables are merged
  back with their header row (#1290, #1100); borderless and text-heavy grids keep their row
  associations (#1316, #1319).
- **Reading order & structure:** two-column reading order no longer scrambles headings (#1170); stale
  page boundaries after reordering no longer panic on multibyte text or drop documents (#1270, #1272);
  numbered and cover-page headings are classified correctly (#961, #966, #1096, #1098); filled form
  field values are placed correctly (#1120).
- **OCR:** an explicit PaddleOCR backend no longer silently falls back to Tesseract (#801, #1071, #1088,
  #1102); scanned-page OCR text and page provenance surface consistently across content, pages, and
  chunks (#1095, #1110, #1281); spurious auto-OCR on born-digital PDFs is suppressed (#1176); a SIGBUS
  crash and a NaN-sort panic in the OCR pipeline are fixed (#1057, #1179); the Candle VLM OCR backends
  are stabilized (#1174, #1175, #1208–#1214); model downloads handle TLS-MITM CAs, IPv6 blackholes, and
  connect timeouts (#1146, #1249).
- **Chunking & provenance:** chunk `firstPage`/`lastPage` and byte ranges are correct across output
  formats and long PDFs (#1013, #1074, #1105, #1294); markdown chunks retain markdown (#1073, #1094);
  split-table chunks keep their header and context (#1100).
- **Bindings & packaging:** fixed Go embed symbols, Java `UnsatisfiedLinkError`, missing C# config
  types, wrong Node/PHP embedding shapes, Android `.so` loading, macOS wheel floors, and musl/ONNX
  Runtime runtime deps (#871, #965, #991, #998, #1008, #1055, #1131, #1257, #1304, #1307); plus Homebrew
  404s, Docker stop-signal handling, and multi-arch `-core` images (#1081, #1147, #1247, #1315).
- **Formats:** EML HTML `<table>` bodies, DOCX hyperlink/bold overlap and markdown conversion, archived
  markdown/CSV escaping, and Korean-charset EML detection are fixed (#942, #1086, #1212, #1237, #1278).
- **Config & robustness:** `extraction_timeout_secs` is honoured on every path (#830, #911, #1273);
  `cancel_token`, custom LLM base URLs, and page-classification config all validate correctly (#937,

  #944, #1076).

### Removed

- **R binding** — the Kreuzberg v4 LTS line is the last to ship it.
- **EasyOCR backend** — the Python/torch-only backend did not survive the Rust rewrite; use Tesseract,
  PaddleOCR, a Candle backend, or a VLM backend instead.
- **Hunyuan-OCR Candle backend** — ported during development, then dropped before 1.0.0.
- **Bundled `pdfium-render` fork** and its `KREUZBERG_PDFIUM_BUNDLED_PATH` variable, and the standalone
  `@kreuzberg/core` npm package.

### Performance

- **OCR memory discipline:** concurrent Tesseract sessions are capped, Leptonica/Pix/page buffers are
  released early, decoded RGB buffers are reused, and images are resized without copies.
- **Layout inference:** model sessions are pooled, batch inference threads are balanced, and an
  unnecessary PNG raster round-trip is bypassed.
- **Engine and batch:** bounded batch scheduling, no per-item config clone, single PDF parse per
  structured rasterization, streamed batch JSON output, and base64 hosted embeddings.
- **PDF and text:** streamed RGB conversion, reused OCR render document, skipped redundant compatibility
  parses, and a regex→scanner rewrite that removes backtracking from text/quality cleanup.

### Security

- An untrusted RTF size field in the email extractor could allocate up to 4 GB — now bounded (#1058).
- A redaction path could leak PII across roughly a dozen output fields — fixed (#1223).
- PDF embedded streams are guarded by a decompression-ratio limit and per-embedded-file size caps, and
  a `SecurityBudget` is wired through the PDF and email extractors.
- Excel DDE / external-call formulas raise warnings during extraction.
- FFI image and attachment buffers now carry explicit lengths so callees never read past the buffer
  (#1056, #1059), and panics on malformed input are replaced with recoverable errors (#907, #1057,

  #1198).

### Packaging

- pdf_oxide replaces the pdfium native dependency; libheif (LGPL, documented) and `xberg-libwpd` are
  vendored; retrieval and OCR model presets are self-hosted on `xberg-io` with sha256 manifests.
- Distribution hardening across all 15 targets: ONNX Runtime bundling, glibc/musl floors (musl via
  Alpine images), NuGet runtime-package size splits, Homebrew bottles, Go module tags, Swift C++
  linkage, and Dart/Swift/Kotlin-Android/Zig release matrices. Published to crates.io, PyPI, npm,
  Maven Central, NuGet, RubyGems, Packagist, Hex, pub.dev, Go, Swift Package Manager, Homebrew, Docker
  (`ghcr.io/xberg-io/xberg`), and a Helm chart.
