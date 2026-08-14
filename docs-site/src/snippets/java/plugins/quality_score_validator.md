```java title="Java"
import io.xberg.ExtractedDocument;
import io.xberg.ExtractionConfig;
import io.xberg.IValidator;
import io.xberg.ValidationException;

IValidator qualityValidator = new IValidator() {
    @Override
    public String name() {
        return "quality-score";
    }

    @Override
    public String version() {
        return "1.0.0";
    }

    @Override
    public void validate(ExtractedDocument result, ExtractionConfig config) throws Exception {
        double score = result.qualityScore() != null ? result.qualityScore() : 0.0;

        if (score < 0.5) {
            throw new ValidationException(
                String.format("Quality score too low: %.2f < 0.50", score)
            );
        }
    }

    @Override
    public boolean should_validate(ExtractedDocument _result, ExtractionConfig _config) throws Exception {
        return true;
    }

    @Override
    public int priority() throws Exception {
        return 50;
    }
};
```
