---
id: fixture_node_config_document_structure_with_headings
language: typescript
target: node
level: typecheck
requires: []
side_effect: server
---

Tests document structure with DOCX heading-driven nesting

```typescript title="TypeScript"
import { ExtractInput, ExtractInputKind, ExtractionConfig, extract } from "@xberg-io/xberg";
async function main() {
  const input: ExtractInput = { kind: ExtractInputKind.Uri, uri: "https://example.com/docx/fake.docx" };
  const config: ExtractionConfig = { includeDocumentStructure: true };
  const result = await extract(input, config);
  console.log(result.results[0].documentStructure);
}

void main();

```
