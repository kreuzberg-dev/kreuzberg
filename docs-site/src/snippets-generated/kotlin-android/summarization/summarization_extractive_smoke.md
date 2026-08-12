---
id: fixture_kotlin_android_summarization_extractive_smoke
language: kotlin
target: kotlin_android
level: typecheck
requires: []
side_effect: server
---

TextRank extractive summary over a multi-paragraph plain text document. Pure-Rust, deterministic, no external services required.

```kotlin title="Kotlin (Android)"
import io.xberg.*
import com.fasterxml.jackson.module.kotlin.jacksonObjectMapper

fun main() = kotlinx.coroutines.runBlocking {
    val mapper = jacksonObjectMapper()
    val input = mapper.readValue("{\"kind\":\"uri\",\"uri\":\"https://example.com/text/book_war_and_peace_1p.txt\"}", ExtractionConfig::class.java)
    val config = mapper.readValue("{\"summarization\":{\"max_tokens\":80,\"strategy\":\"extractive\"}}", ExtractionConfig::class.java)
    val result = Xberg.extract(input, config)
}

```
