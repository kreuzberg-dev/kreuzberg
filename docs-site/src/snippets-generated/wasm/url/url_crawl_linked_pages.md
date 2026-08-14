---
id: fixture_wasm_url_crawl_linked_pages
language: typescript
target: wasm
level: typecheck
requires: []
side_effect: server
---

extract: crawl mode follows linked pages

```typescript title="WebAssembly"
import { ExtractInput, ExtractInputKind, extract } from "@xberg-io/xberg-wasm";
async function main() {
  const input: WasmExtractInput = (() => { const _u0 = WasmExtractInput.default(); _u0.kind = ExtractInputKind.Uri; _u0.uri = "https://example.com"; return _u0; })();
  const result = await extract(input, { url: { crawl: { maxDepth: 1, maxPages: 4, respectRobotsTxt: false }, mode: "crawl" } });
  console.log(result);
}

void main();

```
