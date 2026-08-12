---
id: fixture_node_config_llm_structured_extraction
language: typescript
target: node
level: typecheck
requires: []
side_effect: server
---

Tests structured extraction via liter-llm with JSON schema

```typescript title="TypeScript"
import { ExtractInput, ExtractInputKind, ExtractionConfig, WhisperModel, extract } from "@xberg-io/xberg";
async function main() {
  const input: ExtractInput = { kind: ExtractInputKind.Uri, uri: "https://example.com/pdf/fake_memo.pdf" };
  const config: ExtractionConfig = { structuredExtraction: { llm: { model: WhisperModel.OpenaiGpt4o }, schema: { properties: { date: { type: "string" }, summary: { type: "string" }, title: { type: "string" } }, required: ["title"], type: "object" }, schemaName: "memo_data" } };
  const result = await extract(input, config);
  console.log(result.results[0].structuredData);
}

void main();

```
