---
id: fixture_kotlin_android_post_processors_clear
language: kotlin
target: kotlin_android
level: typecheck
requires: []
side_effect: safe
---

Clear all post-processors and verify list is empty

```kotlin title="Kotlin (Android)"
import io.xberg.*

fun main() {
    PostProcessorBridge.clearAll()
}

```
