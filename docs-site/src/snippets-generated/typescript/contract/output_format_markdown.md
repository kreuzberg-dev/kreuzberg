---
id: fixture_node_output_format_markdown
language: typescript
target: node
level: typecheck
requires: []
side_effect: server
---

Tests Markdown output format

```typescript title="TypeScript"
import { ExtractInput, ExtractInputKind, ExtractionConfig, OutputFormat, extract } from "@xberg-io/xberg";
async function main() {
  const input: ExtractInput = { kind: ExtractInputKind.Uri, uri: "https://example.com/pdf/fake_memo.pdf" };
  const config: ExtractionConfig = { outputFormat: OutputFormat.Markdown };
  const result = await extract(input, config);
  console.log(result);
}

void main();

```
