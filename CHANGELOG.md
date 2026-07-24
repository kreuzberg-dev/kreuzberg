# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

---

## [Unreleased]

### Fixed

- PaddleOCR now applies `PaddleOcrConfig::rec_batch_num` to recognition inference.

## [1.0.0-rc.37] - 2026-07-24

Republish of rc.36 under a fresh version: rc.36 again partially published (the core PyPI linux
wheels, the npm/PyPI/Maven integrations, the Homebrew sequoia bottle, and the `-core` Docker image
all failed on release-pipeline bugs while the immutable registries had already shipped, burning the
rc.36 number). The library code is unchanged from rc.36; this release fixes the pipeline so every
channel publishes.

### Fixed

- **Python linux wheels.** The glibc-floor verifier matches auditwheel's hashed ONNX Runtime soname
  (`libonnxruntime*.so*`), so the linux wheels no longer fail verification after correctly bundling
  the runtime.
- **Go module resolution.** The `packages/go` module tag is created on release again, so
  `go get .../packages/go@v1.0.0-rc.37` resolves.
- **Swift C++ runtime linkage.** The Swift package links the C++ standard library (libc++ on Apple
  platforms, libstdc++ on Linux), fixing undefined C++ ABI symbols at link time.
- **Zig package upload.** The Zig package tarball is uploaded via `gh release upload`, fixing the
  silent upload failure that left releases without a Zig artifact.
- **WASM OCR.** The WebAssembly Tesseract backend applies an explicit page-segmentation mode and a
  recognition deadline instead of PSM_AUTO, which could trap or hang under WASI.
- **npm integrations publish.** The n8n / LangChain / LlamaIndex node packages install with
  `npm install`: the exact `@xberg-io/xberg` rc pin only exists once the core node package publishes
  in the same run, so a committed lockfile can never pre-match it.
- **PyPI integrations publish.** Uploads skip files already on the index (`uv publish --check-url`),
  so a re-run over a partially-published set of integration packages no longer fails.
- **spring-ai Maven publish.** The integration deploy waits for the core `io.xberg:xberg` artifact to
  become resolvable on Maven Central (indexing lag) before building.
- **Homebrew bottle upload.** The bottle upload uses a token minted after the slow from-source build
  so it can't 401 on an expired App-token TTL, and a single failed platform no longer drops the
  bottle DSL for the others.
