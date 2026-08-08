//! Citation format extractors for RIS, PubMed/MEDLINE, and EndNote XML.
//!
//! Extracts and parses citation files in various formats, providing structured access
//! to bibliography entries, metadata, and author information.

use crate::Result;
use crate::core::config::ExtractionConfig;
use crate::plugins::{InternalDocumentExtractor, Plugin};
use crate::types::internal::InternalDocument;
use crate::types::internal_builder::InternalDocumentBuilder;
use crate::types::metadata::{CitationMetadata, FormatMetadata, Metadata, YearRange};
use crate::types::uri::ExtractedUri;
use ahash::AHashSet;
use async_trait::async_trait;

#[cfg(feature = "office")]
use biblib::{CitationParser, EndNoteXmlParser, PubMedParser, RisParser};
#[cfg_attr(alef, alef(skip))]
/// Citation format extractor for RIS, PubMed/MEDLINE, and EndNote XML formats.
///
/// Parses citation files and extracts structured bibliography data including
/// entries, authors, publication years, and format-specific metadata.
pub struct CitationExtractor;

impl CitationExtractor {
    /// Create a new citation extractor.
    pub(crate) fn new() -> Self {
        Self
    }
}

impl Default for CitationExtractor {
    fn default() -> Self {
        Self::new()
    }
}

impl Plugin for CitationExtractor {
    fn name(&self) -> &str {
        "citation-extractor"
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
        "Extracts and parses citation files (RIS, PubMed/MEDLINE, EndNote XML) with structured metadata"
    }

    fn author(&self) -> &str {
        "Xberg Team"
    }
}

#[cfg(feature = "office")]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
impl InternalDocumentExtractor for CitationExtractor {
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
        let citation_str = String::from_utf8_lossy(content);

        let mut citations_vec = Vec::new();
        let mut authors_set = AHashSet::new();
        let mut years_set = AHashSet::new();
        let mut dois_vec = Vec::new();
        let mut keywords_set = AHashSet::new();
        let mut formatted_content = String::new();
        let mut citation_details: Vec<String> = Vec::new();

        let (parse_result, format_string) = match mime_type {
            "application/x-research-info-systems" => (RisParser::new().parse(&citation_str), "RIS"),
            "application/x-pubmed" => (PubMedParser::new().parse(&citation_str), "PubMed"),
            "application/x-endnote+xml" => (EndNoteXmlParser::new().parse(&citation_str), "EndNote XML"),
            _ => return Err(crate::XbergError::UnsupportedFormat(mime_type.to_string())),
        };

        let mut builder = InternalDocumentBuilder::new("citation");

