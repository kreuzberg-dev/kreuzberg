<!--
AI-RULEZ :: GENERATED FILE — DO NOT EDIT
Content-Hash: blake3:ab4e073238ce8da179601b4d98b1aa72c0ff620e9f1b1bb3570fe0f1e162e11a
Source-Hash: blake3:c3730974b95d68f86536f7c420114fdefdaf54253473d6bec3cfefe6dacc7d4d
Schema-Version: v1
-->

# Xberg Python API Reference

Comprehensive documentation for the Xberg Python API. All extraction logic and heavy lifting is implemented in a high-performance Rust core; Python adds custom plugin support (post-processors, validators, OCR/embedding backends).

Install with `pip install xberg`.

The Python API is **async-only**. There are exactly two entry points — `extract` and `extract_batch` — and both are coroutines that must be awaited. There are no `extract_file` / `extract_bytes` / `extract_file_sync` / `batch_extract_files` functions.

## Extraction Functions

### `extract` (async)

```python
async def extract(
    input: ExtractInput | None = None,
    config: ExtractionConfig | None = None,
) -> ExtractionResult
```

Extract content from a single input (a local path, `file://` URI, HTTP(S) URL, or raw bytes). Returns an `ExtractionResult` **envelope** — the extracted document(s) live in `result.results`.

**Parameters:**

- `input` (ExtractInput | None): The input to extract. Construct with `ExtractInput(uri=...)` or `ExtractInput(kind="bytes", ...)`.
- `config` (ExtractionConfig | None): Extraction configuration (uses defaults if None).

**Returns:** `ExtractionResult` with `results`, `errors`, and `summary`.

**Example:**

```python
import asyncio
from xberg import ExtractInput, extract, ExtractionConfig

async def main() -> None:
    result = await extract(ExtractInput(uri="document.pdf"), ExtractionConfig())
    doc = result.results[0]
    print(doc.content)
    print(doc.metadata)
    print(doc.tables)

asyncio.run(main())
```

### `extract_batch` (async)

```python
async def extract_batch(
    inputs: list[ExtractInput],
    config: ExtractionConfig | None = None,
) -> ExtractionResult
```

Extract content from multiple inputs in parallel. Returns a single `ExtractionResult` envelope whose `results` list contains one `ExtractedDocument` per input (in input order), plus any per-input `errors`.

**Example:**

```python
import asyncio
from xberg import ExtractInput, extract_batch

async def main() -> None:
    inputs = [
        ExtractInput(kind="uri", uri="document.pdf"),
        ExtractInput(
            kind="bytes",
            bytes=b"Hello from memory",
            mime_type="text/plain",
            filename="note.txt",
        ),
    ]
    output = await extract_batch(inputs)
    for doc in output.results:
        print(doc.content[:200])
    print(f"Inputs: {output.summary.inputs}, results: {output.summary.results}, errors: {output.summary.errors}")

asyncio.run(main())
```

## Input Construction

### ExtractInput

Unified input for both entry points.

**Fields:**

| Field       | Type                     | Description                                                       |
| ----------- | ------------------------ | ---------------------------------------------------------------- |
| `kind`      | `"uri"` \| `"bytes"`     | Source kind. `"uri"` requires `uri`; `"bytes"` requires `bytes`. |
| `uri`       | str \| None              | Local path, `file://` URI, or HTTP(S) URL (for `kind="uri"`).    |
| `bytes`     | bytes \| None            | Raw bytes (for `kind="bytes"`).                                  |
| `mime_type` | str \| None              | Optional MIME type hint.                                         |
| `filename`  | str \| None              | Filename hint used for MIME detection and metadata.             |
| `config`    | FileExtractionConfig \| None | Per-input extraction overrides.                              |

**Construction:**

```python
from xberg import ExtractInput

# From a path, file:// URI, or URL
ExtractInput(uri="document.pdf")
ExtractInput(uri="https://example.com/report.pdf")

# From raw bytes (mime_type or filename helps detection)
ExtractInput(kind="bytes", bytes=b"%PDF-1.4 ...", mime_type="application/pdf", filename="document.pdf")
```

