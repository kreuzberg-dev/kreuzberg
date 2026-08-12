---
id: fixture_java_post_processors_list
language: java
target: java
level: typecheck
requires: []
side_effect: safe
---

List all registered post-processors

```java title="Java"
import io.xberg.*;

public final class Example {
    public static void main(String[] args) throws Exception {
        var result = Xberg.listPostProcessors();
        System.out.println(result);
    }
}

```
