---
id: fixture_node_extract_batch_uri_not_found
language: typescript
target: node
level: typecheck
requires: []
side_effect: safe
---

extract_batch with missing URI input

```typescript title="TypeScript"
import { extractBatch } from "@xberg-io/xberg";
async function main() {
  const result = await extractBatch([{ kind: "uri", uri: "/nonexistent/a.pdf" }], undefined);
  console.log(result);
}

void main();

```
