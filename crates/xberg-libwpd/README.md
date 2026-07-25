# xberg-libwpd

WordPerfect (`.wpd`, `.wp5`, `.wp6`, and the wider WordPerfect binary family
from WP 4.2 through the X-series) text extraction for
[Xberg](https://xberg.io), backed by [libwpd](https://libwpd.sourceforge.net/)
and its document-model dependency
[librevenge](https://wiki.documentfoundation.org/DLP/Libraries/librevenge).

## How it works

libwpd exposes no `extract()` call. It drives librevenge's SAX-like
`RVNGTextInterface`: the caller supplies a concrete implementation and libwpd
invokes its callbacks. A hand-written C++ shim (`src/shim.cpp`) implements that
interface, accumulates a text (or, via `extract_markdown`, lightly
Markdown-marked-up) rendering, and exposes a small flat C API. `src/lib.rs`
wraps it in a safe Rust surface:

```rust
let text = xberg_libwpd::extract_text(&bytes)?;
let markdown = xberg_libwpd::extract_markdown(&bytes)?; // headings, bold/italic, lists
let ok = xberg_libwpd::is_supported(&bytes);
```

Footnotes, endnotes, comments, text boxes, headers and footers are never
concatenated straight into the surrounding narrative text: each is collected
separately and spliced back in behind a `[kind: ...]` marker (headers/footers,
which recur on every page rather than at one point in the document, are
exposed once at the start/end instead). Tables stay tab/newline-separated in
both modes — WordPerfect tables can have ragged rows and merged cells that
don't map cleanly onto Markdown's fixed-column pipe-table syntax.

## Building

`build.rs` downloads the librevenge and libwpd release tarballs (checksum
verified) and compiles them from source together with the shim into one
static library, using the C++ toolchain via the `cc` crate. Both libraries
are built against their **MPL-2.0** arm.

Downloaded sources are cached in a workspace-relative directory (derived from
`OUT_DIR`) so they survive `cargo clean`, mirroring `xberg-tesseract`. Override
the location with `XBERG_LIBWPD_CACHE_DIR`.

### Requirements

- A C++17 compiler.
- **boost headers.** librevenge and libwpd both use header-only `boost::spirit`
  at build time. Install boost (`brew install boost`, `apt-get install
  libboost-dev`, or on Windows `vcpkg install boost-spirit:x64-windows-static-md
  boost-serialization:x64-windows-static-md`)
  or point `BOOST_INCLUDE_DIR` at a directory containing `boost/version.hpp`.
- zlib (librevenge's zip stream links against it) — system zlib on
  Linux/macOS, or `vcpkg install zlib:x64-windows-static-md` on Windows
  (resolved via `VCPKG_ROOT`/`VCPKG_INSTALLATION_ROOT`, matching the
  `x64-windows-static-md` triplet this workspace's CI already uses for
  `libheif`).

## Platform support

Desktop only: Linux (glibc and musl), macOS, and Windows (MSVC). On any other
target the crate compiles to stub functions that return
`WpdError::UnsupportedPlatform`, so wasm/android builds pull in no C++
toolchain.

## Licensing

This crate is MIT. libwpd and librevenge are used under their MPL-2.0 arm; their
source is fetched at build time and is not redistributed in this repository.
MPL-2.0 is file-level copyleft and permits static linking into a
differently-licensed larger work.
