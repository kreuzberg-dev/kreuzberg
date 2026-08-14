---
id: fixture_python_config_tree_sitter
language: python
target: python
level: typecheck
requires: []
side_effect: server
---

Tests tree-sitter configuration round-trip

```python title="Python"
import asyncio
from xberg import extract, ExtractInput, ExtractionConfig, ExtractInputKind

async def main() -> None:
    input = ExtractInput(kind=ExtractInputKind("uri"), uri="https://example.com/code/hello.py")
    config = ExtractionConfig(tree_sitter={"groups": ["web"], "languages": ["python", "rust"], "process": {"comments": False, "diagnostics": False, "docstrings": False, "exports": True, "imports": True, "structure": True, "symbols": False}})
    _ = await extract(input, config)
    print(result)

asyncio.run(main())

```
