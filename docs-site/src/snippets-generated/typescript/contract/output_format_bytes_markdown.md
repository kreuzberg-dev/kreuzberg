---
id: fixture_node_output_format_bytes_markdown
language: typescript
target: node
level: typecheck
requires: []
side_effect: safe
---

Tests markdown output format via bytes extraction API

```typescript title="TypeScript"
import { ExtractInput, ExtractInputKind, ExtractionConfig, OutputFormat, extract } from "@xberg-io/xberg";
async function main() {
  const input: ExtractInput = { bytes: await (await import("node:fs/promises")).readFile("test_documents/pdf/fake_memo.pdf"), config: { outputFormat: OutputFormat.Markdown }, filename: "fake_memo.pdf", kind: ExtractInputKind.Bytes, mimeType: "application/pdf" };
  const config: ExtractionConfig = { outputFormat: OutputFormat.Markdown };
  const result = await extract(input, config);
  console.log(result);
}

void main();

```
