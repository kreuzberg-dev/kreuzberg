---
name: xberg
description: >-
  Extract text, tables, metadata, and images from 101 document formats
  (PDF, Office, images, HTML, email, archives, academic) using Xberg.
  Use when writing code that calls Xberg APIs in Python, Node.js/TypeScript,
  Rust, or CLI. Covers installation, extraction (sync/async), configuration
  (OCR, chunking, output format), batch processing, error handling, and plugins.
license: Elastic-2.0
metadata:
  author: xberg-io
  version: "0.1.0"
  repository: https://github.com/xberg-io/xberg
---

<!--
AI-RULEZ :: GENERATED FILE — DO NOT EDIT
Content-Hash: blake3:99de599640f2b3a9128bd4d1b4d281cf91f0d6d70802f8e283c96537a8287ec9
Source-Hash: blake3:ac8bab9ae4a76e61f437af29b4a45a38326056a50769a177038927c69353edaf
Schema-Version: v1
-->

# Xberg Document Extraction

Xberg is a high-performance document intelligence library with a Rust core and native bindings for Python, Node.js/TypeScript, Ruby, Go, Java, C#, PHP, and Elixir. It extracts text, tables, metadata, and images from 101 file formats across 115 file extensions including PDF, Office documents, images (with OCR), HTML, email, archives, and academic formats.

Use this skill when writing code that:

- Extracts text or metadata from documents
- Performs OCR on scanned documents or images
- Batch-processes multiple files
- Configures extraction options (output format, chunking, OCR, language detection)
- Implements custom plugins (post-processors, validators, OCR backends)

> If the `xberg` MCP server is registered in this session, prefer its tools over shelling out to the CLI — they expose the same extraction surface with structured arguments and results.

## Installation

### Python

```bash
pip install xberg
```

### Node.js

```bash
npm install @xberg-io/xberg
```

### Rust

```bash
cargo add xberg
```

```toml
# Cargo.toml
[dependencies]
xberg = { version = "1.0.2", features = ["full"] }
tokio = { version = "1", features = ["full"] }
# feature flags: pdf, ocr, chunking, embeddings, language-detection, keywords, api, mcp
#                (or "formats" / "full" aggregates); tokio-runtime is on by default
```

### CLI

```bash
brew install xberg-io/tap/xberg
# or run without a persistent install (the CLI proxy package self-installs the binary):
npx @xberg-io/xberg-cli --help
uvx --from xberg-cli xberg --help
# or download a prebuilt binary from the latest GitHub release:
#   https://github.com/xberg-io/xberg/releases/latest
# or build from source:
cargo install xberg-cli
```

## Quick Start

The library entry points are `extract(input, config)` and `extract_batch(inputs, config)`. Both return an `ExtractionResult` **envelope** — the extracted document(s) live in `result.results`, and per-document data (`content`, `tables`, `metadata`, …) is on each `result.results[i]`. Python and Node are async-only.

### Python

```python
import asyncio
from xberg import ExtractInput, extract, ExtractionConfig

async def main() -> None:
    result = await extract(ExtractInput(uri="document.pdf"), ExtractionConfig())
    doc = result.results[0]
    print(doc.content)    # extracted text
    print(doc.metadata)   # document metadata
    print(doc.tables)     # extracted tables

asyncio.run(main())
```

### Node.js

```typescript
import { extract } from "@xberg-io/xberg";

const output = await extract({ kind: "uri", uri: "document.pdf" });
const doc = output.results[0];
console.log(doc.content);
console.log(doc.metadata);
console.log(doc.tables);
```

### Rust

```rust
use xberg::{extract, ExtractInput, ExtractionConfig};

#[tokio::main]
async fn main() -> xberg::Result<()> {
    let output = extract(ExtractInput::from_uri("document.pdf"), &ExtractionConfig::default()).await?;
    println!("{}", output.results[0].content);
    Ok(())
}
```

### CLI

```bash
xberg extract document.pdf
xberg extract document.pdf --format json
xberg extract document.pdf --content-format markdown
```

## Configuration

All languages use the same configuration structure with language-appropriate naming conventions.

### Python (snake_case)

