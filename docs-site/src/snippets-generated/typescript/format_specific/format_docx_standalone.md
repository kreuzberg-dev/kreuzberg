---
id: fixture_node_format_docx_standalone
language: typescript
target: node
level: typecheck
requires: []
side_effect: server
---

Standalone DOCX extraction using extract

```typescript title="TypeScript"
import { ExtractInput, ExtractInputKind, extract } from "@xberg-io/xberg";
async function main() {
  const input: ExtractInput = { filename: "fake.docx", kind: ExtractInputKind.Uri, mimeType: "application/vnd.openxmlformats-officedocument.wordprocessingml.document", uri: "https://example.com/docx/fake.docx" };
  const result = await extract(input, undefined);
  console.log(result);
}

void main();

```
