---
title: "Changelog"
---

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

---

## [1.1.0] - Unreleased

### Added

- Added HWPX (Hangul Word Processor XML) extraction to the WebAssembly package. `unhwp`
  target-gates its ZIP reader to a deflate-only, LZMA-free build under `wasm32`, so the
  native-C dependency that previously kept `hwpx` off `wasm-target` does not apply there.
- Added diagram recovery from flat OpenDocument drawings (`.fodg`), including content-based
  detection of the `application/vnd.oasis.opendocument.graphics-flat-xml` MIME type. Connectors
  name their endpoints outright, so the recovered graph is exact rather than inferred from
  geometry (#1545 corpus fixture).
- Added structural extraction for MyST Markdown syntax and MyST text notebooks, including saved
  inline `{eval}` values in Jupyter markdown cells
  ([#1538](https://github.com/xberg-io/xberg/issues/1538)).
- Added extraction of Jupytext percent- and light-format notebook scripts, including
  `text/x-python`, `text/x-r-source`, and `text/x-julia` MIME aliases
  ([#1538](https://github.com/xberg-io/xberg/issues/1538)).
- Added bounded, cancellable SQLite and GeoPackage table extraction with schema-based GeoPackage
  detection, `.sqlite3` and `.gpkx` filename support, and defensive handling for untrusted
  databases (#1510).
- Added configurable MIME inference policies for preferring content signatures, trusting supported filename
  extensions, or ignoring extensions, with per-input overrides (#1509).
- Added native KML and GeoJSON extraction with canonical MIME routing (#1508).
- Added Rust `SUPPORTED_FORMAT_COUNT` and `SUPPORTED_EXTENSION_COUNT` constants derived from the
  MIME registry, plus automated synchronization for published format-count claims (#1511).
- Added reusable Rust PDF render sessions for querying page counts and rendering multiple pages
  without reopening the document (#1485).
- Added cooperative cancellation for single and batch extraction (#1476).
- Added dynamic system linking for Tesseract and Leptonica through the `tesseract-dynamic` feature
  (#1407).
- Added managed Azure AD, Google Vertex AI, and AWS STS credential providers, with credential values
  redacted from debug output.
- Added reasoning-effort, provider-specific request-body, and Bedrock configuration for LLM
  extraction.
- Added `xberg doctor` and the Rust `doctor()` API for validating configuration and probing every
  compiled OCR, VLM, layout, table, formula-recognition, and cache capability without downloading
  models or contacting remote providers. `xberg doctor --clean` removes stray files only from
  Xberg-owned caches (#1347).
- Added the Sceptre EasyOCR Gen2 backend for desktop, mobile, and WebAssembly.
- Added sparse and late-interaction embeddings to chunk output.
- Added a Prometheus `/metrics` endpoint to the API server (#1391).
- Added explicit CSV delimiters and comment-line prefixes through `CsvOptions`.
- Added `xberg tree-sitter` commands for downloading, listing, and cleaning language assets, with
  optional configuration-file loading.
- Added VLM extraction for complex PDF regions and LaTeX formula extraction from VLM OCR.
- Added structural AsciiDoc and WebVTT extraction.
- Added Docling DocTags input and output, including tables and page geometry (#1383).
- Added formula recognition for rasterized pages and exposed formulas consistently across extracted
  formats (#1385).
- Added JATS, EPUB, ODT, and ODP MathML-to-LaTeX conversion.
- Added deterministic diagram recovery from SVG and PDF sources with Graphviz DOT output (#579).
- Added `SecurityLimits.max_pages` for PDF, presentations, Keynote, and multi-frame TIFF documents
  (#1451).
- Added explicit PDF backend selection through `PdfConfig.backend` and `--pdf-backend` (#1448).
- Added musllinux Python wheels and a Windows x86_64 Ruby gem.
- Added PDF and HTML extraction plus layout and transcription types to the WebAssembly package.
- Added `--ocr-no-cache` to bypass the Tesseract result cache.
- Added `ContentFilterConfig.include_footnotes` for retaining footnotes classified as page furniture.
- Added a public `render_heading_breadcrumb` helper for retrieval-oriented chunk content (#1393).
- Added structured-output merge, citation, and vision-fallback helpers for Rust embedders.
- Added a Tower-compatible extraction service, request type, and builder for Rust applications.
- Added typed configuration for TrOCR, PaddleOCR-VL, GLM-OCR, and DeepSeek-OCR backends.
- Added `classify_chunks_owned` for classifying and returning an owned document.
- Exposed chunk-classification and LLM concurrency, provider, cache, budget, and rate-limit configuration
  types at the Rust crate root.
- Added `OcrConfig::security_limits`. `ExtractionConfig::security_limits` is now threaded through to
  every OCR route — embedded images, Tesseract, PaddleOCR, and scanned PDF pages — instead of each
  route decoding images under a hardcoded `SecurityLimits::default()`
  ([#1554](https://github.com/xberg-io/xberg/issues/1554)).

### Changed

- **Breaking (Python binding):** `ExtractionConfig` and `DoctorReport` are now frozen dataclasses
  rather than `TypedDict`s, matching the 121 option types that were already dataclasses. Passing a
  plain `dict` or a JSON string as `config` still works — `extract()` coerces both — but an
  `ExtractionConfig` *object* no longer supports mapping operations, so `config.get("chunking")` and
  `config["chunking"] = ...` now raise `AttributeError`/`TypeError`, and the instance is immutable.
  Build a modified config with `dataclasses.replace(config, chunking=...)`.
- PDF parsing no longer reports recoverable input at WARN. A missing embedded font, an object
  outside the xref table, an unreadable CFF version, and a reading-order fallback are ordinary
  properties of real PDFs rather than conditions an operator can act on; they are now TRACE (or
  DEBUG for strategy fallbacks), and each document emits a single DEBUG summary on the
  `xberg_native_pdf::recovery` target carrying the totals instead of one event per occurrence.
  Measured over a 4,000-document corpus this removed 4,012,488 of 4,014,206 log events, against
  which 44 genuine parse failures had been sitting at a ratio of about 1 in 91,000. ERROR
  behaviour is unchanged — it already corresponded one to one with documents that failed
  ([#1547](https://github.com/xberg-io/xberg/issues/1547)).

- **Breaking (Rust source):** `validate_mime_type` no longer accepts any value with an `image/`
  prefix. It now parses the MIME type and requires exact membership in the supported-format
  registry, so unregistered vendor image subtypes such as `image/x-custom-format` are rejected as
  `UnsupportedFormat` instead of validating (#1511).
- Per-page OCR recognition-noise detail (fragmented-word ratio, word count, mean confidence) now
  reaches the page accept/reject decision and is emitted at `DEBUG` instead of being discarded one
  frame earlier. No threshold is gated on it yet; the blended stage score alone cannot discriminate
  noise pages.
- **Breaking (Rust source):** `ExtractionConfig` adds `apply_notebook_cell_tags`. Notebook
  extraction now honors MyST and Jupyter Book remove/hide cell tags by default; set the field to
  `false` to retain all saved cell content
  ([#1538](https://github.com/xberg-io/xberg/issues/1538)).
- **Breaking (Rust source):** `OcrQualityThresholds` adds `discard_suspected_ocr_noise`; exhaustive
  struct literals must set the field or use `..Default::default()`.
- **Breaking:** configuration deserialization now rejects unknown fields in nested Xberg
  configuration tables instead of silently ignoring misspelled settings.
- **Breaking:** PDF backend configuration now uses `"native"` and `PdfBackend::Native` instead of
  `"pdf_oxide"` and `PdfBackend::PdfOxide`. Update explicit configuration values; the default is
  unchanged.
- **Breaking:** `EmbeddingModelType::Llm` and `RerankerModelType::Llm` now carry their model name in
  the enum variant.
- **Breaking:** `Formula.bbox` and `Formula.page` are optional so formulas from formats without page
  geometry can be represented.
- **Breaking:** unknown multipart fields on extraction endpoints now return an error instead of being
  ignored.
- Chunk `content` now contains the exact source span; heading breadcrumbs are available separately.
- The CLI `all` feature now includes audio transcription.
- `security_limits.max_pages` now applies to presentations, Keynote, and multi-frame TIFF as well as
  PDF.
- `create_client_with_credential_provider` now returns `ManagedClient`, and an LLM concurrency limit
  of zero is rejected.
- Native PDF pages now expose their final per-page reading order.
- WebVTT cue timing is optional for blocks without a timing line.
- OpenDocument packages without `content.xml` now return an extraction error.
- CLI text output now includes the extraction envelope with warnings, timings, and metadata.
- CLI JSON output now reports peak resident memory.
- Windows builds now include the same supported feature set as other desktop builds.
- **Breaking:** Rust element identifiers now use `String` directly; the `ElementId` wrapper has been
  removed.
- **Breaking:** Public tuple fields for ranges, coordinates, dimensions, links, code blocks, and attributes now
  use named Rust structs and serialize as JSON objects. Legacy positional JSON arrays are still accepted when
  parsing, so payloads written by 1.0.x keep deserializing, but they are no longer emitted.
- **Breaking:** removed the duplicate `xberg::llm::region_extractor::RegionKind`; import `xberg::RegionKind`
  instead.
- Parsing and configuration deserialization now reject invalid region, redaction, and reranker values.
- Corrected and expanded installation, CLI, configuration, extraction, migration, integration, and
  cross-language API documentation.
- Corrected canonical MIME and extension routing for DBF, YAML, reStructuredText, Org, Typst,
  XHTML, Djot, JPEG 2000, HEIC/HEIF, MP4, and MPEG inputs.
- GeoJSON extraction now returns a bounded aggregate summary by default, including feature,
  geometry, property-key, position, and bounds metadata. Set
  `geojson.include_full_coordinates = true` to retain the complete document and coordinate arrays.
- `quality_score` now explicitly measures the cleanliness and readability of retained text, not
  extraction completeness; inspect `processing_warnings` for known partial or degraded results.
- The default `security_limits.max_table_cells` remains 100,000 aggregate cells per document;
  limit errors now explain how to raise it for trusted inputs or reduce the source table.

### Removed

- **Breaking:** removed the inert `ChunkingConfig::prepend_heading_context`, `breadcrumb_target`,
  `BreadcrumbTarget`, and corresponding CLI and environment options; use chunk metadata or
  `render_heading_breadcrumb` when a retrieval index needs headings inline.
- **Breaking:** removed `OutputFormat::Structured`; use `Plain` for unrendered content or `Json` for a structured
  content tree.
- **Breaking:** removed `ExtractedDocument.formatted_content` from language bindings; use `content`
  or select the desired output format during extraction.
- Removed advertised support for troff, mdoc, POD, and DokuWiki because they did not have structural
  extractors.
- Removed fabricated OCR `script_name` and `script_confidence` values.
- Removed the unused public `LanguageRegistry`, `BatchProcessor`, object-pooling APIs, and unused
  tree-sitter re-exports.
- Removed the nonfunctional `wasm-threads` feature.
- Removed PDF writing, editing, building, and XFA conversion APIs from the native PDF crate; read-only
  XFA analysis remains available.
- **Breaking:** removed the inert `Engine` structured-policy, preset-resolver, LLM-client, and model-provider
  injection methods.
- **Breaking:** removed the inert transcription field from `EnrichmentConfig`; configure transcription during
  extraction instead.
- **Breaking:** embedding, reranking, sparse-embedding, late-interaction, and preset APIs are now exposed only
  when their required features are enabled.
- **Breaking:** `core::batch_mode`, `core::formats`, and `core::io` are now crate-private, and the public
  `DocumentStructureBuilder` has been removed.

### Fixed

- Fixed a ruled troubleshooting page collapsing into one table, taking its section headings
  down with it as cell text. `split_rows_by_text_positions` subdivides a producer-drawn row
  band by the Y positions of the text inside it, and since the #1555 fix a candidate split was
  accepted only when EVERY resulting Y-cluster carried text in at least two columns, with the
  rejection all-or-nothing for the band. A band that mixes multi-column data rows with
  single-column lines -- a section heading, a lead-in, a wrapped continuation -- can never
  satisfy that, so one such line vetoed the split for the whole band and every line inside it
  became cell text. On one 56-page installation manual, six ~20 pt row bands became a single
  522 pt table, the document went from 808 elements to 759, and four numbered headings
  disappeared from the outline. The band is now split once at least two of its clusters are
  independently evidenced, and each deficient cluster is resolved on its own terms: it folds
  into the cluster above only when it introduces no column that cluster left empty, which is
  the signature of a wrapped continuation. Anything else -- a heading, a lead-in -- stays a row
  of its own, one cell wide, which is what such a line inside a ruled band actually is. Two
  independently evidenced clusters are required rather than one because a single evidenced
  cluster can be coincidence, which is precisely the #1555 case
  ([#1565](https://github.com/xberg-io/xberg/issues/1565)).
- Fixed a word split across two touching PDF spans being rejoined with a space, so `prijs`
  extracted as `pri js`. The gap between the two spans measures 0.069 pt -- 0.008 em at 9 pt,
  against a 2.5 pt space glyph -- on an identical baseline at an identical font size, so no gap
  threshold produced the space: `segments_need_space` reached one of its unconditional
  `return true` branches first. `SegmentData` keeps only `is_bold`/`is_italic`/`is_monospace`
  and drops `font_name`, so a mid-word switch between two embedded subset fonts whose
  `/FontDescriptor`s disagree on `ForceBold`, `ItalicAngle` or `FixedPitch` reads as a style
  change carrying no geometric signal at all. That is why the defect never reproduced against
  base-14 Helvetica, and why widening the gap to 2 pt changed nothing. A touching-spans guard
  now runs before those branches: two segments on the same baseline, at the same font size,
  with alphanumeric characters on both sides of the boundary and a gap under 0.025 em are one
  word and are concatenated. The guard can only join, never split, and it never fires across an
  explicitly drawn space. The table path needed the same test one stage earlier, in
  `segments_to_words`, because `HocrWord` is integer-rounded and cannot represent a sub-point
  gap by the time cell text is joined. Affects ordinary prose, not just tables: of 18 confirmed
  cases, 14 were `NarrativeText`, 3 `ListItem` and 3 `Table`
  ([#1566](https://github.com/xberg-io/xberg/issues/1566)).
- Fixed PDF table reconstruction dropping early rows when data-start inference classified more
  than two leading rows as headers. The two-row header cap is retained, but surplus inferred
  header rows are now demoted to data in source order instead of being discarded
  ([#1558](https://github.com/xberg-io/xberg/issues/1558)).
- Fixed native PDF top-to-bottom reading order splitting one visual table row at an absolute
  3-point coordinate-band boundary, which could move an article number before its position and
  fuse the two identifiers. Visual rows now use an anchored, font-scaled tolerance, reconstructed
  lines restore left-to-right fragment order, and narrative assembly preserves a separator after
  a severe geometric backtrack ([#1560](https://github.com/xberg-io/xberg/issues/1560)).
- Fixed PDF dehyphenation treating inline run/style boundaries as visual line wraps. Suspended
  hyphens such as `vracht- en verzendkosten` are now preserved, while compounds genuinely split
  across different baselines are still rejoined
  ([#1561](https://github.com/xberg-io/xberg/issues/1561)).
- Fixed DOCX page attribution staying permanently low after Word omitted a rendered-page marker
  between vertically stacked inline images. The parser now conservatively infers missing breaks
  from each section's usable page height, including documents with different section geometries
  ([#1559](https://github.com/xberg-io/xberg/issues/1559)).
- Fixed DOCX DrawingML and VML text boxes dropping XML and numeric character references such as
  `&amp;` and `&#8364;` from extracted text
  ([#1562](https://github.com/xberg-io/xberg/issues/1562)).
- Fixed OCR image decoding ignoring the caller's configured `security_limits`. Every OCR route —
  embedded images, Tesseract, PaddleOCR, and scanned PDF pages — decoded raw image bytes under a
  hardcoded `SecurityLimits::default()`, so raising `ExtractionConfig::security_limits` to accept a
  large scan still had it rejected at the OCR decode step. The configured limits now reach all four
  routes, and PaddleOCR also honors a per-call `backend_options["security_limits"]` override
  ([#1554](https://github.com/xberg-io/xberg/issues/1554)).
- Fixed a drawn PDF table row with a wrapped cell being shattered into extra rows. Splitting a row
  band by text Y-position now requires at least two columns to have independent text evidence for
  every candidate row before splitting; a band where only one column wraps to a second line now
  stays a single row ([#1555](https://github.com/xberg-io/xberg/issues/1555)).
- Fixed monospace font detection matching any font name containing "mono", misclassifying foundry
  names such as "Monotype Corsiva" as a monospace font and skewing the word-spacing heuristic and
  code-block detection that depend on it. "Monotype" is now excluded from the substring match, and
  the PDF text run buffer's separate ad hoc monospace check was replaced with the same shared
  helper.
- Fixed a standalone multi-line monospace paragraph not being recognized as a code block unless it
  had a consecutive monospace neighbor paragraph. A lone paragraph that already carries two or more
  monospace lines is now fenced as a code block on its own
  ([#1557](https://github.com/xberg-io/xberg/issues/1557)).
- Fixed PDF text extraction silently corrupting ordinary text. A contextual ligature-repair pass
  rewrote `:` to `ti` and an uppercase `M` between lowercase letters to `tti` on every element of
  every document, mangling identifiers, ratios, times, URLs, and units such as `nM` (for example
  `aMb` became `attib`). The repair was introduced for European PDFs that encode ligature glyphs
  at ASCII code points, but it was gated at the time on a per-font broken-CMap signal from
  pdfium's `has_unicode_map_error()`. That gate was lost when pdfium was removed as a backend and
  was never ported to pdf_oxide, leaving the rewrite running unconditionally. Both substitutions
  are removed; they can only return alongside a real document-level evidence gate
  ([#1556](https://github.com/xberg-io/xberg/issues/1556)).
- Fixed optional fields in the Python and PHP bindings rejecting payloads that omit them.
  The generated mirror structs lost their `#[serde(default)]` attributes, so deserializing a
  document whose JSON left an optional field out failed instead of falling back to the default.

- Fixed legacy `.doc` headings being guessed from line length rather than read from the document's
  own styles. A paragraph styled `heading 1`..`heading 9` — directly or through a custom style
  derived from one, such as `TOC Heading` — now becomes a `Heading` at that level, instead of every
  detected heading being a level 2. Documents that apply no heading style keep the previous
  shape-based detection, because roughly half the test corpus styles its headings as bold `Normal`
  and would otherwise lose every one; the choice is made per document, not per paragraph. A
  heading-styled paragraph that is also list-bound stays a `ListItem`, matching how the DOCX path
  treats `w:numPr` ([#1553](https://github.com/xberg-io/xberg/issues/1553)).
- Fixed legacy `.doc` automatic list numbering being dropped entirely: a paragraph Word numbers
  through its list tables arrived as prose, indistinguishable from an unnumbered sentence, while
  the DOCX path emitted a `ListItem` for the same construct. Auto-numbered paragraphs now arrive
  as `ListItem`s inside an ordered or bulleted list container, with their nesting depth, matching
  the DOCX path. The number Word paints (`1.1`, `a.`) is still not rendered — recovering it needs
  list-table counter state — so a document mixing automatic and hand-typed numbering shows the
  typed numbers as text and the automatic ones as list structure
  ([#1550](https://github.com/xberg-io/xberg/issues/1550)).
- Fixed legacy `.doc` elements being split on blank lines rather than on Word's paragraph marks,
  which merged every pair of consecutive paragraphs not separated by a blank line into a single
  element. One corpus letter returned its entire ten-paragraph body as one element. Word97 and
  later documents now emit one element per Word paragraph, matching what the DOCX path does with
  `w:p`. **This changes element boundaries, counts and indices for most `.doc` documents**, and
  alters `content` line spacing accordingly; consumers keying on element position will see the
  difference. Word 6/95 documents and those falling back to contiguous text extraction keep the
  previous blank-line behaviour, because they carry no paragraph properties to use.
- Fixed legacy `.doc` extraction reading `fcClx` from `FibRgFcLcb97` pair 66 — an obsolete field
  Word writes as zero — instead of pair 33, so the piece table was never walked for any document
  and extraction always fell back to reading `reserved5`/`reserved6`, bytes [MS-DOC] requires a
  reader to ignore. Where those bytes disagreed with the real text start, whole documents were
  decoded as UTF-16LE and returned as glued CJK-looking code points; multi-piece and fast-saved
  documents could not be assembled at all. Footnote, header/footer, comment, and text-box
  subdocument text now also reaches the output for these files
  ([#1551](https://github.com/xberg-io/xberg/issues/1551)).
- Fixed the Elixir NIF's vendored `Cargo.lock`, shipped in the Hex package, pinning
  `tree-sitter-language-pack` 1.15.12 while the crate requires 1.16.1 — a source build of the NIF
  with `--locked` could not resolve. This affects anyone whose platform has no precompiled
  artifact and therefore builds from source.
- Fixed a DOCX table cell spanning several grid columns (`w:gridSpan`) or rows (`w:vMerge`) being
  returned once per covered column and again for every covered row, so a cell merged across 4
  columns and 3 rows came back 12 times in `result.tables[].cells`, `result.tables[].markdown`,
  and `result.content` alike — a 39 KB document could extract to 232 KB. A merged/spanned cell's
  text is now written once, at its origin, with the columns and rows it covers left blank. This
  also fixes a DOCX header or footer table with a merged cell shifting every following cell one
  column to the left ([#1549](https://github.com/xberg-io/xberg/issues/1549)).
- Fixed PDF render diagnostics matching a captured engine warning against a hardcoded message
  substring to decide whether it meant a glyph actually failed to paint. The message it was built
  to exclude no longer reaches this capture at all (it moved to TRACE under #1547), so the match
  could only ever misfire: a future warning whose text happened to share that substring would have
  been silently dropped instead of surfacing as a `ProcessingWarning`. Every captured warning is
  now reported ([#1548](https://github.com/xberg-io/xberg/issues/1548)).
- Fixed a PDF page that places a statistics table beside a prose column being emitted in
  full-width Y order, which spliced the prose apart mid-sentence (`more likely to be aged
  35Female 51.5 ...`) and welded the table's two label/value panels together on every row. The
  table region is now emitted whole, in row order, ahead of the prose column, and a repeated
  panel is emitted panel by panel
  ([#1545](https://github.com/xberg-io/xberg/issues/1545)).
- Fixed PDF text coming back scrambled when a short `Tj` run sat between two `TJ` arrays: the run
  was emitted at an earlier run's stale position and sorted into the wrong place, so
  `within a period ... after conclusion` extracted as `wincthin a period ... after co lusion`.
  Every text-showing boundary operator closed the pending run except `TJ`
  ([#1544](https://github.com/xberg-io/xberg/issues/1544)).
- Fixed every image in a DOCX reporting `page_number` 1 regardless of the page it sits on. The page
  was resolved by searching rendered Markdown for a per-image placeholder that is never written --
  every drawing renders to the same link target -- so the lookup always missed. Page numbers now
  come from the parsed element order ([#1546](https://github.com/xberg-io/xberg/issues/1546)).
- Fixed an author's hyphen being deleted when it fell at a line break, so `price-` + `determining`
  joined as `pricedetermining`. A hyphen written mid-line elsewhere in the same document is now
  treated as evidence that the compound is real and its hyphen is kept. Compounds that appear only
  broken, with no such occurrence anywhere in the document, are still joined without the hyphen
  ([#1543](https://github.com/xberg-io/xberg/issues/1543)).
- Fixed OCR backends registered through `register_ocr_backend` being rejected before extraction
  started: configuration validation checked the backend name against the built-in list only, which
  made every custom plugin OCR backend unusable once validation was wired into `extract` and
  `extract_batch`.
- Fixed the native C FFI library shipping without eleven features the crate advertises, so the
  Java, Go, C#, Swift, Zig, and C bindings had no summarization, translation, analysis, HEIC,
  captioning, ML redaction, or static-embedding support. The desktop dependency hand-maintained a
  feature list that had drifted from `full`; a regression test now fails on any future omission.
- Fixed HTML pages fetched over HTTP(S) losing every format-specific metadata field: results were
  reported as `text/html` while `metadata.format` stayed empty, because the extraction ran over the
  crawler's pre-rendered Markdown and never reached the HTML extractor. Title, headings, Open Graph,
  Twitter card, links, and structured data are now recovered from the page HTML.
- Fixed `pdf_options.hierarchy.enabled` silently producing no hierarchy: headings were detected and
  then discarded unless the caller also set the unrelated `pages.extract_pages`. Requesting the
  heading hierarchy now enables the per-page tracking it requires.
- Fixed the bundled Tesseract build failing to configure on Windows when the MSVC developer
  environment is not present, which broke building Xberg from source with the default OCR features.
- Fixed URL extraction reporting internally converted HTML pages as `text/markdown`; results now
  retain a validated, canonical source MIME type.
- Fixed `clear_post_processors` stopping at the first failed shutdown hook and permanently removing
  enabled built-ins; it now attempts every shutdown, returns the first error, and restores built-ins
  before the next post-processed extraction while custom processors remain cleared.
- Fixed VLM concurrency limits increasing concurrent local OCR work and raster memory use (#1465).
- Fixed structured extraction forcing every caller schema to JSON Schema Draft 2020-12; validation
  now honors the schema's declared draft while keeping external reference resolution offline
  ([#1539](https://github.com/xberg-io/xberg/issues/1539)).
- Fixed hybrid PDF OCR dropping surrounding prose when a table-bearing bare-text page was
  restructured alongside geometry-backed pages.
- Fixed automatic PDF OCR fallback reporting an empty success when OCR failed and no native text
  remained; recoverable failures still return available native text with a warning.
- Fixed degraded VLM fallback output replacing denser OCR text, while abstaining from the density
  comparison for short text and non-space-delimited CJK or kana content.
- Fixed Windows source and Ruby package builds failing on stable Rust while validating the
  identity of staged Tesseract source directories.
- Fixed GCC 12+ WordPerfect builds by adding the standard header that declares `size_t` before
  compiling the pinned libwpd source.
- Fixed Ruby source-package installation by aligning the Gemfile and lockfile with the gemspec's
  supported `rb_sys` range.
- Fixed generated Ruby development commands so Bundler and its tools use the active Ruby
  interpreter, avoiding native-extension ABI conflicts on systems with multiple Ruby versions.
- Fixed generated Python optional constructor arguments so Pyrefly receives precise keyword types
  without unused helper declarations.
- Fixed generated Dart tests for nested tagged unions, nullable payloads, and Flutter Rust Bridge
  tuple accessors; added e2e analyzer coverage and refreshed the Dart lock file to the generated
  Flutter Rust Bridge version.
- Fixed compressed image inputs with oversized declared dimensions exhausting memory during OCR,
  layout and QR detection, image classification, re-encoding, HEIF conversion, or structured-image
  rasterization; decoded allocations now obey `security_limits.max_content_size` and are rejected
  from the image header before pixel decode.
- Fixed PDF OCR fallback being suppressed for image-only pages when dot leaders or other
  non-textual native content pushed the document below the alphanumeric-ratio threshold.
- Fixed process-global native PDF font-cache collisions that made glyph spacing, geometry, and
  batch output depend on document order and concurrency when fonts used indirect width tables.
- Fixed Markdown OCR metadata so word counts and confidence statistics describe only text retained
  after dictionary filtering; fully filtered output now reports zero words and omits confidence
  quantiles.
- Fixed repeated bold PDF presenter labels and same-row legend keys being promoted to headings,
  which could invert document hierarchy and fragment retrieval chunks.
- Fixed PDF OCR so fragmented, low-confidence, and dictionary-suspect non-empty text is retained
  with a processing warning by default instead of silently emptying pages. Set
  `ocr.quality_thresholds.discard_suspected_ocr_noise = true` (or the equivalent pipeline quality
  threshold) to opt into the previous destructive filtering behavior.
- Fixed runtime crashes in system-linked Tesseract OCR builds by linking the required native exception-safety
  shim.
- Fixed `xberg batch` so mixed-success runs emit every successful document and every attributed
  per-input error before returning a nonzero status; JSON and TOON timing slots remain aligned with
  inputs, and TOON now uses the documented batch envelope.
- Fixed `xberg extract --ocr false` so it authoritatively disables implicit OCR fallback, overrides
  conflicting loaded OCR routing, and rejects contradictory OCR flags.
- Fixed Tesseract preprocessing so deskew, denoise, contrast enhancement, and Otsu, adaptive, and
  Sauvola binarization settings transform the OCR raster on native and WebAssembly backends;
  `none` (with `off` as an alias) preserves unthresholded grayscale when deskew is disabled,
  sparse receipt-image fallback and faint colored text no longer lose content to global
  thresholding, dark labels over bright map fills still receive Otsu preprocessing without
  isolated or clustered dark artifacts triggering it, and
  WebAssembly Tesseract now rejects images exceeding 4096 × 4096 pixels before decoding.
- Fixed OCR measurement tooling so line-filter comparisons score the intended ground-truth lines
  and report filtering regressions accurately.
- Fixed the OpenAPI document's dangling Djot attribute reference so schema validators and client generators can
  resolve every advertised component (#1505).
- XML and JSON content with unsupported specialized extensions now routes through the supported generic extractor
  instead of failing MIME validation (#1507).
- File extraction now falls back to bounded content sniffing when a path has an unknown or missing extension (#1506).
- Explicit `application/octet-stream` hints now trigger configured MIME detection instead of being
  treated as an authoritative document type.
- Fixed documentation-snippet fixtures that named non-existent result fields, which made the generated
  snippets silently drop the affected presentation block: element `content` is now `text`, table `rows`
  is now `cells`, and the result paths `keywords`, `structured_data`, and `document_structure` are now
  `extracted_keywords`, `structured_output`, and `document`.
- Fixed EPUB extraction for `text/html` spine items, named entities, declared non-UTF-8 encodings,
  navigation documents, SVG fallbacks, nested tables, MathML, headings, images, and malformed HTML
  (#1486, #1488-#1494).
- EPUB extraction now preserves usable chapters when another spine item fails and reports per-item
  warnings instead of failing the whole document (#1491).
- Fixed EPUB metadata, EPUB 2/3 cover selection, DRM detection, and font-obfuscation handling (#1492,
  #1494).
- Fixed PDF OCR and rendering for highly compressed scans, CCITT images, CFF fonts, maximum-size font
  tables, malformed embedded fonts, rotated content, missing glyph warnings, and concurrent Pdfium
  extraction.
- Fixed native PDF tracing so corrupt optional content is reported as a recoverable warning, while
  mandatory cross-reference failures emit a single operation-boundary error without changing the returned error type.
- Fixed annotation-only PDFs so visible FreeText content is recovered into page-aware document text,
  including when OCR replaces the page text, without exposing hidden, transparent, cropped, or
  disabled annotations when annotation extraction is off.
- Fixed the Swift package manifest so SwiftPM no longer warns about a nonexistent target-relative license file.
- Fixed scanned PDF extraction so CCITT parameters align with their filter in multi-filter streams,
  referenced JBIG2 image masks are available to OCR, and stencil-mask polarity renders text as opaque.
- Fixed PDF reading order for dense two-column layouts, hanging clause numbers, split list markers,
  and modest font-size changes on one baseline.
- Fixed PDF heading recovery for repeated bold section titles set at body font size while retaining
  short bold labels, presenter attributions, and calendar legends as body text (#1513).
- Fixed PDF table extraction so multi-word cells, rule-less prose regions, OCR-derived tables, and
  page-local table failures are handled correctly (#688, #1358, #1542).
- Fixed PDF Markdown and Djot output so native text is retained when structured conversion is
  incomplete.
- Fixed PDF configuration so metadata suppression and header/footer settings are honored by every
  backend; invalid or unsupported PDF and OCR settings now return configuration errors.
- Fixed OCR-backed PDFs so filtering, confidence thresholds, hierarchy, tables, formulas, lists,
  bounding boxes, page boundaries, and partial page results are preserved consistently across output
  formats and OCR backends (#1444).
- Fixed Tesseract caching, configuration, preprocessing, page segmentation, and font-size extraction.
- Tesseract Markdown extraction now reports a `ProcessingWarning` when dictionary filtering removes
  physical text lines, including the number removed.
- OCR element hierarchy output now honors `build_hierarchy` and contains only resolvable parent
  references.
- Fixed Sceptre and PaddleOCR line grouping, region ordering, per-page resizing, table validation,
  and font-size reporting.
- Fixed DOCX extraction for nested tables, VML images, text boxes, comments, fields, headings,
  hyperlinks, headers, footers, table-of-contents entries, nested lists, and page attribution;
  element output now preserves explicit page breaks and single-page documents report page metadata
  consistently (#1452, #1460, #1503).
- Fixed PPTX extraction for malformed relationships, nested image paths, equations, fallback shapes,
  comments, metadata, and security limits.
- Fixed spreadsheet extraction for hyperlinks, formulas, names, comments, hidden state, dates, and
  OpenDocument metadata.
- Fixed ODT, ODP, iWork, HWP, DBF, RTF, email, and PST extraction across nested content, metadata,
  binary data, folder traversal, and repeated text.
- Fixed Markdown, MDX, RST, HTML, DocBook, JATS, FictionBook, Djot, Org, YAML frontmatter, and Jupyter
  extraction so supported structure and content are retained.
- Fixed `result.elements` so headings report their level (`metadata.additional["heading_level"]`)
  instead of every `##`-`######` heading collapsing into indistinguishable `Heading` elements with
  empty metadata; `result.document.nodes` already carried the level correctly (#1504).
- Fixed CSV parsing for stray quotes and archive extraction order.
- Fixed MIME routing so HTML is detected before the generic XML fallback and supported-format lists
  reflect the active extractor registry.
- Fixed post-processing, chunking, enrichment, translation, NER, QR codes, captions, and caching so
  extracted structure is preserved consistently.
- Fixed chunking presets so standalone and pipeline APIs apply the documented size and overlap while
  preserving unrelated chunking settings.
- Fixed extraction timeout handling so timed-out work is cancelled.
- Fixed configuration merging so changing one CLI option no longer erases sibling settings.
- Fixed multipart API extraction to accept `json` and `doctags` values for `output_format`.
- Fixed cache keys to reflect only settings that affect the corresponding extraction or OCR result.
- Fixed model caching so OCR, embedding, and reranking settings no longer reuse incompatible models.
- Fixed Node.js native-library loading, Swift iOS resolution, Windows DirectML packaging, and
  `cargo install xberg-cli` (#1456).
- Fixed Docker image builds and reduced the CLI image to runtime dependencies.
- Fixed API and packaging defects in the Python, PHP, Dart, Go, Java, C#, Kotlin, Elixir, Ruby, Zig,
  and C packages.
- Fixed Windows wheel and gem packaging, manylinux compatibility, musl smoke tests, and dynamic
  Tesseract builds (#1495, #1497).
- Fixed archive and ZIP validation for small compressed entries and impossible declared sizes
  (#1496).
- Fixed batch extraction so configured caches are used and progress callbacks report completed items.
- Fixed extraction configuration validation so invalid nested values, including OCR quality and
  scanned-page thresholds, are rejected consistently by every public entry point.
- Fixed error classification so callers can distinguish all documented extraction failure categories.
- Built-in path and byte extraction now always reports a recognized `extraction_method`; custom
  extractors retain explicit provenance and otherwise leave it unspecified.
- Fixed owned document classification so detected labels are written back to the returned document.
- Fixed `ContentFilterConfig.include_watermarks` so enabling it retains watermark content.
- Fixed `JsonExtractionConfig.flatten_nested_objects` so disabling it preserves nested objects instead of
  flattening them.
- Fixed standalone-image and OCR-backed PDF results so preprocessing scale, dimensions, and DPI are retained.
- Fixed Candle OCR configuration so supported backend options are validated and applied.
- Fixed PaddleOCR-VL so the task selected when constructing the backend is honored unless a request
  explicitly overrides it.
- Fixed keyword extraction so invalid n-gram ranges return an error instead of silently producing
  empty results.
- Fixed builds that enable only the `api` or `mcp` feature.
- Fixed the `excel-wasm` feature so spreadsheet extraction builds for WebAssembly.
- Fixed WebAssembly configuration so unsupported managed credential providers are rejected explicitly.

### Security

- Bounded DOCX image and iWork archive member reads by the member's declared uncompressed size
  instead of trusting that declaration. A crafted document could forge a small declared size in the
  ZIP central directory while carrying a deflate stream that inflated to multiple gigabytes,
  exhausting memory during DOCX image extraction (`images.extract_images`) or `.pages`/`.numbers`/
  `.key` extraction. Reported by Syed Anas Mohiuddin
  ([GHSA-85w9-wqcq-x48r](https://github.com/xberg-io/xberg/security/advisories/GHSA-85w9-wqcq-x48r)).
- Pinned downloaded Tesseract, Leptonica, and English tessdata inputs to immutable revisions with
  verified sizes and SHA-256 digests, race-safe content-addressed caches, private build directories,
  and bounded fail-closed archive extraction.
- Structured extraction now resolves caller-provided JSON Schemas strictly offline and rejects
  external HTTP and file references without performing I/O.
- REST and MCP requests can no longer override LLM credentials, provider registrations, or other
  server-controlled settings.
- Hardened ZIP accounting against overflow, impossible sizes, and compression-ratio bypasses.
- Hardened DOCX, PPTX, and EPUB relationship resolution against container traversal, malformed UTF-8,
  NUL bytes, drive-letter paths, UNC paths, and symlink escapes.
- Added bounded EPUB traversal and retained-content accounting to prevent resource-limit bypasses.
- Cache namespaces are validated before directories are created.
- Redaction now reports only content that was actually removed, never exposes pre-redaction element
  text, and rejects invalid strategies instead of silently falling back to masking.

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

- OCR-backed PDF extraction now keeps consecutive Tesseract paragraphs grouped within their shared
  hOCR text area instead of splitting them, including pages replaced by mixed native/OCR extraction.
  This is paragraph/block grouping only; font-clustering headings and list-marker detection for
  OCR-backed pages are tracked separately (see Unreleased).
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
