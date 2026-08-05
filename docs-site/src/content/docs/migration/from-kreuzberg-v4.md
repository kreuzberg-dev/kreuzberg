---
title: Migrating from Kreuzberg v4
description: How Xberg relates to Kreuzberg, what changed in v5, and how to stay on the v4 LTS line.
---

Xberg is the direct continuation of **Kreuzberg**. The Rust core and extraction API are the same
lineage — v5 is the Xberg-branded release line. The Kreuzberg **v4** line continues as a
long-term-support release.

## Backwards compatibility

- **Existing v4 installs keep working.** Kreuzberg v4 packages remain published under their original
  names and are supported as LTS (see below). Nothing you have deployed on v4 stops functioning.
- **Go module pins keep resolving.** Old imports of `github.com/kreuzberg-dev/kreuzberg` continue to
  resolve from the Go module proxy cache. New v4 releases publish at
  `github.com/kreuzberg-dev/kreuzberg-lts/v4`; v5 is `github.com/xberg-io/xberg`.
- **The v4 LTS line is MIT-licensed** (earlier v4 shipped under Elastic License 2.0).

## The v4 LTS line

Kreuzberg v4 is maintained at **[kreuzberg-dev/kreuzberg-lts](https://github.com/kreuzberg-dev/kreuzberg-lts)**
with docs at **[kreuzberg.dev](https://kreuzberg.dev)**. It receives critical bug and security fixes
**until the end of 2026, on a best-effort basis**. No new features land on v4 — feature work happens
in Xberg.

**Stay on v4 LTS if** you depend on the **R binding** (removed in v5 — v4 is the last line to ship it)
or you are not ready to migrate.

## What changed in v5 (Xberg)

### Package names

Package identifiers moved from `kreuzberg` to `xberg`:

| Ecosystem | v4 (Kreuzberg) | v5 (Xberg) |
|-----------|----------------|------------|
| Rust (crates.io) | `kreuzberg` | `xberg` |
| Python (PyPI) | `kreuzberg` | `xberg` |
| npm | `@kreuzberg/node`, `@kreuzberg/wasm` | `@xberg-io/xberg`, `@xberg-io/xberg-wasm` |
| Maven | `dev.kreuzberg:kreuzberg` | `io.xberg:xberg` |
| NuGet | `Kreuzberg` | `XbergIo.Xberg` |
| Packagist | `kreuzberg/kreuzberg` | `xberg-io/xberg` |
| RubyGems | `kreuzberg` | `xberg` |
| Hex (Elixir) | `kreuzberg` | `xberg` |
| pub.dev (Dart) | — | `xberg` |
| Go module | `github.com/kreuzberg-dev/kreuzberg` | `github.com/xberg-io/xberg/packages/go` |
| R binding | supported | **removed** (use v4 LTS) |

The standalone `@kreuzberg/core` npm package is gone — the Node package (`@xberg-io/xberg`) is
self-contained.

### API identifiers

The extraction API is compatible in shape — the entry points (`extract_file`/`extract_bytes` and
their batch and sync variants), config, and result types keep the same structure. The identifiers
that changed:

- **Rust error type:** `KreuzbergError` → `XbergError` (and `Result<T>` now aliases
  `Result<T, XbergError>`).
- **Config file discovery:** `kreuzberg.{toml,yaml,json}` → `xberg.{toml,yaml,yml,json}` (the `.yml`
  extension is now also recognized).

Update the import/package name and consult the [API reference](/reference/api-python/) for your
language.

### Environment variables

All environment variables are re-prefixed `KREUZBERG_*` → `XBERG_*` — for example
`KREUZBERG_CACHE_DIR` → `XBERG_CACHE_DIR`, `KREUZBERG_OCR_BACKEND` → `XBERG_OCR_BACKEND`,
`KREUZBERG_OUTPUT_FORMAT` → `XBERG_OUTPUT_FORMAT`. Rename any `KREUZBERG_`-prefixed variables in your
environment or deployment config.

- **Removed:** `KREUZBERG_PDFIUM_BUNDLED_PATH` — the bundled PDFium fork was dropped.
- **New:** xberg adds `XBERG_`-prefixed variables for layout tuning, OCR model tier/version, CoreML,
  and the ONNX Runtime execution provider. See the configuration reference for the full list.

### Models and cache

Downloaded models and the extraction cache move from the `kreuzberg` path segment to `xberg`
(e.g. `~/.cache/kreuzberg` → `~/.cache/xberg`), and model repositories are pulled from the
**`xberg-io`** Hugging Face org (was `kreuzberg-dev`). Existing caches are re-downloaded on first run.

### Removed OCR backend

The Python/torch **EasyOCR** backend from Kreuzberg is removed. Xberg 1.1 adds **Sceptre**, a Rust
implementation of the same CRAFT and Gen2 CRNN architecture, not a drop-in EasyOCR API replacement. You can also use **Tesseract**,
**PaddleOCR**, the pure-Rust **Candle** backend, or a **VLM** backend.

See the [installation guide](/getting-started/installation/) for the current package names.