## Configuration Classes

### ExtractionConfig

Main extraction configuration. All fields are optional and use sensible defaults when omitted. Construct with keyword arguments.

**Common fields:**

| Field                       | Type                              | Default     | Description                                                          |
| --------------------------- | --------------------------------- | ----------- | ------------------------------------------------------------------- |
| `use_cache`                 | bool                              | True        | Cache extraction results to speed up repeated extractions.          |
| `enable_quality_processing` | bool                              | True        | Quality post-processing to clean and normalize extracted text.      |
| `ocr`                       | OcrConfig \| None                 | None        | OCR configuration. None = OCR disabled.                             |
| `force_ocr`                 | bool                              | False       | Force OCR even for searchable PDFs that contain extractable text.   |
| `chunking`                  | ChunkingConfig \| None            | None        | Text chunking configuration. None = disabled.                      |
| `images`                    | ImageExtractionConfig \| None     | None        | Extract images FROM documents. None = no image extraction.         |
| `pdf_options`               | PdfConfig \| None                 | None        | PDF-specific options.                                              |
| `token_reduction`           | TokenReductionOptions \| None     | None        | Reduce token count in extracted content. None = no reduction.       |
| `language_detection`        | LanguageDetectionConfig \| None   | None        | Detect document language(s).                                       |
| `keywords`                  | KeywordConfig \| None             | None        | Keyword extraction.                                                |
| `pages`                     | PageConfig \| None                | None        | Per-page content extraction and tracking.                          |
| `postprocessor`             | PostProcessorConfig \| None       | None        | Post-processor pipeline configuration.                            |
| `html_output`               | HtmlOutputConfig \| None          | None        | Styled HTML output configuration (when `output_format="html"`).    |
| `max_concurrent_extractions`| int \| None                       | num_cpus×1.5| Maximum concurrent extractions in batch operations.               |
| `result_format`             | `"unified"` \| `"element_based"`  | "unified"   | Result structure format.                                          |
| `output_format`             | `"plain"` \| `"markdown"` \| `"djot"` \| `"html"` | "plain" | Content text format.                          |
| `security_limits`           | SecurityLimits \| None            | None        | Security limits for archive extraction.                            |

**Example:**

```python
from xberg import ExtractInput, extract, ExtractionConfig, OcrConfig, ChunkingConfig

config = ExtractionConfig(
    use_cache=True,
    enable_quality_processing=True,
    output_format="markdown",
    ocr=OcrConfig(backend="tesseract", language="eng"),
    chunking=ChunkingConfig(max_characters=1000, overlap=200),
)
result = await extract(ExtractInput(uri="document.pdf"), config)
```

### FileExtractionConfig

Per-input extraction overrides, set on `ExtractInput.config`. All fields are `Optional` — `None` means "use the batch/default config value". Mirrors `ExtractionConfig` minus batch-level concerns (`max_concurrent_extractions`, `use_cache`, `acceleration`, `security_limits`).

```python
from xberg import ExtractInput, FileExtractionConfig, OcrConfig

scanned = ExtractInput(
    kind="uri",
    uri="scanned.pdf",
    config=FileExtractionConfig(
        force_ocr=True,
        ocr=OcrConfig(backend="tesseract", language="deu"),
    ),
)
```

### OcrConfig

OCR configuration for extracting text from images and scanned pages.

| Field              | Type                    | Default     | Description                                                                                  |
| ------------------ | ----------------------- | ----------- | ------------------------------------------------------------------------------------------- |
| `enabled`          | bool                    | True        | Whether OCR is enabled.                                                                      |
| `backend`          | str                     | `""`        | OCR backend: `"tesseract"`, `"paddleocr"`, `"paddle-ocr"`, or `"vlm"`.                       |
| `language`         | str \| list[str]        | `["eng"]`   | Language code(s). Accepts a single code (`"eng"`), a `+`-joined string (`"eng+fra"`), or a list (`["eng", "deu"]`). |
| `tesseract_config` | TesseractConfig \| None | None        | Tesseract-specific configuration.                                                            |

