//! Document extractor plugin system.
//!
//! This module provides the trait and registry for implementing custom document extractors.

mod registry;
mod r#trait;

#[cfg(feature = "otel")]
pub(crate) mod instrumented;

pub use r#trait::{DocumentExtractor, InternalDocumentExtractor};

use std::sync::Arc;

/// Register a document extractor with the global registry.
///
/// The extractor is keyed by [`crate::plugins::Plugin::name`] and indexed for
/// every MIME type returned by
/// [`crate::plugins::DocumentExtractor::supported_mime_types`].
///
/// # Errors
///
/// - [`crate::XbergError::Validation`] if the plugin name is empty or
///   contains whitespace.
/// - Any error returned by the extractor's `initialize()` method.
pub fn register_document_extractor(extractor: Arc<dyn DocumentExtractor>) -> crate::Result<()> {
    use crate::plugins::registry::get_document_extractor_registry;

    let registry = get_document_extractor_registry();
    let mut registry = registry.write();
    registry.register(extractor)
}

/// Unregister a document extractor by name.
///
/// Removes the extractor from the global registry and calls its `shutdown()`
/// method. No-op if no extractor with that name is registered.
///
/// # Errors
///
/// - Any error returned by the extractor's `shutdown()` method.
pub fn unregister_document_extractor(name: &str) -> crate::Result<()> {
    use crate::plugins::registry::get_document_extractor_registry;

    let registry = get_document_extractor_registry();
    let mut registry = registry.write();
    registry.remove(name)
}

/// List names of all registered document extractors.
pub fn list_document_extractors() -> crate::Result<Vec<String>> {
    use crate::plugins::registry::get_document_extractor_registry;

    let registry = get_document_extractor_registry();
    let registry = registry.read();

    Ok(registry.list())
}