```python
from xberg import (
    ExtractInput, extract,
    ExtractionConfig, OcrConfig, TesseractConfig, PdfConfig, ChunkingConfig, OutputFormat,
)

config = ExtractionConfig(
    ocr=OcrConfig(
        backend="tesseract",
        language=["eng"],
        tesseract_config=TesseractConfig(psm=6, enable_table_detection=True),
    ),
    pdf_options=PdfConfig(passwords=["secret123"]),
    chunking=ChunkingConfig(max_characters=1000, overlap=200),
    output_format=OutputFormat("markdown"),
)

result = await extract(ExtractInput(uri="document.pdf"), config)
```

### Node.js (camelCase)

```typescript
import { extract, type ExtractionConfig } from "@xberg-io/xberg";

const config: ExtractionConfig = {
  ocr: { backend: "tesseract", language: ["eng"] },
  pdfOptions: { passwords: ["secret123"] },
  chunking: { maxCharacters: 1000, overlap: 200 },
  outputFormat: "markdown",
};

const output = await extract({ kind: "uri", uri: "document.pdf" }, config);
```

### Rust (snake_case)

```rust
use xberg::{extract, ExtractInput, ExtractionConfig, OcrConfig, ChunkingConfig, OutputFormat};

let config = ExtractionConfig {
    ocr: Some(OcrConfig {
        backend: "tesseract".into(),
        language: vec!["eng".to_string()],
        ..Default::default()
    }),
    chunking: Some(ChunkingConfig {
        max_characters: 1000,
        overlap: 200,
        ..Default::default()
    }),
    output_format: OutputFormat::Markdown,
    ..Default::default()
};

let output = extract(ExtractInput::from_uri("document.pdf"), &config).await?;
```

### Config File (TOML)

```toml
output_format = "markdown"

[ocr]
backend = "tesseract"
language = "eng"

[chunking]
max_characters = 1000
overlap = 200

[pdf_options]
passwords = ["secret123"]
```

```bash
# CLI: auto-discovers xberg.toml in current/parent directories
xberg extract doc.pdf
# or explicit:
xberg extract doc.pdf --config xberg.toml
xberg extract doc.pdf --config-json '{"ocr":{"backend":"tesseract","language":"deu"}}'
```

## Batch Processing

`extract_batch` takes a list of `ExtractInput`s and returns one envelope whose `results` array holds a document per input (in input order); per-input failures are reported in `result.errors`.

### Python

```python
from xberg import ExtractInput, extract_batch, ExtractionConfig

inputs = [
    ExtractInput(uri="doc1.pdf"),
    ExtractInput(uri="doc2.docx"),
    ExtractInput(uri="doc3.xlsx"),
]
output = await extract_batch(inputs, ExtractionConfig())

for doc in output.results:
    print(f"{len(doc.content)} chars extracted")
```

### Node.js

```typescript
import { extractBatch } from "@xberg-io/xberg";

const output = await extractBatch([
  { kind: "uri", uri: "doc1.pdf" },
  { kind: "uri", uri: "doc2.docx" },
]);
for (const doc of output.results) {
  console.log(`${doc.content.length} chars`);
}
```

### Rust

```rust
use xberg::{extract_batch, ExtractInput, ExtractionConfig};

let config = ExtractionConfig::default();
let inputs = vec![ExtractInput::from_uri("doc1.pdf"), ExtractInput::from_uri("doc2.docx")];
let output = extract_batch(inputs, &config).await?;
```

### CLI

```bash
xberg batch *.pdf --format json
xberg batch docs/*.docx --content-format markdown
```

## OCR

OCR runs automatically for images and scanned PDFs. Tesseract is the default backend (native binding, no external install required).

### Backends

Select with `OcrConfig.backend`:

- **tesseract** (default): built-in native binding. All Tesseract languages supported.
- **paddleocr** (`"paddleocr"` / `"paddle-ocr"`): ONNX-based PaddleOCR.
- **vlm**: Vision-Language-Model OCR (configure via `OcrConfig.vlm_config`).

Custom backends can be registered in Python/Node via `register_ocr_backend` (see [Advanced Features](references/advanced-features.md)).

### Language Codes

```python
config = ExtractionConfig(ocr=OcrConfig(language=["eng"]))          # English
config = ExtractionConfig(ocr=OcrConfig(language=["eng", "deu"]))   # Multiple
# The single-string shorthand ("eng+deu") is only accepted in config files / --config-json,
# not in the OcrConfig constructor (Python takes a list, Node takes an array).
```

### Force OCR

