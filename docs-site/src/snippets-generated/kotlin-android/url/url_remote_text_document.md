---
id: fixture_kotlin_android_url_remote_text_document
language: kotlin
target: kotlin_android
level: typecheck
requires: []
side_effect: server
---

extract: remote text document URL

```kotlin title="Kotlin (Android)"
import io.xberg.*
import com.fasterxml.jackson.module.kotlin.jacksonObjectMapper

fun main() = kotlinx.coroutines.runBlocking {
    val mapper = jacksonObjectMapper()
    val input = mapper.readValue("{\"kind\":\"uri\",\"uri\":\"https://example.com\"}", ExtractInput::class.java)
    val config = mapper.readValue("{\"url\":{\"mode\":\"document\"}}", ExtractionConfig::class.java)
    val result = Xberg.extract(input, config)
}

```
