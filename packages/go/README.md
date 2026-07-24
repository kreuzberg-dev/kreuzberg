# Xberg

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

High-performance document intelligence for Go backed by the Rust core that powers every Xberg binding.

> **Version 1.0.0-rc.37**
> Report issues at [github.com/xberg-io/xberg](https://github.com/xberg-io/xberg/issues).

## What This Package Provides

- **Go module over the Rust core** — context-aware extraction with Go structs and errors.
- **Structured results** — text, tables, images, metadata, language detection, chunks, and warnings.
- **Static-link workflow** — build against `xberg-ffi` and ship a self-contained Go binary.
- **Cross-binding parity** — output matches the Python, Node.js, Ruby, Java, .NET, PHP, Elixir, Dart, Swift, Zig, WASM, and C FFI packages.

## Install

Xberg Go binaries are **statically linked** — once built, they are self-contained and require no runtime library dependencies. Only the static library is needed at build time.

### Quick Start (Monorepo Development)

For development in the Xberg monorepo:

```bash
# Build the static FFI library
cargo build -p xberg-ffi --release

# Go build will automatically link against the static library
# (from target/release/libxberg_ffi.a)
cd packages/go
go build -v

# Run your binary (no library path needed - it's statically linked)
./v4
```

That's it! The resulting binary is self-contained and has no runtime dependencies on Xberg libraries.

### Using Go Modules

To use this package via `go get`:

```bash
# Get the latest release
go get github.com/xberg-io/xberg/packages/go@latest

# Or a specific version
go get github.com/xberg-io/xberg/packages/go@v1.0.0-rc.37
```

> ⚠️ Do not run `go get github.com/xberg-io/xberg` — the repository root is not a Go module.
> Always target the `/packages/go` subdirectory as shown above; the bare-root path resolves
> against a stale Go module-proxy cache entry and fails.

You'll need to provide the static library at build time. See [Building with Static Libraries](#building-with-static-libraries) below.

### Building with Static Libraries

When building outside the Xberg monorepo, you need to provide the static library (`.a` file on Unix, `.lib` on Windows).

#### Option 1: Download Pre-built Static Library

Download the static library for your platform from [GitHub Releases](https://github.com/xberg-io/xberg/releases):

```bash
# Example: Linux x86_64
curl -LO https://github.com/xberg-io/xberg/releases/download/v1.0.0-rc.37/go-ffi-linux-x86_64.tar.gz
tar -xzf go-ffi-linux-x86_64.tar.gz

# Copy to a permanent location
mkdir -p ~/xberg/lib
cp xberg-ffi/lib/libxberg_ffi.a ~/xberg/lib/
```

Then build with `CGO_LDFLAGS`:

```bash
# Linux/macOS
CGO_LDFLAGS="-L$HOME/xberg/lib -lxberg_ffi" go build

# Windows (MSVC)
set CGO_LDFLAGS=-L%USERPROFILE%\xberg\lib -lxberg_ffi
go build
```

#### Option 2: Build Static Library Yourself

If pre-built libraries aren't available for your platform:

```bash
# Clone the repository
git clone https://github.com/xberg-io/xberg.git
cd xberg

# Build the static library
cargo build -p xberg-ffi --release

# The static library is now at: target/release/libxberg_ffi.a
# Copy it to a permanent location
mkdir -p ~/xberg/lib
cp target/release/libxberg_ffi.a ~/xberg/lib/

# Now you can build Go projects
cd ~/my-go-project
CGO_LDFLAGS="-L$HOME/xberg/lib -lxberg_ffi" go build
```

### System Requirements

#### ONNX Runtime (for embeddings)

If using embeddings functionality, ONNX Runtime must be installed **at build time**:

```bash
# macOS
brew install onnxruntime

# Ubuntu/Debian
sudo apt install libonnxruntime libonnxruntime-dev

# Windows (MSVC)
scoop install onnxruntime
# OR download from https://github.com/microsoft/onnxruntime/releases
```

The resulting binary will have ONNX Runtime statically linked or dynamically linked depending on how the FFI library was built. Check the build configuration.

**Note:** Windows MinGW builds do not support embeddings (ONNX Runtime requires MSVC). Use Windows MSVC for embeddings support.

## Quickstart

```go
package main

import (
	"fmt"
	"log"

	"github.com/xberg-io/xberg/packages/go"
)

func main() {
	input := xberg.ExtractInputFromURI("document.pdf")
	output, err := xberg.Extract(*input, xberg.ExtractionConfig{})
	if err != nil {
		log.Fatalf("extract failed: %v", err)
	}
	if len(output.Results) == 0 {
		log.Fatal("extract produced no results")
	}
	result := output.Results[0]

	fmt.Println("MIME:", result.MimeType)
	fmt.Println("First 200 chars:")
	fmt.Println(result.Content[:200])
}
```

Build and run:

```bash
# Build (make sure you have the static library available - see Install)
CGO_LDFLAGS="-L$HOME/xberg/lib -lxberg_ffi" go build

# Run - no library paths needed!
./myapp
```

The binary is self-contained and can be distributed without any Xberg library dependencies.

## Examples

### Extract bytes

```go
data, err := os.ReadFile("slides.pptx")
if err != nil {
	log.Fatal(err)
}
filename := "slides.pptx"
input := xberg.ExtractInputFromBytes(
	data,
	"application/vnd.openxmlformats-officedocument.presentationml.presentation",
	&filename,
)
output, err := xberg.Extract(*input, xberg.ExtractionConfig{})
if err != nil {
	log.Fatal(err)
}
result := output.Results[0]
fmt.Println("MIME:", result.MimeType)
```

### Use advanced configuration

```go
lang := "eng"
useCache := true
cfg := xberg.ExtractionConfig{
	UseCache: &useCache,
	ForceOcr: false,
	Images:   &xberg.ImageExtractionConfig{},
	Ocr: &xberg.OcrConfig{
		Backend: "tesseract",
		Language: []string{lang},
	},
}
input := xberg.ExtractInputFromURI("scanned.pdf")
output, err := xberg.Extract(*input, cfg)
result := output.Results[0]
```

### URL extraction

```go
input := xberg.ExtractInputFromURI("https://example.com/report.pdf")
output, err := xberg.Extract(*input, xberg.ExtractionConfig{})
if err != nil {
	log.Fatal(err)
}
fmt.Println("Results:", len(output.Results))
```

### Batch extract

```go
inputs := []xberg.ExtractInput{
	*xberg.ExtractInputFromURI("doc1.pdf"),
	*xberg.ExtractInputFromURI("https://example.com/report.pdf"),
}
output, err := xberg.ExtractBatch(inputs, xberg.ExtractionConfig{})
if err != nil {
	log.Fatal(err)
}
for i, res := range output.Results {
	fmt.Printf("[%d] %s => %d bytes\n", i, res.MimeType, len(res.Content))
}
```

### Register a validator

```go
//export customValidator
func customValidator(resultJSON *C.char) *C.char {
	// Validate JSON payload and return an error string (or NULL if ok)
	return nil
}

func init() {
	if err := xberg.RegisterValidator("go-validator", 50, (C.ValidatorCallback)(C.customValidator)); err != nil {
		log.Fatalf("validator registration failed: %v", err)
	}
}
```

## API Reference

- **GoDoc**: [pkg.go.dev/github.com/xberg-io/xberg/packages/go](<https://pkg.go.dev/github.com/xberg-io/xberg/packages/go>)
- **Full documentation**: [xberg.io](https://xberg.io) (configuration, formats, OCR backends)

## Troubleshooting

| Issue                                                                          | Fix                                                                                                                                                                                                                 |
| ------------------------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `ld returned 1 exit status` or `undefined reference to 'html_to_markdown_...'` | The static library wasn't found. Make sure `CGO_LDFLAGS` points to the directory containing `libxberg_ffi.a`: `CGO_LDFLAGS="-L/path/to/lib -lxberg_ffi" go build`                                           |
| `cannot find -lxberg_ffi`                                                  | The static library file is missing or in the wrong location. Download it from [GitHub Releases](https://github.com/xberg-io/xberg/releases) or build it yourself: `cargo build -p xberg-ffi --release` |
| `undefined: xberg.Extract`                                                  | Regenerate the binding or update to Xberg v1; extraction is exposed as `Extract(ExtractInput, ExtractionConfig)`.                                                                                           |
| `Missing dependency: tesseract`                                                | Install the OCR backend and ensure it is on `PATH`. Errors bubble up as typed Xberg errors.                                                                                                               |
| `undefined: C.customValidator` during build                                    | Export the callback with `//export` in a `*_cgo.go` file before using it in `Register*` helpers.                                                                                                                    |
| `Missing dependency: onnxruntime`                                              | Install ONNX Runtime at build time: `brew install onnxruntime` (macOS), `apt install libonnxruntime libonnxruntime-dev` (Linux), `scoop install onnxruntime` (Windows). Required for embeddings functionality.      |
| Embeddings not available on Windows MinGW                                      | Windows MinGW builds cannot link ONNX Runtime (MSVC-only). Use Windows MSVC build for embeddings support, or build without embeddings feature.                                                                      |

## Testing / Tooling

- `task go:lint` – runs `gofmt` and `golangci-lint` (`golangci-lint` pinned to v2.11.3).
- `task go:test` – executes `go test ./...` (after building the static FFI library).
- `task e2e:go:verify` – regenerates fixtures via the e2e generator and runs `go test ./...` inside `e2e/go`.

Need help? Join the [Discord](https://discord.gg/xt9WY3GnKR) or open an issue with logs, platform info, and the steps you tried.

## Part of Xberg.dev

- [crawlberg](https://github.com/xberg-io/crawlberg) — web crawling and scraping with HTML→Markdown and headless-Chrome fallback.
- [html-to-markdown](https://github.com/xberg-io/html-to-markdown) — fast, lossless HTML→Markdown engine.
- [liter-llm](https://github.com/xberg-io/liter-llm) — universal LLM API client with native bindings for 14 languages and 143 providers.
- [tree-sitter-language-pack](https://github.com/xberg-io/tree-sitter-language-pack) — tree-sitter grammars and code-intelligence primitives.
- [alef](https://github.com/xberg-io/alef) — the polyglot binding generator that produces this README and all per-language bindings.
- [Discord](https://discord.gg/xt9WY3GnKR) — community, roadmap, announcements.