```python
config = ExtractionConfig(force_ocr=True)  # OCR even if text is extractable
```

## Result Envelope and Document Fields

`extract` / `extract_batch` return an `ExtractionResult` envelope: `results` (list of documents), `errors` (per-input failures), and `summary` (counts). Per-document fields live on each document in `results` — bind `doc = result.results[0]` (Python/Node) or `&output.results[0]` (Rust) first.

| Field        | Python (`doc.`)        | Node.js (`doc.`)      | Rust (`document.`)      | Description                                   |
| ------------ | ---------------------- | --------------------- | ----------------------- | --------------------------------------------- |
| Text content | `content`              | `content`             | `content`               | Extracted text (str/String)                   |
| MIME type    | `mime_type`            | `mimeType`            | `mime_type`             | Input document MIME type                      |
| Metadata     | `metadata`             | `metadata`            | `metadata`              | Document metadata (flat mapping)              |
| Tables       | `tables`               | `tables`              | `tables`                | Extracted tables with cells + markdown        |
| Languages    | `detected_languages`   | `detectedLanguages`   | `detected_languages`    | Detected languages (if enabled)               |
| Chunks       | `chunks`               | `chunks`              | `chunks`                | Text chunks (if chunking enabled)             |
| Images       | `images`               | `images`             | `images`                | Extracted images (if enabled)                 |
| Elements     | `elements`             | `elements`            | `elements`              | Semantic elements (if element_based format)   |
| Pages        | `pages`                | `pages`               | `pages`                 | Per-page content (if page extraction enabled) |
| Keywords     | `extracted_keywords`   | `extractedKeywords`   | `extracted_keywords`    | Extracted keywords (if enabled)               |

## Error Handling

### Python

`extract` / `extract_batch` raise a plain `RuntimeError` on failure — the typed `XbergError` subclasses are not raised by these entry points, so catch `RuntimeError`. Per-input failures during `extract_batch` are reported non-fatally in `result.errors`.

```python
from xberg import ExtractInput, extract, ExtractionConfig

try:
    result = await extract(ExtractInput(uri="file.pdf"), ExtractionConfig())
    for err in result.errors:
        print(f"Per-input error: {err}")
except RuntimeError as e:
    print(f"Extraction failed: {e}")
```

### Node.js

The Node binding throws plain `Error` objects (it does not export typed error subclasses). Catch with `instanceof Error`, and inspect `output.errors` for non-fatal per-input failures.

```typescript
import { extract } from "@xberg-io/xberg";

try {
  const output = await extract({ kind: "uri", uri: "file.pdf" });
  if (output.errors.length > 0) {
    console.error("Per-input errors:", output.errors);
  }
} catch (e) {
  if (e instanceof Error) {
    console.error(`Extraction failed: ${e.message}`);
  }
}
```

### Rust

```rust
use xberg::{extract, ExtractInput, ExtractionConfig, XbergError};

let config = ExtractionConfig::default();
match extract(ExtractInput::from_uri("file.pdf"), &config).await {
    Ok(output) => println!("{}", output.results[0].content),
    Err(XbergError::Parsing { message, .. }) => eprintln!("Parse error: {message}"),
    Err(XbergError::Ocr { message, .. }) => eprintln!("OCR error: {message}"),
    Err(XbergError::UnsupportedFormat(mime)) => eprintln!("Unsupported: {mime}"),
    Err(e) => eprintln!("Error: {e}"),
}
```

## Common Pitfalls

1. **Result is an envelope**: `extract` / `extract_batch` return `ExtractionResult` with `results`, `errors`, and `summary`. Per-document fields (`content`, `tables`, `chunks`, …) are on `result.results[i]`, NOT on the top-level return.
2. **Async-only**: Python and Node have no sync variants — always `await extract(...)`. Rust `extract` is async; use `#[tokio::main]` or an async context.
3. **Build the input**: pass an `ExtractInput`, not a bare path. Use `ExtractInput(uri=...)` / `ExtractInput::from_uri(...)` (Python/Rust) or `{ kind: "uri", uri: "..." }` (Node); for bytes use `kind="bytes"` with `bytes`/`mime_type`.
4. **Python ChunkingConfig fields**: construct with `max_characters` and `overlap` (defaults 1000 / 200); these are also the readable attributes. When passing config as a dict/JSON, the `max_chars` / `max_overlap` aliases are also accepted. Node uses `maxCharacters` / `overlap`; Rust struct fields are `max_characters` / `overlap`.
5. **Python errors**: `extract` / `extract_batch` raise a plain `RuntimeError` on failure, not typed `XbergError` subclasses — catch `RuntimeError`. Node throws plain `Error` (no typed error subclasses).
6. **Rust extract signature**: `extract(input, &config)` — the config is a reference. Use `&ExtractionConfig::default()` for defaults.
7. **CLI --format vs --content-format**: `--format` controls CLI output (text/json). `--content-format` controls content format (plain/markdown/djot/html).
8. **Config file field names**: Use snake_case in TOML/YAML/JSON config files — `[chunking]` fields are `max_characters` and `overlap`; other fields use names like `output_format`, `pdf_options`.

