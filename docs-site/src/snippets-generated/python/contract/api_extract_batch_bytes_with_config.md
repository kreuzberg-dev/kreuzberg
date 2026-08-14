---
id: fixture_python_api_extract_batch_bytes_with_config
language: python
target: python
level: typecheck
requires: []
side_effect: safe
---

Tests batch bytes extraction with per-input config (extract_batch)

```python title="Python"
import asyncio
from pathlib import Path
from xberg import extract_batch, ExtractInput, ExtractionConfig

async def main() -> None:
    inputs = [ExtractInput(bytes=Path("test_documents/pdf/fake_memo.pdf").read_bytes(), config={"output_format": "markdown"}, filename="fake_memo.pdf", kind="bytes")]
    await extract_batch(inputs, None)
    print(result)

asyncio.run(main())

```
