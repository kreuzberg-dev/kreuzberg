<!--
AI-RULEZ :: GENERATED FILE — DO NOT EDIT
Content-Hash: blake3:917bbfdad5cea593917f1a41af806fd4fb83410a4acc13558ce217d6548702ef
Source-Hash: blake3:c3730974b95d68f86536f7c420114fdefdaf54253473d6bec3cfefe6dacc7d4d
Schema-Version: v1
-->

# Node.js/TypeScript API Reference

## Overview

**Package**: `@xberg-io/xberg` — a high-performance TypeScript SDK built on a Rust core for document intelligence and content extraction.

Install with `npm install @xberg-io/xberg`. Supports both **ESM** (`import`) and **CommonJS** (`require`):

```typescript
// ESM
import { extract, extractBatch } from "@xberg-io/xberg";

// CommonJS
const { extract, extractBatch } = require("@xberg-io/xberg");
```

There are exactly two extraction entry points — `extract` and `extractBatch` — and both are `async` (return `Promise`). There are no `extractFile` / `extractBytes` / `extractFileSync` / `batchExtractFiles` / worker-pool functions.

---

## Core Extraction Functions

### `extract(input?, config?): Promise<ExtractionResult>`

Extract content from a single input. Returns an `ExtractionResult` **envelope** — the extracted document(s) live in `output.results`.

```typescript
import { extract } from "@xberg-io/xberg";

const output = await extract({ kind: "uri", uri: "document.pdf" });

console.log(output.results[0].content);
console.log(`MIME type: ${output.results[0].mimeType}`);
console.log(`Results: ${output.summary.results}`);
```

**Parameters**:

