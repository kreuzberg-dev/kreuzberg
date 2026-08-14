---
id: fixture_kotlin_android_error_extract_input_conflicting_ocr
language: kotlin
target: kotlin_android
level: typecheck
requires: []
side_effect: safe
---

extract force+disable OCR

```kotlin title="Kotlin (Android)"
import io.xberg.*
import com.fasterxml.jackson.module.kotlin.jacksonObjectMapper

fun main() = kotlinx.coroutines.runBlocking {
    val mapper = jacksonObjectMapper()
    try {
    val inputFile0 = java.util.Base64.getEncoder().encodeToString(java.nio.file.Files.readAllBytes(java.nio.file.Path.of("test_documents/text/fake_text.txt")))
    val input = mapper.readValue("{\"bytes\":\"__ALEF_DOC_FILE_0__\",\"config\":{\"disable_ocr\":true,\"force_ocr\":true},\"filename\":\"fake_text.txt\",\"kind\":\"bytes\",\"mime_type\":\"text/plain\"}".replace("__ALEF_DOC_FILE_0__", inputFile0), ExtractInput::class.java)
    val config = mapper.readValue("{\"disable_ocr\":true,\"force_ocr\":true}", ExtractionConfig::class.java)
    val result = Xberg.extract(input, config)
    } catch (error: Exception) {
        System.err.println("${error::class.simpleName}: ${error.message}")
    }
}

```
