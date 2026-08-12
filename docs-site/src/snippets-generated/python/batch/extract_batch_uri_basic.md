---
id: fixture_python_extract_batch_uri_basic
language: python
target: python
level: typecheck
requires: []
side_effect: safe
---

extract_batch over URI inputs

```python title="Python"
import asyncio
from xberg import extract_batch, ExtractInput, ExtractionConfig

async def main() -> None:
    inputs = [ExtractInput(kind="uri", uri="pdf/fake_memo.pdf"), ExtractInput(kind="uri", uri="text/fake_text.txt")]
    result = await extract_batch(inputs, None)
    for result in result.results:
        print(result.content)

asyncio.run(main())

```
