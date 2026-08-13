import { detectRuntime, getWasmCapabilities, initWasm, isBrowser, isBun, isDeno, isNode } from "@xberg-io/xberg-wasm";

async function setupForRuntime() {
  const runtime = detectRuntime();
  const caps = getWasmCapabilities();

  console.log(`Running in ${runtime} environment`);
  console.log(`Workers: ${caps.hasWorkers}`);
  console.log(`SharedArrayBuffer: ${caps.hasSharedArrayBuffer}`);

  if (isBrowser()) {
    console.log("Browser features available");
  } else if (isNode()) {
    console.log("Node.js features available");
  } else if (isDeno()) {
    console.log("Deno features available");
  } else if (isBun()) {
    console.log("Bun features available");
  }

  await initWasm();
}

setupForRuntime().catch(console.error);
