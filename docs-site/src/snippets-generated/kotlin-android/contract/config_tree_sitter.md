---
id: fixture_kotlin_android_config_tree_sitter
language: kotlin
target: kotlin_android
level: typecheck
requires: []
side_effect: server
---

Tests tree-sitter configuration round-trip

```kotlin title="Kotlin (Android)"
import io.xberg.*
import com.fasterxml.jackson.module.kotlin.jacksonObjectMapper

fun main() = kotlinx.coroutines.runBlocking {
    val mapper = jacksonObjectMapper()
    val input = mapper.readValue("{\"kind\":\"uri\",\"uri\":\"https://example.com/code/hello.py\"}", ExtractionConfig::class.java)
    val config = mapper.readValue("{\"tree_sitter\":{\"groups\":[\"web\"],\"languages\":[\"python\",\"rust\"],\"process\":{\"comments\":false,\"diagnostics\":false,\"docstrings\":false,\"exports\":true,\"imports\":true,\"structure\":true,\"symbols\":false}}}", ExtractionConfig::class.java)
    val result = Xberg.extract(input, config)
}

```
