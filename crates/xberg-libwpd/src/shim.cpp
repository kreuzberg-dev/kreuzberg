/* Flat C shim over libwpd + librevenge for Xberg.
 *
 * libwpd exposes no `extract()` call. It drives librevenge's SAX-like
 * RVNGTextInterface: the caller passes a concrete implementation into
 * WPDocument::parse and libwpd invokes its callbacks. This file provides such
 * an implementation (TextCollector) that accumulates a text (or, optionally,
 * lightly-marked-up Markdown) rendering of the document, and exposes it to
 * Rust through a flat C API returning owned UTF-8 that the Rust side frees.
 *
 * Every entry point catches all C++ exceptions: libwpd throws on malformed
 * input, and an exception must never unwind across the FFI boundary.
 */
#include <librevenge-stream/librevenge-stream.h>
#include <librevenge/librevenge.h>
#include <libwpd/libwpd.h>

#include <cstdlib>
#include <cstring>
#include <string>
#include <vector>

namespace {
using librevenge::RVNGPropertyList;
using librevenge::RVNGString;

/* Accumulates document text.
 *
 * Content is written through `sink`, a pointer that is redirected (via
 * `pushSink`/`popSink`) whenever we enter text that is not part of the main
 * narrative flow: headers, footers, footnotes, endnotes, comments and text
 * boxes. Each of these is collected into its own scratch buffer and then
 * spliced back in behind a `[kind: ...]` marker rather than being appended
 * directly to the surrounding prose, so callers can tell page furniture and
 * annotations apart from body text instead of finding them silently mixed
 * in. Headers and footers recur on every page rather than at one point in
 * the flow, so they are collected once and exposed at the start/end of the
 * document instead of spliced inline.
 *
 * When `markdown` is set, span-level emphasis (bold/italic), heading
 * paragraphs and list items are additionally rendered as Markdown syntax.
 * Tables are intentionally left as tab/newline-separated text in both modes:
 * WordPerfect tables can have ragged rows and merged cells that don't map
 * cleanly onto Markdown's fixed-column pipe-table syntax, and a best-effort
 * translation would risk producing tables that look valid but are wrong. */
class TextCollector : public librevenge::RVNGTextInterface {
  public:
    explicit TextCollector(bool markdown) : markdown_(markdown) {}

    std::string text;
    std::string header;
    std::string footer;

    std::string result() const {
        std::string out;
        if (!header.empty()) {
            out += "[header: " + header + "]\n\n";
        }
        out += text;
        if (!footer.empty()) {
            out += "\n\n[footer: " + footer + "]";
        }
        return out;
    }

    // Content callbacks that carry text.
    void insertText(const RVNGString &s) override {
        if (s.cstr())
            *sink += s.cstr();
    }
    void insertTab() override {
        *sink += '\t';
    }
    void insertSpace() override {
        *sink += ' ';
    }
    void insertLineBreak() override {
        *sink += '\n';
    }
    void closeParagraph() override {
        *sink += "\n\n";
    }
    void closeListElement() override {
        *sink += '\n';
    }

    // Tables: tab between cells, newline between rows, blank line after table.
    void closeTableCell() override {
        *sink += '\t';
    }
    void closeTableRow() override {
        *sink += '\n';
    }
    void closeTable() override {
        *sink += '\n';
    }

    void openParagraph(const RVNGPropertyList &props) override {
        if (markdown_) {
            const librevenge::RVNGProperty *outline = props["text:outline-level"];
            if (outline) {
                int level = outline->getInt();
                if (level >= 1 && level <= 6) {
                    *sink += std::string(static_cast<size_t>(level), '#') + ' ';
                }
            }
        }
    }

    void openSpan(const RVNGPropertyList &props) override {
        std::string markers;
        if (markdown_) {
            const librevenge::RVNGProperty *weight = props["fo:font-weight"];
            const librevenge::RVNGProperty *style = props["fo:font-style"];
            if (weight && weight->getStr() == "bold")
                markers += "**";
            if (style && style->getStr() == "italic")
                markers += "_";
        }
        spanMarkers_.push_back(markers);
        *sink += markers;
    }
    void closeSpan() override {
        if (spanMarkers_.empty())
            return;
        std::string markers = spanMarkers_.back();
        spanMarkers_.pop_back();
        // Close in reverse of the order they were opened.
        for (auto it = markers.rbegin(); it != markers.rend();) {
            if (*it == '_') {
                *sink += '_';
                ++it;
            } else {
                *sink += "**";
                it += 2;
            }
        }
    }

