---
id: fixture_kotlin_android_config_pages
language: kotlin
target: kotlin_android
level: typecheck
requires: []
side_effect: server
---

Tests page extraction and page marker configuration

```kotlin title="Kotlin (Android)"
import io.xberg.*
import com.fasterxml.jackson.module.kotlin.jacksonObjectMapper

fun main() = kotlinx.coroutines.runBlocking {
    val mapper = jacksonObjectMapper()
    val input = mapper.readValue("{\"kind\":\"uri\",\"uri\":\"https://example.com/pdf/fake_memo.pdf\"}", ExtractInput::class.java)
    val config = mapper.readValue("{\"pages\":{\"extract_pages\":true,\"insert_page_markers\":true}}", ExtractionConfig::class.java)
    val result = Xberg.extract(input, config)
}

```
