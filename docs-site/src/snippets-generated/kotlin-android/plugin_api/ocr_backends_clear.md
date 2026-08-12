---
id: fixture_kotlin_android_ocr_backends_clear
language: kotlin
target: kotlin_android
level: typecheck
requires: []
side_effect: safe
---

Clear all OCR backends and verify list is empty

```kotlin title="Kotlin (Android)"
import io.xberg.*

fun main() {
    OcrBackendBridge.clearAll()
}

```
