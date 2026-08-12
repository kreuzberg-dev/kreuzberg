---
id: fixture_wasm_config_document_structure_with_headings
language: typescript
target: wasm
level: typecheck
requires: []
side_effect: server
---

Tests document structure with DOCX heading-driven nesting

```typescript title="WebAssembly"
import { ExtractInput, ExtractInputKind, extract } from "@xberg-io/xberg-wasm";
async function main() {
  const input: WasmExtractInput = (() => { const _u0 = WasmExtractInput.default(); _u0.kind = ExtractInputKind.Uri; _u0.uri = "https://example.com/docx/fake.docx"; return _u0; })();
  const result = await extract(input, { includeDocumentStructure: true });
  console.log(result.results[0].documentStructure);
}

void main();

```
