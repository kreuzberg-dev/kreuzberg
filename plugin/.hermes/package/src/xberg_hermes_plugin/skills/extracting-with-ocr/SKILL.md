---
name: extracting-with-ocr
description: Use when extracting text from scanned PDFs, photographed pages, or images that have no embedded text layer. Covers OCR backends, language packs, force-OCR, and performance tuning.
---

<!--
AI-RULEZ :: GENERATED FILE — DO NOT EDIT
Content-Hash: blake3:3ec8b7cf60f56cbe5cc15a5a0b29d0c2f8e3cf4c3823503eb1128fb7f9ee11db
Source-Hash: blake3:c3730974b95d68f86536f7c420114fdefdaf54253473d6bec3cfefe6dacc7d4d
Schema-Version: v1
-->

# Extracting with OCR

Use this when a document is image-based: scanned PDFs, photographed pages,
screenshots, JPEG/PNG/TIFF with text. Xberg auto-OCRs raster images and
auto-detects PDFs that lack a text layer. Force it on when extraction
returned empty/garbled text from a PDF that "looks" textual.

## When to force OCR

- Extraction returned an empty `content` field, but the file opens visually.
- The PDF text layer is junk (copy-paste from a viewer produces gibberish).
- You want consistent output across mixed scanned + digital PDFs.

```bash
xberg extract scan.pdf --force-ocr=true
xberg extract scan.pdf --ocr=true --ocr-language eng
```

If a page has an unreliable text layer, `--force-ocr=true` re-rasterizes
and runs OCR on every page.

## Backends

Tesseract is the default and ships with the CLI — no extra install. Other
backends are opt-in:

| Backend       | Flag                                  | Install                                          | Notes                                                          |
| ------------- | ------------------------------------- | ------------------------------------------------ | -------------------------------------------------------------- |
| Tesseract     | `--ocr-backend tesseract` (default)   | bundled                                          | Best general-purpose, 100+ languages via tessdata.             |
| PaddleOCR     | `--ocr-backend paddle-ocr`            | bundled (ONNX Runtime)                           | Strong on Asian scripts. Not available on WASM or Windows.     |
| Candle VLM    | `--ocr-backend candle-trocr` (and other `candle-*`) | bundled (Candle)                  | Local vision OCR models (`candle-trocr`, `candle-paddleocr-vl`, `candle-glm-ocr`, `candle-deepseek-ocr`). |
| VLM (hosted)  | `--ocr-backend vlm` + `--vlm-model`   | liter-llm provider (`--vlm-api-key`)             | Multimodal LLM via liter-llm. Use when OCR fails on dense or handwritten layouts. |

Pick Tesseract first. Switch only when accuracy is unacceptable.

## Language packs

Tesseract uses ISO 639-2 codes. Default is `eng`. Combine with `+`:

```bash
xberg extract menu.jpg --ocr=true --ocr-language "eng+deu"
xberg extract bilingual.pdf --ocr-language "eng+jpn"
xberg extract any.pdf --ocr-language all   # all installed packs
```

Install missing packs at the OS level:

```bash
# macOS
brew install tesseract-lang

# Debian/Ubuntu
sudo apt install tesseract-ocr-deu tesseract-ocr-jpn tesseract-ocr-fra

# Specific lang only
sudo apt install tesseract-ocr-<iso639-2>
```

Xberg fails fast with a helpful error if you request a language pack
that is not installed. Read the error — it names the missing file.

## Useful flags

- `--ocr=true` — enable OCR (auto-enabled for images and scanned PDFs).
- `--force-ocr=true` — OCR every page even if a text layer exists.
- `--disable-ocr=true` — never OCR (extract embedded text only or fail).
- `--ocr-language <lang>` — single code or `+`-joined list, or `all`.
- `--ocr-backend <tesseract|paddle-ocr|vlm|candle-trocr|candle-paddleocr-vl|candle-glm-ocr|candle-deepseek-ocr>` — pick backend.
- `--ocr-auto-rotate=true` — pre-rotate via the auto-rotate model.
- `--acceleration <cpu|coreml|cuda|tensorrt|auto>` — ONNX accelerator for
  paddle-ocr / auto-rotate / layout models.

## Performance tips

- Cache is on by default. Repeated extraction of the same file + config is
  instant. Do not pass `--no-cache=true` unless you have a reason.
- For batch OCR, use `xberg batch *.pdf --ocr=true` — internal worker
  pool parallelizes across CPU cores. Cap with `--max-concurrent N` if
  memory is tight.
- Raise `--target-dpi` (default 300) only for low-resolution scans. Higher
  DPI is slower; 200 is usually enough for printed text.
- Enable `--ocr-auto-rotate=true` only when pages may be rotated; the
  classifier adds latency.
- On Apple Silicon, `--acceleration coreml` typically beats CPU for
  paddle-ocr and layout detection.

## Config file alternative

Long flag chains belong in `xberg.toml` — auto-discovered from cwd
upward.

```toml
force_ocr = true
output_format = "markdown"

[ocr]
backend = "tesseract"
language = "eng+deu"
auto_rotate = true
```

Then just run:

```bash
xberg extract document.pdf
```

## Common failure modes

- **"missing tessdata"** — install the language pack at OS level (see above).
- **Empty content on a scanned PDF without `--force-ocr`** — the file has a
  bogus zero-width text layer. Re-run with `--force-ocr=true`.
- **OCR on a rotated page** — add `--ocr-auto-rotate=true` or pre-rotate.
- **Garbled CJK output** — ensure the right language pack is installed and
  passed via `--ocr-language`; consider `paddle-ocr` for Chinese/Japanese.

See `references/cli-reference.md` and `references/configuration.md` in the
sibling `xberg` skill for the full flag and config schema.
