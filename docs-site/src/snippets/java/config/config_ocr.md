```java title="Java"
import io.xberg.ExtractionConfig;
import io.xberg.OcrConfig;
import io.xberg.TesseractConfig;
import java.util.List;

ExtractionConfig config = ExtractionConfig.builder()
    .withOcr(OcrConfig.builder()
        .withBackend("tesseract")
        .withLanguage(List.of("eng", "fra"))
        .withTesseractConfig(TesseractConfig.builder()
            .withPsm(3)
            .build())
        .build())
    .build();
```
