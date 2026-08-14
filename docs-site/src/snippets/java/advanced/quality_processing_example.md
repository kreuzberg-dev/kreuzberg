```java title="Java"
import io.xberg.Xberg;
import io.xberg.ExtractInputKind;
import io.xberg.ExtractionResult;
import io.xberg.ExtractedDocument;
import io.xberg.ExtractionConfig;
import io.xberg.ExtractInput;
import java.util.Map;

ExtractionConfig config = ExtractionConfig.builder()
    .withEnableQualityProcessing(true)
    .build();
ExtractionResult output = Xberg.extract(
    ExtractInput.builder().withKind(ExtractInputKind.Uri).withUri("scanned_document.pdf").build(),
    config
);
ExtractedDocument result = output.results().get(0);
double qualityScore = result.qualityScore() != null ? result.qualityScore() : 0.0;
if (qualityScore < 0.5) {
    System.out.println(String.format("Warning: Low quality extraction (%.2f)", qualityScore));
    System.out.println("Consider re-scanning with higher DPI or adjusting OCR settings");
} else {
    System.out.println(String.format("Quality score: %.2f", qualityScore));
}
```
