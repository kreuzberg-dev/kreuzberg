import {
  type IBinaryKeyData,
  type IDataObject,
  type IExecuteFunctions,
  type INodeExecutionData,
  type INodeType,
  type INodeTypeDescription,
  NodeConnectionTypes,
  NodeOperationError,
} from "n8n-workflow";

import {
  extract,
  extractBatch,
  mapUrl,
  type ExtractInput,
  type ExtractionConfig,
  type UrlExtractionConfig,
} from "@xberg-io/xberg";

// The binding's config types are deeply `readonly` (1.0.7 API tightening), but these
// nodes build the config incrementally by assignment. Strip the top-level `readonly`
// for the local builder; a `Writable<T>` is still assignable to the `readonly` `T` the
// `extract` / `mapUrl` calls expect. ~keep
type Writable<T> = { -readonly [P in keyof T]: T[P] };

// Output format identifiers mirror the `OutputFormat` string enum exported by
// `@xberg-io/xberg`. Kept as a local alias so the node compiles against the
// binding's type without importing the runtime enum value. ~keep
type XbergOutputFormat = NonNullable<ExtractionConfig["outputFormat"]>;

// Default chunk sizing mirrors the binding's `ChunkingConfig` defaults. ~keep
const DEFAULT_CHUNK_SIZE = 1000;
const DEFAULT_CHUNK_OVERLAP = 200;

// MIME type emitted for the optional binary output, keyed by output format. ~keep
const OUTPUT_MIME_TYPES: Record<string, string> = {
  plain: "text/plain",
  markdown: "text/markdown",
  djot: "text/plain",
  html: "text/html",
  json: "application/json",
  structured: "application/json",
};

const OUTPUT_EXTENSIONS: Record<string, string> = {
  plain: "txt",
  markdown: "md",
  djot: "dj",
  html: "html",
  json: "json",
  structured: "json",
};

// A single extracted document as returned by the binding. Field types are kept
// loose because the binding's generated `.d.ts` names them via unexported
// aliases; the node only reads a documented subset. ~keep
interface XbergDocument {
  content?: string;
  mimeType?: string;
  extractionMethod?: string;
  detectedLanguages?: string[];
  qualityScore?: number;
  counts?: unknown;
  metadata?: unknown;
  tables?: unknown[];
  chunks?: unknown[];
  images?: unknown[];
  entities?: unknown[];
  summary?: unknown;
}

interface XbergError {
  index: number;
  message: string;
}

function parseOcrLanguages(raw: string): string[] {
  const languages = raw
    .split(",")
    .map((code) => code.trim())
    .filter((code) => code.length > 0);
  return languages.length > 0 ? languages : ["eng"];
}

// Translate the node's option collection into an `ExtractionConfig` accepted by
// the binding. Only options the user opted into are set so the binding's own
// defaults apply everywhere else. ~keep
function buildExtractionConfig(outputFormat: XbergOutputFormat, options: IDataObject): ExtractionConfig {
  const enableOcr = options.enableOcr !== false;

  const config: Writable<ExtractionConfig> = {
    outputFormat,
    forceOcr: enableOcr ? options.forceOcr === true : false,
    disableOcr: !enableOcr,
    enableQualityProcessing: options.enableQualityProcessing === true,
  };

  if (enableOcr) {
    config.ocr = { enabled: true, language: parseOcrLanguages((options.ocrLanguages as string) || "eng") };
  }
  if (options.enableChunking === true) {
    config.chunking = {
      maxCharacters: (options.chunkSize as number) || DEFAULT_CHUNK_SIZE,
      overlap: (options.chunkOverlap as number) ?? DEFAULT_CHUNK_OVERLAP,
    };
  }
  if (options.extractImages === true) {
    config.images = { extractImages: true };
  }

  return config;
}

