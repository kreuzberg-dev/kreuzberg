```java title="Java"
import io.xberg.ExtractionConfig;
import io.xberg.TokenReductionOptions;

ExtractionConfig config = ExtractionConfig.builder()
    .withTokenReduction(TokenReductionOptions.builder()
        .withMode("moderate")
        .withPreserveImportantWords(true)
        .build())
    .build();
```
