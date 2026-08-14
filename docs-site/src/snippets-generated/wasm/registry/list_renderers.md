---
id: fixture_wasm_list_renderers
language: typescript
target: wasm
level: typecheck
requires: []
side_effect: safe
---

List renderers

```typescript title="WebAssembly"
import { listRenderers } from "@xberg-io/xberg-wasm";
function main() {
  const result = listRenderers();
  console.log(result);
}

void main();

```
