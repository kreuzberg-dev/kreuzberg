//! Renderer registry.

use crate::plugins::{InternalRenderer, Plugin, Renderer};
use crate::types::internal::InternalDocument;
use crate::{Result, XbergError};
use ahash::AHashMap;
use std::sync::Arc;

#[derive(Clone)]
struct RegisteredRenderer {
    renderer: Arc<dyn Renderer>,
    internal: Option<Arc<dyn InternalRenderer>>,
}

impl RegisteredRenderer {
    fn public(renderer: Arc<dyn Renderer>) -> Self {
        Self {
            renderer,
            internal: None,
        }
    }

    fn internal<T>(renderer: Arc<T>) -> Self
    where
        T: InternalRenderer + 'static,
    {
        Self {
            renderer: renderer.clone(),
            internal: Some(renderer),
        }
    }

    fn render(&self, doc: &InternalDocument) -> Result<String> {
        if let Some(internal) = &self.internal {
            return internal.render(doc);
        }

        // Public renderers only ever see `ExtractedDocument`. The plain `From` conversion
        // derives without document structure and never fills in `elements`, which strips
        // every layout signal a renderer needs. Derive the structure tree explicitly and
        // attach both the element list and the source document so a foreign renderer can
        // do layout-aware rendering.
        let mut result = crate::extraction::derive::derive_extraction_result(
            doc.clone(),
            true,
            crate::core::config::OutputFormat::Plain,
        );
        result.internal_document = Some(doc.clone());
        result.elements = Some(crate::extraction::transform::transform_extraction_result_to_elements(
            &result,
        ));
        self.renderer.render_result(&result)
    }
}

/// Built-in Markdown renderer.
struct MarkdownRenderer;

impl Plugin for MarkdownRenderer {
    fn name(&self) -> &str {
        "markdown"
    }
}

impl InternalRenderer for MarkdownRenderer {
    fn render(&self, doc: &InternalDocument) -> Result<String> {
        Ok(crate::rendering::render_markdown(doc))
    }
}

/// Built-in HTML renderer.
struct HtmlRenderer;

impl Plugin for HtmlRenderer {
    fn name(&self) -> &str {
        "html"
    }
}

impl InternalRenderer for HtmlRenderer {
    fn render(&self, doc: &InternalDocument) -> Result<String> {
        Ok(crate::rendering::render_html(doc))
    }
}

/// Built-in Djot renderer.
struct DjotRenderer;

impl Plugin for DjotRenderer {
    fn name(&self) -> &str {
        "djot"
    }
}

impl InternalRenderer for DjotRenderer {
    fn render(&self, doc: &InternalDocument) -> Result<String> {
        Ok(crate::rendering::render_djot(doc))
    }
}

/// Built-in Docling DocTags renderer.
struct DocTagsRenderer;

impl Plugin for DocTagsRenderer {
    fn name(&self) -> &str {
        "doctags"
    }
}

impl InternalRenderer for DocTagsRenderer {
    fn render(&self, doc: &InternalDocument) -> Result<String> {
        Ok(crate::rendering::render_doctags(doc))
    }
}

/// Built-in Graphviz DOT renderer.
struct DotRenderer;

impl Plugin for DotRenderer {
    fn name(&self) -> &str {
        "dot"
    }
}

impl InternalRenderer for DotRenderer {
    fn render(&self, doc: &InternalDocument) -> Result<String> {
        Ok(crate::rendering::render_dot(doc))
    }
}

/// Built-in plain text renderer.
struct PlainRenderer;

impl Plugin for PlainRenderer {
    fn name(&self) -> &str {
        "plain"
    }
}

impl InternalRenderer for PlainRenderer {
    fn render(&self, doc: &InternalDocument) -> Result<String> {
        Ok(crate::rendering::render_plain(doc))
    }
}

/// Registry for document renderer plugins.
///
/// Manages renderers that convert internal pipeline documents to output format strings.
///
/// # Thread Safety
///
/// The registry is thread-safe and can be accessed concurrently from multiple threads.
///
/// # Example
///
/// ```rust,no_run
/// use xberg::plugins::registry::RendererRegistry;
/// use std::sync::Arc;
///
/// let registry = RendererRegistry::new();
/// let available = registry.list();
/// // Built-in renderers: "markdown", "html", "djot", "doctags", "dot", "plain"
/// ```
#[cfg_attr(alef, alef(skip))]
pub struct RendererRegistry {
    renderers: AHashMap<String, RegisteredRenderer>,
}

impl RendererRegistry {
    /// Create a new renderer registry with built-in renderers.
    ///
    /// Registers the following built-in renderers:
    /// - `markdown` — GFM Markdown (via comrak)
    /// - `html` — HTML5 (via comrak)
    /// - `djot` — Djot markup
    /// - `doctags` — Docling DocTags (tables as OTSL)
    /// - `dot` — Graphviz DOT (diagrams recovered from vector sources)
    /// - `plain` — Plain text (no formatting)
    pub fn new() -> Self {
        let mut registry = Self {
            renderers: AHashMap::new(),
        };

        registry.register_builtins();
        registry
    }