- `input?: ExtractInput` — the input to extract (see [ExtractInput](#extractinput)).
- `config?: ExtractionConfig` — optional extraction configuration.

**Returns**: `Promise<ExtractionResult>`.

### `extractBatch(inputs, config?): Promise<ExtractionResult>`

Extract content from multiple inputs in parallel. Returns a single envelope whose `results` array holds one document per input (in input order), plus any per-input `errors`.

```typescript
import { extractBatch } from "@xberg-io/xberg";

const output = await extractBatch([
  { kind: "uri", uri: "document.pdf" },
  {
    kind: "bytes",
    bytes: Buffer.from("Hello from memory"),
    mimeType: "text/plain",
    filename: "note.txt",
  },
]);

for (const doc of output.results) {
  console.log(doc.content.slice(0, 200));
}
```

**Parameters**:

- `inputs: ExtractInput[]` — array of inputs.
- `config?: ExtractionConfig` — configuration applied to all inputs.

**Returns**: `Promise<ExtractionResult>`.

---

## ExtractInput

The input to `extract` / `extractBatch` is a plain object discriminated by `kind`:

```typescript
// From a path, file:// URI, or HTTP(S) URL
{ kind: "uri", uri: "document.pdf" }

// From raw bytes (Buffer or Uint8Array)
{
  kind: "bytes",
  bytes: Buffer.from(/* ... */),
  mimeType: "application/pdf",   // optional hint
  filename: "document.pdf",      // optional hint
}
```

`ExtractInputKind` is exported as an enum (`ExtractInputKind.Uri === "uri"`, `ExtractInputKind.Bytes === "bytes"`), but the string literals shown above work directly.

---

## Configuration Interface

### `ExtractionConfig`

Configuration object controlling extraction behavior. All fields optional; field names are **camelCase**.

```typescript
interface ExtractionConfig {
  // Caching and processing
  useCache?: boolean; // default: true
  enableQualityProcessing?: boolean; // default: true

  // OCR
  ocr?: OcrConfig;
  forceOcr?: boolean; // default: false

  // Document processing
  chunking?: ChunkingConfig;
  images?: ImageExtractionConfig;
  pdfOptions?: PdfConfig;
  tokenReduction?: TokenReductionOptions;
  languageDetection?: LanguageDetectionConfig;
  postprocessor?: PostProcessorConfig;
  htmlOutput?: HtmlOutputConfig;
  keywords?: KeywordConfig;
  pages?: PageConfig;

  // Output control
  maxConcurrentExtractions?: number;
  outputFormat?: "plain" | "markdown" | "djot" | "html"; // default: 'plain'
  resultFormat?: "unified" | "element_based"; // default: 'unified'
  securityLimits?: SecurityLimits;
}
```

```typescript
import { extract, type ExtractionConfig } from "@xberg-io/xberg";

const config: ExtractionConfig = {
  useCache: true,
  enableQualityProcessing: true,
  outputFormat: "markdown",
  ocr: { backend: "tesseract", language: ["eng", "deu"], tesseractConfig: { psm: 6 } },
  chunking: { maxCharacters: 1000, overlap: 200 },
};

const output = await extract({ kind: "uri", uri: "document.pdf" }, config);
console.log(output.results[0].content);
```

### `ChunkingConfig`

```typescript
interface ChunkingConfig {
  maxCharacters?: number; // max characters per chunk
  overlap?: number; // overlap between chunks
  chunkerType?: "text" | "markdown" | "semantic";
  prependHeadingContext?: boolean; // markdown chunker: prefix heading breadcrumb
  embedding?: { model?: { type: "preset"; name: string } } | Record<string, unknown>; // embedding config
  preset?: string; // named preset
}
```

**Key Point**: Use `maxCharacters` and `overlap` (there are no `maxChars`/`maxOverlap` fields).

```typescript
const config = {
  chunking: {
    maxCharacters: 1000,
    overlap: 200,
    embedding: { model: { type: "preset", name: "balanced" } },
  },
};
const output = await extract({ kind: "uri", uri: "document.pdf" }, config);
console.log(`Total chunks: ${output.results[0].chunks?.length ?? 0}`);
```

### `OcrConfig`

```typescript
interface OcrConfig {
  backend: string; // 'tesseract', 'paddleocr', 'paddle-ocr', 'vlm'
  language?: string[]; // ['eng'], ['eng', 'deu']; joined with '+' for Tesseract
  tesseractConfig?: TesseractConfig;
}

interface TesseractConfig {
  psm?: number; // Page Segmentation Mode (0-13)
  oem?: number; // OCR Engine Mode (0-3)
  enableTableDetection?: boolean;
  minConfidence?: number;
  tesseditCharWhitelist?: string;
}
```

### `PdfConfig`

```typescript
interface PdfConfig {
  extractImages?: boolean; // default: false
  extractTables?: boolean; // default: true
  passwords?: string[]; // passwords for encrypted PDFs
  extractMetadata?: boolean; // default: true
  hierarchy?: HierarchyConfig;
}
```

### `LanguageDetectionConfig`

```typescript
interface LanguageDetectionConfig {
  enabled?: boolean; // default: true
  minConfidence?: number; // 0.0-1.0 (default: 0.8)
  detectMultiple?: boolean; // default: false
}
```

### `TokenReductionOptions`

```typescript
interface TokenReductionOptions {
  mode?: "off" | "light" | "moderate" | "aggressive" | "maximum";
  preserveImportantWords?: boolean; // default: true
}
```

### `KeywordConfig`

```typescript
interface KeywordConfig {
  algorithm?: "yake" | "rake"; // default: 'yake'
  maxKeywords?: number;
  minScore?: number;
  language?: string;
  yakeParams?: { windowSize?: number };
  rakeParams?: { minWordLength?: number; maxWordsPerPhrase?: number };
}
```

### `PageConfig`

```typescript
interface PageConfig {
  extractPages?: boolean;
  insertPageMarkers?: boolean;
  markerFormat?: string; // contains {page_num}
}
```

---

## Result Types

### `ExtractionResult` (envelope)

`extract` and `extractBatch` both resolve to this envelope. Per-document data is in `results`.

```typescript
interface ExtractionResult {
  results: ExtractedDocument[]; // one per input, in input order
  errors: ExtractionErrorItem[]; // non-fatal per-input errors
  summary: ExtractionSummary; // { inputs, results, errors, ... }
}
```

### `ExtractedDocument`

```typescript
interface ExtractedDocument {
  content: string; // main extracted text
  mimeType: string;
  metadata: Metadata; // common fields at top level; format-specific under metadata.format, custom under metadata.additional
  tables: Table[];
  detectedLanguages?: string[] | null;
  chunks?: Chunk[] | null;
  images?: ExtractedImage[] | null;
  pages?: PageContent[] | null;
  elements?: Element[] | null; // when resultFormat: 'element_based'
  extractedKeywords?: Keyword[] | null;
  qualityScore?: number | null;
  processingWarnings?: ProcessingWarning[];
}
```

### `Table`

```typescript
interface Table {
  cells: string[][]; // 2D array (rows × columns)
  markdown: string;
  pageNumber: number; // 1-indexed
}
```

```typescript
const output = await extract({ kind: "uri", uri: "document.pdf" });
output.results[0].tables?.forEach((table) => {
  console.log(`Table with ${table.cells?.length ?? 0} rows`);
  console.log(table.markdown);
});
```

### `Chunk`

```typescript
interface Chunk {
  content: string;
  embedding?: number[] | null; // when chunking embedding is configured
  metadata: ChunkMetadata;
}
```

### `Keyword`

```typescript
interface Keyword {
  text: string;
  score: number;
  algorithm: KeywordAlgorithm; // 'yake' | 'rake'
  positions?: number[] | null;
}
```

### `Metadata`

Metadata is **not** flat. Common fields (`title`, `subject`, `authors`, `keywords`, `language`, `createdAt`/`modifiedAt`, `pages`, etc.) sit at the top level, but format-specific fields nest under `metadata.format` (a `FormatMetadata` discriminated union keyed by `format_type`), and custom post-processor fields nest under `metadata.additional`. There is no `page_count`/`pageCount` member on `Metadata`, and use `authors` (an array), not `author`. Read page count from format-specific metadata or from `metadata.pages` (PageStructure):

```typescript
const doc = output.results[0];
if (doc.metadata?.format?.format_type === "pdf") {
  console.log(`Pages: ${doc.metadata.format.pageCount}`);
} else if (doc.metadata?.pages) {
  console.log(`Pages: ${doc.metadata.pages}`);
}
```

---

## Error Handling

The Node binding throws standard `Error` objects (it does **not** export typed error classes such as `ParsingError` / `OcrError`). Catch with `instanceof Error`, and inspect the per-input `errors` array on the envelope for non-fatal failures.

```typescript
import { extract } from "@xberg-io/xberg";

try {
  const output = await extract({ kind: "uri", uri: "missing.pdf" });
  if (output.errors.length > 0) {
    console.error("Per-input errors:", output.errors);
  }
  console.log(output.results[0]?.content);
} catch (error: unknown) {
  if (error instanceof Error) {
    console.error(`Extraction failed: ${error.message}`);
  }
  throw error;
}
```

---

## Plugin System

Custom plugins extend the Rust pipeline. Plugin callbacks receive a single `ExtractedDocument`.

### Post-Processors

```typescript
import { registerPostProcessor } from "@xberg-io/xberg";

const processor = {
  name() {
    return "my_processor";
  },
  async process(result) {
    result.metadata["custom_field"] = "value";
    return result;
  },
  processingStage() {
    return "Late"; // "Early" | "Middle" | "Late"
  },
};

registerPostProcessor(processor);
```

`unregisterPostProcessor(name)`, `listPostProcessors()`, and `clearPostProcessors()` manage the registry.

### Validators

```typescript
import { registerValidator } from "@xberg-io/xberg";

const validator = {
  name() {
    return "content_length_validator";
  },
  validate(result) {
    if (result.content.length < 10) {
      throw new Error("Content too short");
    }
  },
  priority() {
    return 100; // higher = runs first
  },
};

registerValidator(validator);
```

`unregisterValidator(name)`, `listValidators()`, and `clearValidators()` manage the registry.

### OCR Backends

```typescript
import { registerOcrBackend } from "@xberg-io/xberg";

const backend = {
  name() {
    return "my-ocr";
  },
  supportedLanguages() {
    return ["eng", "deu", "fra"];
  },
  async processImage(imageBytes, config) {
    return {
      content: "extracted text",
      mimeType: "text/plain",
      metadata: { confidence: 0.95, language: config.language },
      tables: [],
    };
  },
};

registerOcrBackend(backend);
```

`unregisterOcrBackend(name)`, `listOcrBackends()`, and `clearOcrBackends()` manage the registry.

---

## Embeddings

Embedding models are selected inside `chunking.embedding` via a `model` tagged union (`{ type: "preset", name }` for a named preset, or a custom model object). Presets: `fast`, `balanced`, `quality`, `multilingual`.

```typescript
const config = {
  chunking: {
    maxCharacters: 512,
    embedding: { model: { type: "preset", name: "balanced" } },
  },
};

const output = await extract({ kind: "uri", uri: "document.pdf" }, config);
for (const chunk of output.results[0].chunks ?? []) {
  console.log(`Embedding dims: ${chunk.embedding?.length ?? 0}`);
}
```

---

## Other Functions

```typescript
import { listSupportedFormats } from "@xberg-io/xberg";

console.log(listSupportedFormats());
```

---

## Supported Document Formats

Xberg supports 96 file formats: PDF, Office (DOCX, PPTX, XLSX, …), eBooks, images (with OCR), HTML/XML/SVG, email (EML, MSG), archives, data (JSON/YAML/TOML/CSV), academic, and source code (371 languages). See [supported-formats.md](supported-formats.md) for the complete list.
