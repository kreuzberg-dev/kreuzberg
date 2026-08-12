---
id: fixture_python_summarization_abstractive_smoke
language: python
target: python
level: typecheck
requires: []
side_effect: server
---

LLM-driven abstractive summary. Skipped automatically when XBERG_LLM_API_KEY (or OPENAI_API_KEY) is not set.

```python title="Python"
import asyncio

async def main() -> None:
    input = ExtractInput(kind=ExtractInputKind("uri"), uri="https://example.com/text/book_war_and_peace_1p.txt")
    config = ExtractionConfig(summarization={"llm": {"max_tokens": 200, "model": "openai/gpt-4o-mini", "temperature": 0.0}, "max_tokens": 150, "strategy": "abstractive"})
    result = await extract(input, config)
    print(result.results[0].summary)

asyncio.run(main())

```
