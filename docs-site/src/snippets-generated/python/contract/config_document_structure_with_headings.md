---
id: fixture_python_config_document_structure_with_headings
language: python
target: python
level: typecheck
requires: []
side_effect: server
---

Tests document structure with DOCX heading-driven nesting

```python title="Python"
import asyncio
from xberg import extract, ExtractInput, ExtractionConfig, ExtractInputKind

async def main() -> None:
    input = ExtractInput(kind=ExtractInputKind("uri"), uri="https://example.com/docx/fake.docx")
    config = ExtractionConfig(include_document_structure=True)
    result = await extract(input, config)
    print(result.results[0].document_structure)

asyncio.run(main())

```
