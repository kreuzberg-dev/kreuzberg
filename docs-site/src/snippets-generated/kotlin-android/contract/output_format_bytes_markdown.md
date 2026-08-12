---
id: fixture_kotlin_android_output_format_bytes_markdown
language: kotlin
target: kotlin_android
level: typecheck
requires: []
side_effect: safe
---

Tests markdown output format via bytes extraction API

```kotlin title="Kotlin (Android)"
import io.xberg.*
import com.fasterxml.jackson.module.kotlin.jacksonObjectMapper

fun main() = kotlinx.coroutines.runBlocking {
    val mapper = jacksonObjectMapper()
    val inputFile0 = java.util.Base64.getEncoder().encodeToString(java.nio.file.Files.readAllBytes(java.nio.file.Path.of("test_documents/pdf/fake_memo.pdf")))
    val input = mapper.readValue("{\"bytes\":\"__ALEF_DOC_FILE_0__\",\"config\":{\"output_format\":\"markdown\"},\"filename\":\"fake_memo.pdf\",\"kind\":\"bytes\",\"mime_type\":\"application/pdf\"}".replace("__ALEF_DOC_FILE_0__", inputFile0), ExtractionConfig::class.java)
    val config = mapper.readValue("{\"output_format\":\"markdown\"}", ExtractionConfig::class.java)
    val result = Xberg.extract(input, config)
}

```
