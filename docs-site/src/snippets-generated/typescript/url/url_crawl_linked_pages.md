---
id: fixture_node_url_crawl_linked_pages
language: typescript
target: node
level: typecheck
requires: []
side_effect: server
---

extract: crawl mode follows linked pages

```typescript title="TypeScript"
import { ExtractInput, ExtractInputKind, ExtractionConfig, UrlExtractionMode, extract } from "@xberg-io/xberg";
async function main() {
  const input: ExtractInput = { kind: ExtractInputKind.Uri, uri: "https://example.com" };
  const config: ExtractionConfig = { url: { crawl: { maxDepth: 1, maxPages: 4, respectRobotsTxt: false }, mode: UrlExtractionMode.Crawl } };
  const result = await extract(input, config);
  console.log(result);
}

void main();

```
