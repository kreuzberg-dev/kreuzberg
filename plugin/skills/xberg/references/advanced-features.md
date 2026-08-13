<!--
AI-RULEZ :: GENERATED FILE — DO NOT EDIT
Content-Hash: blake3:b034a43a10995a097cdd745d1d2a207f7d7970da5a92d2c60b55da43e6aa9e15
Source-Hash: blake3:ac8bab9ae4a76e61f437af29b4a45a38326056a50769a177038927c69353edaf
Schema-Version: v1
-->

# Advanced Features Reference

Xberg provides advanced features for customization, semantic processing, and integration with external systems. All library examples use the 1.0 unified API: `extract(input, config)` returns an `ExtractionResult` envelope, and per-document data lives in `result.results[i]`.

## Plugin System

The plugin system extends Xberg's extraction pipeline with custom post-processors, validators, and OCR backends. Plugin callbacks receive a single `ExtractedDocument` (not the envelope), with `.content`, `.mime_type`/`.mimeType`, and `.metadata`.

### Custom Post-Processors

Post-processors enrich extraction results after document parsing. They run non-destructively — if a post-processor fails, the extraction still succeeds (errors are logged).

=== "Python"

    ```python
    import asyncio
    from xberg import register_post_processor, ExtractInput, extract, ExtractionConfig, ExtractedDocument

    class MetadataEnricher:
        def name(self) -> str:
            return "metadata_enricher"

        def version(self) -> str:
            return "1.0.0"

        def process(self, result: ExtractedDocument, config: ExtractionConfig) -> ExtractedDocument:
            result.metadata.additional["processed_by"] = "metadata_enricher"
            result.metadata.additional["char_count"] = str(len(result.content))
            return result

        def processing_stage(self) -> str:
            return "middle"  # "early", "middle", or "late"

    register_post_processor(MetadataEnricher())

    async def main() -> None:
        result = await extract(ExtractInput(uri="document.pdf"), ExtractionConfig())
        print(len(result.results[0].content))

    asyncio.run(main())
    ```

=== "TypeScript"

    ```typescript
    import { registerPostProcessor, extract } from "@xberg-io/xberg";

    const enricher = {
      name() {
        return "metadata_enricher";
      },
      async process(result) {
        result.metadata.processed_by = "metadata_enricher";
        result.metadata.char_count = result.content.length;
        return result;
      },
      processingStage() {
        return "middle"; // "early" | "middle" | "late"
      },
    };

    registerPostProcessor(enricher);

    const output = await extract({ kind: "uri", uri: "document.pdf" });
    console.log(output.results[0].content.length);
    ```

### Custom Validators

Validators perform quality checks. Unlike post-processors, a validator that raises causes the whole extraction to fail.

=== "Python"

    ```python
    from xberg import register_validator, ExtractedDocument, ExtractionConfig, ValidationError

    class MinimumContentValidator:
        def name(self) -> str:
            return "min_content_validator"

        def version(self) -> str:
            return "1.0.0"

        def validate(self, result: ExtractedDocument, config: ExtractionConfig) -> None:
            if len(result.content) < 100:
                raise ValidationError("Extracted content too short (< 100 chars)")

        def priority(self) -> int:
            return 100  # higher runs first

        def should_validate(self, result: ExtractedDocument, config: ExtractionConfig) -> bool:
            return "pdf" in result.mime_type.lower()

    register_validator(MinimumContentValidator())
    ```

=== "TypeScript"

    ```typescript
    import { registerValidator } from "@xberg-io/xberg";

    const validator = {
      name() {
        return "min_content_validator";
      },
      validate(result) {
        if (result.content.length < 100) {
          throw new Error("Extracted content too short (< 100 chars)");
        }
      },
      priority() {
        return 100;
      },
      shouldValidate(result) {
        return result.mimeType.toLowerCase().includes("pdf");
      },
    };

    registerValidator(validator);
    ```

### Custom OCR Backends

Register a custom OCR engine to integrate proprietary or specialized OCR.

=== "Python"

    ```python
    from xberg import register_ocr_backend, ExtractInput, extract, ExtractionConfig, OcrConfig, OcrBackendType

    class CustomOcrBackend:
        def name(self) -> str:
            return "custom_ocr"

        def version(self) -> str:
            return "1.0.0"

        def supported_languages(self) -> list[str]:
            return ["eng", "deu", "fra", "spa"]

        def supports_language(self, language: str) -> bool:
            return language in self.supported_languages()

        def backend_type(self) -> OcrBackendType:
            return OcrBackendType.CUSTOM

        # The second argument is the resolved OcrConfig, not a language string.
        def process_image(self, image_bytes: bytes, config: OcrConfig) -> dict:
            language = config.language[0] if config.language else "eng"
            # text = my_ocr_engine.recognize(image_bytes, language)
            return {
                "content": "Extracted text from image",
                "tables": [],
            }

    register_ocr_backend(CustomOcrBackend())

    config = ExtractionConfig(ocr=OcrConfig(backend="custom_ocr", language="eng"))
    # result = await extract(ExtractInput(uri="scanned.pdf"), config)
    ```

