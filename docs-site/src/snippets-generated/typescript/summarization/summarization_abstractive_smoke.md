---
id: fixture_node_summarization_abstractive_smoke
language: typescript
target: node
level: typecheck
requires: []
side_effect: server
---

LLM-driven abstractive summary. Skipped automatically when XBERG_LLM_API_KEY (or OPENAI_API_KEY) is not set.

```typescript title="TypeScript"
import { ExtractInput, ExtractInputKind, ExtractionConfig, SummaryStrategy, WhisperModel, extract } from "@xberg-io/xberg";
async function main() {
  const input: ExtractInput = { kind: ExtractInputKind.Uri, uri: "https://example.com/text/book_war_and_peace_1p.txt" };
  const config: ExtractionConfig = { summarization: { llm: { maxTokens: 200, model: WhisperModel.OpenaiGpt4oMini, temperature: 0.0 }, maxTokens: 150, strategy: SummaryStrategy.Abstractive } };
  const result = await extract(input, config);
  console.log(result.results[0].summary);
}

void main();

```
