---
id: fixture_kotlin_android_extract_batch_bytes_size_cap
language: kotlin
target: kotlin_android
level: typecheck
requires: []
side_effect: safe
---

extract_batch: archive size cap triggers error

```kotlin title="Kotlin (Android)"
import io.xberg.*
import com.fasterxml.jackson.module.kotlin.jacksonObjectMapper

fun main() = kotlinx.coroutines.runBlocking {
    val mapper = jacksonObjectMapper()
    try {
    val config = mapper.readValue("{\"security_limits\":{\"max_content_size\":1}}", ExtractionConfig::class.java)
    val result = Xberg.extractBatch(listOf(MAPPER.readValue("{\"bytes\":\"test_documents/text/fake_text.txt\",\"kind\":\"bytes\",\"mime_type\":\"text/plain\"}", ExtractInput::class.java)), config)
    } catch (error: Exception) {
        System.err.println("${error::class.simpleName}: ${error.message}")
    }
}

```
