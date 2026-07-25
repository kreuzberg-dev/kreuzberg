/* Flat C shim over libwpd + librevenge for Xberg.
 *
 * libwpd exposes no `extract()` call. It drives librevenge's SAX-like
 * RVNGTextInterface: the caller passes a concrete implementation into
 * WPDocument::parse and libwpd invokes its callbacks. This file provides such
 * an implementation (DocumentBuilder) that records a flat, format-agnostic
 * internal document (a `std::vector<Node>`) as libwpd walks the document, and
 * exposes it to Rust through a flat C API returning owned UTF-8 that the Rust
 * side frees. Text and Markdown are two renderings of that one internal
 * document, produced only at the end, not two different things recorded
 * during the walk.
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

/* One recorded event from the libwpd/librevenge callback walk. The document
 * is a flat `std::vector<Node>`; rendering (see `render` below) is the only
 * place that knows about output formats. */
enum class NodeKind {
    Text,
    Tab,
    Space,
    LineBreak,
    ParagraphEnd,
    ListItemEnd,
    Heading, // level in `level`
    BoldStart,
    BoldEnd,
    ItalicStart,
    ItalicEnd,
    ListItemStart, // level (nesting depth) + ordered + counter
    TableCellEnd,
    TableRowEnd,
    TableEnd,
    HeaderStart,
    HeaderEnd,
    FooterStart,
    FooterEnd,
    AsideStart, // `text` carries the kind label ("footnote", "endnote", ...)
    AsideEnd,
};

struct Node {
    NodeKind kind;
    std::string text;
    int level = 0;
    int counter = 0;
    bool ordered = false;
};

/* Records the document as a flat, format-agnostic `std::vector<Node>` while
 * libwpd walks it. Carries no notion of "plain text" vs "Markdown" — that
 * distinction exists only in `render`, which runs once, after the walk is
 * complete, over the recorded nodes. */
class DocumentBuilder : public librevenge::RVNGTextInterface {
  public:
    std::vector<Node> nodes;

    void insertText(const RVNGString &s) override {
        if (s.cstr())
            nodes.push_back({NodeKind::Text, s.cstr()});
    }
    void insertTab() override {
        nodes.push_back({NodeKind::Tab});
    }
    void insertSpace() override {
        nodes.push_back({NodeKind::Space});
    }
    void insertLineBreak() override {
        nodes.push_back({NodeKind::LineBreak});
    }
    void closeParagraph() override {
        nodes.push_back({NodeKind::ParagraphEnd});
    }
    void closeListElement() override {
        nodes.push_back({NodeKind::ListItemEnd});
    }
    void closeTableCell() override {
        nodes.push_back({NodeKind::TableCellEnd});
    }
    void closeTableRow() override {
        nodes.push_back({NodeKind::TableRowEnd});
    }
    void closeTable() override {
        nodes.push_back({NodeKind::TableEnd});
    }

    void openParagraph(const RVNGPropertyList &props) override {
        const librevenge::RVNGProperty *outline = props["text:outline-level"];
        if (outline) {
            int level = outline->getInt();
            if (level >= 1 && level <= 6) {
                Node n{NodeKind::Heading};
                n.level = level;
                nodes.push_back(n);
            }
        }
    }

    void openSpan(const RVNGPropertyList &props) override {
        const librevenge::RVNGProperty *weight = props["fo:font-weight"];
        const librevenge::RVNGProperty *style = props["fo:font-style"];
        bool bold = weight && weight->getStr() == "bold";
        bool italic = style && style->getStr() == "italic";
        // Recorded in open order; closeSpan below closes in reverse.
        if (bold)
            nodes.push_back({NodeKind::BoldStart});
        if (italic)
            nodes.push_back({NodeKind::ItalicStart});
        spanStack_.push_back({bold, italic});
    }
    void closeSpan() override {
        if (spanStack_.empty())
            return;
        SpanFlags flags = spanStack_.back();
        spanStack_.pop_back();
        if (flags.italic)
            nodes.push_back({NodeKind::ItalicEnd});
        if (flags.bold)
            nodes.push_back({NodeKind::BoldEnd});
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
        if (listStack_.empty())
            return;
        ListLevel &level = listStack_.back();
        Node n{NodeKind::ListItemStart};
        n.level = static_cast<int>(listStack_.size());
        n.ordered = level.ordered;
        if (level.ordered) {
            level.counter += 1;
            n.counter = level.counter;
        }
        nodes.push_back(n);
    }

