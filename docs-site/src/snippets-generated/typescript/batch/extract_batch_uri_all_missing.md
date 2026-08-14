---
id: fixture_node_extract_batch_uri_all_missing
language: typescript
target: node
level: typecheck
requires: []
side_effect: safe
---

extract_batch with missing URI inputs

```typescript title="TypeScript"
import { extractBatch } from "@xberg-io/xberg";
async function main() {
  const result = await extractBatch([{ kind: "uri", uri: "/nonexistent/a.pdf" }, { kind: "uri", uri: "/nonexistent/b.txt" }], undefined);
  console.log(result);
}

void main();

```
