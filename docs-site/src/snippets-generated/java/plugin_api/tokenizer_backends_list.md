---
id: fixture_java_tokenizer_backends_list
language: java
target: java
level: typecheck
requires: []
side_effect: safe
---

List all registered tokenizer backends

```java title="Java"
import io.xberg.*;

public final class Example {
    public static void main(String[] args) throws Exception {
        var result = Xberg.listTokenizerBackends();
        System.out.println(result);
    }
}

```
