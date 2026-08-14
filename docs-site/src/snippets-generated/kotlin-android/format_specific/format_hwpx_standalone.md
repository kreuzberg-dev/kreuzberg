---
id: fixture_kotlin_android_format_hwpx_standalone
language: kotlin
target: kotlin_android
level: typecheck
requires: []
side_effect: server
---

Standalone HWPX extraction using extract

```kotlin title="Kotlin (Android)"
import io.xberg.*
import com.fasterxml.jackson.module.kotlin.jacksonObjectMapper

fun main() = kotlinx.coroutines.runBlocking {
    val mapper = jacksonObjectMapper()
    val input = mapper.readValue("{\"filename\":\"simple.hwpx\",\"kind\":\"uri\",\"mime_type\":\"application/haansofthwpx\",\"uri\":\"https://example.com/hwpx/simple.hwpx\"}", ExtractInput::class.java)
    val result = Xberg.extract(input, ExtractionConfig())
}

```
