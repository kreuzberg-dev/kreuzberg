---
id: fixture_wasm_config_element_types
language: typescript
target: wasm
level: typecheck
requires: []
side_effect: server
---

Tests element-based result format with element type assertions on DOCX

```typescript title="WebAssembly"
import { ExtractInput, ExtractInputKind, extract } from "@xberg-io/xberg-wasm";
async function main() {
  const input: WasmExtractInput = (() => { const _u0 = WasmExtractInput.default(); _u0.kind = WasmExtractInputKind.Uri; _u0.uri = "https://example.com/docx/unit_test_headers.docx"; return _u0; })();
  const result = await extract(input, { resultFormat: "element_based" });
  const [first] = result.results ?? [];
  for (const element of first?.elements) {
    console.log(element.elementType);
    console.log(element.content);
  }
}

void main();

```
