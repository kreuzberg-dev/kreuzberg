package io.xberg.springai;

import com.fasterxml.jackson.core.JsonProcessingException;
import com.fasterxml.jackson.databind.ObjectMapper;
import io.xberg.BoundingBox;
import io.xberg.Chunk;
import io.xberg.ChunkMetadata;
import io.xberg.Element;
import io.xberg.ElementMetadata;
import io.xberg.ExtractInput;
import io.xberg.ExtractInputKind;
import io.xberg.ExtractedDocument;
import io.xberg.ExtractionConfig;
import io.xberg.ExtractionErrorItem;
import io.xberg.ExtractionResult;
import io.xberg.Metadata;
import io.xberg.PageContent;
import io.xberg.Xberg;
import io.xberg.XbergRsException;
import java.io.IOException;
import java.net.URLConnection;
import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.stream.Collectors;
import org.springframework.ai.document.Document;
import org.springframework.ai.document.DocumentReader;
import org.springframework.core.io.FileSystemResource;
import org.springframework.core.io.Resource;

/**
 * A Spring AI {@link DocumentReader} that uses Xberg for document extraction.
 *
 * <p>
 * Supports 101 document formats including PDF, DOCX, PPTX, images (with OCR),
 * and more. Each extracted document is split into Spring AI {@link Document}
 * instances using a priority-based strategy: chunks &gt; elements &gt; pages
 * &gt; whole document.
 *
 * <p>
 * A single resource is extracted through {@link Xberg#extract}; multiple
 * resources are extracted in one call through {@link Xberg#extractBatch}, which
 * is substantially faster than running a reader per resource.
 *
 * <p>
 * Use the {@link #builder()} to configure the reader:
 *
 * <pre>{@code
 * var reader = XbergDocumentReader.builder().resource(new
 * FileSystemResource("report.pdf")).build(); List<Document> docs =
 * reader.get();
 * }</pre>
 *
 * <p>
 * Read several files in one batch:
 *
 * <pre>{@code
 * var reader = XbergDocumentReader.builder()
 * 		.resources(List.of(new FileSystemResource("a.pdf"), new
 * FileSystemResource("b.docx"))).build(); List<Document> docs = reader.get();
 * }</pre>
 *
 * <p>
 * For a single non-file resource (e.g. {@code ByteArrayResource}), a MIME type
 * must be provided:
 *
 * <pre>{@code
 * var reader = XbergDocumentReader.builder().resource(new
 * ByteArrayResource(bytes)).mimeType("application/pdf") .build();
 * }</pre>
 */
public final class XbergDocumentReader implements DocumentReader {

  private static final ObjectMapper OBJECT_MAPPER = new ObjectMapper();

  private final List<Resource> resources;
  private final String mimeType;
  private final ExtractionConfig extractionConfig;
  private final Map<String, Object> additionalMetadata;

  private XbergDocumentReader(Builder builder) {
    this.resources = List.copyOf(builder.resources);
    this.mimeType = builder.mimeType;
    this.extractionConfig = builder.extractionConfig;
    this.additionalMetadata = Map.copyOf(builder.additionalMetadata);
  }

  /**
   * Extracts the configured resources and returns a flat list of Spring AI
   * {@link Document} instances.
   *
   * <p>
   * Every extracted document is split following priority order: if it contains
   * chunks, each chunk becomes a document; otherwise elements are used, then
   * pages, and finally the whole content as a single document. Documents from
   * multiple resources are concatenated in resource order.
   *
   * @return list of documents with extracted text and metadata
   * @throws RuntimeException
   *             if extraction or I/O fails
   */
  @Override
  public List<Document> get() {
    try {
      List<ExtractInput> inputs = buildInputs();
      List<String> sources =
          resources.stream().map(this::resolveSource).toList();
      ExtractionResult result = runExtraction(inputs);
      checkErrors(result);
      List<ExtractedDocument> documents = result.results();
      if (documents == null || documents.isEmpty()) {
        throw new IllegalStateException("Xberg extraction returned no results");
      }
      List<Document> output = new ArrayList<>();
      for (int i = 0; i < documents.size(); i++) {
        String source = i < sources.size() ? sources.get(i)
                                           : sources.get(sources.size() - 1);
        output.addAll(mapToDocuments(documents.get(i), source));
      }
      return output;
    } catch (IOException e) {
      throw new RuntimeException("Failed to extract document", e);
    } catch (XbergRsException e) {
      throw new RuntimeException("Xberg extraction failed", e);
    }
  }

  /**
   * Returns a new {@link Builder} for constructing a
   * {@link XbergDocumentReader}.
   */
  public static Builder builder() {
    return new Builder();
  }

