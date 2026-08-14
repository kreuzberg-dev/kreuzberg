```java title="Java"
import io.xberg.ExtractedDocument;
import io.xberg.ExtractionConfig;
import io.xberg.IPostProcessor;
import io.xberg.Metadata;
import org.junit.jupiter.api.Test;
import static org.junit.jupiter.api.Assertions.*;

class PostProcessorTest {
    // process() returns void and ExtractedDocument is immutable, so a test
    // double captures the observed result on a field instead of returning a
    // mutated copy.
    static class WordCountProcessor implements IPostProcessor {
        long lastWordCount;

        @Override
        public String name() {
            return "word-count";
        }

        @Override
        public String version() {
            return "1.0.0";
        }

        @Override
        public void process(ExtractedDocument result, ExtractionConfig config) {
            lastWordCount = result.content().split("\\s+").length;
        }

        @Override
        public String processing_stage() {
            return "word-count";
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

    @Test
    void testWordCountProcessor() throws Exception {
        WordCountProcessor processor = new WordCountProcessor();

        ExtractedDocument input = ExtractedDocument.builder()
            .withContent("Hello world test")
            .withMimeType("text/plain")
            .withMetadata(Metadata.builder().build())
            .build();

        processor.process(input, ExtractionConfig.builder().build());

        assertEquals(3, processor.lastWordCount);
    }
}
```
