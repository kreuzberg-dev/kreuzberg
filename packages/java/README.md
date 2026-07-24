# Java

<div align="center" style="display: flex; flex-wrap: wrap; gap: 8px; justify-content: center; margin: 20px 0;">
  <a href="https://github.com/xberg-io/alef">
    <img src="https://img.shields.io/badge/Bindings-alef%20%D7%90-007ec6" alt="Bindings">
  </a>
  <!-- Language Bindings -->
  <a href="https://crates.io/crates/xberg">
    <img src="https://img.shields.io/crates/v/xberg?label=Rust&color=007ec6" alt="Rust">
  </a>
  <a href="https://pypi.org/project/xberg/">
    <img src="https://img.shields.io/pypi/v/xberg?label=Python&color=007ec6" alt="Python">
  </a>
  <a href="https://www.npmjs.com/package/@xberg-io/xberg">
    <img src="https://img.shields.io/npm/v/@xberg-io/xberg?label=Node.js&color=007ec6" alt="Node.js">
  </a>
  <a href="https://www.npmjs.com/package/@xberg-io/xberg-wasm">
    <img src="https://img.shields.io/npm/v/@xberg-io/xberg-wasm?label=WASM&color=007ec6" alt="WASM">
  </a>
  <a href="https://central.sonatype.com/artifact/io.xberg/xberg">
    <img src="https://img.shields.io/maven-central/v/io.xberg/xberg?label=Java&color=007ec6" alt="Java">
  </a>
  <a href="https://github.com/xberg-io/xberg/tree/main/packages/go">
    <img src="https://img.shields.io/github/v/tag/xberg-io/xberg?label=Go&color=007ec6&filter=v1*" alt="Go">
  </a>
  <a href="https://www.nuget.org/packages/Xberg/">
    <img src="https://img.shields.io/nuget/v/Xberg?label=C%23&color=007ec6" alt="C#">
  </a>
  <a href="https://packagist.org/packages/xberg-io/xberg">
    <img src="https://img.shields.io/packagist/v/xberg-io/xberg?label=PHP&color=007ec6" alt="PHP">
  </a>
  <a href="https://rubygems.org/gems/xberg">
    <img src="https://img.shields.io/gem/v/xberg?label=Ruby&color=007ec6" alt="Ruby">
  </a>
  <a href="https://hex.pm/packages/xberg">
    <img src="https://img.shields.io/hexpm/v/xberg?label=Elixir&color=007ec6" alt="Elixir">
  </a>
  <a href="https://pub.dev/packages/xberg">
    <img src="https://img.shields.io/pub/v/xberg?label=Dart&color=007ec6" alt="Dart">
  </a>
  <a href="https://central.sonatype.com/artifact/io.xberg/xberg-android">
    <img src="https://img.shields.io/maven-central/v/io.xberg/xberg-android?label=Kotlin&color=007ec6" alt="Kotlin">
  </a>
  <a href="https://github.com/xberg-io/xberg/tree/main/packages/swift">
    <img src="https://img.shields.io/badge/Swift-SPM-007ec6" alt="Swift">
  </a>
  <a href="https://github.com/xberg-io/xberg/tree/main/packages/zig">
    <img src="https://img.shields.io/badge/Zig-package-007ec6" alt="Zig">
  </a>
  <a href="https://github.com/xberg-io/xberg/releases">
    <img src="https://img.shields.io/badge/C-FFI-007ec6" alt="C FFI">
  </a>
  <a href="https://github.com/xberg-io/xberg/pkgs/container/xberg">
    <img src="https://img.shields.io/badge/Docker-ghcr.io-007ec6?logo=docker&logoColor=white" alt="Docker">
  </a>
  <a href="https://docs.xberg.io/guides/kubernetes/">
    <img src="https://img.shields.io/badge/Helm-chart-007ec6?logo=helm&logoColor=white" alt="Helm chart">
  </a>
  <!-- Project Info -->
  <a href="https://github.com/xberg-io/xberg/blob/main/LICENSE">
    <img src="https://img.shields.io/badge/License-MIT-007ec6" alt="License">
  </a>
  <a href="https://docs.xberg.io">
    <img src="https://img.shields.io/badge/Docs-xberg-007ec6" alt="Documentation">
  </a>
  <a href="https://huggingface.co/xberg-io">
    <img src="https://img.shields.io/badge/Hugging%20Face-Xberg-007ec6" alt="Hugging Face">
  </a>
</div>

