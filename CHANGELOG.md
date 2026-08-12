# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

---

## [Unreleased]

### Changed

- **Breaking:** `Formula.bbox` and `Formula.page` are now optional. Markup sources (DOCX, PPTX,
  ODT, EPUB, HTML, JATS, LaTeX, Markdown, and related formats) produce formulas with no geometry;
  the old required fields forced fake values on those paths (a zeroed bbox and `page: 1` on VLM
  OCR results). Layout-guided OCR still reports both. The C FFI reports an absent bbox as a null
  pointer and an absent page as `0`; JSON omits absent fields. The field docs now state the real
  coordinate space: pixels of the rendered page image at the OCR render DPI (base 150, reduced
  automatically for very large pages), not the previously claimed 300 DPI (#1385).

### Added

- Opt-in `formula-recognition` feature: layout-detected formula regions on rasterized pages (image inputs and PDF pages) are
  recognized as LaTeX by the RapidLaTeXOCR model set (MIT, pix2tex-derived; resizer + encoder +
  autoregressive decoder ONNX, ~180 MB, downloaded on demand and SHA256-verified). Enable with
  `LayoutDetectionConfig.formula_model = latex_ocr` or `--layout-formula-model latex_ocr`; the
  region's plain OCR text stays as the fallback whenever recognition yields nothing (#1385).

- `ExtractedDocument.formulas` is now populated for every format, not only layout-guided OCR.
  Formula elements produced by markup extractors are projected into the public list in reading
  order, with `$$` delimiters stripped, after any OCR-detected formulas. A new public element type
  `formula` identifies these elements in element-based output, where they previously degraded to
  `narrative_text` (#1385).
- JATS `disp-formula` and `inline-formula` content now yields LaTeX. A `tex-math` alternative is
  used verbatim when present; otherwise the `mml:math` subtree runs through the shared MathML
  converter; plain text remains the fallback. Inline formulas in the all-in-one path render as
  `$...$` (#1385).

### Fixed

- PDF OCR formulas now report their bounding boxes in PDF point coordinates (origin bottom-left),
  comparable to native PDF geometry, instead of rendered-image pixels whose DPI varied per page.
  Image inputs, and PDF pages whose geometry is unavailable, keep pixel coordinates and say so.
- The RST text path now renders `.. math::` directives inside `$$` display-math delimiters instead
  of a literal `math:` prose prefix.
- Alef now extracts Crawlberg binding types from the pinned registry dependency instead of a
  neighboring checkout, keeping generated bindings aligned with the version Cargo compiles.

### Added

- Diagram recovery: vector SVG and vector PDF sources that draw a node/edge diagram (Graphviz,
  Mermaid, PlantUML, and LibreOffice Draw are corpus-verified producers) are recovered
  deterministically from their geometry — no detection model involved — and can be rendered as
  Graphviz DOT via `output_format="dot"`. Recovery rejects drawings with no connector (charts,
  logos, illustrations) and closed regions ruled like a table, so it does not misfire on ordinary
  page content. `output_format="dot"` replaces `content` with the DOT text and yields the empty
  string when no diagram is found on the page (#579).

## [1.1.0] - 2026-08-07

### Added

- The benchmark harness now has an exact `pdf-regressions` cohort for the six unique PDFs tracked
  by #1406, with checked document paths, ground-truth references, and byte sizes. This provides a
  stable baseline/layout comparison target without weakening the existing calibrated quality floors.
- `LlmConfig` gains a `credential_provider` field for managed OAuth2/STS authentication modes
  liter-llm cannot express via a static `api_key`: Azure AD client-credentials, Google Vertex AI
  OAuth2 (service-account key file), Vertex AI Application Default Credentials, and AWS STS
  `AssumeRoleWithWebIdentity` for Bedrock (EKS IRSA). Every variant is plain data so it round-trips
  through TOML/JSON/YAML and every language binding; `Debug` redacts the Azure AD client secret,
  the only variant that carries one. Inert on `wasm32`, where the whole `llm` module is compiled
  out: the field deserializes and is then ignored, with no error. Copilot's device flow has no
  variant because it takes no configuration and drives an interactive prompt; Rust embedders can
  pass any custom provider to `create_client_with_credential_provider` directly.
- `LlmConfig` gains `reasoning_effort` and `extra_body`, the two request-time parameters liter-llm
  exposes that the config previously could not reach. `extra_body` is the generic escape hatch for
  provider-specific request fields (guardrails, safety settings, grounding configuration). Both are
  applied at every site that builds a completion request — text completion, structured extraction,
  VLM OCR and NER — so a value set in configuration cannot take effect on one path and silently do
  nothing on another. An unrecognised `reasoning_effort` is a validation error rather than a silent
  drop (#1381).
- The `ttf-parser` used when rendering PDF pages is redirected onto `xberg-ttf-parser`, a fork of
  upstream 0.25.1 carrying the CFF `dotsection` fix described under Fixed below plus eight other
  correctness and denial-of-service fixes that are merged upstream but unreleased. The redirect is a
  workspace-level `[patch.crates-io]` entry, routed through a small name-compatibility shim
  (`crates/ttf-parser-compat`) because Cargo matches patch entries on package name alone. No
  first-party code calls the parser directly. `[patch]` entries only apply to builds that go
  through this workspace root: a caller depending on the published `xberg` crate from their own
  `Cargo.toml` still resolves `pdf_oxide`'s and `fontdb`'s transitive `ttf-parser` to the real,
  unpatched upstream, with none of these nine fixes.
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
- `Chunk` gains `sparse_embedding` and `late_interaction` fields, populated when sparse or
  ColBERT-style multi-vector embedding generation is configured for chunking and omitted from
  the wire otherwise. Both compile on every feature combination, including default features.
- `render_heading_breadcrumb` is now public, so a consumer that wants the heading breadcrumb
  prepended to a chunk — the right shape for dense retrieval, where each chunk should be a
  self-contained passage — can render exactly the same string the chunker used to produce, instead
  of reimplementing its format string. `ChunkingConfig::breadcrumb_target` is also new, but it is
  inert and documented as such: it is retained rather than removed only because it is already
  visible in three places — the Rust field (and its binding equivalents across the generated
  packages), the CLI's `--chunk-breadcrumb-target` flag, and the `XBERG_CHUNKING_BREADCRUMB_TARGET`
  environment variable — and removing it needs a coordinated regeneration (#1393).
- New opt-in Prometheus `/metrics` endpoint for the API server, backed by a real
  `opentelemetry_sdk` meter provider instead of the OTel no-op meter, so metrics registered
  anywhere in the process are actually exported when scraped (#1391).
- `LlmConfig` gains a `bedrock` table (region, cross-region routing prefix, and explicit
  credentials) for `bedrock/`-prefixed models. Credentials are never printed: `Debug` on
  `LlmConfig` and `BedrockConfig` redacts every credential field.
- New `CsvOptions` config lets callers set an explicit delimiter and comment-line prefixes for
  CSV/TSV extraction instead of relying solely on auto-detection.
- DOCX comments parsed from `word/comments.xml` now get their own `Comment` element kind when
  joined to the body, instead of being folded into surrounding content.
- The `xberg tree-sitter` subcommand and its `download`, `list`, `clean` and `cache-dir` children
  are reachable from the command line for the first time. The module implementing them was never
  declared, so it had never been compiled and none of it could be invoked. It also gains
  `--from-config`, which loads `cache_dir`, `languages` and `groups` from the resolved Xberg
  configuration instead of requiring them all as arguments, and `cache_dir` is now honored at
  extraction time as well; command-line arguments still win over file configuration.
- Dense-table and other complex-layout PDF regions can now be routed to a configured VLM for
  extraction instead of only the native/OCR pipeline.
- Language detection results (`LanguageConfidence`) now report per-language confidence, document
  proportion, and script, not just a single detected language.
- `code_intelligence` is now populated from `tree_sitter_language_pack`'s `ProcessResult` for
  code extraction instead of always being `None`.
- PDF annotations preserve their real subtype (highlight, underline, strikeout, squiggly, link,
  stamp, text/free text, etc.) instead of being collapsed into one generic type, and are now
  rendered onto the page.
- Applications can now capture `pdf_oxide`'s glyph-drop warnings as a `ProcessingWarning` when its
  rasterizer silently drops a glyph it cannot paint, instead of the gap going unreported (#1364).
- The batch-level extraction counter and duration histogram are now emitted, closing four metrics
  that were declared but never recorded.
- Presentation MathML in ODT, ODP and EPUB documents is now converted to LaTeX through a shared
  converter modelled on the existing OMML (DOCX) one. EPUB discarded every `<math>` subtree
  outright, and ODT/ODP emitted a plain-text approximation (`num/den`, `base^exp`) that was still
  wrapped in `$$...$$` delimiters and therefore was not valid LaTeX. This also fixes entity
  double-rendering, where a `<mo>` element followed by an XML comment emitted its content twice.
- VLM OCR now extracts LaTeX formulas. The prompt never mentioned mathematics and the backend never
  populated `formulas`, so equations came back as prose and were dropped from the structured result.
- Classical PaddleOCR — DBNet detection, CRNN recognition and the AngleNet orientation classifier —
  can now run on the pure-Rust tract backend via the new `paddle-ocr-tract` feature, so it works on
  `wasm32` and the Android x86_64 emulator, where ONNX Runtime cannot link. `paddle-ocr-ort` carries
  the previous ONNX Runtime implementation and `paddle-ocr` remains an alias for it, so `full`,
  `server` and `windows-target` builds are unaffected. TATR, PP-DocLayout-V3 and SLANeXt remain
  ONNX-Runtime-only.
- AsciiDoc and WebVTT now have real structural extractors. The plain-text extractor claimed both
  MIME types and did nothing but strip a byte-order mark and split on blank lines, so AsciiDoc
  titles and tables became prose and VTT timestamps became body noise.
- `ContentFilterConfig` gains `include_footnotes` (default `false`, preserving current behaviour),
  so a footnote-classed paragraph that was classified as page furniture can be recovered. It mirrors
  the existing `include_headers` and `include_footers` knobs.
- `show_download_progress` is honored on all four model configurations (embedding, sparse embedding,
  reranker and late interaction). The field existed on each of them and had no reader anywhere, so
  the CLI's `embed` command set it expecting progress output and got none.
- The `DocSecurity` protection flags of DOCX, XLSX and PPTX documents are decoded into named flags
  on `Metadata::additional`, so a caller can tell a password-protected or read-only-recommended
  document from an unrestricted one. The field was parsed and then discarded, and for XLSX and PPTX
  it was not read at all.
- Python wheels are now published for musllinux, so Alpine and other musl-based installs no longer
  fall back to building an sdist that requires a full Rust toolchain, and the Ruby gem gains a
  Windows x86_64 (UCRT/MinGW) build.
- The pipeline and batch-extraction tracing spans, five of the eight pipeline stage spans, and the
  extractor-priority, batch-size and batch-index span attributes are now recorded. They were
  declared in the telemetry conventions but emitted by no code path, so an operator filtering on
  them saw nothing.
- `Engine::extract` now honors an injected cache backend and progress sink, which were accepted and
  silently inert because the single-document path did not consult them. A cache hit on a bytes input
  — keyed on a content hash of the bytes plus the resolved configuration, not the path — skips
  extraction, and coarse start, complete, error and cache-hit events are emitted. The no-op cache and
  progress sink remain the defaults, so callers that inject nothing see no change.
- The `xberg extract` JSON envelope reports the kernel's peak resident set size for the process, the
  same `ru_maxrss` high-water mark other document-extraction CLIs report, so memory comparisons no
  longer depend on a sampling loop that can miss a spike.
- `finalize::merge_and_cite` and `prompts::escalate_if_below_threshold` compose the structured-output
  merge, citation and vision-fallback decision into callable units. They are public API for embedders
  that build their own structured-extraction path; the built-in pipeline still calls the simpler
  text-only structured extraction and does not route through them.
- Docling DocTags is supported in both directions: it can be emitted as an output format and read as
  an input format, with tables carried as OTSL and geometry as `<loc_*>` tokens. Emission maps the
  internal model onto the DocTags vocabulary (headings to `section_header_level_N`, header/footer/
  footnote content layers to `page_header`/`page_footer`/`footnote`, captions nested inside
  `<otsl>`/`<picture>`/`<code>`, list items wrapped in `<ordered_list>`/`<unordered_list>`). Location
  tokens are emitted only when an element has a bounding box and its page recorded dimensions; the
  vertical axis is flipped because `BoundingBox` is PDF space with a bottom-left origin while DocTags
  counts from the top-left. Ingestion registers an extractor for `text/vnd.docling.doctags` and a
  `.doctags` extension, expands the OTSL merge tokens `lcel`/`ucel`/`xcel` into the flat grid, and
  tokenizes by recognised tag name rather than by scanning to the next `>` — DocTags has no escaping
  and real Docling output carries literal `<` in prose, so a naive scan corrupts the document from
  the first caption that discusses markup. Parsed pages are rebuilt as 500-unit squares because the
  real page size is not recoverable from the stream, which makes re-emitting a parsed document
  reproduce its original tokens exactly. Emitted OTSL cannot currently produce `lcel`/`ucel`/`xcel`/
  `rhed` and merged cells come through duplicated, because `Table::cells` is a flat `Vec<Vec<String>>`
  with no span data; ingestion parses the full grammar, so only the emit direction is lossy (#1383).
- `ConversionOptions` is re-exported from the crate root under the `html` feature.
  `ExtractionConfig::html_options` and `FileConfig::html_options` are public fields of this type, so
  callers already had to name it; without the re-export they had to take a direct
  `html-to-markdown-rs` dependency and keep its version in lockstep with ours.
- The WebAssembly build now enables `pdf`, `html`, `heuristics`, `layout-types`, `transcription-types`
  and `simd-utf8`, so PDF and HTML extraction, the heuristics surface and the layout and transcription
  types are reachable from the browser bindings rather than being compiled out. These were previously
  excluded because generating them tripped two binding-generator defects, both since fixed. Expect a
  larger bundle.

### Changed

- Extracted PDF pages now reflect reading-order reordering per page instead of only in the
  joined document text, so `AUTO` and `ALWAYS` reading-order modes no longer return
  byte-identical `pages[].content`.
- A batch's worker thread cap now honors a real Linux cgroup CPU quota larger than the
  hardcoded serverless default (8) instead of clamping it down, so containers with a higher
  quota use the cores they were actually granted (#1392).
- Dependency bumps: `crawlberg` 1.1.4, `liter-llm` 1.16.0, `sceptre` 0.4.0,
  `tree-sitter-language-pack` 1.14.3.
- `LlmConfig` now passes through liter-llm's full configuration surface instead of a hand-tracked
  subset: `providers`, `cache`, `budget`, `rate_limit`, `cost_tracking`, `tracing`,
  `cooldown_secs`, and `health_check_secs`. A configured custom `providers` entry is now registered
  with liter-llm before the client is built — it previously round-tripped as configuration and did
  nothing, because custom providers only take effect through a separate registration call that was
  made nowhere, so a provider defined in TOML had no effect and reported no error. A provider that
  fails to register is now an error rather than a silent no-op. `cache`, `budget`, `rate_limit`,
  `cost_tracking`, `tracing`, `cooldown_secs`, and `health_check_secs` only take effect when
  liter-llm's `tower` feature is compiled in; otherwise they are accepted but unused.
- The OpenAPI document now declares the `415 Unsupported Media Type` response on `/extract`,
  `/extract-async`, and `/cache/warm`, and the `429 Too Many Requests` response on
  `/extract-async`. These responses were already possible at runtime; clients generated from the
  spec previously had no typed way to handle them.
- **Breaking (Rust API):** `EmbeddingModelType::Llm` and `RerankerModelType::Llm` now hold
  `Box<LlmConfig>` instead of `LlmConfig`. Code that constructs either variant directly in Rust
  must wrap the config in `Box::new(..)`; `match` arms are unaffected.
- **Breaking (behaviour):** a chunk's `content` now always equals the exact source span it was cut
  from. The heading breadcrumb was previously prepended into every chunk's content unconditionally,
  which is right for dense retrieval but hurts lexical indexes such as BM25 and TF-IDF: every chunk
  under a heading repeated those tokens, so the term's document frequency equalled the number of
  chunks in the section and its inverse document frequency collapsed toward zero. It also made
  `content.len()` disagree with `byte_end - byte_start`, so slicing the source by a chunk's own
  offsets returned a different string than the chunk carried, and `token_count` described text other
  than the text stored beside it. `heading_path` is populated either way, and consumers that want
  the old shape can call `render_heading_breadcrumb` themselves (#1393). `prepend_heading_context`
  is likewise deprecated and now inert: setting it no longer changes `content`. Both fields, along
  with the CLI's `--chunk-breadcrumb-target` and the `XBERG_CHUNKING_BREADCRUMB_TARGET` environment
  variable, are retained only so existing callers keep compiling.
- **Breaking (HTTP API):** an unrecognised multipart field name on the extraction endpoints is now
  rejected instead of being silently ignored. A misspelled field previously fell through a catch-all,
  was dropped, and the request quietly ran against the server defaults, which reads as the setting
  having no effect. Requests that relied on that behaviour now fail.
- `xberg extract` in text mode writes the extraction envelope — processing warnings, timings and the
  remaining envelope fields — to stderr instead of discarding it, so stdout stays exactly the
  extracted document and remains pipeable. `--format toon` now carries the same timing and
  peak-memory fields JSON consumers already received, and `xberg formats` resolves against the
  extractor registry rather than the core's ungated static catalogue, so it no longer advertises
  formats the binary would reject.
- An OpenDocument file whose ZIP container has no `content.xml` is now an extraction error. It
  previously returned an empty document as a success, which a caller could not tell from a document
  that genuinely has no content.
- WebVTT cue timings are now optional rather than always present, so a block with no timing line
  cannot fabricate a `00:00:00.000` start and end. It emits no timing attributes at all, and the cue
  count still counts only genuinely timed cues.
- Behind the `heuristics` feature, the text-coverage signal that feeds `extraction_confidence`
  is now measured from the document instead of hardcoded to `1.0`: for page-addressable formats
  it is the fraction of pages with non-blank content, and for formats without a page breakdown it
  is `1.0` when `content` has any non-whitespace text and `0.0` otherwise. `extraction_confidence`
  itself was already computed from `score_confidence` over several signals; only this one input
  signal was fixed.

### Removed

- **Breaking (bindings):** `ExtractedDocument.formatted_content` / `formattedContent` is no longer
  exposed by any language binding (Python, Node, Ruby, PHP, Go, Java, C#, Elixir, Dart, Swift,
  Kotlin, Zig, WASM, C FFI), and the C symbol `xberg_extracted_document_formatted_content` is gone.
  The field is pipeline-internal scratch space: `apply_output_format` moves it into `content` as the
  last pipeline step, so every document a binding could ever observe carried `null` there. Read the
  rendering from `content` — it is already in the configured `output_format`. The field remains
  `pub` on the Rust type, where extractor and post-processor plugins legitimately use it.
- The troff, mdoc, POD and DokuWiki MIME types are no longer advertised as supported. They were
  claimed by the plain-text extractor, which produced silently wrong output; each needs a full macro
  or host-language parser, and an honest rejection is better than plausible-looking wrong text.
- OCR results no longer carry `script_name` and `script_confidence`. The values were fabricated
  rather than detected, so they are no longer produced at all.
- `LanguageRegistry` is removed. It was exported from the crate root but its entire implementation,
  its `Default`, its global and both of its backend data sources were test-only, so it was a public
  type with no public constructor and no production caller.
- Fourteen unused tree-sitter re-exports are removed from the crate root. Each was checked against
  the binding inclusion tables, the benchmarks and the CI feature legs and had no reference anywhere;
  the nine that are genuinely reachable are kept.
- The `wasm-threads` feature is removed. It activated `wasm-bindgen-rayon` with no corresponding
  conditional compilation anywhere, so it compiled a dependency and changed nothing.

### Security

- `biblib` moves from the exact pin `=0.4.3` to `0.8`, taking the citation parser off `quick-xml`
  0.37 and onto 0.41. That clears RUSTSEC-2026-0194 (quadratic duplicate-attribute checking) and
  RUSTSEC-2026-0195 (unbounded namespace-declaration allocation), both scored 7.5, which reached
  every downstream consumer of the `office` feature. The old pin existed because biblib 0.4.4 called
  a `quick-xml` API that had been removed; 0.8 no longer does. Its `regex` feature is gone —
  matching is now unconditional `regex-lite` — and its RIS parser keeps records that earlier
  versions rejected, so RIS input that used to fail to parse now yields a citation.
- Zip-bomb accounting no longer overflows or skips entries. Declared entry sizes are read from the
  central directory, where a ZIP64 extended field can carry a full eight-byte value, and were summed
  with an unchecked addition, so two crafted entries could wrap the accumulator to zero and pass a
  cap of 18 exabytes — or panic outright in a debug build. Entries whose compression method the
  reader rejected were also dropped from the totals while validation still returned success. The
  depth budget additionally took the looser of the XML-depth and nesting-depth limits, so tightening
  either one had no effect.
- The configured cache namespace is validated before any directory is created. It reached
  `create_dir_all` unchecked, so a traversing or absolute value wrote outside the cache root. It is
  now allowlisted at both boundaries. Cache entries also carried no build fingerprint, so an entry
  written by a different build was served after extraction behaviour had changed; the key now embeds
  a hash over the package version and a schema version.
- Redaction no longer reports personally identifying information as redacted that it did not remove.
  LLM-detected entities were resolved by a first-match search, so only the first mention of a
  repeated name was replaced; findings were recorded before the span was checked to be applicable, so
  skipped spans still counted as redacted; detections were applied to `content` alone, leaving the
  same name intact in metadata, chunks, pages, formulas, revisions, document structure, format
  metadata and nested archive members; custom labels were dropped by the category filter, and
  requesting only custom labels skipped the detection backend entirely; and the audit total counted
  findings in `content` only. One pass now walks every text-bearing field and records a finding only
  once the replacement is provably applicable.
- The public `elements` field no longer carries pre-post-processing text. The element tree was
  snapshotted before the Early, Middle and Late post-processors, token reduction and Unicode
  normalisation ran, and then never updated, while the renderer preferred that tree over `content` —
  so with redaction configured, `elements` and the copy handed to foreign renderers held unredacted
  text. The tree is now discarded when `content` diverges from what it stands for.

### Fixed

- PDF Markdown and Djot extraction now falls back to complete native text when the structured
  hierarchy retains less than 70% of native tokens, including tokens represented in table cells.
  Repeating-text cleanup is also limited to unpositioned fallback content, preserving semantic form
  fields and restoring the six quality regressions tracked by #1406 without lowering quality floors.
- Rotated PDF text now keeps its text-matrix rotation through hierarchy extraction and performs
  ordering, line/paragraph grouping, spacing, and gap detection in the run's upright frame. This
  restores natural order for sideways tables while keeping mixed upright page furniture separate
  and leaving rotation-zero extraction unchanged (#1358).
- Tesseract source caches now require a valid source-tree marker instead of trusting directory
  existence alone. Incomplete Leptonica or Tesseract trees are removed and downloaded again for
  both native and WebAssembly builds, with WebAssembly patches reapplied after recovery (#1401).
- The crate compiles cleanly under feature sets that omit `api-types`, the WebAssembly build among
  them. The server-boundary validators (listen host, port, CORS origin, upload size) and their
  constants were unconditionally compiled but their only consumer, `ServerConfig::validate`, is gated
  on `api-types`, so every one of them was dead code and any build with `-D warnings` failed. They
  now carry the same gate as their consumer, and their tests moved alongside them.
- Two-column PDF reading order is now detected per horizontal band instead of per page. Page
  furniture (running headers/footers, titles, rules) used to make the whole page look
  single-column to the repair heuristic, so genuinely two-column pages kept their columns
  interleaved line-by-line; furniture also now emits at its true position between bands rather
  than between the two columns.
- The dense two-column band-split reading-order repair now also runs on the element path used to
  build page structure (headings, paragraphs, tables), not only on the plain-text path. A
  two-column page could previously come out with an identical, still-interleaved order whether
  `ColumnAware` or `TopToBottom` reading order was selected, because `pdf_oxide`'s own `ColumnAware`
  XY-Cut pass ran unconditionally on the element path and the same-order repair never got a chance
  to run first (#1397).
- A rule-less PDF table candidate is now rejected as prose when a word crosses the same column
  boundary on more than 60% of its (column boundary, row) pairs, catching multi-column prose the
  geometric gate alone let through. This check only ever applies when the region has no drawn
  ruling lines; a page with ruling lines is still admitted on that stronger signal, which remains
  the load-bearing check (#1399).
- The Python and PHP bindings' `StructuredDataResult` gain the `value` and `flattened` fields already
  present in the other language bindings, restoring parity after a binding regeneration gap.
- The Dart package passes `dart analyze` again: the generated `xberg.dart` exported `traits.dart`
  without importing it, leaving every plugin-trait doc reference unresolvable, and also carried an
  unused `dart:typed_data` import; `bin/download_libs.dart` reached into `lib/` by relative path.
- DOCX documents with many legacy VML `w:pict` picture elements are no longer falsely rejected for
  exceeding the nesting-depth limit: each `w:pict` was leaking one extra level of nesting budget,
  and content inside it was also exempt from the iteration cap.
- Archive, AsciiDoc, VTT, and XML extraction now report when a decode lost bytes to a U+FFFD
  replacement, instead of silently returning mangled text. The XML declared-encoding path also
  gained the mojibake repair already applied by every other extractor.
- Image OCR benchmarks now score structural F1 only against genuinely structured Markdown ground
  truth; scene-text fixtures remain text-only, and a dedicated structured image cohort covers
  receipts, document pages, tables, and invoices.
- Benchmark adapters now honor fixture OCR languages, partition batch-global backends into
  homogeneous native batches, and record unsupported-language exclusions in provenance instead of
  silently evaluating non-English documents with default English models.
- PDF pages that embed a CFF font converted from Type 1 no longer lose their dot-bearing glyphs.
  The font parser rejected the deprecated `dotsection` operator and discarded the entire glyph, so
  every `i`, `j`, `!` and `.` on the page rendered as blank space, and OCR run over those pages
  transcribed the gaps. The parser now ignores the operator, matching FreeType and read-fonts. This
  fix ships via the `xberg-ttf-parser` patch described under Added above, so it applies only to
  builds through this workspace root; a consumer of the published `xberg` crate still hits the
  unpatched upstream parser and the original bug.
- A malformed embedded font can no longer make PDF rendering hang. Composite glyph outlining and
  COLRv1 colour painting both bounded only how deeply they recursed, not how much total work a
  crafted font could force, so a glyph whose components all point at one shared child could drive
  exponential work. Both now carry a total visit budget.
- Fonts at the maximum 65535 glyphs now parse. The glyph offset table needs 65536 entries at that
  size, which overflowed a counter and dropped the table, leaving the font with no outlines at all.
- DOCX extraction no longer truncates tables nested inside other constructs. `w:tblPr`,
  `w:tblGrid`, `w:trPr`, `w:tcPr`, `w:drawing`, `w:sectPr`, and several OMML branches each
  consumed their own closing tag without releasing the `SecurityLimits` nesting-depth budget
  they had claimed, so the leaked depth eventually tripped the limit and cut extraction short
  partway through a document's tables (#1395).
- Consecutive numbered subsection headings in PDF structure detection (e.g. `1.1`, `1.2`, `1.3`)
  are no longer merged into a single paragraph, while prose that happens to start with a bare
  year or a Roman-numeral/ALL-CAPS heading still merges correctly (#1386).
- The native `xberg-ffi` build (desktop/server Linux and macOS-arm64, used by the C, C#, Go, and
  Java bindings) no longer silently drops excel, hwp, hwpx, iwork, wordperfect, mdx, xml, and
  QR-code support while still advertising those formats as supported. The advertised format
  catalogue is now filtered through the extractor registry actually compiled into the binary, so
  a feature-flag gap can no longer make a binding claim a format it cannot extract (#1387).
- Windows MSVC builds linking `xberg` no longer fail with `LNK2038: RuntimeLibrary mismatch`
  between `esaxx-rs`'s C++ static runtime (`MT_StaticRelease`) and the Tesseract capi object's
  dynamic one (`MD_DynamicRelease`). `model2vec-rs`'s tokenizer dependency now takes
  `fancy-regex` directly instead of pulling in `tokenizers/esaxx_fast`, which only accelerated
  BPE training and was never needed for inference (#1389).
- Rotated PDF text (90/180/270-degree runs, most visibly sideways tables) is now assembled along
  each run's own rotated reading axis instead of page-x order, so a rotated run's words and lines
  no longer come out glued together or out of order.
- OCR pipeline scratch metadata (`word_iterator_skipped_count`, `auto_rotate_unavailable`) no
  longer leaks into the user-visible `Metadata::additional` map. Both are consumed internally to
  produce `ProcessingWarning`s and are now stripped before the result is returned.
- Standalone and embedded image OCR preprocessing now honors the caller's
  `ImageExtractionConfig` dimension and auto-adjust limits, instead of silently falling back to
  defaults whenever a Tesseract-specific `target_dpi` was also set.
- Post-processors that rewrite `content` (redaction, summarisation, translation) no longer have
  their changes silently discarded from Markdown/Djot/HTML/JSON/Custom output.
  `formatted_content` is rendered from the extractor's element tree before post-processors run,
  then substituted into `content` at the end of the pipeline; a processor that rewrote `content`
  without also updating `formatted_content` previously had its stale, pre-processing rendering
  win — with redaction configured, the returned document could be the *unredacted* rendering. The
  stale rendering is now discarded in favor of the post-processed plain text, with a
  `ProcessingWarning` explaining the downgrade.
- The browser WASM demo's upload cap is raised to 10MB, covering the formats that carry
  meaningful content at that size without risking the demo's 30-second in-browser worker timeout;
  the library itself imposes no upload ceiling.
- `tokio::spawn(xberg::extract(..))` compiles again. The batch item future drove the compiler past
  its recursion limit proving a `Send` bound, failing with `E0275` on a proof chain made up entirely
  of third-party types, and every generated binding crate hit it too. The future is now type-erased
  at the boundary where the bound is demanded, which cuts the chain; a generated crate cannot carry
  the crate-level attribute the compiler suggests instead, because a regeneration would drop it.
- DOCX extraction recovers text boxes, reviewer comments, field results and every header and footer.
  Text inside `w:txbxContent` and VML `v:textbox` was dropped, `comments.xml` was never parsed and
  joined to its reference, headers and footers were guessed from a fixed filename loop rather than
  read from the document relationships, and content inside headers, footers and notes bypassed the
  body element loop, so tables, hyperlinks, math and fields did not work there. `HYPERLINK` field
  URLs are recovered from both field forms, and `w:sym` and `w:noBreakHyphen` map to their Unicode
  characters instead of being dropped.
- PPTX extraction recovers OMML equations as LaTeX, `mc:AlternateContent` fallback shape trees,
  connector text, chart and SmartArt text, cached field text and line breaks in document order.
  Images are paired with their own shape's dimensions and alt text by relationship id rather than by
  hash-map iteration order, which previously mis-paired them with an unrelated shape's geometry, and
  a corrupt document-properties or comment part is reported instead of yielding a document that
  looks complete.
- Legacy binary Office formats recover more content: `.doc` piece-table text is bucketed by
  subdocument, so footnotes, headers, footers, comments and text boxes are extracted rather than
  discarded at the end of the main text; a piece whose declared byte range overruns the stream warns
  and keeps the available bytes instead of silently truncating; `.ppt` text is segmented on the slide
  record rather than the slide-list record, so slide counts and per-slide grouping are correct; and
  RTF shape text survives a nested ignorable ancestor while annotations are emitted as labelled
  comments.
- OOXML application properties are sliced by their `HeadingPairs` boundaries instead of being read as
  one flat vector, so slide titles no longer begin with the theme name and worksheet names no longer
  silently include named ranges with every later index shifted. Custom properties of a further ten
  value types are read rather than dropped, co-authored documents keep every author instead of only
  the first, and embedded OLE containers are unwrapped rather than discarded as unidentifiable.
- Excel extraction surfaces cell hyperlinks, formulas, defined names and comments, records hidden
  sheets while still extracting their content, attaches embedded objects as child documents, and
  emits a warning naming any sheet whose part is missing or whose range cannot be read instead of
  dropping it from the workbook as though it never existed. Row and column truncation now names the
  sheet and the effective cap. The `excel` feature also gained the dependencies it needs, so a bare
  `--features excel` build compiles.
- ODS spreadsheets return their title, author, subject and dates. Document metadata was computed only
  for the OOXML spreadsheet extensions, even though the ODT and ODP extractors already read the very
  same `meta.xml`.
- ODT and ODP extraction recovers nested inline text, hyperlink URLs and labels, annotations, index
  containers, page-anchored frames, list styles, footnotes and nested lists and tables inside table
  cells, and distinguishes endnotes from footnotes. ODP additionally processes presentation notes and
  recurses through shape groups and bare shapes, which commonly carry placeholder text with no
  text-box wrapper.
- iWork extraction stops deleting repeated text. Deduplication collapsed any repeat anywhere in the
  document rather than adjacent ones, so a heading reused twice or a footer on every page survived
  only its first occurrence, and Keynote threaded one set across slides so a repeated footer was
  removed outright. Short strings such as `5`, `OK` and `Q1` were also discarded by a length floor,
  and a member that fails to parse now warns instead of being dropped in silence. Numbers sheet names
  are emitted as their own top-level heading with tables nested beneath, matching the Excel extractor.
- HWP 5.0 documents extract their body text at all. Stream listing returned absolute paths while
  every caller tested for a root-relative prefix, so no body section and no embedded image was ever
  found, and the paragraph header and text record tags held the wrong values, so every body record
  failed to match even once sections were located. Tables, document metadata and equations are also
  extracted, and HWPX gains section headers and footers, footnotes, link annotations, SVG/WMF/EMF
  images and rich content inside table cells.
- DBF `DateTime` fields render as a real timestamp rather than an empty cell of unknown type, and a
  memo field is resolved from its `.dbt`/`.fpt` sidecar beside the `.dbf` instead of failing the file
  outright.
- Eight markup extractors no longer discard content their parser had already reached. RST unknown
  directives degrade to their body text instead of taking the whole indented block with them, losing
  figures and list/CSV tables; DocBook keeps simple paragraphs, variable-list terms and definitions
  and cross-references; JATS keeps figure captions, graphics, the whole back matter and citation DOIs
  and publishers; Org keeps body-level captions; OPML reads note bodies; djot keeps captions,
  description terms and div classes and no longer degrades inline math to plain text; LaTeX produces
  tables for `longtable`, `tabularx` and `tabulary` rather than re-emitting their rows as prose;
  Typst recognises multi-line display math, figures, quotes and citations; and FictionBook keys
  footnote definitions by id so references resolve.
- MDX and Markdown share one document builder. The MDX extractor held a drifted copy that handled no
  inline or display math, inline or block-level raw HTML, superscript, subscript or definition-list
  titles, so all of it was silently dropped from `.mdx` files, and the two dialects parsed with
  different option sets. TOML frontmatter delimited by `+++` is now recognised, so Hugo and Zola
  documents no longer leak their raw frontmatter into the body and yield no metadata. EPUB stops
  dropping definition items, citations, admonitions, footnotes, titles, page breaks, raw blocks,
  blockquotes and the contents of SVG, object, embed and iframe subtrees.
- YAML frontmatter keeps keys outside a fixed eleven-key allowlist, which were previously dropped,
  and reads both `author` and `authors` including list-valued forms, which silently vanished.
  Malformed or unclosed frontmatter emits a processing warning rather than being indistinguishable
  from a document that has none.
- Jupyter notebooks extract HTML-only and Markdown-only outputs, error tracebacks, SVG images,
  `update_display_data`, and cells whose source is empty but whose outputs are not — all of which
  were previously collapsed to `text/plain` or skipped. Markdown cell attachments now warn instead of
  being dropped silently.
- Audio and video transcription parses Whisper's timestamp tokens into real segments. The tokens are
  not marked special in the vocabulary, so decoding left them in the transcript as literal text; each
  segment now becomes a paragraph carrying start and end times. The four declared audio and video
  alias MIME types are also claimed.
- An RTF `\bin` payload is consumed by its declared byte count rather than by character count, so a
  multi-byte character in the payload no longer makes the parser overrun, swallow the group's closing
  brace and discard the rest of the document. Separately, the HTML link-annotation path sliced text by
  byte offsets with no character-boundary check, so a non-ASCII document with a span landing mid
  character panicked; the label is now dropped, the URL kept, and a processing warning names the loss.
- Email extraction repairs header parsing, which validated the whole message as UTF-8 and therefore
  dropped `Content-Type`, `MIME-Version`, `List-Id` and the rest whenever a single 8-bit byte appeared
  anywhere in the body, and which could panic when its scan cap landed mid character. Embedded
  messages are inlined with their own attachments attached as children, skipped attachments warn
  rather than being listed with their content gone, threading headers reach the content, and PST
  extraction surfaces its failures instead of discarding them and returning a clean-looking result.
- PST extraction enumerates non-IPM top-level folders without hanging. The first attempt called an
  upstream routine that holds a lock across a call re-acquiring the same non-reentrant lock on the
  same thread — an unconditional self-deadlock reached before a single row is read, so no iteration
  cap could help. Opening the root folder and reading its hierarchy table takes a different path
  through the same library and does not deadlock, so the folders are enumerated rather than skipped.
  Search folders are traversed but contribute no messages: upstream tags a search folder's linked
  rows with a distinct node type it never reads, so their contents table is always absent and no
  message can be emitted twice under two different folder paths. A regression test pins that, so an
  upstream version that starts returning those rows fails loudly instead of silently duplicating
  messages.
- CSV parsing no longer collapses on a stray quote. A quote anywhere mid-field flipped the parser into
  quoted mode and swallowed every delimiter and newline to the next quote or end of file, silently
  folding the rest of the file into one cell; quoted mode now opens only on an empty field. Rows whose
  fields are all empty are kept rather than dropped, which previously shifted every later row index,
  and delimiter sampling widened from ten to fifty lines while skipping blank and comment lines. YAML,
  TOML, JSONL and top-level JSON arrays render headings and fields instead of falling through to one
  opaque code block, and a multi-document YAML stream — Kubernetes manifests, Compose bundles — is
  iterated rather than rejected outright.
- Archive members are emitted in the archive's own order rather than in the randomized iteration
  order of a hash map, so the same archive no longer renders differently on every run and the member
  bodies agree with the file listing printed above them. Entries that were skipped or could not be
  read are named rather than counted, and the text-extension allowlist widened to common source and
  configuration formats.
- HTML is detected before the generic XML fallback. The `starts_with('<')` branch preceded the
  doctype and `<html>` checks, so those could never fire, and a bare HTML fragment was typed as XML
  and handed to the XML extractor. Fragments are additionally recognised by the name of their first
  element, using an allowlist that deliberately omits names shared with the XML vocabularies this
  crate extracts. The `application/wordperfect` and `application/x-quarto` alias MIME types are
  now claimed by an extractor, instead of being advertised and then rejected as unsupported.
- `list_supported_formats()` is derived from the live extractor registry rather than from an ungated
  static table, so it can no longer advertise a format whose extractor is compiled out. This is the
  library-level counterpart to the native FFI fix above and covers third-party registered extractors
  as well.
- PDF extraction reads image alt text from the structure tree, XMP metadata (which is where modern
  PDFs carry title, author and subject when the info dictionary is empty), page labels, and optional
  content group visibility, so content on layers that are off by default is no longer extracted as
  though it were visible. Filled AcroForm values reach the rendered output, and unencodable images,
  annotation failures and form failures emit processing warnings instead of being dropped at debug
  log level.
- An OCR'd PDF page that produced a table or an image is no longer discarded whole and replaced by a
  naive paragraph split, which meant its tables never reached the document and its page-local table
  and image references were dropped rather than rebased onto the parent. The mixed native/OCR route
  also discarded backend warnings, and words whose bounding box was missing were thrown away entirely
  rather than kept without geometry.
- An annotation whose end offset falls in trailing whitespace is clamped rather than discarded. The
  shift compensated for leading trim but compared against a fully trimmed length.
- Building with `--features pdf,layout-detection` and no OCR feature compiles. Several layout code
  paths were gated on an OCR feature while being called from layout-only code.
- Reordered PDF page text is reassembled rather than concatenated, so two spans whose original
  adjacency supplied the space between them are no longer glued together.
- The JSON renderer emits a node for every element kind. A catch-all with fourteen unhandled arms
  silently dropped page breaks, footnote references and definitions, citations, slides, definition
  terms and descriptions, admonitions, raw blocks and metadata blocks from the body. Footnote
  definitions in particular were unreachable in JSON, because every extractor moves them onto the
  footnote content layer and the body filter skipped them before the renderer's arm was reached, and
  a definition with no reference pointing at it was dropped from every rendered format. Styled HTML
  also closes its slide sections and renders slide titles, which were opened and never closed and
  never emitted, and renders formulas as delimited display math in a math-classed element rather than
  as a code block, so KaTeX and MathJax can pick them up.
- Table rendering is unified on one implementation. Eight divergent copies meant the same table had
  different integrity depending on which extractor produced it: one sized the grid from the header row
  and dropped every cell past it, and none of them escaped pipes or newlines in cell content.
- FictionBook, djot, HTML, Org, RST, Markdown and MDX link their extracted tables and images to
  document elements. Several extractors recorded the data without creating a corresponding element,
  and every renderer walks the element list, so that content was silently absent from the output;
  Markdown and MDX images carried a sentinel index that a later pass never patched, so every image was
  dropped. HTML, Org and RST additionally re-pushed tables that had already been created in flow,
  duplicating them.
- Renderers reached through the public entry point no longer emit an empty shell. The blanket
  implementation round-tripped through a conversion that yields zero elements; rendering now runs from
  the preserved internal document, and the registry attaches it so plugin renderers see per-element
  structure. A plugin-produced document's pre-rendered content is honored when the element rendering
  is empty, and the conversion at the plugin trait boundary copies the seven fields it previously
  dropped — URIs, children, annotations, processing warnings, LLM usage, pages and OCR elements.
- `enrich()` writes its results onto the document instead of only into the side struct it returns.
  Named-entity recognition, classification and captioning output reached nobody, because the document
  is the only thing that serializes, that the REST schema and the language bindings expose, and that
  splitting and post-processing operate on — and every LLM and VLM call's token and cost record was
  discarded outright.
- `split_and_extract` preserves every enrichment field. Each segment was rebuilt from a handful of
  fields, silently dropping twenty-three others including keywords, entities, summaries, chunks and
  warnings, so splitting a document threw away most of what extraction produced. An off-by-one in the
  chunk image-index remap also pointed chunks at the wrong image.
- Chunk, keyword and quality signals survive the pipeline: chunks are reclassified after heading
  context resolves, block nodes link to the chunks containing them, byte offsets survive the
  heading-context rewrite, and token counts, heading paths and page-less image links are populated.
  Semantic chunking warns when it degrades to the structural fallback, chunk classification reports
  partial batch failures and keeps confidence and usage, keywords carry positions and warn on
  documents skipped as too short, the quality score no longer bypasses its navigation and script
  penalties for short text, and token reduction honors `preserve_important_words`.
- The post-processor cache is rebuilt when the registry changes instead of being populated once and
  never again, a re-registered extractor no longer orphans a stale name-index entry, and extraction
  falls back to lower-priority extractors when the first reports an unsupported format or a plugin
  error — parsing errors deliberately do not cascade, so a corrupt file fails on the right extractor.
  Fields excluded from serialization, such as the cancellation token and OCR acceleration settings,
  are restored after a configuration JSON merge rather than reset to their defaults, and requesting a
  custom output format that produces no rendering reports plain rather than the requested name. A
  built-in processor that fails to register now reaches the caller as a processing warning rather
  than yielding a clean success with no output and no explanation.
- Decoded QR-code payloads reach chunks and embeddings. The section was appended to `content` only,
  and the output-format step replaces `content` with the rendered document before the final chunking
  pass, so for every non-plain output format the payload was destroyed before it was ever chunked.
  URL-shaped payloads are also routed into the document's URI list.
- The image-captioning prepass merges its full result back onto the document. It zipped captioned
  images against the document's own, truncating to the shorter side whenever a processor added or
  removed one, and carried back only descriptions, warnings and usage. It also destructively consumed
  the code-intelligence scratch key from a clone and then overwrote the original's metadata with the
  stripped copy, so code intelligence fell back to a chunks-only payload whenever captioning was
  enabled.
- The keyword post-processor is re-registered after a registry clear. A one-shot guard around
  registration never re-ran, so any registry cycle in the same process left it permanently
  unregistered.
- An OCR cache hit no longer returns less than a miss. The structured document is excluded from
  serialization, so a hit returned a result without it, and the cache key omitted the output format,
  so a Markdown request was served the plain entry.
- OCR results forward Tesseract block type, justification and paragraph attributes, hOCR font size and
  text angle, and per-word language to callers; words whose parent block is an image or noise region
  are kept and counted rather than filtered away. The Tesseract path clusters words into multiple
  per-page table regions instead of assuming one table per page, table detections consume header and
  spanning-cell information, and produced tables carry bounding boxes and identifiers. Backend
  metadata and tables are populated through a shared builder, and the layout model's full class
  taxonomy is mapped.
- GLM-OCR paired-mode output is structured into regions rather than returned as one undifferentiated
  block.
- Detection under the tract OCR backend builds a plan for each page's own resized extent instead of
  padding every page into one fixed square canvas. The detection backbone reduces over the whole
  spatial extent, so enlarging the input rescaled every channel gate and shifted the probability map
  across the entire page rather than only at the padding seam, which merged adjacent text lines and
  made tract diverge from ONNX Runtime on text-dense pages. The two engines now agree.
- Merging OCR results from an embedded image keeps every backend field. The merge rebuilt the document
  from content, MIME type and OCR elements alone, discarding tables, language, page-segmentation mode
  and confidence metadata, formulas, LLM usage, detected languages and processing warnings.
- Named-entity recognition scans the whole document. The splitter stopped at a fixed token limit and
  discarded everything past it with no error and no warning, so a long document was silently only
  partly scanned for personally identifying information; input is now split into overlapping windows
  and detections merged back into source coordinates.
- Translation covers every text-bearing field. It previously reached only `content`, the rendered
  content and chunk text, so a translated document silently returned untranslated tables, pages,
  metadata and document structure. A build with translation but without redaction also compiled the
  document-structure translation path down to a stub that returned success and did nothing.
- Configurations loaded from TOML, YAML or JSON files, and configurations merged in as JSON overrides,
  are validated. Validation ran only in the environment-variable loader, so an invalid OCR backend or
  DPI surfaced far from the setting that caused it.
- Paragraph splitting normalizes line endings first, so a Windows-authored document no longer
  collapses into a single paragraph carrying stray carriage returns. This affects plain text, email
  and PST bodies — which are mandated to use CRLF — OCR backend output and djot conversion.
- A document with more links than the per-document URI cap emits one warning naming how many were
  found and how many were kept, instead of silently looking as though it had exactly the cap.
- A cross-reference or citation whose target was never extracted is reported rather than dropped at
  debug log level, so a caller can tell "no cross-references" from "cross-references silently
  discarded". One warning per document names the keys, capped at ten with a count for the rest.
- HTML, RTF, DocBook and JATS report a lossy decode instead of returning a mojibake'd document that is
  indistinguishable from a clean one, and an OPML file with no outline element is reported rather than
  returning an empty document that looks like an outline with no entries.
- The Node.js binding's native library is built with the linker flag that keeps it loaded, so
  `linux-gnu` artifacts can no longer segfault when the module is unloaded. The build script that
  applies it was declared as a dependency but never invoked.
- The Swift package resolves for iOS again. The FFI dependency was pinned in an ungated dependency
  table while its sibling was correctly gated per target, and because Cargo unifies features across
  edges, iOS resolved two mutually exclusive OCR backends at once and tripped the mobile guards.
- Docker images build again after the name-compatibility shim became a workspace member: it was in no
  image's copy list, so the build failed while loading the manifest, before compilation started. A
  guard script now fails when a workspace member is neither copied into the build context nor stripped
  from the manifest.
- Bounding-box, timeout and margin settings are honored during extraction rather than accepted and
  ignored, span flattening grows the table grid to fit an overflow row instead of clamping it into the
  last one, spans not covered by layout detection are interleaved through the reading-order graph
  rather than appended as a tail, a page that fails to render emits a warning instead of vanishing,
  and a detected table's bounding box is threaded into cell-grid construction.

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
