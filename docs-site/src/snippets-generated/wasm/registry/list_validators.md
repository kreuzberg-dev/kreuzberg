---
id: fixture_wasm_list_validators
language: typescript
target: wasm
level: typecheck
requires: []
side_effect: safe
---

List validators

```typescript title="WebAssembly"
import { listValidators } from "@xberg-io/xberg-wasm";
function main() {
  const result = listValidators();
  console.log(result);
}

void main();

```
