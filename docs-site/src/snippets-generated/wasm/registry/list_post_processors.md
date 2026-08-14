---
id: fixture_wasm_list_post_processors
language: typescript
target: wasm
level: typecheck
requires: []
side_effect: safe
---

List post-processors

```typescript title="WebAssembly"
import { listPostProcessors } from "@xberg-io/xberg-wasm";
function main() {
  const result = listPostProcessors();
  console.log(result);
}

void main();

```
