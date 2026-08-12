---
id: fixture_java_config_llm_structured_extraction
language: java
target: java
level: typecheck
requires: []
side_effect: server
---

Tests structured extraction via liter-llm with JSON schema

```java title="Java"
import io.xberg.*;

public final class Example {
    public static void main(String[] args) throws Exception {
        var inputJson = "{\"kind\":\"uri\",\"uri\":\"https://example.com/pdf/fake_memo.pdf\"}";
var input = JsonUtil.fromJson(inputJson, ExtractInput.class);
        var configJson = "{\"structured_extraction\":{\"llm\":{\"model\":\"openai/gpt-4o\"},\"schema\":{\"properties\":{\"date\":{\"type\":\"string\"},\"summary\":{\"type\":\"string\"},\"title\":{\"type\":\"string\"}},\"required\":[\"title\"],\"type\":\"object\"},\"schema_name\":\"memo_data\"}}";
var config = JsonUtil.fromJson(configJson, ExtractionConfig.class);
        var result = Xberg.extract(input, config);
        System.out.println(result.results().get(0).structuredData());
    }
}

```
