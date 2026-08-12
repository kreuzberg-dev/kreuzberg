---
id: fixture_kotlin_android_url_crawl_linked_pages
language: kotlin
target: kotlin_android
level: typecheck
requires: []
side_effect: server
---

extract: crawl mode follows linked pages

```kotlin title="Kotlin (Android)"
import io.xberg.*
import com.fasterxml.jackson.module.kotlin.jacksonObjectMapper

fun main() = kotlinx.coroutines.runBlocking {
    val mapper = jacksonObjectMapper()
    val input = mapper.readValue("{\"kind\":\"uri\",\"uri\":\"https://example.com\"}", ExtractionConfig::class.java)
    val config = mapper.readValue("{\"url\":{\"crawl\":{\"max_depth\":1,\"max_pages\":4,\"respect_robots_txt\":false},\"mode\":\"crawl\"}}", ExtractionConfig::class.java)
    val result = Xberg.extract(input, config)
}

```