```python
from xberg import OcrConfig, TesseractConfig

OcrConfig(backend="tesseract", language="deu")
OcrConfig(backend="tesseract", language=["eng", "fra"], tesseract_config=TesseractConfig(psm=6))
OcrConfig(backend="paddleocr", language="chinese")
```

### TesseractConfig

Detailed Tesseract OCR tuning. Common fields:

| Field                    | Type                             | Default | Description                                                                                |
| ------------------------ | -------------------------------- | ------- | ----------------------------------------------------------------------------------------- |
| `psm`                    | int                              | 3       | Page Segmentation Mode: 0 (detection only), 3 (auto), 6 (uniform block), 11 (sparse text). |
| `oem`                    | int                              | 3       | OCR Engine Mode: 0 (legacy), 1 (LSTM), 2 (both), 3 (auto).                                 |
| `min_confidence`         | float                            | 0.0     | Minimum confidence threshold (0.0–1.0).                                                    |
| `preprocessing`          | ImagePreprocessingConfig \| None | None    | Image preprocessing before OCR.                                                            |
| `enable_table_detection` | bool                             | True    | Detect and extract tables from OCR output.                                                 |
| `tessedit_char_whitelist`| str                              | `""`    | Whitelist of characters to recognize (empty = all).                                        |

```python
from xberg import TesseractConfig, ImagePreprocessingConfig

TesseractConfig(psm=6, oem=1, min_confidence=0.8, enable_table_detection=True)
TesseractConfig(preprocessing=ImagePreprocessingConfig(target_dpi=300, denoise=True))
```

### ChunkingConfig

Text chunking for RAG, indexing, and length-limited systems.

| Field                     | Type                      | Default   | Description                                                                  |
| ------------------------- | ------------------------- | --------- | -------------------------------------------------------------------------- |
| `max_characters`          | int                       | 1000      | Maximum size per chunk (in units determined by `sizing`).                  |
| `overlap`                 | int                       | 200       | Overlap between consecutive chunks.                                         |
| `trim`                    | bool                      | True      | Trim whitespace from chunk boundaries.                                      |
| `chunker_type`            | `"text"` \| `"markdown"` \| `"semantic"` | "text" | Chunking strategy.                                          |
| `embedding`               | EmbeddingConfig \| None   | None      | Generate an embedding per chunk. None = no embeddings.                      |
| `preset`                  | str \| None               | None      | Named preset (overrides individual settings if provided).                  |
| `prepend_heading_context` | bool                      | False     | (Markdown chunker) prepend the heading breadcrumb to each chunk's content. |

> **IMPORTANT:** The fields are `max_characters` and `overlap` (NOT `max_chars`/`max_overlap` or `max_chars`/`max_chars`).

```python
from xberg import ExtractionConfig, ChunkingConfig, EmbeddingConfig, EmbeddingModelType

# Defaults (1000 / 200)
ExtractionConfig(chunking=ChunkingConfig())

# Custom size and overlap
ExtractionConfig(chunking=ChunkingConfig(max_characters=512, overlap=100))

# Markdown chunker with embeddings
ExtractionConfig(
    chunking=ChunkingConfig(
        chunker_type="markdown",
        max_characters=512,
        overlap=50,
        embedding=EmbeddingConfig(model=EmbeddingModelType.preset("balanced")),
    )
)
```

### EmbeddingConfig

Embedding generation for chunks using ONNX models.

| Field                    | Type                       | Default            | Description                                              |
| ------------------------ | -------------------------- | ------------------ | ------------------------------------------------------- |
| `model`                  | EmbeddingModelType \| None | "balanced" preset  | The embedding model to use.                             |
| `normalize`              | bool                       | True               | Normalize vectors to unit length (cosine similarity).  |
| `batch_size`             | int                        | 32                 | Texts to embed per batch.                              |
| `show_download_progress` | bool                       | False              | Show model download progress.                          |
| `cache_dir`              | str \| None                | None               | Custom model cache directory.                          |

