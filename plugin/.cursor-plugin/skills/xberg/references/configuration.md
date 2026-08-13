<!--
AI-RULEZ :: GENERATED FILE — DO NOT EDIT
Content-Hash: blake3:c540cf03f9754e263644f9876f7921d8744f7e901d8eae41111c5792460f2b20
Source-Hash: blake3:ac8bab9ae4a76e61f437af29b4a45a38326056a50769a177038927c69353edaf
Schema-Version: v1
-->

# Configuration Reference

Xberg uses a hierarchical configuration system supporting multiple formats and auto-discovery mechanisms. This reference covers the `ExtractionConfig` schema (xberg 1.0.0-rc.1), the config-file keys, defaults, and loading strategies.

Every key and default below is backed by the Rust config structs in `crates/xberg/src/core/config/` (the serde field name, after any `#[serde(rename)]`, is the authoritative config-file key) and mirrored by the generated binding option dataclasses in `packages/python/xberg/options.py`.

## Supported Formats

Xberg configurations can be defined in three formats:

- **TOML** (recommended): `xberg.toml`
- **YAML**: `xberg.yaml` / `xberg.yml`
- **JSON**: `xberg.json`

All formats deserialize into the same `ExtractionConfig` schema. The schema uses `#[serde(deny_unknown_fields)]`, so an unrecognized key is a hard error — verify spelling against this reference.

## Auto-Discovery

`ExtractionConfig::discover()` searches for `xberg.toml` in the current working directory and then each parent directory up the tree, loading the first match. Explicit loads via `ExtractionConfig::from_file(path)` accept `.toml`, `.yaml`/`.yml`, and `.json` (format chosen by extension).

## Loading Configuration

Config-file loading and auto-discovery live in the Rust core and the CLI. The language bindings (Python, Node, etc.) accept a config object/dict directly — they do not expose file-loading class methods.

### Rust

```rust
use xberg::core::config::ExtractionConfig;

// Explicit path (.toml / .yaml / .json by extension)
let config = ExtractionConfig::from_file("xberg.toml")?;

// Auto-discover xberg.toml up the directory tree
let config = ExtractionConfig::discover()?; // -> Option<ExtractionConfig>
```

### CLI

```bash
# Explicit configuration file
xberg extract --config xberg.toml document.pdf

# Auto-discovery (searches cwd and parents for xberg.toml)
xberg extract document.pdf

# Inline JSON (field-level merge over the discovered/loaded config)
xberg extract --config-json '{"force_ocr": true}' document.pdf
xberg extract --config-json-base64 "$B64" document.pdf
```

### Python

In Python, `ExtractionConfig` is a `TypedDict`; pass a plain dict to `extract`. There is no `ExtractionConfig.from_file` / `.discover` in the binding — read the file yourself (or use the CLI) and pass the resulting dict.

```python
from xberg import extract, ExtractInput

result = await extract(
    ExtractInput(uri="document.pdf"),
    config={"force_ocr": True, "chunking": {"max_characters": 1000}},
)
```

### Node.js / TypeScript

The Node binding likewise takes a config object passed to `extract` — file loading/discovery is a CLI/Rust-core concern, not a binding class method.

## Configuration Schema

### Top-Level Options

```toml
use_cache = true
enable_quality_processing = true
force_ocr = false
disable_ocr = false
output_format = "plain"
result_format = "unified"
extraction_timeout_secs = 600
max_archive_depth = 3
```

