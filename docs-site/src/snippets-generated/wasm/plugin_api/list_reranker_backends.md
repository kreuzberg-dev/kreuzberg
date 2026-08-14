---
id: fixture_wasm_list_reranker_backends
language: typescript
target: wasm
level: typecheck
requires: []
side_effect: safe
---

List all registered reranker backends

```typescript title="WebAssembly"
import { listRerankerBackends } from "@xberg-io/xberg-wasm";
function main() {
  const result = listRerankerBackends();
  console.log(result);
}

void main();

```
