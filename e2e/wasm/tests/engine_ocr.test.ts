// Hand-written e2e coverage for the XbergEngine OCR bridge (not alef-generated).
import { describe, expect, it } from "vitest";
import { XbergEngine } from "@xberg-io/xberg-wasm";

const JPEG_MAGIC = new Uint8Array([0xff, 0xd8, 0xff, 0xe0]);

describe("XbergEngine construction", () => {
  it("constructs with empty config and injection", () => {
    const engine = new XbergEngine({}, {});
    expect(engine).toBeDefined();
  });

  it("constructs with null injection", () => {
    const engine = new XbergEngine({}, null);
    expect(engine).toBeDefined();
  });

  it("ignores unknown injection keys", () => {
    const engine = new XbergEngine({}, { somethingElse: { a: 1 } });
    expect(engine).toBeDefined();
  });
});

describe("XbergEngine.ocr with injected backend", () => {
  it("returns per-line text, confidence, and bounding boxes", async () => {
    const injection = {
      ocr: async (_bytes: Uint8Array, _opts: unknown) => ({
        text: "hello from ocr",
        lines: [{ text: "hello from ocr", confidence: 0.98, bbox: { x: 1, y: 2, width: 3, height: 4 } }],
      }),
    };
    const engine = new XbergEngine({}, injection);

    const result = await engine.ocr(JPEG_MAGIC, undefined);

    expect(result.text).toBe("hello from ocr");
    expect(result.lines).toHaveLength(1);
    expect(result.lines[0].text).toBe("hello from ocr");
    expect(result.lines[0].confidence).toBeCloseTo(0.98);
    expect(result.lines[0].bbox).toEqual({ x: 1, y: 2, width: 3, height: 4 });
  });

  it("degrades to empty lines when the backend omits geometry", async () => {
    const injection = {
      ocr: async () => ({ text: "no geometry available" }),
    };
    const engine = new XbergEngine({}, injection);

    const result = await engine.ocr(JPEG_MAGIC, undefined);

    expect(result.text).toBe("no geometry available");
    expect(result.lines).toEqual([]);
  });

  it("forwards the language option to the backend", async () => {
    let seenLanguage: string | undefined;
    const injection = {
      ocr: async (_bytes: Uint8Array, opts: { language?: string }) => {
        seenLanguage = opts.language;
        return { text: "ok", lines: [] };
      },
    };
    const engine = new XbergEngine({}, injection);

    await engine.ocr(JPEG_MAGIC, { language: "deu" });

    expect(seenLanguage).toBe("deu");
  });
});

describe("XbergEngine.ocr error paths", () => {
  it("rejects when no OCR backend is injected", async () => {
    const engine = new XbergEngine({}, {});

    await expect(engine.ocr(JPEG_MAGIC, undefined)).rejects.toMatch(/OCR unavailable/);
  });

  it("rejects on empty image bytes", async () => {
    const injection = { ocr: async () => ({ text: "unreachable", lines: [] }) };
    const engine = new XbergEngine({}, injection);

    await expect(engine.ocr(new Uint8Array([]), undefined)).rejects.toMatch(/empty/);
  });

  it("rejects when the injected object has no ocr function", async () => {
    const engine = new XbergEngine({}, { ocr: { notAFunction: true } });

    await expect(engine.ocr(JPEG_MAGIC, undefined)).rejects.toMatch(/no 'ocr' function/);
  });
});