  /**
   * Builder for {@link XbergDocumentReader}.
   *
   * <p>
   * At least one {@link Resource} must be provided. A single resource without a
   * filename (e.g. {@code ByteArrayResource}) also requires an explicit MIME
   * type. When reading multiple resources, each resource must carry a filename
   * so its MIME type can be resolved; per-resource MIME overrides are not
   * supported in batch mode.
   */
  public static final class Builder {

    private final List<Resource> resources = new ArrayList<>();
    private String mimeType;
    private ExtractionConfig extractionConfig;
    private final Map<String, Object> additionalMetadata =
        new LinkedHashMap<>();

    private Builder() { }

    /**
     * Adds a Spring {@link Resource} to extract text from. At least one is
     * required.
     */
    public Builder resource(Resource resource) {
      this.resources.add(resource);
      return this;
    }

    /**
     * Adds all of the given resources for batch extraction via {@link
     * Xberg#extractBatch}.
     */
    public Builder resources(List<Resource> resources) {
      this.resources.addAll(resources);
      return this;
    }

    /**
     * Sets an explicit MIME type for the resource. Required when a single
     * resource has no filename (e.g. {@code ByteArrayResource}). Overrides any
     * MIME type guessed from the filename. Only valid when exactly one resource
     * is configured.
     */
    public Builder mimeType(String mimeType) {
      this.mimeType = mimeType;
      return this;
    }

    /**
     * Sets the Xberg {@link ExtractionConfig} to control extraction behavior
     * (chunking, OCR, keywords, NER, and every other capability the engine
     * exposes).
     */
    public Builder extractionConfig(ExtractionConfig config) {
      this.extractionConfig = config;
      return this;
    }

    /**
     * Adds all entries from the given map as additional metadata on each output
     * document.
     */
    public Builder metadata(Map<String, Object> metadata) {
      this.additionalMetadata.putAll(metadata);
      return this;
    }

    /**
     * Adds a single key-value pair as additional metadata on each output
     * document.
     */
    public Builder metadata(String key, Object value) {
      this.additionalMetadata.put(key, value);
      return this;
    }

    /**
     * Builds the reader, validating that required fields are set.
     *
     * @throws IllegalArgumentException
     *             if no resource is set, if a lone resource has neither a
     * filename nor a MIME type, or if a batch resource has no filename
     */
    public XbergDocumentReader build() {
      if (resources.isEmpty()) {
        throw new IllegalArgumentException("at least one resource is required");
      }
      if (resources.size() == 1) {
        validateSingle(resources.get(0));
      } else {
        validateBatch();
      }
      return new XbergDocumentReader(this);
    }

    private void validateSingle(Resource resource) {
      if (resource.getFilename() == null && mimeType == null) {
        throw new IllegalArgumentException(
            "mimeType is required when resource has no filename (e.g. "
            + "ByteArrayResource)");
      }
    }

    private void validateBatch() {
      if (mimeType != null) {
        throw new IllegalArgumentException(
            "mimeType is only supported when reading a single resource");
      }
      for (Resource resource : resources) {
        if (resource.getFilename() == null) {
          throw new IllegalArgumentException(
              "each resource must have a filename when reading multiple "
              + "resources");
        }
      }
    }
  }

  /**
   * Builds an {@link ExtractInput} per resource. A {@link FileSystemResource}
   * takes the URI fast path (Xberg reads the file directly); every other
   * resource is read into memory and submitted as bytes.
   */
  private List<ExtractInput> buildInputs() throws IOException {
    List<ExtractInput> inputs = new ArrayList<>(resources.size());
    for (Resource resource : resources) {
      inputs.add(toInput(resource));
    }
    return inputs;
  }

  private ExtractInput toInput(Resource resource) throws IOException {
    if (resource instanceof FileSystemResource) {
      return ExtractInput.builder()
          .withKind(ExtractInputKind.Uri)
          .withUri(resource.getFile().getAbsolutePath())
          .withFilename(resource.getFilename())
          .build();
    }
    byte[] bytes = resource.getInputStream().readAllBytes();
    return ExtractInput.builder()
        .withKind(ExtractInputKind.Bytes)
        .withBytes(bytes)
        .withMimeType(resolveMimeType(resource))
        .withFilename(resource.getFilename())
        .build();
  }

  /**
   * Runs {@link Xberg#extract} for a single input and {@link
   * Xberg#extractBatch} for multiple inputs. Batch extraction shares the
   * engine's work across inputs and is markedly faster than one reader per
   * resource.
   */
  private ExtractionResult runExtraction(List<ExtractInput> inputs)
      throws XbergRsException {
    ExtractionConfig config = extractionConfig != null
                                  ? extractionConfig
                                  : ExtractionConfig.builder().build();
    if (inputs.size() == 1) {
      return Xberg.extract(inputs.get(0), config);
    }
    return Xberg.extractBatch(inputs, config);
  }

