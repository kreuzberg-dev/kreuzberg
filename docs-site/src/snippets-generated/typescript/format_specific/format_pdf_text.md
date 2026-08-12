---
id: fixture_node_format_pdf_text
language: typescript
target: node
level: typecheck
requires: []
side_effect: server
---

Standalone PDF text extraction using extract

```typescript title="TypeScript"
import { ExtractInput, ExtractInputKind, extract } from "@xberg-io/xberg";
async function main() {
  const input: ExtractInput = { filename: "fake_memo.pdf", kind: ExtractInputKind.Uri, mimeType: "application/pdf", uri: "https://example.com/pdf/fake_memo.pdf" };
  const result = await extract(input, undefined);
  console.log(result.results[0].metadata);
}

void main();

```
