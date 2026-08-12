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

async def main() -> None:
    try:
        inputs = [ExtractInput(bytes=Path("test_documents/text/fake_text.txt").read_bytes(), kind="bytes", mime_type="text/plain")]
        config = ExtractionConfig(security_limits={"max_content_size": 1})
        _ = await extract_batch(inputs, config)
    except Exception as error:
        print(f"Call failed as expected: {error}")
    else:
        raise AssertionError("expected call to fail")

asyncio.run(main())

```