  /**
   * Fails loudly if the engine reported any per-input error, surfacing the
   * offending source and message rather than silently dropping a resource.
   */
  private static void checkErrors(ExtractionResult result) {
    List<ExtractionErrorItem> errors = result.errors();
    if (errors == null || errors.isEmpty()) {
      return;
    }
    String detail = errors.stream()
                        .map(error -> error.source() + ": " + error.message())
                        .collect(Collectors.joining("; "));
    throw new IllegalStateException("Xberg extraction reported errors: "
                                    + detail);
  }

  private String resolveSource(Resource resource) {
    String filename = resource.getFilename();
    if (filename != null) {
      return filename;
    }
    return "bytes://" + resolveMimeType(resource);
  }

  private String resolveMimeType(Resource resource) {
    if (mimeType != null && resources.size() == 1) {
      return mimeType;
    }
    String filename = resource.getFilename();
    if (filename != null) {
      String guessed = URLConnection.guessContentTypeFromName(filename);
      if (guessed != null) {
        return guessed;
      }
      return "application/octet-stream";
    }
    throw new IllegalStateException("Cannot resolve MIME type: no explicit "
                                    + "mimeType and resource has no filename");
  }

  /**
   * Maps an extracted document to Spring AI documents using the
   * highest-granularity splitting available: chunks &gt; elements &gt; pages
   * &gt; whole document.
   */
  private List<Document> mapToDocuments(ExtractedDocument document,
                                        String source) {
    Map<String, Object> baseMetadata = buildBaseMetadata(document, source);

    List<Chunk> chunks = document.chunks();
    if (chunks != null && !chunks.isEmpty()) {
      return mapChunksToDocuments(chunks, baseMetadata);
    }

    List<Element> elements = document.elements();
    if (elements != null && !elements.isEmpty()) {
      return mapElementsToDocuments(elements, baseMetadata);
    }

    List<PageContent> pages = document.pages();
    if (pages != null && !pages.isEmpty()) {
      return mapPagesToDocuments(pages, baseMetadata);
    }

    return List.of(new Document(document.content(), baseMetadata));
  }

  private List<Document>
  mapChunksToDocuments(List<Chunk> chunks, Map<String, Object> baseMetadata) {
    return chunks.stream()
        .map(chunk -> {
          Map<String, Object> metadata = new LinkedHashMap<>(baseMetadata);
          ChunkMetadata chunkMeta = chunk.metadata();
          metadata.put("chunk_index", chunkMeta.chunkIndex());
          metadata.put("total_chunks", chunkMeta.totalChunks());
          if (chunk.chunkType() != null) {
            metadata.put("chunk_type", chunk.chunkType().getValue());
          }
          if (chunkMeta.tokenCount() != null) {
            metadata.put("token_count", chunkMeta.tokenCount());
          }
          if (chunkMeta.firstPage() != null) {
            metadata.put("first_page", chunkMeta.firstPage());
          }
          if (chunkMeta.lastPage() != null) {
            metadata.put("last_page", chunkMeta.lastPage());
          }
          if (chunkMeta.headingPath() != null
              && !chunkMeta.headingPath().isEmpty()) {
            metadata.put("heading_path",
                         String.join(" > ", chunkMeta.headingPath()));
          }
          if (chunkMeta.headingContext() != null) {
            metadata.put("heading_context", toJson(chunkMeta.headingContext()));
          }
          return new Document(chunk.content(), metadata);
        })
        .toList();
  }

  private List<Document>
  mapElementsToDocuments(List<Element> elements,
                         Map<String, Object> baseMetadata) {
    return elements.stream()
        .map(element -> {
          Map<String, Object> metadata = new LinkedHashMap<>(baseMetadata);
          metadata.put("element_type", element.elementType().getValue());
          ElementMetadata elemMeta = element.metadata();
          if (elemMeta.elementIndex() != null) {
            metadata.put("element_index", elemMeta.elementIndex());
          }
          if (elemMeta.pageNumber() != null) {
            metadata.put("page_number", elemMeta.pageNumber());
          }
          BoundingBox bbox = elemMeta.coordinates();
          if (bbox != null) {
            metadata.put("bbox_x0", bbox.x0());
            metadata.put("bbox_y0", bbox.y0());
            metadata.put("bbox_x1", bbox.x1());
            metadata.put("bbox_y1", bbox.y1());
          }
          return new Document(element.text(), metadata);
        })
        .toList();
  }

  private List<Document> mapPagesToDocuments(List<PageContent> pages,
                                             Map<String, Object> baseMetadata) {
    return pages.stream()
        .map(page -> {
          Map<String, Object> metadata = new LinkedHashMap<>(baseMetadata);
          metadata.put("page", page.pageNumber());
          return new Document(page.content(), metadata);
        })
        .toList();
  }