| Option                       | Type             | Default       | Description                                                                                  |
| ---------------------------- | ---------------- | ------------- | -------------------------------------------------------------------------------------------- |
| `use_cache`                  | boolean          | `true`        | Enable caching of extraction results                                                         |
| `enable_quality_processing`  | boolean          | `true`        | Enable quality post-processing                                                               |
| `force_ocr`                  | boolean          | `false`       | Force OCR even for searchable PDFs                                                            |
| `force_ocr_pages`            | array of int     | unset         | Force OCR only on these 1-indexed pages (PDF only). Ignored when `force_ocr = true`          |
| `disable_ocr`                | boolean          | `false`       | Disable OCR entirely, even for images (cannot be `true` together with `force_ocr`)           |
| `output_format`              | string           | `"plain"`     | Content render format: `plain`, `markdown`, `djot`, `html`, `json`, `structured`             |
| `result_format`              | string           | `"unified"`   | Result shape: `unified` or `element_based`                                                   |
| `extraction_timeout_secs`    | int \| null      | `600`         | Per-file timeout in seconds for batch extraction (10 minutes). `null` disables the timeout   |
| `max_concurrent_extractions` | int \| null      | `null`        | Max concurrent extractions (a ceiling within the thread budget). `null` = resolved thread budget = `min(num_cpus, 8)` (or the configured `concurrency.max_threads`) |
| `max_embedded_file_bytes`    | int \| null      | `52428800`    | Max uncompressed size (bytes) of a single embedded file before recursive extraction (50 MiB) |
| `max_archive_depth`          | int              | `3`           | Max recursion depth for archive extraction. `0` disables recursive extraction               |
| `use_layout_for_markdown`    | boolean          | `false`       | Use layout detection on the non-OCR PDF markdown path (requires `layout` set)               |
| `include_document_structure` | boolean          | `false`       | Populate the structured `document` tree on the result                                        |
| `cache_namespace`            | string \| null   | `null`        | Cache namespace for tenant isolation (alphanumeric, `-`, `_`; max 64 chars)                  |
| `cache_ttl_secs`             | int \| null      | `null`        | Per-request cache TTL in seconds. `0` skips the cache entirely                               |
| `qr_codes`                   | boolean \| null  | `null`        | Enable QR-code detection in extracted images                                                 |

Nested sections (each `None`/unset by default unless noted): `ocr`, `chunking`, `content_filter`, `images`, `pdf_options`, `token_reduction`, `language_detection`, `pages`, `keywords`, `postprocessor`, `html_output`, `security_limits`, `layout`, `transcription`, `acceleration`, `email`, `tree_sitter`, `structured_extraction`, `ner`, `redaction`, `summarization`, `translation`, `page_classification`, `captioning`, and `url`. The `url` section always has a default (it is not optional). The sections documented below are the ones most commonly set in config files.

### OCR Configuration

```toml
[ocr]
enabled = true
backend = "tesseract"
language = ["eng"]
auto_rotate = false
```

| Option         | Type             | Default     | Description                                                            |
| -------------- | ---------------- | ----------- | --------------------------------------------------------------------- |
| `enabled`      | boolean          | `true`      | Whether OCR is enabled. Setting `false` is equivalent to `disable_ocr` |
| `backend`      | string           | `"tesseract"` | OCR backend: `tesseract`, `paddleocr`/`paddle-ocr`, or `vlm`         |
| `language`     | string or array  | `["eng"]`   | Language code(s). Accepts `"eng"` or `["eng", "deu"]`. Tesseract joins with `+` |
| `auto_rotate`  | boolean          | `false`     | Enable automatic page rotation based on orientation detection         |
| `vlm_fallback` | string           | `"disabled"` | VLM fallback policy                                                   |
| `output_format`| string \| null   | `null`      | Optional OCR output format override                                    |

Additional `[ocr]` fields exist for advanced use: `tesseract_config`, `paddle_ocr_config`, `backend_options`, `element_config`, `quality_thresholds`, `pipeline`, `vlm_config`, `vlm_prompt`, `acceleration`, `tessdata_bytes`, `tessdata_path`.

#### Tesseract Configuration

```toml
[ocr.tesseract_config]
language = ["eng"]
psm = 3
oem = 3
output_format = "markdown"
min_confidence = 0.0
enable_table_detection = true
table_min_confidence = 0.0
table_column_threshold = 50
table_row_threshold_ratio = 0.5
use_cache = true
```

| Option                      | Type            | Default      | Description                                                  |
| --------------------------- | --------------- | ------------ | ------------------------------------------------------------ |
| `language`                  | array           | `["eng"]`    | Language code(s); joined with `+` for Tesseract              |
| `psm`                       | integer         | `3`          | Page Segmentation Mode (0–13). WASM target defaults to `6`   |
| `oem`                       | integer         | `3`          | OCR Engine Mode (0–3)                                        |
| `output_format`             | string          | `"markdown"` | OCR output format (`text` or `markdown`)                     |
| `min_confidence`            | float           | `0.0`        | Minimum OCR confidence threshold (0.0–100.0)                 |
| `enable_table_detection`    | boolean         | `true`       | Enable table detection during OCR                           |
| `table_min_confidence`      | float           | `0.0`        | Minimum confidence for table cells (0.0–1.0)                 |
| `table_column_threshold`    | integer         | `50`         | Pixel threshold for column detection                        |
| `table_row_threshold_ratio` | float           | `0.5`        | Row height ratio threshold (0.0–1.0)                        |
| `use_cache`                 | boolean         | `true`       | Cache OCR results                                            |
| `preprocessing`             | table \| null   | `null`       | Image preprocessing (see below)                             |

