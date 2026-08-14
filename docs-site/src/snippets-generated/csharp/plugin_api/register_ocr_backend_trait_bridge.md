---
id: fixture_csharp_register_ocr_backend_trait_bridge
language: csharp
target: csharp
level: typecheck
requires: []
side_effect: safe
---

register_ocr_backend: trait bridge

```csharp title="C#"
using Xberg;

XbergConverter.RegisterOcrBackend(OcrBackendBridge.Register(new TestStub_RegisterOcrBackendTraitBridge()));
    private class TestStub_RegisterOcrBackendTraitBridge : IOcrBackend
    {
        public string Name => "register_ocr_backend_trait_bridge";
        public string Version => "1.0.0";

        public ExtractedDocument ProcessImage(byte[] imageBytes, OcrConfig config)
            => default(ExtractedDocument);
        public ExtractedDocument ProcessImageFile(string path, OcrConfig config)
            => default(ExtractedDocument);
        public bool SupportsLanguage(string lang)
            => false;
        public OcrBackendType BackendType { get; } = default(OcrBackendType);
        public List<string> SupportedLanguages { get; } = [];
        public bool SupportsTableDetection { get; } = false;
        public bool SupportsDocumentProcessing { get; } = false;
        public bool EmitsStructuredMarkdown { get; } = false;
        public ExtractedDocument ProcessDocument(string path, OcrConfig config)
            => default(ExtractedDocument);
        public void Initialize() { }
        public void Shutdown() { }
        public string Description { get; } = "";
        public string Author { get; } = "";
    }

```
