//! Name-compatibility shim re-exporting [`xberg_ttf_parser`].
//!
//! `ttf-parser` reaches Xberg only through third-party crates (`pdf_oxide` and
//! `fontdb`) that request it by name. Cargo matches `[patch.crates-io]` entries
//! on package name alone, so this crate carries the upstream name and forwards
//! everything to the vendored parser. One parser is compiled, so types are
//! identical across every consumer.

#![no_std]

pub use xberg_ttf_parser::*;