Advanced Tesseract engine knobs (all with the listed defaults): `classify_use_pre_adapted_templates` (`true`), `language_model_ngram_on` (`false`), `tessedit_dont_blkrej_good_wds` (`true`), `tessedit_dont_rowrej_good_wds` (`true`), `tessedit_enable_dict_correction` (`true`), `tessedit_char_whitelist` (`""`), `tessedit_char_blacklist` (`""`), `tessedit_use_primary_params_model` (`true`), `textord_space_size_is_variable` (`true`), `thresholding_method` (`false`).

#### Tesseract Preprocessing

```toml
[ocr.tesseract_config.preprocessing]
target_dpi = 300
auto_rotate = false
deskew = true
denoise = false
contrast_enhance = false
binarization_method = "otsu"
invert_colors = false
```

| Option                | Type    | Default  | Description                                    |
| --------------------- | ------- | -------- | ---------------------------------------------- |
| `target_dpi`          | integer | `300`    | Target DPI (300 standard, 600 for small text)  |
| `auto_rotate`         | boolean | `false`  | Auto-detect and correct image rotation         |
| `deskew`              | boolean | `true`   | Correct skewed (tilted) images                 |
| `denoise`             | boolean | `false`  | Remove noise from images                       |
| `contrast_enhance`    | boolean | `false`  | Enhance image contrast                         |
| `binarization_method` | string  | `"otsu"` | Binarization method: `otsu`, `sauvola`, `adaptive` |
| `invert_colors`       | boolean | `false`  | Invert image colors                            |

### PDF Options

```toml
[pdf_options]
extract_images = false
extract_tables = true
extract_metadata = true
extract_form_fields = true

[pdf_options.hierarchy]
enabled = true
k_clusters = 3
include_bbox = true
```

| Option                             | Type            | Default | Description                                                              |
| ---------------------------------- | --------------- | ------- | ----------------------------------------------------------------------- |
| `extract_images`                   | boolean         | `false` | Extract images from PDF documents                                       |
| `extract_tables`                   | boolean         | `true`  | Extract tables from PDF                                                  |
| `extract_metadata`                 | boolean         | `true`  | Extract PDF metadata                                                     |
| `extract_form_fields`              | boolean         | `true`  | Extract AcroForm / XFA form fields                                      |
| `extract_annotations`              | boolean         | `false` | Extract annotations (notes, highlights, links, stamps)                 |
| `passwords`                        | array \| null   | `null`  | Passwords to try when opening encrypted PDFs                            |
| `top_margin_fraction`              | float \| null   | `null`  | Top margin fraction (0.0–1.0) to exclude headers. Effective default 0.06 |
| `bottom_margin_fraction`           | float \| null   | `null`  | Bottom margin fraction (0.0–1.0) to exclude footers. Effective default 0.05 |
| `allow_single_column_tables`       | boolean         | `false` | Allow single-column pseudo-tables                                      |
| `ocr_inline_images`                | boolean         | `false` | OCR inline images extracted from PDF pages                             |
| `reading_order`                    | boolean         | `false` | Reorder text by layout-detected reading order                         |
| `hierarchy`                        | table \| null   | `null`  | Hierarchy extraction config (see below)                               |

#### PDF Hierarchy

| Option                   | Type          | Default | Description                                          |
| ------------------------ | ------------- | ------- | ---------------------------------------------------- |
| `enabled`                | boolean       | `true`  | Enable hierarchy extraction                          |
| `k_clusters`             | integer       | `3`     | Number of font-size clusters for hierarchy (1–7)     |
| `include_bbox`           | boolean       | `true`  | Include bounding boxes in hierarchy blocks           |

### Image Processing

The `[images]` section configures `ImageExtractionConfig` (None = no image extraction).

