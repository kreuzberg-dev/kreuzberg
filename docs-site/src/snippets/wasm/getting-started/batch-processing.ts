import { extractBatch, initWasm } from "@xberg-io/xberg-wasm";

interface DocumentJob {
  name: string;
  bytes: Uint8Array;
  mimeType: string;
}

async function _processBatch(documents: DocumentJob[], concurrency: number = 3) {
  await initWasm();

  const results: Record<string, string> = {};

  for (let index = 0; index < documents.length; index += concurrency) {
    const batch = documents.slice(index, index + concurrency);
    const output = await extractBatch(
      batch.map((doc) => ({
        kind: "bytes",
        bytes: doc.bytes,
        mimeType: doc.mimeType,
        filename: doc.name,
      })),
    );

    output.results.forEach((result, resultIndex) => {
      results[batch[resultIndex].name] = result.content ?? "";
    });
  }
  return results;
}
