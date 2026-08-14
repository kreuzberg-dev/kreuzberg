```java title="Java"
import io.xberg.Xberg;
import io.xberg.ExtractInputKind;
import io.xberg.ExtractionResult;
import io.xberg.ExtractedDocument;
import io.xberg.ExtractionConfig;
import io.xberg.ExtractInput;
import io.xberg.TokenReductionOptions;
import java.util.Map;

ExtractionConfig config = ExtractionConfig.builder()
    .withTokenReduction(TokenReductionOptions.builder()
        .withMode("moderate")
        .withPreserveImportantWords(true)
        .build())
    .build();
ExtractionResult output = Xberg.extract(
    ExtractInput.builder().withKind(ExtractInputKind.Uri).withUri("verbose_document.pdf").build(),
    config
);
ExtractedDocument result = output.results().get(0);
Map<String, Object> metadata = result.metadata() != null && result.metadata().additional() != null
    ? result.metadata().additional()
    : Map.of();
int original = metadata.containsKey("original_token_count")
    ? ((Number) metadata.get("original_token_count")).intValue()
    : 0;
int reduced = metadata.containsKey("token_count")
    ? ((Number) metadata.get("token_count")).intValue()
    : 0;
double ratio = metadata.containsKey("token_reduction_ratio")
    ? ((Number) metadata.get("token_reduction_ratio")).doubleValue()
    : 0.0;
System.out.println("Reduced from " + original + " to " + reduced + " tokens");
System.out.println(String.format("Reduction: %.1f%%", ratio * 100));
```
