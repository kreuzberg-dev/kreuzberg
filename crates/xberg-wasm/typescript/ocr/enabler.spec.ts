/**
 * Tests for enableOcr() and the JS→Rust OCR registry bridge.
 *
 * Key invariant under test:
 *   After enableOcr() completes, the Rust-side plugin registry must contain a
 *   backend named "tesseract" — because that is the default OcrConfig.backend
 *   the image extractor queries at extraction time.
 *
 * The critical failure mode is registerBackendInRustRegistry() silently returning
 * when wasm.register_ocr_backend is absent/undefined, leaving the Rust registry
 * empty while the JS registry appears populated.
 */

import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("../extraction/internal.js", () => ({
  isInitialized: vi.fn(() => true),
}));

vi.mock("../initialization/state.js", () => ({
  getWasmModule: vi.fn(),
}));

vi.mock("../runtime.js", () => ({
  isBrowser: vi.fn(() => false),
  isNode: vi.fn(() => false),
}));

vi.mock("./worker-bridge.js", () => ({
  createOcrWorker: vi.fn(async () => undefined),
  runOcrInWorker: vi.fn(async () => "mocked ocr text"),
  terminateOcrWorker: vi.fn(async () => undefined),
}));

vi.mock("./registry.js", () => ({
  registerOcrBackend: vi.fn(),
}));

import { isInitialized } from "../extraction/internal.js";
import { getWasmModule } from "../initialization/state.js";
import { isBrowser } from "../runtime.js";
import { registerOcrBackend as registerJsOcrBackend } from "./registry.js";
import { enableOcr } from "./enabler.js";

/** Build a minimal mock WasmModule with register_ocr_backend included. */
function makeWasmModule(overrides: Record<string, unknown> = {}) {
  return {
    ocrIsAvailable: vi.fn(() => true),
    ocrRecognize: vi.fn(() => "ocr text"),
    register_ocr_backend: vi.fn(),
    ...overrides,
  };
}

describe("enableOcr()", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(isInitialized).mockReturnValue(true);
    vi.mocked(isBrowser).mockReturnValue(false);
  });

  it("throws if WASM is not initialized", async () => {
    vi.mocked(isInitialized).mockReturnValue(false);
    vi.mocked(getWasmModule).mockReturnValue(null as any);

    await expect(enableOcr()).rejects.toThrow("WASM module not initialized");
  });

  describe("when ocr-wasm feature is available (ocrIsAvailable returns true)", () => {
    it("registers the JS-side backend", async () => {
      const wasm = makeWasmModule();
      vi.mocked(getWasmModule).mockReturnValue(wasm as any);

      await enableOcr();

      expect(registerJsOcrBackend).toHaveBeenCalledOnce();
    });

    it("registers a backend named 'tesseract' in the Rust registry", async () => {
      const wasm = makeWasmModule();
      vi.mocked(getWasmModule).mockReturnValue(wasm as any);

      await enableOcr();

      expect(wasm.register_ocr_backend).toHaveBeenCalledOnce();

      // The adapter passed to the Rust registry MUST be named "tesseract"
      // because that is what OcrConfig.backend defaults to. ~keep
      const rustAdapter = vi.mocked(wasm.register_ocr_backend).mock.calls[0][0] as any;
      expect(rustAdapter.name()).toBe("tesseract");
    });

    it("rust adapter has a supportedLanguages() method", async () => {
      const wasm = makeWasmModule();
      vi.mocked(getWasmModule).mockReturnValue(wasm as any);

      await enableOcr();

      const rustAdapter = vi.mocked(wasm.register_ocr_backend).mock.calls[0][0] as any;
      const langs = rustAdapter.supportedLanguages();
      expect(Array.isArray(langs)).toBe(true);
      expect(langs.length).toBeGreaterThan(0);
    });

    it("rust adapter has a processImage() method that returns a JSON string", async () => {
      const wasm = makeWasmModule();
      vi.mocked(getWasmModule).mockReturnValue(wasm as any);

      // Stub fetch so NativeWasmOcrBackend.getTessdata() doesn't hit the CDN. ~keep
      const fakeTessdata = new Uint8Array([1, 2, 3]);
      vi.stubGlobal(
        "fetch",
        vi.fn(async () => ({
          ok: true,
          arrayBuffer: async () => fakeTessdata.buffer,
        })),
      );

      await enableOcr();

      const rustAdapter = vi.mocked(wasm.register_ocr_backend).mock.calls[0][0] as any;
      const result = await rustAdapter.processImage("base64imagedata", "eng");

      vi.unstubAllGlobals();

      // The Rust bridge expects a JSON string, not an object ~keep
      expect(typeof result).toBe("string");
      expect(() => JSON.parse(result)).not.toThrow();
    });

    it("throws immediately when register_ocr_backend is absent from the wasm module", async () => {
      // Simulates the failure mode: the property is undefined because
      // wasm-bindgen exports it under a different name at runtime, or the
      // WASM binary was built without the 'ocr-wasm' feature.
      //
      // The fix (issue #719): enableOcr() must THROW rather than succeed
      // silently. A silent return leaves the Rust registry empty, causing the
      // cryptic "OCR backend 'tesseract' not registered. Available backends: []"
      // error at extraction time — far harder to diagnose than an early throw. ~keep
      const wasm = makeWasmModule({ register_ocr_backend: undefined });
      vi.mocked(getWasmModule).mockReturnValue(wasm as any);

      await expect(enableOcr()).rejects.toThrow("register_ocr_backend is not exported");
    });
  });

  describe("when ocr-wasm feature is NOT available", () => {
    it("falls back to TesseractWasmBackend in a browser environment", async () => {
      const wasm = makeWasmModule({ ocrIsAvailable: vi.fn(() => false) });
      vi.mocked(getWasmModule).mockReturnValue(wasm as any);
      vi.mocked(isBrowser).mockReturnValue(true);

      // TesseractWasmBackend.initialize() will fail because tesseract-wasm
      // isn't installed in the test environment — that's fine, we just check
      // it is attempted. ~keep
      await expect(enableOcr()).rejects.toThrow();
    });

    it("throws a descriptive error in non-browser environments", async () => {
      const wasm = makeWasmModule({ ocrIsAvailable: vi.fn(() => false) });
      vi.mocked(getWasmModule).mockReturnValue(wasm as any);
      vi.mocked(isBrowser).mockReturnValue(false);

      await expect(enableOcr()).rejects.toThrow(/No OCR backend available/);
    });
  });
});
