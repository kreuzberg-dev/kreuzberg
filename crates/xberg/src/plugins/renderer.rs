//! Renderer plugin trait.
//!
//! This module defines the trait for implementing custom document renderers
//! that convert extraction results to output format strings.

use std::sync::Arc;

use crate::Result;
use crate::plugins::Plugin;
use crate::types::ExtractedDocument;
use crate::types::internal::InternalDocument;

/// Trait for document renderers that convert extraction results to output strings.
///
/// Renderers are typically stateless converters that transform extracted
/// content into a specific output format (Markdown, HTML, Djot, plain text,
/// etc.). They participate in the standard [`Plugin`]
/// lifecycle so custom renderers can be registered from any supported binding
/// language.
///
/// The format name is exposed via [`Plugin::name`]. For stateless renderers
/// the [`Plugin`] lifecycle methods (`version`, `initialize`, `shutdown`) all
/// take no-op defaults and need not be overridden.
///
/// # Thread Safety
///
/// Renderers must be `Send + Sync` (inherited from [`Plugin`]).
///
/// # Example
///
/// ```rust
/// use xberg::plugins::{Plugin, Renderer};
/// use xberg::{ExtractedDocument, Result};
///
/// struct CustomRenderer;
///
/// impl Plugin for CustomRenderer {
///     fn name(&self) -> &str { "custom" }
/// }
///
/// impl Renderer for CustomRenderer {
///     fn render_result(&self, result: &ExtractedDocument) -> Result<String> {
///         Ok(result.content.to_uppercase())
///     }
/// }
/// ```
pub trait Renderer: Plugin {
    /// Binding-safe rendering entry point for foreign-language plugin bridges.
    ///
    /// Accepts one public extraction result and returns the rendered output.
    fn render_result(&self, result: &ExtractedDocument) -> Result<String> {
        Ok(result.content.clone())
    }
}

/// Crate-private rendering capability used by native Rust renderers.
pub(crate) trait InternalRenderer: Plugin {
    /// Render the pipeline representation to the output format.
    fn render(&self, doc: &InternalDocument) -> Result<String>;
}

impl<T> Renderer for T
where
    T: InternalRenderer + ?Sized,
{
    fn render_result(&self, result: &ExtractedDocument) -> Result<String> {
        // `ExtractedDocument` carries no element tree of its own: `InternalDocument::from`
        // produces a document with zero elements, and rendering that emits an empty
        // document shell instead of the document's content. Render from the preserved
        // `internal_document` when the caller supplied one; otherwise fall back to the
        // `Renderer` default of returning the already-rendered text verbatim.
        match result.internal_document.as_ref() {
            Some(doc) => InternalRenderer::render(self, doc),
            None => Ok(result.content.clone()),
        }
    }
}

/// Register a renderer plugin with the global registry.
///
/// The renderer's format name is taken from [`Plugin::name`]. Registering a
/// renderer with a name that already exists replaces the previous renderer
/// for that format.
///
/// # Note on `Result` return type
///
/// Returns `Result<()>` for cross-language API symmetry required by the alef
/// trait-bridge codegen. The underlying `parking_lot::RwLock` cannot be
/// poisoned (parking_lot provides no poisoning semantics), so this function
/// never returns `Err` in practice.
pub fn register_renderer(renderer: Arc<dyn Renderer>) -> Result<()> {
    use crate::plugins::registry::get_renderer_registry;

    let registry = get_renderer_registry();
    let mut registry = registry.write();
    registry.register(renderer)
}

/// Unregister a renderer by format name.
///
/// # Errors
///
/// Returns an error if the registry lock is poisoned.
pub fn unregister_renderer(name: &str) -> Result<()> {
    use crate::plugins::registry::get_renderer_registry;

    let registry = get_renderer_registry();
    let mut registry = registry.write();
    registry.remove(name)
}

/// List names of all registered renderers.
///
/// # Errors
///
/// Returns an error if the registry lock is poisoned.
pub fn list_renderers() -> Result<Vec<String>> {
    use crate::plugins::registry::get_renderer_registry;

    let registry = get_renderer_registry();
    let registry = registry.read();
    Ok(registry.list())
}

/// Clear all renderers from the global registry.
///
/// Removes every renderer, including the built-in defaults (markdown, html,
/// djot, plain). After calling this no renderers are registered; re-register
/// as needed.
///
/// # Errors
///
/// Returns an error if the registry lock is poisoned.
pub fn clear_renderers() -> Result<()> {
    use crate::plugins::registry::get_renderer_registry;

    let registry = get_renderer_registry();
    let mut registry = registry.write();
    registry.clear_all()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::registry::test_support::RendererRegistryGuard;

    struct MockRenderer {
        format: &'static str,
    }

    impl Plugin for MockRenderer {
        fn name(&self) -> &str {
            self.format
        }
    }

    impl InternalRenderer for MockRenderer {
        fn render(&self, doc: &crate::types::internal::InternalDocument) -> crate::Result<String> {
            Ok(format!("mock-{}-{}", self.format, doc.elements.len()))
        }
    }

    #[test]
    fn register_list_unregister_roundtrip() {
        let _guard = RendererRegistryGuard::acquire();
        register_renderer(Arc::new(MockRenderer { format: "test-fmt-a" })).unwrap();
        assert!(list_renderers().unwrap().contains(&"test-fmt-a".to_string()));

        unregister_renderer("test-fmt-a").unwrap();
        assert!(!list_renderers().unwrap().contains(&"test-fmt-a".to_string()));
    }

    /// Regression test for defect #51.
    ///
    /// The blanket `impl<T: InternalRenderer> Renderer for T` used to round-trip through
    /// `InternalDocument::from(result.clone())`, which yields a document with zero
    /// elements, so every native renderer reached through the public `render_result`
    /// entry point emitted an empty shell instead of the document's content.
    #[test]
    fn render_result_renders_from_the_preserved_internal_document() {
        use crate::types::internal::{ElementKind, InternalElement};

        let renderer = MockRenderer { format: "test-fmt-c" };

        let mut doc = InternalDocument::new("text/plain");
        doc.push_element(InternalElement::text(ElementKind::Paragraph, "one", 0));
        doc.push_element(InternalElement::text(ElementKind::Paragraph, "two", 0));

        let mut result = ExtractedDocument {
            content: "one\n\ntwo".to_string(),
            ..Default::default()
        };
        result.internal_document = Some(doc);

        assert_eq!(
            Renderer::render_result(&renderer, &result).unwrap(),
            "mock-test-fmt-c-2"
        );
    }

    /// Regression test for defect #51: without a preserved `InternalDocument` there is no
    /// element tree to render from, so the blanket impl must fall back to the documented
    /// `Renderer` default (return the already-rendered text) rather than render an empty
    /// document.
    #[test]
    fn render_result_falls_back_to_content_without_an_internal_document() {
        let renderer = MockRenderer { format: "test-fmt-d" };

        let result = ExtractedDocument {
            content: "already rendered".to_string(),
            ..Default::default()
        };

        assert_eq!(Renderer::render_result(&renderer, &result).unwrap(), "already rendered");
    }

    #[test]
    fn register_list_clear_list_roundtrip() {
        // The guard restores the built-in renderers on both entry and exit, so this test can
        // clear the registry outright without stranding later tests with an empty one.
        let _guard = RendererRegistryGuard::acquire();
        register_renderer(Arc::new(MockRenderer { format: "test-fmt-b" })).unwrap();
        assert!(list_renderers().unwrap().contains(&"test-fmt-b".to_string()));

        clear_renderers().unwrap();
        assert!(list_renderers().unwrap().is_empty());
    }
}