// Build the per-item output JSON. `content` plus cheap structural signals are
// always emitted; heavier collections are gated on the matching include option. ~keep
function documentToJson(document: XbergDocument, options: IDataObject): IDataObject {
  const outputField = (options.outputField as string) || "text";

  const json: IDataObject = {
    [outputField]: document.content ?? "",
    mimeType: document.mimeType,
    extractionMethod: document.extractionMethod,
    detectedLanguages: document.detectedLanguages,
    counts: document.counts as IDataObject,
  };

  if (typeof document.qualityScore === "number") {
    json.qualityScore = document.qualityScore;
  }
  if (options.includeMetadata !== false && document.metadata) {
    json.metadata = document.metadata as IDataObject;
  }
  if (options.includeTables === true && document.tables) {
    json.tables = document.tables as IDataObject[];
  }
  if (options.includeChunks === true && document.chunks) {
    json.chunks = document.chunks as IDataObject[];
  }
  if (document.entities) {
    json.entities = document.entities as IDataObject[];
  }
  if (document.summary) {
    json.summary = document.summary as IDataObject;
  }

  return json;
}

async function buildBinaryOutput(
  context: IExecuteFunctions,
  document: XbergDocument,
  fileName: string | undefined,
  outputFormat: XbergOutputFormat,
  options: IDataObject,
): Promise<IBinaryKeyData> {
  const binaryOutputField = (options.binaryOutputField as string) || "data";
  const mimeType = OUTPUT_MIME_TYPES[outputFormat] ?? "text/plain";
  const extension = OUTPUT_EXTENSIONS[outputFormat] ?? "txt";
  const baseName = (fileName ?? "document").replace(/\.[^.]+$/, "");

  return {
    [binaryOutputField]: await context.helpers.prepareBinaryData(
      Buffer.from(document.content ?? "", "utf-8"),
      `${baseName}.${extension}`,
      mimeType,
    ),
  };
}

// Resolve the extraction input for one item from either uploaded binary data or
// a URL/path string, per the node's Input Source parameter. ~keep
async function buildExtractInput(
  context: IExecuteFunctions,
  itemIndex: number,
): Promise<{ input: ExtractInput; fileName?: string }> {
  const inputSource = context.getNodeParameter("inputSource", itemIndex, "binary") as string;

  if (inputSource === "url") {
    const uri = (context.getNodeParameter("sourceUri", itemIndex, "") as string).trim();
    if (!uri) {
      throw new NodeOperationError(context.getNode(), "No URL or path provided for extraction", { itemIndex });
    }
    return { input: { kind: "uri" as NonNullable<ExtractInput["kind"]>, uri } };
  }

  const binaryPropertyName = context.getNodeParameter("binaryPropertyName", itemIndex) as string;
  const binaryData = context.helpers.assertBinaryData(itemIndex, binaryPropertyName);
  const buffer = await context.helpers.getBinaryDataBuffer(itemIndex, binaryPropertyName);

  return {
    input: {
      // The binding requires an explicit source kind; "bytes" reads from the
      // in-memory `bytes` field rather than fetching a URI. ~keep
      kind: "bytes" as NonNullable<ExtractInput["kind"]>,
      bytes: new Uint8Array(buffer),
      filename: binaryData.fileName,
      mimeType: binaryData.mimeType,
    },
    fileName: binaryData.fileName,
  };
}

function errorItem(context: IExecuteFunctions, itemIndex: number, error: unknown): INodeExecutionData {
  return {
    json: context.getInputData(itemIndex)[0].json,
    error: error as NodeOperationError,
    pairedItem: { item: itemIndex },
  };
}

async function runExtract(context: IExecuteFunctions): Promise<INodeExecutionData[]> {
  const items = context.getInputData();
  const returnData: INodeExecutionData[] = [];

  for (let itemIndex = 0; itemIndex < items.length; itemIndex++) {
    try {
      const outputFormat = context.getNodeParameter("outputFormat", itemIndex) as XbergOutputFormat;
      const options = context.getNodeParameter("options", itemIndex, {}) as IDataObject;
      const config = buildExtractionConfig(outputFormat, options);

      const { input, fileName } = await buildExtractInput(context, itemIndex);
      const result = await extract(input, config);

      const firstError = result.errors?.[0];
      if (firstError) {
        throw new NodeOperationError(context.getNode(), `Xberg extraction failed: ${firstError.message}`, {
          itemIndex,
        });
      }
      const document = result.results?.[0] as XbergDocument | undefined;
      if (!document) {
        throw new NodeOperationError(context.getNode(), "Xberg returned no extracted document for the input", {
          itemIndex,
        });
      }

      const outputItem: INodeExecutionData = {
        json: documentToJson(document, options),
        pairedItem: { item: itemIndex },
      };
      if (options.returnBinary === true) {
        outputItem.binary = await buildBinaryOutput(context, document, fileName, outputFormat, options);
      }
      returnData.push(outputItem);
    } catch (error) {
      if (context.continueOnFail()) {
        returnData.push(errorItem(context, itemIndex, error));
        continue;
      }
      throw error;
    }
  }

  return returnData;
}

