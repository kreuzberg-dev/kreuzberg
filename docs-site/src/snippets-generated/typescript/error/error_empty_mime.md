---
id: fixture_node_error_empty_mime
language: typescript
target: node
level: typecheck
requires: []
side_effect: safe
---

Show how an empty MIME type is rejected consistently.

```typescript title="TypeScript"
import { ExtractInput, ExtractInputKind, XbergError, extract } from "@xberg-io/xberg";
async function main() {
  const input: ExtractInput = { bytes: await (await import("node:fs/promises")).readFile("test_documents/text/plain.txt"), config: {  }, filename: "plain.txt", kind: ExtractInputKind.Bytes, mimeType: "" };
  try {
    await extract(input, undefined);
  } catch (error) {
    if (error instanceof XbergError) {
      console.error(`${error.name}: ${error.message}`);
    }
  }
}

void main();

```
