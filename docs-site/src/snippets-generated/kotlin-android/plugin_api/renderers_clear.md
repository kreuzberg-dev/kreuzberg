---
id: fixture_kotlin_android_renderers_clear
language: kotlin
target: kotlin_android
level: typecheck
requires: []
side_effect: safe
---

Clear all renderers and verify list is empty

```kotlin title="Kotlin (Android)"
import io.xberg.*

fun main() {
    RendererBridge.clearAll()
}

```
