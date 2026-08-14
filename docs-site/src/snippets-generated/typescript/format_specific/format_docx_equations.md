---
id: fixture_node_format_docx_equations
language: typescript
target: node
level: typecheck
requires: []
side_effect: server
---

DOCX equations extract to LaTeX math in markdown output

```typescript title="TypeScript"
import { ExtractInput, ExtractInputKind, ExtractionConfig, OutputFormat, extract } from "@xberg-io/xberg";
async function main() {
  const input: ExtractInput = { filename: "equations.docx", kind: ExtractInputKind.Uri, mimeType: "application/vnd.openxmlformats-officedocument.wordprocessingml.document", uri: "https://example.com/docx/equations.docx" };
  const config: ExtractionConfig = { outputFormat: OutputFormat.Markdown };
  const result = await extract(input, config);
}

void main();

```
