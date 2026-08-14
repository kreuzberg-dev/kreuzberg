```java title="Java"
import io.xberg.Xberg;
import io.xberg.ExtractInputKind;
import io.xberg.ExtractedDocument;
import io.xberg.Metadata;
import io.xberg.XbergRsException;
import java.util.Map;
import java.util.List;

public class Main {
    public static void main(String[] args) {
        try {
            var resultOutput = Xberg.extract(
                io.xberg.ExtractInput.builder()
                    .withKind(io.xberg.ExtractInputKind.Uri)
                    .withUri("document.pdf")
                    .build(),
                io.xberg.ExtractionConfig.builder().build()
            );
            ExtractedDocument result = resultOutput.results().get(0);
            // Metadata is flat — format-specific fields are at the top level
            Metadata metadata = result.metadata();
            if (metadata.title() != null) {
                System.out.println("Title: " + metadata.title());
            }
            if (metadata.authors() != null) {
                System.out.println("Authors: " + String.join(", ", metadata.authors()));
            }
            // Format-specific fields are in the additional map
            Map<String, Object> extra = metadata.additional();
            if (extra != null && extra.get("page_count") != null) {
                System.out.println("Pages: " + extra.get("page_count"));
            }
            // Access HTML metadata
            var htmlResultOutput = Xberg.extract(
                io.xberg.ExtractInput.builder()
                    .withKind(io.xberg.ExtractInputKind.Uri)
                    .withUri("page.html")
                    .build(),
                io.xberg.ExtractionConfig.builder().build()
            );
            ExtractedDocument htmlResult = htmlResultOutput.results().get(0);
            Metadata htmlMeta = htmlResult.metadata();
            if (htmlMeta.title() != null) {
                System.out.println("Title: " + htmlMeta.title());
            }
            Map<String, Object> htmlExtra = htmlMeta.additional();
            String description = htmlExtra != null ? (String) htmlExtra.get("description") : null;
            if (description != null) {
                System.out.println("Description: " + description);
            }
            // Access keywords as array
            if (htmlMeta.keywords() != null) {
                System.out.println("Keywords: " + htmlMeta.keywords());
            }
            // Access canonical URL (renamed from canonical)
            String canonicalUrl = htmlExtra != null ? (String) htmlExtra.get("canonical_url") : null;
            if (canonicalUrl != null) {
                System.out.println("Canonical URL: " + canonicalUrl);
            }
            // Access Open Graph fields from map
            @SuppressWarnings("unchecked")
            Map<String, String> openGraph = htmlExtra != null ? (Map<String, String>) htmlExtra.get("open_graph") : null;
            if (openGraph != null) {
                System.out.println("Open Graph Image: " + openGraph.get("image"));
                System.out.println("Open Graph Title: " + openGraph.get("title"));
                System.out.println("Open Graph Type: " + openGraph.get("type"));
            }
            // Access Twitter Card fields from map
            @SuppressWarnings("unchecked")
            Map<String, String> twitterCard = htmlExtra != null ? (Map<String, String>) htmlExtra.get("twitter_card") : null;
            if (twitterCard != null) {
                System.out.println("Twitter Card Type: " + twitterCard.get("card"));
                System.out.println("Twitter Creator: " + twitterCard.get("creator"));
            }
            // Access new fields
            if (htmlMeta.language() != null) {
                System.out.println("Language: " + htmlMeta.language());
            }
            String textDirection = htmlExtra != null ? (String) htmlExtra.get("text_direction") : null;
            if (textDirection != null) {
                System.out.println("Text Direction: " + textDirection);
            }
            // Access headers
            @SuppressWarnings("unchecked")
            List<Map<String, Object>> headers = htmlExtra != null ? (List<Map<String, Object>>) htmlExtra.get("headers") : null;
            if (headers != null) {
                headers.stream()
                    .map(h -> h.get("text"))
                    .forEach(text -> System.out.print(text + ", "));
                System.out.println();
            }
            // Access links
            @SuppressWarnings("unchecked")
            List<Map<String, Object>> links = htmlExtra != null ? (List<Map<String, Object>>) htmlExtra.get("links") : null;
            if (links != null) {
                for (Map<String, Object> link : links) {
                    System.out.println("Link: " + link.get("href") + " (" + link.get("text") + ")");
                }
            }
            // Access images
            @SuppressWarnings("unchecked")
            List<Map<String, Object>> images = htmlExtra != null ? (List<Map<String, Object>>) htmlExtra.get("images") : null;
            if (images != null) {
                for (Map<String, Object> image : images) {
                    System.out.println("Image: " + image.get("src"));
                }
            }
            // Access structured data
            @SuppressWarnings("unchecked")
            List<Map<String, Object>> structuredData = htmlExtra != null ? (List<Map<String, Object>>) htmlExtra.get("structured_data") : null;
            if (structuredData != null) {
                System.out.println("Structured data items: " + structuredData.size());
            }
        } catch (XbergRsException e) {
            System.err.println("Extraction failed: " + e.getMessage());
        }
    }
}
```