<div align="center" style="display: flex; flex-wrap: wrap; gap: 12px; justify-content: center; margin: 28px 0 24px;">
  <a href="https://discord.gg/xt9WY3GnKR">
    <img height="22" src="https://img.shields.io/badge/Discord-Chat-007ec6?logo=discord&logoColor=white" alt="Join Discord">
  </a>
  <a href="https://docs.xberg.io/demo.html">
    <img height="22" src="https://img.shields.io/badge/Live%20Demo-Open-007ec6?logo=webassembly&logoColor=white" alt="Live Demo">
  </a>
  <a href="https://github.com/xberg-io/xberg/stargazers">
    <img height="22" src="https://img.shields.io/github/stars/xberg-io/xberg?style=social" alt="GitHub Stars">
  </a>
</div>

Extract text, tables, images, metadata, and code intelligence from 97 file formats and 306 programming languages including PDF, Office documents, images, and audio/video transcripts where native transcription is available. Java bindings with type-safe API, Foreign Function & Memory API integration, and native performance.

## What This Package Provides

- **Document intelligence core** — extract text, tables, images, metadata, entities, keywords, code intelligence, and transcripts in builds that enable transcription.
- **Format coverage** — PDF, Office, images, HTML/XML, email, archives, notebooks, citations, scientific formats, plain text, and audio/video formats in builds that enable transcription.
- **OCR choices** — Tesseract, PaddleOCR, Candle where supported, VLM OCR through liter-llm, and plugin hooks for custom backends.
- **Same engine as every binding** — Rust, Python, Node.js, Go, Java, PHP, Ruby, .NET, Elixir, WASM, Kotlin Android, Swift, Dart, Zig, and C FFI share the same Rust implementation.
- **Java package** — FFM binding for direct native document extraction.

## Installation

### Package Installation

Add to your `pom.xml`:

```xml
<dependency>
    <groupId>io.xberg</groupId>
    <artifactId>xberg</artifactId>
    <version>1.0.0-rc.37</version>
</dependency>
```

Kotlin DSL (`build.gradle.kts`):

```kotlin
implementation("io.xberg:xberg:1.0.0-rc.37")
```

Groovy DSL (`build.gradle`):

```groovy
implementation 'io.xberg:xberg:1.0.0-rc.37'
```