## Supported Formats (Summary)

| Category          | Extensions                                                                                                                                                  |
| ----------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **PDF**           | `.pdf`                                                                                                                                                      |
| **Word**          | `.docx`, `.odt`                                                                                                                                             |
| **Spreadsheets**  | `.xlsx`, `.xlsm`, `.xlsb`, `.xls`, `.xla`, `.xlam`, `.xltm`, `.ods`                                                                                         |
| **Presentations** | `.pptx`, `.ppt`, `.ppsx`                                                                                                                                    |
| **eBooks**        | `.epub`, `.fb2`                                                                                                                                             |
| **Images**        | `.png`, `.jpg`, `.jpeg`, `.gif`, `.webp`, `.bmp`, `.tiff`, `.tif`, `.jp2`, `.jpx`, `.jpm`, `.mj2`, `.jbig2`, `.jb2`, `.pnm`, `.pbm`, `.pgm`, `.ppm`, `.svg` |
| **Markup**        | `.html`, `.htm`, `.xhtml`, `.xml`                                                                                                                           |
| **Data**          | `.json`, `.yaml`, `.yml`, `.toml`, `.csv`, `.tsv`                                                                                                           |
| **Text**          | `.txt`, `.md`, `.markdown`, `.djot`, `.rst`, `.org`, `.rtf`                                                                                                 |
| **Email**         | `.eml`, `.msg`                                                                                                                                              |
| **Archives**      | `.zip`, `.tar`, `.tgz`, `.gz`, `.7z`                                                                                                                        |
| **Academic**      | `.bib`, `.biblatex`, `.ris`, `.nbib`, `.enw`, `.csl`, `.tex`, `.latex`, `.typ`, `.jats`, `.ipynb`, `.docbook`, `.opml`, `.pod`, `.mdoc`, `.troff`           |

See [references/supported-formats.md](references/supported-formats.md) for the complete format reference with MIME types.

## Additional Resources

Detailed reference files for specific topics:

- **[Python API Reference](references/python-api.md)** — All functions, config classes, plugin protocols, exact signatures
- **[Node.js API Reference](references/nodejs-api.md)** — All functions, TypeScript interfaces, worker pool APIs
- **[Rust API Reference](references/rust-api.md)** — All functions with feature gates, structs, Cargo.toml examples
- **[CLI Reference](references/cli-reference.md)** — All commands, flags, config precedence, exit codes
- **[Configuration Reference](references/configuration.md)** — TOML/YAML/JSON formats, auto-discovery, env vars, full schema
- **[Supported Formats](references/supported-formats.md)** — All 101 formats (115 file extensions) with file extensions and MIME types
- **[Advanced Features](references/advanced-features.md)** — Plugins, embeddings, MCP server, API server, security limits
- **[Other Language Bindings](references/other-bindings.md)** — Go, Ruby, Java, C#, PHP, Elixir, WASM, Docker

## Related skills

Task-focused sibling skills go deeper than this overview:

- **extracting-with-ocr** — OCR backends, language packs, force-OCR, tuning.
- **extracting-tables** — layout-aware table detection and table models.
- **chunking** — chunk size/overlap, markdown/yaml/semantic chunkers, the `chunk` command.
- **extracting-keywords** — YAKE/RAKE keywords, language detection, the `embed` command.
- **batch-extraction** — the `batch` command, `--file-configs`, parallelism, error recovery.
- **picking-a-format** — choosing `--format` / `--content-format` per consumer.

Full documentation: <https://docs.xberg.io>
GitHub: <https://github.com/xberg-io/xberg>
