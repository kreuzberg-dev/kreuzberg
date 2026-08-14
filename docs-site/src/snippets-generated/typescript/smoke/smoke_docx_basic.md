---
id: fixture_node_smoke_docx_basic
language: typescript
target: node
level: typecheck
requires: []
side_effect: server
---

Smoke test: DOCX with formatted text

```typescript title="TypeScript"
import { ExtractInput, ExtractInputKind, extract } from "@xberg-io/xberg";
async function main() {
  const input: ExtractInput = { kind: ExtractInputKind.Uri, mimeType: "application/vnd.openxmlformats-officedocument.wordprocessingml.document", uri: "https://example.com/docx/fake.docx" };
  const result = await extract(input, undefined);
  console.log(result);
}

void main();

```
