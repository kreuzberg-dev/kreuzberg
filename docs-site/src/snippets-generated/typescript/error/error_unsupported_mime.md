---
id: fixture_node_error_unsupported_mime
language: typescript
target: node
level: typecheck
requires: []
side_effect: safe
---

Error when extracting with unsupported MIME type

```typescript title="TypeScript"
import { ExtractInput, ExtractInputKind, XbergError, extract } from "@xberg-io/xberg";
async function main() {
  const input: ExtractInput = { bytes: await (await import("node:fs/promises")).readFile("test_documents/text/plain.txt"), config: {  }, filename: "plain.txt", kind: ExtractInputKind.Bytes, mimeType: "application/x-nonexistent" };
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
