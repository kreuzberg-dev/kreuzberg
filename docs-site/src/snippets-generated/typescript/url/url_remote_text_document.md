---
id: fixture_node_url_remote_text_document
language: typescript
target: node
level: typecheck
requires: []
side_effect: server
---

extract: remote text document URL

```typescript title="TypeScript"
import { ExtractInput, ExtractInputKind, ExtractionConfig, UrlExtractionMode, extract } from "@xberg-io/xberg";
async function main() {
  const input: ExtractInput = { kind: ExtractInputKind.Uri, uri: "https://example.com" };
  const config: ExtractionConfig = { url: { mode: UrlExtractionMode.Document } };
  const result = await extract(input, config);
  console.log(result);
}

void main();

```
