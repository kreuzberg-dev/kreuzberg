---
id: fixture_kotlin_android_summarization_abstractive_smoke
language: kotlin
target: kotlin_android
level: typecheck
requires: []
side_effect: server
---

LLM-driven abstractive summary. Skipped automatically when XBERG_LLM_API_KEY (or OPENAI_API_KEY) is not set.

```kotlin title="Kotlin (Android)"
import io.xberg.*
import com.fasterxml.jackson.module.kotlin.jacksonObjectMapper

fun main() = kotlinx.coroutines.runBlocking {
    val mapper = jacksonObjectMapper()
    val input = mapper.readValue("{\"kind\":\"uri\",\"uri\":\"https://example.com/text/book_war_and_peace_1p.txt\"}", ExtractionConfig::class.java)
    val config = mapper.readValue("{\"summarization\":{\"llm\":{\"max_tokens\":200,\"model\":\"openai/gpt-4o-mini\",\"temperature\":0.0},\"max_tokens\":150,\"strategy\":\"abstractive\"}}", ExtractionConfig::class.java)
    val result = Xberg.extract(input, config)
}

```
