---
id: fixture_kotlin_android_smoke_txt_basic
language: kotlin
target: kotlin_android
level: typecheck
requires: []
side_effect: server
---

Smoke test: Plain text file

```kotlin title="Kotlin (Android)"
import io.xberg.*
import com.fasterxml.jackson.module.kotlin.jacksonObjectMapper

fun main() = kotlinx.coroutines.runBlocking {
    val mapper = jacksonObjectMapper()
    val input = mapper.readValue("{\"kind\":\"uri\",\"mime_type\":\"text/plain\",\"uri\":\"https://example.com/text/report.txt\"}", ExtractInput::class.java)
    val config = mapper.readValue("{}", ExtractionConfig::class.java)
    val result = Xberg.extract(input, config)
}

```
