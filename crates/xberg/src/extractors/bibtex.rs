//! BibTeX bibliography extractor.
//!
//! Extracts and parses BibTeX bibliography files, providing structured access
//! to bibliography entries, metadata, and author information.

use crate::Result;
use crate::core::config::ExtractionConfig;
use crate::plugins::{InternalDocumentExtractor, Plugin};
use crate::types::internal::InternalDocument;
use crate::types::internal_builder::InternalDocumentBuilder;
use crate::types::metadata::{BibtexMetadata, FormatMetadata, Metadata, YearRange};
use crate::types::uri::ExtractedUri;
use ahash::AHashMap;
use ahash::AHashSet;
use async_trait::async_trait;
use std::borrow::Cow;
use std::collections::BTreeMap;

#[cfg(feature = "office")]
use crate::types::document_structure::{AnnotationKind, TextAnnotation};
#[cfg(feature = "office")]
use biblatex::{Bibliography, ChunksExt};
#[cfg_attr(alef, alef(skip))]
/// BibTeX bibliography extractor.
///
/// Parses BibTeX files and extracts structured bibliography data including
/// entries, authors, publication years, and entry type distribution.
pub struct BibtexExtractor;

impl BibtexExtractor {
    /// Create a new BibTeX extractor.
    pub(crate) fn new() -> Self {
        Self
    }
}

impl Default for BibtexExtractor {
    fn default() -> Self {
        Self::new()
    }
}

impl Plugin for BibtexExtractor {
    fn name(&self) -> &str {
        "bibtex-extractor"
    }

    fn version(&self) -> String {
        env!("CARGO_PKG_VERSION").to_string()
    }

    fn initialize(&self) -> Result<()> {
        Ok(())
    }

    fn shutdown(&self) -> Result<()> {
        Ok(())
    }

    fn description(&self) -> &str {
        "Extracts and parses BibTeX bibliography files with structured metadata"
    }

    fn author(&self) -> &str {
        "Xberg Team"
    }
}

