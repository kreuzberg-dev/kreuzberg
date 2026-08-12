---
id: fixture_kotlin_android_ocr_backends_unregister
language: kotlin
target: kotlin_android
level: typecheck
requires: []
side_effect: safe
---

Unregister nonexistent OCR backend gracefully

```kotlin title="Kotlin (Android)"
import io.xberg.*

fun main() {
    OcrBackendBridge.unregister("nonexistent-backend-xyz")
}

```
