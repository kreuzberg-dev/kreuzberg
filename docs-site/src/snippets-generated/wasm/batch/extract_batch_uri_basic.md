---
id: fixture_wasm_extract_batch_uri_basic
language: typescript
target: wasm
level: typecheck
requires: []
side_effect: safe
---

extract_batch over URI inputs

```typescript title="WebAssembly"
import { extractBatch } from "@xberg-io/xberg-wasm";
async function main() {
  const result = await extractBatch([{ kind: "uri", uri: "pdf/fake_memo.pdf" }, { kind: "uri", uri: "text/fake_text.txt" }], undefined);
  for (const result of result.results) {
    console.log(result.content);
  }
}

void main();

```
