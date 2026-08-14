```java title="Java"
import io.xberg.Xberg;
import io.xberg.ExtractInputKind;
import io.xberg.ExtractionResult;
import io.xberg.ExtractedDocument;
import io.xberg.ExtractInput;
import io.xberg.ExtractionConfig;
import io.xberg.IValidator;
import io.xberg.ValidationException;
import io.xberg.XbergRsException;

public class MinLengthValidatorExample implements IValidator {
    private final int minLength;

    public MinLengthValidatorExample(int minLength) {
        this.minLength = minLength;
    }

    @Override
    public String name() {
        return "min-length";
    }

    @Override
    public String version() {
        return "1.0.0";
    }

    @Override
    public void validate(ExtractedDocument result, ExtractionConfig config) throws Exception {
        if (result.content().length() < minLength) {
            throw new ValidationException(
                "Content too short: " + result.content().length() +
                " < " + minLength
            );
        }
    }

    @Override
    public boolean should_validate(ExtractedDocument _result, ExtractionConfig _config) throws Exception {
        return true;
    }

    @Override
    public int priority() throws Exception {
        return 100;
    }

    public static void main(String[] args) {
        try {
            Xberg.registerValidator(new MinLengthValidatorExample(100));
            ExtractionResult output = Xberg.extract(
                ExtractInput.builder().withKind(ExtractInputKind.Uri).withUri("document.pdf").build(),
                ExtractionConfig.builder().build()
            );
            ExtractedDocument result = output.results().get(0);
            System.out.println("Validation passed!");
        } catch (XbergRsException e) {
            // A ValidationException thrown from validate() is reported here,
            // wrapped by the native bridge, once the plugin call crosses back into Java.
            System.err.println("Validation failed: " + e.getMessage());
        }
    }
}
```