    // Headers and footers recur on every page rather than at one point in the
    // flow; rendering collects them once and exposes them at the start/end of
    // the document instead of splicing them inline (see `render`).
    void openHeader(const RVNGPropertyList &) override {
        nodes.push_back({NodeKind::HeaderStart});
    }
    void closeHeader() override {
        nodes.push_back({NodeKind::HeaderEnd});
    }
    void openFooter(const RVNGPropertyList &) override {
        nodes.push_back({NodeKind::FooterStart});
    }
    void closeFooter() override {
        nodes.push_back({NodeKind::FooterEnd});
    }

    // Footnotes, endnotes, comments and text boxes: rendering brackets these
    // apart from surrounding narrative text rather than letting them bleed
    // into it (see `render`).
    void openFootnote(const RVNGPropertyList &) override {
        nodes.push_back({NodeKind::AsideStart, "footnote"});
    }
    void closeFootnote() override {
        nodes.push_back({NodeKind::AsideEnd});
    }
    void openEndnote(const RVNGPropertyList &) override {
        nodes.push_back({NodeKind::AsideStart, "endnote"});
    }
    void closeEndnote() override {
        nodes.push_back({NodeKind::AsideEnd});
    }
    void openComment(const RVNGPropertyList &) override {
        nodes.push_back({NodeKind::AsideStart, "comment"});
    }
    void closeComment() override {
        nodes.push_back({NodeKind::AsideEnd});
    }
    void openTextBox(const RVNGPropertyList &) override {
        nodes.push_back({NodeKind::AsideStart, "box"});
    }
    void closeTextBox() override {
        nodes.push_back({NodeKind::AsideEnd});
    }

    // Remaining pure virtuals are structural and record no node of their own.
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
    struct SpanFlags {
        bool bold;
        bool italic;
    };
    struct ListLevel {
        bool ordered;
        int counter;
    };

    std::vector<SpanFlags> spanStack_;
    std::vector<ListLevel> listStack_;
};

/* Renders a recorded `std::vector<Node>` to text (`markdown = false`) or to
 * lightly Markdown-marked-up text (`markdown = true`). This is the only place
 * that knows about output formats — `DocumentBuilder` above records the same
 * structure regardless of which rendering will eventually be requested.
 *
 * Handles header/footer/aside placement identically in both modes: each is
 * accumulated into its own buffer via a sink stack and spliced back in
 * (headers/footers once, at the start/end; asides inline, bracketed) rather
 * than left to bleed into the surrounding narrative text. Tables render as
 * tab/newline-separated text in both modes: WordPerfect tables can have
 * ragged rows and merged cells that don't map cleanly onto Markdown's
 * fixed-column pipe-table syntax, and a best-effort translation would risk
 * producing tables that look valid but are wrong. */
