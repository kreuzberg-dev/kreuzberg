```java title="Java"
import io.xberg.Xberg;
import io.xberg.ExtractInputKind;
import io.xberg.ExtractionResult;
import io.xberg.ExtractedDocument;
import io.xberg.ExtractInput;
import io.xberg.ExtractionConfig;
import io.xberg.IPostProcessor;
import io.xberg.XbergRsException;
import java.util.concurrent.atomic.AtomicInteger;
import java.util.logging.Logger;

public class PdfMetadataExtractorExample implements IPostProcessor {
    private static final Logger logger = Logger.getLogger(
        PdfMetadataExtractorExample.class.getName()
    );
    private final AtomicInteger processedCount = new AtomicInteger(0);

    @Override
    public String name() {
        return "pdf-metadata-extractor";
    }

    @Override
    public String version() {
        return "1.0.0";
    }

    // Post-processors observe the extracted document; ExtractedDocument and
    // Metadata are immutable records, so this hook cannot inject new metadata
    // fields the way older PostProcessor implementations could.
    @Override
    public void process(ExtractedDocument result, ExtractionConfig config) throws Exception {
        if (!result.mimeType().equals("application/pdf")) {
            return;
        }
        processedCount.incrementAndGet();
        logger.info("Processed PDF: " + processedCount.get());
    }

    @Override
    public String processing_stage() throws Exception {
        return "pdf-metadata-extractor";
    }

    @Override
    public boolean should_process(ExtractedDocument _result, ExtractionConfig _config) throws Exception {
        return true;
    }

    @Override
    public long estimated_duration_ms(ExtractedDocument _result) throws Exception {
        return 0;
    }

    @Override
    public int priority() throws Exception {
        return 50;
    }

    public static void main(String[] args) {
        PdfMetadataExtractorExample pdfMetadata = new PdfMetadataExtractorExample();
        try {
            Xberg.registerPostProcessor(pdfMetadata);
            logger.info("PDF metadata extractor initialized");
            ExtractionResult output = Xberg.extract(
                ExtractInput.builder().withKind(ExtractInputKind.Uri).withUri("document.pdf").build(),
                ExtractionConfig.builder().build()
            );
            ExtractedDocument result = output.results().get(0);
            logger.info("Processed " + pdfMetadata.processedCount.get() + " PDFs");
        } catch (XbergRsException e) {
            e.printStackTrace();
        }
    }
}
```