#[cfg(feature = "office")]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
impl InternalDocumentExtractor for BibtexExtractor {
    #[cfg_attr(feature = "otel", tracing::instrument(
        skip(self, content, _config),
        fields(
            extractor.name = self.name(),
            content.size_bytes = content.len(),
        )
    ))]
    async fn extract_content(
        &self,
        content: &[u8],
        mime_type: &str,
        _config: &ExtractionConfig,
    ) -> Result<InternalDocument> {
        let bibtex_str = String::from_utf8_lossy(content);

        let mut entries_vec = Vec::new();
        let mut authors_set = AHashSet::new();
        let mut years_set = AHashSet::new();
        let mut entry_types_map: AHashMap<String, i32> = AHashMap::new();
        let mut formatted_entries = String::new();

        let mut builder = InternalDocumentBuilder::new("bibtex");

        match Bibliography::parse(&bibtex_str) {
            Ok(bib) => {
                for entry in bib.iter() {
                    let key = entry.key.clone();
                    let entry_type = entry.entry_type.clone();

                    let entry_start = formatted_entries.len();

                    let mut entry_fields: AHashMap<String, String> = AHashMap::new();
                    entry_fields.insert("entry_type".to_string(), entry_type.to_string());

                    formatted_entries.push_str(&format!("@{}{{{},\n", entry_type, key));

                    for (field_name, field_chunks) in &entry.fields {
                        let field_text = field_chunks.format_verbatim();
                        formatted_entries.push_str(&format!("  {} = {{{}}},\n", field_name, field_text));

                        entry_fields.insert(field_name.to_lowercase(), field_text.clone());

                        if field_name.to_lowercase() == "author" {
                            for author in field_text.split(" and ") {
                                let trimmed_author = author.trim().to_string();
                                if !trimmed_author.is_empty() {
                                    authors_set.insert(trimmed_author);
                                }
                            }
                        }

                        if field_name.to_lowercase() == "year"
                            && let Ok(year) = field_text.parse::<u32>()
                        {
                            years_set.insert(year);
                        }
                    }

                    formatted_entries.push_str("}\n\n");

                    let link_label = entry_fields
                        .get("title")
                        .filter(|t| !t.is_empty())
                        .cloned()
                        .unwrap_or_else(|| key.clone());

                    if let Some(url) = entry_fields.get("url")
                        && !url.is_empty()
                    {
                        builder.push_uri(ExtractedUri::hyperlink(url.as_str(), Some(link_label.clone())));
                    }
                    if let Some(doi) = entry_fields.get("doi")
                        && !doi.is_empty()
                    {
                        builder.push_uri(ExtractedUri::citation(
                            format!("https://doi.org/{}", doi),
                            Some(link_label.clone()),
                        ));
                    }

                    let citation_text = formatted_entries[entry_start..].trim().to_string();
                    let idx = builder.push_citation(&citation_text, &key, None);

                    let mut link_annotations = Vec::new();
                    let text_len = citation_text.len() as u32;

                    if let Some(url) = entry_fields.get("url")
                        && !url.is_empty()
                    {
                        link_annotations.push(TextAnnotation {
                            start: 0,
                            end: text_len,
                            kind: AnnotationKind::Link {
                                url: url.clone(),
                                title: Some(link_label.clone()),
                            },
                        });
                    }

                    if let Some(doi) = entry_fields.get("doi")
                        && !doi.is_empty()
                    {
                        let doi_url = if doi.starts_with("http") {
                            doi.clone()
                        } else {
                            format!("https://doi.org/{doi}")
                        };
                        link_annotations.push(TextAnnotation {
                            start: 0,
                            end: text_len,
                            kind: AnnotationKind::Link {
                                url: doi_url,
                                title: Some(link_label.clone()),
                            },
                        });
                    }

                    if !link_annotations.is_empty() {
                        builder.set_annotations(idx, link_annotations);
                    }

                    let fields_json: serde_json::Map<String, serde_json::Value> = entry_fields
                        .iter()
                        .map(|(k, v)| (k.clone(), serde_json::json!(v)))
                        .collect();

                    if !entry_fields.is_empty() {
                        builder.set_attributes(idx, std::mem::take(&mut entry_fields));
                    }

                    *entry_types_map
                        .entry(entry_type.to_string().to_lowercase())
                        .or_insert(0) += 1;

                    entries_vec.push((key, fields_json));
                }
            }
            Err(_err) => {
                #[cfg(feature = "otel")]
                tracing::warn!("BibTeX parsing failed, returning raw content: {}", _err);
                builder.add_warning(crate::core::diagnostics::warning(
                    "bibtex",
                    "BibTeX parsing failed; returning raw text as a fallback",
                ));
                formatted_entries = bibtex_str.to_string();
                builder.push_code(&formatted_entries, None, None, None);
            }
        }

        let citation_keys: Vec<String> = entries_vec.iter().map(|(k, _)| k.clone()).collect();

        let mut authors_list: Vec<String> = authors_set.into_iter().collect();
        authors_list.sort();

        let year_range = if !years_set.is_empty() {
            let min_year = years_set.iter().min().copied();
            let max_year = years_set.iter().max().copied();
            let mut years: Vec<u32> = years_set.into_iter().collect();
            years.sort_unstable();
            Some(YearRange {
                min: min_year,
                max: max_year,
                years,
            })
        } else {
            None
        };

        let entry_types = if !entry_types_map.is_empty() {
            let typed: BTreeMap<String, usize> = entry_types_map.into_iter().map(|(k, v)| (k, v as usize)).collect();
            Some(typed)
        } else {
            None
        };

        let bibtex_metadata = BibtexMetadata {
            entry_count: entries_vec.len(),
            citation_keys,
            authors: authors_list.clone(),
            year_range,
            entry_types,
        };

        let mut additional: AHashMap<Cow<'static, str>, serde_json::Value> = AHashMap::new();
        let entries_metadata: Vec<serde_json::Value> = entries_vec
            .iter()
            .map(|(key, fields)| {
                let mut entry_obj = serde_json::Map::new();
                entry_obj.insert("key".to_string(), serde_json::json!(key));
                for (k, v) in fields {
                    entry_obj.insert(k.clone(), v.clone());
                }
                serde_json::Value::Object(entry_obj)
            })
            .collect();
        additional.insert(Cow::Borrowed("entries"), serde_json::json!(entries_metadata));

        let meta_authors = if authors_list.is_empty() {
            None
        } else {
            Some(authors_list)
        };

        let mut doc = builder.build();
        doc.mime_type = mime_type.to_string();
        doc.metadata = Metadata {
            authors: meta_authors,
            format: Some(FormatMetadata::Bibtex(bibtex_metadata)),
            additional,
            ..Default::default()
        };

        Ok(doc)
    }

    fn supported_mime_types(&self) -> &[&str] {
        &["application/x-bibtex", "text/x-bibtex", "application/x-biblatex"]
    }

    fn priority(&self) -> i32 {
        50
    }
}