/// Clear all document extractors from the global registry.
///
/// Calls `shutdown()` on every registered extractor, then empties the registry.
///
/// # Errors
///
/// - Any error returned by an extractor's `shutdown()` method. The first error
///   encountered stops processing of remaining extractors.
pub fn clear_document_extractors() -> crate::Result<()> {
    use crate::plugins::registry::get_document_extractor_registry;

    let registry = get_document_extractor_registry();
    let mut registry = registry.write();
    registry.shutdown_all()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Result;
    use crate::core::config::ExtractionConfig;
    use crate::plugins::Plugin;
    use async_trait::async_trait;

    struct MockExtractor {
        mime_types: Vec<&'static str>,
        priority: i32,
    }

    impl Plugin for MockExtractor {
        fn name(&self) -> &str {
            "mock-extractor"
        }

        fn version(&self) -> String {
            "1.0.0".to_string()
        }

        fn initialize(&self) -> Result<()> {
            Ok(())
        }

        fn shutdown(&self) -> Result<()> {
            Ok(())
        }
    }

    #[async_trait]
    impl InternalDocumentExtractor for MockExtractor {
        async fn extract_content(
            &self,
            content: &[u8],
            mime_type: &str,
            _config: &ExtractionConfig,
        ) -> Result<crate::types::internal::InternalDocument> {
            let mut doc = crate::types::internal::InternalDocument::new("mock");
            doc.mime_type = mime_type.to_string();
            let text = String::from_utf8_lossy(content).to_string();
            if !text.is_empty() {
                doc.push_element(crate::types::internal::InternalElement::text(
                    crate::types::internal::ElementKind::Paragraph,
                    text,
                    0,
                ));
            }
            Ok(doc)
        }

        fn supported_mime_types(&self) -> &[&str] {
            &self.mime_types
        }

        fn priority(&self) -> i32 {
            self.priority
        }
    }

    #[tokio::test]
    async fn test_internal_document_extractor_extract_content() {
        let extractor = MockExtractor {
            mime_types: vec!["text/plain"],
            priority: 50,
        };

        let config = ExtractionConfig::default();
        let doc = extractor
            .extract_content(b"test content", "text/plain", &config)
            .await
            .unwrap();
        let result =
            crate::extraction::derive::derive_extraction_result(doc, true, crate::core::config::OutputFormat::Plain);

        assert_eq!(result.content, "test content");
        assert_eq!(result.mime_type, "text/plain");
    }

    #[test]
    fn test_document_extractor_supported_mime_types() {
        let extractor = MockExtractor {
            mime_types: vec!["text/plain", "text/markdown"],
            priority: 50,
        };

        let supported = InternalDocumentExtractor::supported_mime_types(&extractor);
        assert_eq!(supported.len(), 2);
        assert!(supported.contains(&"text/plain"));
        assert!(supported.contains(&"text/markdown"));
    }

    #[test]
    fn test_document_extractor_priority() {
        let low_priority = MockExtractor {
            mime_types: vec!["text/plain"],
            priority: 10,
        };

        let high_priority = MockExtractor {
            mime_types: vec!["text/plain"],
            priority: 100,
        };

        assert_eq!(InternalDocumentExtractor::priority(&low_priority), 10);
        assert_eq!(InternalDocumentExtractor::priority(&high_priority), 100);
    }

    #[test]
    fn test_document_extractor_can_handle_default() {
        let extractor = MockExtractor {
            mime_types: vec!["text/plain"],
            priority: 50,
        };

        use std::path::PathBuf;
        let path = PathBuf::from("test.txt");

        assert!(InternalDocumentExtractor::can_handle(&extractor, &path, "text/plain"));
        assert!(InternalDocumentExtractor::can_handle(
            &extractor,
            &path,
            "application/pdf"
        ));
    }

    #[tokio::test]
    async fn test_internal_document_extractor_extract_path_default_impl() {
        use std::io::Write;
        use tempfile::NamedTempFile;

        let extractor = MockExtractor {
            mime_types: vec!["text/plain"],
            priority: 50,
        };

        let mut temp_file = NamedTempFile::new().unwrap();
        temp_file.write_all(b"file content").unwrap();
        let path = temp_file.path();

        let config = ExtractionConfig::default();
        let doc = extractor.extract_path(path, "text/plain", &config).await.unwrap();
        let result =
            crate::extraction::derive::derive_extraction_result(doc, true, crate::core::config::OutputFormat::Plain);

        assert_eq!(result.content, "file content");
        assert_eq!(result.mime_type, "text/plain");
    }

    #[tokio::test]
    async fn test_document_extractor_empty_content() {
        let extractor = MockExtractor {
            mime_types: vec!["text/plain"],
            priority: 50,
        };

        let config = ExtractionConfig::default();
        let doc = extractor.extract_content(b"", "text/plain", &config).await.unwrap();
        let result =
            crate::extraction::derive::derive_extraction_result(doc, true, crate::core::config::OutputFormat::Plain);

        assert_eq!(result.content, "");
        assert_eq!(result.mime_type, "text/plain");
    }

    #[tokio::test]
    async fn test_document_extractor_non_utf8_content() {
        let extractor = MockExtractor {
            mime_types: vec!["text/plain"],
            priority: 50,
        };

        let invalid_utf8 = vec![0xFF, 0xFE, 0xFD];
        let config = ExtractionConfig::default();
        let doc = extractor
            .extract_content(&invalid_utf8, "text/plain", &config)
            .await
            .unwrap();
        let result =
            crate::extraction::derive::derive_extraction_result(doc, true, crate::core::config::OutputFormat::Plain);

        assert!(!result.content.is_empty());
    }

    #[test]
    fn test_document_extractor_plugin_interface() {
        let extractor = MockExtractor {
            mime_types: vec!["text/plain"],
            priority: 50,
        };

        assert_eq!(extractor.name(), "mock-extractor");
        assert_eq!(extractor.version(), "1.0.0");
        assert!(extractor.initialize().is_ok());
        assert!(extractor.shutdown().is_ok());
    }

    #[test]
    fn test_document_extractor_default_priority() {
        struct DefaultPriorityExtractor;

        impl Plugin for DefaultPriorityExtractor {
            fn name(&self) -> &str {
                "default"
            }
            fn version(&self) -> String {
                "1.0.0".to_string()
            }
            fn initialize(&self) -> Result<()> {
                Ok(())
            }
            fn shutdown(&self) -> Result<()> {
                Ok(())
            }
        }

        #[async_trait]
        impl InternalDocumentExtractor for DefaultPriorityExtractor {
            async fn extract_content(
                &self,
                _content: &[u8],
                _mime_type: &str,
                _config: &ExtractionConfig,
            ) -> Result<crate::types::internal::InternalDocument> {
                Ok(crate::types::internal::InternalDocument::new("mock"))
            }

            fn supported_mime_types(&self) -> &[&str] {
                &["text/plain"]
            }
        }

        let extractor = DefaultPriorityExtractor;
        assert_eq!(InternalDocumentExtractor::priority(&extractor), 50);
    }

    #[test]
    fn test_document_extractor_empty_mime_types() {
        let extractor = MockExtractor {
            mime_types: vec![],
            priority: 50,
        };

        assert_eq!(InternalDocumentExtractor::supported_mime_types(&extractor).len(), 0);
    }

    #[test]
    fn test_document_extractor_wildcard_mime_types() {
        let extractor = MockExtractor {
            mime_types: vec!["text/*", "image/*"],
            priority: 50,
        };

        let supported = InternalDocumentExtractor::supported_mime_types(&extractor);
        assert_eq!(supported.len(), 2);
        assert!(supported.contains(&"text/*"));
        assert!(supported.contains(&"image/*"));
    }

    #[test]
    fn test_document_extractor_priority_ranges() {
        let fallback = MockExtractor {
            mime_types: vec!["text/plain"],
            priority: 10,
        };
        let alternative = MockExtractor {
            mime_types: vec!["text/plain"],
            priority: 40,
        };
        let default = MockExtractor {
            mime_types: vec!["text/plain"],
            priority: 50,
        };
        let premium = MockExtractor {
            mime_types: vec!["text/plain"],
            priority: 75,
        };
        let specialized = MockExtractor {
            mime_types: vec!["text/plain"],
            priority: 100,
        };

        assert!(InternalDocumentExtractor::priority(&fallback) < InternalDocumentExtractor::priority(&alternative));
        assert!(InternalDocumentExtractor::priority(&alternative) < InternalDocumentExtractor::priority(&default));
        assert!(InternalDocumentExtractor::priority(&default) < InternalDocumentExtractor::priority(&premium));
        assert!(InternalDocumentExtractor::priority(&premium) < InternalDocumentExtractor::priority(&specialized));
    }

    #[tokio::test]
    async fn test_document_extractor_preserves_mime_type() {
        let extractor = MockExtractor {
            mime_types: vec!["application/json"],
            priority: 50,
        };

        let config = ExtractionConfig::default();
        let result = extractor
            .extract_content(b"{\"key\":\"value\"}", "application/json", &config)
            .await
            .unwrap();

        assert_eq!(result.mime_type, "application/json");
    }

    struct LifecycleMock {
        mime: String,
    }

    impl Plugin for LifecycleMock {
        fn name(&self) -> &str {
            &self.mime
        }
        fn version(&self) -> String {
            "1.0.0".to_string()
        }
        fn initialize(&self) -> Result<()> {
            Ok(())
        }
        fn shutdown(&self) -> Result<()> {
            Ok(())
        }
    }

    #[async_trait]
    impl InternalDocumentExtractor for LifecycleMock {
        async fn extract_content(
            &self,
            _content: &[u8],
            _mime_type: &str,
            _config: &ExtractionConfig,
        ) -> Result<crate::types::internal::InternalDocument> {
            Ok(crate::types::internal::InternalDocument::new("mock"))
        }

        fn supported_mime_types(&self) -> &[&str] {
            &[]
        }
    }

    /// Registers/unregisters a uniquely-named extractor against the *global* registry and
    /// checks membership rather than the full list, so the assertions hold regardless of
    /// whatever else is concurrently registered globally by other tests in this binary.
    #[test]
    fn register_list_unregister_roundtrip() {
        let mime = "application/x-mock-rlu".to_string();
        let extractor = Arc::new(LifecycleMock { mime: mime.clone() });

        register_document_extractor(Arc::clone(&extractor) as Arc<dyn DocumentExtractor>).unwrap();
        assert!(
            list_document_extractors().unwrap().contains(&mime),
            "the newly registered extractor must be listed"
        );

        unregister_document_extractor(&mime).unwrap();
        assert!(
            !list_document_extractors().unwrap().contains(&mime),
            "the unregistered extractor must no longer be listed"
        );
    }

    /// Exercises register/list/clear against a *local* registry instance instead of the
    /// process-global one, since `clear_document_extractors()` empties the global registry
    /// outright and would race any concurrently running extraction test that relies on it.
    #[test]
    fn register_list_clear_list_roundtrip() {
        use crate::plugins::registry::DocumentExtractorRegistry;

        let mime = "application/x-mock-rlcl".to_string();
        let extractor = Arc::new(LifecycleMock { mime: mime.clone() });

        let mut registry = DocumentExtractorRegistry::new();
        registry
            .register(Arc::clone(&extractor) as Arc<dyn DocumentExtractor>)
            .unwrap();
        assert_eq!(registry.list(), vec![mime]);

        registry.clear().unwrap();
        assert!(registry.list().is_empty());
    }
}
