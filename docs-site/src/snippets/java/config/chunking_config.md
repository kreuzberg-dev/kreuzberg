```java title="Java"
import io.xberg.ExtractionConfig;
import io.xberg.ExtractInputKind;
import io.xberg.ExtractionResult;
import io.xberg.ExtractedDocument;
import io.xberg.ChunkingConfig;

ExtractionConfig config = ExtractionConfig.builder()
    .withChunking(ChunkingConfig.builder()
        .withMaxCharacters(1000L)
        .withOverlap(200L)
        .build())
    .build();
```

```java title="Java - Markdown with Heading Context"
import io.xberg.Xberg;
import io.xberg.ExtractInput;
import io.xberg.ExtractInputKind;
import io.xberg.ExtractionConfig;
import io.xberg.ExtractionResult;
import io.xberg.ExtractedDocument;
import io.xberg.ChunkingConfig;
import io.xberg.ChunkerType;
import io.xberg.ChunkSizing;
import io.xberg.HeadingContext;
import java.util.Optional;

ExtractionConfig config = ExtractionConfig.builder()
    .withChunking(ChunkingConfig.builder()
        .withChunkerType(ChunkerType.Markdown)
        .withMaxCharacters(500L)
        .withOverlap(50L)
        .withSizing(new ChunkSizing.Tokenizer("Xenova/gpt-4o", Optional.empty()))
        .build())
    .build();
ExtractionResult output = Xberg.extract(
    ExtractInput.builder().withKind(ExtractInputKind.Uri).withUri("document.md").build(),
    config
);
ExtractedDocument result = output.results().get(0);
result.chunks().forEach(chunk -> {
    HeadingContext headingContext = chunk.metadata().headingContext();
    if (headingContext != null) {
        System.out.println("Headings:");
        headingContext.headings().forEach(heading ->
            System.out.println("  Level " + heading.level() + ": " + heading.text())
        );
    }
});
```

```java title="Java - Prepend Heading Context"
import io.xberg.Xberg;
import io.xberg.ExtractInput;
import io.xberg.ExtractInputKind;
import io.xberg.ExtractionConfig;
import io.xberg.ExtractionResult;
import io.xberg.ExtractedDocument;
import io.xberg.ChunkingConfig;

ExtractionConfig config = ExtractionConfig.builder()
    .withChunking(ChunkingConfig.builder()
        .withPrependHeadingContext(true)
        .build())
    .build();
ExtractionResult output = Xberg.extract(
    ExtractInput.builder().withKind(ExtractInputKind.Uri).withUri("document.md").build(),
    config
);
ExtractedDocument result = output.results().get(0);
// Each chunk's content is prefixed with its heading breadcrumb
result.chunks().forEach(chunk ->
    System.out.println(chunk.content().substring(0, Math.min(100, chunk.content().length())))
);
```