std::string render(const std::vector<Node> &nodes, bool markdown) {
    std::string body;
    std::string header;
    std::string footer;
    std::string *sink = &body;
    std::vector<std::string *> sinkStack;
    std::vector<std::string> asideStack;
    std::vector<std::string> asideLabels;

    auto pushSink = [&](std::string *s) {
        sinkStack.push_back(sink);
        sink = s;
    };
    auto popSink = [&]() {
        if (!sinkStack.empty()) {
            sink = sinkStack.back();
            sinkStack.pop_back();
        }
    };

    for (const Node &n : nodes) {
        switch (n.kind) {
        case NodeKind::Text:
            *sink += n.text;
            break;
        case NodeKind::Tab:
            *sink += '\t';
            break;
        case NodeKind::Space:
            *sink += ' ';
            break;
        case NodeKind::LineBreak:
            *sink += '\n';
            break;
        case NodeKind::ParagraphEnd:
            *sink += "\n\n";
            break;
        case NodeKind::ListItemEnd:
            *sink += '\n';
            break;
        case NodeKind::Heading:
            if (markdown)
                *sink += std::string(static_cast<size_t>(n.level), '#') + ' ';
            break;
        case NodeKind::BoldStart:
            if (markdown)
                *sink += "**";
            break;
        case NodeKind::BoldEnd:
            if (markdown)
                *sink += "**";
            break;
        case NodeKind::ItalicStart:
            if (markdown)
                *sink += '_';
            break;
        case NodeKind::ItalicEnd:
            if (markdown)
                *sink += '_';
            break;
        case NodeKind::ListItemStart:
            if (markdown) {
                std::string indent(static_cast<size_t>(n.level - 1) * 2, ' ');
                *sink += n.ordered ? indent + std::to_string(n.counter) + ". " : indent + "- ";
            }
            break;
        case NodeKind::TableCellEnd:
            *sink += '\t';
            break;
        case NodeKind::TableRowEnd:
            *sink += '\n';
            break;
        case NodeKind::TableEnd:
            *sink += '\n';
            break;
        case NodeKind::HeaderStart:
            pushSink(&header);
            break;
        case NodeKind::HeaderEnd:
            popSink();
            break;
        case NodeKind::FooterStart:
            pushSink(&footer);
            break;
        case NodeKind::FooterEnd:
            popSink();
            break;
        case NodeKind::AsideStart:
            asideLabels.push_back(n.text);
            asideStack.push_back(std::string());
            pushSink(&asideStack.back());
            break;
        case NodeKind::AsideEnd: {
            if (asideStack.empty())
                break;
            std::string content = std::move(asideStack.back());
            asideStack.pop_back();
            std::string label = std::move(asideLabels.back());
            asideLabels.pop_back();
            popSink();
            // Trim the trailing paragraph separator so the marker reads as
            // one bounded aside rather than trailing empty lines.
            while (!content.empty() && content.back() == '\n')
                content.pop_back();
            *sink += "\n[" + label + ": " + content + "]\n";
            break;
        }
        }
    }

    std::string out;
    if (!header.empty())
        out += "[header: " + header + "]\n\n";
    out += body;
    if (!footer.empty())
        out += "\n\n[footer: " + footer + "]";
    return out;
}
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
 * text) from an in-memory WordPerfect document. Parses once into an internal
 * `std::vector<Node>` document via `DocumentBuilder`, then renders that one
 * document to the requested format — the two output modes are two renderings
 * of the same recorded structure, not two different things produced during
 * the libwpd walk.
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

        DocumentBuilder builder;
        if (libwpd::WPDocument::parse(&input, &builder, nullptr) != libwpd::WPD_OK)
            return XBERG_WPD_PARSE_ERROR;

        std::string rendered = render(builder.nodes, markdown != 0);
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

/* Internal self-test for the aside-separation logic in `render` (see its
 * comment above): drives `DocumentBuilder`'s callbacks directly, the same way
 * libwpd would, without needing a real WordPerfect document on disk. Exposed
 * so the Rust test suite has real evidence that footnote/header content is
 * bracketed apart from body text rather than concatenated into it. Not part
 * of the crate's public API contract. Returns non-zero on success. */
int xberg_wpd_self_test_separation(void) {
    DocumentBuilder b;

    RVNGPropertyList empty;
    b.openHeader(empty);
    b.insertText(RVNGString("Confidential Draft"));
    b.closeHeader();

    b.openParagraph(empty);
    b.insertText(RVNGString("Body start."));
    b.openFootnote(empty);
    b.insertText(RVNGString("See appendix A."));
    b.closeFootnote();
    b.insertText(RVNGString("Body continues."));
    b.closeParagraph();

    b.openFooter(empty);
    b.insertText(RVNGString("Page 1 of 1"));
    b.closeFooter();

    std::string out = render(b.nodes, false);

    bool ok = true;
    ok = ok && out.find("[header: Confidential Draft]") != std::string::npos;
    ok = ok && out.find("[footer: Page 1 of 1]") != std::string::npos;
    ok = ok && out.find("[footnote: See appendix A.]") != std::string::npos;
    ok = ok && out.find("Body start.Body continues.") == std::string::npos;
    ok = ok && out.find("Body start.") != std::string::npos;
    ok = ok && out.find("Body continues.") != std::string::npos;
    // The header text must never appear anywhere but inside its own marker.
    size_t body_start = out.find("Body start.");
    ok = ok && body_start != std::string::npos &&
         out.find("Confidential Draft", body_start) == std::string::npos;

    return ok ? 1 : 0;
}

} // extern "C"
