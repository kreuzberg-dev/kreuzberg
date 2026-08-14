```java title="Java"
import io.xberg.ExtractedDocument;
import io.xberg.ExtractionConfig;
import io.xberg.IPostProcessor;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.atomic.AtomicInteger;

class StatefulPlugin implements IPostProcessor {
    // Use atomic types for simple counters
    private final AtomicInteger callCount = new AtomicInteger(0);

    // Use concurrent collections for complex state
    private final ConcurrentHashMap<String, String> cache = new ConcurrentHashMap<>();

    @Override
    public String name() {
        return "stateful-plugin";
    }

    @Override
    public String version() {
        return "1.0.0";
    }

    @Override
    public void process(ExtractedDocument result, ExtractionConfig config) {
        // Increment counter atomically
        callCount.incrementAndGet();

        // Update cache (thread-safe)
        cache.put("last_mime", result.mimeType());
    }

    @Override
    public String processing_stage() {
        return "stateful-plugin";
    }

    @Override
    public boolean should_process(ExtractedDocument result, ExtractionConfig config) {
        return true;
    }

    @Override
    public long estimated_duration_ms(ExtractedDocument result) {
        return 0;
    }

    @Override
    public int priority() {
        return 50;
    }

    public int getCallCount() {
        return callCount.get();
    }
}
```
