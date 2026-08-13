import { ExtractInputKind, extract, initWasm } from "@xberg-io/xberg-wasm";

async function setupFileInput() {
  await initWasm();

  const fileInput = document.getElementById("file-input") as HTMLInputElement;

  fileInput.addEventListener("change", async (event) => {
    const file = (event.target as HTMLInputElement).files?.[0];
    if (!file) return;

    try {
      const bytes = new Uint8Array(await file.arrayBuffer());
      const output = await extract({
        kind: "bytes",
        bytes,
        mimeType: file.type || "application/octet-stream",
        filename: file.name,
      });

      console.log("Extracted text:", output.results[0].content);
      displayResults(output.results[0]);
    } catch (error) {
      console.error("Extraction failed:", error);
    }
  });
}

function displayResults(result: any) {
  const output = document.getElementById("output");
  if (output) {
    output.textContent = `${result.content?.substring(0, 500) ?? ""}...`;
  }
}

setupFileInput().catch(console.error);
