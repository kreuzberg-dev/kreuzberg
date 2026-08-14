---
id: fixture_wasm_url_remote_text_document
language: typescript
target: wasm
level: typecheck
requires: []
side_effect: server
---

extract: remote text document URL

```typescript title="WebAssembly"
import { ExtractInput, ExtractInputKind, extract } from "@xberg-io/xberg-wasm";
async function main() {
  const input: WasmExtractInput = (() => { const _u0 = WasmExtractInput.default(); _u0.kind = WasmExtractInputKind.Uri; _u0.uri = "https://example.com"; return _u0; })();
  const result = await extract(input, { url: { mode: "document" } });
  console.log(result);
}

void main();

```
