---
id: fixture_kotlin_android_register_ocr_backend_trait_bridge
language: kotlin
target: kotlin_android
level: typecheck
requires: []
side_effect: safe
---

register_ocr_backend: trait bridge

```kotlin title="Kotlin (Android)"
import io.xberg.*

fun main() {
    class TestStubRegisterOcrBackendTraitBridge : IOcrBackend {
    override fun name(): String = "register_ocr_backend_trait_bridge"
    override suspend fun processImage(imageBytes: ByteArray, config: OcrConfig): ExtractedDocument = ExtractedDocument()
    override suspend fun processImageFile(path: String, config: OcrConfig): ExtractedDocument = ExtractedDocument()
    override fun supportsLanguage(lang: String): Boolean = false
    override fun backendType(): OcrBackendType = OcrBackendType.TESSERACT
    override fun supportedLanguages(): List<String> = emptyList()
    override fun supportsTableDetection(): Boolean = false
    override fun supportsDocumentProcessing(): Boolean = false
    override fun emitsStructuredMarkdown(): Boolean = false
    override suspend fun processDocument(path: String, config: OcrConfig): ExtractedDocument = ExtractedDocument()
    override fun version(): String = ""
    override fun initialize(): Unit {}
    override fun shutdown(): Unit {}
    override fun description(): String = ""
    override fun author(): String = ""
}
// register via: OcrBackendBridge.register(TestStubRegisterOcrBackendTraitBridge())

    OcrBackendBridge.register(TestStubRegisterOcrBackendTraitBridge())
}

```
