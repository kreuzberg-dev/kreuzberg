---
id: fixture_python_config_chunking_prepend_heading_context
language: python
target: python
level: typecheck
requires: []
side_effect: server
---

Tests markdown chunker records heading hierarchy on chunk metadata

```python title="Python"
import asyncio
from xberg import extract, ExtractInput, ExtractionConfig, ExtractInputKind

async def main() -> None:
    input = ExtractInput(kind=ExtractInputKind("uri"), uri="document.md")
    config = ExtractionConfig(chunking={"chunker_type": "markdown", "max_characters": 500, "overlap": 50, "prepend_heading_context": True})
    result = await extract(input, config)
    for chunk in result.results[0].chunks or []:
        print(chunk.content)
        print(chunk.metadata)

asyncio.run(main())

```
