---
id: fixture_python_summarization_extractive_smoke
language: python
target: python
level: typecheck
requires: []
side_effect: server
---

TextRank extractive summary over a multi-paragraph plain text document. Pure-Rust, deterministic, no external services required.

```python title="Python"
import asyncio
from xberg import extract, ExtractInput, ExtractionConfig, ExtractInputKind

async def main() -> None:
    input = ExtractInput(kind=ExtractInputKind("uri"), uri="https://example.com/text/book_war_and_peace_1p.txt")
    config = ExtractionConfig(summarization={"max_tokens": 80, "strategy": "extractive"})
    result = await extract(input, config)
    print(result.results[0].summary)

asyncio.run(main())

```
