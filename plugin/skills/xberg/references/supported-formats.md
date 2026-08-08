<!--
AI-RULEZ :: GENERATED FILE — DO NOT EDIT
Content-Hash: blake3:ff19d1ca4cc6da797d5c6600d53cbb5404eba67965a3c8a840a49e1d87a02466
Source-Hash: blake3:5907a9cc29a5d72bbd3eaf5b820cac5133c8724895664c64fa8eafc2227716af
Schema-Version: v1
-->

# Supported Formats Reference

Xberg supports 101 file formats across 115 file extensions in 8 major categories with intelligent format detection and comprehensive metadata extraction. All formats support text and metadata extraction. Additional capabilities like OCR and table detection are noted per format.

## Office Documents

### Word Processing

| Format             | Extensions               | MIME Type                                                                 | Capabilities                                                    |
| ------------------ | ------------------------ | ------------------------------------------------------------------------- | --------------------------------------------------------------- |
| Microsoft Word     | `.docx`                  | `application/vnd.openxmlformats-officedocument.wordprocessingml.document` | Full text extraction, tables, embedded images, metadata, styles |
| Word Macro-Enabled | `.docm`                  | `application/vnd.ms-word.document.macroEnabled.12`                        | Macro-enabled document extraction, metadata                     |
| Word Template      | `.dotx`, `.dotm`, `.dot` | Various Word template MIME types                                          | Template document extraction, metadata                          |
| OpenDocument Text  | `.odt`                   | `application/vnd.oasis.opendocument.text`                                 | Full text extraction, tables, embedded images, metadata, styles |

### Spreadsheets

| Format                   | Extensions | MIME Type                                                              | Capabilities                                             |
| ------------------------ | ---------- | ---------------------------------------------------------------------- | -------------------------------------------------------- |
| Excel Workbook           | `.xlsx`    | `application/vnd.openxmlformats-officedocument.spreadsheetml.sheet`    | Sheet data, cell values, formulas, cell metadata, charts |
| Excel Macro-Enabled      | `.xlsm`    | `application/vnd.ms-excel.sheet.macroEnabled.12`                       | Sheet data, formulas, macros (text only), metadata       |
| Excel Binary             | `.xlsb`    | `application/vnd.ms-excel.sheet.binary.macroEnabled.12`                | Binary sheet data extraction, metadata                   |
| Excel Legacy             | `.xls`     | `application/vnd.ms-excel`                                             | Legacy sheet data extraction, metadata                   |
| Excel Add-in             | `.xla`     | `application/vnd.ms-excel.template.macroEnabled.12`                    | Add-in data extraction                                   |
| Excel Macro Add-in       | `.xlam`    | `application/vnd.ms-excel.addin.macroEnabled.12`                       | Macro add-in metadata                                    |
| Excel Template (XML)     | `.xltx`    | `application/vnd.openxmlformats-officedocument.spreadsheetml.template` | XML template data and metadata                           |
| Excel Template (Legacy)  | `.xlt`     | `application/vnd.ms-excel`                                             | Legacy template data extraction                          |
| OpenDocument Spreadsheet | `.ods`     | `application/vnd.oasis.opendocument.spreadsheet`                       | Sheet data, cell values, formulas, metadata              |

### Presentations

| Format                  | Extensions               | MIME Type                                                                   | Capabilities                                         |
| ----------------------- | ------------------------ | --------------------------------------------------------------------------- | ---------------------------------------------------- |
| PowerPoint Presentation | `.pptx`                  | `application/vnd.openxmlformats-officedocument.presentationml.presentation` | Slide text, speaker notes, embedded images, metadata |
| PowerPoint Legacy       | `.ppt`                   | `application/vnd.ms-powerpoint`                                             | Legacy slide text extraction, metadata               |
| PowerPoint Slideshow    | `.ppsx`                  | `application/vnd.openxmlformats-officedocument.presentationml.slideshow`    | Slideshow content, speaker notes, metadata           |
| PowerPoint Template     | `.potx`, `.potm`, `.pot` | Various PowerPoint template MIME types                                      | Template slide extraction, metadata                  |

### PDF

| Format                   | Extensions | MIME Type         | Capabilities                                                                                       |
| ------------------------ | ---------- | ----------------- | -------------------------------------------------------------------------------------------------- |
| Portable Document Format | `.pdf`     | `application/pdf` | Text extraction, tables, embedded images, metadata, OCR (when needed), password protection support |

### eBooks

| Format      | Extensions | MIME Type                       | Capabilities                                           |
| ----------- | ---------- | ------------------------------- | ------------------------------------------------------ |
| EPUB        | `.epub`    | `application/epub+zip`          | Chapter text, metadata, embedded resources, navigation |
| FictionBook | `.fb2`     | `application/x-fictionbook+xml` | Book content, metadata, chapter structure              |

### Database

| Format | Extensions | MIME Type           | Capabilities                                          |
| ------ | ---------- | ------------------- | ----------------------------------------------------- |
| dBASE  | `.dbf`     | `application/x-dbf` | Table data extraction as markdown, field type support |

