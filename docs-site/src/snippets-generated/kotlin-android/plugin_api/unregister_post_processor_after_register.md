---
id: fixture_kotlin_android_unregister_post_processor_after_register
language: kotlin
target: kotlin_android
level: typecheck
requires: []
side_effect: safe
---

unregister_post_processor

```kotlin title="Kotlin (Android)"
import io.xberg.*

fun main() {
    PostProcessorBridge.unregister("test-processor")
}

```
