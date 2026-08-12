---
id: fixture_node_summarization_extractive_smoke
language: typescript
target: node
level: typecheck
requires: []
side_effect: server
---

TextRank extractive summary over a multi-paragraph plain text document. Pure-Rust, deterministic, no external services required.

```typescript title="TypeScript"
import { ExtractInput, ExtractInputKind, ExtractionConfig, SummaryStrategy, extract } from "@xberg-io/xberg";
async function main() {
  const input: ExtractInput = { kind: ExtractInputKind.Uri, uri: "https://example.com/text/book_war_and_peace_1p.txt" };
  const config: ExtractionConfig = { summarization: { maxTokens: 80, strategy: SummaryStrategy.Extractive } };
  const result = await extract(input, config);
  console.log(result.results[0].summary);
}

void main();

```
