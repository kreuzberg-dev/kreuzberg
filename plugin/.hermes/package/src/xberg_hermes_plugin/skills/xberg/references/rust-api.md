<!--
AI-RULEZ :: GENERATED FILE — DO NOT EDIT
Content-Hash: blake3:12b636531e164bffa6358fe0c6fe897658f543df198e0d04d2a430b0fba77b22
Source-Hash: blake3:5907a9cc29a5d72bbd3eaf5b820cac5133c8724895664c64fa8eafc2227716af
Schema-Version: v1
-->

# Xberg Rust API Reference

Complete API reference for the Xberg document extraction library in Rust. Xberg is the Rust core that powers every language binding.

## Setup

```bash
cargo add xberg
```

The crate version is `1.0.2`. Add it to `Cargo.toml`:

```toml
[dependencies]
xberg = { version = "1.0.2", features = ["full"] }
tokio = { version = "1", features = ["full"] }
```

`extract` / `extract_batch` are async and need a Tokio runtime, which you bring yourself (`#[tokio::main]` or an existing runtime).

### Feature flags

The `tokio-runtime` feature is on by default. Capabilities are gated behind features:

| Feature              | What it enables                                            |
| -------------------- | --------------------------------------------------------- |
| `tokio-runtime`      | Async runtime support (default).                          |
| `formats`            | All document formats (no OCR/ML/api/mcp/chunking).        |
| `full`               | `formats` + ocr + layout + embeddings + tree-sitter + LLM. |
| `pdf`                | PDF extraction.                                           |
| `ocr`                | Tesseract-based OCR.                                      |
| `chunking`           | Text chunking for RAG pipelines.                         |
| `embeddings`         | ONNX embedding generation.                                |
| `language-detection` | Detect document language(s).                              |
| `keywords`           | YAKE + RAKE keyword extraction.                          |
| `api`                | HTTP API server (Axum).                                  |
| `mcp`                | Model Context Protocol server.                            |

---

## Core Extraction Functions