```toml
[images]
extract_images = true
target_dpi = 300
max_image_dimension = 4096
auto_adjust_dpi = true
min_dpi = 72
max_dpi = 600
run_ocr_on_images = true
inject_placeholders = true
```

| Option                | Type           | Default    | Description                                              |
| --------------------- | -------------- | ---------- | ------------------------------------------------------- |
| `extract_images`      | boolean        | `true`     | Extract images from documents                           |
| `target_dpi`          | integer        | `300`      | Target DPI for image normalization                     |
| `max_image_dimension` | integer        | `4096`     | Maximum image dimension (width or height) in pixels    |
| `inject_placeholders` | boolean        | `true`     | Inject `![Image](embedded:...)` placeholders into markdown |
| `auto_adjust_dpi`     | boolean        | `true`     | Automatically adjust DPI based on image content        |
| `min_dpi`             | integer        | `72`       | Minimum DPI threshold                                   |
| `max_dpi`             | integer        | `600`      | Maximum DPI threshold                                   |
| `max_images_per_page` | int \| null    | `null`     | Cap on image objects per PDF page (`null` = no limit)  |
| `classify`            | boolean        | `false`    | Classify and cluster extracted images by figure        |
| `include_page_rasters`| boolean        | `false`    | Capture full-page renders as `PageRaster` images (PDF + OCR) |
| `run_ocr_on_images`   | boolean        | `true`     | OCR extracted images and include the text in content   |
| `ocr_text_only`       | boolean        | `false`    | Render image OCR as plain text without the markdown placeholder |
| `append_ocr_text`     | boolean        | `false`    | Append OCR text after the image placeholder            |
| `output_format`       | string/table   | `"native"` | Re-encode format: `native`, `png`, `jpeg`, `webp` (tagged by `type`) |
| `include_data_base64` | boolean        | `false`    | Populate `ExtractedImage.data_base64`                  |

### Chunking Configuration

```toml
[chunking]
max_characters = 1000
overlap = 200
trim = true
chunker_type = "text"
table_chunking = "split"

[chunking.embedding]
batch_size = 32
normalize = true
show_download_progress = false

[chunking.embedding.model]
type = "preset"
name = "balanced"
```

| Option                             | Type            | Default     | Description                                                              |
| ---------------------------------- | --------------- | ----------- | ----------------------------------------------------------------------- |
| `max_characters`                   | integer         | `1000`      | Maximum size per chunk (units determined by `sizing`)                   |
| `overlap`                          | integer         | `200`       | Overlap between consecutive chunks (units determined by `sizing`)       |
| `trim`                             | boolean         | `true`      | Trim whitespace from chunk boundaries                                   |
| `chunker_type`                     | string          | `"text"`    | Chunker: `text`, `markdown`, `yaml`, `semantic`                         |
| `preset`                           | string \| null  | `null`      | Use a preset config (overrides individual settings when set)           |
| `sizing`                           | table \| null   | `null`      | How to measure chunk size; default `characters`. `tokenizer` requires the `chunking-tokenizers` feature |
| `prepend_heading_context`          | boolean         | `false`     | For `markdown` chunker, prepend the heading hierarchy path to each chunk |
| `topic_threshold`                  | float \| null   | `null`      | Cosine similarity threshold for `semantic` chunker (effective default 0.75) |
| `table_chunking`                   | string          | `"split"`   | Oversized markdown tables: `split` or `repeat_header`                   |
| `embedding`                        | table \| null   | `null`      | Embedding configuration (see below)                                    |

#### Embedding

```toml
[chunking.embedding]
normalize = true
batch_size = 32
show_download_progress = false
```

| Option                   | Type           | Default | Description                                                       |
| ------------------------ | -------------- | ------- | ----------------------------------------------------------------- |
| `model`                  | table          | preset `gte-modernbert-base` | Embedding model (tagged enum, see below). Defaults to the `gte-modernbert-base` preset (`balanced` is still a selectable preset) |
| `normalize`              | boolean        | `true`  | Normalize embedding vectors (recommended for cosine similarity)   |
| `batch_size`             | integer        | `32`    | Batch size for embedding generation                               |
| `show_download_progress` | boolean        | `false` | Show model download progress                                      |
| `cache_dir`              | string \| null | `null`  | Custom cache directory (defaults to `~/.cache/xberg/embeddings/`) |
| `acceleration`           | table \| null  | `null`  | Hardware acceleration for the embedding ONNX model               |
| `max_embed_duration_secs`| int \| null    | `60`    | Per-`embed()` timeout for the `plugin` model variant             |

