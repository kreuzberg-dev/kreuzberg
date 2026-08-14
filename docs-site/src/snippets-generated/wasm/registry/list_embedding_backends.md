---
id: fixture_wasm_list_embedding_backends
language: typescript
target: wasm
level: typecheck
requires: []
side_effect: safe
---

List embedding backends

```typescript title="WebAssembly"
import { listEmbeddingBackends } from "@xberg-io/xberg-wasm";
function main() {
  const result = listEmbeddingBackends();
  console.log(result);
}

void main();

```
