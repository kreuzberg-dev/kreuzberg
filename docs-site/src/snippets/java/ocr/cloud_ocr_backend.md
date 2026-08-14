```java title="Java"
import io.xberg.*;
import io.xberg.ExtractInputKind;
import java.net.http.*;
import java.net.URI;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.List;

public class CloudOcrExample implements IOcrBackend {
    private final String apiKey;

    public CloudOcrExample(String apiKey) {
        this.apiKey = apiKey;
    }

    @Override
    public String name() {
        return "cloud-ocr";
    }

    @Override
    public String version() {
        return "1.0.0";
    }

    @Override
    public ExtractedDocument process_image(byte[] image_bytes, OcrConfig config) throws Exception {
        // Call cloud OCR API
        HttpClient client = HttpClient.newHttpClient();
        HttpRequest request = HttpRequest.newBuilder()
            .uri(URI.create("https://api.example.com/ocr"))
            .header("Authorization", "Bearer " + apiKey)
            .POST(HttpRequest.BodyPublishers.ofByteArray(image_bytes))
            .build();
        HttpResponse<String> response = client.send(request, HttpResponse.BodyHandlers.ofString());
        String text = parseTextFromResponse(response.body());
        return ExtractedDocument.builder()
            .withContent(text)
            .withMimeType("text/plain")
            .withMetadata(Metadata.builder().build())
            .build();
    }

    @Override
    public ExtractedDocument process_image_file(Path path, OcrConfig config) throws Exception {
        return process_image(Files.readAllBytes(path), config);
    }

    @Override
    public boolean supports_language(String lang) throws Exception {
        return true;
    }

    @Override
    public String backend_type() throws Exception {
        return "cloud-ocr";
    }

    @Override
    public List<String> supported_languages() throws Exception {
        return List.of("en");
    }

    @Override
    public boolean supports_table_detection() throws Exception {
        return false;
    }

    @Override
    public boolean supports_document_processing() throws Exception {
        return false;
    }

    @Override
    public boolean emits_structured_markdown() throws Exception {
        return false;
    }

    @Override
    public ExtractedDocument process_document(Path _path, OcrConfig _config) throws Exception {
        throw new UnsupportedOperationException("cloud-ocr does not support whole-document processing");
    }

    private static String parseTextFromResponse(String json) {
        // Parse JSON response and extract text field
        return json; // Simplified
    }

    public static void main(String[] args) {
        try {
            Xberg.registerOcrBackend(new CloudOcrExample("your-api-key"));
            // Use custom OCR backend in extraction
            // Note: Requires ExtractionConfig with OCR enabled
            var resultOutput = Xberg.extract(
                io.xberg.ExtractInput.builder()
                    .withKind(io.xberg.ExtractInputKind.Uri)
                    .withUri("scanned.pdf")
                    .build(),
                io.xberg.ExtractionConfig.builder().build()
            );
            ExtractedDocument result = resultOutput.results().get(0);
        } catch (Exception e) {
            e.printStackTrace();
        }
    }
}
```
