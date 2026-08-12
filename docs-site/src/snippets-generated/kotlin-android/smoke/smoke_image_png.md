---
id: fixture_kotlin_android_smoke_image_png
language: kotlin
target: kotlin_android
level: typecheck
requires: []
side_effect: server
---

Smoke test: PNG image (without OCR, metadata only)

```kotlin title="Kotlin (Android)"
import io.xberg.*
import com.fasterxml.jackson.module.kotlin.jacksonObjectMapper

fun main() = kotlinx.coroutines.runBlocking {
    val mapper = jacksonObjectMapper()
    val input = mapper.readValue("{\"kind\":\"uri\",\"uri\":\"https://example.com/images/sample.png\"}", ExtractionConfig::class.java)
    val config = mapper.readValue("{\"disable_ocr\":true}", ExtractionConfig::class.java)
    val result = Xberg.extract(input, config)
}

```
