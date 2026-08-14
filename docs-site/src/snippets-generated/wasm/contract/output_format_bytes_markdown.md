---
id: fixture_wasm_output_format_bytes_markdown
language: typescript
target: wasm
level: typecheck
requires: []
side_effect: safe
---

Tests markdown output format via bytes extraction API

```typescript title="WebAssembly"
import { ExtractInput, ExtractInputKind, OutputFormat, extract } from "@xberg-io/xberg-wasm";
async function main() {
  const input: WasmExtractInput = await (async () => { const _u0 = WasmExtractInput.default(); _u0.bytes = await (await import("node:fs/promises")).readFile("test_documents/pdf/fake_memo.pdf"); _u0.config = await (async () => { const _u1 = WasmFileExtractionConfig.default(); _u1.outputFormat = OutputFormat.Markdown; return _u1; })(); _u0.filename = "fake_memo.pdf"; _u0.kind = ExtractInputKind.Bytes; _u0.mimeType = "application/pdf"; return _u0; })();
  const result = await extract(input, { outputFormat: "markdown" });
  console.log(result);
}

void main();

```
