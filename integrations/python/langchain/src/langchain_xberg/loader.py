"""Xberg document loader for LangChain."""

from __future__ import annotations

import asyncio
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
from typing import TYPE_CHECKING, Any

from langchain_core.document_loaders import BaseLoader
from langchain_core.documents import Document
from xberg import ExtractInput, XbergError, extract, extract_batch

if TYPE_CHECKING:
    from collections.abc import AsyncIterator, Callable, Coroutine, Iterator

    from xberg import ExtractedDocument, ExtractionConfig, ExtractionResult

# Metadata fields (from xberg.Metadata) that carry JSON-friendly scalar/list values.
# Complex nested fields (pages, format, image_preprocessing, json_schema, error) are
# skipped because they are opaque native objects, not plain data. ~keep
_METADATA_FIELDS: tuple[str, ...] = (
    "title",
    "subject",
    "authors",
    "keywords",
    "language",
    "created_at",
    "modified_at",
    "created_by",
    "modified_by",
    "category",
    "tags",
    "document_version",
    "abstract_text",
    "output_format",
    "ocr_used",
    "extraction_duration_ms",
)


class XbergLoader(BaseLoader):
    """Load documents using Xberg, supporting 88+ file formats with true async.

    Xberg is a Rust-powered document intelligence library. This loader wraps its
    async extraction API to produce LangChain Documents with rich metadata.

    Extraction is async at the core; ``lazy_load`` bridges to it synchronously.
    Multiple paths (a list or a directory glob) are extracted with a single
    ``extract_batch`` call so concurrency happens Rust-side.

    By default each source becomes one Document. Enable chunking on the
    ``ExtractionConfig`` to emit one Document per chunk (with heading path, page
    span and token count metadata), or page splitting for one Document per page.

    Examples:
        Load a single file:
            >>> loader = XbergLoader(file_path="document.pdf")
            >>> docs = loader.load()

        Load multiple files (one batched extraction):
            >>> loader = XbergLoader(file_path=["a.pdf", "b.docx", "c.txt"])
            >>> docs = loader.load()

        Load from bytes:
            >>> loader = XbergLoader(data=raw_bytes, mime_type="application/pdf")
            >>> docs = loader.load()

        Load a directory:
            >>> loader = XbergLoader(file_path="./docs/", glob="**/*.pdf")
            >>> docs = loader.load()

        Per-page splitting:
            >>> from xberg import ExtractionConfig, PageConfig
            >>> config = ExtractionConfig(pages=PageConfig(extract_pages=True))
            >>> loader = XbergLoader(file_path="document.pdf", config=config)
            >>> docs = loader.load()  # One Document per page

        Chunking for retrieval (one Document per chunk):
            >>> from xberg import ExtractionConfig, ChunkingConfig
            >>> config = ExtractionConfig(chunking=ChunkingConfig(max_characters=1000, overlap=200))
            >>> loader = XbergLoader(file_path="document.pdf", config=config)
            >>> docs = loader.load()  # One Document per chunk, ready to embed

        Tune batch concurrency:
            >>> from xberg import ExtractionConfig
            >>> config = ExtractionConfig(max_concurrent_extractions=8)
            >>> loader = XbergLoader(file_path="./docs/", config=config)
            >>> docs = loader.load()

        Async loading:
            >>> loader = XbergLoader(file_path="document.pdf")
            >>> docs = await loader.aload()

    """

    def __init__(
        self,
        *,
        file_path: str | Path | list[str | Path] | None = None,
        data: bytes | None = None,
        mime_type: str | None = None,
        glob: str | None = None,
        config: ExtractionConfig | None = None,
    ) -> None:
        """Initialize the XbergLoader.

        Args:
            file_path: File path, list of file paths, or directory path to load.
            data: Raw bytes to extract text from. Mutually exclusive with file_path.
            mime_type: MIME type hint. Required when using data, optional for file_path.
            glob: Glob pattern for directory mode. Defaults to None (matches all files).
            config: Xberg ``ExtractionConfig`` controlling extraction behavior —
                output format, OCR settings, page splitting, chunking, batch
                concurrency, etc. Enable ``chunking`` to emit one Document per
                chunk, or ``pages`` to emit one Document per page. Defaults to
                None (Xberg defaults, one Document per file).

        Raises:
            ValueError: If neither file_path nor data is provided.
            ValueError: If both file_path and data are provided.
            ValueError: If data is provided without mime_type.

        """
        if file_path is None and data is None:
            msg = "Either 'file_path' or 'data' must be provided."
            raise ValueError(msg)
        if file_path is not None and data is not None:
            msg = "Cannot specify both 'file_path' and 'data'. Use one or the other."
            raise ValueError(msg)
        if data is not None and mime_type is None:
            msg = "'mime_type' is required when using 'data'."
            raise ValueError(msg)

        if isinstance(file_path, (str, Path)):
            self._file_path: Path | list[Path] | None = Path(file_path)
        elif file_path is not None:
            self._file_path = [Path(p) for p in file_path]
        else:
            self._file_path = None

        self._data = data
        self._mime_type = mime_type
        self._glob = glob
        self._config = config

    def _config_field(self, name: str) -> Any:
        """Read a top-level config field regardless of how the config was supplied.

        ``ExtractionConfig`` is a frozen dataclass, so its fields are attributes and
        mapping access raises. ``extract()`` also accepts a plain ``dict``, which
        reaches the loader unchanged, so both shapes are read here.
        """
        config = self._config
        if config is None:
            return None
        if isinstance(config, dict):
            return config.get(name)
        return getattr(config, name, None)

    @property
    def _per_page(self) -> bool:
        """Whether per-page splitting is enabled in the config."""
        pages = self._config_field("pages")
        if pages is None:
            return False
        if isinstance(pages, dict):
            return bool(pages.get("extract_pages"))
        return bool(getattr(pages, "extract_pages", None))

    @property
    def _chunking(self) -> bool:
        """Whether chunking is enabled in the config."""
        return self._config_field("chunking") is not None

    def _resolve_paths(self) -> Iterator[Path]:
        """Yield concrete file paths for the configured source."""
        file_path = self._file_path
        if isinstance(file_path, list):
            yield from file_path
        elif isinstance(file_path, Path):
            if file_path.is_dir():
                pattern = self._glob or "**/*"
                yield from sorted(p for p in file_path.glob(pattern) if p.is_file())
            else:
                yield file_path

    def _is_batch(self) -> bool:
        """Whether this load targets multiple inputs (list or directory)."""
        if self._data is not None:
            return False
        file_path = self._file_path
        if isinstance(file_path, list):
            return True
        return isinstance(file_path, Path) and file_path.is_dir()

    def _build_inputs(self) -> tuple[list[ExtractInput], list[str]]:
        """Build Xberg ExtractInput objects and their human-readable sources."""
        if self._data is not None:
            source = f"bytes://{self._mime_type}"
            extract_input = ExtractInput(kind="bytes", bytes=self._data, mime_type=self._mime_type)
            return [extract_input], [source]

        paths = list(self._resolve_paths())
        inputs = [ExtractInput(kind="uri", uri=str(path), mime_type=self._mime_type) for path in paths]
        sources = [str(path) for path in paths]
        return inputs, sources

    def _result_to_documents(self, result: ExtractionResult, sources: list[str]) -> Iterator[Document]:
        """Map an ExtractionResult envelope to LangChain Documents.

        Raises XbergError on the first per-input error, mirroring the fail-fast
        behaviour of single extraction. On success, ``result.results`` is aligned
        positionally with ``sources``.
        """
        if result.errors:
            error = result.errors[0]
            msg = f"Failed to extract '{error.source}': {error.message}"
            raise XbergError(msg)

        for index, document in enumerate(result.results):
            source = sources[index] if index < len(sources) else (sources[-1] if sources else "")
            yield from self._document_to_documents(document, source)

    def _document_to_documents(self, document: ExtractedDocument, source: str) -> Iterator[Document]:
        """Convert a single ExtractedDocument into one or more Documents.

        When chunking is enabled and the document was chunked, one Document is
        emitted per chunk. Otherwise, when per-page splitting is enabled, one
        Document is emitted per page. Failing both, the whole document becomes a
        single Document.
        """
        if self._chunking and document.chunks:
            yield from self._chunks_to_documents(document, source)
        elif self._per_page and document.pages:
            yield from self._pages_to_documents(document, source)
        else:
            metadata = self._build_metadata(document, source)
            page_content = self._assemble_content(document.content, document.tables)
            yield Document(page_content=page_content, metadata=metadata)

    def _chunks_to_documents(self, document: ExtractedDocument, source: str) -> Iterator[Document]:
        """Yield one Document per chunk from an ExtractedDocument.

        Chunk content is already segmented by Xberg, so it is used verbatim.
        Each Document carries the document-level metadata plus chunk-specific
        keys (index, heading path, page span, token count, chunk type).
        """
        base_metadata = self._build_metadata(document, source)

        for chunk in document.chunks or []:
            chunk_metadata = dict(base_metadata)
            meta = chunk.metadata
            chunk_metadata["chunk_index"] = meta.chunk_index
            chunk_metadata["total_chunks"] = meta.total_chunks
            chunk_metadata["chunk_type"] = str(chunk.chunk_type)
            if meta.heading_path:
                chunk_metadata["heading_path"] = list(meta.heading_path)
            if meta.token_count is not None:
                chunk_metadata["token_count"] = meta.token_count
            if meta.first_page is not None:
                # Xberg uses 1-indexed pages; LangChain convention is 0-indexed. ~keep
                chunk_metadata["page"] = meta.first_page - 1
                chunk_metadata["first_page"] = meta.first_page
            if meta.last_page is not None:
                chunk_metadata["last_page"] = meta.last_page

            yield Document(page_content=chunk.content, metadata=chunk_metadata)

    def _pages_to_documents(self, document: ExtractedDocument, source: str) -> Iterator[Document]:
        """Yield one Document per page from an ExtractedDocument."""
        base_metadata = self._build_metadata(document, source)

        for page in document.pages or []:
            page_metadata = dict(base_metadata)
            # Xberg uses 1-indexed pages; LangChain convention is 0-indexed. ~keep
            page_metadata["page"] = page.page_number - 1
            if page.is_blank is not None:
                page_metadata["is_blank"] = page.is_blank

            page_content = self._assemble_content(page.content, page.tables)
            yield Document(page_content=page_content, metadata=page_metadata)

    def _build_metadata(self, document: ExtractedDocument, source: str) -> dict[str, Any]:
        """Build a flat metadata dict from an ExtractedDocument."""
        metadata: dict[str, Any] = self._flatten_metadata(document.metadata)

        metadata["mime_type"] = document.mime_type
        if document.quality_score is not None:
            metadata["quality_score"] = document.quality_score
        if document.detected_languages:
            metadata["detected_languages"] = document.detected_languages
        if document.counts is not None:
            metadata["page_count"] = document.counts.pages

        if document.extracted_keywords:
            metadata["extracted_keywords"] = [
                {"text": keyword.text, "score": keyword.score, "algorithm": str(keyword.algorithm)}
                for keyword in document.extracted_keywords
            ]

        metadata["table_count"] = len(document.tables)
        if document.tables:
            metadata["tables"] = [
                {"cells": table.cells, "markdown": table.markdown, "page_number": table.page_number}
                for table in document.tables
            ]

        if document.processing_warnings:
            metadata["processing_warnings"] = [
                {"source": warning.source, "message": warning.message} for warning in document.processing_warnings
            ]

        metadata["source"] = source
        return metadata

    @staticmethod
    def _flatten_metadata(document_metadata: Any) -> dict[str, Any]:
        """Flatten the Xberg Metadata object into a dict of non-null scalar values.

        Works whether Metadata is exposed as a native object (attribute access) or a
        dataclass. Only JSON-friendly fields are copied; opaque nested objects are
        skipped. The free-form ``additional`` map is merged when present.
        """
        if document_metadata is None:
            return {}

        flat: dict[str, Any] = {}
        for name in _METADATA_FIELDS:
            value = getattr(document_metadata, name, None)
            if value is not None:
                flat[name] = value

        additional = getattr(document_metadata, "additional", None)
        if additional:
            flat["additional"] = dict(additional)

        return flat

    @staticmethod
    def _assemble_content(content: str, tables: Any) -> str:
        """Combine text content with table markdown."""
        if not tables:
            return content
        table_parts = [getattr(table, "markdown", "") for table in tables]
        parts = [part for part in table_parts if part]
        if not parts:
            return content
        return "\n\n".join([content, *parts])

    async def alazy_load(self) -> AsyncIterator[Document]:
        """Load documents asynchronously, yielding one Document at a time.

        Uses Xberg's native async extraction backed by Rust's tokio runtime. A
        single input uses ``extract``; multiple inputs use ``extract_batch`` for
        Rust-side concurrency.

        Yields:
            Document objects with extracted text and metadata.

        """
        inputs, sources = self._build_inputs()
        if not inputs:
            return

        try:
            if self._is_batch():
                result = await extract_batch(inputs, config=self._config)
            else:
                result = await extract(inputs[0], config=self._config)
        except (XbergError, OSError, RuntimeError) as exc:
            source = sources[0] if sources else "input"
            msg = f"Failed to extract '{source}': {exc}"
            raise XbergError(msg) from exc

        for document in self._result_to_documents(result, sources):
            yield document

    async def _collect(self) -> list[Document]:
        """Collect all Documents from the async loader into a list."""
        return [document async for document in self.alazy_load()]

    @staticmethod
    def _run_sync(factory: Callable[[], Coroutine[Any, Any, list[Document]]]) -> list[Document]:
        """Run an async coroutine to completion from synchronous code.

        Uses ``asyncio.run`` when no event loop is running. When already inside a
        running loop, the coroutine is run on a fresh loop in a worker thread to
        avoid "loop already running" errors.
        """
        try:
            asyncio.get_running_loop()
        except RuntimeError:
            return asyncio.run(factory())

        with ThreadPoolExecutor(max_workers=1) as pool:
            return pool.submit(lambda: asyncio.run(factory())).result()

    def lazy_load(self) -> Iterator[Document]:
        """Load documents lazily, yielding one Document at a time.

        Bridges to the async extraction core. Documents are materialized eagerly
        (extraction is a single batched call) then yielded one at a time.

        Yields:
            Document objects with extracted text and metadata.

        """
        documents = self._run_sync(self._collect)
        yield from documents
