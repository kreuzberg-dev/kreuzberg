---
id: fixture_wasm_embedding_backends_list
language: typescript
target: wasm
level: typecheck
requires: []
side_effect: safe
---

List all registered embedding backends

```typescript title="WebAssembly"
import { listEmbeddingBackends } from "@xberg-io/xberg-wasm";
function main() {
  const result = listEmbeddingBackends();
  console.log(result);
}

void main();

```
