---
id: fixture_kotlin_android_api_extract_bytes_input
language: kotlin
target: kotlin_android
level: typecheck
requires: []
side_effect: safe
---

Tests bytes input extraction API (extract)

```kotlin title="Kotlin (Android)"
import io.xberg.*
import com.fasterxml.jackson.module.kotlin.jacksonObjectMapper

fun main() = kotlinx.coroutines.runBlocking {
    val mapper = jacksonObjectMapper()
    val inputFile0 = java.util.Base64.getEncoder().encodeToString(java.nio.file.Files.readAllBytes(java.nio.file.Path.of("test_documents/pdf/fake_memo.pdf")))
    val input = mapper.readValue("{\"bytes\":\"__ALEF_DOC_FILE_0__\",\"filename\":\"fake_memo.pdf\",\"kind\":\"bytes\"}".replace("__ALEF_DOC_FILE_0__", inputFile0), ExtractInput::class.java)
    val result = Xberg.extract(input, ExtractionConfig())
}

```
