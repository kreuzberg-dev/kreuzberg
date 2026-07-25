"""Shared test fixtures for XbergNodeParser tests."""

from typing import Any

from llama_index.core.schema import Document


def make_element(
    element_type: str = "narrative_text",
    text: str = "Some narrative text.",
    element_id: str = "el-001",
    page_number: int | None = 1,
    element_index: int | None = 0,
) -> dict[str, Any]:
    """Create a xberg Element dict matching the Element TypedDict shape."""
    return {
        "element_id": element_id,
        "element_type": element_type,
        "text": text,
        "metadata": {
            "page_number": page_number,
            "filename": "test.pdf",
            "coordinates": None,
            "element_index": element_index,
        },
    }


def make_xberg_document(
    elements: list[dict[str, Any]] | None = None,
    text: str = "Full document text.",
    doc_id: str = "doc-001",
) -> Document:
    """Create a Document with _xberg_elements metadata, matching XbergReader output."""
    if elements is None:
        elements = [
            make_element(element_type="title", text="Document Title", element_id="el-001", element_index=0),
            make_element(element_type="narrative_text", text="First paragraph.", element_id="el-002", element_index=1),
            make_element(
                element_type="table",
                text="| A | B |\n| 1 | 2 |",
                element_id="el-003",
                page_number=2,
                element_index=2,
            ),
        ]
    return Document(
        text=text,
        id_=doc_id,
        metadata={
            "_xberg_elements": elements,
            "file_path": "/tmp/test.pdf",
            "mime_type": "application/pdf",
        },
        excluded_llm_metadata_keys=["_xberg_elements"],
        excluded_embed_metadata_keys=["_xberg_elements"],
    )


def make_chunk(
    *,
    content: str = "Some chunk content.",
    chunk_type: str = "unknown",
    chunk_index: int = 0,
    total_chunks: int = 1,
    first_page: int | None = 1,
    last_page: int | None = 1,
    heading_path: list[str] | None = None,
    token_count: int | None = None,
) -> dict[str, Any]:
    """Create a xberg Chunk dict matching the reader's _xberg_chunks contract."""
    return {
        "content": content,
        "chunk_type": chunk_type,
        "metadata": {
            "chunk_index": chunk_index,
            "total_chunks": total_chunks,
            "first_page": first_page,
            "last_page": last_page,
            "heading_path": heading_path or [],
            "token_count": token_count,
        },
    }


def make_xberg_chunk_document(
    chunks: list[dict[str, Any]] | None = None,
    text: str = "Full document text.",
    doc_id: str = "doc-001",
) -> Document:
    """Create a Document with _xberg_chunks metadata, matching XbergReader output."""
    if chunks is None:
        chunks = [
            make_chunk(
                content="Introduction paragraph.",
                chunk_type="heading",
                chunk_index=0,
                total_chunks=2,
                heading_path=["Introduction"],
            ),
            make_chunk(
                content="Body paragraph on page two.",
                chunk_type="unknown",
                chunk_index=1,
                total_chunks=2,
                first_page=2,
                last_page=2,
                heading_path=["Introduction", "Details"],
                token_count=7,
            ),
        ]
    return Document(
        text=text,
        id_=doc_id,
        metadata={
            "_xberg_chunks": chunks,
            "file_path": "/tmp/test.pdf",
            "mime_type": "application/pdf",
        },
        excluded_llm_metadata_keys=["_xberg_chunks"],
        excluded_embed_metadata_keys=["_xberg_chunks"],
    )
