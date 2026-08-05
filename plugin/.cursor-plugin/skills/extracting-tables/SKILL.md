---
name: extracting-tables
description: Use when extracting tabular data from PDFs, spreadsheets, or images. Covers layout-aware table detection, table model selection, output formats (markdown / JSON cells), and known limits.
---

<!--
AI-RULEZ :: GENERATED FILE — DO NOT EDIT
Content-Hash: blake3:7667a52a8674605a45cc61b67e7879a0104d5e86c0d82b4bde5ced9e6e3463a8
Source-Hash: blake3:c3730974b95d68f86536f7c420114fdefdaf54253473d6bec3cfefe6dacc7d4d
Schema-Version: v1
-->

# Extracting tables

Use this when the user wants structured tabular data — financial
statements, scientific tables, invoices, spreadsheet-style PDFs. Xberg
detects tables via a layout model (RT-DETR v2) and reconstructs cell
structure with a configurable table model.

## Basic usage

```bash
# Markdown tables embedded in the content stream
xberg extract report.pdf --layout --content-format markdown

# Structured JSON output, tables appear under result.tables
xberg extract report.pdf --layout --format json
```

`--layout` turns on layout-aware extraction; without it, tables fall back
to plain text reflow and you lose cell boundaries.

## Output shapes

Two surfaces, picked via `--format` (CLI shape) and `--content-format`
(content rendering):

- **Markdown tables in `content`** — `--content-format markdown`. Tables
  appear inline as `| col | col |` blocks. Good for LLM ingestion.
- **Structured `tables` array** — `--format json`. Each entry has
  `cells[][]` (rows × cols), `markdown` (pre-rendered), `page_number`,
  `bounding_box`. Use this when downstream code needs exact cell access.
  (`bounding_box` is omitted when no position data is available.)

Both are populated at once when `--layout` is on. The `tables` array is
always structured; the `content` stream switches representation.

```bash
xberg extract financials.pdf --layout --format json \
  | jq '.result.tables[] | {page: .page_number, rows: (.cells | length)}'
```

## Table models

`--layout-table-model` picks the reconstruction backend:

| Model              | Best for                                              | Notes                                       |
| ------------------ | ----------------------------------------------------- | ------------------------------------------- |
| `tatr`             | dense complex tables (academic, financial)            | **Default.** Heaviest, highest accuracy.    |
| `slanet_auto`      | dispatches per-table to wired/wireless                | Good when table styles are mixed.           |
| `slanet_wired`     | tables with visible borders                           | Faster than tatr.                           |
| `slanet_wireless`  | tables without borders (whitespace-separated)         | For invoices, simple grids.                 |
| `slanet_plus`      | hybrid wired / wireless                               | Lighter than `slanet_auto`.                 |
| `disabled`         | layout detection only, no table structure             | Use to skip table model cost.               |

```bash
xberg extract bank-statement.pdf \
  --layout --layout-table-model tatr --content-format markdown
```

Drop `--layout-confidence` when the layout model misses tables (default
threshold ~0.5):

```bash
xberg extract noisy-scan.pdf --layout --layout-confidence 0.3
```

## Spreadsheets

`.xlsx`, `.ods`, `.csv`, `.tsv` are extracted by dedicated parsers — no
layout model needed. Each sheet becomes a markdown table (or structured
table) automatically:

```bash
xberg extract workbook.xlsx --content-format markdown
xberg extract data.csv --format json
```

Pass `--no-cache=true` only when iterating on the same file with different
configs.

## Config file alternative

```toml
# `output_format` in config files equals `--content-format` on the CLI.
output_format = "markdown"

[layout]
confidence_threshold = 0.5
table_model = "tatr"
```

Then:

```bash
xberg extract report.pdf --format json
```

## Programmatic access

From Python, structured tables live on the document in the result envelope
(`result.results[0].tables`):

```python
from xberg import ExtractInput, extract, ExtractionConfig, LayoutDetectionConfig

config = ExtractionConfig(
    layout=LayoutDetectionConfig(table_model="tatr"),
    output_format="markdown",
)
result = await extract(ExtractInput(uri="report.pdf"), config)
for table in result.results[0].tables:
    print(table.markdown)        # rendered markdown
    print(table.cells[0][0])     # cell access
```

Node.js mirrors this (`extract`, `output.results[0].tables`, camelCase fields).
See `references/python-api.md` and `references/nodejs-api.md` in the
sibling `xberg` skill for full type signatures.

## Known limitations

- **Merged cells** — reconstructed as repeated values across the spanned
  region; the merge is not preserved as metadata.
- **Rotated tables** — enable `--ocr-auto-rotate true` for image-based
  PDFs before extraction.
- **Nested tables** — flattened. Detection succeeds; structural nesting is
  lost.
- **Multi-page tables** — each page yields a separate `tables[]` entry.
  Stitch by matching column headers if needed.
- **ONNX Runtime required** — layout and table models are unavailable in
  WASM builds and on the Android x86_64 emulator; native targets ship
  full support.

## Common failure modes

- **Empty `tables` with `--layout` on** — confidence threshold too high or
  table model mismatched. Drop `--layout-confidence` to 0.3, try
  `--layout-table-model tatr`.
- **Markdown tables look ragged** — switch `--layout-table-model` to
  `slanet_wired` for bordered grids or `slanet_wireless` for invoices.
- **Slow extraction** — `tatr` is heavy. Use `slanet_auto` or
  `slanet_plus` as a default; reach for `tatr` only when accuracy matters.

See `references/cli-reference.md` for the full layout flag set and
`references/advanced-features.md` for the layout pipeline internals.
