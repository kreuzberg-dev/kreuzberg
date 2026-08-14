```java title="Java"
import io.xberg.ExtractedDocument;
import io.xberg.ExtractionConfig;
import io.xberg.IPostProcessor;

// Post-processors observe the extracted document (process() returns void);
// ExtractedDocument is an immutable record, so this hook can no longer
// inject new metadata fields the way older PostProcessor implementations could.
IPostProcessor pdfOnly = new IPostProcessor() {
    @Override
    public String name() {
        return "pdf-only";
    }

    @Override
    public String version() {
        return "1.0.0";
    }

    @Override
    public void process(ExtractedDocument result, ExtractionConfig config) throws Exception {
        if (!result.mimeType().equals("application/pdf")) {
            return;
        }
        // Handle PDF-specific processing here.
    }

    @Override
    public String processing_stage() throws Exception {
        return "pdf-only";
    }

    @Override
    public boolean should_process(ExtractedDocument _result, ExtractionConfig _config) throws Exception {
        return _result.mimeType().equals("application/pdf");
    }

    @Override
    public long estimated_duration_ms(ExtractedDocument _result) throws Exception {
        return 0;
    }

    @Override
    public int priority() throws Exception {
        return 50;
    }
};
```
