---
name: extracting-keywords
description: Use when extracting keywords (YAKE/RAKE) from documents — and, secondarily, when detecting document language or generating embeddings for RAG and search. Covers the keyword config (and its feature gating), `--detect-language`, and the standalone `embed` command with real flags.
---

<!--
AI-RULEZ :: GENERATED FILE — DO NOT EDIT
Content-Hash: blake3:0da13ce50f1fef8c192e912e7c2b0d039209a7b676829abe13f4fc86b5ea20bc
Source-Hash: blake3:ac8bab9ae4a76e61f437af29b4a45a38326056a50769a177038927c69353edaf
Schema-Version: v1
-->

# Extracting keywords, language, and embeddings

Use this for the enrichment surface around extraction: statistical keyword
extraction, language detection, and vector embeddings. Keywords and
language detection ride along with extraction and land on the result;
embeddings are produced by a dedicated `embed` command.

## Keywords (YAKE / RAKE)

Keyword extraction is configured via the `[keywords]` config block (or
inline JSON) — there is no single `--keywords` CLI flag. When enabled,
extracted keywords appear on `result.extracted_keywords` (`extractedKeywords`
in Node.js; the CLI JSON field is `extracted_keywords`). Two algorithms are
available:

- **YAKE** (`"yake"`) — statistical, unsupervised single-document
  extraction. Good general default.
- **RAKE** (`"rake"`) — co-occurrence / phrase-based. Favors multi-word
  key phrases.

> Feature-gated: keyword extraction requires the CLI to be built with the
> `keywords-yake` and/or `keywords-rake` Cargo features (both are in the
> default/`full` build). If the CLI was built without them, the `[keywords]`
> config block is silently ignored — `result.extracted_keywords` simply stays empty
> rather than erroring. The `"yake"` algorithm needs `keywords-yake`; `"rake"`
> needs `keywords-rake`.

Enable via inline JSON on the CLI:

```bash
xberg extract paper.pdf --format json \
  --config-json '{"keywords":{"algorithm":"yake","max_keywords":15,"language":"en"}}' \
  | jq '.extracted_keywords'
```

Or in a config file:

```toml
[keywords]
algorithm = "rake"       # "yake" or "rake"
max_keywords = 10        # default 10
min_score = 0.0          # filter below this score (normalized 0.0-1.0 for both algorithms)
ngram_range = [1, 3]     # unigrams..trigrams (default); config-file only
language = "en"          # stopword language; omit to skip stopword filtering
```

```bash
xberg extract report.pdf --config xberg.toml --format json | jq '.extracted_keywords'
```

Field notes:

- `max_keywords` caps how many keywords are returned (default 10).
- `min_score` filters low-scoring keywords. Both YAKE and RAKE normalize
  their scores to the `0.0`-`1.0` range with *higher-is-better*, so
  `min_score` retains keywords with `score >= min_score` identically for
  either algorithm.
- `ngram_range` is `[min, max]`: `[1,1]` unigrams only, `[1,2]` adds
  bigrams, `[1,3]` (default) adds trigrams. Config-file only — it is not a
  field on the language bindings' `KeywordConfig`.
- `language` enables stopword filtering for that language; omit it to
  disable stopword filtering entirely.

## Language detection

Language detection is a real CLI flag: `--detect-language`. Detected
languages appear on `result.detected_languages`:

```bash
xberg extract multilingual.pdf --detect-language true --format json \
  | jq '.detected_languages'
```

In a config file it lives under `[language_detection]`:

```toml
[language_detection]
enabled = true
min_confidence = 0.8
detect_multiple = false
```

The CLI flag enables detection with `min_confidence = 0.8` and
single-language mode; use the config block to detect multiple languages or
tune confidence.

## Embeddings (`embed` command)

The standalone `embed` command produces vector embeddings for text from
`--text` (repeatable) or stdin. It does not run extraction — pipe
extracted content in if you want document embeddings.

```bash
# Local ONNX preset model (default provider)
xberg embed --text "first passage" --text "second passage" --preset balanced

# Embed extracted document text
xberg extract report.pdf | xberg embed --preset quality
```

Presets for the local provider: `fast`, `balanced` (default), `quality`,
`multilingual`. Output defaults to JSON (`--format json`).

`--provider` selects the embedding source:

| Provider | Flag                                  | Notes                                         |
| -------- | ------------------------------------- | --------------------------------------------- |
| `local`  | `--preset <fast\|balanced\|quality\|multilingual>` | **Default.** ONNX model, no API key. |
| `llm`    | `--model <id>` `--api-key <key>`      | liter-llm routing, e.g. `openai/text-embedding-3-small`. |
| `plugin` | `--plugin <name>`                     | A backend pre-registered in-process via the plugin API. |

```bash
# Provider-hosted embeddings via an LLM
xberg embed --text "query text" \
  --provider llm --model openai/text-embedding-3-small --api-key "$OPENAI_API_KEY"
```

Local embedding presets must be downloaded first if not cached. Pre-warm
them with the cache command:

```bash
xberg cache warm --embedding-model balanced   # one preset
xberg cache warm --all-embeddings             # all available presets (currently 8)
```

## Programmatic access

Keywords and detected languages live on the document in the result envelope:

```python
from xberg import ExtractInput, extract, ExtractionConfig, KeywordConfig, KeywordAlgorithm

config = ExtractionConfig(
    keywords=KeywordConfig(algorithm=KeywordAlgorithm.YAKE, max_keywords=15, language="en"),
)
result = await extract(ExtractInput(uri="paper.pdf"), config)
doc = result.results[0]
print(doc.extracted_keywords)   # extracted keywords (when enabled)
print(doc.detected_languages)   # detected languages (when enabled)
```

See `references/python-api.md` and `references/configuration.md` in the
sibling `xberg` skill for the keyword / language-detection config
classes and the embedding presets.

## Common pitfalls

- **No `--keywords` flag** — keyword extraction is config-only. Use
  `--config-json '{"keywords":{...}}'` or a `[keywords]` config block.
- **`min_score` direction** — scores are normalized to `0.0`-`1.0` with
  higher-is-better for both YAKE and RAKE, so the same threshold behaves
  identically for either algorithm.
- **Embeddings ≠ extraction** — `embed` only takes raw text. Pipe
  `xberg extract` output into it for document vectors.
- **Cold embedding models** — first local run downloads the preset; run
  `xberg cache warm --all-embeddings` to pre-populate.

See `references/advanced-features.md` for the embeddings pipeline and
`references/cli-reference.md` for the `embed` and `cache warm` flag sets.