        match parse_result {
            Ok(citations) => {
                for citation in &citations {
                    citations_vec.push(citation.title.clone());
                    let entry_start = formatted_content.len();

                    for author in &citation.authors {
                        let author_name = if let Some(given) = &author.given_name {
                            format!("{} {}", given, author.name)
                        } else {
                            author.name.clone()
                        };
                        if !author_name.is_empty() {
                            authors_set.insert(author_name);
                        }
                    }

                    if let Some(date) = &citation.date
                        && date.year > 0
                    {
                        years_set.insert(date.year as u32);
                    }

                    if let Some(doi) = &citation.doi
                        && !doi.is_empty()
                    {
                        dois_vec.push(doi.clone());
                        builder.push_uri(ExtractedUri::citation(
                            format!("https://doi.org/{}", doi),
                            Some(citation.title.clone()),
                        ));
                    }

                    for keyword in &citation.keywords {
                        if !keyword.is_empty() {
                            keywords_set.insert(keyword.clone());
                        }
                    }

                    if !citation.title.is_empty() {
                        formatted_content.push_str(&format!("Title: {}\n", citation.title));
                    }

                    if !citation.authors.is_empty() {
                        let author_strings: Vec<String> = citation
                            .authors
                            .iter()
                            .map(|a| {
                                if let Some(given) = &a.given_name {
                                    format!("{} {}", given, a.name)
                                } else {
                                    a.name.clone()
                                }
                            })
                            .collect();
                        formatted_content.push_str(&format!("Authors: {}\n", author_strings.join(", ")));
                    }

                    if let Some(journal) = &citation.journal {
                        formatted_content.push_str(&format!("Journal: {}\n", journal));
                    }

                    if let Some(date) = &citation.date {
                        formatted_content.push_str(&format!("Year: {}\n", date.year));
                    }

                    if let Some(volume) = &citation.volume {
                        formatted_content.push_str(&format!("Volume: {}", volume));
                        if let Some(issue) = &citation.issue {
                            formatted_content.push_str(&format!(", Issue: {}", issue));
                        }
                        if let Some(pages) = &citation.pages {
                            formatted_content.push_str(&format!(", Pages: {}", pages));
                        }
                        formatted_content.push('\n');
                    }

                    if let Some(doi) = &citation.doi {
                        formatted_content.push_str(&format!("DOI: {}\n", doi));
                    }

                    if let Some(pmid) = &citation.pmid {
                        formatted_content.push_str(&format!("PMID: {}\n", pmid));
                    }

                    if let Some(abstract_text) = &citation.abstract_text
                        && !abstract_text.is_empty()
                    {
                        formatted_content.push_str(&format!("Abstract: {}\n", abstract_text));
                    }

                    if !citation.keywords.is_empty() {
                        formatted_content.push_str(&format!("Keywords: {}\n", citation.keywords.join(", ")));
                    }

                    citation_details.push(formatted_content[entry_start..].trim().to_string());

                    formatted_content.push_str("---\n");
                }

                for (i, (title, detail)) in citations_vec.iter().zip(citation_details.iter()).enumerate() {
                    let key = if title.is_empty() {
                        format!("citation_{}", i + 1)
                    } else {
                        title.clone()
                    };
                    builder.push_citation(detail, &key, None);
                }
            }
            Err(_err) => {
                #[cfg(feature = "otel")]
                tracing::warn!("Citation parsing failed, returning raw content: {}", _err);
                builder.add_warning(crate::core::diagnostics::warning(
                    "citation",
                    "Citation parsing failed; returning raw text as a fallback",
                ));
                formatted_content = citation_str.to_string();
                builder.push_code(&formatted_content, None, None, None);
            }
        }

        let mut authors_list: Vec<String> = authors_set.into_iter().collect();
        authors_list.sort();
        let meta_authors = if authors_list.is_empty() {
            None
        } else {
            Some(authors_list.clone())
        };

        let year_range = if !years_set.is_empty() {
            let min_year = years_set.iter().min().copied();
            let max_year = years_set.iter().max().copied();
            let mut years_sorted: Vec<u32> = years_set.into_iter().collect();
            years_sorted.sort_unstable();
            Some(YearRange {
                min: min_year,
                max: max_year,
                years: years_sorted,
            })
        } else {
            None
        };

        let mut keywords_list: Vec<String> = keywords_set.into_iter().collect();
        keywords_list.sort();
        let meta_keywords = if keywords_list.is_empty() {
            None
        } else {
            Some(keywords_list.clone())
        };

        let citation_metadata = CitationMetadata {
            citation_count: citations_vec.len(),
            format: Some(format_string.to_string()),
            authors: authors_list,
            year_range,
            dois: dois_vec,
            keywords: keywords_list,
        };

        let mut doc = builder.build();
        doc.mime_type = mime_type.to_string();
        doc.metadata = Metadata {
            authors: meta_authors,
            keywords: meta_keywords,
            format: Some(FormatMetadata::Citation(citation_metadata)),
            ..Default::default()
        };

        Ok(doc)
    }

    fn supported_mime_types(&self) -> &[&str] {
        &[
            "application/x-research-info-systems",
            "application/x-pubmed",
            "application/x-endnote+xml",
        ]
    }

    fn priority(&self) -> i32 {
        60
    }
}

