---
id: fixture_java_ocr_image_png
language: java
target: java
level: typecheck
requires: []
side_effect: safe
---

OCR: PNG image extraction with OCR enabled. In WASM this exercises the Uint8Array bridge parameter and Promise await in the generated OcrBackend bridge.

```java title="Java"
import io.xberg.*;

public final class Example {
    public static void main(String[] args) throws Exception {
        var inputFile0 = java.util.Base64.getEncoder().encodeToString(
    java.nio.file.Files.readAllBytes(java.nio.file.Path.of("test_documents/images/test_hello_world.png"))
);
var inputJson = "{\"bytes\":\"__ALEF_DOC_FILE_0__\",\"config\":{},\"filename\":\"test_hello_world.png\",\"kind\":\"bytes\",\"mime_type\":\"image/png\"}";
inputJson = inputJson.replace("__ALEF_DOC_FILE_0__", inputFile0);
var input = JsonUtil.fromJson(inputJson, ExtractInput.class);
        var configJson = "{}";
var config = JsonUtil.fromJson(configJson, ExtractionConfig.class);
        var result = Xberg.extract(input, config);
        System.out.println(result.results().get(0).content());
    }
}

```
