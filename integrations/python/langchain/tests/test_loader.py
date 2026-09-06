"""Synchronous test suite for XbergLoader."""

from pathlib import Path
from unittest.mock import AsyncMock, patch

import pytest
from xberg import ChunkingConfig, ExtractionConfig, OcrConfig, PageConfig, XbergError

from langchain_xberg import XbergLoader
from tests.conftest import (
    make_chunk,
    make_document,
    make_error,
    make_keyword,
    make_page,
    make_result,
    make_table,
)


def test_no_input_raises() -> None:
    with pytest.raises(ValueError, match="Either 'file_path' or 'data'"):
        XbergLoader()


def test_both_inputs_raises() -> None:
    with pytest.raises(ValueError, match="Cannot specify both"):
        XbergLoader(file_path="test.pdf", data=b"test")


def test_bytes_requires_mime_type() -> None:
    with pytest.raises(ValueError, match="'mime_type' is required"):
        XbergLoader(data=b"test")


def test_valid_file_path() -> None:
    loader = XbergLoader(file_path="test.pdf")
    assert loader._file_path == Path("test.pdf")


def test_valid_bytes_input() -> None:
    loader = XbergLoader(data=b"test", mime_type="text/plain")
    assert loader._data == b"test"
    assert loader._mime_type == "text/plain"


def test_valid_multiple_files() -> None:
    loader = XbergLoader(file_path=["a.pdf", "b.docx"])
    assert loader._file_path == [Path("a.pdf"), Path("b.docx")]


def test_valid_path_object() -> None:
    loader = XbergLoader(file_path=Path("test.pdf"))
    assert loader._file_path == Path("test.pdf")


def test_default_config_is_none() -> None:
    loader = XbergLoader(file_path="test.pdf")
    assert loader._config is None


def test_custom_config_passthrough() -> None:
    custom_config = ExtractionConfig(
        output_format="html",
        force_ocr=True,
        ocr=OcrConfig(backend="paddleocr"),
    )
    loader = XbergLoader(file_path="test.pdf", config=custom_config)
    assert loader._config is custom_config
    assert loader._config.output_format == "html"


def test_per_page_flag_from_config() -> None:
    config = ExtractionConfig(pages=PageConfig(extract_pages=True))
    assert XbergLoader(file_path="doc.pdf", config=config)._per_page is True
    assert XbergLoader(file_path="doc.pdf")._per_page is False


def test_chunking_flag_from_config() -> None:
    config = ExtractionConfig(chunking=ChunkingConfig(max_characters=500))
    assert XbergLoader(file_path="doc.pdf", config=config)._chunking is True
    assert XbergLoader(file_path="doc.pdf")._chunking is False


@patch("langchain_xberg.loader.extract", new_callable=AsyncMock)
def test_load_single_text_file(mock_extract: AsyncMock, sample_txt_path: Path) -> None:
    mock_extract.return_value = make_result([make_document(content="Sample text content")])

    loader = XbergLoader(file_path=str(sample_txt_path))
    docs = loader.load()

    assert len(docs) == 1
    assert docs[0].page_content == "Sample text content"
    assert docs[0].metadata["source"] == str(sample_txt_path)
    mock_extract.assert_called_once()


@patch("langchain_xberg.loader.extract", new_callable=AsyncMock)
def test_load_single_pdf(mock_extract: AsyncMock) -> None:
    mock_extract.return_value = make_result(
        [
            make_document(
                content="PDF content",
                mime_type="application/pdf",
                metadata={"title": "Report", "output_format": "markdown"},
                page_count=3,
            )
        ]
    )

    loader = XbergLoader(file_path="document.pdf")
    docs = loader.load()

    assert len(docs) == 1
    assert docs[0].page_content == "PDF content"
    assert docs[0].metadata["mime_type"] == "application/pdf"
    assert docs[0].metadata["title"] == "Report"
    assert docs[0].metadata["output_format"] == "markdown"
    assert docs[0].metadata["page_count"] == 3


@patch("langchain_xberg.loader.extract", new_callable=AsyncMock)
def test_load_bytes_mode(mock_extract: AsyncMock) -> None:
    mock_extract.return_value = make_result([make_document(content="Bytes content")])

    loader = XbergLoader(data=b"raw data", mime_type="text/plain")
    docs = loader.load()

    assert len(docs) == 1
    assert docs[0].page_content == "Bytes content"
    assert docs[0].metadata["source"] == "bytes://text/plain"
    mock_extract.assert_called_once()


