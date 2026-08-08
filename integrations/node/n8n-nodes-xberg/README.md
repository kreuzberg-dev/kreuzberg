<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="https://cdn.jsdelivr.net/gh/xberg-io/assets@v1/banner/readme-banner-dark.svg">
    <img alt="Xberg" width="420" src="https://cdn.jsdelivr.net/gh/xberg-io/assets@v1/banner/readme-banner-light.svg">
  </picture>
</p>

# n8n-nodes-xberg

An [n8n](https://n8n.io) community node that runs [Xberg](https://github.com/xberg-io/xberg)
document extraction inside your workflows. Point it at a binary file on an incoming item and it
returns the extracted text, tables, and metadata — with optional OCR for scans and images.

Extraction runs locally in-process through the `@xberg-io/xberg` native binding. No API key, no
cloud call, no data leaves your n8n instance.

## Requirements

- Self-hosted n8n (native N-API addons are not available on n8n Cloud, so this node is
  self-hosted only).
- Node.js 20.15+ on Linux (x64/arm64, glibc or musl), macOS (arm64), or Windows (x64/arm64) — the
  platforms for which `@xberg-io/xberg` ships prebuilt binaries.

## Installation

In your n8n instance go to **Settings > Community Nodes > Install**, enter the package name, and
confirm you understand the risks of installing community code:

```text
@xberg-io/n8n-nodes-xberg
```

Or install manually into a self-hosted instance:

```bash
npm install @xberg-io/n8n-nodes-xberg
```

## Operations

The node exposes one resource, **Document**, with three operations.

### Document › Extract

Extract content from one document per incoming item. Set **Input Source** to read the document from
a binary property (uploaded file) or from a URL/local path.

### Document › Extract Batch

Extract every incoming item in a single `extractBatch` call. The native binding schedules the inputs
concurrently, so this is substantially faster than a loop of Extract nodes. Use it for file/binary
inputs; for crawling a URL use Extract or Map URL instead. Output items are paired back to their
input; per-item failures surface through **Continue On Fail**.

### Document › Map URL

List the URLs reachable from a web page or sitemap without extracting them. Set **Mode** to `auto`,
`document`, or `crawl`, and optionally cap results with **Max Total URLs**. Each output item's JSON
holds the seed `url` and the discovered `urls` array.

### Extract parameters

| Parameter | Description |
| --- | --- |
| Input Source | `Binary Data` (a binary property on the item) or `URL or Path`. |
| Input Binary Field | Binary property holding the document (default `data`). Binary source only. |
| URL or Path | HTTP(S) URL or local filesystem path. URL source only. |
| Output Format | `markdown`, `plain`, `html`, `djot`, `json`, or `structured`. |
| Enable OCR | Run OCR on images and scanned PDF pages (default on). |
| Force OCR | OCR every page even when a text layer exists. |
| OCR Languages | Comma-separated ISO 639-2 codes, e.g. `eng,deu`. |
| Enable Chunking | Split the content into overlapping chunks for RAG pipelines. |
| Chunk Size / Chunk Overlap | Max characters per chunk and overlap between chunks. |
| Extract Images | Extract embedded images and report them in the output. |
| Enable Quality Processing | Clean up extracted text with post-processing. |
| Include Metadata | Attach document metadata (title, author, dates). |
| Include Tables | Attach structured table data. |
| Include Chunks | Attach the generated chunks (requires Enable Chunking). |
| Output Content Field | JSON field to write the extracted text into (default `text`). |
| Return As Binary | Also attach the extracted content as a binary property. |

### Output

Each Extract / Extract Batch output item's JSON contains the extracted content under the configured
field (default `text`), plus `mimeType`, `extractionMethod`, `detectedLanguages`, and `counts`
(pages, tables, images). The content is already rendered in the configured **Output Format**
(Markdown, HTML, …). `qualityScore`, `entities`, and `summary` appear when the binding populates
them. `metadata`, `tables`, and `chunks` are included when their toggles are
set. When **Return As Binary** is set, the content is also attached as a binary property with a
format-appropriate MIME type and extension.

## Supported formats

Xberg extracts from 101 formats including PDF, DOCX, PPTX, XLSX, HTML, EPUB, images, and more. See
the [Xberg documentation](https://docs.xberg.io) for the full list.

## Compatibility

Tested against n8n's `n8n-workflow` 2.x node API (`n8nNodesApiVersion` 1).

## Part of Xberg.io

- [Xberg](https://github.com/xberg-io/xberg) — document intelligence: text, tables, metadata from 101 formats with optional OCR.
- [Xberg Enterprise](https://github.com/xberg-io/xberg-enterprise) — managed extraction API with SDKs, dashboards, and observability.

## License

[MIT](./LICENSE)
