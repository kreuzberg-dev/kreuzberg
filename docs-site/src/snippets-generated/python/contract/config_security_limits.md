---
id: fixture_python_config_security_limits
language: python
target: python
level: typecheck
requires: []
side_effect: server
---

Tests archive extraction with custom security limits

```python title="Python"
import asyncio
from xberg import extract, ExtractInput, ExtractionConfig, ExtractInputKind

async def main() -> None:
    input = ExtractInput(kind=ExtractInputKind("uri"), uri="https://example.com/archives/documents.zip")
    config = ExtractionConfig(security_limits={"max_archive_size": 104857600, "max_compression_ratio": 50, "max_files_in_archive": 100})
    await extract(input, config)
    print(result)

asyncio.run(main())

```
