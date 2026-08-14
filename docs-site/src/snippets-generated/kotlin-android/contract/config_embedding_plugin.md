---
id: fixture_kotlin_android_config_embedding_plugin
language: kotlin
target: kotlin_android
level: typecheck
requires: []
side_effect: server
---

Tests EmbeddingModelType::Plugin variant deserialization in ChunkingConfig — config accepts the plugin variant shape; actual dispatch requires a host-language backend registered via register_embedding_backend at runtime

```kotlin title="Kotlin (Android)"
import io.xberg.*
import com.fasterxml.jackson.module.kotlin.jacksonObjectMapper

fun main() = kotlinx.coroutines.runBlocking {
    val mapper = jacksonObjectMapper()
    val input = mapper.readValue("{\"kind\":\"uri\",\"uri\":\"https://example.com/pdf/fake_memo.pdf\"}", ExtractInput::class.java)
    val config = mapper.readValue("{\"chunking\":{\"embedding\":{\"max_embed_duration_secs\":30,\"model\":{\"name\":\"test-plugin-backend\",\"type\":\"plugin\"},\"normalize\":true},\"max_chars\":500,\"max_overlap\":50}}", ExtractionConfig::class.java)
    val result = Xberg.extract(input, config)
}

```