The `[chunking.embedding.model]` table is a tagged enum keyed on `type`:

| `type`     | Fields                          | Description                                            |
| ---------- | ------------------------------- | ----------------------------------------------------- |
| `preset`   | `name` (e.g. `"balanced"`)      | Built-in preset model (recommended)                   |
| `custom`   | `model_id`, `dimensions`        | Custom ONNX model from HuggingFace                    |
| `llm`      | `llm` (LlmConfig)               | Provider-hosted embedding model via liter-llm         |
| `plugin`   | `name`                          | In-process backend registered via the plugin system  |

### Keywords Configuration

```toml
[keywords]
algorithm = "yake"
max_keywords = 10
min_score = 0.0
language = "en"
```

| Option         | Type            | Default  | Description                                              |
| -------------- | --------------- | -------- | ------------------------------------------------------- |
| `algorithm`    | string          | `"yake"` | Keyword extraction algorithm: `yake` or `rake`          |
| `max_keywords` | integer         | `10`     | Maximum keywords to extract                             |
| `min_score`    | float           | `0.0`    | Minimum relevance score (range is algorithm-specific)  |
| `ngram_range`  | array `[min,max]` | `[1, 3]` | N-gram range. Config-file/Rust-only — `alef(skip)` so it is absent from the generated language-binding dataclasses |
| `language`     | string \| null  | `"en"`   | Language code for stopword filtering (`null` = no filtering) |
| `yake_params`  | table \| null   | `null`   | YAKE tuning: `window_size` (default `2`)               |
| `rake_params`  | table \| null   | `null`   | RAKE tuning: `min_word_length` (`1`), `max_words_per_phrase` (`3`) |

### Token Reduction

```toml
[token_reduction]
mode = "off"
preserve_important_words = true
```

| Option                     | Type    | Default | Description                                              |
| -------------------------- | ------- | ------- | ------------------------------------------------------- |
| `mode`                     | string  | `"off"` | Mode: `off`, `light`, `moderate`, `aggressive`, `maximum` |
| `preserve_important_words` | boolean | `true`  | Preserve important words (capitalized, technical terms) |

### Language Detection

```toml
[language_detection]
enabled = true
min_confidence = 0.8
detect_multiple = false
```

| Option            | Type    | Default | Description                                |
| ----------------- | ------- | ------- | ------------------------------------------ |
| `enabled`         | boolean | `true`  | Enable automatic language detection        |
| `min_confidence`  | float   | `0.8`   | Minimum confidence threshold for detection |
| `detect_multiple` | boolean | `false` | Detect multiple languages in the document  |

### Content Filter

```toml
[content_filter]
include_headers = false
include_footers = false
strip_repeating_text = true
include_watermarks = false
```

| Option                 | Type    | Default | Description                                         |
| ---------------------- | ------- | ------- | -------------------------------------------------- |
| `include_headers`      | boolean | `false` | Include running headers in output                  |
| `include_footers`      | boolean | `false` | Include running footers in output                  |
| `strip_repeating_text` | boolean | `true`  | Enable the cross-page repeating-text detector      |
| `include_watermarks`   | boolean | `false` | Include watermark text in output                   |

### Post-Processor

```toml
[postprocessor]
enabled = true
```

| Option                | Type           | Default | Description                                     |
| --------------------- | -------------- | ------- | ----------------------------------------------- |
| `enabled`             | boolean        | `true`  | Enable post-processors                          |
| `enabled_processors`  | array \| null  | `null`  | Whitelist of processor names (`null` = all)     |
| `disabled_processors` | array \| null  | `null`  | Blacklist of processor names (`null` = none)    |

### Security Limits

```toml
[security_limits]
max_archive_size = 524288000
max_compression_ratio = 100
max_files_in_archive = 10000
```