### EmbeddingModelType

Embedding model selector. Static constructors:

```python
@staticmethod
def preset(name: str) -> EmbeddingModelType    # "fast", "balanced", "quality", "multilingual"

@staticmethod
def custom(model_id: str, dimensions: int) -> EmbeddingModelType   # any ONNX model on HuggingFace

@staticmethod
def llm(llm) -> EmbeddingModelType   # an LLM-backed embedding model

@staticmethod
def plugin(name: str) -> EmbeddingModelType   # a registered embedding backend
```

```python
from xberg import EmbeddingConfig, EmbeddingModelType

EmbeddingConfig(model=EmbeddingModelType.preset("balanced"))
EmbeddingConfig(model=EmbeddingModelType.custom(model_id="BAAI/bge-small-en-v1.5", dimensions=384))
```

Presets: `fast` (384 dims), `balanced` (768 dims), `quality` (1024 dims), `multilingual` (768 dims).

### KeywordConfig

Keyword extraction (YAKE or RAKE).

| Field          | Type                              | Default | Description                                |
| -------------- | --------------------------------- | ------- | ------------------------------------------ |
| `algorithm`    | KeywordAlgorithm \| str           | YAKE    | `KeywordAlgorithm.YAKE` or `.RAKE` (or `"yake"`/`"rake"`). |
| `max_keywords` | int                               | 10      | Maximum number of keywords to extract.     |
| `min_score`    | float                             | 0.0     | Minimum score threshold.                   |
| `language`     | str \| None                       | "en"    | Language hint.                             |
| `yake_params`  | YakeParams \| None                | None    | YAKE tuning (`window_size`).               |
| `rake_params`  | RakeParams \| None                | None    | RAKE tuning (`min_word_length`, `max_words_per_phrase`). |

```python
from xberg import ExtractionConfig, KeywordConfig, KeywordAlgorithm, YakeParams

ExtractionConfig(
    keywords=KeywordConfig(
        algorithm=KeywordAlgorithm.YAKE,
        max_keywords=10,
        min_score=0.1,
        language="en",
        yake_params=YakeParams(window_size=1),
    )
)
```

### PdfConfig

| Field              | Type                    | Default | Description                                       |
| ------------------ | ----------------------- | ------- | ------------------------------------------------- |
| `extract_images`   | bool                    | False   | Extract images from PDF documents.                |
| `extract_tables`   | bool                    | True    | Extract tables from PDF.                          |
| `passwords`        | list[str] \| None       | None    | Passwords to try when opening encrypted PDFs.     |
| `extract_metadata` | bool                    | True    | Extract PDF metadata.                             |
| `hierarchy`        | HierarchyConfig \| None | None    | Document hierarchy detection.                     |

```python
from xberg import ExtractionConfig, PdfConfig, HierarchyConfig

ExtractionConfig(
    pdf_options=PdfConfig(
        extract_images=True,
        extract_metadata=True,
        passwords=["password1", "password2"],
        hierarchy=HierarchyConfig(enabled=True, k_clusters=6),
    )
)
```

### LanguageDetectionConfig

| Field             | Type  | Default | Description                                          |
| ----------------- | ----- | ------- | ---------------------------------------------------- |
| `enabled`         | bool  | True    | Enable language detection.                           |
| `min_confidence`  | float | 0.8     | Minimum confidence threshold (0.0–1.0).             |
| `detect_multiple` | bool  | False   | Detect multiple languages instead of only the top one. |

### TokenReductionOptions

| Field                      | Type | Default | Description                                                            |
| -------------------------- | ---- | ------- | --------------------------------------------------------------------- |
| `mode`                     | str  | `""`    | `"off"`, `"light"`, `"moderate"`, `"aggressive"`, or `"maximum"`.     |
| `preserve_important_words` | bool | True    | Preserve capitalized words, technical terms, and proper nouns.        |

