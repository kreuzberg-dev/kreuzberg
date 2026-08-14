---
id: fixture_node_extract_bytes_input
language: typescript
target: node
level: typecheck
requires: []
side_effect: safe
---

extract bytes input from PDF document

```typescript title="TypeScript"
import { ExtractInput, ExtractInputKind, extract } from "@xberg-io/xberg";
async function main() {
  const input: ExtractInput = { bytes: await (await import("node:fs/promises")).readFile("test_documents/pdf/fake_memo.pdf"), filename: "fake_memo.pdf", kind: ExtractInputKind.Bytes, mimeType: "application/pdf" };
  const result = await extract(input, undefined);
  console.log(result);
}

void main();

```