### Hangul

| Format                | Extensions      | MIME Type                                       | Capabilities                            |
| --------------------- | --------------- | ----------------------------------------------- | --------------------------------------- |
| Hangul Word Processor | `.hwp`, `.hwpx` | `application/x-hwp`, `application/haansofthwpx` | Korean document format, text extraction |

## Images (OCR-Enabled)

### Raster Images

| Format | Extensions      | MIME Type    | Capabilities                                                                 |
| ------ | --------------- | ------------ | ---------------------------------------------------------------------------- |
| PNG    | `.png`          | `image/png`  | OCR text extraction, table detection, EXIF metadata, dimensions, color space |
| JPEG   | `.jpg`, `.jpeg` | `image/jpeg` | OCR text extraction, table detection, EXIF metadata, color profile           |
| GIF    | `.gif`          | `image/gif`  | OCR text extraction, animation metadata, dimensions                          |
| WebP   | `.webp`         | `image/webp` | OCR text extraction, metadata, lossy/lossless detection                      |
| Bitmap | `.bmp`          | `image/bmp`  | OCR text extraction, dimensions, color depth                                 |
| TIFF   | `.tiff`, `.tif` | `image/tiff` | OCR text extraction, multi-page support, EXIF metadata, compression info     |

### Advanced Image Formats

| Format             | Extensions                     | MIME Type                 | Capabilities                                                                     |
| ------------------ | ------------------------------ | ------------------------- | -------------------------------------------------------------------------------- |
| JPEG 2000          | `.jp2`                         | `image/jp2`               | OCR via pure Rust decoder (hayro-jpeg2000), table detection, resolution metadata |
| JPEG 2000 Extended | `.jpx`                         | `image/jpx`               | Advanced JPEG 2000 features, high-resolution content, metadata                   |
| JPEG 2000 Compound | `.jpm`                         | `image/jpm`               | Compound image support, mixed content                                            |
| Motion JPEG 2000   | `.mj2`                         | `image/mj2`               | JPEG 2000 video/sequence metadata                                                |
| JBIG2              | `.jbig2`, `.jb2`               | `image/x-jbig2`           | Bi-level image OCR, high compression, technical documents                        |
| Portable PixMap    | `.pnm`, `.pbm`, `.pgm`, `.ppm` | `.pnm`=`image/x-portable-anymap`, `.pbm`=`image/x-portable-bitmap`, `.pgm`=`image/x-portable-graymap`, `.ppm`=`image/x-portable-pixmap` | OCR for plain image formats, raw pixel data                                      |

### Vector Graphics

| Format                   | Extensions | MIME Type       | Capabilities                                                              |
| ------------------------ | ---------- | --------------- | ------------------------------------------------------------------------- |
| Scalable Vector Graphics | `.svg`     | `image/svg+xml` | DOM parsing, embedded text extraction, graphics metadata, vector elements |

## Web & Data

### Markup & Structured Text

| Format           | Extensions      | MIME Type               | Capabilities                                                                       |
| ---------------- | --------------- | ----------------------- | ---------------------------------------------------------------------------------- |
| HyperText Markup | `.html`, `.htm` | `text/html` (alias `application/xhtml+xml`) | DOM parsing, text extraction, metadata (Open Graph, Twitter Card), link extraction |
| XML              | `.xml`          | `application/xml`       | DOM parsing, namespace handling, text extraction, structure analysis               |

### Structured Data Formats

| Format | Extensions      | MIME Type                   | Capabilities                                               |
| ------ | --------------- | --------------------------- | ---------------------------------------------------------- |
| JSON   | `.json`         | `application/json`          | Schema detection, nested structure parsing, validation     |
| YAML   | `.yaml`, `.yml` | `application/x-yaml`        | Hierarchical data parsing, custom tags, nested structures  |
| TOML   | `.toml`         | `application/toml`          | Configuration parsing, table structures, type preservation |
| CSV    | `.csv`          | `text/csv`                  | Delimiter detection, header inference, type detection      |
| TSV    | `.tsv`          | `text/tab-separated-values` | Tab-separated value parsing, header detection              |

### Text & Markup Languages

| Format           | Extensions         | MIME Type         | Capabilities                                      |
| ---------------- | ------------------ | ----------------- | ------------------------------------------------- |
| Plain Text       | `.txt`             | `text/plain`      | Raw text extraction, encoding detection           |
| Markdown         | `.md`, `.markdown` | `text/markdown`   | CommonMark parsing, GFM extensions, front matter  |
| Djot             | `.djot`            | `text/djot`       | Djot format parsing, semantic structure           |
| reStructuredText | `.rst`             | `text/x-rst`      | RST parsing, directive handling, role extraction  |
| Org Mode         | `.org`             | `text/org`        | Org mode structure, outline parsing, metadata     |
| Rich Text Format | `.rtf`             | `application/rtf` | Text with formatting extraction, font information |

## Email & Archives

### Email Formats