    /// Create a new empty renderer registry without built-in renderers.
    ///
    /// Useful for testing or when you want full control over renderer registration.
    pub fn new_empty() -> Self {
        Self {
            renderers: AHashMap::new(),
        }
    }

    /// Register built-in renderers.
    fn register_builtins(&mut self) {
        self.renderers.insert(
            "markdown".to_string(),
            RegisteredRenderer::internal(Arc::new(MarkdownRenderer)),
        );
        self.renderers
            .insert("html".to_string(), RegisteredRenderer::internal(Arc::new(HtmlRenderer)));
        self.renderers
            .insert("djot".to_string(), RegisteredRenderer::internal(Arc::new(DjotRenderer)));
        self.renderers.insert(
            "doctags".to_string(),
            RegisteredRenderer::internal(Arc::new(DocTagsRenderer)),
        );
        self.renderers
            .insert("dot".to_string(), RegisteredRenderer::internal(Arc::new(DotRenderer)));
        self.renderers.insert(
            "plain".to_string(),
            RegisteredRenderer::internal(Arc::new(PlainRenderer)),
        );
    }

    /// Register a renderer.
    ///
    /// # Arguments
    ///
    /// * `renderer` - The renderer to register
    ///
    /// # Returns
    ///
    /// - `Ok(())` if registration succeeded
    /// - `Err(...)` if the renderer name is invalid
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// # use xberg::plugins::registry::RendererRegistry;
    /// # use std::sync::Arc;
    /// let mut registry = RendererRegistry::new();
    /// // let renderer = Arc::new(MyRenderer);
    /// // registry.register(renderer)?;
    /// # Ok::<(), xberg::XbergError>(())
    /// ```
    pub fn register(&mut self, renderer: Arc<dyn Renderer>) -> Result<()> {
        let name = renderer.name().to_string();

        super::validate_plugin_name(&name)?;

        self.renderers.insert(name, RegisteredRenderer::public(renderer));
        Ok(())
    }

    /// Render a document using the named renderer.
    ///
    /// Convenience method that looks up the renderer by name and renders the document.
    ///
    /// # Arguments
    ///
    /// * `name` - Renderer name (e.g., "markdown", "html")
    /// * `doc` - The internal document to render
    ///
    /// # Returns
    ///
    /// The rendered output string, or an error if the renderer is not found or rendering fails.
    pub(crate) fn render(&self, name: &str, doc: &InternalDocument) -> Result<String> {
        self.renderers
            .get(name)
            .ok_or_else(|| XbergError::Plugin {
                message: format!("Renderer '{}' not registered", name),
                plugin_name: name.to_string(),
            })?
            .render(doc)
    }

    /// List all registered renderer names.
    pub fn list(&self) -> Vec<String> {
        self.renderers.keys().cloned().collect()
    }

    /// Remove a renderer from the registry, calling its `shutdown()` method.
    pub fn remove(&mut self, name: &str) -> Result<()> {
        if let Some(renderer) = self.renderers.remove(name) {
            renderer.renderer.shutdown()?;
        }
        Ok(())
    }

    /// Clear all renderers from the registry.
    ///
    /// Removes every renderer, including the built-in defaults. After calling
    /// this the registry is empty; re-register renderers as needed.
    pub fn clear_all(&mut self) -> Result<()> {
        let names: Vec<_> = self.renderers.keys().cloned().collect();
        for name in names {
            self.remove(&name)?;
        }
        Ok(())
    }

    /// Drain the registry. Alias for `clear_all` used by alef trait-bridge codegen.
    pub fn clear(&mut self) -> Result<()> {
        self.clear_all()
    }

    /// Clear all renderers and re-register the built-in defaults.
    #[cfg(test)]
    pub(crate) fn reset_to_defaults(&mut self) -> Result<()> {
        self.renderers.clear();
        self.register_builtins();
        Ok(())
    }
}

