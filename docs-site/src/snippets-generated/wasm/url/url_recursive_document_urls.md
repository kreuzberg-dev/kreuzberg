---
id: fixture_wasm_url_recursive_document_urls
language: typescript
target: wasm
level: typecheck
requires: []
side_effect: server
---

extract: recursive URL extraction follows document links discovered in results

```typescript title="WebAssembly"
import { ExtractInput, ExtractInputKind, extract } from "@xberg-io/xberg-wasm";
async function main() {
  const input: WasmExtractInput = (() => { const _u0 = WasmExtractInput.default(); _u0.kind = ExtractInputKind.Uri; _u0.uri = "https://example.com"; return _u0; })();
  const result = await extract(input, { url: { crawl: { documentUrlDepth: 1, followDocumentUrls: true, respectRobotsTxt: false }, mode: "document" } });
  console.log(result);
}

void main();

```
