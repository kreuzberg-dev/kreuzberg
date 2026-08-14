---
id: fixture_node_extract_batch_bytes_mixed_format
language: typescript
target: node
level: typecheck
requires: []
side_effect: safe
---

extract_batch: handles unsupported MIME gracefully

```typescript title="TypeScript"
import { extractBatch } from "@xberg-io/xberg";
async function main() {
  const result = await extractBatch([{ bytes: [80, 68, 70, 32, 112, 108, 97, 99, 101, 104, 111, 108, 100, 101, 114], kind: "bytes", mimeType: "application/x-unknown" }], undefined);
  console.log(result);
}

void main();

```