@patch("langchain_xberg.loader.extract_batch", new_callable=AsyncMock)
def test_load_multiple_files(mock_batch: AsyncMock) -> None:
    mock_batch.return_value = make_result([make_document(), make_document(), make_document()])

    loader = XbergLoader(file_path=["a.txt", "b.txt", "c.txt"])
    docs = loader.load()

    assert len(docs) == 3
    mock_batch.assert_called_once()
    sources = [d.metadata["source"] for d in docs]
    assert sources == ["a.txt", "b.txt", "c.txt"]


@patch("langchain_xberg.loader.extract_batch", new_callable=AsyncMock)
def test_load_directory_with_glob(mock_batch: AsyncMock, tmp_dir_with_files: Path) -> None:
    mock_batch.return_value = make_result([make_document(), make_document()])

    loader = XbergLoader(file_path=str(tmp_dir_with_files), glob="*.txt")
    docs = loader.load()

    assert len(docs) == 2
    mock_batch.assert_called_once()


@patch("langchain_xberg.loader.extract_batch", new_callable=AsyncMock)
def test_load_directory_default_glob(mock_batch: AsyncMock, tmp_dir_with_files: Path) -> None:
    mock_batch.return_value = make_result([make_document(), make_document(), make_document()])

    loader = XbergLoader(file_path=str(tmp_dir_with_files))
    docs = loader.load()

    assert len(docs) == 3
    mock_batch.assert_called_once()


def test_load_empty_directory(tmp_path: Path) -> None:
    loader = XbergLoader(file_path=str(tmp_path))
    docs = loader.load()

    assert len(docs) == 0


@patch("langchain_xberg.loader.extract", new_callable=AsyncMock)
def test_per_page_splitting(mock_extract: AsyncMock) -> None:
    mock_extract.return_value = make_result(
        [
            make_document(
                pages=[
                    make_page(1, "Page 1 text", is_blank=False),
                    make_page(2, "Page 2 text", is_blank=False),
                    make_page(3, "", is_blank=True),
                ],
                page_count=3,
            )
        ]
    )

    config = ExtractionConfig(pages=PageConfig(extract_pages=True))
    loader = XbergLoader(file_path="doc.pdf", config=config)
    docs = loader.load()

    assert len(docs) == 3
    assert docs[0].page_content == "Page 1 text"
    assert docs[1].page_content == "Page 2 text"
    assert docs[2].page_content == ""


@patch("langchain_xberg.loader.extract", new_callable=AsyncMock)
def test_per_page_metadata(mock_extract: AsyncMock) -> None:
    mock_extract.return_value = make_result(
        [
            make_document(
                pages=[
                    make_page(1, "Page 1", is_blank=False),
                    make_page(2, "Page 2", is_blank=True),
                ],
                page_count=2,
            )
        ]
    )

    config = ExtractionConfig(pages=PageConfig(extract_pages=True))
    loader = XbergLoader(file_path="doc.pdf", config=config)
    docs = loader.load()

    # Page numbers are 0-indexed in LangChain convention ~keep
    assert docs[0].metadata["page"] == 0
    assert docs[0].metadata["is_blank"] is False
    assert docs[1].metadata["page"] == 1
    assert docs[1].metadata["is_blank"] is True


@patch("langchain_xberg.loader.extract", new_callable=AsyncMock)
def test_per_page_with_tables(mock_extract: AsyncMock) -> None:
    page = make_page(1, "Text", tables=[make_table(markdown="| X |\n|---|\n| Y |")], is_blank=False)
    mock_extract.return_value = make_result([make_document(pages=[page], page_count=1)])

    config = ExtractionConfig(pages=PageConfig(extract_pages=True))
    loader = XbergLoader(file_path="doc.pdf", config=config)
    docs = loader.load()

    assert "| X |" in docs[0].page_content


@patch("langchain_xberg.loader.extract", new_callable=AsyncMock)
def test_per_page_fallback_when_no_pages(mock_extract: AsyncMock) -> None:
    """When per_page is configured but the document has no pages, use the whole document."""
    mock_extract.return_value = make_result([make_document(content="Whole document", pages=None)])

    config = ExtractionConfig(pages=PageConfig(extract_pages=True))
    loader = XbergLoader(file_path="doc.txt", config=config)
    docs = loader.load()

    assert len(docs) == 1
    assert docs[0].page_content == "Whole document"


@patch("langchain_xberg.loader.extract", new_callable=AsyncMock)
def test_chunking_one_document_per_chunk(mock_extract: AsyncMock) -> None:
    chunks = [
        make_chunk("First chunk", chunk_index=0, total_chunks=2, heading_path=["Intro"], first_page=1),
        make_chunk("Second chunk", chunk_index=1, total_chunks=2, heading_path=["Body"], first_page=2),
    ]
    mock_extract.return_value = make_result([make_document(content="whole doc", chunks=chunks)])

    config = ExtractionConfig(chunking=ChunkingConfig(max_characters=100))
    loader = XbergLoader(file_path="doc.pdf", config=config)
    docs = loader.load()

    assert len(docs) == 2
    assert docs[0].page_content == "First chunk"
    assert docs[1].page_content == "Second chunk"


