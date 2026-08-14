---
id: fixture_kotlin_android_register_validator_trait_bridge
language: kotlin
target: kotlin_android
level: typecheck
requires: []
side_effect: safe
---

register_validator: trait bridge

```kotlin title="Kotlin (Android)"
import io.xberg.*

fun main() {
    class TestStubRegisterValidatorTraitBridge : IValidator {
    override fun name(): String = "register_validator_trait_bridge"
    override suspend fun validate(result: ExtractedDocument, config: ExtractionConfig): Unit {}
    override fun shouldValidate(result: ExtractedDocument, config: ExtractionConfig): Boolean = false
    override fun priority(): Int = 0
    override fun version(): String = ""
    override fun initialize(): Unit {}
    override fun shutdown(): Unit {}
    override fun description(): String = ""
    override fun author(): String = ""
}
// register via: ValidatorBridge.register(TestStubRegisterValidatorTraitBridge())

    ValidatorBridge.register(TestStubRegisterValidatorTraitBridge())
}

```