| Option                  | Type    | Default      | Description                                          |
| ----------------------- | ------- | ------------ | --------------------------------------------------- |
| `max_archive_size`      | integer | `524288000`  | Max uncompressed archive size (500 MB)              |
| `max_compression_ratio` | integer | `100`        | Max compression ratio before flagging a bomb (100:1) |
| `max_files_in_archive`  | integer | `10000`      | Max files in an archive                             |
| `max_nesting_depth`     | integer | `1024`       | Max nesting depth for structures                    |
| `max_entity_length`     | integer | `1048576`    | Max length of a single XML entity/token (1 MiB)     |
| `max_content_size`      | integer | `104857600`  | Max string growth per document (100 MB)             |
| `max_iterations`        | integer | `10000000`   | Max iterations per operation                        |
| `max_xml_depth`         | integer | `1024`       | Max XML depth                                       |
| `max_table_cells`       | integer | `100000`     | Max cells per table                                 |

### URL Ingestion

```toml
[url]
mode = "auto"
allow_local_file_inputs = true
allow_file_uris = true
```

| Option                         | Type           | Default  | Description                                          |
| ------------------------------ | -------------- | -------- | --------------------------------------------------- |
| `mode`                         | string         | `"auto"` | URL extraction mode: `auto`, `document`, `crawl`    |
| `document_url_pattern`         | string \| null | `null`   | Regex filter for document-discovered URLs           |
| `max_document_urls_per_result` | int \| null    | `100`    | Max URLs followed per extraction result             |
| `max_total_urls`               | int \| null    | `1000`   | Max URLs followed across the whole call             |
| `allow_local_file_inputs`      | boolean        | `true`   | Allow bare local filesystem path inputs             |
| `allow_file_uris`              | boolean        | `true`   | Allow local `file://` URI inputs                    |

## Per-Input Configuration (`ExtractInput.config`)

Per-input overrides are no longer a separate batch function. Each `ExtractInput` carries an optional `config` field of type `FileExtractionConfig` — every field is `Option<T>` (`None` = use the batch-level default). Overrides are merged onto the batch `ExtractionConfig` per input: `Some(value)` replaces the batch default; `None` falls through.

```python
from xberg import extract_batch, ExtractInput, FileExtractionConfig

result = await extract_batch(
    [
        ExtractInput(uri="scan.pdf", config=FileExtractionConfig(force_ocr=True)),
        ExtractInput(uri="report.docx"),  # uses batch defaults
    ],
    config={"chunking": {"max_characters": 1000}},  # batch-level default
)
```

**Overridable fields:** `enable_quality_processing`, `ocr`, `force_ocr`, `force_ocr_pages`, `disable_ocr`, `chunking`, `content_filter`, `images`, `pdf_options`, `token_reduction`, `language_detection`, `pages`, `keywords`, `postprocessor`, `html_output` (and `html_options`), `result_format`, `output_format`, `include_document_structure`, `layout`, `transcription`, `timeout_secs`, `tree_sitter`, `structured_extraction`, `url`, `ner`, `redaction`, `summarization`, `translation`, `page_classification`, `captioning`, `qr_codes`.

**Batch-level only (not overridable per input):** `max_concurrent_extractions`, `use_cache`, `acceleration`, `security_limits`.

`FileExtractionConfig` is a programmatic, per-input API. It cannot be expressed in a standalone config file — config files describe the batch-level `ExtractionConfig`.

## Naming Conventions

| Context              | Convention | Example                                                |
| -------------------- | ---------- | ----------------------------------------------------- |
| Python               | snake_case | `max_characters`, `pdf_options`, `use_cache`          |
| Node.js / TypeScript | camelCase  | `maxCharacters`, `pdfOptions`, `useCache`             |
| Rust                 | snake_case | `max_characters`, `pdf_options`, `use_cache`          |
| TOML / YAML / JSON   | snake_case | `max_characters`, `pdf_options`, `use_cache`          |
| CLI flags            | kebab-case | `--output-format`, `--config`                         |

When switching between languages:

- Python → Node.js: `snake_case` to `camelCase`
- TOML → Python: no conversion needed (both `snake_case`)

## Environment Variables

`ExtractionConfig::apply_env_overrides()` applies these at runtime (highest precedence). Unset variables are ignored; invalid values raise a validation error.

