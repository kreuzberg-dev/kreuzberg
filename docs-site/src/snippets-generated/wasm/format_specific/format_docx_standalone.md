---
id: fixture_wasm_format_docx_standalone
language: typescript
target: wasm
level: typecheck
requires: []
side_effect: server
---

Standalone DOCX extraction using extract

```typescript title="WebAssembly"
import { ExtractInput, ExtractInputKind, extract } from "@xberg-io/xberg-wasm";
async function main() {
  const input: WasmExtractInput = (() => { const _u0 = WasmExtractInput.default(); _u0.filename = "fake.docx"; _u0.kind = WasmExtractInputKind.Uri; _u0.mimeType = "application/vnd.openxmlformats-officedocument.wordprocessingml.document"; _u0.uri = "https://example.com/docx/fake.docx"; return _u0; })();
  const result = await extract(input, undefined);
  console.log(result);
}

void main();

```
