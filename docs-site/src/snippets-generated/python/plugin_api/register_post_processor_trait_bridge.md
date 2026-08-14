---
id: fixture_python_register_post_processor_trait_bridge
language: python
target: python
level: typecheck
requires: []
side_effect: safe
---

register_post_processor: trait bridge

```python title="Python"
from xberg import register_post_processor, unregister_post_processor, ExtractionConfig

def main() -> None:
    class _TestStub_register_post_processor_trait_bridge:
        def name(self):
            return "test-processor"
        def initialize(self):
            pass
        def shutdown(self):
            pass
        async def process(self, _p0, _p1):
            return None
        def processing_stage(self):
            return {}
    _ = register_post_processor(_TestStub_register_post_processor_trait_bridge())
    unregister_post_processor("test-processor")

main()

```
