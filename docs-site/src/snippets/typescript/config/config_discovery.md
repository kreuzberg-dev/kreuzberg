# Configuration Discovery Example

The Node binding has no built-in config-file discovery helper — automatic
`xberg.toml`/`xberg.yaml`/`xberg.json` lookup is a CLI/server-side feature.
From Node, read and parse the file yourself and pass the result as `config`:

```typescript title="config_discovery.ts"
import { existsSync, readFileSync } from "node:fs";
import { extract, type ExtractionConfig } from "@xberg-io/xberg";

const input = {
  kind: "uri" as const,
  uri: "document.pdf",
};

const configPath = "xberg.json";

if (existsSync(configPath)) {
  console.log("Found configuration file");
  const config = JSON.parse(readFileSync(configPath, "utf8")) as ExtractionConfig;
  const output = await extract(input, config);
  console.log(output.results[0].content);
} else {
  console.log("No configuration file found, using defaults");
  const output = await extract(input);
  console.log(output.results[0].content);
}
```
