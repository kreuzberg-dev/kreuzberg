---
name: picking-a-format
description: Use when choosing an output format for extracted documents — text, markdown, djot, html, or JSON. Maps consumer (LLM, parser, archive) to the right `--format` / `--content-format` pair.
---

<!--
AI-RULEZ :: GENERATED FILE — DO NOT EDIT
Content-Hash: blake3:470e563273f21e16138f65bd8a6c9b7a7b0c4adb003af8747c2957f3c17df0c4
Source-Hash: blake3:5907a9cc29a5d72bbd3eaf5b820cac5133c8724895664c64fa8eafc2227716af
Schema-Version: v1
-->

# Picking a format

Xberg has two orthogonal format knobs. Get them right up front and the
downstream code stays simple.

| Knob                | What it controls                                  | Values                                 | Default          |
| ------------------- | ------------------------------------------------- | -------------------------------------- | ---------------- |
| `--format`          | How the CLI prints the result                     | `text`, `json`, `toon`                 | `text` (`extract`), `json` (`batch`) |
| `--content-format`  | How extracted content is rendered inside `result` | `plain`, `markdown`, `djot`, `html`, `json` | `plain`     |
| `--token-reduction` | Strip whitespace / boilerplate for LLM contexts   | `off`, `light`, `moderate`, `aggressive`, `maximum` | `off`  |

`--format json` returns an envelope wrapping the `ExtractedDocument` — the
document lives under `.result` for `extract` and under `.results[]` for
`batch`, with `content`, `metadata`, `tables`, and `images` as fields of
that nested document. `--format text` prints just `content`.
`--content-format` is what shows up inside that `content` field.

## Decision tree

```text
Who consumes the output?
├── LLM (Claude, GPT, Gemini, local) — embed/prompt context
│       --format text --content-format markdown
├── Vector store / RAG indexer
│       --format json --content-format markdown
│       (markdown preserves structure for chunking)
├── Downstream parser that expects machine-readable JSON
│       --format json --content-format plain
│       (cleanest text + structured metadata)
├── Human review / archival
│       --format text --content-format markdown
├── HTML re-rendering / web display
│       --format json --content-format html
├── Lossless intermediate for pandoc / academic tooling
│       --format json --content-format djot
└── Token-budget-constrained pipeline
        --format text --content-format plain
        (drops markup; add --token-reduction moderate for further savings)
```

## Examples

Feed a PDF directly into an LLM:

```bash
xberg extract paper.pdf --content-format markdown
```

Index a corpus into a RAG store with tables and headings preserved:

```bash
xberg batch docs/*.pdf --format json --content-format markdown \
  | jq -c '.results[] | {content: .content, tables: .tables}'
```

Strip a file to bare text for a token-tight summarizer:

```bash
xberg extract long.pdf \
  --content-format plain \
  --token-reduction moderate
```

Pull metadata only, ignore content:

```bash
xberg extract file.pdf --format json | jq '.result.metadata'
```

## When in doubt

- **Default to `markdown`** as the content format. It is the best
  compromise across LLMs, RAG, and human review, and Xberg has the
  most faithful renderer for it.
- Reach for `plain` only when downstream cannot tolerate any markup.
- Reach for `djot` only if you're already in a djot/pandoc pipeline.
- Reach for `html` only when re-rendering for the web.

## Token-reduction (orthogonal)

`--token-reduction` collapses whitespace, strips repeated headers/footers,
and trims boilerplate. It composes with any `--content-format`:

- `off` (default), `light`, `moderate`, `aggressive`, `maximum`.

Use `moderate` as a safe starting point for LLM context windows. `maximum`
is lossy — verify before relying on it.

See `references/cli-reference.md` for the full flag set and
`references/configuration.md` for the equivalent `output_format` and
`token_reduction` keys in `xberg.toml`.
