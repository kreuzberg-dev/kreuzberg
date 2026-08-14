---
id: fixture_wasm_ocr_backends_list
language: typescript
target: wasm
level: typecheck
requires: []
side_effect: safe
---

List all registered OCR backends

```typescript title="WebAssembly"
import { listOcrBackends } from "@xberg-io/xberg-wasm";
function main() {
  const result = listOcrBackends();
  console.log(result);
}

void main();

```