=== "TypeScript"

    ```typescript
    import { registerOcrBackend, extract } from "@xberg-io/xberg";

    const supportedLangs = ["eng", "deu", "fra", "spa"];

    const backend = {
      name() {
        return "custom_ocr";
      },
      supportedLanguages() {
        return supportedLangs;
      },
      supportsLanguage(lang) {
        return supportedLangs.includes(lang);
      },
      backendType() {
        return "Custom";
      },
      // The second argument is the resolved OcrConfig, not a language string.
      async processImage(imageBytes, config) {
        const language = config?.language?.[0] ?? "eng";
        // const text = await myOcrEngine.recognize(imageBytes, language);
        return {
          content: "Extracted text from image",
          mime_type: "text/plain",
          tables: [],
        };
      },
    };

    registerOcrBackend(backend);

    const config = { ocr: { backend: "custom_ocr", language: "eng" } };
    const output = await extract({ kind: "uri", uri: "scanned.pdf" }, config);
    ```

## Per-Input Configuration in Batch Operations

Override extraction settings for individual inputs by setting `config` on each `ExtractInput` to a `FileExtractionConfig`. Inputs without an override use the batch-level `ExtractionConfig`. This suits mixed-format batches where different documents need different OCR, output, or processing.

=== "Python"

    ```python
    import asyncio
    from xberg import ExtractInput, extract_batch, ExtractionConfig, FileExtractionConfig, OcrConfig

    async def main() -> None:
        report = ExtractInput(uri="report.pdf")
        scan = ExtractInput(
            kind="uri",
            uri="scan.tiff",
            config=FileExtractionConfig(
                force_ocr=True,
                ocr=OcrConfig(backend="tesseract", language="deu"),
            ),
        )

        output = await extract_batch([report, scan], ExtractionConfig(output_format="markdown"))
        for doc in output.results:
            print(len(doc.content))

    asyncio.run(main())
    ```

=== "TypeScript"

    ```typescript
    import { extractBatch } from "@xberg-io/xberg";

    const output = await extractBatch(
      [
        { kind: "uri", uri: "report.pdf" },
        {
          kind: "uri",
          uri: "scan.tiff",
          config: { forceOcr: true, ocr: { backend: "tesseract", language: "deu" } },
        },
      ],
      { outputFormat: "markdown" },
    );
    ```

All `ExtractionConfig` fields except batch-level concerns (`max_concurrent_extractions`, `use_cache`, `acceleration`, `security_limits`) can be overridden per input. Unset (`None`/omitted) fields inherit the batch default.

## Embeddings

Generate vector embeddings for text chunks using ONNX models. Embeddings enable semantic search, clustering, and similarity over extracted content. Available presets: `fast` (384 dims), `balanced` (768 dims), `quality` (1024 dims), `multilingual` (768 dims).

=== "Python"

    ```python
    from xberg import (
        ExtractInput, extract, ExtractionConfig,
        ChunkingConfig, EmbeddingConfig, EmbeddingModelType,
    )

    # Preset model (recommended)
    config = ExtractionConfig(
        chunking=ChunkingConfig(
            max_characters=512,
            overlap=100,
            embedding=EmbeddingConfig(
                model=EmbeddingModelType.preset("balanced"),
                normalize=True,
                batch_size=32,
            ),
        )
    )

    # Custom ONNX model from HuggingFace
    config = ExtractionConfig(
        chunking=ChunkingConfig(
            embedding=EmbeddingConfig(
                model=EmbeddingModelType.custom(
                    model_id="sentence-transformers/all-MiniLM-L6-v2",
                    dimensions=384,
                ),
            )
        )
    )

    result = await extract(ExtractInput(uri="document.pdf"), config)
    for chunk in result.results[0].chunks or []:
        print(f"{chunk.content[:50]}... -> {len(chunk.embedding) if chunk.embedding else 0} dims")
    ```

