---
id: fixture_wasm_renderers_list
language: typescript
target: wasm
level: typecheck
requires: []
side_effect: safe
---

List all registered renderers

```typescript title="WebAssembly"
import { listRenderers } from "@xberg-io/xberg-wasm";
function main() {
  const result = listRenderers();
  console.log(result);
}

void main();

```