    void openOrderedListLevel(const RVNGPropertyList &) override {
        listStack_.push_back({true, 0});
    }
    void openUnorderedListLevel(const RVNGPropertyList &) override {
        listStack_.push_back({false, 0});
    }
    void closeOrderedListLevel() override {
        if (!listStack_.empty())
            listStack_.pop_back();
    }
    void closeUnorderedListLevel() override {
        if (!listStack_.empty())
            listStack_.pop_back();
    }
    void openListElement(const RVNGPropertyList &) override {
        if (markdown_ && !listStack_.empty()) {
            std::string indent((listStack_.size() - 1) * 2, ' ');
            ListLevel &level = listStack_.back();
            if (level.ordered) {
                level.counter += 1;
                *sink += indent + std::to_string(level.counter) + ". ";
            } else {
                *sink += indent + "- ";
            }
        }
    }

    // Headers and footers: collected once into their own buffer, not spliced
    // into the middle of body text (see class comment).
    void openHeader(const RVNGPropertyList &) override {
        pushSink(&header);
    }
    void closeHeader() override {
        popSink();
    }
    void openFooter(const RVNGPropertyList &) override {
        pushSink(&footer);
    }
    void closeFooter() override {
        popSink();
    }

    // Footnotes, endnotes, comments and text boxes: collected into a scratch
    // buffer and spliced back into whichever sink was active as a labeled,
    // bounded aside, so they never bleed into surrounding narrative text.
    void openFootnote(const RVNGPropertyList &) override {
        openAside();
    }
    void closeFootnote() override {
        closeAside("footnote");
    }
    void openEndnote(const RVNGPropertyList &) override {
        openAside();
    }
    void closeEndnote() override {
        closeAside("endnote");
    }
    void openComment(const RVNGPropertyList &) override {
        openAside();
    }
    void closeComment() override {
        closeAside("comment");
    }
    void openTextBox(const RVNGPropertyList &) override {
        openAside();
    }
    void closeTextBox() override {
        closeAside("box");
    }

    // Remaining pure virtuals are structural and produce no text of their own.
    void setDocumentMetaData(const RVNGPropertyList &) override {}
    void startDocument(const RVNGPropertyList &) override {}
    void endDocument() override {}
    void definePageStyle(const RVNGPropertyList &) override {}
    void defineEmbeddedFont(const RVNGPropertyList &) override {}
    void openPageSpan(const RVNGPropertyList &) override {}
    void closePageSpan() override {}
    void defineParagraphStyle(const RVNGPropertyList &) override {}
    void defineCharacterStyle(const RVNGPropertyList &) override {}
    void openLink(const RVNGPropertyList &) override {}
    void closeLink() override {}
    void defineSectionStyle(const RVNGPropertyList &) override {}
    void openSection(const RVNGPropertyList &) override {}
    void closeSection() override {}
    void insertField(const RVNGPropertyList &) override {}
    void openTable(const RVNGPropertyList &) override {}
    void openTableRow(const RVNGPropertyList &) override {}
    void openTableCell(const RVNGPropertyList &) override {}
    void insertCoveredTableCell(const RVNGPropertyList &) override {}
    void openFrame(const RVNGPropertyList &) override {}
    void closeFrame() override {}
    void insertBinaryObject(const RVNGPropertyList &) override {}
    void insertEquation(const RVNGPropertyList &) override {}
    void openGroup(const RVNGPropertyList &) override {}
    void closeGroup() override {}
    void defineGraphicStyle(const RVNGPropertyList &) override {}
    void drawRectangle(const RVNGPropertyList &) override {}
    void drawEllipse(const RVNGPropertyList &) override {}
    void drawPolygon(const RVNGPropertyList &) override {}
    void drawPolyline(const RVNGPropertyList &) override {}
    void drawPath(const RVNGPropertyList &) override {}
    void drawConnector(const RVNGPropertyList &) override {}

  private:
    struct ListLevel {
        bool ordered;
        int counter;
    };

    void pushSink(std::string *s) {
        sinkStack_.push_back(sink);
        sink = s;
    }
    void popSink() {
        if (!sinkStack_.empty()) {
            sink = sinkStack_.back();
            sinkStack_.pop_back();
        }
    }
    void openAside() {
        asideStack_.push_back(std::string());
        pushSink(&asideStack_.back());
    }
    void closeAside(const char *kind) {
        if (asideStack_.empty())
            return;
        std::string content = std::move(asideStack_.back());
        asideStack_.pop_back();
        popSink();
        // Trim the trailing blank-paragraph separator so the marker reads
        // as one bounded aside rather than trailing empty lines.
        while (!content.empty() && (content.back() == '\n'))
            content.pop_back();
        *sink += std::string("\n[") + kind + ": " + content + "]\n";
    }

    bool markdown_;
    std::string *sink = &text;
    std::vector<std::string *> sinkStack_;
    std::vector<std::string> asideStack_;
    std::vector<std::string> spanMarkers_;
    std::vector<ListLevel> listStack_;
};
} // namespace

