---
id: fixture_node_config_chunking_prepend_heading_context
language: typescript
target: node
level: typecheck
requires: []
side_effect: server
---

Tests markdown chunker records heading hierarchy on chunk metadata

```typescript title="TypeScript"
import { ChunkerType, ExtractInput, ExtractInputKind, ExtractionConfig, extract } from "@xberg-io/xberg";
async function main() {
  const input: ExtractInput = { kind: ExtractInputKind.Uri, uri: "document.md" };
  const config: ExtractionConfig = { chunking: { chunkerType: ChunkerType.Markdown, maxCharacters: 500, overlap: 50, prependHeadingContext: true } };
  const result = await extract(input, config);
  const [first] = result.results ?? [];
  for (const chunk of first?.chunks ?? []) {
    console.log(chunk.content);
    console.log(chunk.metadata);
  }
}

void main();

```
