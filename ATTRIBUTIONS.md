# Attributions

This document acknowledges sources of vendored code, runtime artifacts, test documents, and baseline data used in the Xberg project.

## Pandoc Test Suite

Test documents and reference baseline outputs derived from the Pandoc test suite:

- **Source**: <https://github.com/jgm/pandoc>
- **License**: GPL-2.0-or-later
- **Usage**: Test documents and reference baselines only (no code copied from Pandoc)
- **Attribution**: John MacFarlane and Pandoc contributors
- **Purpose**: Baseline reference testing - used to validate our native Rust extractors work correctly on the same documents that Pandoc processes

### Test Documents from Pandoc

The following test documents were copied from the Pandoc repository to `/test_documents/`:

#### Org Mode

- `org-select-tags.org` - SELECT_TAGS and EXCLUDE_TAGS testing
- `pandoc-tables.org` - Org Mode table formats
- `pandoc-writer.org` - Comprehensive Pandoc test suite in Org Mode format

#### Typst

- `typst-reader.typ` - Fibonacci sequence with mathematical formulas
- `undergradmath.typ` - Comprehensive undergraduate mathematics document (16KB)

#### DocBook

- `docbook-chapter.docbook` - Recursive section hierarchy (7 nested levels)
- `docbook-reader.docbook` - Comprehensive DocBook 4.4 test suite (36KB, 1704 lines)
- `docbook-xref.docbook` - Cross-reference (xref) functionality testing

#### JATS

- `jats-reader.xml` - Comprehensive JATS (Z39.96) Journal Archiving test document (38KB, 1460 lines)

#### FictionBook

- `test_documents/fictionbook/pandoc/` - 13 FictionBook test files including:
  - `basic.fb2` - Basic FictionBook structure
  - `images-embedded.fb2` - Embedded base64 images
  - `math.fb2` - Mathematical content
  - `meta.fb2` - Document metadata testing
  - `reader/emphasis.fb2` - Text emphasis testing
  - `reader/epigraph.fb2` - Epigraph/quote elements
  - `reader/meta.fb2` - Document metadata and title info
  - `reader/notes.fb2` - Footnotes/endnotes with cross-references
  - `reader/poem.fb2` - Poem/verse structure
  - `reader/titles.fb2` - Section titles and heading hierarchy
  - And others

#### OPML

- `opml-reader.opml` - OPML 2.0 outline structure (US states example)
- `pandoc-writer.opml` - Comprehensive Pandoc test suite in OPML format

### Baseline Outputs Generated

For each test document listed above, three baseline outputs were generated using Pandoc 3.8.3:

1. **Plain Text** (`*_pandoc_baseline.txt`) - Raw text content extraction
2. **JSON Metadata** (`*_pandoc_meta.json`) - Full Pandoc AST with document structure and metadata
3. **Markdown** (`*_pandoc_markdown.md`) - Markdown representation for format comparison

**Total**: 132 baseline files for 44 documents across 6 formats

### GPL Compliance Statement

We acknowledge that Pandoc is licensed under GPL-2.0-or-later. We have:

- ✓ Used Pandoc's test documents (test data is allowed under GPL)
- ✓ Generated baseline outputs using Pandoc for comparison purposes
- ✓ NOT copied any Pandoc source code
- ✓ Implemented our extractors independently in Rust
- ✓ Used Pandoc only as a behavioral baseline for testing

Our Rust extractors are independently implemented and do not contain any GPL-licensed code from Pandoc.

### Verification

Test documents and baselines can be regenerated at any time using:

```bash
./generate_pandoc_baselines.sh
```

This script processes all test documents and generates fresh baselines using the installed version of Pandoc.

## docx-lite

DOCX XML parser vendored into `crates/xberg/src/extraction/docx/parser.rs`:

- **Source**: <https://github.com/v-lawyer/docx-lite>
- **License**: MIT OR Apache-2.0
- **Authors**: V-Lawyer Team
- **Version**: 0.2.0 (vendored with modifications)
- **Usage**: DOCX text extraction parser inlined into xberg core
- **Modifications**:
  - Fixed `Paragraph::to_text()` joining text runs without whitespace (#359)
  - Adapted to xberg's `quick-xml` v0.39 and `zip` v7.x APIs
  - Removed file-path based APIs (only bytes/reader needed)

---

## xberg-gliner

Span-mode GLiNER preprocessing, prompt construction, tensor construction, and decoding logic vendored into
`crates/xberg-gliner/`:

- **Source lineage**: `gline-rs` → `xberg-gliner`
- **Source URLs**: <https://github.com/fbilhaut/gline-rs>, <https://github.com/xberg-io/gline-rs-fork>
- **Source crate/version**: `xberg-gliner`, vendored from `gline-rs` 0.2.1 (fork at `xberg-io/gline-rs-fork`)
- **License**: Apache-2.0
- **Author**: Frédérik Bilhaut
- **Usage**: Local ONNX named-entity recognition backend for xberg
- **Scope**: Span-mode text splitting, zero-shot prompt construction, Hugging Face tokenizer adapter, GLiNER tensor construction, logits decoding, and greedy overlap filtering
- **Excluded**: ORP pipeline code, examples, CSV helpers, relation extraction, token-mode runtime, memory profiling, and generic pipeline abstractions
- **Modifications**:
  - Adapted crate identity to `xberg-gliner`
  - Removed ORP dependency and replaced the pipeline wrapper with direct `ort` session creation and synchronized inference
  - Integrated with xberg's model catalog/cache and `xberg-io/gliner-models` artifact layout
  - Added focused unit tests for validation, splitting, prompt construction, tensor shapes, decoding, and overlap filtering

## GLiNER Models

GLiNER ONNX runtime artifacts consumed by xberg are exported and governed through `xberg-io/gliner-models`.
Their source model lineage is the `gliner-community` Hugging Face organization.

- **Upstream source**: <https://huggingface.co/gliner-community>
- **Runtime artifact repository**: <https://huggingface.co/xberg-io/gliner-models>
- **Default license**: Apache-2.0 unless a model manifest declares otherwise
- **Original GLiNER authors**: Urchade Zaratiana, Nadi Tomeh, Pierre Holat, Thierry Charnois
- **Usage**: ONNX weights, tokenizer files, manifests, checksums, and notices for xberg NER
- **Policy**: Model weights are not committed to this repository; verified artifacts are downloaded into the xberg model cache

---

## hwpers

Vendored HWP text extraction code from the hwpers crate:

- **Source**: <https://github.com/Indosaram/hwpers>
- **License**: MIT OR Apache-2.0
- **Authors**: HWP Parser Contributors
- **Vendored Version**: 0.5.0
- **Location**: `crates/xberg/src/extraction/hwp/`
- **Purpose**: Text extraction from Korean Hangul Word Processor (.hwp) files
- **Scope**: Minimal subset — CFB reader, binary record parser, text extraction only
- **Excluded**: HWPX (XML/ZIP), writer, renderer, crypto, preview modules

---

## paddle-ocr-rs

Vendored source code from the paddle-ocr-rs crate for PaddleOCR via ONNX Runtime integration:

- **Source**: <https://github.com/mg-chao/paddle-ocr-rs>
- **Original License**: Apache-2.0
- **Author**: mg-chao (<chao@mgchao.top>)
- **Vendored Version**: 0.6.1
- **Location**: `crates/xberg-paddle-ocr/`
- **Purpose**: Text detection and recognition using PaddlePaddle's OCR models via ONNX Runtime

### Vendored Files

The following source files were vendored from paddle-ocr-rs:

- `ocr_lite.rs` - Core OCR pipeline and high-level API
- `db_net.rs` - DBNet text detection network
- `crnn_net.rs` - CRNN text recognition network
- `angle_net.rs` - Text angle detection network
- `base_net.rs` - Base network trait
- `ocr_utils.rs` - Image preprocessing utilities
- `ocr_result.rs` - Result type definitions
- `scale_param.rs` - Scaling parameter calculations
- `ocr_error.rs` - Error type definitions

### Modifications

The vendored code has been modified for Xberg integration:

- Updated to Rust 2024 edition
- Aligned with Xberg workspace dependencies
- License changed to MIT with dual copyright (original author retained)

### License Compatibility

The original Apache-2.0 license is compatible with MIT relicensing. The original copyright and attribution are preserved in the vendored crate's LICENSE file.

---

## fastembed-rs

Text embedding inference pipeline vendored into `crates/xberg/src/embeddings/engine.rs`:

- **Source**: <https://github.com/Anush008/fastembed-rs>
- **License**: Apache-2.0
- **Author**: Anush008 and contributors
- **Vendored Version**: Based on 0.2.x
- **Location**: `crates/xberg/src/embeddings/engine.rs`
- **Purpose**: ONNX-based text embedding inference with thread-safe concurrent embedding generation

### Modifications

The vendored code has been modified from the original fastembed-rs:

- Changed `embed()` method signature from `&mut self` to `&self` for thread-safe concurrent inference without mutex contention
- Adapted to Xberg's ONNX Runtime integration and error handling
- Integrated with Xberg's embedding configuration and model management

### License Compatibility

The original Apache-2.0 license is fully compatible with Xberg's Elastic License 2.0 (ELv2). The original copyright and attribution are preserved in the vendored code's comments.

---

## numbers-parser Test Fixtures

Test documents derived from the `numbers-parser` test suite:

- **Source**: <https://github.com/masaccio/numbers-parser>
- **License**: MIT
- **Author**: Jon Connell (masaccio)
- **Usage**: Test documents and reference baselines only (no code copied)
- **Modifications**: Fixtures downloaded directly for integration testing.
- **Location**: `test_documents/iwork/`

---

---

## yake-rust

YAKE keyword extraction algorithm vendored into xberg:

- **Source**: <https://github.com/quesurifn/yake-rust>
- **License**: MIT
- **Authors**: Kyle Fahey, Anton Vikstrom, Igor Strebz
- **Vendored Version**: 1.0.3
- **Location**: `crates/xberg/src/keywords/yake/`
- **Purpose**: YAKE (Yet Another Keyword Extractor) statistical keyword extraction

### Modifications

- Replaced segtok dependency with custom memchr-based sentence splitter (fixes #676 BacktrackLimitExceeded on large files)
- Integrated with xberg's stopwords module (64 languages vs original 34)
- Replaced hashbrown with ahash, inlined streaming-stats and levenshtein
- Optimized punctuation checks with byte lookup tables
- Removed itertools dependency (manual dedup)

### License Compatibility

The original MIT license is compatible with Xberg's Elastic License 2.0 (ELv2).

## text-splitter (inlined)

The chunking submodule `crates/xberg/src/chunking/text_splitter/` is a trimmed inline copy of [text-splitter](https://github.com/benbrandt/text-splitter) v0.30.1 by Benjamin Brandt. We inlined it because upstream pins `tokenizers = "0.22"`, which conflicts with xberg's direct `tokenizers 0.23` dependency and pulls a duplicate copy of `tokenizers` into the build graph (breaking the `Tokenizer: ChunkSizer` bound in `chunking::core`).

- **Source**: <https://github.com/benbrandt/text-splitter> @ v0.30.1
- **License**: MIT
- **Copyright**: © 2023 Benjamin Brandt <benjamin.j.brandt@gmail.com>
- **Location**: `crates/xberg/src/chunking/text_splitter/`

### Modifications

- Dropped the `code` (tree-sitter) splitter — xberg has its own tree-sitter integration and does not use the upstream code splitter.
- Dropped the `tiktoken-rs` sizer — unused.
- Rebuilt against `tokenizers 0.23`.
- Renamed feature gate `tokenizers` → `chunking-tokenizers`; the `markdown` splitter is always available because `pulldown-cmark` is already a non-optional xberg dependency.
- Tightened visibility on internal types to `pub(crate)`.
- Path rewiring: upstream `crate::*` paths inside the inlined module rewritten relative to the new submodule root.

### License Compatibility

The MIT license is compatible with Xberg's Elastic License 2.0 (ELv2). The full upstream license text is reproduced below:

```text
MIT License

Copyright (c) 2023 Benjamin Brandt

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

### Test Documents from text-splitter

The following test inputs were copied from the text-splitter repository to `/test_documents/text_splitter/`:

- `text/romeo_and_juliet.txt` — Shakespeare, public domain (Project Gutenberg)
- `text/room_with_a_view.txt` — E. M. Forster, public domain (Project Gutenberg)
- `markdown/commonmark_spec.md` — CommonMark spec, CC-BY-SA-4.0
- `markdown/github_flavored.md` — GitHub Flavored Markdown spec, CC-BY-4.0

---

## libheif-rs

Safe Rust bindings around `libheif-sys` for decoding HEIF / HEIC / AVIF
containers, vendored as the `xberg-libheif` crate:

- **Source**: <https://github.com/Cykooz/libheif-rs>
- **License**: MIT
- **Author(s)**: Kirill Kuzminykh (Cykooz) and contributors
- **Vendored Version**: 2.7.0
- **Location**: `crates/xberg-libheif/`
- **Purpose**: Decode HEIF-family containers (HEIC, HEIF, AVIF, HEICS, AVCS) to
  interleaved RGBA pixels and expose EXIF / XMP metadata blocks for xberg's
  image-extraction and OCR pipeline.

### Vendored Files / Scope

- Full upstream `src/` tree (LibHeif, HeifContext, ImageHandle, Image, decoder,
  encoder, color profile, regions, metadata, reader, security limits, track,
  utils, and the optional `image`-crate integration module).
- Vendored verbatim from upstream v2.7.0 so the public API mirrors `libheif-rs`
  exactly; we continue to depend on upstream `libheif-sys` from crates.io for
  the underlying C bindings.

### Modifications

- Workspace dependency alignment (`libc`, `image` pinned via the workspace).
- Rust 2024 edition (upstream is 2021).
- Replaced upstream `include_str!("../README.md")` doc shim with an inline
  module-level vendoring header pointing at this file.
- Workspace clippy / rust lints applied via `[lints] workspace = true`.

### License Compatibility

MIT is permissive and compatible with re-distribution alongside Xberg's
Elastic License 2.0 (ELv2) workspace. The upstream MIT license text is
preserved verbatim at `crates/xberg-libheif/LICENSE`.

### System Library Requirement

`libheif-rs` is a safe wrapper around the C library `libheif`, which in turn
requires `libde265` (HEVC) and `libaom` (AV1). These must be available at build
and runtime. `libheif` is LGPL-licensed and is linked dynamically only (never
statically vendored). Pixel decoding is therefore native-only — the `heic` feature is excluded from
xberg's `wasm-target` and `android-target` aggregate features. EXIF
extraction via `nom-exif` is pure Rust and works on every target.

---

## ttf-parser

A safe, zero-allocation TrueType / OpenType / AAT font parser, vendored as the
`xberg-ttf-parser` crate:

- **Source**: <https://github.com/harfbuzz/ttf-parser>
- **License**: MIT OR Apache-2.0
- **Author(s)**: Yevhenii Reizner, Khaled Hosny, Laurenz Stampfl, Caleb
  Maclennan and contributors
- **Vendored Version**: 0.25.1, plus nine upstream pull requests carried on top:
  `#203` (`b422ac0`), `#207` (`52a9811`), `#216` (`9b9e55f`), `#222` (`3a585f1`),
  `#223` (`32439d2`), `#224` (`dd2337b`), `#225` (`99aa5e3`), `#226` (`86daf57`)
  and `#228` (`023f8163`). See `crates/xberg-ttf-parser/README.md` for what each
  one fixes.
- **Location**: `crates/xberg-ttf-parser/`
- **Purpose**: Parse embedded font programs when rendering PDF pages. Reaches
  xberg transitively through `pdf_oxide` and `fontdb`, the only two consumers in
  `Cargo.lock`.

### Why Vendored

Upstream is currently unmaintained: at the time of vendoring the most recent
commit was 2025-11-22, with correctness fixes sitting unreviewed. Upstream
v0.25.1 rejects any CFF charstring containing the deprecated `dotsection`
operator, dropping the affected glyphs entirely (`i`, `j`, `!`, `.`), so PDF
pages using Type 1 derived CFF fonts silently lose those characters. Vendoring
lets xberg carry the fix without waiting on an upstream release.

### Vendored Files / Scope

- Full upstream `src/` tree and `tests/`, plus `CHANGELOG.md` and both license
  texts. Not vendored: `benches/`, `examples/`, `c-api/`, `testing-tools/`,
  `meson.build`.
- Kept byte-identical to upstream wherever possible so future upstream fixes
  cherry-pick without conflict.
- `tests/fonts/` contains upstream's test font binaries. `colr_1.ttf` and
  `colr_1_variable.ttf` ship their own Apache-2.0 license text at
  `tests/fonts/colr_1_LICENSE`. Test data is excluded from the published crate.

### Modifications

- Package renamed to `xberg-ttf-parser`, with `[lib] name = "xberg_ttf_parser"`.
- Crate-level lint allows in `src/lib.rs` and `tests/tables/main.rs`; the
  workspace lint set is stricter than upstream's CI. Allowed rather than fixed
  because `div_ceil` and `is_multiple_of` postdate upstream's 1.63 MSRV.
- A dev-dependency on the `compat/` shim so the vendored tests keep referring to
  `ttf_parser` unchanged.
- `compat/` is an xberg addition, not upstream code: a name-compatibility shim
  named `ttf-parser` that re-exports this crate. Cargo matches
  `[patch.crates-io]` on package name alone, so it is the only way to redirect
  the transitive consumers onto the fixed parser. It is never published.
- The `glyf` / `gvar` component visit budget from upstream `#224` is charged per
  component record rather than once per call, which is the only behavior-affecting
  xberg change inside the vendored `src/` tree.
- Two xberg-authored regression tests at `tests/xberg_fvar.rs` and
  `tests/xberg_set_variation.rs`, pinning upstream `#216` and `#207`, which ship
  no tests of their own. Placed at the top level so `tests/tables/main.rs` stays
  byte-identical to upstream.

### License Compatibility

MIT and Apache-2.0 are both permissive and already on the `deny.toml` allow
list. Upstream's license texts are preserved verbatim at
`crates/xberg-ttf-parser/LICENSE-MIT` and
`crates/xberg-ttf-parser/LICENSE-APACHE`.

---

## libwpd + librevenge

Native WordPerfect document libraries built from source and statically linked
into the `xberg-libwpd` crate:

- **Source**: <https://libwpd.sourceforge.net/>
- **License**: MPL-2.0 OR LGPL-2.1+ (xberg builds under the MPL-2.0 arm)
- **Authors**: Document Liberation Project (LibreOffice/AbiWord ecosystem) contributors
- **Versions**: librevenge 0.0.6, libwpd 0.10.3
- **Location**: vendored as committed `.tar.gz` archives under `crates/xberg-libwpd/vendor/`, decompressed and compiled by `crates/xberg-libwpd/build.rs` (checksum-verified at extract time); consumed via `crates/xberg-libwpd/src/shim.cpp`
- **Purpose**: WordPerfect (.wpd) document extraction
- **Modifications**: none; both libraries are built from unmodified upstream source

### License Compatibility

See `THIRD_PARTY_LICENSES.md` for the full MPL-2.0 vs LGPL-2.1+ linking analysis. In short: xberg statically links under the MPL-2.0 arm (file-level copyleft, static linking explicitly permitted), so no copyleft obligation reaches xberg's MIT-licensed core or language bindings.

---

## Boost C++ Libraries (build-time, xberg-libwpd)

A header-only subset of Boost, used at build time by the `xberg-libwpd` C++ shim:

- **Source**: <https://www.boost.org/>
- **License**: BSL-1.0 (Boost Software License 1.0)
- **Author**: Boost contributors
- **Location**: `bcp`-extracted subset vendored as `crates/xberg-libwpd/vendor/boost-subset.tar.gz`, decompressed by `crates/xberg-libwpd/build.rs`
- **Purpose**: header-only dependency of libwpd/librevenge (build-time only; no Boost code is redistributed in xberg binaries)
- **Modifications**: none; subset extracted verbatim with `bcp`

---

## jhqxxx/aha

Rust-native VLM-OCR backends vendored into `crates/xberg-candle-ocr/`:

- **Source**: <https://github.com/jhqxxx/aha>
- **License**: Apache-2.0
- **Author**: jhqxxx
- **Vendored Version**: `e29ddc589d089042afd66ab8ea76409d8d33f701` (jhqxxx/aha @ HEAD on 2026-06-17)
- **Location**: `crates/xberg-candle-ocr/src/vendor/aha/` (shared infra), `crates/xberg-candle-ocr/src/models/deepseek_ocr/` (DeepSeek-OCR + Qwen2 decoder), `crates/xberg-candle-ocr/src/models/paddleocr_vl/` (PaddleOCR-VL 1.5 upgrade)
- **Purpose**: Rust-native VLM-OCR backends (PaddleOCR-VL 1.5, DeepSeek-OCR) including their Qwen2 decoder dependency and shared SigLIP/CLIP/MRoPE infrastructure subsets.

### Vendored Files

| File | Source | Notes |
| ---- | ------ | ----- |
| (populated as Phases 3-4 land each file) | | |

### Modifications

- `anyhow::Result<T>` → `Result<T, CandleOcrError>` throughout
- `assert!`/`assert_eq!`/`panic!`/`unwrap()` on user-controlled values → typed `CandleOcrError` returns
- Candle 0.9.2 → Candle 0.10 API migration
- Generation loop routed through `xberg_candle_ocr::models::glm_ocr::mtp::generate_mrope` (with a new `forward_step_with_position_ids` trait extension per A3) — NOT aha's `common/generate.rs`
- rustdoc on every `pub` item
- Dead code blocks dropped (specific examples: aha `modules.rs:214-230, 265-266`)
- `paddleocr_vl::model::get_rope_index`: vision-block position ids restored to aha's (t, h, w) mrope layout after the vendoring rewrite transposed the h index and dropped the t row (crashed on non-square grids); images always carry a single frame so the t row is constant
- `paddleocr_vl::model::forward`: restored aha's seqlen_offset-based position continuation for cache_position-less decode steps (dropped during vendoring, failing every decode step with a dtype mismatch)
- `paddleocr_vl::engine::generate`: rewritten from a full-sequence re-feed (which double-appended the KV cache each step and read the argmax at the wrong rank) to aha's prefill + cached single-token step loop
- `paddleocr_vl::model::Ernie4_5Model::forward`: restored aha's causal attention mask for the multi-token prefill (dropped during vendoring; bidirectional prefill degenerated generation into single-token repetition)

### License Compatibility

Apache-2.0 is compatible with Xberg's Elastic License 2.0 (ELv2). Original Apache-2.0 copyright headers are preserved at file-top in each vendored file.

**Last Updated**: 2026-06-17

---

## Self-hosted retrieval models

xberg re-hosts ONNX weights for 17 upstream retrieval models across four `xberg-io/*` Hugging Face
repos and serves them as embedding/reranker/sparse/late-interaction presets. Weights are re-hosted
unmodified (unchanged upstream weights, xberg-added ONNX export/manifest/tokenizer artifacts only)
under the `xberg-io` org. All upstream licenses are permissive (Apache-2.0 or MIT), consistent with
xberg's permissive-only dependency policy.

### xberg-io/embedding-models

Dense sentence-embedding presets:

- **Runtime artifact repository**: <https://huggingface.co/xberg-io/embedding-models>
- **Usage**: ONNX weights, tokenizer files, and manifests for xberg's embeddings engine

| Upstream model | License | Preset |
| --- | --- | --- |
| [sentence-transformers/all-MiniLM-L6-v2](https://huggingface.co/sentence-transformers/all-MiniLM-L6-v2) | Apache-2.0 | `fast` |
| [BAAI/bge-base-en-v1.5](https://huggingface.co/BAAI/bge-base-en-v1.5) | MIT | `balanced` |
| [BAAI/bge-large-en-v1.5](https://huggingface.co/BAAI/bge-large-en-v1.5) | MIT | `quality` |
| [intfloat/multilingual-e5-base](https://huggingface.co/intfloat/multilingual-e5-base) | MIT | `multilingual` |
| [Alibaba-NLP/gte-modernbert-base](https://huggingface.co/Alibaba-NLP/gte-modernbert-base) | Apache-2.0 | `gte-modernbert-base` (default) |
| [minishlab/potion-base-8M](https://huggingface.co/minishlab/potion-base-8M) | MIT | `lightweight` (model2vec static) |
| [Snowflake/snowflake-arctic-embed-m-v2.0](https://huggingface.co/Snowflake/snowflake-arctic-embed-m-v2.0) | Apache-2.0 | `arctic-embed-m-v2.0` |
| [Qwen/Qwen3-Embedding-0.6B](https://huggingface.co/Qwen/Qwen3-Embedding-0.6B) | Apache-2.0 | `qwen3-embedding-0.6b` |

### xberg-io/reranker-models

Cross-encoder reranker presets:

- **Runtime artifact repository**: <https://huggingface.co/xberg-io/reranker-models>
- **Usage**: ONNX weights, tokenizer files, and manifests for xberg's reranking engine

| Upstream model | License | Preset |
| --- | --- | --- |
| [BAAI/bge-reranker-base](https://huggingface.co/BAAI/bge-reranker-base) | MIT | — |
| [BAAI/bge-reranker-v2-m3](https://huggingface.co/BAAI/bge-reranker-v2-m3) | Apache-2.0 | — |
| [jinaai/jina-reranker-v1-turbo-en](https://huggingface.co/jinaai/jina-reranker-v1-turbo-en) | Apache-2.0 | — |
| [Qwen/Qwen3-Reranker-0.6B](https://huggingface.co/Qwen/Qwen3-Reranker-0.6B) | Apache-2.0 | — |
| [cross-encoder/ettin-reranker-150m-v1](https://huggingface.co/cross-encoder/ettin-reranker-150m-v1) | Apache-2.0 | `balanced` (default alias) |

### xberg-io/sparse-embeddings

SPLADE-family sparse embedding presets:

- **Runtime artifact repository**: <https://huggingface.co/xberg-io/sparse-embeddings>
- **Usage**: ONNX weights, tokenizer files, and manifests for xberg's sparse embeddings engine

| Upstream model | License | Preset |
| --- | --- | --- |
| [prithivida/Splade_PP_en_v1](https://huggingface.co/prithivida/Splade_PP_en_v1) | Apache-2.0 | `splade` |
| [opensearch-project/opensearch-neural-sparse-encoding-doc-v3-distill](https://huggingface.co/opensearch-project/opensearch-neural-sparse-encoding-doc-v3-distill) | Apache-2.0 | `opensearch-v3-distill` (default) |

### xberg-io/late-interaction-models

ColBERT-family late-interaction presets:

- **Runtime artifact repository**: <https://huggingface.co/xberg-io/late-interaction-models>
- **Usage**: ONNX weights, tokenizer files, and manifests for xberg's late-interaction engine

| Upstream model | License | Preset |
| --- | --- | --- |
| [answerdotai/answerai-colbert-small-v1](https://huggingface.co/answerdotai/answerai-colbert-small-v1) | Apache-2.0 | `colbert` |
| [lightonai/GTE-ModernColBERT-v1](https://huggingface.co/lightonai/GTE-ModernColBERT-v1) | Apache-2.0 | `gte-moderncolbert` (default) |

### Policy

Model weights are not committed to this repository; verified artifacts are downloaded into the
xberg model cache at runtime, matching the GLiNER Models policy above.

**Last Updated**: 2026-07-08

---

**Last Updated**: June 17, 2026
**Pandoc Version Used**: 3.8.3
**Baseline Generation Date**: December 6, 2025
