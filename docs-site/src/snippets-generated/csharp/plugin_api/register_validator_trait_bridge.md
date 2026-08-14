---
id: fixture_csharp_register_validator_trait_bridge
language: csharp
target: csharp
level: typecheck
requires: []
side_effect: safe
---

register_validator: trait bridge

```csharp title="C#"
using Xberg;

XbergConverter.RegisterValidator(ValidatorBridge.Register(new TestStub_RegisterValidatorTraitBridge()));
    private class TestStub_RegisterValidatorTraitBridge : IValidator
    {
        public string Name => "register_validator_trait_bridge";
        public string Version => "1.0.0";

        public void Validate(ExtractedDocument result, ExtractionConfig config) { }
        public bool ShouldValidate(ExtractedDocument result, ExtractionConfig config)
            => false;
        public int Priority { get; } = 0;
        public void Initialize() { }
        public void Shutdown() { }
        public string Description { get; } = "";
        public string Author { get; } = "";
    }

```
