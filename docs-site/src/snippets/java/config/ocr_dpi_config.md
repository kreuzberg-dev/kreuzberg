```java title="Java"
import io.xberg.Xberg;
import io.xberg.ExtractInputKind;
import io.xberg.ExtractionResult;
import io.xberg.ExtractedDocument;
import io.xberg.ExtractionConfig;
import io.xberg.ExtractInput;
import io.xberg.OcrConfig;
import io.xberg.TesseractConfig;
import io.xberg.ImagePreprocessingConfig;

ExtractionConfig config = ExtractionConfig.builder()
    .withOcr(OcrConfig.builder()
        .withBackend("tesseract")
        .withTesseractConfig(TesseractConfig.builder()
            .withPreprocessing(ImagePreprocessingConfig.builder()
                .withTargetDpi(300)
                .build())
            .build())
        .build())
    .build();
ExtractionResult output = Xberg.extract(
    ExtractInput.builder().withKind(ExtractInputKind.Uri).withUri("scanned.pdf").build(),
    config
);
ExtractedDocument result = output.results().get(0);
```
