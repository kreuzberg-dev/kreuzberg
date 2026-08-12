---
id: fixture_java_summarization_abstractive_smoke
language: java
target: java
level: typecheck
requires: []
side_effect: server
---

LLM-driven abstractive summary. Skipped automatically when XBERG_LLM_API_KEY (or OPENAI_API_KEY) is not set.

```java title="Java"
import io.xberg.*;

public final class Example {
    public static void main(String[] args) throws Exception {
        var inputJson = "{\"kind\":\"uri\",\"uri\":\"https://example.com/text/book_war_and_peace_1p.txt\"}";
var input = JsonUtil.fromJson(inputJson, ExtractInput.class);
        var configJson = "{\"summarization\":{\"llm\":{\"max_tokens\":200,\"model\":\"openai/gpt-4o-mini\",\"temperature\":0.0},\"max_tokens\":150,\"strategy\":\"abstractive\"}}";
var config = JsonUtil.fromJson(configJson, ExtractionConfig.class);
        var result = Xberg.extract(input, config);
        System.out.println(result.results().get(0).summary());
    }
}

```
