---
id: fixture_wasm_tokenizer_backends_list
language: typescript
target: wasm
level: typecheck
requires: []
side_effect: safe
---

List all registered tokenizer backends

```typescript title="WebAssembly"
import { listTokenizerBackends } from "@xberg-io/xberg-wasm";
function main() {
  const result = listTokenizerBackends();
  console.log(result);
}

void main();

```