// Extract every input item in a single `extractBatch` call, which the binding
// schedules concurrently and is substantially faster than looping `extract`. ~keep
async function runExtractBatch(context: IExecuteFunctions): Promise<INodeExecutionData[]> {
  const items = context.getInputData();
  const outputFormat = context.getNodeParameter("outputFormat", 0) as XbergOutputFormat;
  const options = context.getNodeParameter("options", 0, {}) as IDataObject;
  const config = buildExtractionConfig(outputFormat, options);

  const inputs: ExtractInput[] = [];
  const fileNames: Array<string | undefined> = [];
  for (let itemIndex = 0; itemIndex < items.length; itemIndex++) {
    const { input, fileName } = await buildExtractInput(context, itemIndex);
    inputs.push(input);
    fileNames.push(fileName);
  }

  const result = await extractBatch(inputs, config);
  const errorsByIndex = new Map<number, XbergError>();
  for (const error of (result.errors ?? []) as XbergError[]) {
    errorsByIndex.set(error.index, error);
  }

  // Results come back in ascending input order with errored inputs omitted, so
  // a single forward cursor keeps successful documents aligned to their item. ~keep
  const documents = (result.results ?? []) as XbergDocument[];
  const returnData: INodeExecutionData[] = [];
  let cursor = 0;

  for (let itemIndex = 0; itemIndex < items.length; itemIndex++) {
    const failure = errorsByIndex.get(itemIndex);
    if (failure) {
      const error = new NodeOperationError(context.getNode(), `Xberg extraction failed: ${failure.message}`, {
        itemIndex,
      });
      if (context.continueOnFail()) {
        returnData.push(errorItem(context, itemIndex, error));
        continue;
      }
      throw error;
    }

    const document = documents[cursor++];
    if (!document) {
      const error = new NodeOperationError(context.getNode(), "Xberg returned no extracted document for the input", {
        itemIndex,
      });
      if (context.continueOnFail()) {
        returnData.push(errorItem(context, itemIndex, error));
        continue;
      }
      throw error;
    }

    const outputItem: INodeExecutionData = {
      json: documentToJson(document, options),
      pairedItem: { item: itemIndex },
    };
    if (options.returnBinary === true) {
      outputItem.binary = await buildBinaryOutput(context, document, fileNames[itemIndex], outputFormat, options);
    }
    returnData.push(outputItem);
  }

  return returnData;
}

async function runMapUrl(context: IExecuteFunctions): Promise<INodeExecutionData[]> {
  const items = context.getInputData();
  const returnData: INodeExecutionData[] = [];

  for (let itemIndex = 0; itemIndex < items.length; itemIndex++) {
    try {
      const uri = (context.getNodeParameter("mapUri", itemIndex, "") as string).trim();
      if (!uri) {
        throw new NodeOperationError(context.getNode(), "No URL provided to map", { itemIndex });
      }
      const mapOptions = context.getNodeParameter("mapOptions", itemIndex, {}) as IDataObject;

      const config: Writable<UrlExtractionConfig> = {};
      if (mapOptions.mode) {
        config.mode = mapOptions.mode as NonNullable<UrlExtractionConfig["mode"]>;
      }
      if (mapOptions.maxTotalUrls) {
        config.maxTotalUrls = mapOptions.maxTotalUrls as number;
      }

      const result = await mapUrl(uri, config);
      returnData.push({
        json: { url: uri, urls: (result.urls ?? []) as IDataObject[] },
        pairedItem: { item: itemIndex },
      });
    } catch (error) {
      if (context.continueOnFail()) {
        returnData.push(errorItem(context, itemIndex, error));
        continue;
      }
      throw error;
    }
  }

  return returnData;
}

