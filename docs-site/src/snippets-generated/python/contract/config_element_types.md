---
id: fixture_python_config_element_types
language: python
target: python
level: typecheck
requires: []
side_effect: server
---

Tests element-based result format with element type assertions on DOCX

```python title="Python"
import asyncio
from xberg import extract, ExtractInput, ExtractionConfig, ExtractInputKind, ResultFormat

async def main() -> None:
    input = ExtractInput(kind=ExtractInputKind("uri"), uri="https://example.com/docx/unit_test_headers.docx")
    config = ExtractionConfig(result_format=ResultFormat("element_based"))
    result = await extract(input, config)
    for element in result.results[0].elements:
        print(element.element_type)
        print(element.content)

asyncio.run(main())

```
