---
id: fixture_node_error_extract_input_conflicting_ocr
language: typescript
target: node
level: typecheck
requires: []
side_effect: safe
---

extract force+disable OCR

```typescript title="TypeScript"
import { ExtractInput, ExtractInputKind, ExtractionConfig, XbergError, extract } from "@xberg-io/xberg";
async function main() {
  const input: ExtractInput = { bytes: await (await import("node:fs/promises")).readFile("test_documents/text/fake_text.txt"), config: { disableOcr: true, forceOcr: true }, filename: "fake_text.txt", kind: ExtractInputKind.Bytes, mimeType: "text/plain" };
  const config: ExtractionConfig = { disableOcr: true, forceOcr: true };
  try {
    await extract(input, config);
  } catch (error) {
    if (error instanceof XbergError) {
      console.error(`${error.name}: ${error.message}`);
    }
  }
}

void main();

```
