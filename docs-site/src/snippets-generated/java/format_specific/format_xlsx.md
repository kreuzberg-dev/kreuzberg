---
id: fixture_java_format_xlsx
language: java
target: java
level: typecheck
requires: []
side_effect: server
---

XLSX spreadsheet extraction using extract

```java title="Java"
import io.xberg.*;

public final class Example {
    public static void main(String[] args) throws Exception {
        var inputJson = "{\"kind\":\"uri\",\"mime_type\":\"application/vnd.openxmlformats-officedocument.spreadsheetml.sheet\",\"uri\":\"https://example.com/xlsx/stanley_cups.xlsx\"}";
var input = JsonUtil.fromJson(inputJson, ExtractInput.class);
        var result = Xberg.extract(input, ExtractionConfig.builder().build());
        System.out.println(result);
    }
}

```
