//! Metadata extraction from EPUB OPF files.
//!
//! Handles parsing of OPF (Open Packaging Format) files and extraction of
//! Dublin Core metadata following EPUB2 and EPUB3 standards.

use crate::Result;
use crate::extractors::security::SecurityBudget;
use crate::types::ProcessingWarning;
use std::collections::{BTreeMap, BTreeSet};

use super::parsing::resolve_path;

/// Metadata extracted from OPF (Open Packaging Format) file
#[derive(Debug, Default, Clone)]
pub(super) struct OepbMetadata {
    pub(super) title: Option<String>,
    pub(super) creator: Option<String>,
    pub(super) date: Option<String>,
    pub(super) language: Option<String>,
    pub(super) identifier: Option<String>,
    pub(super) publisher: Option<String>,
    pub(super) subject: Option<String>,
    pub(super) description: Option<String>,
    pub(super) rights: Option<String>,
    pub(super) coverage: Option<String>,
    pub(super) format: Option<String>,
    pub(super) relation: Option<String>,
    pub(super) source: Option<String>,
    pub(super) dc_type: Option<String>,
    pub(super) cover_image_href: Option<String>,
}

#[derive(Debug, Clone)]
pub(super) struct EpubPackageDocument {
    pub(super) metadata: OepbMetadata,
    pub(super) manifest: BTreeMap<String, ManifestItem>,
    pub(super) spine_items: Vec<EpubSpineItem>,
    guide_toc_paths: BTreeSet<String>,
}

