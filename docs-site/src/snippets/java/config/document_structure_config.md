```java title="Document Structure Config (Java)"
import io.xberg.Xberg;
import io.xberg.ExtractInputKind;
import io.xberg.ExtractionConfig;
import io.xberg.ExtractedDocument;

ExtractionConfig config = ExtractionConfig.builder()
    .withIncludeDocumentStructure(true)
    .build();
var resultOutput = Xberg.extract(
    io.xberg.ExtractInput.builder()
        .withKind(io.xberg.ExtractInputKind.Uri)
        .withUri("document.pdf")
        .build(),
    config
);
ExtractedDocument result = resultOutput.results().get(0);
if (result.document() != null) {
    var document = result.document();
    for (var node : document.nodes()) {
        System.out.println("[" + node.content().getClass().getSimpleName() + "]");
    }
}
```
