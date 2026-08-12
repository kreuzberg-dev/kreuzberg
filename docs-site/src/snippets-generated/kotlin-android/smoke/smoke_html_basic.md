---
id: fixture_kotlin_android_smoke_html_basic
language: kotlin
target: kotlin_android
level: typecheck
requires: []
side_effect: server
---

Smoke test: HTML table extraction

```kotlin title="Kotlin (Android)"
import io.xberg.*
import com.fasterxml.jackson.module.kotlin.jacksonObjectMapper

fun main() = kotlinx.coroutines.runBlocking {
    val mapper = jacksonObjectMapper()
    val input = mapper.readValue("{\"kind\":\"uri\",\"mime_type\":\"text/html\",\"uri\":\"https://example.com/html/simple_table.html\"}", ExtractionConfig::class.java)
    val config = mapper.readValue("{}", ExtractionConfig::class.java)
    val result = Xberg.extract(input, config)
}

```
