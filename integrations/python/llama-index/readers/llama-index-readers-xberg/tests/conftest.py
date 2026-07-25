"""Shared test fixtures for XbergReader tests.

xberg returns attribute-based objects, so the fixtures here build lightweight
stand-ins with ``SimpleNamespace`` (attribute access) rather than dicts.
"""

from types import SimpleNamespace
from typing import Any

_METADATA_DEFAULTS = {
    "title": "Test Document",
    "subject": None,
    "authors": ["Author One"],
    "keywords": ["test"],
    "language": "eng",
    "created_at": "2026-01-01T00:00:00Z",
    "modified_at": None,
    "created_by": "TestApp",
    "modified_by": None,
    "category": None,
    "tags": None,
    "document_version": None,
    "abstract_text": None,
    "output_format": "plain",
}


def make_metadata(**overrides: Any) -> SimpleNamespace:
    """Build a stand-in for xberg ``Metadata`` (attribute access)."""
    fields = {**_METADATA_DEFAULTS, **overrides}
    return SimpleNamespace(**fields)


def make_element(
    element_type: str = "narrative_text",
    text: str = "Some text.",
    page_number: int | None = 1,
    element_index: int | None = 0,
) -> SimpleNamespace:
    """Build a stand-in for xberg ``Element``."""
    return SimpleNamespace(
        element_type=element_type,
        text=text,
        metadata=SimpleNamespace(page_number=page_number, element_index=element_index),
    )


def make_chunk(
    *,
    content: str = "Chunk content.",
    chunk_type: str = "unknown",
    chunk_index: int = 0,
    total_chunks: int = 1,
    first_page: int | None = 1,
    last_page: int | None = 1,
    heading_path: list[str] | None = None,
    token_count: int | None = None,
) -> SimpleNamespace:
    """Build a stand-in for xberg ``Chunk`` (attribute access)."""
    return SimpleNamespace(
        content=content,
        chunk_type=chunk_type,
        metadata=SimpleNamespace(
            chunk_index=chunk_index,
            total_chunks=total_chunks,
            first_page=first_page,
            last_page=last_page,
            heading_path=heading_path or [],
            token_count=token_count,
        ),
    )


def make_page_content(
    page_number: int = 1,
    content: str = "Page content",
    tables: list[Any] | None = None,
) -> SimpleNamespace:
    """Build a stand-in for xberg ``PageContent`` (attribute access, no images)."""
    return SimpleNamespace(page_number=page_number, content=content, tables=tables or [])


def make_document(
    *,
    content: str = "Hello world",
    mime_type: str = "application/pdf",
    metadata: SimpleNamespace | None = None,
    pages: list[Any] | None = None,
    elements: list[Any] | None = None,
    tables: list[Any] | None = None,
    images: list[Any] | None = None,
    chunks: list[Any] | None = None,
    quality_score: float | None = 0.95,
    detected_languages: list[str] | None = None,
    extracted_keywords: list[Any] | None = None,
    processing_warnings: list[Any] | None = None,
    annotations: list[Any] | None = None,
    page_count: int = 1,
) -> SimpleNamespace:
    """Build a stand-in for xberg ``ExtractedDocument``."""
    return SimpleNamespace(
        content=content,
        mime_type=mime_type,
        metadata=metadata or make_metadata(),
        counts=SimpleNamespace(pages=page_count),
        tables=tables or [],
        pages=pages,
        elements=elements,
        chunks=chunks,
        images=images,
        quality_score=quality_score,
        detected_languages=detected_languages or ["eng"],
        extracted_keywords=extracted_keywords or [],
        processing_warnings=processing_warnings or [],
        annotations=annotations,
    )


def make_error(index: int = 0, error_type: str = "other", message: str = "extraction failed") -> SimpleNamespace:
    """Build a stand-in for xberg ``ExtractionErrorItem``."""
    return SimpleNamespace(index=index, error_type=error_type, message=message)


def make_result(
    documents: list[SimpleNamespace] | None = None,
    errors: list[SimpleNamespace] | None = None,
) -> SimpleNamespace:
    """Build a stand-in for xberg ``ExtractionResult``."""
    return SimpleNamespace(results=documents or [], errors=errors or [])
