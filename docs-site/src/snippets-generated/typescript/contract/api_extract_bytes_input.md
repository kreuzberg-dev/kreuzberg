---
id: fixture_node_api_extract_bytes_input
language: typescript
target: node
level: typecheck
requires: []
side_effect: safe
---

Tests bytes input extraction API (extract)

```typescript title="TypeScript"
import { ExtractInput, ExtractInputKind, extract } from "@xberg-io/xberg";
async function main() {
  const input: ExtractInput = { bytes: await (await import("node:fs/promises")).readFile("test_documents/pdf/fake_memo.pdf"), filename: "fake_memo.pdf", kind: ExtractInputKind.Bytes };
  const result = await extract(input, undefined);
  console.log(result);
}

void main();

```
