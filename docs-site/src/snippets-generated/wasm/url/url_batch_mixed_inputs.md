---
id: fixture_wasm_url_batch_mixed_inputs
language: typescript
target: wasm
level: typecheck
requires: []
side_effect: server
---

extract_batch: mixed bytes and URL inputs share one output envelope

```typescript title="WebAssembly"
import { UrlExtractionMode, WasmExtractionConfig, extractBatch } from "@xberg-io/xberg-wasm";
async function main() {
  const config: WasmExtractionConfig = (() => { const _u0 = WasmExtractionConfig.default(); _u0.url = (() => { const _u1 = WasmUrlExtractionConfig.default(); _u1.mode = UrlExtractionMode.Document; return _u1; })(); return _u0; })();
  const result = await extractBatch([{ kind: "uri", uri: "https://example.com" }, { bytes: [66, 97, 116, 99, 104, 32, 98, 121, 116, 101, 115, 32, 99, 111, 110, 116, 101, 110, 116], filename: "inline.txt", kind: "bytes", mimeType: "text/plain" }], config);
  console.log(result);
}

void main();

```
