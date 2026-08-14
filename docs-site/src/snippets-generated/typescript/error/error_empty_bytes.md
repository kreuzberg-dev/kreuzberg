---
id: fixture_node_error_empty_bytes
language: typescript
target: node
level: typecheck
requires: []
side_effect: safe
---

Graceful handling of empty bytes (should not error)

```typescript title="TypeScript"
import { ExtractInput, ExtractInputKind, extract } from "@xberg-io/xberg";
async function main() {
  const input: ExtractInput = { bytes: Uint8Array.from([]), config: {  }, filename: "empty.txt", kind: ExtractInputKind.Bytes, mimeType: "text/plain" };
  const result = await extract(input, undefined);
  console.log(result);
}

void main();

```
