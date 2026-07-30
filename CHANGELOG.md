# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

---

## [Unreleased]

### Fixed

- **#1346**: PaddleOCR emits a `ProcessingWarning` when requested languages are not covered by
  the single selected recognition model (previously their text was silently dropped), and OCR
  metadata now reports the recognition model actually used instead of joining every requested
  language.

## [1.0.4] - 2026-07-30

### Added

- MCP clients can run `extract`, `extract_batch`, and `cache_warm` as cancellable SEP-2663 tasks
  when they advertise task support; synchronous clients remain compatible.
- MCP `cache_clear` and `cache_warm` return typed structured results with cleared-file totals and
  model availability separated from confirmed cache-hit and download status.

### Changed

- Dependency bumps: `crawlberg` 1.0.11, `tree-sitter-language-pack` 1.13.6, `base64` 0.23
  (xberg-jni).

### Fixed

- **#1338**: Default `OcrStrategy::Auto` extraction OCRs scanned PDFs with no native text layer
  instead of returning empty content; explicit OCR disablement remains authoritative.
- **#1341**: Synthesized VLM fallback pipelines run for mixed native/OCR PDFs, preserve skipped
  and failed-stage diagnostics, and retain the last non-empty fallback when every stage scores
  below threshold.
- **#1340**: PDF images and generated captions render at bounding-box-aware reading-order
  positions, remain within the correct layout column, preserve source order, and stay consistent
  through chunking, translation, and redaction.
- **#1343**: Archive extraction skips macOS/tooling metadata entries (`__MACOSX/`, AppleDouble
  `._*`, `.DS_Store`, `Thumbs.db`, `desktop.ini`, `__pycache__/`, `.pyc`/`.pyo`) instead of emitting
  them as `text/plain` children, and unsniffable extensionless members default to
  `application/octet-stream`; a single aggregated warning records what was filtered.
- Per-file OCR language overrides now also apply to explicit Tesseract pipeline stages, preserving
  override precedence.
- PDF plain-text extraction repairs detached subscripts, phone suffixes, and final glyphs while
  preserving RTL, rotated, vertical-writing, and mathematical span order.
- PDF Markdown atomically replaces adjacent native side-by-side table cohorts with validated layout
  table cohorts, avoiding mixed grids and dropped financial-table structure.
- OCR Markdown applies layout hints to line-local geometry while preserving soft-wrapped body
  paragraphs and merging multi-line headings, code, pictures, and wrapped list items by hint.
- Tesseract OCR Markdown aligns layout hints and table-cell matching with DPI-normalized and
  auto-rotated image coordinates, restoring semantic structure on scanned PDFs.
- OCR Markdown recovers missing ordered-list successors only when an existing numeric list item
  anchors a complete, bounded three-item sequence across pages.
- PDF Markdown preserves strong native headings when a lower-confidence layout Code hint lacks
  structured code evidence.
- OCR Markdown recovers a title from a guarded first-block logo/title pattern when the layout model
  emits no semantic heading region.
- PDF Markdown preserves native heading, list, code, and formula semantics while using layout
  geometry for reading order, grouping, and tables, tolerates minor crop jitter in side-by-side
  cohorts, merges sparse currency-affix columns without dropping markers, and folds wrapped
  financial-table lines into logical records; table-dominant pages also discard bbox-confirmed crop
  spill while retaining surrounding prose and annotations.
- PDF Markdown reconstructs paired wrapped financial tables as semantic three-column grids and
  repairs consistently merged numeric columns from native PDF table detection.
- **#1342**: PDF table reconstruction retains short numeric grids when a small number of inferred
  columns make the principal data row nearly complete instead of fully populated.
- PDF Markdown recognizes repeated large-font heading tiers across sparse multi-page documents while
  retaining the single-page sparse-document safeguard against display-text false positives.
- Wide PDF numeric tables retain validated three-row headers while preserving the compact prose and
  caption safeguards used by smaller grids.

## [1.0.3] - 2026-07-29

### Added

- PDF benchmark fixtures can pin Tesseract OCR languages. The benchmark harness validates language
  codes, checks required packs before timed extraction, and preserves the effective OCR backend and
  cache settings when applying per-file batch overrides.

### Changed

- Upgraded `rmcp` to 3.0.0 and migrated the MCP server to its 3.0 API (schema output, the new
  cache-scope/result-type/TTL list-result fields, and the `GetPromptResponse`/`ReadResourceResponse`
  handler enums). The exposed tools, prompts, and resources are unchanged.
- OCR now emits `tracing` logs when it materializes a Tesseract language pack at runtime: an
  info line naming the language, destination, and source before the download, one per candidate
  URL as it is tried, and one on success. Previously a runtime language-pack download was silent,
  making a first-use OCR stall on a missing pack hard to diagnose. English is unaffected on builds
  with the `bundle-tessdata-eng` feature (embedded, no download).
- Dependency bumps: `liter-llm` 1.11.4, `toml` 1.1.4.

### Fixed

- **#1333**: A sparse continuation row no longer dilutes the numeric ratio used to classify a grid, so
  numeric line-item tables with a trailing partial row are kept as tables instead of being flattened to
  prose.
- **#1336**: Tesseract no longer creates OCR cache directories when caching is disabled; the cache
  directory is created lazily, only when a result is written.
