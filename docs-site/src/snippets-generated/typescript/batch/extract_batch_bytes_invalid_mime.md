---
id: fixture_node_extract_batch_bytes_invalid_mime
language: typescript
target: node
level: typecheck
requires: []
side_effect: safe
---

extract_batch with invalid bytes MIME type

```typescript title="TypeScript"
import { extractBatch } from "@xberg-io/xberg";
async function main() {
  const result = await extractBatch([{ bytes: [72, 101, 108, 108, 111], kind: "bytes", mimeType: "application/x-nonexistent" }], undefined);
  console.log(result);
}

void main();

```
