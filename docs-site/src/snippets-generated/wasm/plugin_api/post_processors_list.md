---
id: fixture_wasm_post_processors_list
language: typescript
target: wasm
level: typecheck
requires: []
side_effect: safe
---

List all registered post-processors

```typescript title="WebAssembly"
import { listPostProcessors } from "@xberg-io/xberg-wasm";
function main() {
  const result = listPostProcessors();
  console.log(result);
}

void main();

```
