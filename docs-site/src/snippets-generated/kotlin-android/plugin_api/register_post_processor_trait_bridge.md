---
id: fixture_kotlin_android_register_post_processor_trait_bridge
language: kotlin
target: kotlin_android
level: typecheck
requires: []
side_effect: safe
---

register_post_processor: trait bridge

```kotlin title="Kotlin (Android)"
import io.xberg.*

fun main() {
    class TestStubRegisterPostProcessorTraitBridge : IPostProcessor {
    override fun name(): String = "register_post_processor_trait_bridge"
    override suspend fun process(result: ExtractedDocument, config: ExtractionConfig): Unit {}
    override fun processingStage(): ProcessingStage = ProcessingStage.EARLY
    override fun shouldProcess(result: ExtractedDocument, config: ExtractionConfig): Boolean = false
    override fun estimatedDurationMs(result: ExtractedDocument): Long = 0
    override fun priority(): Int = 0
    override fun version(): String = ""
    override fun initialize(): Unit {}
    override fun shutdown(): Unit {}
    override fun description(): String = ""
    override fun author(): String = ""
}
// register via: PostProcessorBridge.register(TestStubRegisterPostProcessorTraitBridge())

    PostProcessorBridge.register(TestStubRegisterPostProcessorTraitBridge())
}

```