### System Requirements
- **Java 25+** required (Foreign Function & Memory API; build run with `--enable-preview` and `--enable-native-access=ALL-UNNAMED`)
- Native libraries bundled in the JAR for macOS (arm64, x64), Linux (x64, arm64), and Windows (x64)
- Optional: [ONNX Runtime](https://github.com/microsoft/onnxruntime/releases) version 1.24+ for ORT-dependent inference features
- Optional: [Tesseract OCR](https://github.com/tesseract-ocr/tesseract) for OCR functionality

## Quick Start

### Basic Extraction

Extract text, metadata, and structure from any supported document format:

```java title="Java"
import io.xberg.ExtractInput;
import io.xberg.ExtractInputKind;
import io.xberg.ExtractedDocument;
import io.xberg.ExtractionConfig;
import io.xberg.Xberg;
import io.xberg.ExtractionResult;
import io.xberg.XbergRsException;

public class BasicUsage {
    public static void main(String[] args) throws XbergRsException {
        ExtractInput input = ExtractInput.builder()
            .withKind(ExtractInputKind.Uri)
            .withUri("document.pdf")
            .build();

        ExtractionResult output = Xberg.extract(input, ExtractionConfig.builder().build());
        ExtractedDocument document = output.results().get(0);

        System.out.println("Content:");
        System.out.println(document.content());

        System.out.println("\nMetadata:");
        if (document.metadata().title() != null) {
            System.out.println("Title: " + document.metadata().title());
        }
        if (document.metadata().authors() != null) {
            System.out.println("Authors: " + String.join(", ", document.metadata().authors()));
        }

        System.out.println("\nTables found: " + document.tables().size());
        System.out.println("Images found: " + (document.images() == null ? 0 : document.images().size()));
    }
}
```

### Common Use Cases

#### Extract with Custom Configuration

Most use cases benefit from configuration to control extraction behavior:

**With OCR (for scanned documents):**

```java title="Java"
import io.xberg.ExtractInput;
import io.xberg.ExtractInputKind;
import io.xberg.ExtractedDocument;
import io.xberg.ExtractionConfig;
import io.xberg.ExtractionResult;
import io.xberg.OcrConfig;
import io.xberg.Xberg;
import io.xberg.XbergRsException;
import java.util.List;

public class Main {
    public static void main(String[] args) {
        try {
            ExtractionConfig config = ExtractionConfig.builder()
                .withForceOcr(true)
                .withOcr(OcrConfig.builder()
                    .withBackend("tesseract")
                    .withLanguage(List.of("eng"))
                    .build())
                .build();

            ExtractInput input = ExtractInput.builder()
                .withKind(ExtractInputKind.Uri)
                .withUri("scanned.pdf")
                .build();

            ExtractionResult output = Xberg.extract(input, config);
            ExtractedDocument document = output.results().get(0);
            System.out.println(document.content());
        } catch (XbergRsException e) {
            System.err.println("Extraction failed: " + e.getMessage());
        }
    }
}
```

#### Table Extraction

See [Configuration Guide](https://docs.xberg.io/guides/configuration/) for table extraction options.

#### Processing Multiple Files

```java title="Java"
import io.xberg.ExtractInput;
import io.xberg.ExtractInputKind;
import io.xberg.ExtractionConfig;
import io.xberg.Xberg;
import java.nio.charset.StandardCharsets;
import java.util.List;

var inputs = List.of(
    ExtractInput.builder()
        .withKind(ExtractInputKind.Uri)
        .withUri("document.pdf")
        .build(),
    ExtractInput.builder()
        .withKind(ExtractInputKind.Bytes)
        .withBytes("Hello from memory".getBytes(StandardCharsets.UTF_8))
        .withMimeType("text/plain")
        .withFilename("note.txt")
        .build()
);

var output = Xberg.extractBatch(inputs, ExtractionConfig.builder().build());
for (var result : output.results()) {
    System.out.println(result.content());
}
```

#### Async Processing

For non-blocking document processing:

```java title="Java"
import io.xberg.ExtractInput;
import io.xberg.ExtractInputKind;
import io.xberg.ExtractionConfig;
import io.xberg.Xberg;

var input = ExtractInput.builder()
    .withKind(ExtractInputKind.Uri)
    .withUri("document.pdf")
    .build();

var output = Xberg.extract(input, ExtractionConfig.builder().build());
System.out.println(output.results().get(0).content());
```

### Next Steps

- **[Installation Guide](https://docs.xberg.io/getting-started/installation/)** - Platform-specific setup
- **[API Documentation](https://docs.xberg.io/reference/api-python/)** - Complete API reference
- **[Examples & Guides](https://docs.xberg.io/)** - Full code examples and usage guides
- **[Configuration Guide](https://docs.xberg.io/guides/configuration/)** - Advanced configuration options

## Features

### Supported File Formats (96)

97 file formats across 8 major categories with intelligent format detection and comprehensive metadata extraction.

#### Office Documents

| Category | Formats | Capabilities |
|----------|---------|--------------|
| **Word Processing** | `.docx`, `.docm`, `.doc`, `.dotx`, `.dotm`, `.dot`, `.odt`, `.pages` | Full text, tables, images, metadata, styles |
| **Spreadsheets** | `.xlsx`, `.xlsm`, `.xlsb`, `.xls`, `.xla`, `.xlam`, `.xltm`, `.xltx`, `.xlt`, `.ods`, `.numbers` | Sheet data, formulas, cell metadata, charts |
| **Presentations** | `.pptx`, `.pptm`, `.ppt`, `.ppsx`, `.potx`, `.potm`, `.pot`, `.odp`, `.key` | Slides, speaker notes, images, metadata |
| **PDF** | `.pdf` | Text, tables, images, metadata, OCR support |
| **eBooks** | `.epub`, `.fb2` | Chapters, metadata, embedded resources |
| **Database** | `.dbf` | Table data extraction, field type support |
| **Hangul** | `.hwp`, `.hwpx` | Korean document format, text extraction |

#### Images (OCR-Enabled)

| Category | Formats | Features |
|----------|---------|----------|
| **Raster** | `.png`, `.jpg`, `.jpeg`, `.gif`, `.webp`, `.bmp`, `.tiff`, `.tif` | OCR, table detection, EXIF metadata, dimensions, color space |
| **Advanced** | `.jp2`, `.jpx`, `.jpm`, `.mj2`, `.jbig2`, `.jb2`, `.pnm`, `.pbm`, `.pgm`, `.ppm` | OCR via hayro-jpeg2000 (pure Rust decoder), JBIG2 support, table detection, format-specific metadata |
| **HEIC family** | `.heic`, `.heics`, `.heif`, `.avif`, `.avcs` | EXIF metadata, optional libheif pixel decoding |
| **Vector** | `.svg` | DOM parsing, embedded text, graphics metadata |

#### Audio & Video

| Category | Formats | Features |
|----------|---------|----------|
| **Audio** | `.mp3`, `.mpga`, `.m4a`, `.wav`, `.webm` | Whisper transcription when native transcription is available |
| **Video audio track** | `.mp4`, `.mpeg`, `.webm` | Audio-track transcription only |

#### Web & Data

| Category | Formats | Features |
|----------|---------|----------|
| **Markup** | `.html`, `.htm`, `.xhtml`, `.xml`, `.svg` | DOM parsing, metadata (Open Graph, Twitter Card), link extraction |
| **Structured Data** | `.json`, `.yaml`, `.yml`, `.toml`, `.csv`, `.tsv` | Schema detection, nested structures, validation |
| **Text & Markdown** | `.txt`, `.md`, `.markdown`, `.djot`, `.mdx`, `.rst`, `.org`, `.rtf` | CommonMark, GFM, Djot, MDX, reStructuredText, Org Mode |

#### Email & Archives

| Category | Formats | Features |
|----------|---------|----------|
| **Email** | `.eml`, `.msg`, `.pst` | Headers, body (HTML/plain), attachments, threading |
| **Archives** | `.zip`, `.tar`, `.tgz`, `.gz`, `.7z` | File listing, nested archives, metadata |

#### Academic & Scientific

| Category | Formats | Features |
|----------|---------|----------|
| **Citations** | `.bib`, `.ris`, `.nbib`, `.enw` | Structured parsing: RIS, PubMed/MEDLINE, EndNote XML, BibTeX/BibLaTeX, CSL JSON by MIME type |
| **Scientific** | `.tex`, `.latex`, `.typ`, `.typst`, `.jats`, `.ipynb` | LaTeX, Typst, Jupyter notebooks, PubMed JATS |
| **Publishing** | `.fb2`, `.docbook`, `.dbk`, `.docbook4`, `.docbook5`, `.opml` | FictionBook, DocBook XML, OPML outlines |
| **Documentation** | MIME-only POD, mdoc, troff | Technical documentation formats |

#### Code Intelligence (306 Languages)

| Feature | Description |
|---------|-------------|
| **Structure Extraction** | Functions, classes, methods, structs, interfaces, enums |
| **Import/Export Analysis** | Module dependencies, re-exports, wildcard imports |
| **Symbol Extraction** | Variables, constants, type aliases, properties |
| **Docstring Parsing** | Google, NumPy, Sphinx, JSDoc, RustDoc, and 10+ formats |
| **Diagnostics** | Parse errors with line/column positions |
| **Syntax-Aware Chunking** | Split code by semantic boundaries, not arbitrary byte offsets |

Powered by [tree-sitter-language-pack](https://github.com/xberg-io/tree-sitter-language-pack) — [documentation](https://docs.tree-sitter-language-pack.xberg.io).

**[Complete Format Reference](https://docs.xberg.io/reference/formats/)**

### Key Capabilities

- **Text Extraction** - Extract all text content with position and formatting information
- **Metadata Extraction** - Retrieve document properties, creation date, author, etc.
- **Table Extraction** - Parse tables with structure and cell content preservation
- **Image Extraction** - Extract embedded images and render page previews
- **Audio/Video Transcription** - Extract speech transcripts from MP3, M4A, WAV, WebM, and MP4 inputs when the native transcription feature is available
- **OCR Support** - Integrate multiple OCR backends for scanned documents
- **Async/Await** - Non-blocking document processing with concurrent operations
- **Plugin System** - Extensible post-processing for custom text transformation
- **Embeddings** - Generate vector embeddings using ONNX Runtime models or provider-hosted services
- **Batch Processing** - Efficiently process multiple documents in parallel
- **Memory Efficient** - Stream large files without loading entirely into memory
- **Language Detection** - Detect and support multiple languages in documents
- **Code Intelligence** - Extract structure, imports, exports, symbols, and docstrings from [306 programming languages](https://docs.tree-sitter-language-pack.xberg.io) via tree-sitter
- **Configuration** - Fine-grained control over extraction behavior
- **Six Output Formats** - Plain text, Markdown, Djot, HTML, JSON tree structure, or Structured JSON with OCR metadata

## OCR Support

Xberg supports multiple OCR backends for extracting text from scanned documents and images:

- **Tesseract**

- **Paddleocr**

### OCR Configuration Example

```java title="Java"
import io.xberg.ExtractInput;
import io.xberg.ExtractInputKind;
import io.xberg.ExtractedDocument;
import io.xberg.ExtractionConfig;
import io.xberg.ExtractionResult;
import io.xberg.OcrConfig;
import io.xberg.Xberg;
import io.xberg.XbergRsException;
import java.util.List;

public class Main {
    public static void main(String[] args) {
        try {
            ExtractionConfig config = ExtractionConfig.builder()
                .withForceOcr(true)
                .withOcr(OcrConfig.builder()
                    .withBackend("tesseract")
                    .withLanguage(List.of("eng"))
                    .build())
                .build();

            ExtractInput input = ExtractInput.builder()
                .withKind(ExtractInputKind.Uri)
                .withUri("scanned.pdf")
                .build();

            ExtractionResult output = Xberg.extract(input, config);
            ExtractedDocument document = output.results().get(0);
            System.out.println(document.content());
        } catch (XbergRsException e) {
            System.err.println("Extraction failed: " + e.getMessage());
        }
    }
}
```

## Async Support

This binding provides full async/await support for non-blocking document processing:

```java title="Java"
import io.xberg.ExtractInput;
import io.xberg.ExtractInputKind;
import io.xberg.ExtractionConfig;
import io.xberg.Xberg;

var input = ExtractInput.builder()
    .withKind(ExtractInputKind.Uri)
    .withUri("document.pdf")
    .build();

var output = Xberg.extract(input, ExtractionConfig.builder().build());
System.out.println(output.results().get(0).content());
```

## Plugin System

Xberg supports extensible post-processing plugins for custom text transformation and filtering.

For detailed plugin documentation, visit [Plugin System Guide](https://docs.xberg.io/guides/plugins/).

## Embeddings Support

Generate vector embeddings for extracted text using the built-in ONNX Runtime support. Requires ONNX Runtime installation.

**[Embeddings Guide](https://docs.xberg.io/features/#embeddings)**

## Batch Processing

Process multiple documents efficiently:

```java title="Java"
import io.xberg.ExtractInput;
import io.xberg.ExtractInputKind;
import io.xberg.ExtractionConfig;
import io.xberg.Xberg;
import java.nio.charset.StandardCharsets;
import java.util.List;

var inputs = List.of(
    ExtractInput.builder()
        .withKind(ExtractInputKind.Uri)
        .withUri("document.pdf")
        .build(),
    ExtractInput.builder()
        .withKind(ExtractInputKind.Bytes)
        .withBytes("Hello from memory".getBytes(StandardCharsets.UTF_8))
        .withMimeType("text/plain")
        .withFilename("note.txt")
        .build()
);

var output = Xberg.extractBatch(inputs, ExtractionConfig.builder().build());
for (var result : output.results()) {
    System.out.println(result.content());
}
```

## Configuration

For advanced configuration options including language detection, table extraction, OCR settings, and more:

**[Configuration Guide](https://docs.xberg.io/guides/configuration/)**

## Documentation

- **[Official Documentation](https://docs.xberg.io/)**
- **[API Reference](https://docs.xberg.io/reference/api-python/)**
- **[Examples & Guides](https://docs.xberg.io/)**

## Contributing

Contributions are welcome! See [Contributing Guide](https://github.com/xberg-io/xberg/blob/main/CONTRIBUTING.md).

## Part of Xberg.dev

- [crawlberg](https://github.com/xberg-io/crawlberg) — web crawling and scraping with HTML→Markdown and headless-Chrome fallback.
- [html-to-markdown](https://github.com/xberg-io/html-to-markdown) — fast, lossless HTML→Markdown engine.
- [liter-llm](https://github.com/xberg-io/liter-llm) — universal LLM API client with native bindings for 14 languages and 143 providers.
- [tree-sitter-language-pack](https://github.com/xberg-io/tree-sitter-language-pack) — tree-sitter grammars and code-intelligence primitives.
- [alef](https://github.com/xberg-io/alef) — the polyglot binding generator that produces this README and all per-language bindings.
- [Discord](https://discord.gg/xt9WY3GnKR) — community, roadmap, announcements.

## License

MIT License — see [LICENSE](../../LICENSE) for details.

## Support

- **Discord Community**: [Join our Discord](https://discord.gg/xt9WY3GnKR)
- **GitHub Issues**: [Report bugs](https://github.com/xberg-io/xberg/issues)
- **Discussions**: [Ask questions](https://github.com/xberg-io/xberg/discussions)
