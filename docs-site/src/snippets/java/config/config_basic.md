```java title="Java"
import io.xberg.Xberg;
import io.xberg.ExtractInputKind;
import io.xberg.ExtractionResult;
import io.xberg.ExtractedDocument;
import io.xberg.ExtractionConfig;
import io.xberg.ExtractInput;

ExtractionConfig config = ExtractionConfig.builder()
    .withUseCache(true)
    .withEnableQualityProcessing(true)
    .build();
ExtractionResult output = Xberg.extract(
    ExtractInput.builder().withKind(ExtractInputKind.Uri).withUri("document.pdf").build(),
    config
);
ExtractedDocument result = output.results().get(0);
```
