---
id: fixture_java_error_extract_input_conflicting_ocr
language: java
target: java
level: typecheck
requires: []
side_effect: safe
---

extract force+disable OCR

```java title="Java"
import io.xberg.*;

public final class Example {
    public static void main(String[] args) throws Exception {
        try {
        var inputFile0 = java.util.Base64.getEncoder().encodeToString(
    java.nio.file.Files.readAllBytes(java.nio.file.Path.of("test_documents/text/fake_text.txt"))
);
var inputJson = "{\"bytes\":\"__ALEF_DOC_FILE_0__\",\"config\":{\"disable_ocr\":true,\"force_ocr\":true},\"filename\":\"fake_text.txt\",\"kind\":\"bytes\",\"mime_type\":\"text/plain\"}";
inputJson = inputJson.replace("__ALEF_DOC_FILE_0__", inputFile0);
var input = JsonUtil.fromJson(inputJson, ExtractInput.class);
        var configJson = "{\"disable_ocr\":true,\"force_ocr\":true}";
var config = JsonUtil.fromJson(configJson, ExtractionConfig.class);
        var result = Xberg.extract(input, config);
        System.out.println(result);
        } catch (Exception error) {
            System.err.println("Call failed as expected: " + error.getMessage());
            return;
        }
        throw new AssertionError("expected call to fail");
    }
}

```
