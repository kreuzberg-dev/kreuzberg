---
id: fixture_node_ocr_image_png
language: typescript
target: node
level: typecheck
requires: []
side_effect: safe
---

OCR: PNG image extraction with OCR enabled. In WASM this exercises the Uint8Array bridge parameter and Promise await in the generated OcrBackend bridge.

```typescript title="TypeScript"
import { ExtractInput, ExtractInputKind, extract } from "@xberg-io/xberg";
async function main() {
  const input: ExtractInput = { bytes: await (await import("node:fs/promises")).readFile("test_documents/images/test_hello_world.png"), config: {  }, filename: "test_hello_world.png", kind: ExtractInputKind.Bytes, mimeType: "image/png" };
  const result = await extract(input, undefined);
  console.log(result.results[0].content);
}

void main();

```