#[cfg(all(test, feature = "office"))]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_can_extract_citation_mime_types() {
        let extractor = CitationExtractor::new();
        let supported = extractor.supported_mime_types();

        assert!(supported.contains(&"application/x-research-info-systems"));
        assert!(supported.contains(&"application/x-pubmed"));
        assert!(supported.contains(&"application/x-endnote+xml"));
        assert_eq!(supported.len(), 3);
    }

    #[tokio::test]
    async fn test_extract_simple_ris() {
        let extractor = CitationExtractor::new();
        let ris_content = br#"TY  - JOUR
TI  - Sample Title
AU  - Smith, John
PY  - 2023
ER  -"#;

        let config = ExtractionConfig::default();
        let result = extractor
            .extract_content(ris_content, "application/x-research-info-systems", &config)
            .await;

        assert!(result.is_ok());
        let result = result.expect("Should extract valid RIS entry");

        let metadata = &result.metadata;
        if let Some(FormatMetadata::Citation(cit)) = &metadata.format {
            assert_eq!(cit.citation_count, 1);
            assert_eq!(cit.format.as_deref(), Some("RIS"));
        } else {
            panic!("Expected FormatMetadata::Citation");
        }
    }

    #[tokio::test]
    async fn test_extract_multiple_ris_entries() {
        let extractor = CitationExtractor::new();
        let ris_content = br#"TY  - JOUR
TI  - First Paper
AU  - Author One
PY  - 2020
ER  -

TY  - JOUR
TI  - Second Paper
AU  - Author Two
PY  - 2021
ER  -"#;

        let config = ExtractionConfig::default();
        let result = extractor
            .extract_content(ris_content, "application/x-research-info-systems", &config)
            .await;

        assert!(result.is_ok());
        let result = result.expect("Should extract multiple RIS entries");

        let metadata = &result.metadata;

        if let Some(FormatMetadata::Citation(cit)) = &metadata.format {
            assert_eq!(cit.citation_count, 2);
            if let Some(yr) = &cit.year_range {
                assert_eq!(yr.min, Some(2020));
                assert_eq!(yr.max, Some(2021));
            }
        } else {
            panic!("Expected FormatMetadata::Citation");
        }
    }

    #[tokio::test]
    async fn test_extract_ris_with_doi() {
        let extractor = CitationExtractor::new();
        let ris_content = br#"TY  - JOUR
TI  - Sample Article
AU  - Smith, John
DO  - 10.1234/example.doi
PY  - 2023
ER  -"#;

        let config = ExtractionConfig::default();
        let result = extractor
            .extract_content(ris_content, "application/x-research-info-systems", &config)
            .await;

        assert!(result.is_ok());
        let result = result.expect("Should extract RIS with DOI");

        let metadata = &result.metadata;
        if let Some(FormatMetadata::Citation(cit)) = &metadata.format {
            assert!(!cit.dois.is_empty());
        } else {
            panic!("Expected FormatMetadata::Citation");
        }
    }

    #[tokio::test]
    async fn test_extract_empty_citation_file() {
        let extractor = CitationExtractor::new();
        let empty_content = b"";

        let config = ExtractionConfig::default();
        let result = extractor
            .extract_content(empty_content, "application/x-research-info-systems", &config)
            .await;

        assert!(result.is_ok());
        let result = result.expect("Should handle empty citation file");

        let metadata = &result.metadata;
        if let Some(FormatMetadata::Citation(cit)) = &metadata.format {
            assert_eq!(cit.citation_count, 0);
        } else {
            panic!("Expected FormatMetadata::Citation");
        }
    }

    #[tokio::test]
    async fn test_extract_malformed_ris() {
        let extractor = CitationExtractor::new();
        let malformed_content = b"This is not valid RIS format\nJust some random text";

        let config = ExtractionConfig::default();
        let result = extractor
            .extract_content(malformed_content, "application/x-research-info-systems", &config)
            .await;

        assert!(result.is_ok());
        let result = result.expect("Should extract malformed as raw content");

        let metadata = &result.metadata;
        if let Some(FormatMetadata::Citation(cit)) = &metadata.format {
            assert_eq!(cit.citation_count, 0);
        } else {
            panic!("Expected FormatMetadata::Citation");
        }
    }

    #[tokio::test]
    async fn test_citation_extractor_plugin_interface() {
        let extractor = CitationExtractor::new();
        assert_eq!(extractor.name(), "citation-extractor");
        assert_eq!(extractor.version(), env!("CARGO_PKG_VERSION"));
        assert_eq!(extractor.priority(), 60);
        assert!(!extractor.supported_mime_types().is_empty());
    }

    #[test]
    fn test_citation_extractor_default() {
        let extractor = CitationExtractor;
        assert_eq!(extractor.name(), "citation-extractor");
    }

    #[tokio::test]
    async fn test_citation_extractor_initialize_shutdown() {
        let extractor = CitationExtractor::new();
        assert!(extractor.initialize().is_ok());
        assert!(extractor.shutdown().is_ok());
    }

    #[tokio::test]
    async fn test_extract_ris_with_keywords() {
        let extractor = CitationExtractor::new();
        let ris_content = br#"TY  - JOUR
TI  - Sample Article
AU  - Smith, John
KW  - keyword1
KW  - keyword2
KW  - keyword3
PY  - 2023
ER  -"#;

        let config = ExtractionConfig::default();
        let result = extractor
            .extract_content(ris_content, "application/x-research-info-systems", &config)
            .await;

        assert!(result.is_ok());
        let result = result.expect("Should extract RIS with keywords");

        let metadata = &result.metadata;
        if let Some(keywords) = &metadata.keywords {
            assert!(!keywords.is_empty());
        }
    }

    #[tokio::test]
    async fn test_extract_ris_with_multiple_authors() {
        let extractor = CitationExtractor::new();
        let ris_content = br#"TY  - JOUR
TI  - Collaborative Work
AU  - First Author
AU  - Second Author
AU  - Third Author
PY  - 2023
ER  -"#;

        let config = ExtractionConfig::default();
        let result = extractor
            .extract_content(ris_content, "application/x-research-info-systems", &config)
            .await;

        assert!(result.is_ok());
        let result = result.expect("Should extract multiple authors");

        let metadata = &result.metadata;
        if let Some(authors) = &metadata.authors {
            assert!(!authors.is_empty());
        }
    }

    #[tokio::test]
    async fn test_extract_pubmed_format() {
        let extractor = CitationExtractor::new();
        let pubmed_content = br#"PMID- 12345678
TI  - Sample PubMed Article
FAU - Smith, John
DP  - 2023"#;

        let config = ExtractionConfig::default();
        let result = extractor
            .extract_content(pubmed_content, "application/x-pubmed", &config)
            .await;

        assert!(result.is_ok());
        let result = result.expect("Should extract PubMed format");

        let metadata = &result.metadata;
        if let Some(FormatMetadata::Citation(cit)) = &metadata.format {
            assert_eq!(cit.format.as_deref(), Some("PubMed"));
        } else {
            panic!("Expected FormatMetadata::Citation");
        }
    }

    #[tokio::test]
    async fn test_extract_endnote_xml_format() {
        let extractor = CitationExtractor::new();
        let endnote_content = br#"<?xml version="1.0" encoding="UTF-8"?>
<xml>
  <records>
    <record>
      <titles>
        <title>Sample EndNote Article</title>
      </titles>
      <authors>
        <author>Smith, John</author>
      </authors>
    </record>
  </records>
</xml>"#;

        let config = ExtractionConfig::default();
        let result = extractor
            .extract_content(endnote_content, "application/x-endnote+xml", &config)
            .await;

        assert!(result.is_ok());
        let result = result.expect("Should extract EndNote XML format");

        let metadata = &result.metadata;
        if let Some(FormatMetadata::Citation(cit)) = &metadata.format {
            assert_eq!(cit.format.as_deref(), Some("EndNote XML"));
        } else {
            panic!("Expected FormatMetadata::Citation");
        }
    }
}