- **Docker `-core` image (#1299).** The manifest-merge is decoupled from the full build matrix, so a
  timed-out sibling build leg no longer suppresses the `-core` image publish.

### Added

- **Local integration packaging dry-run.** `task integrations:build` builds the publishable Python
  distributions locally to catch packaging breaks before tagging a release.

## [1.0.0-rc.36] - 2026-07-24

Republish of rc.35 (same library code — #1296 chunk-to-node linkage and #1303 base64 embeddings)
under a fresh version, because the rc.35 release only partially published: the linux Node/Python,
NuGet, and Docker channels failed on pre-existing packaging bugs while the immutable registries
(crates.io, Hex, Maven Central, RubyGems, WASM) had already shipped, burning the rc.35 number.

### Fixed

- **ONNX Runtime bundling for linux Node + Python artifacts.** The Node dockerfiles and the wheel
  build no longer drop `libonnxruntime.so.1`: the staging globs match versioned sonames (`*.so*`)
  and the Python wheel job now vendors the native closure like the other targets.
- **NuGet package size (413).** The `XbergIo.Xberg` meta-package is now a thin managed package that
  resolves natives through a `runtime.json` RID graph to the per-RID `runtime.<rid>` packages,
  instead of bundling every RID's native closure itself.
- **Docker/GPU test fixtures.** CI now pulls the `test_documents` submodule's git-LFS objects, so the
  Docker smoke test and GPU extraction tests read real documents instead of LFS pointer stubs.

## [1.0.0-rc.35] - 2026-07-23

### Added

- **Chunk-to-node linkage (#1296).** `ChunkMetadata.node_ids` is now populated with the `id` values
  of the `document.nodes[]` whose text a chunk covers, so consumers can join a chunk back to the
  structural nodes it came from. Populated on the PDF/structure path via best-effort verbatim text
  containment (deduplicated, in node-traversal order); empty for structure-less documents.

### Changed

- **Base64 embedding transport (#1303).** Hosted embedding vectors are now requested in base64,
  which liter-llm decodes ~3× faster and bit-exact than a JSON float array. Providers that ignore
  the hint and still return a float array continue to work unchanged.

## [1.0.0-rc.34] - 2026-07-23

### Added

- **Owned-buffer OCR dispatch.** `OcrBackend::process_image_owned` lets backends move image buffers
  into blocking work without an extra copy; the default implementation preserves existing backend
  behavior.
- **Configurable MCP `allowed_hosts` (#1306).** `xberg mcp --transport http` can now run behind a
  reverse proxy or ingress: supply extra `Host` values via `--allowed-host` (repeatable),
  `XBERG_MCP_ALLOWED_HOSTS`, or an `[mcp] allowed_hosts` config key. Supplied hosts extend, never
  replace, rmcp's loopback-only DNS-rebinding default, so the empty case is unchanged.

### Fixed

- **Wrapped side-by-side financial tables.** PDF table reconstruction now coalesces proven wrapped
  row runs while preserving unrelated rows and a rectangular table schema.
- **Misaligned hosted-provider embeddings (#1302).** A provider returning fewer vectors than inputs,
  or numbering them with a gap, could silently attach the wrong vector to a chunk (or leave chunks
  unembedded) and still return success. Provider indices are now validated as the contiguous `0..n`
  set and the vector count is checked against the chunk count, failing loudly instead.
- **Phantom-column false tables (#1301).** Native TATR table recognition rejected prose that the
  layout model shaped into a grid with a single populated column (a spurious empty second column);
  grids now need at least two populated columns unless `allow_single_column_tables` is set.
- **CLI Linux runtime dependency bundling (#1308).** The published Linux CLI now vendors its native
  runtime closure so it runs without host-installed shared libraries.
- **NuGet package no longer rejected as too large.** The single `XbergIo.Xberg` package bundled
  every runtime's native closure and exceeded nuget.org's upload cap. Natives are now split into
  per-RID `XbergIo.Xberg.runtime.<rid>` packages pulled in transitively via `runtime.json`, so the
  .NET package publishes again.
- **Native and Windows artifact publishing restored.** The glibc-Linux C-FFI/Go/Java/Elixir builds,
  Node NAPI Docker builds, and Windows Python wheels failed to publish; the ONNX Runtime vendoring
  scripts had lost their executable bit and the `windows-target` build omitted the `classification`
  feature that the generated bindings reference. Both are fixed so all native/binding channels build
  and publish.

## [1.0.0-rc.33] - 2026-07-23

### Added

- **Stable node ids on the document tree.** `DocumentNode.id` is now populated with a
  deterministic, content-derived id (stable across extractions), and each chunk carries
  `metadata.nodeIds` linking it back to the nodes it was derived from. Both are exposed as plain
  strings across all language bindings. (#1296)
- **Per-page chunk coordinates (`pageSpans`).** Each chunk now carries `metadata.pageSpans`, a
  list of `{page, bbox}` entries — one per page the chunk overlaps, in page order — for viewer
  highlighting. The bounding box is the union of that page's body-layer node boxes within the
  chunk (omitted when unavailable). Empty (and omitted from the wire format) when page provenance
  is unavailable. (#1295)
- **Stable table ids, headers, and opt-in anchors.** `Table` gains `tableId` (a deterministic,
  reproducible id assigned in document order across the final emitted table set) and `columns`
  (the header row, so a fragment is interpretable on its own). A new `table_anchors` extraction
  option (default off) emits a `[TABLE:{tableId}]` marker before each rendered table so consumers
  can reconcile the markdown in `content` with the structured `tables[]` entries. (#1297)
- **Configurable multi-label chunk classification.** New `ChunkClassificationConfig` (label
  `definitions`, an `LlmConfig`, `batch_size`, `max_concurrency`), set via
  `ExtractionConfig.chunk_classification`, classifies each chunk with zero, one, or many labels via
  bounded-concurrency LLM batches and writes them to `metadata.classifications`
  (`[{label, confidence?}]`). Opt-in — absent config performs no classification and makes no LLM
  calls. The chunk-level analogue of page classification; requires the `classification` feature. (#1255)
- **Automatic OCR of undecodable text layers.** Pages whose embedded text decodes mostly into the
  Unicode Private Use Area, replacement, or control-character garbage (e.g. Identity-H fonts with no
  `ToUnicode`) are now routed to OCR like scanned pages, controlled by
  `OcrQualityThresholds.min_undecodable_ratio` (default 0.5) and gated by a 64-character floor so
  occasional symbols never trip it. (#1254)

### Fixed

- **Concatenated-PDF extraction audit.** A cluster of structure/reading-order defects surfaced by an
  audit of concatenated legal PDFs: duplicate `tables[]` entries and double-counted table counts are
  deduplicated (#1288); fragmented per-row tables are stitched back into one table with a propagated
  header (#1290); bullet lists are no longer flattened into paragraphs or shredded into
  word-per-cell tables (#1301); spurious intra-word spaces in native text (`"T ower"`) are removed
  via geometry-aware span joining (#1291); markdown escaping is consistent with an
  `escape_markdown` opt-out (#1292); OCR'd pages no longer report contradictory `isBlank` (#1293);
  chunk `firstPage`/`lastPage` provenance is restored on long PDFs (#1294); and heading context no
  longer leaks across document boundaries (#1289).
- **Container users can enable page classification** via a `classification` CLI feature carried into
  the `all`/container builds. (#1298)
- **CLI batch resource limits and benchmark isolation.** `extract` and `batch` now support
  `--no-config-discovery`, and their Tokio runtime honors `--max-threads`. The benchmark harness
  uses both controls, records batch subprocess overhead from the reported batch total, and keeps
  user configuration and verbose PDF logs from contaminating measurements.
- **Linux glibc artifacts no longer require a newer glibc than they advertise.** The prebuilt ONNX Runtime that was statically linked into the Python wheel and the Go/C FFI, Java, and Elixir artifacts is compiled against glibc ≥ 2.38 and pulled unversioned `__isoc23_*` and `__libc_single_threaded` symbols into the binary. Because the manylinux tag only reflects *versioned* glibc requirements, these installed everywhere but crashed at import/load on glibc 2.28–2.37 (`undefined symbol: __isoc23_strtoll`). They now link Microsoft's ONNX Runtime (glibc-clean, floors at 2.27) — the same runtime the C#, Node, and Docker builds already use — and ship it beside the binary, so the symbols are gone and the floor is honest. A CI check now fails any glibc artifact that references too-new symbols or omits the bundled runtime. musl, macOS, and Windows are unaffected.

### Changed

- **Dependencies refreshed across all languages.** Rust, Python, Node, Ruby, Java, and Dart
  dependency versions were bumped to their latest compatible releases.

## [1.0.0-rc.31] - 2026-07-21

### Added

- **Sibling-library config passthrough for liter-llm, tree-sitter, and html-to-markdown.**
  Configuration surfaces that previously lived only inside the wrapped crates are now exposed on
  Xberg's own config types and forwarded across every binding: `LlmConfig` gains `load_env` and
  `headers` (wired into client creation), `TreeSitterProcessConfig` gains `data_extraction` — with a
  structured `CodeDataNode` tree (`CodeDataNode` / `CodeDataAttribute` / `CodeDataNodeKind`) attached
  to `CodeMetadata.data` — and the HTML→Markdown pipeline's `html_options` is no longer skipped. Dead
  and redundant per-language binding excludes were pruned in the same pass (alef 0.42.0).
- **PDF headings recovered from document outlines.** Bookmark/outline entries are resolved to their
  target locations and promoted to headings in the structured document, so PDFs that carry their
  table of contents in the outline tree rather than in styled body text now produce a real heading
  hierarchy. Outline destination resolution was also hardened against malformed or dangling targets.
- **Layout regions preserved through structured PDF assembly.** Detected layout regions now flow
  through the reading-order and page-assembly stages into the structured document instead of being
  discarded after detection, improving block segmentation and reading order in the assembled output.
- **Pure-Rust layout detection on no-ORT targets (`layout-tract`).** RT-DETR layout detection and
  the PP-LCNet wired/wireless table classifier can now run through the `tract` engine instead of
  ONNX Runtime, mirroring the `auto-rotate-tract` pattern. A new `layout-tract` feature is the
  no-ORT sibling of `layout-detection`, and `android-target` now enables it (the x86_64 emulator
  previously had no layout detection at all). A new `layout_detection` build cfg (true for either
  engine variant) lets engine-neutral capability sites avoid enumerating both features; ORT-only
  plumbing (the shared ORT session builder, YOLO, TATR, SLANeXT, PP-DocLayout-V3, and the
  `LayoutError::Ort` variant) stays gated on the literal `layout-detection` feature, since table
  STRUCTURE recognition (TATR/SLANeXT) and PP-DocLayout-V3 (a tract 0.23.4 `LayerNormalization`
  op-translation bug — see the Pure-Rust Inference concept doc) are not available under tract;
  `LayoutEngine::from_config` returns a `LayoutError` for those instead of failing to compile or
  panicking. The ORT-backed `layout-detection` remains the native default. With the `pdf` feature
  (which `android-target` enables), `TableClassifier` (wired/wireless) is part of the public
  `crate::layout` API on either engine. Wiring `layout-tract` into the PDF table-structure pipeline
  remains a deferred follow-up. Part of #1275.
- **Layout detection and document-orientation on WebAssembly (via `tract`).** `wasm-target` now
  enables `layout-tract` + `auto-rotate-tract`, so RT-DETR layout detection and PP-LCNet
  document-orientation run in the browser through the pure-Rust `tract` engine — capabilities that
  were impossible on WASM while inference required ONNX Runtime. Two new `xberg-wasm` exports,
  `detectLayout(imageBytes, modelBytes)` and `detectOrientation(imageBytes, modelBytes)`, take the
  `.onnx` weights as bytes: the JS host fetches them (weights are never embedded — RT-DETR alone
  would blow the CDN per-file cap) and hands them to the seam's `load_from_memory`, its first
  production consumer. The release `.wasm` stays under jsdelivr's 50 MB cap (`wasm-opt -Oz`, wired
  through alef's `[crates.wasm].wasm_opt`). Part of #1275.
- **Windows bindings gain the full ONNX Runtime ML surface (issue #1276).** `windows-target` now
  enables `auto-rotate`, `layout-detection`, `paddle-ocr`, `embeddings`, `reranker`,
  `sparse-embeddings`, `late-interaction`, `transcription`, and `ner-onnx` (previously curated out).
  Windows already links ONNX Runtime for the python/node/CLI paths via `ort-bundled` (pyke ships a
  win-x64 prebuilt), so the Go/Java/C# FFI bindings now expose layout, OCR, embeddings, reranking,
  transcription, and NER on Windows too. `heic` stays excluded (libheif is not on the
  `windows-latest` runner).

### Changed

- **Layout-model inference errors are engine-neutral.** Layout models on the `crate::inference`
  seam (RT-DETR, PP-DocLayout-V3, table classifier) now surface seam load/run failures as a new
  `LayoutError::Inference(String)` variant instead of funnelling them through `LayoutError::Ort`,
  so they no longer name ONNX Runtime's error type at the seam boundary — a prerequisite for
  running layout off ORT. Two `.expect()` panics in the layout preprocessing paths were replaced
  with `Result` propagation, and the tract boundary tensor conversions plus `default_backend()`
  selection gained offline (model-free) test coverage. Part of #1275.

### Fixed

- **VLM OCR no longer times out at ~60s (#1273).** An unset `vlm_config.timeout_secs` inherited
  liter-llm's built-in 60s client timeout, too short for full-page VLM transcription, so extractions
  failed around the one-minute mark. The VLM OCR path now applies a 300s default when
  `timeout_secs` is unset; explicit values still win.
- **EML bodies in legacy multi-byte charsets no longer decode to replacement characters (#1278).**
  CJK encodings such as `ks_c_5601-1987`/EUC-KR, Shift-JIS, Big5, and GBK rendered as U+FFFD because
  mail-parser gates its multi-byte decoders behind the `full_encoding` feature, which xberg did not
  enable. That feature is now on, and the standalone `email` extractor pulls in the `html` path it
  renders bodies through.
- **C# NuGet musl natives no longer fail with `DllNotFoundException` (#1280).** The musl FFI build
  shipped `libxberg_ffi.so` alone, so .NET could not resolve its `libonnxruntime.so` at runtime. The
  vendored shared-library closure (ONNX Runtime + image codecs) is now bundled beside the FFI library
  with `RUNPATH=$ORIGIN`, and every `*.so` from the build output is shipped in publish staging.
- **PDF structured output preserves structure and region boundaries through page assembly.**
  Reading order is now selected per page, structure role boundaries and semantic region boundaries
  survive assembly, explicit groups close correctly beneath headings, dense numeric tables and inline
  style runs are recovered, native text is suppressed only for the tables actually emitted, mixed
  native/OCR text is preserved, and word compounds survive the text-repair pass.
- **OCR native-backend runtime enabled; CLI layout timing gated by feature.** The native OCR backend
  now has its runtime dependency wired in the feature graph, and the CLI's layout-timing output is
  gated by the layout feature so builds without it stay clean.
- **WASM builds exclude host-only async paths.** tokio OS primitives, rayon, and `JoinSet` code
  paths are now excluded from `wasm32`, keeping the wasm target buildable and free of unsupported
  runtime imports.
- **Hugging Face model cache keys are consistent and correctly gated** across the ML feature set,
  avoiding divergent or unused cache paths.

## [1.0.0-rc.30] - 2026-07-20

### Added

- **In-crate Rust tests for the wasm engine.** The `XbergEngine` bridge surface (constructor
  validation, OCR/NER dispatch through injected JS backends, wire-shape handling, bridge
  timeout) is now covered by `#[wasm_bindgen_test]` suites inside `xberg-wasm` itself, run
  under Node via `scripts/ci/wasm/run-crate-tests.sh` in the wasm e2e job. The generated
  manifest carries `wasm-bindgen-test` as a dev-dependency through alef's
  `extra_dev_dependencies`, and `test-shims/` supplies the `env` / `wasi_snapshot_preview1`
  stub modules the test glue needs for the same reason the published package needs
  `fix-wasi-imports.mjs`. The vitest e2e suites keep covering the JS side of the contract.

- **Pure-Rust document orientation on no-ORT targets (`auto-rotate-tract`).** The PP-LCNet
  auto-rotate classifier can now run through the `tract` engine instead of ONNX Runtime, so
  page-orientation detection works where native ORT cannot link. A new `auto-rotate-tract` feature
  mirrors `auto-rotate` but selects the pure-Rust backend, and `android-target` now enables it
  (the x86_64 emulator previously had no orientation detection at all). The ORT-backed `auto-rotate`
  remains the native default; the engine is chosen at compile time by the inference seam, and tract
  matches ORT within 1e-3 on the classifier logits. WASM support (embedded-weight loading) follows in
  a later phase. Part of #1275.

- **Layout detectors on the engine-neutral inference seam.** The RT-DETR and PP-DocLayout-V3 layout
  models now load and run through the `crate::inference` seam instead of holding a bare
  `ort::Session`, making layout model execution engine-neutral (the prerequisite for running layout
  off ONNX Runtime). ONNX Runtime stays the native default and its output is unchanged (pure
  refactor). RT-DETR additionally runs on the pure-Rust `tract` engine — a new parity test asserts
  tract tracks ORT within 5e-3 on every RT-DETR output, and the seam now materializes tract's
  symbolic-dimension (`TDim`) integer outputs (RT-DETR class labels) as `i64`. PP-DocLayout-V3 stays
  ORT-only under tract pending input-fact pinning (see the Pure-Rust Inference concept doc). Part of #1275.

- **Reversible redaction for authorized callers (`redaction-rehydrate`).** Token-replacement
  redaction can now capture a token to original-text map, encrypt it with a passphrase
  (AES-256-GCM, scrypt-derived key, fresh salt and nonce per encryption), and later search or
  selectively delete subjects from it: `find_subject` matches a token exactly or an original
  value by substring, and `forget_subject` removes every matching entry and returns what was
  removed so the caller can re-encrypt the remainder. The map never touches disk inside xberg.

- **In-binary GLiNER2 NER on Candle (`ner-candle`).** A new pure-Rust backend runs GLiNER2
  inference through Candle with no ONNX Runtime dependency: DeBERTa-v2 encoder, span scoring
  heads, runtime PEFT LoRA adapter merge-at-load, and a streaming safetensors loader that
  converts F32 to F16 from raw bytes. Enable `ner-candle` and construct
  `text::ner::candle::CandleBackend` from a model directory; the redaction pipeline and the NER
  post-processor consume it through the existing `NerBackend` trait.

- **Per-line OCR geometry through the wasm engine.** The wasm package gains a hand-written
  engine layer: `XbergEngine`, constructed with a config and an injection object, exposing
  `extract()`, `ocr()`, and `ner()` to JS hosts. An injected OCR backend returns
  `{ text, lines: [{ text, confidence, bbox? }] }` so a UI can highlight and overlay real
  per-line geometry instead of a flat string; a backend without geometry degrades to an empty
  `lines` array while `text` stays usable. NER works the same way through an injected
  `ner(text, categories)` backend. All bridge calls run under a configurable timeout.

- **Scanned PDFs are now detectable, and can be OCR'd without forcing OCR on the whole file.**
  PDF metadata gains `scanned_confidence` (0.0–1.0) and `scanned_pages`, so you can tell whether a
  document is a scan before deciding how to extract it. The new `ocr_strategy` config selects which
  pages get OCR'd: the default `auto` keeps today's behaviour, while
  `scanned_pages { min_confidence }` also OCRs pages that look like scans and leaves the rest on
  native text. On the CLI this is `--ocr-scanned-pages [--scanned-min-confidence 0.7]`.

  This closes a gap that had no workaround. Consumer scanners embed an invisible OCR text layer
  whose words are well-formed but wrong (`Tbe` for `The`, `rn` for `m`, `l2,45O.OO` for `12,450.00`).
  Every quality check xberg applied to a native text layer was lexical, so that text passed and
  OCR never ran; the only escape was `force_ocr`, which degrades clean PDFs. Detection instead asks
  whether the page is a picture of a document — how much of it is raster, whether its text layer is
  hidden or absent, its image codec, and the producer. A born-digital slide with a full-bleed
  background image scores 0.50 and is never OCR'd by default.

  Detection cannot judge whether a scanner's text is *accurate*, only that a scanner produced it, so
  a page carrying a good hidden text layer is OCR'd too.

### Removed

- **`HierarchyConfig.ocr_coverage_threshold`.** The field promised to "trigger OCR if less than 50%
  of page has text" and was documented that way in every language binding, but nothing ever read it —
  setting it did nothing. Its only test had been disabled since the alef migration. `ocr_strategy`
  now provides the behaviour it advertised. Existing config files that still set the key keep loading;
  the key is ignored.

### Fixed

- **Hugging Face models now share the standard Hub cache instead of creating Xberg-specific
  copies.** Dense, sparse, late-interaction, static, and reranking models, Whisper exports, and
  pretrained chunking tokenizers resolve revision-pinned snapshot paths through hf-hub. Explicit
  cache directories are alternate Hugging Face roots, both offline environment variables are
  honored, and checksum-backed artifacts are repaired under bounded cross-process locks.
- **PDF benchmark runs now fail closed and compare equivalent execution modes.** Xberg JSON batches
  use bounded concurrent extraction, both Xberg and LiteParse receive the same worker limit, OCR and
  heuristic cohorts are validated separately, missing ground truth or partial failures abort the
  run, and throughput excludes warmup and adapter-only staging overhead.
- **Layout models now use the standard Hugging Face cache directly and repair corrupt artifacts
  safely.** Downloads are revision-pinned and integrity-checked, offline mode never attempts the
  network, and bounded cross-process locks prevent duplicate or destructive repairs across Xberg
  processes while preserving concurrent Hugging Face publications.
- **Heron and TATR layout inputs now match their official preprocessing contracts.** Heron uses
  exact 640×640 bilinear RGB scaling without ImageNet normalization and reports source image
  dimensions to the model; TATR follows Hugging Face's shortest-edge and longest-edge resize
  truncation, capped at 1000 pixels.
- **PDF OCR now reuses layout rasters and detections without duplicating tables.** Layout rendering
  and inference run off the async executor, rendered pages transfer into OCR without another PDF
  render, failed pages degrade independently, and direct or inherited page rotation stays in the
  displayed coordinate frame. OCR-produced tables replace native tables for full-document OCR and
  are inserted into the structured document exactly once.
- **Layout detection with reading order no longer crashes pages whose text contains bullets,
  curly quotes, or other multibyte characters.** Reading-order reordering rebuilds the extracted
  text but kept the page boundaries computed against the original string, so downstream code
  sliced the new text at stale byte offsets — a panic whenever an offset landed inside a
  multibyte character, silently dropping the whole document. On OCR-heavy corpora this lost the
  majority of pages with layout detection on. Boundaries are now recomputed against the reordered
  text (including the copy used for chunk page ranges), and the per-page OCR gate and OCR/native
  merge skip-and-log invalid boundaries instead of panicking. The rebuilt text also keeps
  `insert_page_markers` markers, which reading-order reordering used to drop.
- **macOS wheels and the npm darwin package now target macOS 11, instead of only macOS 15.**
  Wheels were built with a deployment target of 15.0, so pip and uv matched no wheel below
  macOS 15 and silently fell back to compiling the Rust sdist; the npm darwin package vendored
  the same Homebrew libheif closure, compiled for the runner's macOS 15. Both artifacts now
  bundle a libheif decode stack built from source at the 11.0 target — as the Linux wheels
  already do inside the manylinux container — and CI fails if any bundled library misses that
  floor or is sourced from Homebrew.
- **The Intel CLI tarball's ONNX Runtime now runs on macOS 13.4+ and is a single file.** It
  previously vendored Homebrew's unpinned onnxruntime bottle (macOS 14 floor) plus its
  abseil/onnx/protobuf/re2 closure. It now ships Microsoft's official 1.23.2 build, the last
  x86_64 macOS release, which links only system frameworks.
- **XML entities (`&amp;`, `&lt;`, `&gt;`) no longer disappear from extracted text.** quick-xml
  0.38 started delivering entity and character references as separate events instead of inlining
  them in text, and every streaming reader that only matched text events silently dropped them —
  `Falafel & Hummus <combo>` in a DOCX came out as `Falafel  Hummus combo`. Affected formats:
  DOCX (body, tables, headers/footers, footnotes/endnotes, math), DocBook, JATS, and generic
  XML/SVG. Text fragments are now coalesced with their resolved references before any
  whitespace handling, so entities survive with correct spacing (`5>3` stays `5>3`, not `5 > 3`).
- **Markdown, CSV, and other text members inside an archive are no longer flattened to escaped
  prose.** Recursive archive extraction resolved each member's MIME by content sniffing first, but
  markdown/CSV/YAML are all plain UTF-8 and sniff to `text/plain` — so a zipped `.md` reached the
  plain-text extractor, which escaped its headings (`# Title` -> `\# Title`) and dropped its
  structure. Member detection now prefers the file extension whenever the sniff is only the generic
  `text/plain`, matching how the top-level path resolves a file by `detect_mime_type(path)` first,
  so a `.md` member routes to the markdown extractor. Concrete binary formats (PDF/DOCX/images),
  which sniff to a specific type, are unaffected.
- **Model downloads can no longer hang the extraction pipeline on a blocked network.** hf-hub
  builds its ureq agent with no read/connect timeout, so a firewalled or stalled HuggingFace
  connection made the blocking `ApiRepo::get()` block forever — wedging the whole pipeline at 0%
  CPU with no error. Every model-file fetch (embeddings, reranker, transcription, PaddleOCR/layout
  managers, and the Candle TrOCR / GLM-OCR backends) now runs under a wall-clock watchdog: on
  expiry it logs a warning and returns an error so the caller degrades (skips the model-backed
  backend) instead of hanging. Default ceiling 300s; override with
  `XBERG_MODEL_DOWNLOAD_TIMEOUT_SECS`.

### Added

- **Bring your own tokenizer for token-budgeted chunking.** Register a `TokenizerBackend`
  plugin (`register_tokenizer_backend`) — from Rust or any language binding — and reference
  it by name from `ChunkSizing::Tokenizer { model }`. The registry is checked before the
  HuggingFace path, so chunks are sized with the exact tokenizer the consumer's embedder
  uses (llama.cpp/GGUF vocabularies, SentencePiece models, custom vocabs). Existing
  HuggingFace-id configs behave unchanged.
- **Configurable embedding truncation length.** `EmbeddingConfig.max_sequence_length` sets how
  many tokens a chunk keeps before the tokenizer truncates it (default 512, always capped at the
  model's own `model_max_length`). Point it at a long-context model's window — e.g. 8192 for
  Jina/Nomic — so long chunks embed in full instead of only their first 512 tokens. It also
  participates in the embedding-engine cache key, so two configs that differ only in truncation
  length don't share a tokenizer.

### Changed

- **`OcrExtractionResult` now derives `Default`.** Downstream bindings and callers can
  construct and extend it without spelling out every field.

### Fixed

- **Concurrent model downloads no longer race the Hugging Face cache lock.** When two
  threads needed the same cold model at once — e.g. parallel-page OCR workers, or two
  GPU tests sharing a layout model — hf-hub errored one of them with `Lock acquisition
  failed` instead of waiting. Downloads of the same file now serialize above hf-hub, so
  the first populates the cache and the rest get the warm copy; different files still
  download in parallel.
- **Hunyuan-OCR auto-downloads its weights instead of requiring a local `model_path`.**
  The docs said the model downloads on first use, but the backend errored unless
  `backend_options.model_path` pointed at a pre-staged directory — and the checkpoint's
  Hugging Face repo was pulled upstream, so there was nowhere obvious to stage it from.
  The backend now fetches the weights on first use from Tencent's official ModelScope
  release, verifies every file against a checked-in sha256 manifest, and caches them
  under the xberg cache directory. An explicit `model_path` still takes precedence for
  offline or custom weights.
- **PaddleOCR-VL crashed on non-square pages.** The multimodal rope index built the
  vision-block position tensors with a transposed height/width layout and dropped the
  temporal row, so any image whose patch grid wasn't square failed inference with
  `cannot broadcast [1, N] to [N, M]`. Position ids now follow the reference
  Qwen2-VL layout (t constant per frame, h per grid row, w per grid column).
- **PaddleOCR-VL generation never decoded a token.** The greedy loop read the argmax
  as a rank-1 tensor where the model returns `[1, 1]`, re-fed the full sequence every
  step while the attention KV cache kept appending (duplicating keys), and the
  position-continuation branch for cached decode steps was missing, failing with a
  dtype mismatch. The loop now prefills once and feeds each new token through the
  cached decode path at its absolute position, like the other Candle OCR engines,
  and returns only newly generated tokens so the prompt is never echoed into the
  OCR output.
- **PaddleOCR-VL prefill attended bidirectionally, degenerating generation.** The
  ERNIE decoder ran the multi-token prefill without a causal mask, so every prompt
  token attended to future positions, the KV cache was built from contaminated
  hidden states, and generation collapsed into repeating a single token. The prefill
  now applies the standard additive causal mask, like the other Candle OCR decoders.
- **Hunyuan-OCR failed to load checkpoints whose `config.json` omits
  `tie_word_embeddings`.** Later revisions of the released checkpoint drop the field
  (transformers defaults it to `true`); the config parser now does the same instead
  of rejecting the whole model.
- **PDF/OCR worker-stack overflow.** The deep per-page OCR extraction futures are now
  boxed (`Box::pin`) so their large state lives on the heap instead of inflating the
  worker-thread stack frame. Together with the stack the binding runtimes provision for
  the async path, this stops scanned / image-only PDFs from aborting the process with a
  stack overflow (SIGBUS) during OCR.
- **Tesseract image OCR no longer fails on an empty language list.** `OcrConfig { language: [] }`
  joined to an empty Tesseract language string, which the native backend tried to load as a
  language pack named `""` — surfacing as the confusing `Failed to download language pack ''`.
  An empty language now defaults to English consistently across every OCR backend, matching the
  documented `OcrConfig` default. PaddleOCR results also report English in their metadata instead
  of an empty language when none is configured.
- **WASM Tesseract backend builds again.** It still treated the OCR `language` config as a single
  string after it became a list, so the WebAssembly build stopped compiling. It now uses the
  primary language (the in-memory WASI Tesseract handles one language at a time, like the PaddleOCR
  and VLM backends) and warns when more than one is requested.
- **Vertical-text (tategaki) PDF pages return their text again.** pdf_oxide's reading-order
  sort panicked on pages whose vertical-mode spans sit closer together than the median span
  width — scanned pages with vertical OCR layers, typeset tategaki books. The panic guard kept
  extraction alive, but the affected page came back as a per-page error with its text lost.
  pdf_oxide 0.3.73 fixes the sort, so those pages now extract normally.
- **Bordered tables with stroke-width-rendered rules are detected (#1213).** Some print-era PDF
  generators draw a vertical table rule as a ~1pt segment stroked with a line width equal to the
  table height, so the rule's geometric bounding box was a speck and the Lines-strategy detector
  saw no vertical rulings — whole fuse-chart-style tables were missed (their only detected "table"
  being a false-positive page footer) and their text flowed out column-major, destroying row
  associations. pdf_oxide 0.3.74 accounts for stroke width in path bounding boxes, so these grids
  are now detected natively with their rows intact.
- **Inter-word spaces are no longer dropped in positioned/tabular PDF text.** Words in
  TJ-positioned runs — the header cells of rate tables and similar tabular layouts — extracted
  glued together (`Comparisonrate`, `roadvehicles`, `transportlayer`) while the same words in
  flowing prose on the page were spaced correctly. pdf_oxide 0.3.74 accounts for the `TJ` numeric
  adjustment that carries the space in those runs, so positioned text is spaced too.
- **Redaction now scrubs every text-bearing field.** The redaction pass rewrote the main content
  and a handful of fields but left table cells, page content, form-field values, image captions,
  URIs, metadata, and structured output carrying the original text — while still reporting success.
  All of these are now redacted before the result is returned.
- **Encrypted PDFs honor the configured passwords.** `PdfConfig.passwords` had no effect, so a
  password-protected PDF came back as an empty success even with the right password supplied. Each
  configured password is now tried, and a still-locked document returns an error instead of empty text.
- **Merged table cells keep their column.** A cell following a horizontal merge (or under a vertical
  merge) shifted left into the spanning column in HTML, DOCX, and the document-structure grid,
  misaligning every following row against its headers. Cells now keep their true column position.
- **Text encoding is detected per document, not assumed UTF-8.** Latin-1 plain text and CSV no longer
  turn accented characters into replacement characters, XML honors its `encoding=` declaration, and a
  UTF-8 BOM is stripped from the first CSV header instead of being glued onto the field name.
- **Files are routed by content, not just their extension.** A misnamed file (e.g. a DOCX named
  `report.pdf`) is now detected from its bytes and sent to the correct extractor.
- **Token reduction applies to Markdown and HTML output.** The reduction was computed and then
  discarded for non-plain output formats; it now takes effect for the formatted content too.
- **Non-UTF-8 text inside archives is recovered.** Text members of zip/tar/7z archives whose bytes
  weren't valid UTF-8 were silently dropped; they are now decoded with the same detection used elsewhere.
- **OCR failures surface instead of returning empty text.** A failed or empty OCR pass no longer masks
  itself as a clean empty result, and an empty OCR result no longer wipes a page's native text; a
  `ProcessingWarning` is attached so callers can tell the page fell back.
- **Dense unruled tables are no longer dropped by the density guard.** A real reference table with many
  short-valued rows and few columns was rejected on row count alone; it is kept when its cells are
  short values, while columned prose is still rejected.
- **Language detection honors `min_confidence` and orders results deterministically.** The confidence
  threshold was silently capped, and equal-frequency languages came back in a nondeterministic order.
- **Config changes that alter output no longer serve a stale cached result.** The source name and OCR
  tessdata now participate in the cache key.
- **CSV `NaN`/`inf`/`infinity` are treated as text, not numbers**, so they no longer flip header and
  column-type detection.
- **Table diffs report shape changes.** A table whose row/column shape changed produced an
  information-free empty diff instead of showing the old table removed and the new one added.
