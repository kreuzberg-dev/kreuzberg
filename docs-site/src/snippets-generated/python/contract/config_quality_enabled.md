---
id: fixture_python_config_quality_enabled
language: python
target: python
level: typecheck
requires: []
side_effect: server
---

Tests quality scoring produces a score value in [0.0, 1.0]

```python title="Python"
import asyncio
from xberg import extract, ExtractInput, ExtractionConfig, ExtractInputKind

async def main() -> None:
    input = ExtractInput(kind=ExtractInputKind("uri"), uri="https://example.com/pdf/fake_memo.pdf")
    config = ExtractionConfig(enable_quality_processing=True)
    result = await extract(input, config)
    print(result.results[0].quality_score)

asyncio.run(main())

```
