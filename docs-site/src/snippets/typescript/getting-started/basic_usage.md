```typescript title="TypeScript"
import { ExtractInputKind, extract } from "@xberg-io/xberg";

const config = {
  useCache: true,
  enableQualityProcessing: true,
};

const output = await extract(
  {
    kind: "uri",
    uri: "document.pdf",
  },
  config,
);

console.log(output.results[0].content);
console.log(`MIME Type: ${output.results[0].mimeType}`);
```
