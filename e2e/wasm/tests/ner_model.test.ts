// Hand-written e2e coverage for the in-binary NerModel (not alef-generated).
//
// Argument contract and failure paths only: the success path needs real GLiNER2
// weights, which are not checked into CI.
import { describe, expect, it } from "vitest";
import { NerModel, XbergEngine } from "@xberg-io/xberg-wasm";

const someBytes = () => new Uint8Array([1, 2, 3, 4]);

describe("NerModel.load argument contract", () => {
	it("rejects a non-object argument", async () => {
		await expect(NerModel.load(42)).rejects.toMatch(/expects an object/);
	});

	it("names the field that is missing", async () => {
		await expect(NerModel.load({})).rejects.toMatch(/weights/);
	});

	it("names a later missing field rather than the first", async () => {
		await expect(NerModel.load({ weights: someBytes() })).rejects.toMatch(/tokenizer/);
	});

	it("names the field that has the wrong type", async () => {
		const options = {
			weights: "not bytes",
			tokenizer: someBytes(),
			encoderConfig: someBytes(),
		};

		await expect(NerModel.load(options)).rejects.toMatch(/weights.*Uint8Array/);
	});

	it("rejects an empty buffer", async () => {
		const options = {
			weights: new Uint8Array([]),
			tokenizer: someBytes(),
			encoderConfig: someBytes(),
		};

		await expect(NerModel.load(options)).rejects.toMatch(/empty/);
	});

	it("accepts ArrayBuffer as well as Uint8Array", async () => {
		const options = {
			weights: new ArrayBuffer(8),
			tokenizer: new ArrayBuffer(8),
			encoderConfig: new ArrayBuffer(8),
		};

		// Must pass the type check and fail on the bytes instead.
		await expect(NerModel.load(options)).rejects.toMatch(/NerModel\.load:/);
	});
});

describe("NerModel.load failure handling", () => {
	// The tokenizer parses before the weights, so malformed input stops there.
	// This pins that a bad model rejects rather than trapping.
	it("reports malformed model input as an error, not a trap", async () => {
		const options = {
			weights: new Uint8Array([1, 2, 3, 4, 5, 6, 7, 8]),
			tokenizer: new Uint8Array([123, 125]),
			encoderConfig: new Uint8Array([123, 125]),
		};

		await expect(NerModel.load(options)).rejects.toMatch(/NerModel\.load:/);
	});
});

describe("XbergEngine.ner without a backend", () => {
	it("points at both the injection and the in-binary route", async () => {
		const engine = new XbergEngine({}, {});

		await expect(engine.ner("text", undefined)).rejects.toMatch(/NerModel\.load/);
	});
});