### PageConfig

| Field                 | Type | Default                                | Description                                  |
| --------------------- | ---- | -------------------------------------- | -------------------------------------------- |
| `extract_pages`       | bool | False                                  | Extract pages into `ExtractedDocument.pages`. |
| `insert_page_markers` | bool | False                                  | Insert page markers into content.            |
| `marker_format`       | str  | `"\n\n<!-- PAGE {page_num} -->\n\n"`   | Marker template containing `{page_num}`.     |

### PostProcessorConfig

| Field                 | Type              | Default | Description                                                 |
| --------------------- | ----------------- | ------- | ----------------------------------------------------------- |
| `enabled`             | bool              | True    | Enable post-processors in the extraction pipeline.          |
| `enabled_processors`  | list[str] \| None | None    | Whitelist of processor names to run. None = run all enabled. |
| `disabled_processors` | list[str] \| None | None    | Blacklist of processor names to skip.                       |

### ImagePreprocessingConfig

Preprocessing applied to images **before OCR** (not for extracting images from documents).

| Field                 | Type | Default | Description                                       |
| --------------------- | ---- | ------- | ------------------------------------------------- |
| `target_dpi`          | int  | 300     | Target DPI for normalization before OCR.          |
| `auto_rotate`         | bool | False   | Detect and correct image rotation.                |
| `deskew`              | bool | True    | Correct skewed images.                            |
| `denoise`             | bool | False   | Apply denoising filters.                          |
| `contrast_enhance`    | bool | False   | Enhance contrast.                                 |
| `binarization_method` | str  | "otsu"  | Method for black-and-white conversion.            |
| `invert_colors`       | bool | False   | Invert colors (white text on black).              |

### ImageExtractionConfig

Configuration for extracting images **from** documents.

| Field                 | Type | Default | Description                                       |
| --------------------- | ---- | ------- | ------------------------------------------------- |
| `extract_images`      | bool | True    | Enable image extraction.                          |
| `target_dpi`          | int  | 300     | Target DPI for image normalization.               |
| `max_image_dimension` | int  | 4096    | Maximum width/height for extracted images.        |
| `auto_adjust_dpi`     | bool | True    | Automatically adjust DPI by content quality.      |
| `min_dpi`             | int  | 72      | Minimum DPI threshold.                            |
| `max_dpi`             | int  | 600     | Maximum DPI threshold.                            |

## ExtractionResult (envelope) and ExtractedDocument

`extract` and `extract_batch` both return an **`ExtractionResult` envelope**. Per-document data lives on each `ExtractedDocument` in `result.results`.

### ExtractionResult

| Field     | Type                        | Description                                  |
| --------- | --------------------------- | -------------------------------------------- |
| `results` | list[ExtractedDocument]     | Extracted documents in discovery/input order. |
| `errors`  | list[ExtractionErrorItem]   | Non-fatal per-input errors.                  |
| `summary` | ExtractionSummary           | Aggregate counts for the operation.          |

`ExtractionSummary` fields: `inputs`, `results`, `errors`, `remote_urls`, `pages_crawled`, `documents_downloaded`.

### ExtractedDocument

| Field                 | Type                           | Description                                                         |
| --------------------- | ------------------------------ | ----------------------------------------------------------------- |
| `content`             | str                            | Main extracted text in the configured `output_format`.            |
| `mime_type`           | str                            | MIME type of the source document.                                 |
| `metadata`            | Metadata                       | Document metadata (flat — format-specific fields at the top level). |
| `tables`              | list[Table]                    | Extracted tables (each with `cells` and `markdown`).              |
| `detected_languages`  | list[str] \| None              | Detected ISO 639-1 codes (if language detection enabled).         |
| `chunks`              | list[Chunk] \| None            | Text chunks (if chunking enabled).                               |
| `images`              | list[ExtractedImage] \| None   | Extracted images (if enabled).                                   |
| `pages`               | list[PageContent] \| None      | Per-page content (if page extraction enabled).                   |
| `elements`            | list[Element] \| None          | Semantic elements (if `result_format="element_based"`).          |
| `extracted_keywords`  | list[Keyword] \| None          | Extracted keywords (if enabled).                                 |
| `quality_score`       | float \| None                  | Overall quality score (0.0–1.0).                                 |
| `processing_warnings` | list[ProcessingWarning]        | Non-fatal warnings from the pipeline.                            |