#[cfg(all(test, feature = "office"))]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_can_extract_bibtex_mime_types() {
        let extractor = BibtexExtractor::new();
        let supported = extractor.supported_mime_types();

        assert!(supported.contains(&"application/x-bibtex"));
        assert!(supported.contains(&"text/x-bibtex"));
        assert!(supported.contains(&"application/x-biblatex"));
        assert_eq!(supported.len(), 3);
    }

    #[tokio::test]
    async fn test_extract_simple_bibtex() {
        let extractor = BibtexExtractor::new();
        let bibtex_content = br#"@article{key2023,
    title = {Sample Title},
    author = {John Doe},
    year = {2023}
}"#;

        let config = ExtractionConfig::default();
        let result = extractor
            .extract_content(bibtex_content, "application/x-bibtex", &config)
            .await;

        assert!(result.is_ok());
        let result = result.expect("Should extract valid BibTeX entry");

        let metadata = &result.metadata;
        if let Some(FormatMetadata::Bibtex(bib)) = &metadata.format {
            assert_eq!(bib.entry_count, 1);
        } else {
            panic!("Expected FormatMetadata::Bibtex");
        }
    }

    #[tokio::test]
    async fn test_extract_multiple_entries() {
        let extractor = BibtexExtractor::new();
        let bibtex_content = br#"@article{first2020,
    title = {First Paper},
    author = {Author One},
    year = {2020},
    journal = {Test Journal}
}

@book{second2021,
    title = {Test Book},
    author = {Author Two},
    year = {2021},
    publisher = {Test Publisher}
}

@inproceedings{third2022,
    title = {Conference Paper},
    author = {Author Three},
    year = {2022}
}"#;

        let config = ExtractionConfig::default();
        let result = extractor
            .extract_content(bibtex_content, "application/x-bibtex", &config)
            .await;

        assert!(result.is_ok());
        let result = result.expect("Should extract valid BibTeX entries");

        let metadata = &result.metadata;

        if let Some(FormatMetadata::Bibtex(bib)) = &metadata.format {
            assert_eq!(bib.entry_count, 3);
            assert_eq!(bib.citation_keys.len(), 3);
            if let Some(types) = &bib.entry_types {
                assert!(types.contains_key("article"));
                assert!(types.contains_key("book"));
                assert!(types.contains_key("inproceedings"));
            }
        } else {
            panic!("Expected FormatMetadata::Bibtex");
        }
    }

    #[tokio::test]
    async fn test_extract_article_entry() {
        let extractor = BibtexExtractor::new();
        let bibtex_content = br#"@article{einstein1905,
    author = {Albert Einstein},
    title = {On the Electrodynamics of Moving Bodies},
    journal = {Annalen der Physik},
    year = {1905},
    volume = {17},
    pages = {891-921}
}"#;

        let config = ExtractionConfig::default();
        let result = extractor
            .extract_content(bibtex_content, "application/x-bibtex", &config)
            .await;

        assert!(result.is_ok());
        let result = result.expect("Should extract valid article entry");

        let metadata = &result.metadata;
        if let Some(authors) = &metadata.authors {
            assert!(!authors.is_empty());
            assert!(authors[0].contains("Einstein"));
        }
    }

    #[tokio::test]
    async fn test_extract_book_entry() {
        let extractor = BibtexExtractor::new();
        let bibtex_content = br#"@book{knuth1984,
    author = {Donald E. Knuth},
    title = {The TeXbook},
    publisher = {Addison-Wesley},
    year = {1984}
}"#;

        let config = ExtractionConfig::default();
        let result = extractor
            .extract_content(bibtex_content, "application/x-bibtex", &config)
            .await;

        assert!(result.is_ok());
        let result = result.expect("Should extract valid book entry");

        let metadata = &result.metadata;
        if let Some(FormatMetadata::Bibtex(bib)) = &metadata.format {
            assert_eq!(bib.entry_count, 1);
            if let Some(yr) = &bib.year_range {
                assert_eq!(yr.min, Some(1984));
                assert_eq!(yr.max, Some(1984));
            }
        } else {
            panic!("Expected FormatMetadata::Bibtex");
        }
    }

    #[tokio::test]
    async fn test_extract_metadata() {
        let extractor = BibtexExtractor::new();
        let bibtex_content = br#"@article{paper1,
    author = {Alice Smith and Bob Jones},
    title = {Title 1},
    year = {2020}
}

@article{paper2,
    author = {Charlie Brown},
    title = {Title 2},
    year = {2021}
}

