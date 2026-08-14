---
id: fixture_csharp_register_post_processor_trait_bridge
language: csharp
target: csharp
level: typecheck
requires: []
side_effect: safe
---

register_post_processor: trait bridge

```csharp title="C#"
using Xberg;

XbergConverter.RegisterPostProcessor(PostProcessorBridge.Register(new TestStub_RegisterPostProcessorTraitBridge()));
    private class TestStub_RegisterPostProcessorTraitBridge : IPostProcessor
    {
        public string Name => "register_post_processor_trait_bridge";
        public string Version => "1.0.0";

        public void Process(ExtractedDocument result, ExtractionConfig config) { }
        public ProcessingStage ProcessingStage { get; } = default(ProcessingStage);
        public bool ShouldProcess(ExtractedDocument result, ExtractionConfig config)
            => false;
        public ulong EstimatedDurationMs(ExtractedDocument result)
            => 0;
        public int Priority { get; } = 0;
        public void Initialize() { }
        public void Shutdown() { }
        public string Description { get; } = "";
        public string Author { get; } = "";
    }

```
