---
id: fixture_java_smoke_xlsx_basic
language: java
target: java
level: typecheck
requires: []
side_effect: server
---

Smoke test: XLSX with basic spreadsheet data including tables

```java title="Java"
import io.xberg.*;

public final class Example {
    public static void main(String[] args) throws Exception {
        var inputJson = "{\"kind\":\"uri\",\"mime_type\":\"application/vnd.openxmlformats-officedocument.spreadsheetml.sheet\",\"uri\":\"https://example.com/xlsx/stanley_cups.xlsx\"}";
var input = JsonUtil.fromJson(inputJson, ExtractInput.class);
        var configJson = "{}";
var config = JsonUtil.fromJson(configJson, ExtractionConfig.class);
        var result = Xberg.extract(input, config);
        System.out.println(result);
    }
}

```
