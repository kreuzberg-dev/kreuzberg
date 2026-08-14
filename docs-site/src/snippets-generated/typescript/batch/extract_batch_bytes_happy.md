---
id: fixture_node_extract_batch_bytes_happy
language: typescript
target: node
level: typecheck
requires: []
side_effect: safe
---

Extract multiple in-memory documents in one batch.

```typescript title="TypeScript"
import { extractBatch } from "@xberg-io/xberg";
async function main() {
  const result = await extractBatch([{ bytes: [72, 101, 108, 108, 111, 44, 32, 119, 111, 114, 108, 100, 33], kind: "bytes", mimeType: "text/plain" }, { bytes: "test_documents/html/html.html", kind: "bytes", mimeType: "text/html" }], undefined);
  console.log(result);
}

void main();

```
