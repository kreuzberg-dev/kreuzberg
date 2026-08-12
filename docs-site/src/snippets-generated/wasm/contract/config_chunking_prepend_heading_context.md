---
id: fixture_wasm_config_chunking_prepend_heading_context
language: typescript
target: wasm
level: typecheck
requires: []
side_effect: server
---

Tests markdown chunker records heading hierarchy on chunk metadata

```typescript title="WebAssembly"
import { ExtractInput, ExtractInputKind, extract } from "@xberg-io/xberg-wasm";
async function main() {
  const input: WasmExtractInput = (() => { const _u0 = WasmExtractInput.default(); _u0.kind = ExtractInputKind.Uri; _u0.uri = "document.md"; return _u0; })();
  const result = await extract(input, { chunking: { chunkerType: "markdown", maxCharacters: 500, overlap: 50, prependHeadingContext: true } });
  const [first] = result.results ?? [];
  for (const chunk of first?.chunks ?? []) {
    console.log(chunk.content);
    console.log(chunk.metadata);
  }
}

void main();

```
