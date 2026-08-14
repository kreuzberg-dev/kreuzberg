---
id: fixture_kotlin_android_config_security_limits
language: kotlin
target: kotlin_android
level: typecheck
requires: []
side_effect: server
---

Tests archive extraction with custom security limits

```kotlin title="Kotlin (Android)"
import io.xberg.*
import com.fasterxml.jackson.module.kotlin.jacksonObjectMapper

fun main() = kotlinx.coroutines.runBlocking {
    val mapper = jacksonObjectMapper()
    val input = mapper.readValue("{\"kind\":\"uri\",\"uri\":\"https://example.com/archives/documents.zip\"}", ExtractInput::class.java)
    val config = mapper.readValue("{\"security_limits\":{\"max_archive_size\":104857600,\"max_compression_ratio\":50,\"max_files_in_archive\":100}}", ExtractionConfig::class.java)
    val result = Xberg.extract(input, config)
}

```