@patch("langchain_xberg.loader.extract", new_callable=AsyncMock)
def test_chunk_metadata(mock_extract: AsyncMock) -> None:
    chunk = make_chunk(
        "Chunk text",
        chunk_index=3,
        total_chunks=10,
        heading_path=["Chapter 1", "Section 2"],
        first_page=4,
        last_page=5,
        token_count=42,
    )
    mock_extract.return_value = make_result([make_document(chunks=[chunk])])

    config = ExtractionConfig(chunking=ChunkingConfig(max_characters=100))
    loader = XbergLoader(file_path="doc.pdf", config=config)
    docs = loader.load()

    meta = docs[0].metadata
    assert meta["chunk_index"] == 3
    assert meta["total_chunks"] == 10
    assert meta["heading_path"] == ["Chapter 1", "Section 2"]
    assert meta["token_count"] == 42
    # 1-indexed first_page becomes 0-indexed "page" per LangChain convention. ~keep
    assert meta["page"] == 3
    assert meta["first_page"] == 4
    assert meta["last_page"] == 5
    assert meta["source"] == "doc.pdf"
    assert "chunk_type" in meta


@patch("langchain_xberg.loader.extract", new_callable=AsyncMock)
def test_chunk_empty_heading_path_omitted(mock_extract: AsyncMock) -> None:
    chunk = make_chunk("Text", chunk_index=0, total_chunks=1, heading_path=[])
    mock_extract.return_value = make_result([make_document(chunks=[chunk])])

    config = ExtractionConfig(chunking=ChunkingConfig(max_characters=100))
    loader = XbergLoader(file_path="doc.pdf", config=config)
    docs = loader.load()

    # Empty heading path / unset page span / token count are dropped, not surfaced. ~keep
    assert "heading_path" not in docs[0].metadata
    assert "page" not in docs[0].metadata
    assert "token_count" not in docs[0].metadata


@patch("langchain_xberg.loader.extract", new_callable=AsyncMock)
def test_chunking_fallback_when_no_chunks(mock_extract: AsyncMock) -> None:
    """When chunking is configured but the document has no chunks, use the whole document."""
    mock_extract.return_value = make_result([make_document(content="Whole document", chunks=None)])

    config = ExtractionConfig(chunking=ChunkingConfig(max_characters=100))
    loader = XbergLoader(file_path="doc.txt", config=config)
    docs = loader.load()

    assert len(docs) == 1
    assert docs[0].page_content == "Whole document"


@patch("langchain_xberg.loader.extract", new_callable=AsyncMock)
def test_metadata_source_key(mock_extract: AsyncMock) -> None:
    mock_extract.return_value = make_result([make_document()])

    loader = XbergLoader(file_path="doc.txt")
    docs = loader.load()

    assert docs[0].metadata["source"] == "doc.txt"


@patch("langchain_xberg.loader.extract", new_callable=AsyncMock)
def test_metadata_flattening(mock_extract: AsyncMock) -> None:
    mock_extract.return_value = make_result(
        [
            make_document(
                metadata={"title": "Test Doc", "authors": ["Alice", "Bob"], "language": "en"},
            )
        ]
    )

    loader = XbergLoader(file_path="doc.txt")
    docs = loader.load()

    meta = docs[0].metadata
    assert meta["title"] == "Test Doc"
    assert meta["authors"] == ["Alice", "Bob"]
    assert meta["language"] == "en"
    # Unset (None) metadata fields are dropped, not surfaced as keys. ~keep
    assert "subject" not in meta
    assert "keywords" not in meta


@patch("langchain_xberg.loader.extract", new_callable=AsyncMock)
def test_metadata_enrichment(mock_extract: AsyncMock) -> None:
    mock_extract.return_value = make_result(
        [
            make_document(
                metadata={"output_format": "markdown"},
                quality_score=0.85,
                detected_languages=["eng", "deu"],
                page_count=1,
            )
        ]
    )

    loader = XbergLoader(file_path="doc.txt")
    docs = loader.load()

    meta = docs[0].metadata
    assert meta["quality_score"] == 0.85
    assert meta["detected_languages"] == ["eng", "deu"]
    assert meta["output_format"] == "markdown"
    assert meta["mime_type"] == "text/plain"
    assert meta["page_count"] == 1


