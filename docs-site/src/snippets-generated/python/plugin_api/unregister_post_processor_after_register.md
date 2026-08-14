---
id: fixture_python_unregister_post_processor_after_register
language: python
target: python
level: typecheck
requires: []
side_effect: safe
---

unregister_post_processor

```python title="Python"
from xberg import unregister_post_processor, ExtractionConfig

def main() -> None:
    name = "test-processor"
    _ = unregister_post_processor(name)

main()

```
