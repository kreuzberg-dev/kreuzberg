```typescript title="TypeScript"
import { readFileSync } from "fs";

async function extractViaClient() {
  const formData = new FormData();
  const fileData = readFileSync("document.pdf");
  formData.append("files", new Blob([fileData]), "document.pdf");

  try {
    const response = await fetch("http://localhost:8000/extract", {
      method: "POST",
      body: formData,
    });

    if (!response.ok) {
      const error = await response.json();
      console.error(`Error: ${error.error_type}: ${error.message}`);
      return;
    }

    const results = await response.json();
    console.log(`Extracted ${results.length} document(s)`);
    console.log(results[0].content);
  } catch (error: unknown) {
    if (error instanceof Error) {
      console.error(`Request failed: ${error.message}`);
    }
  }
}

extractViaClient();
```
