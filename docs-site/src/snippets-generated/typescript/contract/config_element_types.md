---
id: fixture_node_config_element_types
language: typescript
target: node
level: typecheck
requires: []
side_effect: server
---

Tests element-based result format with element type assertions on DOCX

```typescript title="TypeScript"
import { ExtractInput, ExtractInputKind, ExtractionConfig, ResultFormat, extract } from "@xberg-io/xberg";
async function main() {
  const input: ExtractInput = { kind: ExtractInputKind.Uri, uri: "https://example.com/docx/unit_test_headers.docx" };
  const config: ExtractionConfig = { resultFormat: ResultFormat.ElementBased };
  const result = await extract(input, config);
  const [first] = result.results ?? [];
  for (const element of first?.elements) {
    console.log(element.elementType);
    console.log(element.content);
  }
}

void main();

```
