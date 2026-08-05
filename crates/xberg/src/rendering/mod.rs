//! Unified rendering of document content to output formats.
//!
//! - `render_markdown` — GFM Markdown (via comrak)
//! - `render_html` — HTML5 (via comrak)
//! - `render_djot` — Djot markup
//! - `render_doctags` — Docling DocTags (tables as OTSL)
//! - `render_plain` — Plain text (no formatting)

pub(crate) mod common;
mod comrak_bridge;
mod djot;
mod doctags;
mod html;
#[cfg(feature = "html")]
pub mod html_styled;
mod json;
mod markdown;
mod plain;

pub(crate) use djot::render_djot;
pub(crate) use doctags::render_doctags;
pub(crate) use html::render_html;
#[cfg(feature = "html")]
pub use html_styled::StyledHtmlRenderer;
pub use json::render_json;
pub(crate) use markdown::render_markdown;
pub(crate) use plain::render_plain;