There are exactly two entry points. Both are `async`, take an [`ExtractInput`](#extractinput) and a `&ExtractionConfig`, and return an [`ExtractionResult`](#extractionresult-envelope) **envelope**.

### `extract`

```rust
pub async fn extract(input: ExtractInput, config: &ExtractionConfig) -> Result<ExtractionResult>
```

```rust
use xberg::{extract, ExtractInput, ExtractionConfig};

#[tokio::main]
async fn main() -> xberg::Result<()> {
    let config = ExtractionConfig {
        use_cache: true,
        enable_quality_processing: true,
        ..Default::default()
    };

    let output = extract(ExtractInput::from_uri("document.pdf"), &config).await?;
    println!("{}", output.results[0].content);
    println!("MIME type: {}", output.results[0].mime_type);
    Ok(())
}
```

### `extract_batch`

```rust
pub async fn extract_batch(inputs: Vec<ExtractInput>, config: &ExtractionConfig) -> Result<ExtractionResult>
```

Extract multiple inputs in parallel. The returned envelope's `results` holds one `ExtractedDocument` per input (in input order), with non-fatal failures in `errors`.

```rust
use xberg::{extract_batch, ExtractInput, ExtractionConfig};

#[tokio::main]
async fn main() -> xberg::Result<()> {
    let config = ExtractionConfig::default();
    let inputs = vec![
        ExtractInput::from_uri("document.pdf"),
        ExtractInput::from_bytes(
            b"Hello from memory".to_vec(),
            "text/plain",
            Some("note.txt".to_string()),
        ),
    ];

    let output = extract_batch(inputs, &config).await?;
    for document in &output.results {
        println!("{}", document.content);
    }
    Ok(())
}
```

---

## ExtractInput

Unified input for both entry points.

```rust
pub struct ExtractInput {
    pub kind: ExtractInputKind,            // Bytes | Uri
    pub bytes: Option<Vec<u8>>,            // for kind = Bytes
    pub uri: Option<String>,               // path, file:// URI, or HTTP(S) URL
    pub mime_type: Option<String>,         // MIME type hint
    pub filename: Option<String>,          // filename hint
    pub config: Option<FileExtractionConfig>, // per-input overrides
}
```

Constructors:

```rust
// From a path, file:// URI, or URL
let input = ExtractInput::from_uri("document.pdf");

// From raw bytes (mime_type required, filename optional)
let input = ExtractInput::from_bytes(bytes.to_vec(), "application/pdf", Some("document.pdf".to_string()));
```

---

## Configuration

### `ExtractionConfig`

Construct with a struct literal and `..Default::default()`. Common fields:

```rust
pub struct ExtractionConfig {
    pub use_cache: bool,                   // default: true
    pub enable_quality_processing: bool,   // default: true
    pub ocr: Option<OcrConfig>,            // None = OCR disabled
    pub force_ocr: bool,                   // default: false
    pub chunking: Option<ChunkingConfig>,  // None = disabled
    pub images: Option<ImageExtractionConfig>,
    pub pdf_options: Option<PdfConfig>,
    pub token_reduction: Option<TokenReductionOptions>,
    pub language_detection: Option<LanguageDetectionConfig>,
    pub pages: Option<PageConfig>,
    pub keywords: Option<KeywordConfig>,
    pub postprocessor: Option<PostProcessorConfig>,
    pub max_concurrent_extractions: Option<usize>,
    pub result_format: ResultFormat,       // Unified | ElementBased
    pub output_format: OutputFormat,       // Plain | Markdown | Djot | Html
    pub security_limits: Option<SecurityLimits>,
    // ... additional optional fields
}
```

```rust
use xberg::{extract, ChunkingConfig, ExtractionConfig, ExtractInput, OcrConfig, TesseractConfig};

#[tokio::main]
async fn main() -> xberg::Result<()> {
    let config = ExtractionConfig {
        use_cache: true,
        ocr: Some(OcrConfig {
            backend: "tesseract".to_string(),
            language: vec!["eng".to_string(), "deu".to_string()],
            tesseract_config: Some(TesseractConfig { psm: 6, ..Default::default() }),
            ..Default::default()
        }),
        chunking: Some(ChunkingConfig {
            max_characters: 1000,
            overlap: 200,
            ..Default::default()
        }),
        ..Default::default()
    };

    let output = extract(ExtractInput::from_uri("document.pdf"), &config).await?;
    println!("Content length: {}", output.results[0].content.len());
    Ok(())
}
```

### `OcrConfig`

```rust
pub struct OcrConfig {
    pub enabled: bool,                       // default: true
    pub backend: String,                     // "tesseract", "paddleocr", "vlm"
    pub language: Vec<String>,               // ["eng"], ["eng", "fra"], ...
    pub tesseract_config: Option<TesseractConfig>,
    // ...
}
```

### `ChunkingConfig`

```rust
pub struct ChunkingConfig {
    pub max_characters: usize,    // default: 1000
    pub overlap: usize,           // default: 200
    pub trim: bool,               // default: true
    pub chunker_type: ChunkerType,  // Text | Markdown | Semantic
    pub embedding: Option<EmbeddingConfig>,
    pub preset: Option<String>,
    pub prepend_heading_context: bool,
    // ...
}
```

The chunk-size fields are `max_characters` and `overlap`.

```rust
use xberg::{extract, ChunkingConfig, ChunkerType, ExtractInput, ExtractionConfig};

let config = ExtractionConfig {
    chunking: Some(ChunkingConfig {
        max_characters: 512,
        overlap: 50,
        chunker_type: ChunkerType::Markdown,
        prepend_heading_context: true,
        ..Default::default()
    }),
    ..Default::default()
};
```

### Output formats

`ExtractionConfig::output_format` controls the `content` text format:

```rust
pub enum OutputFormat { Plain, Markdown, Djot, Html, Json, Structured, Custom(String) }
```

`ExtractionConfig::result_format` controls the result structure:

```rust
pub enum ResultFormat { Unified, ElementBased }
```

```rust
use xberg::{ExtractionConfig, OutputFormat};

let config = ExtractionConfig {
    output_format: OutputFormat::Markdown,
    ..Default::default()
};
```

---

## ExtractionResult (envelope)

`extract` and `extract_batch` return this envelope. Per-document data lives in each `ExtractedDocument` in `results`.

```rust
pub struct ExtractionResult {
    pub results: Vec<ExtractedDocument>,       // one per input, in order
    pub errors: Vec<ExtractionErrorItem>,      // non-fatal per-input errors
    pub summary: ExtractionSummary,            // aggregate counts
    pub crawl_final_urls: Vec<String>,
    pub crawl_redirect_count: usize,
    pub crawl_unique_normalized_urls: Vec<String>,
}

pub struct ExtractionSummary {
    pub inputs: usize,
    pub results: usize,
    pub errors: usize,
    pub remote_urls: usize,
    pub pages_crawled: usize,
    pub documents_downloaded: usize,
}

pub struct ExtractionErrorItem {
    pub index: usize,
    pub code: u32,
    pub error_type: String,
    pub source: String,
    pub message: String,
}
```

### `ExtractedDocument`

```rust
pub struct ExtractedDocument {
    pub content: String,
    pub mime_type: Cow<'static, str>,
    pub metadata: Metadata,
    pub extraction_method: Option<ExtractionMethod>,
    pub tables: Vec<Table>,
    pub detected_languages: Option<Vec<String>>,
    pub chunks: Option<Vec<Chunk>>,
    pub images: Option<Vec<ExtractedImage>>,
    pub pages: Option<Vec<PageContent>>,
    pub elements: Option<Vec<Element>>,
    pub extracted_keywords: Option<Vec<Keyword>>,
    pub quality_score: Option<f64>,
    pub processing_warnings: Vec<ProcessingWarning>,
    // ... additional optional fields
}
```

Note that `detected_languages`, `chunks`, `images`, `pages`, `elements`, and `extracted_keywords` are `Option<Vec<...>>` — match or `if let Some(...)` before iterating.

```rust
use xberg::{extract, ChunkingConfig, ExtractInput, ExtractionConfig};

#[tokio::main]
async fn main() -> xberg::Result<()> {
    let config = ExtractionConfig {
        chunking: Some(ChunkingConfig { max_characters: 512, overlap: 50, ..Default::default() }),
        ..Default::default()
    };

    let output = extract(ExtractInput::from_uri("document.pdf"), &config).await?;
    let document = &output.results[0];

    println!("Content: {}", document.content);
    println!("MIME: {}", document.mime_type);

    if let Some(langs) = &document.detected_languages {
        println!("Languages: {langs:?}");
    }
    if let Some(chunks) = &document.chunks {
        println!("Chunks: {}", chunks.len());
        for chunk in chunks {
            println!("  - {}", &chunk.content[..50.min(chunk.content.len())]);
        }
    }
    Ok(())
}
```

### `Chunk`

```rust
pub struct Chunk {
    pub content: String,
    pub chunk_type: ChunkType,
    pub embedding: Option<Vec<f32>>,
    pub metadata: ChunkMetadata,
}
```

---

## Error Handling

### `XbergError`

```rust
pub enum XbergError {
    Io(std::io::Error),
    Parsing { message: String, source: Option<Box<dyn std::error::Error + Send + Sync>> },
    Ocr { message: String, source: Option<Box<dyn std::error::Error + Send + Sync>> },
    Validation { message: String, source: Option<Box<dyn std::error::Error + Send + Sync>> },
    Cache { message: String, source: Option<Box<dyn std::error::Error + Send + Sync>> },
    ImageProcessing { message: String, source: Option<Box<dyn std::error::Error + Send + Sync>> },
    Serialization { message: String, source: Option<Box<dyn std::error::Error + Send + Sync>> },
    MissingDependency(String),
    Plugin { message: String, plugin_name: String },
    LockPoisoned(String),
    UnsupportedFormat(String),
    Other(String),
}
```

```rust
use xberg::{extract, ExtractInput, ExtractionConfig, XbergError, Result};

async fn extract_text(bytes: &[u8], mime_type: &str) -> Result<String> {
    let config = ExtractionConfig::default();
    let output = extract(
        ExtractInput::from_bytes(bytes.to_vec(), mime_type, Some("document.pdf".to_string())),
        &config,
    )
    .await?;

    Ok(output.results.first().map(|d| d.content.clone()).unwrap_or_default())
}

#[tokio::main]
async fn main() {
    let bytes = std::fs::read("document.pdf").unwrap_or_default();
    match extract_text(&bytes, "application/pdf").await {
        Ok(text) => println!("Extracted {} chars", text.len()),
        Err(XbergError::UnsupportedFormat(mime)) => eprintln!("Format not supported: {mime}"),
        Err(XbergError::Ocr { message, .. }) => eprintln!("OCR failed: {message}"),
        Err(e) => eprintln!("Error: {e}"),
    }
}
```

### Result type alias

```rust
pub type Result<T> = std::result::Result<T, XbergError>;
```

---

## Version

This reference targets Xberg `1.0.2`.
