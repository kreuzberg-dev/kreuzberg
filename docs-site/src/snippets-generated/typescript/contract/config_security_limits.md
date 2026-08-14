---
id: fixture_node_config_security_limits
language: typescript
target: node
level: typecheck
requires: []
side_effect: server
---

Tests archive extraction with custom security limits

```typescript title="TypeScript"
import { ExtractInput, ExtractInputKind, ExtractionConfig, extract } from "@xberg-io/xberg";
async function main() {
  const input: ExtractInput = { kind: ExtractInputKind.Uri, uri: "https://example.com/archives/documents.zip" };
  const config: ExtractionConfig = { securityLimits: { maxArchiveSize: 104857600, maxCompressionRatio: 50, maxFilesInArchive: 100 } };
  const result = await extract(input, config);
  console.log(result);
}

void main();

```
