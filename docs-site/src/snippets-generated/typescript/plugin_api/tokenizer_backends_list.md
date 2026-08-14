---
id: fixture_node_tokenizer_backends_list
language: typescript
target: node
level: typecheck
requires: []
side_effect: safe
---

List all registered tokenizer backends

```typescript title="TypeScript"
import { listTokenizerBackends } from "@xberg-io/xberg";
function main() {
  const result = listTokenizerBackends();
  console.log(result);
}

void main();

```
