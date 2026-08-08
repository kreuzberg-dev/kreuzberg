# xberg-cli

[![Bindings](https://img.shields.io/badge/Bindings-alef%20%D7%90-007ec6)](https://github.com/xberg-io/alef)

[![Docker](https://img.shields.io/badge/Docker-ghcr.io-007ec6?logo=docker&logoColor=white)](https://github.com/xberg-io/xberg/pkgs/container/xberg)
[![Helm chart](https://img.shields.io/badge/Helm-chart-007ec6?logo=helm&logoColor=white)](https://docs.xberg.io/guides/kubernetes/)

Command-line interface for the Xberg document intelligence library.

## Overview

This crate provides a production-ready CLI tool for document extraction, MIME type detection, batch processing, embeddings, chunking, and cache management. It exposes the core extraction capabilities of the Xberg Rust library through an easy-to-use command-line interface.

The CLI supports 101 file formats across 115 file extensions including PDF, DOCX, PPTX, XLSX, images, HTML, and more, with optional OCR support for scanned documents.

## Architecture

### Binary Structure

```text
Xberg Core Library (crates/xberg)
    |
Xberg CLI (crates/xberg-cli) <- This crate
    |
Command-line interface with configuration and caching
```

### Commands

| Command       | Description                                                                                            |
| ------------- | ------------------------------------------------------------------------------------------------------ |
| `extract`     | Extract text from a document                                                                           |
| `batch`       | Batch extract from multiple documents                                                                  |
| `detect`      | Detect MIME type of a file                                                                             |
| `formats`     | List all supported document formats                                                                    |
| `version`     | Show version information                                                                               |
| `cache`       | Cache management (stats, clear, warm, manifest)                                                        |
| `serve`       | Start the API server (requires `api` feature)                                                          |
| `mcp`         | Start the MCP server (requires `mcp` feature)                                                          |
| `api`         | API utilities (schema) (requires `api` feature)                                                        |
| `embed`       | Generate embeddings for text (requires `embeddings` feature) <span class="version-badge">v4.5.2</span> |
| `chunk`       | Chunk text for processing <span class="version-badge">v4.5.2</span>                                    |
| `completions` | Generate shell completions <span class="version-badge">v4.5.2</span>                                   |

### Platform Support

The CLI is tested and officially supported on:

- Linux x86_64 (glibc and musl static)
- Linux aarch64 / ARM64 (glibc and musl static)
- macOS aarch64 (Apple Silicon)
- Windows x86_64

All platforms receive precompiled binaries through GitHub releases. Linux musl binaries are fully statically linked with zero runtime dependencies.

## Installation

### Install Script (Linux / macOS)

```bash
curl -fsSL https://raw.githubusercontent.com/xberg-io/xberg/main/scripts/install.sh | bash
```

### Homebrew

Homebrew 6.0+ requires explicit trust for third-party taps. Trust once, then install:

```bash
brew trust xberg-io/tap
brew install xberg-io/tap/xberg
```

### Cargo

```bash
cargo install xberg-cli
```

### Docker

```bash
docker pull ghcr.io/xberg-io/xberg-cli:latest
docker run -v $(pwd):/data ghcr.io/xberg-io/xberg-cli:latest extract /data/document.pdf
```

### From Source

```bash
cargo install --path crates/xberg-cli
```

Or via the workspace:

```bash
cargo build --release -p xberg-cli
```

### Platform-Specific Requirements

#### ONNX Runtime (for embeddings)

If using embeddings functionality, ONNX Runtime must be installed:

```bash
# macOS
brew install onnxruntime

# Ubuntu/Debian
sudo apt install libonnxruntime libonnxruntime-dev

# Windows (MSVC)
scoop install onnxruntime
# OR download from https://github.com/microsoft/onnxruntime/releases
```

Without ONNX Runtime, embeddings will raise `MissingDependencyError` with installation instructions.

#### OCR Support (Optional)

To enable optical character recognition for scanned documents:

- **macOS**: `brew install tesseract`
- **Ubuntu/Debian**: `sudo apt-get install tesseract-ocr`
- **Windows**: Download from [tesseract-ocr/tesseract](https://github.com/tesseract-ocr/tesseract)

## Quick Start

> The CLI is available for Linux (x86_64/aarch64), macOS (Apple Silicon), and Windows with consistent behavior across all platforms.

### Basic Text Extraction

```bash
# Extract text from a PDF
xberg extract document.pdf

# Extract with JSON output
xberg extract document.pdf --format json
```

### Extract with OCR

```bash
# Enable OCR for scanned documents
xberg extract scanned.pdf --ocr true

# Force OCR even if text extraction succeeds
xberg extract mixed.pdf --force-ocr true
```

### Batch Processing

```bash
# Process multiple documents in parallel
xberg batch *.pdf --format json

# Process with custom configuration
xberg batch documents/*.docx --config config.toml --format json
```

### MIME Type Detection

```bash
# Detect file type
xberg detect unknown-file

# JSON output
xberg detect unknown-file --format json
```

### Generate Embeddings (with `embeddings` feature)

```bash
# Embed a single text
xberg embed --text "hello world" --preset balanced

# Embed multiple texts
xberg embed --text "first" --text "second" --format json

# Read from stdin
echo "some text" | xberg embed --preset fast
```

### Chunk Text

```bash
# Chunk text from flag
xberg chunk --text "Long document content..." --chunk-size 512

# Chunk from stdin with overlap
echo "Long document content..." | xberg chunk --chunk-size 512 --chunk-overlap 64

# Use markdown-aware chunking
xberg chunk --text "# Heading\nContent..." --chunker-type markdown
```

### Cache Management

```bash
# View cache statistics
xberg cache stats

# Clear the cache
xberg cache clear --cache-dir /path/to/cache

# Pre-download all models
xberg cache warm

# Pre-download models including all embedding presets
xberg cache warm --all-embeddings

# Show model manifest (paths, checksums, sizes)
xberg cache manifest
```

### Shell Completions

```bash
# Bash
eval "$(xberg completions bash)"

# Zsh
xberg completions zsh > ~/.zfunc/_xberg

# Fish
xberg completions fish | source
```

### API Server (with `api` feature)

```bash
# Start API server on localhost:8000
xberg serve

# Custom host and port
xberg serve --host 0.0.0.0 --port 3000

# With configuration file
xberg serve --config xberg.toml --host 127.0.0.1 --port 8080
```

### API Utilities (with `api` feature)

```bash
# Dump the OpenAPI 3.1 schema
xberg api schema
```

### MCP Server (with `mcp` feature)

```bash
# Start Model Context Protocol server (stdio transport)
xberg mcp

# With HTTP transport
xberg mcp --transport http --host 127.0.0.1 --port 8001

# With configuration file
xberg mcp --config xberg.toml
```

## Global Flags

| Flag                  | Description                                                                                                                        |
| --------------------- | ---------------------------------------------------------------------------------------------------------------------------------- |
| `--log-level <LEVEL>` | Set log level (`trace`, `debug`, `info`, `warn`, `error`). Overrides `RUST_LOG` env var. <span class="version-badge">v4.5.2</span> |

## Configuration

The CLI supports configuration files in TOML, YAML, or JSON formats. Configuration can be:

1. **Explicit**: Passed via `--config /path/to/config.{toml,yaml,json}`
2. **Auto-discovered**: Searches for `xberg.{toml,yaml,json}` in current and parent directories
3. **Inline JSON**: Passed via `--config-json '{"ocr":{"backend":"tesseract"}}'`
4. **Base64 JSON**: Passed via `--config-json-base64 <BASE64>` (useful when shell quoting is tricky)
5. **Default**: Uses built-in defaults if no config found

Configuration precedence (highest to lowest):

1. Individual CLI flags (`--ocr`, `--chunk-size`, etc.)
2. Inline JSON config (`--config-json` or `--config-json-base64`)
3. Config file (`--config path.toml`)
4. Default values

### Example Configuration (TOML)

```toml
# Basic extraction settings
use_cache = true
enable_quality_processing = true
force_ocr = false

# OCR configuration
[ocr]
backend = "tesseract"
language = "eng"

[ocr.tesseract_config]
enable_table_detection = true
psm = 6
min_confidence = 50.0

# Text chunking (useful for LLM processing)
[chunking]
max_chars = 1000
max_overlap = 200

# PDF-specific options
[pdf_options]
extract_images = true
extract_metadata = true
passwords = []

# Language detection
[language_detection]
enabled = true
min_confidence = 0.8
detect_multiple = false

# Image extraction
[images]
extract_images = true
target_dpi = 300
max_image_dimension = 4096
auto_adjust_dpi = true
```

### Configuration Overrides

Command-line flags override configuration file settings:

```bash
# Override OCR setting from config
xberg extract document.pdf --config config.toml --ocr false

# Override chunking settings
xberg extract long.pdf --chunk true --chunk-size 2000 --chunk-overlap 400

# Disable cache despite config file
xberg extract document.pdf --no-cache true

# Enable language detection
xberg extract multilingual.pdf --detect-language true
```

## Command Reference

### extract

Extract text, tables, and metadata from a document.

```bash
xberg extract <PATH> [OPTIONS]
```

**Options:**

- `--config <PATH>`: Configuration file (TOML, YAML, or JSON)
- `--config-json <JSON>`: Inline JSON configuration
- `--config-json-base64 <BASE64>`: Base64-encoded JSON configuration
- `--mime-type <TYPE>`: MIME type hint (auto-detected if not provided)
- `--format <FORMAT>`: Output format (`text` or `json`), default: `text`

**Extraction override flags** <span class="version-badge">v4.5.2</span> (also available on `batch`):

| Flag                                   | Description                                                                                                |
| -------------------------------------- | ---------------------------------------------------------------------------------------------------------- |
| `--ocr <true\|false>`                  | Enable/disable OCR                                                                                         |
| `--ocr-backend <BACKEND>`              | OCR backend: `tesseract`, `paddle-ocr`, `vlm`, or `candle-*`                                               |
| `--ocr-language <LANG>`                | OCR language code (e.g. `eng`, `fra`, `ch`)                                                                |
| `--ocr-auto-rotate <true\|false>`      | Auto-rotate images before OCR                                                                              |
| `--force-ocr <true\|false>`            | Force OCR even if text extraction succeeds                                                                 |
| `--no-cache <true\|false>`             | Disable result caching                                                                                     |
| `--chunk <true\|false>`                | Enable text chunking                                                                                       |
| `--chunk-size <SIZE>`                  | Maximum chunk size in characters (default: 1000)                                                           |
| `--chunk-overlap <SIZE>`               | Overlap between chunks in characters (default: 200)                                                        |
| `--chunking-tokenizer <MODEL>`         | Tokenizer model for token-based sizing (e.g. `Xenova/gpt-4o`)                                              |
| `--content-format <FORMAT>`            | Content format: `plain`, `markdown`, `djot`, `html`                                                        |
| `--include-structure <true\|false>`    | Include hierarchical document structure                                                                    |
| `--quality <true\|false>`              | Enable quality post-processing                                                                             |
| `--detect-language <true\|false>`      | Enable language detection                                                                                  |
| `--layout`                             | Enable layout detection (RT-DETR v2) (enables with defaults, use `--layout false` to disable)              |
| `--layout-confidence <FLOAT>`          | Layout confidence threshold (0.0 - 1.0)                                                                    |
| `--acceleration <PROVIDER>`            | ONNX execution provider: `auto`, `cpu`, `coreml`, `cuda`, `tensorrt`                                       |
| `--max-concurrent <N>`                 | Max parallel extractions in batch mode                                                                     |
| `--max-threads <N>`                    | Cap all internal thread pools                                                                              |
| `--extract-pages <true\|false>`        | Extract pages as separate array                                                                            |
| `--page-markers <true\|false>`         | Insert page marker comments                                                                                |
| `--extract-images <true\|false>`       | Enable image extraction                                                                                    |
| `--target-dpi <DPI>`                   | Target DPI for images (36 - 2400)                                                                          |
| `--pdf-password <PASS>`                | Password for encrypted PDFs (repeatable)                                                                   |
| `--pdf-extract-images <true\|false>`   | Extract images from PDF pages                                                                              |
| `--pdf-extract-metadata <true\|false>` | Extract PDF metadata                                                                                       |
| `--token-reduction <LEVEL>`            | Token reduction: `off`, `light`, `moderate`, `aggressive`, `maximum`                                       |
| `--layout-table-model <MODEL>`         | Table structure model: `tatr`, `slanet_wired`, `slanet_wireless`, `slanet_plus`, `slanet_auto`, `disabled` |
| `--disable-ocr <true\|false>`          | Disable OCR entirely (even for images)                                                                     |
| `--cache-namespace <NAMESPACE>`        | Cache namespace for tenant isolation                                                                       |
| `--cache-ttl-secs <SECONDS>`           | Per-request cache TTL in seconds (0 = skip cache)                                                          |
| `--msg-codepage <CODE>`                | Windows codepage fallback for MSG files                                                                    |

**Examples:**

```bash
# Simple extraction
xberg extract invoice.pdf

# With configuration and JSON output
xberg extract document.pdf --config config.toml --format json

# With chunking for LLM processing
xberg extract report.pdf --chunk true --chunk-size 2000

# With OCR for scanned document
xberg extract scanned.pdf --ocr true --format json

# Markdown output with page markers
xberg extract report.pdf --content-format markdown --page-markers true

# Layout-aware extraction with GPU acceleration
xberg extract document.pdf --layout --content-format markdown --acceleration coreml

# GPU-accelerated extraction
xberg extract scanned.pdf --ocr true --acceleration coreml
```

### batch

Process multiple documents in parallel.

```bash
xberg batch <PATHS>... [OPTIONS]
```

**Options:**

- `--config <PATH>`: Configuration file (TOML, YAML, or JSON)
- `--config-json <JSON>`: Inline JSON configuration
- `--config-json-base64 <BASE64>`: Base64-encoded JSON configuration
- `--format <FORMAT>`: Output format (`text` or `json`), default: `json`
- `--file-configs <PATH>`: JSON file mapping per-file extraction overrides
- All extraction override flags (see `extract` above)

**Examples:**

```bash
# Batch process multiple files
xberg batch doc1.pdf doc2.docx doc3.xlsx

# With glob patterns
xberg batch *.pdf *.docx

# With custom configuration
xberg batch documents/* --config batch-config.toml --format json

# With OCR and concurrency limit
xberg batch scanned/*.pdf --ocr true --max-concurrent 4 --format json

# Per-file overrides
xberg batch doc1.pdf doc2.pdf --file-configs overrides.json
```

### detect

Identify the MIME type of a file.

```bash
xberg detect <PATH> [OPTIONS]
```

**Options:**

- `--format <FORMAT>`: Output format (`text` or `json`), default: `text`

**Examples:**

```bash
# Simple detection
xberg detect unknown-file

# JSON output
xberg detect mystery.bin --format json
```

### formats

List all supported document formats.

```bash
xberg formats [OPTIONS]
```

**Options:**

- `--format <FORMAT>`: Output format (`text` or `json`), default: `text`

**Examples:**

```bash
# List formats as table
xberg formats

# JSON output for tooling
xberg formats --format json
```

### cache

Manage extraction result cache and model downloads.

```bash
xberg cache <COMMAND> [OPTIONS]
```

**Subcommands:**

#### stats

Show cache statistics.

```bash
xberg cache stats [--cache-dir <DIR>] [--format <FORMAT>]
```

**Options:**

- `--cache-dir <DIR>`: Cache directory (default: `.xberg` in current directory)
- `--format <FORMAT>`: Output format (`text` or `json`), default: `text`

#### clear

Clear the cache.

```bash
xberg cache clear [--cache-dir <DIR>] [--format <FORMAT>]
```

**Options:**

- `--cache-dir <DIR>`: Cache directory (default: `.xberg` in current directory)
- `--format <FORMAT>`: Output format (`text` or `json`), default: `text`

#### warm

Pre-download all models (OCR, layout detection, and optionally embeddings).

```bash
xberg cache warm [--cache-dir <DIR>] [--format <FORMAT>] [--all-embeddings] [--embedding-model <PRESET>]
```

**Options:**

- `--cache-dir <DIR>`: Cache directory (default: `.xberg` or `XBERG_CACHE_DIR`)
- `--format <FORMAT>`: Output format (`text` or `json`), default: `text`
- `--all-embeddings`: Download all 4 embedding model presets (fast, balanced, quality, multilingual)
- `--embedding-model <PRESET>`: Download a specific embedding model preset

#### manifest

Output model manifest (expected model files, checksums, sizes).

```bash
xberg cache manifest [--format <FORMAT>]
```

**Options:**

- `--format <FORMAT>`: Output format (`text` or `json`), default: `json`

**Examples:**

```bash
# View cache statistics
xberg cache stats

# Clear cache with custom directory
xberg cache clear --cache-dir ~/.xberg-cache

# Pre-download all models for offline/container use
xberg cache warm

# Also download embedding models
xberg cache warm --all-embeddings

# Download only the fast embedding model
xberg cache warm --embedding-model fast

# Get model manifest as JSON
xberg cache manifest
```

### serve (requires `api` feature)

Start the REST API server.

```bash
xberg serve [OPTIONS]
```

**Options:**

- `-H, --host <HOST>`: Host to bind to (default: `127.0.0.1`)
- `--port <PORT>`: Port to bind to (default: `8000`)
- `--config <PATH>`: Configuration file (TOML, YAML, or JSON)

**Examples:**

```bash
# Default: localhost:8000
xberg serve

# Public access on port 3000
xberg serve --host 0.0.0.0 --port 3000

# With custom configuration
xberg serve --config server-config.toml --port 8080
```

### mcp (requires `mcp` feature)

Start the Model Context Protocol server.

```bash
xberg mcp [OPTIONS]
```

**Options:**

- `--config <PATH>`: Configuration file (TOML, YAML, or JSON)
- `--transport <MODE>`: Transport mode: `stdio` (default) or `http`
- `--host <HOST>`: HTTP host (only for `--transport http`, default: `127.0.0.1`)
- `--port <PORT>`: HTTP port (only for `--transport http`, default: `8001`)

**Examples:**

```bash
# Start MCP server (stdio, for editor integration)
xberg mcp

# HTTP transport for remote access
xberg mcp --transport http --host 0.0.0.0 --port 8001

# With custom configuration
xberg mcp --config mcp-config.toml
```

### api (requires `api` feature)

API utility commands.

#### schema <span class="version-badge">v4.5.2</span>

Output the full OpenAPI 3.1 specification as JSON.

```bash
xberg api schema
```

**Examples:**

```bash
# Dump OpenAPI spec to file
xberg api schema > openapi.json

# Pipe to jq for inspection
xberg api schema | jq '.paths | keys'
```

### embed <span class="version-badge">v4.5.2</span> (requires `embeddings` feature)

Generate vector embeddings for text.

```bash
xberg embed [OPTIONS]
```

**Options:**

- `--text <TEXT>`: Text to embed (repeatable for batch embedding; reads from stdin if omitted)
- `--preset <PRESET>`: Embedding preset (`fast`, `balanced`, `quality`, `multilingual`), default: `balanced`
- `--format <FORMAT>`: Output format (`text` or `json`), default: `json`

**Examples:**

```bash
# Embed a single text
xberg embed --text "hello world"

# Batch embed multiple texts
xberg embed --text "first" --text "second"

# Use a specific preset
xberg embed --text "bonjour" --preset multilingual

# Read from stdin
cat document.txt | xberg embed --preset fast
```

### chunk <span class="version-badge">v4.5.2</span>

Chunk text for processing (useful for LLM context windows).

```bash
xberg chunk [OPTIONS]
```

**Options:**

- `--text <TEXT>`: Text to chunk (reads from stdin if omitted)
- `--config <PATH>`: Configuration file (TOML, YAML, or JSON)
- `--chunk-size <SIZE>`: Chunk size in characters
- `--chunk-overlap <SIZE>`: Chunk overlap in characters
- `--chunker-type <TYPE>`: Chunker type: `text` (default), `markdown`, `yaml`, or `semantic`
- `--chunking-tokenizer <MODEL>`: Tokenizer model for token-based sizing (e.g. `Xenova/gpt-4o`)
- `--format <FORMAT>`: Output format (`text` or `json`), default: `json`

**Examples:**

```bash
# Chunk text with defaults
xberg chunk --text "Long document content here..."

# Custom chunk size and overlap
xberg chunk --text "..." --chunk-size 512 --chunk-overlap 64

# Markdown-aware chunking
xberg chunk --text "# Title\nContent..." --chunker-type markdown

# Token-based chunking with specific tokenizer
xberg chunk --text "..." --chunking-tokenizer "Xenova/gpt-4o"

# Read from stdin
cat long-document.txt | xberg chunk --chunk-size 1000
```

### completions <span class="version-badge">v4.5.2</span>

Generate shell completion scripts.

```bash
xberg completions <SHELL>
```

**Supported shells:** `bash`, `zsh`, `fish`, `elvish`, `powershell`

**Examples:**

```bash
# Bash (add to .bashrc)
eval "$(xberg completions bash)"

# Zsh (add to fpath)
xberg completions zsh > ~/.zfunc/_xberg

# Fish
xberg completions fish | source

# PowerShell
xberg completions powershell | Out-String | Invoke-Expression
```

### version

Show version information.

```bash
xberg version [--format <FORMAT>]
```

**Options:**

- `--format <FORMAT>`: Output format (`text` or `json`), default: `text`

**Examples:**

```bash
# Display version
xberg version

# JSON output
xberg version --format json
```

## Output Formats

### Text Format

The default human-readable format:

```bash
xberg extract document.pdf
# Output:
# Document content here...
```

### JSON Format

For programmatic integration:

```bash
xberg extract document.pdf --format json
# Output:
# {
#   "content": "Document content...",
#   "mime_type": "application/pdf",
#   "metadata": { "title": "...", "author": "..." },
#   "tables": [{ "markdown": "...", "cells": [...], "page_number": 0 }]
# }
```

## Supported File Formats

| Category      | Formats                                                  |
| ------------- | -------------------------------------------------------- |
| **Documents** | PDF, DOCX, DOC, PPTX, PPT, XLSX, XLS, ODT, ODP, ODS, RTF |
| **Images**    | PNG, JPEG, JPG, WEBP, BMP, TIFF, GIF                     |
| **Web**       | HTML, XHTML, XML                                         |
| **Text**      | TXT, MD, CSV, TSV, JSON, YAML, TOML                      |
| **Email**     | EML, MSG                                                 |
| **Archives**  | ZIP, TAR, 7Z                                             |
| **Other**     | 30+ additional formats                                   |

## Exit Codes

- `0`: Successful execution
- `Non-zero`: Error occurred (check stderr for details)

## Logging

Control logging verbosity with the `--log-level` flag or `RUST_LOG` environment variable:

```bash
# Using --log-level flag (overrides RUST_LOG)
xberg --log-level debug extract document.pdf
xberg --log-level warn batch *.pdf

# Using RUST_LOG environment variable
RUST_LOG=info xberg extract document.pdf
RUST_LOG=debug xberg extract document.pdf
RUST_LOG=warn xberg extract document.pdf

# Show logs from specific modules
RUST_LOG=xberg=debug xberg extract document.pdf
```

## Performance Tips

1. **Use batch processing** for multiple files instead of sequential extraction:

   ```bash
   xberg batch *.pdf  # Parallel processing
   ```

2. **Enable caching** to avoid reprocessing the same documents:

   ```bash
   # Cache is enabled by default
   xberg extract document.pdf
   ```

3. **Use appropriate chunk sizes** for LLM processing:

   ```bash
   xberg extract long.pdf --chunk true --chunk-size 2000
   ```

4. **Tune OCR settings** for better performance:

   ```bash
   xberg extract scanned.pdf --ocr true
   # Adjust tesseract_config in configuration file for optimization
   ```

5. **Monitor cache size** and clear when needed:

   ```bash
   xberg cache stats
   xberg cache clear
   ```

6. **Pre-warm models** for containerized deployments:

   ```bash
   xberg cache warm --all-embeddings
   ```

7. **Use hardware acceleration** when available:

   ```bash
   xberg extract scanned.pdf --ocr true --acceleration coreml  # macOS
   xberg extract scanned.pdf --ocr true --acceleration cuda    # NVIDIA GPU
   ```

## Features

### Default Features

None by default. The binary includes core extraction.

### Optional Features

- **`api`**: Enable the REST API server (`xberg serve` command) and API utilities (`xberg api schema`)
- **`mcp`**: Enable Model Context Protocol server (`xberg mcp` command)
- **`embeddings`**: Enable embedding generation (`xberg embed` command)
- **`layout-detection`**: Enable layout detection flags (`--layout`, `--layout-confidence`)
- **`chunking-tokenizers`**: Enable token-based chunking (`--chunking-tokenizer`)
- **`all`**: Enable all features (`api` + `mcp`)

### Building with Features

```bash
# Build with all features
cargo build --release -p xberg-cli --features all

# Build with specific features
cargo build --release -p xberg-cli --features api,mcp,embeddings
```

## Troubleshooting

### File Not Found Error

Ensure the file path is correct and the file is readable:

```bash
# Check if file exists
ls -l /path/to/document.pdf

# Try with absolute path
xberg extract /absolute/path/to/document.pdf
```

### OCR Not Working

Verify Tesseract is installed:

```bash
tesseract --version

# If not found:
# macOS: brew install tesseract
# Ubuntu: sudo apt-get install tesseract-ocr
# Windows: Download from https://github.com/tesseract-ocr/tesseract
```

### Configuration File Not Found

Check that the configuration file has the correct format and location:

```bash
# Use explicit path
xberg extract document.pdf --config /absolute/path/to/config.toml

# Or place xberg.toml in current directory
ls -l xberg.toml
```

### Out of Memory with Large Files

Use chunking to reduce memory usage:

```bash
xberg extract large-document.pdf --chunk true --chunk-size 1000
```

### Cache Directory Permissions

Ensure write access to the cache directory:

```bash
# Check permissions
ls -ld .xberg

# Or use a custom directory with appropriate permissions
xberg extract document.pdf --config config.toml
# In config.toml: cache_dir = "/tmp/xberg-cache"
```

## Key Files

- `src/main.rs`: CLI implementation with command definitions and argument parsing
- `src/commands/overrides.rs`: Extraction override flags (OCR, chunking, layout, acceleration, etc.)
- `Cargo.toml`: Package metadata and dependencies

## Building

### Development Build

```bash
cargo build -p xberg-cli
```

### Release Build

```bash
cargo build --release -p xberg-cli
```

### With All Features

```bash
cargo build --release -p xberg-cli --features all
```

## Testing

```bash
# Run CLI tests
cargo test -p xberg-cli

# With logging
RUST_LOG=debug cargo test -p xberg-cli -- --nocapture
```

## Performance Characteristics

- **Single file extraction**: Typically 10-100ms depending on file size and format
- **Batch processing**: Near-linear scaling with 8 concurrent extractions by default
- **OCR processing**: 100-500ms per page depending on image quality and language
- **Caching**: Sub-millisecond retrieval for cached results

## References

- **Xberg Core**: `../xberg/`
- **Main Documentation**: <https://docs.xberg.io>
- **GitHub Repository**: <https://github.com/xberg-io/xberg>
- **Configuration Guide**: See example configuration sections above

## Contributing

We welcome contributions! Please see the main Xberg repository for contribution guidelines.

## License

MIT License (MIT)