#[allow(dead_code)]
impl EpubPackageDocument {
    pub(super) fn is_guide_toc_candidate_path(&self, path: &str) -> bool {
        self.guide_toc_paths.contains(path)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// A spine entry extracted from the OPF package document.
pub(super) struct EpubSpineItem {
    pub(super) idref: String,
}

#[derive(Debug, Clone)]
/// Manifest metadata used to enrich spine entries after OPF parsing.
pub(super) struct ManifestItem {
    pub(super) raw_href: String,
    pub(super) path: Option<String>,
    path_resolution_error: Option<String>,
    #[allow(dead_code)]
    pub(super) media_type: Option<String>,
    #[allow(dead_code)]
    pub(super) fallback: Option<String>,
    pub(super) properties: Option<String>,
}

#[allow(dead_code)]
impl ManifestItem {
    /// `text/html` is not an EPUB core media type, but real-world EPUB 3 files
    /// (Internet Archive builds, for one) declare every page with it while the
    /// payload is XHTML. The spine loop parses the payload as XML either way, so
    /// accepting the label costs nothing and rescues whole books. ~keep
    pub(super) fn is_renderable_body_document(&self) -> bool {
        match self.normalized_media_type() {
            Some(media_type) => matches!(
                media_type.as_str(),
                "application/xhtml+xml" | "application/x-dtbook+xml" | "text/html" | "text/xml" | "application/xml"
            ),
            None => has_renderable_extension(&self.raw_href),
        }
    }

    /// The media type without parameters, whitespace, or case. `None` when the
    /// attribute is missing or empty, so the extension decides.
    fn normalized_media_type(&self) -> Option<String> {
        self.media_type
            .as_deref()
            .and_then(|media_type| media_type.split(';').next())
            .map(|media_type| media_type.trim().to_ascii_lowercase())
            .filter(|media_type| !media_type.is_empty())
    }

    /// Returns true if this manifest item has the EPUB3 `nav` property.
    pub(super) fn is_nav(&self) -> bool {
        self.properties
            .as_deref()
            .is_some_and(|p| p.split_ascii_whitespace().any(|v| v.eq_ignore_ascii_case("nav")))
    }

    pub(super) fn resolved_path(&self) -> std::result::Result<&str, String> {
        self.path.as_deref().ok_or_else(|| {
            self.path_resolution_error
                .clone()
                .unwrap_or_else(|| format!("unable to resolve manifest href '{}'", self.raw_href))
        })
    }
}

#[allow(dead_code)]
fn has_renderable_extension(href: &str) -> bool {
    let href = href
        .split_once('#')
        .map(|(path, _)| path)
        .unwrap_or(href)
        .rsplit('/')
        .next()
        .unwrap_or(href);

    href.rsplit_once('.')
        .map(|(_, ext)| {
            matches!(
                ext.to_ascii_lowercase().as_str(),
                "xhtml" | "html" | "htm" | "xml" | "dtbook"
            )
        })
        .unwrap_or(false)
}

/// Parse OPF file and extract metadata and spine order
pub(super) fn parse_opf(
    xml: &str,
    opf_dir: &str,
    budget: &mut SecurityBudget,
) -> Result<(EpubPackageDocument, Vec<ProcessingWarning>)> {
    match super::parsing::parse_packaging_xml(xml) {
        Ok(doc) => {
            let root = doc.root();

            let mut warnings = Vec::new();
            let mut package = EpubPackageDocument {
                metadata: OepbMetadata::default(),
                manifest: BTreeMap::new(),
                spine_items: Vec::new(),
                guide_toc_paths: BTreeSet::new(),
            };
            let mut manifest: BTreeMap<String, ManifestItem> = BTreeMap::new();
            let mut budget_depth = 0;

            for node in root.descendants() {
                budget.step()?;
                if node.is_element() {
                    let node_depth = node.ancestors().filter(|ancestor| ancestor.is_element()).count();
                    while budget_depth > node_depth {
                        budget.leave();
                        budget_depth -= 1;
                    }
                    while budget_depth < node_depth {
                        budget.enter()?;
                        budget_depth += 1;
                    }
                    for attr in node.attributes() {
                        budget.check_attr(attr.name(), attr.value())?;
                    }
                }
                match node.tag_name().name() {
                    "title" => {
                        if let Some(text) = node.text() {
                            budget.check_entity(text)?;
                            budget.account_text(text.len())?;
                            package.metadata.title = Some(text.trim().to_string());
                        }
                    }
                    "creator" => {
                        if let Some(text) = node.text() {
                            budget.check_entity(text)?;
                            budget.account_text(text.len())?;
                            package.metadata.creator = Some(text.trim().to_string());
                        }
                    }
                    "date" => {
                        if let Some(text) = node.text() {
                            budget.check_entity(text)?;
                            budget.account_text(text.len())?;
                            package.metadata.date = Some(text.trim().to_string());
                        }
                    }
                    "language" => {
                        if let Some(text) = node.text() {
                            budget.check_entity(text)?;
                            budget.account_text(text.len())?;
                            package.metadata.language = Some(text.trim().to_string());
                        }
                    }
                    "identifier" => {
                        if let Some(text) = node.text() {
                            budget.check_entity(text)?;
                            budget.account_text(text.len())?;
                            package.metadata.identifier = Some(text.trim().to_string());
                        }
                    }
                    "publisher" => {
                        if let Some(text) = node.text() {
                            budget.check_entity(text)?;
                            budget.account_text(text.len())?;
                            package.metadata.publisher = Some(text.trim().to_string());
                        }
                    }
                    "subject" => {
                        if let Some(text) = node.text() {
                            budget.check_entity(text)?;
                            budget.account_text(text.len())?;
                            package.metadata.subject = Some(text.trim().to_string());
                        }
                    }
                    "description" => {
                        if let Some(text) = node.text() {
                            budget.check_entity(text)?;
                            budget.account_text(text.len())?;
                            package.metadata.description = Some(text.trim().to_string());
                        }
                    }
                    "rights" => {
                        if let Some(text) = node.text() {
                            budget.check_entity(text)?;
                            budget.account_text(text.len())?;
                            package.metadata.rights = Some(text.trim().to_string());
                        }
                    }
                    "coverage" => {
                        if let Some(text) = node.text() {
                            budget.check_entity(text)?;
                            budget.account_text(text.len())?;
                            package.metadata.coverage = Some(text.trim().to_string());
                        }
                    }
                    "format" => {
                        if let Some(text) = node.text() {
                            budget.check_entity(text)?;
                            budget.account_text(text.len())?;
                            package.metadata.format = Some(text.trim().to_string());
                        }
                    }
                    "relation" => {
                        if let Some(text) = node.text() {
                            budget.check_entity(text)?;
                            budget.account_text(text.len())?;
                            package.metadata.relation = Some(text.trim().to_string());
                        }
                    }
                    "source" => {
                        if let Some(text) = node.text() {
                            budget.check_entity(text)?;
                            budget.account_text(text.len())?;
                            package.metadata.source = Some(text.trim().to_string());
                        }
                    }
                    "type" => {
                        if let Some(text) = node.text() {
                            budget.check_entity(text)?;
                            budget.account_text(text.len())?;
                            package.metadata.dc_type = Some(text.trim().to_string());
                        }
                    }
                    "item" => {
                        if let Some(id) = node.attribute("id")
                            && let Some(href) = node.attribute("href")
                        {
                            let (path, path_resolution_error) = match resolve_path(opf_dir, href) {
                                Ok(resolved_href) => (Some(resolved_href.path), None),
                                Err(err) => (None, Some(err.to_string())),
                            };
                            manifest.insert(
                                id.to_string(),
                                ManifestItem {
                                    raw_href: href.to_string(),
                                    path,
                                    path_resolution_error,
                                    media_type: node.attribute("media-type").map(ToString::to_string),
                                    fallback: node.attribute("fallback").map(ToString::to_string),
                                    properties: node.attribute("properties").map(ToString::to_string),
                                },
                            );
                        }
                    }
                    "reference" => {
                        if node
                            .attribute("type")
                            .is_some_and(|kind| kind.eq_ignore_ascii_case("toc"))
                            && let Some(href) = node.attribute("href")
                        {
                            match resolve_path(opf_dir, href) {
                                Ok(resolved_href) => {
                                    package.guide_toc_paths.insert(resolved_href.path);
                                }
                                Err(e) => {
                                    warnings.push(ProcessingWarning {
                                        source: std::borrow::Cow::Borrowed("epub"),
                                        message: std::borrow::Cow::Owned(format!(
                                            "Skipping malformed guide reference '{}': {}",
                                            href, e
                                        )),
                                    });
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
            while budget_depth > 0 {
                budget.leave();
                budget_depth -= 1;
            }

            let mut cover_item_id = None;
            for node in root.descendants() {
                if node.tag_name().name() == "meta"
                    && node.attribute("name") == Some("cover")
                    && let Some(content) = node.attribute("content")
                {
                    cover_item_id = Some(content.to_string());
                    break;
                }
            }

            if let Some(cover_id) = cover_item_id
                && let Some(href) = manifest.get(&cover_id)
                && let Ok(path) = href.resolved_path()
            {
                package.metadata.cover_image_href = Some(path.to_string());
            }

            for node in root.descendants() {
                if node.tag_name().name() == "itemref"
                    && let Some(idref) = node.attribute("idref")
                {
                    package.spine_items.push(EpubSpineItem {
                        idref: idref.to_string(),
                    });
                }
            }

            package.manifest = manifest;
            Ok((package, warnings))
        }
        Err(e) => Err(crate::XbergError::Parsing {
            message: format!("Failed to parse OPF file: {}", e),
            source: None,
        }),
    }
}

/// Convert parsed EPUB metadata into the extractor's generic metadata map.
pub(super) fn build_additional_metadata(epub_metadata: &OepbMetadata) -> BTreeMap<String, serde_json::Value> {
    let mut additional_metadata = BTreeMap::new();

    if let Some(ref identifier) = epub_metadata.identifier {
        additional_metadata.insert("identifier".to_string(), serde_json::json!(identifier.clone()));
    }

    if let Some(ref publisher) = epub_metadata.publisher {
        additional_metadata.insert("publisher".to_string(), serde_json::json!(publisher.clone()));
    }

    if let Some(ref subject) = epub_metadata.subject {
        additional_metadata.insert("subject".to_string(), serde_json::json!(subject.clone()));
    }

    if let Some(ref description) = epub_metadata.description {
        additional_metadata.insert("description".to_string(), serde_json::json!(description.clone()));
    }

    if let Some(ref rights) = epub_metadata.rights {
        additional_metadata.insert("rights".to_string(), serde_json::json!(rights.clone()));
    }

    additional_metadata
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extractors::security::SecurityLimits;

    #[test]
    fn should_count_opf_siblings_as_equal_depth() {
        let siblings = (0..100)
            .map(|index| format!(r#"<item id="item{index}" href="chapter{index}.xhtml"/>"#))
            .collect::<String>();
        let xml =
            format!(r#"<package><metadata><title>Book</title></metadata><manifest>{siblings}</manifest></package>"#);
        let limits = SecurityLimits {
            max_nesting_depth: 4,
            max_xml_depth: 4,
            ..SecurityLimits::default()
        };
        let mut budget = SecurityBudget::from_limits(&limits);

        let (package, warnings) =
            parse_opf(&xml, "OEBPS", &mut budget).expect("a shallow OPF with many sibling elements should parse");

        assert_eq!(package.manifest.len(), 100);
        assert_eq!(package.metadata.title.as_deref(), Some("Book"));
        assert!(warnings.is_empty());
    }

    #[test]
    fn should_reject_opf_nesting_beyond_security_limit() {
        let limits = SecurityLimits {
            max_nesting_depth: 3,
            max_xml_depth: 3,
            ..SecurityLimits::default()
        };
        let mut budget = SecurityBudget::from_limits(&limits);
        let xml = "<package><metadata><wrapper><title>Book</title></wrapper></metadata></package>";

        let error = parse_opf(xml, "OEBPS", &mut budget)
            .expect_err("an OPF deeper than the configured limit should be rejected");

        assert!(error.to_string().contains("Nesting too deep"));
    }

    #[test]
    fn should_treat_text_html_manifest_items_as_renderable() {
        let xml = r#"<package><metadata><title>Book</title></metadata><manifest>
            <item id="page" href="page_0.html" media-type="text/html"/>
            <item id="css" href="style.css" media-type="text/css"/>
        </manifest></package>"#;
        let mut budget = SecurityBudget::from_limits(&SecurityLimits::default());

        let (package, _) = parse_opf(xml, "EPUB", &mut budget).expect("OPF should parse");

        assert!(package.manifest["page"].is_renderable_body_document());
        assert!(!package.manifest["css"].is_renderable_body_document());
    }

    #[test]
    fn should_normalise_media_types_before_matching() {
        let item = |media_type: Option<&str>, href: &str| ManifestItem {
            raw_href: href.to_string(),
            path: Some(href.to_string()),
            path_resolution_error: None,
            media_type: media_type.map(str::to_string),
            fallback: None,
            properties: None,
        };
        for accepted in [
            "application/xhtml+xml",
            "application/xhtml+xml; charset=utf-8",
            "Application/XHTML+XML",
            " application/xhtml+xml ",
            "text/html",
            "text/xml",
            "application/xml",
            "application/x-dtbook+xml",
        ] {
            assert!(
                item(Some(accepted), "x.bin").is_renderable_body_document(),
                "{accepted:?}"
            );
        }
        for rejected in ["text/css", "image/jpeg", "image/svg+xml", "application/octet-stream"] {
            assert!(
                !item(Some(rejected), "x.xhtml").is_renderable_body_document(),
                "{rejected:?}"
            );
        }
        assert!(
            item(Some(""), "x.xhtml").is_renderable_body_document(),
            "empty attribute uses the extension"
        );
        assert!(!item(Some(""), "x.css").is_renderable_body_document());
        assert!(item(None, "x.html").is_renderable_body_document());
    }

}
