---
id: fixture_csharp_register_tokenizer_backend_trait_bridge
language: csharp
target: csharp
level: typecheck
requires: []
side_effect: safe
---

register_tokenizer_backend: trait bridge

```csharp title="C#"
using Xberg;

XbergConverter.RegisterTokenizerBackend(TokenizerBackendBridge.Register(new TestStub_RegisterTokenizerBackendTraitBridge()));
    private class TestStub_RegisterTokenizerBackendTraitBridge : ITokenizerBackend
    {
        public string Name => "register_tokenizer_backend_trait_bridge";
        public string Version => "1.0.0";

        public ulong CountTokens(string text)
            => 3;
        public void Initialize() { }
        public void Shutdown() { }
        public string Description { get; } = "";
        public string Author { get; } = "";
    }

```
