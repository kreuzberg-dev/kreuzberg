---
id: fixture_wasm_list_ocr_backends
language: typescript
target: wasm
level: typecheck
requires: []
side_effect: safe
---

List OCR backends

```typescript title="WebAssembly"
import { listOcrBackends } from "@xberg-io/xberg-wasm";
function main() {
  const result = listOcrBackends();
  console.log(result);
}

void main();

```
