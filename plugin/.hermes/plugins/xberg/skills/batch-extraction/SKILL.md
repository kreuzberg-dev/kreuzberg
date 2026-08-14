---
name: batch-extraction
description: Use when extracting from many files at once with shared config, bounded parallelism, per-file overrides, and error recovery. Covers the `batch` command, `--file-configs`, `--max-concurrent`, and output layout.
---

<!--
AI-RULEZ :: GENERATED FILE — DO NOT EDIT
Content-Hash: blake3:89aa763a66dc25e9aa2849d630b288e27b1b8e6aaebf70e4ee4b58f2e3670e73
Source-Hash: blake3:ac8bab9ae4a76e61f437af29b4a45a38326056a50769a177038927c69353edaf
Schema-Version: v1
-->

# Batch extraction

Use this when processing a directory or glob of documents in one pass.
`xberg batch` shares one extraction config across every file, runs
extractions concurrently, and returns one structured array — failures on
individual files do not abort the run.

## Basic usage

```bash
# Glob expands to many paths; results come back as a JSON array (default)
xberg batch *.pdf

# Mixed formats, markdown content for LLM ingestion
xberg batch docs/*.docx --content-format markdown

# Recurse with the shell, then extract
xberg batch $(find ./corpus -name '*.pdf')
```

`batch` defaults to `--format json` (vs `--format text` for single
`extract`). Each array entry is a full extraction result, so downstream
code can index by position into the input path list.

```bash
xberg batch reports/*.pdf \
  | jq '.[] | {chars: (.content | length), mime: .mime_type}'
```

## Parallelism

`--max-concurrent` caps how many files extract at once (default: the CPU
count, capped at 8). Lower it on memory-constrained hosts or when OCR/ML
models are active, since each in-flight extraction holds its own buffers.
Layout-heavy batches are further limited (1 concurrent extraction for
all-PDF-layout batches, 2 for mixed layout):

```bash
# Cap at 4 concurrent extractions
xberg batch scans/*.pdf --ocr true --max-concurrent 4
```

`--max-threads` additionally caps *total* internal threads (Rayon, ONNX
intra-op, the batch semaphore) for tightly constrained environments:

```bash
xberg batch *.pdf --max-concurrent 2 --max-threads 4
```

## Per-file config overrides

A single shared config does not always fit. `--file-configs` points at a
JSON file mapping each path to its own override object, merged on top of
the shared config for that file only:

```json
{
  "scan.pdf": { "force_ocr": true },
  "report.pdf": { "output_format": "markdown" },
  "data.xlsx": { "output_format": "json" }
}
```

```bash
xberg batch scan.pdf report.pdf data.xlsx --file-configs overrides.json
```

Keys are file paths (matching the paths passed on the command line);
values are per-file extraction config objects in snake_case, the same
shape as a config file.

## Output layout

For text/toon output with image extraction, `--output-dir` controls where
referenced image files (e.g. `image_0.png`) are written; the directory
must already exist. JSON output embeds image bytes inline and ignores
`--output-dir`.

```bash
mkdir -p out/images
xberg batch slides/*.pptx --extract-images true --output-dir out/images --format text
```

## Error recovery

Batch extraction is fault-tolerant per file: one unreadable or corrupt
document does not stop the rest. Inspect results for partial content and
surfaced errors rather than relying on the process exit code alone. Pair
with `--max-concurrent` to avoid exhausting memory when a few large files
sit in a big batch.

## Shared config

Every `extract` flag also applies to `batch` (OCR, chunking, layout,
content format, etc.) and is shared across all files unless a
`--file-configs` entry overrides it:

```bash
xberg batch invoices/*.pdf \
  --layout --layout-table-model slanet_wireless \
  --content-format markdown --max-concurrent 8
```

A config file works too and auto-discovers from the cwd upward:

```toml
output_format = "markdown"

[ocr]
backend = "tesseract"
language = "eng"
```

```bash
xberg batch corpus/*.pdf --config xberg.toml
```

## Programmatic access

From Python, `extract_batch` takes a list of `ExtractInput`s and returns one
envelope whose `results` array holds a document per input:

```python
from xberg import ExtractInput, extract_batch, ExtractionConfig

config = ExtractionConfig(output_format="markdown")

inputs = [ExtractInput(uri=p) for p in ["a.pdf", "b.docx", "c.xlsx"]]
output = await extract_batch(inputs, config)

for doc in output.results:
    print(len(doc.content))
```

Per-input overrides go on `ExtractInput.config` (a `FileExtractionConfig`).
Node.js mirrors this with `extractBatch`; Rust uses `extract_batch(inputs, &config)`.
See `references/python-api.md`, `references/nodejs-api.md`, and
`references/rust-api.md` in the sibling `xberg` skill.

## MCP

When the `xberg` MCP server is registered, prefer the
`extract_batch` tool over shelling out — it takes an array of input objects
and a config object and returns structured results directly.

## Common pitfalls

- **Default format differs** — `batch` defaults to `--format json`,
  `extract` to `--format text`. Set `--format` explicitly if a script
  depends on one shape.
- **`--output-dir` must exist** — the CLI does not create it.
- **Memory blowups** — large batches with OCR/layout active need a lower
  `--max-concurrent`; the default is the CPU count, capped at 8.
- **`--file-configs` path keys** — must match the paths as passed on the
  command line, not absolute-resolved variants.

See `references/cli-reference.md` for the full `batch` flag set.
