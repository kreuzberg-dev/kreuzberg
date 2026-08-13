import { ExtractInputKind, extract, initWasm } from "@xberg-io/xberg-wasm";

async function main() {
  await initWasm();

  const buffer = await fetch("document.pdf").then((r) => r.arrayBuffer());
  const bytes = new Uint8Array(buffer);

  const output = await extract({
    kind: "bytes",
    bytes,
    mimeType: "application/pdf",
    filename: "document.pdf",
  });

  console.log("Extracted content:");
  console.log(output.results[0].content);
  console.log("MIME type:", output.results[0].mimeType);
  console.log("Metadata:", output.results[0].metadata);
}

main().catch(console.error);