=== "TypeScript"

    ```typescript
    import { extract } from "@xberg-io/xberg";

    const config = {
      chunking: {
        maxCharacters: 512,
        overlap: 100,
        embedding: { model: { type: "preset", name: "balanced" } },
      },
    };

    const output = await extract({ kind: "uri", uri: "document.pdf" }, config);
    for (const chunk of output.results[0].chunks ?? []) {
      console.log(`Embedding dims: ${chunk.embedding?.length ?? 0}`);
    }
    ```

## Keyword Extraction

Extract important keywords using YAKE or RAKE.

=== "Python"

    ```python
    from xberg import (
        ExtractInput, extract, ExtractionConfig,
        KeywordConfig, KeywordAlgorithm, YakeParams, RakeParams,
    )

    # YAKE
    config = ExtractionConfig(
        keywords=KeywordConfig(
            algorithm=KeywordAlgorithm.YAKE,
            max_keywords=15,
            min_score=0.1,
            language="en",
            yake_params=YakeParams(window_size=1),
        )
    )

    # RAKE
    config = ExtractionConfig(
        keywords=KeywordConfig(
            algorithm=KeywordAlgorithm.RAKE,
            max_keywords=10,
            min_score=5.0,
            language="en",
            rake_params=RakeParams(min_word_length=1, max_words_per_phrase=3),
        )
    )

    result = await extract(ExtractInput(uri="document.pdf"), config)
    for keyword in result.results[0].extracted_keywords or []:
        print(f"{keyword.text}: {keyword.score} ({keyword.algorithm})")
    ```

=== "TypeScript"

    ```typescript
    import { extract } from "@xberg-io/xberg";

    const config = {
      keywords: {
        algorithm: "yake",
        maxKeywords: 15,
        minScore: 0.1,
        language: "en",
        yakeParams: { windowSize: 1 },
      },
    };

    const output = await extract({ kind: "uri", uri: "document.pdf" }, config);
    for (const keyword of output.results[0].extractedKeywords ?? []) {
      console.log(`${keyword.text}: ${keyword.score} (${keyword.algorithm})`);
    }
    ```

## Language Detection

Detect document language(s) using ISO 639-1 codes.

=== "Python"

    ```python
    from xberg import ExtractInput, extract, ExtractionConfig, LanguageDetectionConfig

    config = ExtractionConfig(
        language_detection=LanguageDetectionConfig(
            enabled=True,
            min_confidence=0.8,
            detect_multiple=False,
        )
    )

    result = await extract(ExtractInput(uri="multilingual.pdf"), config)
    for lang in result.results[0].detected_languages or []:
        print(f"Detected language: {lang}")  # e.g. "en", "de", "fr"
    ```

=== "TypeScript"

    ```typescript
    import { extract } from "@xberg-io/xberg";

    const config = {
      languageDetection: { enabled: true, minConfidence: 0.8, detectMultiple: false },
    };

    const output = await extract({ kind: "uri", uri: "multilingual.pdf" }, config);
    for (const lang of output.results[0].detectedLanguages ?? []) {
      console.log(`Detected language: ${lang}`);
    }
    ```

## Token Reduction

Reduce token count in extracted content to lower LLM costs. Higher modes are more aggressive but may discard more information.

=== "Python"

    ```python
    from xberg import ExtractInput, extract, ExtractionConfig, TokenReductionOptions

    config = ExtractionConfig(
        token_reduction=TokenReductionOptions(
            mode="moderate",  # "off", "light", "moderate", "aggressive", "maximum"
            preserve_important_words=True,
        )
    )

    result = await extract(ExtractInput(uri="document.pdf"), config)
    print(f"Reduced content length: {len(result.results[0].content)}")
    ```

=== "TypeScript"

    ```typescript
    import { extract } from "@xberg-io/xberg";

    const config = { tokenReduction: { mode: "moderate", preserveImportantWords: true } };
    const output = await extract({ kind: "uri", uri: "document.pdf" }, config);
    console.log(`Reduced content length: ${output.results[0].content.length}`);
    ```

**Modes:** `off` (default), `light`, `moderate`, `aggressive`, `maximum`.

## Page Extraction

Track per-page content separately.

=== "Python"

    ```python
    from xberg import ExtractInput, extract, ExtractionConfig, PageConfig

    config = ExtractionConfig(
        pages=PageConfig(
            extract_pages=True,
            insert_page_markers=True,
            marker_format="\n\n<!-- PAGE {page_num} -->\n\n",
        )
    )

    result = await extract(ExtractInput(uri="multi_page.pdf"), config)
    for page in result.results[0].pages or []:
        print(f"Page {page.page_number}: {len(page.content)} chars, {len(page.tables)} tables")
    ```

