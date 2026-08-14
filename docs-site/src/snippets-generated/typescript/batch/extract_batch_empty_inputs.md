---
id: fixture_node_extract_batch_empty_inputs
language: typescript
target: node
level: typecheck
requires: []
side_effect: safe
---

extract_batch: empty batch

```typescript title="TypeScript"
import { extractBatch } from "@xberg-io/xberg";
async function main() {
  const result = await extractBatch([], undefined);
  console.log(result);
}

void main();

```
