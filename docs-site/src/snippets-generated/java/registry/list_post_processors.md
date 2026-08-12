---
id: fixture_java_list_post_processors
language: java
target: java
level: typecheck
requires: []
side_effect: safe
---

List post-processors

```java title="Java"
import io.xberg.*;

public final class Example {
    public static void main(String[] args) throws Exception {
        var result = Xberg.listPostProcessors();
        System.out.println(result);
    }
}

```
