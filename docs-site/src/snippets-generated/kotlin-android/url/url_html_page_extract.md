---
id: fixture_kotlin_android_url_html_page_extract
language: kotlin
target: kotlin_android
level: typecheck
requires: []
side_effect: server
---

extract: website URL returns page content

```kotlin title="Kotlin (Android)"
import io.xberg.*
import com.fasterxml.jackson.module.kotlin.jacksonObjectMapper

fun main() = kotlinx.coroutines.runBlocking {
    val mapper = jacksonObjectMapper()
    val input = mapper.readValue("{\"kind\":\"uri\",\"uri\":\"https://example.com\"}", ExtractionConfig::class.java)
    val config = mapper.readValue("{\"url\":{\"mode\":\"document\"}}", ExtractionConfig::class.java)
    val result = Xberg.extract(input, config)
}

```
