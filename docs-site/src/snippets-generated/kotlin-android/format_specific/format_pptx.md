---
id: fixture_kotlin_android_format_pptx
language: kotlin
target: kotlin_android
level: typecheck
requires: []
side_effect: server
---

PPTX presentation extraction using extract

```kotlin title="Kotlin (Android)"
import io.xberg.*
import com.fasterxml.jackson.module.kotlin.jacksonObjectMapper

fun main() = kotlinx.coroutines.runBlocking {
    val mapper = jacksonObjectMapper()
    val input = mapper.readValue("{\"kind\":\"uri\",\"mime_type\":\"application/vnd.openxmlformats-officedocument.presentationml.presentation\",\"uri\":\"https://example.com/pptx/simple.pptx\"}", ExtractionConfig::class.java)
    val result = Xberg.extract(input, ExtractionConfig())
}

```
