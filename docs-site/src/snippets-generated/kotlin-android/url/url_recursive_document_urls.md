---
id: fixture_kotlin_android_url_recursive_document_urls
language: kotlin
target: kotlin_android
level: typecheck
requires: []
side_effect: server
---

extract: recursive URL extraction follows document links discovered in results

```kotlin title="Kotlin (Android)"
import io.xberg.*
import com.fasterxml.jackson.module.kotlin.jacksonObjectMapper

fun main() = kotlinx.coroutines.runBlocking {
    val mapper = jacksonObjectMapper()
    val input = mapper.readValue("{\"kind\":\"uri\",\"uri\":\"https://example.com\"}", ExtractionConfig::class.java)
    val config = mapper.readValue("{\"url\":{\"crawl\":{\"document_url_depth\":1,\"follow_document_urls\":true,\"respect_robots_txt\":false},\"mode\":\"document\"}}", ExtractionConfig::class.java)
    val result = Xberg.extract(input, config)
}

```
