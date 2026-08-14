---
id: fixture_node_format_pptx
language: typescript
target: node
level: typecheck
requires: []
side_effect: server
---

PPTX presentation extraction using extract

```typescript title="TypeScript"
import { ExtractInput, ExtractInputKind, extract } from "@xberg-io/xberg";
async function main() {
  const input: ExtractInput = { kind: ExtractInputKind.Uri, mimeType: "application/vnd.openxmlformats-officedocument.presentationml.presentation", uri: "https://example.com/pptx/simple.pptx" };
  const result = await extract(input, undefined);
  console.log(result);
}

void main();

```
