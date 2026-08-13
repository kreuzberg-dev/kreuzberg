import { enableOcr, ExtractInputKind, extract, initWasm } from "@xberg-io/xberg-wasm";

async function extractWithOcr() {
  await initWasm();

  try {
    await enableOcr();
    console.log("OCR enabled successfully");
  } catch (error) {
    console.error("Failed to enable OCR:", error);
    return;
  }

  const bytes = new Uint8Array(await fetch("scanned-page.png").then((r) => r.arrayBuffer()));

  const output = await extract(
    {
      kind: "bytes",
      bytes,
      mimeType: "image/png",
      filename: "scanned-page.png",
    },
    {
      ocr: {
        backend: "tesseract-wasm",
        language: ["eng"],
      },
    },
  );

  console.log("Extracted text:");
  console.log(output.results[0].content);
}

extractWithOcr().catch(console.error);
