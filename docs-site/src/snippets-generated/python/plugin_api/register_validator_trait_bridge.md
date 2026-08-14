---
id: fixture_python_register_validator_trait_bridge
language: python
target: python
level: typecheck
requires: []
side_effect: safe
---

register_validator: trait bridge

```python title="Python"
from xberg import register_validator, unregister_validator, ExtractionConfig

def main() -> None:
    class _TestStub_register_validator_trait_bridge:
        def name(self):
            return "test-validator"
        def initialize(self):
            pass
        def shutdown(self):
            pass
        async def validate(self, _p0, _p1):
            return None
    register_validator(_TestStub_register_validator_trait_bridge())
    unregister_validator("test-validator")

main()

```
