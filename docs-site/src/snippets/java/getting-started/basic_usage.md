```java title="Java"
import io.xberg.ExtractInput;
import io.xberg.ExtractInputKind;
import io.xberg.ExtractedDocument;
import io.xberg.ExtractionConfig;
import io.xberg.Xberg;
import io.xberg.ExtractionResult;
import io.xberg.XbergRsException;

public class BasicUsage {
    public static void main(String[] args) throws XbergRsException {
        ExtractInput input = ExtractInput.builder()
            .withKind(ExtractInputKind.Uri)
            .withUri("document.pdf")
            .build();

        ExtractionResult output = Xberg.extract(input, ExtractionConfig.builder().build());
        ExtractedDocument document = output.results().get(0);

        System.out.println("Content:");
        System.out.println(document.content());

        System.out.println("\nMetadata:");
        if (document.metadata().title() != null) {
            System.out.println("Title: " + document.metadata().title());
        }
        if (document.metadata().authors() != null) {
            System.out.println("Authors: " + String.join(", ", document.metadata().authors()));
        }

        System.out.println("\nTables found: " + document.tables().size());
        System.out.println("Images found: " + (document.images() == null ? 0 : document.images().size()));
    }
}
```