@patch("langchain_xberg.loader.extract", new_callable=AsyncMock)
def test_extracted_keywords_in_metadata(mock_extract: AsyncMock) -> None:
    keywords = [
        make_keyword(text="python", score=0.95, algorithm="yake"),
        make_keyword(text="machine learning", score=0.88, algorithm="yake"),
    ]
    mock_extract.return_value = make_result([make_document(extracted_keywords=keywords)])

    loader = XbergLoader(file_path="doc.txt")
    docs = loader.load()

    keywords_meta = docs[0].metadata["extracted_keywords"]
    assert len(keywords_meta) == 2
    assert keywords_meta[0] == {"text": "python", "score": pytest.approx(0.95), "algorithm": "yake"}
    assert keywords_meta[1]["text"] == "machine learning"


@patch("langchain_xberg.loader.extract", new_callable=AsyncMock)
def test_processing_warnings_in_metadata(mock_extract: AsyncMock) -> None:
    mock_extract.return_value = make_result(
        [make_document(processing_warnings=["Low quality scan detected", "Missing font fallback"])]
    )

    loader = XbergLoader(file_path="doc.txt")
    docs = loader.load()

    warnings = docs[0].metadata["processing_warnings"]
    assert len(warnings) == 2
    assert warnings[0] == {"source": "extraction", "message": "Low quality scan detected"}
    assert warnings[1] == {"source": "extraction", "message": "Missing font fallback"}


@patch("langchain_xberg.loader.extract", new_callable=AsyncMock)
def test_table_extraction_in_content(mock_extract: AsyncMock) -> None:
    table = make_table(markdown="| Col1 | Col2 |\n|---|---|\n| A | B |")
    mock_extract.return_value = make_result([make_document(content="Main text", tables=[table])])

    loader = XbergLoader(file_path="doc.pdf")
    docs = loader.load()

    assert "Main text" in docs[0].page_content
    assert "| Col1 | Col2 |" in docs[0].page_content


@patch("langchain_xberg.loader.extract", new_callable=AsyncMock)
def test_table_extraction_in_metadata(mock_extract: AsyncMock) -> None:
    table = make_table(cells=[["A", "B"], ["1", "2"]], markdown="| A | B |\n|---|---|\n| 1 | 2 |", page_number=1)
    mock_extract.return_value = make_result([make_document(tables=[table])])

    loader = XbergLoader(file_path="doc.pdf")
    docs = loader.load()

    meta = docs[0].metadata
    assert meta["table_count"] == 1
    assert len(meta["tables"]) == 1
    assert meta["tables"][0]["cells"] == [["A", "B"], ["1", "2"]]
    assert meta["tables"][0]["page_number"] == 1


@patch("langchain_xberg.loader.extract", new_callable=AsyncMock)
def test_multiple_tables_in_content(mock_extract: AsyncMock) -> None:
    tables = [make_table(markdown="| T1 |"), make_table(markdown="| T2 |")]
    mock_extract.return_value = make_result([make_document(content="Text", tables=tables)])

    loader = XbergLoader(file_path="doc.pdf")
    docs = loader.load()

    assert docs[0].page_content == "Text\n\n| T1 |\n\n| T2 |"


@patch("langchain_xberg.loader.extract", new_callable=AsyncMock)
def test_error_propagation(mock_extract: AsyncMock) -> None:
    mock_extract.side_effect = XbergError("Extraction failed")

    loader = XbergLoader(file_path="bad.pdf")

    with pytest.raises(XbergError, match=r"Failed to extract 'bad\.pdf'"):
        loader.load()


@patch("langchain_xberg.loader.extract_batch", new_callable=AsyncMock)
def test_batch_error_propagation(mock_batch: AsyncMock) -> None:
    mock_batch.return_value = make_result(
        documents=[make_document()],
        errors=[make_error(index=1, source="bad.xyz", message="unsupported format")],
    )

    loader = XbergLoader(file_path=["good.txt", "bad.xyz"])

    with pytest.raises(XbergError, match=r"Failed to extract 'bad\.xyz'"):
        loader.load()


@patch("langchain_xberg.loader.extract", new_callable=AsyncMock)
def test_lazy_load_is_iterator(mock_extract: AsyncMock) -> None:
    mock_extract.return_value = make_result([make_document()])

    loader = XbergLoader(file_path="doc.txt")
    result = loader.lazy_load()

    assert hasattr(result, "__next__")


@patch("langchain_xberg.loader.extract_batch", new_callable=AsyncMock)
def test_lazy_load_yields_documents(mock_batch: AsyncMock) -> None:
    mock_batch.return_value = make_result([make_document(), make_document()])

    loader = XbergLoader(file_path=["a.txt", "b.txt"])

    docs = list(loader.lazy_load())

    assert len(docs) == 2
