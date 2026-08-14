```java title="Java"
import io.xberg.Xberg;
import io.xberg.XbergRsException;

try {
    // Unregister specific plugins
    Xberg.unregisterPostProcessor("word-count");
    Xberg.unregisterValidator("min-length");
} catch (XbergRsException e) {
    System.err.println("Failed to unregister: " + e.getMessage());
}
```