| Format            | Extensions | MIME Type                    | Capabilities                                                                           |
| ----------------- | ---------- | ---------------------------- | -------------------------------------------------------------------------------------- |
| Email Message     | `.eml`     | `message/rfc822`             | Headers (from, to, subject, date), body (HTML/plain text), attachments, threading info |
| Microsoft Outlook | `.msg`     | `application/vnd.ms-outlook` | Outlook headers, body content, attachments, recipient metadata                         |

### Archive Formats

| Format      | Extensions | MIME Type                     | Capabilities                                               |
| ----------- | ---------- | ----------------------------- | ---------------------------------------------------------- |
| ZIP Archive | `.zip`     | `application/zip`             | File listing, nested archive support, compression metadata |
| Tar Archive | `.tar`     | `application/x-tar`           | File listing, permission metadata, nested archives         |
| Gzip Tar    | `.tgz`     | `application/gzip`            | Compressed archive listing, metadata                       |
| Gzip        | `.gz`      | `application/gzip`            | Compressed file metadata                                   |
| 7-Zip       | `.7z`      | `application/x-7z-compressed` | File listing, compression info, nested archives            |

## Academic & Scientific

### Citation Formats

| Format                  | Extensions  | MIME Type                                | Capabilities                                      |
| ----------------------- | ----------- | ---------------------------------------- | ------------------------------------------------- |
| BibTeX                  | `.bib`      | `application/x-bibtex`                    | Structured parsing, entry types, field extraction |
| BibLaTeX                | `.bib`      | `application/x-biblatex`                  | Extended BibTeX format, advanced field support    |
| RIS                     | `.ris`      | `application/x-research-info-systems`    | Structured RIS format parsing, type detection     |
| NIH RIS                 | `.nbib`     | `application/x-pubmed`                    | NIH/PubMed format, structured citation data       |
| EndNote                 | `.enw`      | `application/x-endnote+xml`              | EndNote XML format, citation metadata             |
| Citation Style Language | —           | `application/csl+json`                   | CSL JSON parsing, style definitions               |

### Scientific & Technical Formats

| Format           | Extensions       | MIME Type                  | Capabilities                                                |
| ---------------- | ---------------- | -------------------------- | ----------------------------------------------------------- |
| LaTeX            | `.tex`, `.latex` | `application/x-latex`      | LaTeX source parsing, commands, document structure          |
| Typst            | `.typ`           | `application/x-typst`      | Typst markup parsing, document structure                    |
| JATS XML         | `.jats`          | `application/x-jats+xml`   | PubMed JATS parsing, article structure, metadata            |
| Jupyter Notebook | `.ipynb`         | `application/x-ipynb+json` | Cell extraction (code + markdown), output parsing, metadata |
| DocBook          | `.docbook`       | `application/docbook+xml`  | DocBook XML parsing, semantic structure                     |

### Documentation Formats

| Format      | Extensions | MIME Type                | Capabilities                                    |
| ----------- | ---------- | ------------------------ | ----------------------------------------------- |
| OPML        | `.opml`    | `application/x-opml+xml` | Outline parsing, hierarchy extraction, metadata |
| Perl POD    | —          | `text/x-pod`             | Perl documentation parsing, section extraction  |
| Manual Page | —          | `text/x-mdoc`            | UNIX manual page parsing, section structure     |
| Troff/Groff | —          | `text/troff`             | Typesetting markup parsing, document structure  |

## Format Capabilities Summary

### Text Extraction

All 101 formats support full or partial text extraction. Document structure and encoding are automatically detected.

### Metadata Support

Comprehensive metadata extraction includes:

- Document properties (title, author, subject, creation date, modification date)
- Format-specific metadata (page count, dimensions, encoding, language)
- EXIF data (for images)
- Document statistics (word count, character count)

### OCR (Optical Character Recognition)

OCR is available for image formats:

- **Raster Images**: PNG, JPEG, GIF, WebP, BMP, TIFF
- **Advanced Formats**: JPEG 2000, JBIG2, PNM/PBM/PGM/PPM
- **Configurable Backends**: Tesseract (default), PaddleOCR (`paddleocr`/`paddle-ocr`), VLM (Candle-based: TrOCR, PaddleOCR-VL, GLM-OCR, DeepSeek-OCR)

### Table Detection

Smart table detection and reconstruction available for:

- PDF documents (native tables and scanned content with OCR)
- Office documents (Excel, Word)
- Images (via OCR backends)
- HTML/XML (from markup structure)

### Archive & Nested Document Support

Archives and nested formats support file listing and sequential extraction:

- ZIP, TAR, TGZ, 7Z archives
- Email attachments
- Nested archives within archives

## Getting Started

For language-specific examples and detailed API documentation, see the [API Reference](https://docs.xberg.io/reference/api-python/).

For OCR configuration and backend selection, see the [OCR Backends Guide](https://docs.xberg.io/guides/ocr/).

For comprehensive format details and format detection, see the [Complete Format Reference](https://docs.xberg.io/reference/formats/).