impl Default for RendererRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MockRenderer {
        format_name: String,
    }

    impl Plugin for MockRenderer {
        fn name(&self) -> &str {
            &self.format_name
        }
    }

    impl InternalRenderer for MockRenderer {
        fn render(&self, doc: &InternalDocument) -> Result<String> {
            Ok(format!("mock-rendered-{}-elements", doc.elements.len()))
        }
    }

    #[test]
    fn test_renderer_registry_new_has_builtins() {
        let registry = RendererRegistry::new();
        let names = registry.list();
        assert!(names.contains(&"markdown".to_string()));
        assert!(names.contains(&"html".to_string()));
        assert!(names.contains(&"djot".to_string()));
        assert!(names.contains(&"doctags".to_string()));
        assert!(names.contains(&"plain".to_string()));
    }

    #[test]
    fn test_renderer_registry_new_empty() {
        let registry = RendererRegistry::new_empty();
        assert_eq!(registry.list().len(), 0);
    }

    #[test]
    fn test_renderer_registry_register_and_get() {
        let mut registry = RendererRegistry::new_empty();

        let renderer = Arc::new(MockRenderer {
            format_name: "test-format".to_string(),
        });

        registry.register(renderer).unwrap();

        assert!(registry.list().contains(&"test-format".to_string()));
    }

    #[test]
    fn test_renderer_registry_get_missing() {
        let registry = RendererRegistry::new_empty();
        assert!(!registry.list().contains(&"nonexistent".to_string()));
    }

    #[test]
    fn test_renderer_registry_render_convenience() {
        let registry = RendererRegistry::new();
        let doc = InternalDocument::new("text/plain");

        let result = registry.render("plain", &doc);
        assert!(result.is_ok());
    }

    #[test]
    fn test_renderer_registry_render_missing() {
        let registry = RendererRegistry::new_empty();
        let doc = InternalDocument::new("text/plain");

        let result = registry.render("nonexistent", &doc);
        assert!(result.is_err());
    }

    #[test]
    fn test_renderer_registry_remove() {
        let mut registry = RendererRegistry::new_empty();
        let renderer = Arc::new(MockRenderer {
            format_name: "to-remove".to_string(),
        });
        registry.register(renderer).unwrap();

        registry.remove("to-remove").unwrap();
        assert_eq!(registry.list().len(), 0);
    }

    #[test]
    fn test_renderer_registry_remove_nonexistent_returns_ok() {
        let mut registry = RendererRegistry::new_empty();
        let result = registry.remove("nonexistent-renderer");
        assert!(result.is_ok());
        assert_eq!(registry.list().len(), 0);
    }

    #[test]
    fn test_renderer_registry_clear_all_drops_builtins() {
        let mut registry = RendererRegistry::new();
        assert!(!registry.list().is_empty());

        registry.clear_all().unwrap();
        assert!(registry.list().is_empty());
    }

    #[test]
    fn test_renderer_registry_clear_alias_matches_clear_all() {
        let mut registry = RendererRegistry::new();
        let custom = Arc::new(MockRenderer {
            format_name: "custom".to_string(),
        });
        registry.register(custom).unwrap();

        registry.clear().unwrap();
        assert!(registry.list().is_empty());
    }

    #[test]
    fn test_renderer_registry_reset_to_defaults() {
        let mut registry = RendererRegistry::new();
        let custom = Arc::new(MockRenderer {
            format_name: "custom".to_string(),
        });
        registry.register(custom).unwrap();
        assert!(registry.list().contains(&"custom".to_string()));

        registry.reset_to_defaults().unwrap();
        assert!(!registry.list().contains(&"custom".to_string()));
        assert!(registry.list().contains(&"markdown".to_string()));
    }

    #[test]
    fn test_renderer_registry_invalid_name_empty() {
        let mut registry = RendererRegistry::new_empty();
        let renderer = Arc::new(MockRenderer {
            format_name: "".to_string(),
        });

        let result = registry.register(renderer);
        assert!(matches!(result, Err(XbergError::Validation { .. })));
    }

    #[test]
    fn test_renderer_registry_invalid_name_with_spaces() {
        let mut registry = RendererRegistry::new_empty();
        let renderer = Arc::new(MockRenderer {
            format_name: "invalid format".to_string(),
        });

        let result = registry.register(renderer);
        assert!(matches!(result, Err(XbergError::Validation { .. })));
    }

    #[test]
    fn test_renderer_registry_builtin_markdown_renders() {
        let registry = RendererRegistry::new();
        let doc = InternalDocument::new("text/plain");

        let result = registry.render("markdown", &doc).unwrap();
        let _ = result;
    }

    #[test]
    fn test_renderer_registry_builtin_html_renders() {
        let registry = RendererRegistry::new();
        let doc = InternalDocument::new("text/plain");

        let result = registry.render("html", &doc).unwrap();
        let _ = result;
    }

    #[test]
    fn test_renderer_registry_builtin_djot_renders() {
        let registry = RendererRegistry::new();
        let doc = InternalDocument::new("text/plain");

        let result = registry.render("djot", &doc).unwrap();
        let _ = result;
    }

    #[test]
    fn test_renderer_registry_builtin_doctags_renders() {
        let registry = RendererRegistry::new();
        let doc = InternalDocument::new("text/plain");

        let result = registry.render("doctags", &doc).unwrap();
        assert_eq!(result, "<doctag></doctag>");
    }

    #[test]
    fn test_renderer_registry_builtin_plain_renders() {
        let registry = RendererRegistry::new();
        let doc = InternalDocument::new("text/plain");

        let result = registry.render("plain", &doc).unwrap();
        let _ = result;
    }
}
