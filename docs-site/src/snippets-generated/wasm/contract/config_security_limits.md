---
id: fixture_wasm_config_security_limits
language: typescript
target: wasm
level: typecheck
requires: []
side_effect: server
---

Tests archive extraction with custom security limits

```typescript title="WebAssembly"
import { ExtractInput, ExtractInputKind, extract } from "@xberg-io/xberg-wasm";
async function main() {
  const input: WasmExtractInput = (() => { const _u0 = WasmExtractInput.default(); _u0.kind = WasmExtractInputKind.Uri; _u0.uri = "https://example.com/archives/documents.zip"; return _u0; })();
  const result = await extract(input, { securityLimits: { maxArchiveSize: 104857600, maxCompressionRatio: 50, maxFilesInArchive: 100 } });
  console.log(result);
}

void main();

```
