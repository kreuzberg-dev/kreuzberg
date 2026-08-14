---
id: fixture_node_config_tree_sitter
language: typescript
target: node
level: typecheck
requires: []
side_effect: server
---

Tests tree-sitter configuration round-trip

```typescript title="TypeScript"
import { ExtractInput, ExtractInputKind, ExtractionConfig, extract } from "@xberg-io/xberg";
async function main() {
  const input: ExtractInput = { kind: ExtractInputKind.Uri, uri: "https://example.com/code/hello.py" };
  const config: ExtractionConfig = { treeSitter: { groups: ["web"], languages: ["python", "rust"], process: { comments: false, diagnostics: false, docstrings: false, exports: true, imports: true, structure: true, symbols: false } } };
  const result = await extract(input, config);
  console.log(result);
}

void main();

```