@book{book1,
    author = {David Lee},
    title = {Book Title},
    year = {2019}
}"#;

        let config = ExtractionConfig::default();
        let result = extractor
            .extract_content(bibtex_content, "application/x-bibtex", &config)
            .await;

        assert!(result.is_ok());
        let result = result.expect("Should extract valid metadata");
        let metadata = &result.metadata;

        if let Some(FormatMetadata::Bibtex(bib)) = &metadata.format {
            assert_eq!(bib.entry_count, 3);

            if let Some(authors) = &metadata.authors {
                assert!(authors.len() >= 4);
            }

            if let Some(yr) = &bib.year_range {
                assert_eq!(yr.min, Some(2019));
                assert_eq!(yr.max, Some(2021));
            }

            if let Some(types) = &bib.entry_types {
                assert_eq!(types.get("article"), Some(&2));
                assert_eq!(types.get("book"), Some(&1));
            }
        } else {
            panic!("Expected FormatMetadata::Bibtex");
        }
    }

    #[tokio::test]
    async fn test_empty_bibliography() {
        let extractor = BibtexExtractor::new();
        let bibtex_content = b"";

        let config = ExtractionConfig::default();
        let result = extractor
            .extract_content(bibtex_content, "application/x-bibtex", &config)
            .await;

        assert!(result.is_ok());
        let result = result.expect("Should extract empty bibliography");
        let metadata = &result.metadata;

        if let Some(FormatMetadata::Bibtex(bib)) = &metadata.format {
            assert_eq!(bib.entry_count, 0);
        } else {
            panic!("Expected FormatMetadata::Bibtex");
        }
    }

    #[tokio::test]
    async fn test_malformed_entry() {
        let extractor = BibtexExtractor::new();
        let bibtex_content = br#"@article{incomplete
    title = {Missing fields}

Some random text that's not valid BibTeX"#;

        let config = ExtractionConfig::default();
        let result = extractor
            .extract_content(bibtex_content, "application/x-bibtex", &config)
            .await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_multiple_authors_extraction() {
        let extractor = BibtexExtractor::new();
        let bibtex_content = br#"@article{collab2022,
    author = {First Author and Second Author and Third Author},
    title = {Collaborative Work},
    year = {2022}
}"#;

        let config = ExtractionConfig::default();
        let result = extractor
            .extract_content(bibtex_content, "application/x-bibtex", &config)
            .await;

        assert!(result.is_ok());
        let result = result.expect("Should extract multiple authors");
        let metadata = &result.metadata;

        if let Some(authors) = &metadata.authors {
            assert!(authors.len() >= 3);
        }
    }

    #[tokio::test]
    async fn test_bibtex_extractor_plugin_interface() {
        let extractor = BibtexExtractor::new();
        assert_eq!(extractor.name(), "bibtex-extractor");
        assert_eq!(extractor.version(), env!("CARGO_PKG_VERSION"));
        assert_eq!(extractor.priority(), 50);
        assert!(!extractor.supported_mime_types().is_empty());
    }

    #[test]
    fn test_bibtex_extractor_default() {
        let extractor = BibtexExtractor;
        assert_eq!(extractor.name(), "bibtex-extractor");
    }

    #[tokio::test]
    async fn test_bibtex_extractor_initialize_shutdown() {
        let extractor = BibtexExtractor::new();
        assert!(extractor.initialize().is_ok());
        assert!(extractor.shutdown().is_ok());
    }

    #[tokio::test]
    async fn test_bibtex_entry_fields_extraction() {
        let extractor = BibtexExtractor::new();
        let bibtex_content = br#"@article{einstein1905,
    author = {Albert Einstein},
    title = {On the Electrodynamics of Moving Bodies},
    journal = {Annalen der Physik},
    year = {1905},
    volume = {17},
    pages = {891-921},
    doi = {10.1002/andp.19053220806},
    publisher = {Wiley}
}"#;

        let config = ExtractionConfig::default();
        let result = extractor
            .extract_content(bibtex_content, "application/x-bibtex", &config)
            .await
            .expect("Should extract entry fields");

        let metadata = &result.metadata;

        let entries = metadata.additional.get(&Cow::Borrowed("entries"));
        assert!(entries.is_some(), "Should have entries metadata");
        let entries_array = entries
            .expect("entries key should be present")
            .as_array()
            .expect("entries should be an array");
        assert_eq!(entries_array.len(), 1);

        let entry = &entries_array[0];
        assert_eq!(
            entry
                .get("key")
                .expect("key field")
                .as_str()
                .expect("key should be string"),
            "einstein1905"
        );
        assert_eq!(
            entry
                .get("entry_type")
                .expect("entry_type field")
                .as_str()
                .expect("entry_type should be string"),
            "article"
        );
        assert!(entry.get("journal").is_some(), "Should have journal field");
        assert!(entry.get("volume").is_some(), "Should have volume field");
        assert!(entry.get("pages").is_some(), "Should have pages field");
        assert!(entry.get("doi").is_some(), "Should have doi field");
        assert!(entry.get("publisher").is_some(), "Should have publisher field");
    }

    #[tokio::test]
    async fn test_bibtex_document_structure_attributes() {
        let extractor = BibtexExtractor::new();
        let bibtex_content = br#"@book{knuth1984,
    author = {Donald E. Knuth},
    title = {The TeXbook},
    publisher = {Addison-Wesley},
    year = {1984},
    isbn = {0-201-13447-0}
}"#;

        let config = ExtractionConfig {
            include_document_structure: true,
            ..Default::default()
        };
        let result = extractor
            .extract_content(bibtex_content, "application/x-bibtex", &config)
            .await
            .expect("Should extract with document structure");

        assert!(!result.elements.is_empty(), "Document should have elements");
    }
}
