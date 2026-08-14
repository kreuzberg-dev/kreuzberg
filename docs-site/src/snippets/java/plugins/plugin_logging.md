```java title="Java"
import io.xberg.ExtractedDocument;
import io.xberg.ExtractionConfig;
import io.xberg.IPostProcessor;
import java.util.logging.Logger;

class MyPlugin implements IPostProcessor {
    private static final Logger logger = Logger.getLogger(MyPlugin.class.getName());

    @Override
    public String name() {
        return "my-plugin";
    }

    @Override
    public String version() {
        return "1.0.0";
    }

    @Override
    public void process(ExtractedDocument result, ExtractionConfig config) {
        logger.info("Processing " + result.mimeType() +
            " (" + result.content().length() + " bytes)");

        // Processing...

        if (result.content().isEmpty()) {
            logger.warning("Processing resulted in empty content");
        }
    }

    @Override
    public String processing_stage() {
        return "my-plugin";
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
}
```
