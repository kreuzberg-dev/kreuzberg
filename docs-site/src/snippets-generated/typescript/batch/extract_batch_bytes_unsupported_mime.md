---
id: fixture_node_extract_batch_bytes_unsupported_mime
language: typescript
target: node
level: typecheck
requires: []
side_effect: safe
---

extract_batch with unsupported bytes MIME type

```typescript title="TypeScript"
import { extractBatch } from "@xberg-io/xberg";
async function main() {
  const result = await extractBatch([{ bytes: [100, 97, 116, 97], kind: "bytes", mimeType: "application/x-unknown" }], undefined);
  console.log(result);
}

void main();

```
