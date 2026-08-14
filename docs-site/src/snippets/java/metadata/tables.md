```java title="Java"
import io.xberg.Xberg;
import io.xberg.ExtractInputKind;
import io.xberg.ExtractionResult;
import io.xberg.ExtractedDocument;
import io.xberg.XbergRsException;
import io.xberg.ExtractInput;
import io.xberg.ExtractionConfig;
import io.xberg.Table;
import java.util.List;

public class Main {
    public static void main(String[] args) {
        try {
            ExtractionResult output = Xberg.extract(
                ExtractInput.builder().withKind(ExtractInputKind.Uri).withUri("document.pdf").build(),
                ExtractionConfig.builder().build()
            );
            ExtractedDocument result = output.results().get(0);
            for (Table table : result.tables()) {
                System.out.println("Table with " + table.cells().size() + " rows");
                System.out.println(table.markdown());
                for (List<String> row : table.cells()) {
                    System.out.println(row);
                }
            }
        } catch (XbergRsException e) {
            System.err.println("Extraction failed: " + e.getMessage());
        }
    }
}
```
