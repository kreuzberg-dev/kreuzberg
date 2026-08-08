<!--
AI-RULEZ :: GENERATED FILE — DO NOT EDIT
Content-Hash: blake3:fc2412f569929a95cf1b988c5c33aa4d064b52712f8d606de11eb58b3f5d86ff
Source-Hash: blake3:5907a9cc29a5d72bbd3eaf5b820cac5133c8724895664c64fa8eafc2227716af
Schema-Version: v1
-->

# Xberg CLI Reference

Comprehensive command-line interface for the Xberg document intelligence library.

## Installation

```bash
brew install xberg-io/tap/xberg
# or run without a persistent install (self-installs the binary):
npx @xberg-io/xberg-cli --help
uvx --from xberg-cli xberg --help
```

Or download a pre-built binary from the [latest GitHub Release](https://github.com/xberg-io/xberg/releases/latest), or build from source:

```bash
cargo install xberg-cli
```

## Commands

### extract

Extract text and structure from a single document.

```bash
xberg extract <path> [FLAGS]
```

## Positional Arguments

- `<path>` — Path to the document file

## Flags

- `-c, --config <path>` — Path to config file (TOML, YAML, or JSON). Auto-discovers `xberg.{toml,yaml,json}` in current and parent directories if omitted.
- `--config-json <json>` — Inline JSON configuration (merged after config file, before CLI flags).
- `--config-json-base64 <base64>` — Base64-encoded JSON configuration.
- `-m, --mime-type <type>` — MIME type hint (auto-detected if not provided).
- `-f, --format <text|json|toon>` — CLI output format (default: `text`). Controls how results display, not extraction content format.
- `--content-format <plain|markdown|djot|html|json>` — Extraction content format (default: `plain`). Controls format of extracted content. (Note: `--output-format` is a hidden deprecated alias.)
- `--ocr <bool>` — Enable OCR processing.
- `--ocr-backend <BACKEND>` — OCR backend: `tesseract`, `paddle-ocr`, `vlm`, `candle-trocr`, `candle-paddleocr-vl`, `candle-glm-ocr`, `candle-deepseek-ocr`.
- `--ocr-language <LANG>` — OCR language code.
- `--ocr-auto-rotate <bool>` — Auto-rotate images before OCR.
- `--force-ocr <bool>` — Force OCR even if text extraction succeeds.
- `--disable-ocr <bool>` — Disable OCR entirely (even for images).
- `--no-cache <bool>` — Disable caching.
- `--chunk <bool>` — Enable text chunking.
- `--chunk-size <n>` — Chunk size in characters.
- `--chunk-overlap <n>` — Chunk overlap in characters.
- `--chunking-tokenizer <model>` — Tokenizer model for token-based sizing.
- `--include-structure <bool>` — Include hierarchical document structure.
- `--quality <bool>` — Enable quality processing.
- `--detect-language <bool>` — Enable language detection.
- `--layout` — Enable layout detection (RT-DETR v2). Use `--layout false` to disable.
- `--layout-confidence <float>` — Layout confidence threshold (0.0-1.0).
- `--layout-table-model <model>` — Table structure model: `tatr`, `slanet_wired`, `slanet_wireless`, `slanet_plus`, `slanet_auto`, `disabled`.
- `--acceleration <provider>` — ONNX execution provider: `auto`, `cpu`, `coreml`, `cuda`, `tensorrt`.
- `--extract-pages <bool>` — Extract pages as separate array.
- `--page-markers <bool>` — Insert page marker comments.
- `--extract-images <bool>` — Enable image extraction.
- `--target-dpi <n>` — Target DPI for images (36-2400).
- `--pdf-password <pass>` — Password for encrypted PDFs (repeatable).
- `--pdf-extract-images <bool>` — Extract images from PDF pages.
- `--pdf-extract-metadata <bool>` — Extract PDF metadata.
- `--token-reduction <level>` — Token reduction: `off`, `light`, `moderate`, `aggressive`, `maximum`.
- `--msg-codepage <n>` — Windows codepage fallback for MSG files.
- `--max-concurrent <n>` — Max parallel extractions in batch mode.
- `--max-threads <n>` — Cap all internal thread pools.
- `--cache-namespace <name>` — Cache namespace for tenant isolation.
- `--cache-ttl-secs <n>` — Per-request cache TTL in seconds.

## Examples

```bash
# Extract with default settings
xberg extract document.pdf

# Extract with OCR enabled
xberg extract scanned.pdf --ocr=true

# Extract with specific content format
xberg extract doc.docx --content-format markdown

# Extract with inline JSON config
xberg extract file.pdf --config-json '{"ocr":{"backend":"tesseract"}}'

# Extract with base64-encoded config
xberg extract file.pdf --config-json-base64 eyJvY3IiOnsiYmFja2VuZCI6InRlc3NlcmFjdCJ9fQ==

# Extract and output as JSON
xberg extract doc.pdf --format json

# Extract with chunking
xberg extract large-doc.pdf --chunk true --chunk-size 2000 --chunk-overlap 200

# Layout-aware markdown extraction
xberg extract document.pdf --layout --content-format markdown

# With custom confidence threshold
xberg extract document.pdf --layout-confidence 0.7 --content-format markdown
```

### batch

Batch extract from multiple documents in parallel.

```bash
xberg batch <paths...> [FLAGS]
```

## Positional Arguments

- `<paths...>` — One or more document file paths

## Flags

- `-c, --config <path>` — Path to config file (TOML, YAML, or JSON). Auto-discovers `xberg.{toml,yaml,json}` in current and parent directories if omitted.
- `--config-json <json>` — Inline JSON configuration (merged after config file, before CLI flags).
- `--config-json-base64 <base64>` — Base64-encoded JSON configuration.
- `-f, --format <text|json|toon>` — CLI output format (default: `json`). Controls how results display, not extraction content format.
- All extraction override flags from `extract` are also supported (e.g., `--content-format`, `--ocr`, `--layout`, `--force-ocr`, `--no-cache`, `--quality`, `--acceleration`, etc.). See the `extract` command flags for the full list.

## Notes

- Batch command defaults to JSON output format (unlike `extract` which defaults to text).
- Does not support the `--mime-type` flag (it is specific to the `extract` command).

## Examples

```bash
# Batch extract multiple PDFs
xberg batch document1.pdf document2.pdf document3.pdf

# Batch extract with glob patterns (shell expansion)
xberg batch *.pdf

# Batch extract with custom output format
xberg batch doc1.pdf doc2.pdf --content-format markdown

# Batch extract with OCR
xberg batch scanned*.pdf --ocr=true

# Batch extract with text output format
xberg batch files*.docx --format text
```

### detect

Identify MIME type of a file.

```bash
xberg detect <path> [FLAGS]
```

## Positional Arguments

- `<path>` — Path to the file

## Flags

- `-f, --format <text|json|toon>` — Output format (default: `text`)

## Examples

```bash
# Detect MIME type (text output)
xberg detect unknown-file.bin

# Detect MIME type (JSON output)
xberg detect file.xyz --format json
```

### version

Display version information.

```bash
xberg version [FLAGS]
```

## Flags

- `-f, --format <text|json|toon>` — Output format (default: `text`)

## Examples

```bash
# Show version as text
xberg version

# Show version as JSON
xberg version --format json
```

### cache

Manage extraction cache.

#### cache stats

Display cache statistics.

```bash
xberg cache stats [FLAGS]
```

## Flags

- `--cache-dir <path>` — Cache directory (default: `.xberg` in current directory)
- `-f, --format <text|json|toon>` — Output format (default: `text`)

## Examples

```bash
# Show cache stats
xberg cache stats

# Show cache stats as JSON
xberg cache stats --format json

# Show stats for specific cache directory
xberg cache stats --cache-dir /tmp/my-cache
```

### cache clear

Clear all cached extractions.

```bash
xberg cache clear [FLAGS]
```

## Flags

- `--cache-dir <path>` — Cache directory (default: `.xberg` in current directory)
- `-f, --format <text|json|toon>` — Output format (default: `text`)

## Examples

```bash
# Clear cache
xberg cache clear

# Clear specific cache directory
xberg cache clear --cache-dir /tmp/my-cache
```

### serve

Start the API server (requires `api` feature).

```bash
xberg serve [FLAGS]
```

## Flags

- `-H, --host <host>` — Host to bind to (e.g., `127.0.0.1` or `0.0.0.0`). CLI arg overrides config file and environment variables.
- `-p, --port <port>` — Port to bind to. CLI arg overrides config file and environment variables.
- `-c, --config <path>` — Path to config file (TOML, YAML, or JSON). Auto-discovers `xberg.{toml,yaml,json}` in current and parent directories if omitted.

## Configuration Precedence

1. CLI arguments (`--host`, `--port`)
2. Environment variables (`XBERG_HOST`, `XBERG_PORT`)
3. Config file (`[server]` section)
4. Built-in defaults (`127.0.0.1:8000`)

## Examples

```bash
# Start server with defaults
xberg serve

# Start server on specific host and port
xberg serve --host 0.0.0.0 --port 3000

# Start server with config file
xberg serve --config xberg.toml

# Start server (environment variables override defaults)
XBERG_HOST=192.168.1.100 XBERG_PORT=8080 xberg serve
```

### mcp

Start the Model Context Protocol (MCP) server (requires `mcp` feature).

```bash
xberg mcp [FLAGS]
```

## Flags

- `-c, --config <path>` — Path to config file (TOML, YAML, or JSON). Auto-discovers `xberg.{toml,yaml,json}` in current and parent directories if omitted.
- `--transport <stdio|http>` — Transport mode (default: `stdio`)
- `--host <host>` — HTTP host for http transport (default: `127.0.0.1`)
- `--port <port>` — HTTP port for http transport (default: `8001`)

## Examples

```bash
# Start MCP server with stdio transport
xberg mcp

# Start MCP server with HTTP transport
xberg mcp --transport http

# Start MCP server on custom HTTP host/port
xberg mcp --transport http --host 0.0.0.0 --port 9000

# Start MCP server with config file
xberg mcp --config xberg.toml
```

## Configuration

### File Format

Configuration files support three formats with automatic detection:

- **TOML** — `.toml` extension (recommended)
- **YAML** — `.yaml` or `.yml` extension
- **JSON** — `.json` extension

### Configuration Precedence

Settings are applied in order from highest to lowest priority:

1. **Individual CLI flags** (e.g., `--ocr=true`, `--content-format markdown`)
2. **Inline JSON config** (`--config-json` or `--config-json-base64`)
3. **Config file** (explicit `--config path.toml` or auto-discovered)
4. **Default values** (built-in library defaults)

### Auto-Discovery

When no config file is specified, Xberg searches for configuration in this order:

1. `xberg.toml` in current directory
2. `xberg.yaml` in current directory
3. `xberg.json` in current directory
4. Parent directories (same search pattern, up to filesystem root)

### Example Configuration

```toml
# Top-level extraction options
use_cache = true
enable_quality_processing = true
force_ocr = false
output_format = "markdown"

# OCR settings
[ocr]
backend = "tesseract"
language = "eng"

# Chunking settings
[chunking]
max_characters = 2000
overlap = 200

# Language detection
[language_detection]
enabled = true

# Server configuration (for serve command)
[server]
host = "127.0.0.1"
port = 8000
```

## Exit Codes

- `0` — Success
- Non-zero — Error (see stderr for details)

## Error Handling

The CLI validates input and provides clear error messages:

- **File not found** — Verify path exists and is readable
- **Invalid MIME type** — Ensure file is accessible and format is supported
- **Invalid JSON** — Check `--config-json` syntax
- **Invalid config file** — Verify TOML/YAML/JSON format
- **Invalid chunk parameters** — Ensure chunk-size > 0 and overlap < chunk-size

## Environment Variables

- `RUST_LOG` — Set logging level (e.g., `RUST_LOG=debug`)
- `XBERG_HOST` — Server bind host (used by `serve` command)
- `XBERG_PORT` — Server bind port (used by `serve` command)

## Common Patterns

### Extract with Custom Configuration

```bash
xberg extract document.pdf \
  --content-format markdown \
  --ocr=true \
  --quality true
```

### Batch Process with Config File

```bash
xberg batch *.pdf --config extraction-config.toml
```

### CI/CD Integration

```bash
# Extract to JSON for downstream processing
xberg extract file.pdf --format json | jq '.content'

# Batch process with error handling
xberg batch docs/*.pdf --format json || exit 1
```

### Performance Tuning

```bash
# Disable cache for temporary processing
xberg extract file.pdf --no-cache=true

# Enable chunking for large documents
xberg extract large-file.pdf \
  --chunk true \
  --chunk-size 5000 \
  --chunk-overlap 500
```

## Debugging

Enable detailed logging:

```bash
RUST_LOG=debug xberg extract document.pdf
```

Check cache statistics:

```bash
xberg cache stats --format json
```

Detect file MIME type:

```bash
xberg detect unknown-file --format json
```
