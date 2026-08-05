---
name: chunking
description: Use when splitting extracted text into chunks for LLM context windows or RAG ingestion. Covers chunk size, overlap, markdown/yaml/semantic chunkers, tokenizer-based sizing, and the standalone `chunk` command.
---

<!--
AI-RULEZ :: GENERATED FILE — DO NOT EDIT
Content-Hash: blake3:0b5cd4bec9d2a8f3452e07139ef56d6b87f9f7990714d15483a03435e751af59
Source-Hash: blake3:c3730974b95d68f86536f7c420114fdefdaf54253473d6bec3cfefe6dacc7d4d
Schema-Version: v1
-->

# Chunking

Use this when feeding documents into an LLM context window or a vector
store. Xberg chunks two ways: inline during extraction (chunks land on
each document's `chunks` field), or standalone via the `chunk` command for text you
already have. Sizing is character-based by default, or token-based when a
tokenizer model is supplied.

## Inline during extraction

Turn on chunking with `--chunk` and the chunks appear on the structured
result under `chunks`:

```bash
# 1000-char chunks, 200-char overlap (defaults when --chunk is on)
xberg extract report.pdf --chunk --format json | jq '.chunks | length'

# Explicit size + overlap
xberg extract report.pdf --chunk --chunk-size 1500 --chunk-overlap 300 --format json
```

Overlap must be smaller than chunk size — the CLI rejects
`--chunk-overlap >= --chunk-size`. When you set only `--chunk-overlap`
against an existing config, an overlap that exceeds the size is clamped to
`chunk_size / 4`.

## Standalone `chunk` command

Chunk text you already have, from `--text` or stdin. Output defaults to
JSON:

```bash
# From a flag
xberg chunk --text "long document text ..." --chunk-size 800 --chunk-overlap 100

# From stdin (pipe extracted content straight in)
xberg extract notes.md | xberg chunk --chunk-size 500 --format json
```

JSON output carries `chunks` (array of strings), `chunk_count`, the
resolved `config` (`max_characters`, `overlap`, `chunker_type`), and
`input_size_bytes`. Use `--format text` for a human-readable dump with
`--- chunk N ---` separators.

> Note: in the JSON output, `chunker_type` is rendered capitalized (`"Text"`,
> `"Markdown"`, `"Yaml"`, `"Semantic"`) because it is emitted via Rust's Debug
> formatting, whereas the `--chunker-type` input flag is lowercase
> (`text`, `markdown`, `yaml`, `semantic`). Lowercase the value before
> comparing if you parse it back.

## Chunker types

`--chunker-type` selects the splitting strategy (standalone `chunk`
command):

| Type       | Behavior                                                            |
| ---------- | ------------------------------------------------------------------- |
| `text`     | **Default.** Plain character-window splitting with overlap.         |
| `markdown` | Markdown-aware — splits on structure (headings, blocks) where possible. |
| `yaml`     | YAML-aware splitting for structured config/data documents.          |
| `semantic` | Topic-boundary splitting driven by `--topic-threshold` (0.0–1.0, default 0.75). |

```bash
# Markdown-aware chunking keeps headings and blocks intact
xberg chunk --text "$(cat README.md)" --chunker-type markdown

# Semantic chunking — lower threshold = more, smaller topic chunks
xberg chunk --text "$(cat transcript.txt)" --chunker-type semantic --topic-threshold 0.6
```

## Token-based sizing

By default `--chunk-size` counts characters. To size chunks by tokens for
a specific model, pass `--chunking-tokenizer` with a HuggingFace tokenizer
id. On the `extract` command this implicitly enables chunking. Requires the
`chunking-tokenizers` feature (present in the default CLI build).

```bash
# Size chunks by GPT-4o tokens during extraction
xberg extract report.pdf --chunking-tokenizer Xenova/gpt-4o --format json

# Or on the standalone command
xberg chunk --text "$(cat doc.txt)" --chunking-tokenizer Xenova/gpt-4o --chunk-size 512
```

With a tokenizer set, `--chunk-size` is interpreted in tokens, not
characters.

## Config file alternative

Field names in config files are snake_case under `[chunking]`:

```toml
[chunking]
max_characters = 1000
overlap = 200
chunker_type = "markdown"
```

```bash
xberg extract report.pdf --config xberg.toml --format json
```

> CLI flags map to config fields as `--chunk-size` → `max_characters` and
> `--chunk-overlap` → `overlap`. In config files use the snake_case names.

## Programmatic access

From Python, enable chunking on the config and read the chunks off the
document in the result envelope (`result.results[0].chunks`):

```python
from xberg import ExtractInput, extract, ExtractionConfig, ChunkingConfig

config = ExtractionConfig(
    chunking=ChunkingConfig(max_characters=1000, overlap=200),
)
result = await extract(ExtractInput(uri="report.pdf"), config)
for chunk in result.results[0].chunks or []:
    print(len(chunk.content))
```

> The public Python `ChunkingConfig` (a dataclass) uses constructor kwargs
> `max_characters` / `overlap`; the Rust core struct fields are also
> `max_characters` / `overlap`. TOML/JSON config keys are `max_chars` /
> `max_overlap` (with `max_characters` / `overlap` accepted as serde aliases),
> and dict-form config passed to `ExtractionConfig` likewise accepts the
> `max_chars` / `max_overlap` aliases; Node's `ChunkingConfig` interface uses
> `maxCharacters` / `overlap`. See `references/python-api.md` and
> `references/rust-api.md` in the sibling `xberg` skill.

## Picking parameters

- **RAG / vector store** — 500–1000 chars (or 256–512 tokens) with
  10–20% overlap. Use `markdown` chunking for docs to keep sections whole.
- **LLM summarization** — larger chunks (1500–4000 chars) with small
  overlap; size by tokens to stay under the model window.
- **Topic segmentation** — `semantic` chunker; tune `--topic-threshold`
  down for finer splits, up for coarser ones.

## Common pitfalls

- **Overlap ≥ size** — rejected on `extract`; clamped to `size / 4` when
  only overlap is changed against an existing config.
- **Tokenizer without the feature** — `--chunking-tokenizer` errors if the
  CLI was built without `chunking-tokenizers`. The default build includes it.
- **Empty input** — the standalone `chunk` command bails on empty text;
  provide `--text` or pipe non-empty stdin.

See `references/configuration.md` for the full `[chunking]` schema and
`references/cli-reference.md` for every chunk flag.
