---
id: fixture_wasm_url_html_page_extract
language: typescript
target: wasm
level: typecheck
requires: []
side_effect: server
---

extract: website URL returns page content

```typescript title="WebAssembly"
import { ExtractInput, ExtractInputKind, extract } from "@xberg-io/xberg-wasm";
async function main() {
  const input: WasmExtractInput = (() => { const _u0 = WasmExtractInput.default(); _u0.kind = WasmExtractInputKind.Uri; _u0.uri = "https://example.com"; return _u0; })();
  const result = await extract(input, { url: { mode: "document" } });
  console.log(result);
}

void main();

```
