---
id: fixture_node_url_recursive_document_urls
language: typescript
target: node
level: typecheck
requires: []
side_effect: server
---

extract: recursive URL extraction follows document links discovered in results

```typescript title="TypeScript"
import { ExtractInput, ExtractInputKind, ExtractionConfig, UrlExtractionMode, extract } from "@xberg-io/xberg";
async function main() {
  const input: ExtractInput = { kind: ExtractInputKind.Uri, uri: "https://example.com" };
  const config: ExtractionConfig = { url: { crawl: { documentUrlDepth: 1, followDocumentUrls: true, respectRobotsTxt: false }, mode: UrlExtractionMode.Document } };
  const result = await extract(input, config);
  console.log(result);
}

void main();

```
