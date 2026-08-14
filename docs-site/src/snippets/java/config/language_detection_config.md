```java title="Java"
import io.xberg.ExtractionConfig;
import io.xberg.LanguageDetectionConfig;

ExtractionConfig config = ExtractionConfig.builder()
    .withLanguageDetection(LanguageDetectionConfig.builder()
        .withEnabled(true)
        .withMinConfidence(0.8)
        .build())
    .build();
```