export class Xberg implements INodeType {
  description: INodeTypeDescription = {
    displayName: "Xberg",
    name: "xberg",
    icon: "file:xberg.svg",
    group: ["transform"],
    version: 1,
    subtitle: '={{ $parameter["operation"] }}',
    description: "Extract text, tables, and metadata from documents with Xberg",
    defaults: {
      name: "Xberg",
    },
    inputs: [NodeConnectionTypes.Main],
    outputs: [NodeConnectionTypes.Main],
    credentials: [],
    properties: [
      {
        displayName: "Resource",
        name: "resource",
        type: "options",
        noDataExpression: true,
        options: [
          {
            name: "Document",
            value: "document",
          },
        ],
        default: "document",
      },
      {
        displayName: "Operation",
        name: "operation",
        type: "options",
        noDataExpression: true,
        displayOptions: {
          show: {
            resource: ["document"],
          },
        },
        options: [
          {
            name: "Extract",
            value: "extract",
            action: "Extract text and metadata from a document",
            description: "Extract text, tables, and metadata from one document per item",
          },
          {
            name: "Extract Batch",
            value: "extractBatch",
            action: "Extract every input item in one fast batch",
            description: "Extract all incoming items in a single batch call, faster than looping Extract",
          },
          {
            name: "Map URL",
            value: "mapUrl",
            action: "Discover links from a page or sitemap",
            description: "List the URLs reachable from a web page or sitemap without extracting them",
          },
        ],
        default: "extract",
      },
      {
        displayName: "Input Source",
        name: "inputSource",
        type: "options",
        displayOptions: {
          show: {
            operation: ["extract", "extractBatch"],
          },
        },
        options: [
          {
            name: "Binary Data",
            value: "binary",
            description: "Read the document from a binary property on the incoming item",
          },
          {
            name: "URL or Path",
            value: "url",
            description: "Fetch the document from an HTTP(S) URL or read it from a local path",
          },
        ],
        default: "binary",
        description: "Where the document to extract comes from",
      },
      {
        displayName: "Input Binary Field",
        name: "binaryPropertyName",
        type: "string",
        default: "data",
        required: true,
        displayOptions: {
          show: {
            operation: ["extract", "extractBatch"],
            inputSource: ["binary"],
          },
        },
        description: "Name of the binary property on the incoming item that holds the document to extract",
      },
      {
        displayName: "URL or Path",
        name: "sourceUri",
        type: "string",
        default: "",
        required: true,
        displayOptions: {
          show: {
            operation: ["extract", "extractBatch"],
            inputSource: ["url"],
          },
        },
        placeholder: "https://example.com/report.pdf",
        description: "HTTP(S) URL or local filesystem path of the document to extract",
      },
      {
        displayName: "URL",
        name: "mapUri",
        type: "string",
        default: "",
        required: true,
        displayOptions: {
          show: {
            operation: ["mapUrl"],
          },
        },
        placeholder: "https://example.com",
        description: "Web page or sitemap URL to discover links from",
      },
      {
        displayName: "Output Format",
        name: "outputFormat",
        type: "options",
        displayOptions: {
          show: {
            operation: ["extract", "extractBatch"],
          },
        },
        options: [
          {
            name: "Djot",
            value: "djot",
          },
          {
            name: "HTML",
            value: "html",
          },
          {
            name: "JSON Tree",
            value: "json",
          },
          {
            name: "Markdown",
            value: "markdown",
          },
          {
            name: "Plain Text",
            value: "plain",
          },
          {
            name: "Structured JSON",
            value: "structured",
          },
        ],
        default: "markdown",
        description: "Format of the extracted text content",
      },
      {
        displayName: "Options",
        name: "options",
        type: "collection",
        placeholder: "Add Option",
        default: {},
        displayOptions: {
          show: {
            operation: ["extract", "extractBatch"],
          },
        },
        options: [
          {
            displayName: "Binary Output Field",
            name: "binaryOutputField",
            type: "string",
            default: "data",
            displayOptions: {
              show: {
                returnBinary: [true],
              },
            },
            description: "Name of the binary property to attach the extracted content to",
          },
          {
            displayName: "Chunk Overlap",
            name: "chunkOverlap",
            type: "number",
            default: DEFAULT_CHUNK_OVERLAP,
            displayOptions: {
              show: {
                enableChunking: [true],
              },
            },
            description: "Number of overlapping characters between adjacent chunks",
          },
          {
            displayName: "Chunk Size",
            name: "chunkSize",
            type: "number",
            default: DEFAULT_CHUNK_SIZE,
            displayOptions: {
              show: {
                enableChunking: [true],
              },
            },
            description: "Maximum number of characters per chunk",
          },
          {
            displayName: "Enable Chunking",
            name: "enableChunking",
            type: "boolean",
            default: false,
            description: "Whether to split the extracted content into overlapping chunks for RAG pipelines",
          },
          {
            displayName: "Enable OCR",
            name: "enableOcr",
            type: "boolean",
            default: true,
            description:
              "Whether to run OCR on images and scanned PDF pages. Disable for text-only documents to skip OCR entirely.",
          },
          {
            displayName: "Enable Quality Processing",
            name: "enableQualityProcessing",
            type: "boolean",
            default: false,
            description: "Whether to run quality post-processing to clean up the extracted text",
          },
          {
            displayName: "Extract Images",
            name: "extractImages",
            type: "boolean",
            default: false,
            description: "Whether to extract embedded images and report them in the output",
          },
          {
            displayName: "Force OCR",
            name: "forceOcr",
            type: "boolean",
            default: false,
            description:
              "Whether to force OCR on every page even when a usable text layer is present. Ignored when OCR is disabled.",
          },
          {
            displayName: "Include Chunks",
            name: "includeChunks",
            type: "boolean",
            default: false,
            description: "Whether to include the generated chunks in the output. Requires Enable Chunking.",
          },
          {
            displayName: "Include Metadata",
            name: "includeMetadata",
            type: "boolean",
            default: true,
            description: "Whether to include document metadata (title, author, dates) in the output",
          },
          {
            displayName: "Include Tables",
            name: "includeTables",
            type: "boolean",
            default: false,
            description: "Whether to include structured table data in the output",
          },
          {
            displayName: "OCR Languages",
            name: "ocrLanguages",
            type: "string",
            default: "eng",
            description: 'Comma-separated ISO 639-2 language codes for OCR recognition, for example "eng,deu"',
          },
          {
            displayName: "Output Content Field",
            name: "outputField",
            type: "string",
            default: "text",
            description: "Name of the JSON field to write the extracted content into",
          },
          {
            displayName: "Return As Binary",
            name: "returnBinary",
            type: "boolean",
            default: false,
            description: "Whether to also attach the extracted content as a binary property on the output item",
          },
        ],
      },
      {
        displayName: "Options",
        name: "mapOptions",
        type: "collection",
        placeholder: "Add Option",
        default: {},
        displayOptions: {
          show: {
            operation: ["mapUrl"],
          },
        },
        options: [
          {
            displayName: "Max Total URLs",
            name: "maxTotalUrls",
            type: "number",
            default: 0,
            description: "Maximum number of URLs to return. 0 uses the binding default.",
          },
          {
            displayName: "Mode",
            name: "mode",
            type: "options",
            options: [
              {
                name: "Auto",
                value: "auto",
                description: "Classify the resource after fetching it",
              },
              {
                name: "Crawl",
                value: "crawl",
                description: "Crawl from the seed URL and collect discovered links",
              },
              {
                name: "Document",
                value: "document",
                description: "Treat the URL as a single document or page",
              },
            ],
            default: "auto",
            description: "How the URL is interpreted while discovering links",
          },
        ],
      },
    ],
  };

  async execute(this: IExecuteFunctions): Promise<INodeExecutionData[][]> {
    const operation = this.getNodeParameter("operation", 0) as string;

    if (operation === "mapUrl") {
      return [await runMapUrl(this)];
    }
    if (operation === "extractBatch") {
      return [await runExtractBatch(this)];
    }
    return [await runExtract(this)];
  }
}