| Variable                       | Maps to                                   | Notes                                              |
| ------------------------------ | ----------------------------------------- | -------------------------------------------------- |
| `XBERG_OCR_LANGUAGE`           | `ocr.language`                            | ISO 639 code, e.g. `eng`, `fra`, `deu`             |
| `XBERG_OCR_BACKEND`            | `ocr.backend`                             | `tesseract`, `paddleocr`/`paddle-ocr`, `vlm`       |
| `XBERG_CHUNKING_MAX_CHARS`     | `chunking.max_characters`                 | Positive integer (env var name keeps `MAX_CHARS`)  |
| `XBERG_CHUNKING_MAX_OVERLAP`   | `chunking.overlap`                        | Non-negative integer                               |
| `XBERG_CHUNKING_TOKENIZER`     | `chunking.sizing` (tokenizer)             | Requires the `chunking-tokenizers` feature         |
| `XBERG_CACHE_ENABLED`          | `use_cache`                               | `true` / `false`                                   |
| `XBERG_DISABLE_OCR`            | `disable_ocr`                             | `true`/`1` or `false`/`0`                           |
| `XBERG_TOKEN_REDUCTION_MODE`   | `token_reduction.mode`                    | `off`, `light`, `moderate`, `aggressive`, `maximum` |
| `XBERG_OUTPUT_FORMAT`          | `output_format`                           | `plain`, `markdown`, `djot`, `html`, ...           |
| `XBERG_LLM_MODEL`              | `structured_extraction.llm.model`         | e.g. `openai/gpt-4o`                               |
| `XBERG_LLM_API_KEY`            | `structured_extraction.llm.api_key`       |                                                    |
| `XBERG_LLM_BASE_URL`           | `structured_extraction.llm.base_url`      |                                                    |
| `XBERG_VLM_OCR_MODEL`          | `ocr.vlm_config.model`                     | Vision-model OCR                                    |
| `XBERG_VLM_EMBEDDING_MODEL`    | `chunking.embedding.model` (llm variant)  | Mutually exclusive with `XBERG_EMBEDDING_PLUGIN_NAME` |
| `XBERG_EMBEDDING_PLUGIN_NAME`  | `chunking.embedding.model` (plugin variant) | Mutually exclusive with `XBERG_VLM_EMBEDDING_MODEL` |
| `XBERG_HOST`                   | server bind address (serve command)       | e.g. `127.0.0.1`                                   |
| `XBERG_PORT`                   | server port (serve command)               | e.g. `8080`                                        |

## Configuration Merging

For the CLI, sources are merged in priority order (highest to lowest):

1. **Individual CLI flags** (e.g. `--ocr`, `--output-format`)
2. **Inline JSON config** (`--config-json` / `--config-json-base64`) — field-level merge
3. **Configuration file** (`--config path`)
4. **Auto-discovered config** (`xberg.toml` in cwd/parents)
5. **Defaults**

Environment variable overrides (`apply_env_overrides`) are applied on top of the loaded config.

## Example Configurations

### Minimal Configuration

```toml
use_cache = true
enable_quality_processing = true

[ocr]
backend = "tesseract"
language = ["eng"]
```

### High-Quality PDF Extraction

```toml
use_cache = true
force_ocr = false

[ocr]
backend = "tesseract"
language = ["eng"]

[ocr.tesseract_config]
psm = 3
oem = 3
enable_table_detection = true
table_min_confidence = 0.7

[pdf_options]
extract_images = true
extract_tables = true
extract_metadata = true

[pdf_options.hierarchy]
enabled = true
k_clusters = 3

[images]
extract_images = true
target_dpi = 300
```

### Semantic Search Configuration

```toml
[chunking]
max_characters = 800
overlap = 150
chunker_type = "markdown"

[chunking.embedding]
batch_size = 32
normalize = true

[chunking.embedding.model]
type = "preset"
name = "quality"

[keywords]
algorithm = "yake"
max_keywords = 15
```

## Field Name Reference

Critical config-file keys (verify against the source structs when adding options):

- `max_characters` (chunk size; default `1000`)
- `overlap` (chunk overlap; default `200`)
- `chunker_type`, `table_chunking`, `sizing`
- `enable_table_detection`, `table_min_confidence`, `table_column_threshold`, `table_row_threshold_ratio`
- `k_clusters`, `include_bbox`
- `auto_rotate`, `auto_adjust_dpi`, `inject_placeholders`
- `show_download_progress`, `normalize`, `batch_size`
- `min_confidence`, `detect_multiple`
- `extract_tables`, `extract_form_fields`, `extract_annotations`

Always verify field names and defaults against the Rust config structs in `crates/xberg/src/core/config/` and the generated `packages/python/xberg/options.py`.
