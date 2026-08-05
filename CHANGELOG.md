# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

---

## [Unreleased]

### Added

- New `xberg-ttf-parser` crate: a vendored copy of `ttf-parser` 0.25.1 carrying the fix above.
  Upstream is currently unmaintained, so vendoring lets us ship font parser fixes without waiting
  on a release. The crate tracks which upstream pull requests it carries in its README.
- New `xberg doctor` command and `doctor()` API probe the configured OCR backend, layout
  detection, and caches, then report pass / warn / fail / skip with a one-line reason —
  answering "is it my document or my environment?" before the first document, with no
  downloads and no billable API calls. `xberg doctor --clean` removes stray files from
  xberg-owned cache dirs; the shared Hugging Face cache is never modified. Custom OCR
  backends can add their own diagnostics via the new `OcrBackend::probe` hook. (#1347)
- Added Sceptre as an EasyOCR Gen2 backend using ONNX Runtime on desktop/server and tract on
  Android/iOS, with a separate byte-fed WebAssembly API intended for Web Workers. Select it with
  `ocr.backend = "sceptre"` or `--ocr-backend sceptre`; it returns line quadrilaterals and
  recognition confidence, supports all eight Gen2 model groups, and accepts tuning under
  `backend_options`.

### Fixed

- Image OCR benchmarks now score structural F1 only against genuinely structured Markdown ground
  truth; scene-text fixtures remain text-only, and a dedicated structured image cohort covers
  receipts, document pages, tables, and invoices.
- Benchmark adapters now honor fixture OCR languages, partition batch-global backends into
  homogeneous native batches, and record unsupported-language exclusions in provenance instead of
  silently evaluating non-English documents with default English models.
- PDF pages that embed a CFF font converted from Type 1 no longer lose their dot-bearing glyphs.
  The font parser rejected the deprecated `dotsection` operator and discarded the entire glyph, so
  every `i`, `j`, `!` and `.` on the page rendered as blank space, and OCR run over those pages
  transcribed the gaps. The parser now ignores the operator, matching FreeType and read-fonts.
- A malformed embedded font can no longer make PDF rendering hang. Composite glyph outlining and
  COLRv1 colour painting both bounded only how deeply they recursed, not how much total work a
  crafted font could force, so a glyph whose components all point at one shared child could drive
  exponential work. Both now carry a total visit budget.
- Fonts at the maximum 65535 glyphs now parse. The glyph offset table needs 65536 entries at that
  size, which overflowed a counter and dropped the table, leaving the font with no outlines at all.
  
## [1.0.14] - 2026-08-04

### Fixed

- OpenWebUI-compatible endpoints (`PUT /process` and `POST /v1/convert/file`) now honor extraction
  configuration. They previously cloned the server default, forced Markdown output, and ignored all
  inbound parameters, so configuration passed through OpenWebUI had no effect. They now use the
  server's configured defaults as the base and merge a per-request config — a multipart
  `config`/`parameters` field, or the `X-Config` header — matching the `/extract` endpoint, keeping
  Markdown as the default only when neither the server config nor the request selects a format.
- Image captioning is now included in the official Docker images (`--features all`), and the server
  emits a `ProcessingWarning` when a `captioning` config is supplied but the feature is compiled out,
  instead of silently doing nothing (#1382).
- Release builds no longer check out the `test_documents` benchmark submodule, so a benchmark-only
  submodule update can no longer fail every publish build and ship a release with no assets (#1380).

### Changed

- Embedded-image captioning now runs with bounded concurrency (mirroring the image-OCR path) instead
  of one VLM request at a time, reducing wall-clock time on image-heavy documents (#1378).

## [1.0.13] - 2026-08-04

### Fixed

- OCR-backed PDF extraction now preserves geometry-derived document structure without requiring
  optional ML layout detection, including pages replaced by mixed native/OCR extraction, and keeps
  consecutive Tesseract paragraphs in their shared hOCR text area.
- PDF table reconstruction now rejects sparse, short-wide contact blocks that were previously
  misclassified as tables.
- Standalone-image Tesseract OCR now defaults to sparse-text segmentation, while cropped layout
  regions use single-block segmentation and explicit user settings remain unchanged. Vertical
  language packs such as Japanese (`jpn_vert`) use vertical-block segmentation.
- Standalone image extraction now reports successful OCR through `metadata.ocr_used` and the OCR
  extraction method, including layout-aware OCR results.
- Tesseract now applies its default image preprocessing only to clean, near-white document pages;
  shadowed receipts and photographic images keep their source pixels, avoiding quality loss from
  destructive DPI upscaling, background normalization, sharpening, and grayscale conversion.
- Sparse, low-confidence standalone Tesseract results now retry the previous automatic page
  segmentation with explicit preprocessing and use it only when word confidence is consistently
  strong, recovering difficult receipts and scene text without replacing reliable sparse output.
- CSV and TSV plaintext now use the canonical table renderer instead of lossy `Row N` and
  header-value prose.
- Extracted EML and MSG attachment text is now included in the parent document while the structured
  attachment children remain available.
- DOCX extraction now emits a tab character for an in-run `<w:tab/>` instead of dropping it, so
  tab-separated fields — most visibly Word table-of-contents rows — no longer weld adjacent words
  together (`Alpha<tab>Beta` was extracted as `AlphaBeta`). Tab-stop definitions remain invisible.
  (#1377)
- The Swift package builds and publishes again. The cross-compiled desktop `xberg-ffi` dependency no
  longer pulls in HEIC (`libheif-sys`, which has no cross-compile support) or the Candle OCR
  backends, which had broken Swift package publishing in 1.0.12.
- The NuGet runtime packages for macOS and Linux (`osx-x64`, `osx-arm64`, `linux-x64`, `linux-arm64`)
  now publish at the current version instead of being stuck at an older one; previously only the
  Windows runtime package was updated. (#1375)
- The public in-browser (WASM) demo now attributes its file-size limit to the browser sandbox and
  points to the CLI and API for large or multi-page documents, instead of implying the document
  itself is at fault. (#1376)

## [1.0.12] - 2026-08-03

### Fixed

- The `xberg mcp` `extract` and `extract_batch` tools no longer emit structured output that fails
  their own declared output schema. The schema required `errors` and the `crawl_*` fields, but a
  normal extraction omits them when empty, so MCP clients (e.g. Claude Code) rejected the result.
  Those fields are now optional in the schema, matching the serialized output. (#1372)
- The `install.sh` script no longer creates a self-referential `xberg` symlink that shadowed the
  installed binary, and it now selects the glibc (`-gnu`) build on standard Linux distributions
  instead of always downloading the musl build — which failed to run on glibc systems such as
  Ubuntu. musl systems (e.g. Alpine) still get the musl build. (#1371)
- CSV header inference no longer misclassifies all-text tables as headerless. A first row such as
  `Name,City` is now treated as the header (the dominant CSV convention) instead of rendering a
  broken blank header row with the real header pushed down into the data. A numeric-looking first
  row is still treated as data. (#1369)

## [1.0.11] - 2026-08-03

### Fixed

- The extraction HTTP server now bounds in-flight request concurrency so a burst of large uploads
  can no longer exhaust memory and OOM-kill the process in memory-limited containers. The limit
  defaults to `2 × CPU count` clamped to `[4, 32]`; override it with `XBERG_MAX_CONCURRENT_REQUESTS`
  (set `0` to disable). (#1368)
- PaddleOCR output now keeps consecutive visual text lines in the same Markdown paragraph instead
  of turning every detected line into a separate paragraph.
- PaddleOCR and Tesseract automatic image rotation now use the document-orientation model's RGB
  input and existing probability output correctly, and recover sparse edge-aligned text that the
  model's standard center crop omitted.

## [1.0.10] - 2026-08-02

### Fixed

- `cargo install xberg-cli` now succeeds on a stock Windows toolchain. HEIC/HEIF decoding links
  native `libheif`, which has no default build path on Windows, so it is no longer part of the
  CLI's default features and the install no longer fails building `libheif-sys`. Enable HEIC with
  `--features heic`; the prebuilt release binaries, Docker `all` image, and Homebrew bottle
  continue to ship it. (#1361)
- The `cargo binstall xberg-cli` static musl builds now compile. The #1355 image-fallback OCR
  helpers were gated on the `ocr` feature but are reachable under the `ocr-pipeline`-only
  `binstall` profile, which failed to build both musl targets in the 1.0.9 release.
- `brew install xberg-io/tap/xberg` installs a working binary again instead of an empty bottle;
  the 1.0.9 bottle rebuild had been skipped when the CLI asset upload cascaded from the failed
  binstall build. (#1356)
- The hosted demo page (docs.xberg.io/demo.html) no longer 404s its toolbar and file-picker
  icons. (#1360)
- Dart native-library loading now propagates download, filesystem, and checksum failures instead
  of silently falling back to an unverified default library resolution path.
- PaddleOCR concurrent cold starts now run off async worker threads and share one engine
  initialization per model and accelerator, with distinct cache entries for different GPU device IDs.
- Benchmark text F1 now segments CJK around embedded Latin and numeric text while ignoring OCR line
  wrapping, preventing mixed-script output formatting from distorting quality comparisons.

## [1.0.9] - 2026-08-02

### Added

- `cargo binstall xberg-cli` now installs a self-contained, fully static musl CLI binary with no
  ONNX/Tesseract/libheif runtime dependencies. The `x86_64-unknown-linux-musl` build additionally
  bundles the pure-Rust Candle VLM OCR backends (TrOCR and PaddleOCR-VL); `aarch64-unknown-linux-musl`
  ships extraction-only. ONNX/Tesseract/HEIC OCR remain available via Homebrew and the bundled
  per-target release tarballs.

### Changed

- PaddleOCR now exposes the `PaddleOcrEngine` name and detailed word-level quadrilaterals;
  the former `OcrLite` name remains available as a deprecated compatibility alias.
- Dense XLSX extraction now scans worksheet bounds without cloning every cell before normal range
  parsing, while oversized sparse sheets materialize their cells only once.
- Layout-enabled image table recognition now shares its decoded RGB raster with the TATR worker,
  avoiding one full image allocation and pixel-buffer copy per qualifying image.
- Multi-stage PDF OCR now shares rendered page rasters across pipeline tasks instead of copying
  each pixel buffer, reducing peak memory by roughly one RGB raster per concurrent page.
- Batch DOCX extraction reuses one owned input buffer and avoids rebuilding discarded document
  structure, reducing memory copies and structure-processing overhead for large files.

### Fixed

- Canonical PaddleOCR benchmark presets no longer force optional whole-image auto-rotation, avoiding
  confident but incorrect 180-degree rotations that suppressed scene-text quality.
- PaddleOCR now preserves native resolution for 1024-pixel images by default, improving scene-text
  accuracy while retaining explicit detector-size overrides.
- Layout-enabled image OCR now reuses successful single-frame whole-image text when structured
  assembly is unavailable, avoiding repeated region OCR and redundant Tesseract table analysis.
- PaddleOCR-only CLI builds no longer compile PDF Markdown layout reuse code when layout detection
  is disabled.
- The prebuilt macOS CLI tarballs (`aarch64-apple-darwin`, `x86_64-apple-darwin`) now bundle the full
  libheif dynamic-library closure beside the `xberg` binary and rewrite its load commands to
  `@loader_path`, so the binary no longer fails with a `libheif.1.dylib` not-loaded error on machines
  that lack Homebrew's libheif at the baked-in path (#1357).
- PaddleOCR layout and table consumers now use projected CTC word boxes while preserving line-level
  semantic text and caller-requested element granularity, avoiding mixed-level duplicate table text.
- PaddleOCR detection now honors its configured DB threshold and matches upstream dilation,
  perspective-crop, and visual-line ordering behavior, improving small, skewed, and jittered text.
- Apple Keynote packages containing only slide archives now route to the Keynote extractor, and
  Numbers extraction reconstructs tables instead of emitting raw protobuf fragments.
- AsciiDoc, NXML/JATS, and WebVTT files now route through their registered text or JATS extractors
  instead of being reported as unsupported.
- Standalone `excel` and `excel-wasm` feature builds now include the XML parsing and table-capacity
  support required by XLSX extraction.
- Org-mode extraction now distinguishes separator-defined table headers from headerless tables,
  preserving every data row in rendered Markdown.
- EPUB extraction now resolves `epub:switch` branches per output renderer, preserving supported
  XHTML and MathML cases while retaining readable plain-text fallbacks.
- Typst extraction now emits marker-free headings and distinguishes explicit table headers from
  bare table rows, preserving correct Markdown structure.
- MSG extraction now reads the canonical binary `PidTagHtml` stream with the Internet codepage,
  preserving HTML-only message bodies alongside attachments.
- TATR table reconstruction now assigns each selected OCR word exactly once, using the nearest
  cell when predicted cells do not overlap, preventing both duplicated and silently dropped text.
- Layout-enabled image extraction now recognizes TATR table structure from cached OCR elements
  while preserving non-table line structure and requiring complete OCR token retention before
  accepting the reconstructed layout.
- Layout-enabled OCR now preserves detected image headings without losing or reordering fallback text,
  and regroups adjacent PDF OCR lines without collapsing distant paragraphs or separate layout regions.
- Rotated PDF OCR now avoids reusing display-coordinate Markdown layout rasters and reruns layout
  on inverse-`/Rotate`-normalized images, keeping OCR upright without desynchronizing detections.
- Benchmark text F1 treats OCR-inserted line breaks within CJK text as layout whitespace,
  preventing semantically identical Chinese, Japanese, and Korean output from scoring zero.
- Pipeline quality benchmarks allow forced OCR inference enough time to finish instead of
  recording slow but valid OCR documents as zero-quality timeout failures.
- Benchmark fixture validation now accepts descriptor filenames without an explicit parent path.
- Pipeline benchmarks now preserve exact ordered cohort fixture paths, use explicit PP-OCR model
  identities and fixture OCR languages, and score structural image ground truth.
- PaddleOCR now reports processed image dimensions and applied orientation corrections, keeping
  OCR geometry aligned with optional layout detection on rotated documents.
- PaddleOCR now selects the Japanese model for vertical Japanese and prefers Korean or Japanese
  recognition for mixed Latin-script requests those models can cover.
- PP-OCRv6 requests containing Korean now use PaddleOCR's script-specific Korean recognizer,
  recovering Hangul text that the unified recognition model omitted.
- PaddleOCR now preserves the right-to-left column order and contiguous text of traditional
  vertical Chinese and Japanese documents.
- Image OCR now preserves blank-line paragraph boundaries instead of flattening every recognized
  text block into one paragraph.
- Tesseract vertical CJK OCR now removes artificial spaces between adjacent script characters
  while preserving Latin-word and paragraph whitespace.
- Jupyter notebook paths retain `application/x-ipynb+json` routing when generic JSON content
  detection runs, and extracted notebook content no longer exposes diagnostic cell/output markers;
  cell identity, execution, tag, output-type, and MIME details remain available as structured metadata.

## [1.0.7] - 2026-07-31

### Added

- **Candle VLM OCR backends now ship in the published packages.** The pure-Rust Candle OCR
  backends — TrOCR, PaddleOCR-VL, GLM-OCR, and DeepSeek-OCR — are compiled into the published
  packages by default (Python, Node, Go, Java, C#, Ruby, PHP, Elixir, Kotlin/JVM, Zig, and the
  CLI / Docker image) on Linux, macOS, and Windows. Select one with `ocr.backend =
  "candle-glm-ocr"` (or `candle-trocr` / `candle-paddleocr-vl` / `candle-deepseek-ocr`); model
  weights download from Hugging Face on first use. Previously these backends were excluded from
  the `full` feature and reachable only via a custom source build. Not available on WebAssembly,
  Android, iOS, Dart, or Swift.

### Fixed

- **#1355 — `force_ocr` no longer emits a silently blank page** when the PDF rasterizer cannot
  draw an image XObject. When a `force_ocr` page renders blank but carries image XObjects, OCR is
  retried directly on the embedded image bytes (decoded pixels, or the raw JPEG/JP2 stream) and a
  processing warning is recorded, so the page content is recovered instead of dropped without
  notice.
- **Swift artifact-bundle cross-compile**: the cross-compiled Swift binary bundle builds again —
  the HEIC path (which shells out to `pkg-config` and cannot cross-compile) is dropped from the
  Swift / Intel-macOS cross-build feature set (`full-no-heic`), restoring the `x86_64-apple-darwin`
  and Linux Swift builds. The native C FFI distribution keeps HEIC.
- **XLSX extraction on Windows**: the `excel` feature is enabled in the Windows feature set, so
  `.xlsx` files extract on Windows instead of returning `UnsupportedFormat` for a format the
  registry advertises as supported.
- Benchmark CI validates ground truth for every format family plus the exact 101-cell workflow
  matrix and harness contracts before expensive jobs, and the local benchmark task now delegates
  to the same run wrapper.
- Benchmark quality rankings and Pareto SF1 multiply successful-extraction medians by accountable
  coverage exactly once, so partial framework failures cannot retain a perfect rank while harness
  and setup failures remain excluded.
- Benchmark runs abort instead of dropping task errors, verify exact eligible-document
  cardinality before writing artifacts, reject contradictory failure states and unknown pipeline
  names, and report extension success rates with the same accountable-failure semantics as the
  aggregate.
- Benchmark CI invalidates its prebuilt harness cache for harness build scripts, workspace and
  toolchain configuration, compiler/codegen environment, and every transitive workspace crate,
  preventing stale binaries. Release tokens now default to the current repository installation.
- Present best-effort benchmark artifacts receive the same provenance, supported-format
  cardinality, failure-accounting, and aggregate integrity validation as required artifacts;
  only absence and framework-accountable extraction failures remain optional. Consolidated
  provenance, metadata, failure summaries, and rankings are cross-checked against validated
  groups and rows, with ranking optionality derived from the active cohort rather than a global
  framework union.
- Subprocess benchmark results record the framework's declared supported extensions, preserving
  the capability context needed to interpret historical multi-format aggregates.
- Benchmark quality guardrails fail on missing contracted documents or pipeline results instead
  of reporting a vacuous pass, and reject unknown pipelines, empty predicates, and invalid
  thresholds before execution.
- Unstructured benchmark cells advertise only their supported plaintext output, and pipeline
  benchmarks reject unknown sort metrics instead of silently falling back to SF1.
- Benchmark fixtures reject document and ground-truth paths that escape the repository or
  standalone fixture trust boundary, including symlink escapes, and derive repository boundaries
  from runtime fixture locations so cached binaries remain portable across CI runners. Artifact
  provenance hashing uses the same validated path resolution.
- Benchmark CI records declared per-framework format support, validates partial-run thresholds
  before execution, evaluates them independently per framework, and excludes harness/setup errors
  from framework success rates while retaining strict extraction coverage for every xberg pipeline.
- Benchmark comparisons include formats with text-only ground truth, report structural scores as
  unavailable instead of zero when Markdown ground truth is absent, and identify guardrails by file
  type so same-named fixtures cannot be matched across formats. Guardrails are rebased against the
  active corpus, removing retired PDF contracts and covering every actionable current result.
- Image layout extraction reuses safely positioned whole-image OCR elements, falls back when
  region-based OCR drops substantial text or quality, and preserves warnings without redundant OCR
  retries.
- EPUB extraction removes duplicated serialized MathML and embedded-media fallback content, and
  avoids emitting a cover image twice when the spine already references it.
- Email extraction preserves sender display names alongside addresses, and asynchronous attachment
  and nested-message extraction reuses the initial parse instead of parsing messages twice.
- FB2 and DocBook files with generic XML signatures retain their extension-specific MIME types, so
  they route through the semantic FictionBook and DocBook extractors instead of the generic XML
  fallback.
- Nested objects and arrays in JSON documents render as structured Markdown headings and lists
  instead of opaque compact-JSON strings, preserving readable nested keys and values.

## [1.0.6] - 2026-07-31

### Fixed

- **libwpd (Windows/MSVC)**: link the vcpkg-provided static zlib so librevenge's `inflate*` symbols
  resolve at the final link. Windows binding builds previously failed with `undefined symbol:
  inflate` because the MSVC path emitted no usable zlib link directive.
- **#1344 follow-up**: Automatic PDF layout inference retries once on CPU only for runtime inference
  failures, keeps explicitly selected non-Auto providers and recognized `XBERG_ORT_EP` values
  authoritative, ignores blank or unrecognized environment values, and propagates the effective or
  recovered CPU provider to downstream TATR and OCR table reconstruction.
- Side-by-side PDF TATR tables match source words that narrowly cross a detected outer edge to the
  outermost cell without changing the inference crop or center seam, preserving financial-table row
  prefixes that previously fell just outside the recognized cell bounds.

### Changed

- Upgrade sibling dependencies: `crawlberg` 1.0.11 → 1.1.0, `html-to-markdown-rs` 3.9 → 3.10,
  `liter-llm` 1.11 → 1.12. `liter-llm` 1.12 makes `tracing` an always-on dependency and removed its
  `tracing` Cargo feature, so it is dropped from the dependency declaration (no behavior change —
  liter-llm spans are always emitted now).
- The `otel` feature now forwards to `crawlberg` and `liter-llm` (weak, `crawlberg?/otel` /
  `liter-llm?/otel`), so enabling `xberg/otel` compiles those siblings' direct OpenTelemetry
  integration (crawlberg's semconv/propagation, liter-llm's `gen_ai.*` metrics); their spans and
  metrics are exported by the host's provider (e.g. xberg-enterprise). `html-to-markdown-rs` and
  `tree-sitter-language-pack` are pure `tracing` emitters with no `otel` feature — their spans reach
  the collector through the consumer's `tracing-opentelemetry` layer, so nothing is forwarded to them.

## [1.0.5] - 2026-07-30

### Fixed

- **`xberg-libwpd` Windows build**: the WordPerfect extractor now compiles and links on
  `x86_64-pc-windows-msvc`, unblocking the full-feature Windows binary of downstream consumers. Two
  first-ship gaps in the vendored C++ build are fixed: (1) zlib (needed by librevenge's
  `RVNGZipStream`) is now built from source via `libz-sys` on Windows too — as it already was on
  Linux/macOS — instead of relying on a vcpkg-installed zlib that CI did not reliably provide
  (`fatal error C1083: Cannot open include file: 'zlib.h'`); and (2) a narrowing
  `std::make_shared<WP6SubDocument>(…, m_streamData.size())` call in `WP6GeneralTextPacket.cpp` — a
  64-bit `size()` into a 32-bit `const unsigned` param — is patched to cast `(unsigned)` (matching
  every sibling subdocument site), which the newest MSVC toolchain (14.5x) otherwise rejects as a
  hard error. The vcpkg zlib probing in `build.rs` is removed.
- **#1345**: Sparse native two-column PDFs preserve column-block reading order instead of
  interleaving their four text lines row-by-row across the gutter.
- **#1346**: PaddleOCR emits a `ProcessingWarning` when requested languages are not covered by
  the single selected recognition model (previously their text was silently dropped), and OCR
  metadata now reports the recognition model actually used instead of joining every requested
  language.
- **#1344**: Layout inference no longer silently degrades to no-layout output when a hardware
  execution provider fails. macOS `auto` acceleration resolves RT-DETR to CPU up front (its current
  export cannot execute under CoreML), so the common path never attempts a failing provider. When an
  *explicit* accelerated provider does fail at inference (for example a CoreML `ExecuteKernel`
  error), both the markdown and OCR layout paths retry once on the always-available CPU provider and
  recover the layout, and either way surface a `ProcessingWarning` (recovered-on-CPU, or lost
  entirely if CPU also fails) instead of returning byte-identical no-layout output with empty
  `processing_warnings`.
- **#1349**: Successful TATR table reconstruction no longer writes source cell content and
  coordinates to stderr; the debug output is removed.
- **#1350**: The Markdown hierarchy no longer merges a distant header and footer into one block —
  paragraph continuation now rejects merges across a large vertical baseline gap and recomputes the
  merged block's bounding box.
- **#1351**: The published Node package ships the alef-generated `index.d.ts` (clean, consistent
  types) rather than the raw `napi build` output, which emitted references to undefined `Js*` types.
- **#1353**: The install script copies nested runtime library directories (for example
  `lib/libheif`) with `cp -R`, instead of failing with `cp: -r not specified; omitting directory` on
  Linux musl installs.

### Changed

- Raw `println!`/`eprintln!`/`print!`/`eprint!`/`dbg!` are now denied in production code across the
  whole workspace (clippy `print_stdout`/`print_stderr`/`dbg_macro`); `tracing` is the sole
  diagnostic surface. The CLI's machine-readable result output to stdout opts back in per call site
  (`#[expect(clippy::print_stdout)]`), and the regenerated language bindings route their FFI-bridge
  diagnostics through `tracing` instead of `eprintln!`.
- Internal diagnostics that previously wrote to stderr via `eprintln!` (per-page OCR gate decisions,
  GLM-OCR debug tensor stats, the CLI `--output-format` deprecation notice) now emit through
  `tracing` at the appropriate level, so verbosity is controlled with `RUST_LOG` / `--log-level`
  instead of ad-hoc `XBERG_DEBUG_OCR` / `XBERG_GLM_DEBUG` environment variables.
- Repeated per-page and per-backend warnings from external dependencies (OCR engines, layout models)
  are now de-duplicated by `(source, message)`, so an N-page document surfaces one warning per
  distinct problem rather than N copies. The paddle-ocr uncovered-language warning is also logged.

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
