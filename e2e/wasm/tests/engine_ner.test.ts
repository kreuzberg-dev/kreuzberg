// Hand-written e2e coverage for the XbergEngine NER bridge (not alef-generated).
import { describe, expect, it } from "vitest";
import { XbergEngine } from "@xberg-io/xberg-wasm";

describe("XbergEngine.ner with injected backend", () => {
  it("returns entities from the injected backend", async () => {
    const injection = {
      ner: async (_text: string, _categories: string[]) => [
        { category: "person", text: "Alice", start: 0, end: 5, confidence: 0.95 },
        { category: "organization", text: "Acme Corp", start: 15, end: 24, confidence: 0.88 },
      ],
    };
    const engine = new XbergEngine({}, injection);

    const entities = await engine.ner("Alice works at Acme Corp", undefined);

    expect(entities).toHaveLength(2);
    expect(entities[0].text).toBe("Alice");
    expect(entities[1].text).toBe("Acme Corp");
  });

  it("forwards built-in and custom categories as plain strings", async () => {
    let seen: string[] | undefined;
    const injection = {
      ner: async (_text: string, categories: string[]) => {
        seen = categories;
        return [];
      },
    };
    const engine = new XbergEngine({}, injection);

    await engine.ner("some text", { categories: ["person", "invoice_number"] });

    expect(seen).toEqual(["person", "invoice_number"]);
  });
});

describe("XbergEngine.ner error paths", () => {
  it("rejects when no NER backend is injected", async () => {
    const engine = new XbergEngine({}, {});

    await expect(engine.ner("text", undefined)).rejects.toMatch(/NER unavailable/);
  });

  it("rejects when the injected object has no ner function", async () => {
    const engine = new XbergEngine({}, { ner: { notAFunction: true } });

    await expect(engine.ner("text", undefined)).rejects.toMatch(/no 'ner' function/);
  });
});