=== "TypeScript"

    ```typescript
    import { extract } from "@xberg-io/xberg";

    const config = {
      pages: {
        extractPages: true,
        insertPageMarkers: true,
        markerFormat: "\n\n<!-- PAGE {page_num} -->\n\n",
      },
    };

    const output = await extract({ kind: "uri", uri: "multi_page.pdf" }, config);
    for (const page of output.results[0].pages ?? []) {
      console.log(`Page ${page.pageNumber}: ${page.content.length} chars`);
    }
    ```

## Element-Based Output

Extract semantic elements (Unstructured-compatible) instead of unified content.

=== "Python"

    ```python
    from xberg import ExtractInput, extract, ExtractionConfig

    config = ExtractionConfig(result_format="element_based")
    result = await extract(ExtractInput(uri="document.pdf"), config)

    for element in result.results[0].elements or []:
        print(f"Type: {element.element_type}")  # title, heading, narrative_text, ...
        print(f"Text: {element.text}")
    ```

=== "TypeScript"

    ```typescript
    import { extract } from "@xberg-io/xberg";

    const config = { resultFormat: "element_based" };
    const output = await extract({ kind: "uri", uri: "document.pdf" }, config);

    for (const element of output.results[0].elements ?? []) {
      console.log(`Type: ${element.elementType}`);
      console.log(`Text: ${element.text}`);
    }
    ```

**Element types:** `title`, `heading`, `narrative_text`, `list_item`, `table`, `image`, `page_break`, `code_block`, `block_quote`, `footer`, `header`.

## Djot Content

Output extracted content in Djot markup.

=== "Python"

    ```python
    from xberg import ExtractInput, extract, ExtractionConfig

    config = ExtractionConfig(output_format="djot")
    result = await extract(ExtractInput(uri="document.pdf"), config)
    print(result.results[0].content)  # Djot-formatted content
    ```

=== "TypeScript"

    ```typescript
    import { extract } from "@xberg-io/xberg";

    const config = { outputFormat: "djot" };
    const output = await extract({ kind: "uri", uri: "document.pdf" }, config);
    console.log(output.results[0].content);
    ```

## Security Limits

Cap resource consumption for untrusted input (especially archives).

=== "Python"

    ```python
    from xberg import ExtractInput, extract, ExtractionConfig, SecurityLimits

    config = ExtractionConfig(
        security_limits=SecurityLimits(
            max_archive_size=524_288_000,   # 500 MB uncompressed
            max_compression_ratio=100,       # flag potential zip bombs
            max_files_in_archive=10_000,
            max_nesting_depth=1024,
            max_content_size=104_857_600,    # 100 MB
            max_table_cells=100_000,
        )
    )

    result = await extract(ExtractInput(uri="archive.zip"), config)
    ```

=== "TypeScript"

    ```typescript
    import { extract } from "@xberg-io/xberg";

    const config = {
      securityLimits: {
        maxArchiveSize: 524_288_000,
        maxCompressionRatio: 100,
        maxFilesInArchive: 10_000,
        maxNestingDepth: 1024,
        maxContentSize: 104_857_600,
        maxTableCells: 100_000,
      },
    };

    const output = await extract({ kind: "uri", uri: "archive.zip" }, config);
    ```

## Caching

Extraction results are cached by default (content-hash keyed) to speed up repeated extractions of identical documents.

=== "Python"

    ```python
    from xberg import ExtractInput, extract, ExtractionConfig

    # Caching on (default)
    result = await extract(ExtractInput(uri="document.pdf"), ExtractionConfig(use_cache=True))

    # Disable caching
    result = await extract(ExtractInput(uri="document.pdf"), ExtractionConfig(use_cache=False))
    ```

=== "TypeScript"

    ```typescript
    import { extract } from "@xberg-io/xberg";

    const output = await extract({ kind: "uri", uri: "document.pdf" }, { useCache: false });
    ```

**CLI cache management:**

    xberg cache stats
    xberg cache clear

## API Server

Run Xberg as an HTTP API server.

    # Default port 8000
    xberg serve

    # Custom host and port
    xberg serve --host 0.0.0.0 --port 9000

**Example:**

    curl -X POST -F "files=@document.pdf" http://localhost:8000/extract

## MCP Server

Run Xberg as a Model Context Protocol server for AI integrations.

    # stdio transport
    xberg mcp

    # HTTP transport
    xberg mcp --transport http --host 127.0.0.1 --port 8001

The MCP server exposes extraction tools (extract, extract_batch, detect_mime_type, cache management, and more) to AI models.
