```java title="Java"
import com.fasterxml.jackson.databind.ObjectMapper;
import java.io.BufferedReader;
import java.io.BufferedWriter;
import java.io.IOException;
import java.io.InputStreamReader;
import java.io.OutputStreamWriter;
import java.util.Map;

public class McpClient implements AutoCloseable {
    private final Process mcpProcess;
    private final BufferedWriter stdin;
    private final BufferedReader stdout;
    private final ObjectMapper mapper = new ObjectMapper();

    public McpClient() throws IOException {
        ProcessBuilder pb = new ProcessBuilder("xberg", "mcp");
        mcpProcess = pb.start();
        stdin = new BufferedWriter(new OutputStreamWriter(mcpProcess.getOutputStream()));
        stdout = new BufferedReader(new InputStreamReader(mcpProcess.getInputStream()));
    }

    // Note: This is a custom RPC client method, not Xberg.extract() API
    public String extract(String path) throws IOException {
        Map<String, Object> request = Map.of(
            "method", "tools/call",
            "params", Map.of(
                "name", "extract",
                "arguments", Map.of("path", path, "async", true)
            )
        );

        stdin.write(mapper.writeValueAsString(request));
        stdin.newLine();
        stdin.flush();

        String response = stdout.readLine();
        @SuppressWarnings("unchecked")
        Map<String, Object> result = mapper.readValue(response, Map.class);
        @SuppressWarnings("unchecked")
        Map<String, Object> resultData = (Map<String, Object>) result.get("result");
        return (String) resultData.get("content");
    }

    public void close() throws IOException {
        stdin.close();
        stdout.close();
        mcpProcess.destroy();
    }

    public static void main(String[] args) {
        try (McpClient client = new McpClient()) {
            String content = client.extract("contract.pdf");
            System.out.println("Extracted content: " + content);
        } catch (IOException e) {
            System.err.println("Error: " + e.getMessage());
        }
    }
}
```