- **#1337**: Light-text-on-dark-background scans are auto-inverted before OCR via mean-luminance
  polarity detection, and the previously-dead `invert_colors` config is honored as an explicit override
  (`Some` forces, `None` auto-detects).
- **#1338**: NER and summarization processors are now compiled into the container and CLI builds — they
  were feature-gated out, so `ner`/`summarization` config was silently dropped. Under
  `OcrStrategy::ScannedPages`, a whole-document text failure now OCRs every page instead of discarding
  the signal.
- **#1339**: VLM OCR forwards `XBERG_LLM_*` env credentials to `ocr.vlm_config` when a custom
  `base_url` is set, normalizes openai.com model names, routes bare images through the OCR pipeline so
  `vlm_fallback` `on_low_quality` fires, and surfaces per-stage OCR failures as processing warnings.
- Linux builds without CUDA or TensorRT no longer fail under strict warning settings because of an
  unused ONNX Runtime execution-provider trait import.
- Per-file OCR language overrides (CLI and benchmark) now reach nested Tesseract configurations.
- PDF plain-text extraction repairs detached text spans so words are no longer split mid-token.
- PDF plain-text extraction retains table assets without rendering native table text twice.
- PDF extraction recovers and stitches label-heavy financial tables without merging independent
  aligned tables.
- PDF Markdown preserves explicit word boundaries and changelog heading hierarchy.
- OCR Markdown prefers validated semantic layout hints over broad text regions at comparable
  overlap.

## [1.0.2] - 2026-07-28

1.0.2 is a packaging release. It completes the 1.0.1 rollout — the PHP/Packagist binding failed to
build for 1.0.1 — and adds a first-party coding-agent plugin. No core extraction behavior changed.

### Added

- **Coding-agent plugin.** A first-party xberg plugin for Claude Code, Codex, Cursor, and OpenCode,
  with a Hermes variant, ships extraction skills (batch extraction, chunking, OCR, tables, keywords,
  format selection) that drive xberg through its MCP/CLI surface. Published as
  `@xberg-io/opencode-xberg` (npm) and `xberg-hermes-plugin` (PyPI).

### Fixed

- The PHP binding now builds against `ort` 2.0.0-rc.13. rc.13 moved the CoreML/CUDA/TensorRT
  execution-provider types behind matching Cargo features; a fresh dependency resolution (as on the
  PHP build) picked up rc.13 and failed to compile. Those EP features are now enabled unconditionally
  — a compile-time `#[cfg]` unlock only, with no SDK dependency or runtime change — so the
  PHP/Packagist package publishes again.

### Packaging

- Drops the Node `@xberg-io/xberg-win32-arm64-msvc` sub-package. It was declared as an optional
  platform dependency but never built — no xberg binding targets Windows on ARM64 — leaving an
  unresolvable optional dependency. The target is removed from the package manifest and loader for
  parity with the other bindings.
- Republishes every binding at 1.0.2 to close the 1.0.1 gaps (notably PHP/Packagist).

## [1.0.1] - 2026-07-28

### Fixed

- **#1321**: Borderless, text-heavy tables are recovered on pages that also contain an ML-detected
  table. The geometric-table fallback now runs per region instead of per page, so a single ML `Table`
  hint no longer suppresses borderless-grid recovery across the rest of the page; words already inside
  an existing table hint are excluded so regions are not detected twice.
- **#1326**: RTF hex byte escapes now decode through the active font's `\fcharsetN` charset (mapped to
  a Windows codepage), falling back to `\ansicpgNNNN` and then Windows-1252. Documents that declare a
  Cyrillic or other non-ANSI font in the font table now decode as readable text instead of
  Windows-1252 mojibake, and font switches mid-document are tracked across nested groups.
- **#1328**: Page markers now appear verbatim in Markdown and Djot output. Flat documents no longer
  backslash-escape the marker (`\<\!-- PAGE 1 --\>`), and structured native documents no longer drop
  it entirely.
- **#1323**: RTF hex byte escapes now honor `\ansicpgNNNN` via the shared Windows-codepage table, so
  CP1251 Cyrillic and other non-1252 ANSI byte runs decode as readable text instead of Windows-1252
  mojibake; adjacent escapes decode as one multi-byte run, surviving line wraps, and formatting spans
  stay aligned with the decoded text.

### Added

- `LayoutStrategy` enum on `LayoutDetectionConfig` (`strategy` field, default `always`). `auto` pre-screens each PDF page with cheap geometry signals and runs the layout model only on pages likely to benefit; existing configs keep the every-page behavior bit-for-bit. On the OCR path only inference is skipped, since OCR consumes the layout pass's rasters. Skipped pages are auditable via `metadata.format.layout_gated_pages` and `layout_gate_reasons`, and the CLI gains `--layout-strategy` ([#1322](https://github.com/xberg-io/xberg/issues/1322)).

### Packaging

- Republishes `xberg-libwpd` with the static zlib link fix so `xberg-cli` links against a working
  release. The 1.0.0 `xberg-libwpd` crate was published before the fix and left the librevenge
  `inflateInit2_`/`inflate`/`inflateEnd` symbols undefined at final link, breaking `xberg-cli` builds
  from crates.io. No source API changes.

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