extern "C" {

/* Result codes shared with the Rust side (see error.rs). */
enum {
    XBERG_WPD_OK = 0,
    XBERG_WPD_INVALID_ARGS = 1,
    XBERG_WPD_UNSUPPORTED_FORMAT = 2,
    XBERG_WPD_PARSE_ERROR = 3,
    XBERG_WPD_OUT_OF_MEMORY = 4,
    XBERG_WPD_PANIC = 5,
};

namespace {
char *dup_malloc(const char *data, size_t n) {
    char *buf = static_cast<char *>(std::malloc(n + 1));
    if (!buf)
        return nullptr;
    if (n)
        std::memcpy(buf, data, n);
    buf[n] = '\0';
    return buf;
}
} // namespace

/* Returns non-zero if the buffer looks like a WordPerfect document libwpd can
 * parse. Never throws. */
int xberg_wpd_is_supported(const unsigned char *data, unsigned long len) {
    if (!data || len == 0)
        return 0;
    try {
        librevenge::RVNGStringStream input(data, static_cast<unsigned int>(len));
        return libwpd::WPDocument::isFileFormatSupported(&input) != libwpd::WPD_CONFIDENCE_NONE ? 1
                                                                                                : 0;
    } catch (...) {
        return 0;
    }
}

/* Extract text (or, if `markdown` is non-zero, lightly Markdown-marked-up
 * text) from an in-memory WordPerfect document.
 *
 * On XBERG_WPD_OK, *out_text is a malloc'd buffer of *out_len bytes (NOT
 * necessarily NUL-terminated at that length if the document contained an
 * embedded NUL; a trailing NUL is appended anyway for defensive C-string use
 * but callers must use *out_len as the authoritative length) the caller frees
 * via xberg_wpd_free_string. On any other return, *out_text is left null.
 *
 * On failure, *out_err may be set to a malloc'd, NUL-terminated diagnostic
 * message (freed the same way) describing the underlying C++ exception; it
 * is left null when no additional detail is available. */
int xberg_wpd_extract(const unsigned char *data, unsigned long len, int markdown, char **out_text,
                      unsigned long *out_len, char **out_err) {
    if (!out_text || !out_len)
        return XBERG_WPD_INVALID_ARGS;
    *out_text = nullptr;
    *out_len = 0;
    if (out_err)
        *out_err = nullptr;
    if (!data || len == 0)
        return XBERG_WPD_INVALID_ARGS;

    try {
        librevenge::RVNGStringStream input(data, static_cast<unsigned int>(len));
        if (libwpd::WPDocument::isFileFormatSupported(&input) == libwpd::WPD_CONFIDENCE_NONE)
            return XBERG_WPD_UNSUPPORTED_FORMAT;

        TextCollector collector(markdown != 0);
        if (libwpd::WPDocument::parse(&input, &collector, nullptr) != libwpd::WPD_OK)
            return XBERG_WPD_PARSE_ERROR;

        std::string rendered = collector.result();
        char *buf = dup_malloc(rendered.data(), rendered.size());
        if (!buf)
            return XBERG_WPD_OUT_OF_MEMORY;
        *out_text = buf;
        *out_len = static_cast<unsigned long>(rendered.size());
        return XBERG_WPD_OK;
    } catch (const std::exception &e) {
        if (out_err)
            *out_err = dup_malloc(e.what(), std::strlen(e.what()));
        return XBERG_WPD_PANIC;
    } catch (...) {
        return XBERG_WPD_PANIC;
    }
}

void xberg_wpd_free_string(char *s) {
    std::free(s);
}

/* Internal self-test for TextCollector's aside-separation logic (see class
 * comment above): drives the collector's callbacks directly, the same way
 * libwpd would, without needing a real WordPerfect document on disk. Exposed
 * so the Rust test suite has real evidence that footnote/header content is
 * bracketed apart from body text rather than concatenated into it. Not part
 * of the crate's public API contract. Returns non-zero on success. */
int xberg_wpd_self_test_separation(void) {
    TextCollector c(false);

    RVNGPropertyList empty;
    c.openHeader(empty);
    c.insertText(RVNGString("Confidential Draft"));
    c.closeHeader();

    c.openParagraph(empty);
    c.insertText(RVNGString("Body start."));
    c.openFootnote(empty);
    c.insertText(RVNGString("See appendix A."));
    c.closeFootnote();
    c.insertText(RVNGString("Body continues."));
    c.closeParagraph();

    c.openFooter(empty);
    c.insertText(RVNGString("Page 1 of 1"));
    c.closeFooter();

    std::string out = c.result();

    bool ok = true;
    ok = ok && out.find("[header: Confidential Draft]") != std::string::npos;
    ok = ok && out.find("[footer: Page 1 of 1]") != std::string::npos;
    ok = ok && out.find("[footnote: See appendix A.]") != std::string::npos;
    ok = ok && out.find("Body start.Body continues.") == std::string::npos;
    ok = ok && out.find("Body start.") != std::string::npos;
    ok = ok && out.find("Body continues.") != std::string::npos;
    // The header text must never appear in the body run itself.
    ok = ok && c.text.find("Confidential Draft") == std::string::npos;

    return ok ? 1 : 0;
}

} // extern "C"