**Example:**

```python
import asyncio
from xberg import ExtractInput, extract, ExtractionConfig, ChunkingConfig

async def main() -> None:
    config = ExtractionConfig(
        chunking=ChunkingConfig(max_characters=512),
        output_format="markdown",
        language_detection=None,
    )
    result = await extract(ExtractInput(uri="document.pdf"), config)
    doc = result.results[0]

    print(f"Content preview: {doc.content[:200]}")
    print(f"MIME type: {doc.mime_type}")
    print(f"Tables: {len(doc.tables)}")

    if doc.detected_languages:
        print(f"Languages: {doc.detected_languages}")

    for chunk in doc.chunks or []:
        print(f"Chunk: {chunk.content[:80]}")
        if chunk.embedding:
            print(f"  embedding dims: {len(chunk.embedding)}")

asyncio.run(main())
```

### Metadata access

Metadata exposes typed attributes — access fields directly. Format-specific metadata lives under `metadata.format`:

```python
result = await extract(ExtractInput(uri="document.pdf"), ExtractionConfig())
metadata = result.results[0].metadata

if metadata.pages:
    print(f"Pages: {metadata.pages.total_count}")
if metadata.title:
    print(f"Title: {metadata.title}")
if metadata.authors:
    print(f"Authors: {', '.join(metadata.authors)}")
```

### Tables

```python
result = await extract(ExtractInput(uri="document.pdf"), ExtractionConfig())
for table in result.results[0].tables:
    print(f"Table with {len(table.cells)} rows")
    print(table.markdown)
    for row in table.cells:
        print(row)
```

## Error Classes

The errors raised by `extract()` and `extract_batch()` are plain `RuntimeError` — the typed exception classes below are **not** raised by these entry points, so catch `RuntimeError` (or `Exception`) around an `extract()`/`extract_batch()` call.

Xberg also defines a family of typed exception classes re-exported from `xberg`. The `XbergError`-rooted ones inherit from `XbergError` (base): `ParsingError`, `OcrError`, `ValidationError`, `MissingDependencyError`, `UnsupportedFormatError`, `CacheError`, `ImageProcessingError`, `SerializationError`, `PluginError`, `SecurityError`, `TranscriptionError`, `EmbeddingError`, `RerankingError`, `XbergTimeoutError`, `IoError`, and others. A few classes sit in separate hierarchies rooted at `Exception` rather than `XbergError` — e.g. `ConfigError`/`PdfAnalysisError` and `ParseError`/`SchemaValidationError`.

> Note: the OCR exception is `OcrError` (not `OCRError`).

```python
import asyncio
from xberg import ExtractInput, extract, ExtractionConfig

async def main() -> None:
    try:
        result = await extract(ExtractInput(uri="document.pdf"), ExtractionConfig())
    except RuntimeError as e:
        print(f"Extraction failed: {e}")

asyncio.run(main())
```

Per-input failures during `extract_batch` are also reported non-fatally in `result.errors` (a list of `ExtractionErrorItem`) rather than raised.

## Plugin System

Plugins extend the Rust pipeline. Plugin callbacks receive a single `ExtractedDocument` (with `.content`, `.mime_type`, `.metadata`), not the envelope.

### Registering Post-Processors

`register_post_processor(processor)` — runs after extraction to enrich each result.

