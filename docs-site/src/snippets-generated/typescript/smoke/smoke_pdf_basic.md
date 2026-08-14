---
id: fixture_node_smoke_pdf_basic
language: typescript
target: node
level: typecheck
requires: []
side_effect: server
---

Smoke test: PDF with simple text extraction

```typescript title="TypeScript"
import { ExtractInput, ExtractInputKind, extract } from "@xberg-io/xberg";
async function main() {
  const input: ExtractInput = { kind: ExtractInputKind.Uri, mimeType: "application/pdf", uri: "https://example.com/pdf/fake_memo.pdf" };
  const result = await extract(input, undefined);
  console.log(result);
}

void main();

```
