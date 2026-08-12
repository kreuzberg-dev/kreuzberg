---
id: fixture_python_config_llm_structured_extraction
language: python
target: python
level: typecheck
requires: []
side_effect: server
---

Tests structured extraction via liter-llm with JSON schema

```python title="Python"
import asyncio

async def main() -> None:
    input = ExtractInput(kind=ExtractInputKind("uri"), uri="https://example.com/pdf/fake_memo.pdf")
    config = ExtractionConfig(structured_extraction={"llm": {"model": "openai/gpt-4o"}, "schema": {"properties": {"date": {"type": "string"}, "summary": {"type": "string"}, "title": {"type": "string"}}, "required": ["title"], "type": "object"}, "schema_name": "memo_data"})
    result = await extract(input, config)
    print(result.results[0].structured_data)

asyncio.run(main())

```
