---
id: fixture_python_extract_batch_bytes_size_cap
language: python
target: python
level: typecheck
requires: []
side_effect: safe
---

extract_batch: archive size cap triggers error

```python title="Python"
import asyncio
from pathlib import Path
from xberg import XbergError

async def main() -> None:
    try:
        inputs = [ExtractInput(bytes=Path("test_documents/text/fake_text.txt").read_bytes(), kind="bytes", mime_type="text/plain")]
        config = ExtractionConfig(security_limits={"max_content_size": 1})
        await extract_batch(inputs, config)
    except XbergError as error:
        print(f"{type(error).__name__}: {error}")

asyncio.run(main())

```