  /**
   * Builds the base metadata map applied to every output document. Metadata is
   * layered in priority order: format-specific pass-through (lowest), explicit
   * extraction fields, user-supplied additional metadata (highest).
   */
  private Map<String, Object> buildBaseMetadata(ExtractedDocument document,
                                                String source) {
    Map<String, Object> metadata = new LinkedHashMap<>();
    Metadata extractionMetadata = document.metadata();

    // 1. Format-specific pass-through (lowest priority among structured fields)
    // ~keep
    if (extractionMetadata != null) {
      addFormatSpecificMetadata(metadata, extractionMetadata);
    }

    // 2. Explicit fields from ExtractedDocument and Metadata ~keep
    metadata.put("source", source);
    metadata.put("mime_type", document.mimeType());
    metadata.put("page_count",
                 document.counts() != null ? document.counts().pages() : 0L);

    List<String> detectedLanguages = document.detectedLanguages();
    metadata.put(
        "detected_languages",
        detectedLanguages != null ? String.join(", ", detectedLanguages) : "");

    if (document.qualityScore() != null) {
      metadata.put("quality_score", document.qualityScore());
    }

    if (extractionMetadata != null) {
      addExtractionMetadata(metadata, extractionMetadata);
    }

    List<?> tables = document.tables();
    metadata.put("table_count", tables != null ? tables.size() : 0);
    if (tables != null && !tables.isEmpty()) {
      metadata.put("tables", toJson(tables));
    }

    if (document.extractedKeywords() != null) {
      metadata.put("extracted_keywords", toJson(document.extractedKeywords()));
    }
    if (document.processingWarnings() != null) {
      metadata.put("processing_warnings",
                   toJson(document.processingWarnings()));
    }

    // 3. User-supplied additional metadata (highest priority) ~keep
    metadata.putAll(additionalMetadata);

    return metadata;
  }

  /**
   * Copies the typed {@link Metadata} fields into the output map, skipping any
   * that are {@code null}. List-valued fields are joined into comma-separated
   * strings.
   */
  private void addExtractionMetadata(Map<String, Object> metadata,
                                     Metadata extractionMetadata) {
    if (extractionMetadata.title() != null) {
      metadata.put("title", extractionMetadata.title());
    }
    if (extractionMetadata.subject() != null) {
      metadata.put("subject", extractionMetadata.subject());
    }
    if (extractionMetadata.authors() != null) {
      metadata.put("authors", String.join(", ", extractionMetadata.authors()));
    }
    if (extractionMetadata.keywords() != null) {
      metadata.put("keywords",
                   String.join(", ", extractionMetadata.keywords()));
    }
    if (extractionMetadata.language() != null) {
      metadata.put("language", extractionMetadata.language());
    }
    if (extractionMetadata.createdAt() != null) {
      metadata.put("created_at", extractionMetadata.createdAt());
    }
    if (extractionMetadata.modifiedAt() != null) {
      metadata.put("modified_at", extractionMetadata.modifiedAt());
    }
    if (extractionMetadata.createdBy() != null) {
      metadata.put("created_by", extractionMetadata.createdBy());
    }
    if (extractionMetadata.modifiedBy() != null) {
      metadata.put("modified_by", extractionMetadata.modifiedBy());
    }
    if (extractionMetadata.category() != null) {
      metadata.put("category", extractionMetadata.category());
    }
    if (extractionMetadata.tags() != null) {
      metadata.put("tags", String.join(", ", extractionMetadata.tags()));
    }
    if (extractionMetadata.documentVersion() != null) {
      metadata.put("document_version", extractionMetadata.documentVersion());
    }
    if (extractionMetadata.abstractText() != null) {
      metadata.put("abstract_text", extractionMetadata.abstractText());
    }
    if (extractionMetadata.outputFormat() != null) {
      metadata.put("output_format", extractionMetadata.outputFormat());
    }
  }

  /**
   * Passes through format-specific metadata from the extraction result.
   * Primitives are added directly; complex types (lists, maps) are serialized
   * to JSON strings.
   */
  private void addFormatSpecificMetadata(Map<String, Object> metadata,
                                         Metadata extractionMetadata) {
    Map<String, Object> additional = extractionMetadata.additional();
    if (additional == null) {
      return;
    }
    for (Map.Entry<String, Object> entry : additional.entrySet()) {
      Object value = entry.getValue();
      if (value instanceof String || value instanceof Integer
          || value instanceof Long || value instanceof Float
          || value instanceof Double || value instanceof Boolean) {
        metadata.put(entry.getKey(), value);
      } else if (value instanceof List || value instanceof Map) {
        metadata.put(entry.getKey(), toJson(value));
      }
    }
  }

  private static String toJson(Object value) {
    try {
      return OBJECT_MAPPER.writeValueAsString(value);
    } catch (JsonProcessingException e) {
      throw new RuntimeException("Failed to serialize to JSON", e);
    }
  }
}
