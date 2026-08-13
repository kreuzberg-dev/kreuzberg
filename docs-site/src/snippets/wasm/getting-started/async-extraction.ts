import { extract, getWasmCapabilities, initWasm } from "@xberg-io/xberg-wasm";

async function extractDocuments(files: Uint8Array[], mimeTypes: string[]) {
  const caps = getWasmCapabilities();
  if (!caps.hasWasm) {
    throw new Error("WebAssembly not supported");
  }

  await initWasm();

  const results = await Promise.all(
    files.map((bytes, index) => extract({ kind: "bytes", bytes, mimeType: mimeTypes[index] })),
  );

  return results.map((r) => ({
    content: r.content,
    pageCount: r.metadata?.pageCount,
  }));
}

const fileBytes = [new Uint8Array([1, 2, 3])];
const mimes = ["application/pdf"];

extractDocuments(fileBytes, mimes)
  .then((results) => console.log(results))
  .catch(console.error);