Required methods: `process(result: ExtractedDocument) -> ExtractedDocument`, `processing_stage() -> str` (`"early"`, `"middle"`, `"late"`). Optional: `name() -> str`, `version() -> str`, `should_process(result) -> bool`, `initialize()`, `shutdown()`.

```python
from xberg import register_post_processor, ExtractedDocument

class WordCountProcessor:
    def name(self) -> str:
        return "word_count"

    def version(self) -> str:
        return "1.0.0"

    def processing_stage(self) -> str:
        return "early"

    def process(self, result: ExtractedDocument) -> ExtractedDocument:
        result.metadata["word_count"] = len(result.content.split())
        return result

    def should_process(self, result: ExtractedDocument) -> bool:
        return bool(result.content)

register_post_processor(WordCountProcessor())
```

### Registering Validators

`register_validator(validator)` — runs after extraction; raising fails the extraction.

Expected methods: `name() -> str`, `version() -> str`, `validate(result: ExtractedDocument) -> None`. Optional: `should_validate(result) -> bool`, `priority() -> int` (higher runs first), `initialize()`, `shutdown()`.

```python
from xberg import register_validator, ExtractedDocument, ValidationError

class MinLengthValidator:
    def __init__(self, min_length: int = 100):
        self.min_length = min_length

    def name(self) -> str:
        return "min_length_validator"

    def version(self) -> str:
        return "1.0.0"

    def priority(self) -> int:
        return 100

    def validate(self, result: ExtractedDocument) -> None:
        if len(result.content) < self.min_length:
            raise ValidationError(f"Content too short: {len(result.content)}")

register_validator(MinLengthValidator())
```

### Registering Document Extractors

`register_document_extractor(extractor)` — adds support for a custom format.

Expected methods: `name() -> str`, `version() -> str`, `supported_mime_types() -> list[str]`, `priority() -> int`, `extract(content: bytes, mime_type: str, config: dict) -> ExtractedDocument`. Optional: `initialize()`, `shutdown()`.

```python
from xberg import register_document_extractor, ExtractedDocument
import json

class CustomJsonExtractor:
    def name(self) -> str:
        return "custom-json-extractor"

    def version(self) -> str:
        return "1.0.0"

    def supported_mime_types(self) -> list[str]:
        return ["application/json"]

    def priority(self) -> int:
        return 50

    def extract(self, content: bytes, mime_type: str, config: dict) -> ExtractedDocument:
        data = json.loads(content)
        return {"content": json.dumps(data, indent=2), "mime_type": "application/json"}

register_document_extractor(CustomJsonExtractor())
```

### Registering OCR and Embedding Backends

`register_ocr_backend(backend)` and `register_embedding_backend(backend)` plug in custom OCR/embedding engines.

```python
from xberg import register_embedding_backend

class MyEmbedder:
    def name(self) -> str:
        return "my-embedder"

    def version(self) -> str:
        return "1.0.0"

    def dimensions(self) -> int:
        return 768

    def embed(self, texts: list[str]) -> list[list[float]]:
        ...  # return one vector per text

register_embedding_backend(MyEmbedder())
```

Reference an embedding backend from config via `EmbeddingConfig(model={"type": "plugin", "name": "my-embedder"})`.

### Plugin Management

```python
from xberg import (
    list_document_extractors, list_post_processors, list_validators, list_ocr_backends,
    unregister_post_processor, unregister_validator, unregister_document_extractor, unregister_ocr_backend,
    clear_post_processors, clear_validators, clear_document_extractors, clear_ocr_backends,
)

print(list_post_processors())
unregister_post_processor("word_count")
clear_validators()
```

## Format Enums

### Content output format (`output_format`)

`"plain"` (default), `"markdown"`, `"djot"`, `"html"`.

### Result format (`result_format`)

`"unified"` (default — all content in `content`) or `"element_based"` (Unstructured-compatible semantic elements in `elements`).

## Other Functions

```python
from xberg import list_supported_formats, __version__

formats = list_supported_formats()   # all supported document formats
print(__version__)
```
