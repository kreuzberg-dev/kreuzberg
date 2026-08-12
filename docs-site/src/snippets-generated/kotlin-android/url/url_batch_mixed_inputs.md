---
id: fixture_kotlin_android_url_batch_mixed_inputs
language: kotlin
target: kotlin_android
level: typecheck
requires: []
side_effect: server
---

extract_batch: mixed bytes and URL inputs share one output envelope

```kotlin title="Kotlin (Android)"
import io.xberg.*
import com.fasterxml.jackson.module.kotlin.jacksonObjectMapper

fun main() = kotlinx.coroutines.runBlocking {
    val mapper = jacksonObjectMapper()
    val config = mapper.readValue("{\"url\":{\"mode\":\"document\"}}", ExtractionConfig::class.java)
    val result = Xberg.extractBatch(listOf(MAPPER.readValue("{\"kind\":\"uri\",\"uri\":\"https://example.com\"}", ExtractInput::class.java), MAPPER.readValue("{\"bytes\":[66,97,116,99,104,32,98,121,116,101,115,32,99,111,110,116,101,110,116],\"filename\":\"inline.txt\",\"kind\":\"bytes\",\"mime_type\":\"text/plain\"}", ExtractInput::class.java)), config)
}

```
