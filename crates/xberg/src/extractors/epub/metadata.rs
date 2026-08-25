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
    pub(super) creators: Vec<String>,
    pub(super) date: Option<String>,
    pub(super) language: Option<String>,
    pub(super) identifier: Option<String>,
    pub(super) publisher: Option<String>,
    pub(super) subjects: Vec<String>,
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
    pub(super) fn is_renderable_body_document(&self) -> bool {
        matches!(
            self.media_type.as_deref(),
            Some("application/xhtml+xml") | Some("application/x-dtbook+xml")
        ) || self.media_type.is_none() && has_renderable_extension(&self.raw_href)
    }

    /// Returns true if this manifest item has the EPUB3 `nav` property.
    pub(super) fn is_nav(&self) -> bool {
        self.has_property("nav")
    }

    pub(super) fn has_property(&self, property: &str) -> bool {
        self.properties
            .as_deref()
            .is_some_and(|p| p.split_ascii_whitespace().any(|v| v.eq_ignore_ascii_case(property)))
    }

    /// True when the item is an image by media type, or by extension when the
    /// media type is missing.
    pub(super) fn is_image(&self) -> bool {
        match self.media_type.as_deref() {
            Some(media_type) => media_type.trim().to_ascii_lowercase().starts_with("image/"),
            None => self.raw_href.rsplit_once('.').is_some_and(|(_, ext)| {
                matches!(
                    ext.to_ascii_lowercase().as_str(),
                    "jpg" | "jpeg" | "png" | "gif" | "webp" | "svg" | "bmp"
                )
            }),
        }
    }

    /// True when the item is an SVG content document.
    pub(super) fn is_svg(&self) -> bool {
        self.media_type
            .as_deref()
            .is_some_and(|media_type| media_type.trim().eq_ignore_ascii_case("image/svg+xml"))
    }

    pub(super) fn resolved_path(&self) -> std::result::Result<&str, String> {
        self.path.as_deref().ok_or_else(|| {
            self.path_resolution_error
                .clone()
                .unwrap_or_else(|| format!("unable to resolve manifest href '{}'", self.raw_href))
        })
    }
}

/// Dublin Core element namespaces. EPUB 2 and 3 use the 1.1 elements
/// namespace; OEB 1.2 packages used the 1.0 one.
const DUBLIN_CORE_NAMESPACE_PREFIX: &str = "http://purl.org/dc/elements/";

/// Pick the publication date from every `dc:date`. EPUB 2 qualifies dates
/// with `opf:event`; a modification date is used only when no other date
/// exists.
fn select_publication_date(dates: Vec<(Option<String>, String)>) -> Option<String> {
    let preferred = dates.iter().find(|(event, _)| {
        event
            .as_deref()
            .is_none_or(|event| matches!(event, "publication" | "creation" | "original-publication"))
    });
    preferred.or_else(|| dates.first()).map(|(_, value)| value.clone())
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
            let mut dates: Vec<(Option<String>, String)> = Vec::new();
            let mut titles: Vec<(Option<String>, String)> = Vec::new();
            let mut main_title_id: Option<String> = None;
            let unique_identifier_id = root
                .descendants()
                .find(|node| node.tag_name().name() == "package")
                .and_then(|node| node.attribute("unique-identifier"));

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
                let is_dublin_core = node
                    .tag_name()
                    .namespace()
                    .is_some_and(|namespace| namespace.starts_with(DUBLIN_CORE_NAMESPACE_PREFIX));
                let local_name = node.tag_name().name().to_ascii_lowercase();
                if is_dublin_core {
                    let text = match node.text().map(str::trim).filter(|text| !text.is_empty()) {
                        Some(text) => {
                            budget.check_entity(text)?;
                            budget.account_text(text.len())?;
                            text.to_string()
                        }
                        None => continue,
                    };
                    let metadata = &mut package.metadata;
                    match local_name.as_str() {
                        "title" => {
                            titles.push((node.attribute("id").map(str::to_string), text));
                            continue;
                        }
                        "creator" => {
                            metadata.creators.push(text);
                            continue;
                        }
                        "date" => {
                            let event = node
                                .attributes()
                                .find(|attr| attr.name() == "event")
                                .map(|attr| attr.value());
                            dates.push((event.map(str::to_ascii_lowercase), text));
                            continue;
                        }
                        "language" => metadata.language.get_or_insert(text),
                        "identifier" => {
                            let is_unique = node.attribute("id").is_some_and(|id| Some(id) == unique_identifier_id);
                            if is_unique {
                                metadata.identifier = Some(text);
                            } else {
                                metadata.identifier.get_or_insert(text);
                            }
                            continue;
                        }
                        "publisher" => metadata.publisher.get_or_insert(text),
                        "subject" => {
                            metadata.subjects.push(text);
                            continue;
                        }
                        "description" => metadata.description.get_or_insert(text),
                        "rights" => metadata.rights.get_or_insert(text),
                        "coverage" => metadata.coverage.get_or_insert(text),
                        "format" => metadata.format.get_or_insert(text),
                        "relation" => metadata.relation.get_or_insert(text),
                        "source" => metadata.source.get_or_insert(text),
                        "type" => metadata.dc_type.get_or_insert(text),
                        _ => continue,
                    };
                    continue;
                }
                match local_name.as_str() {
                    // EPUB 3 marks the main title with a refining meta element.
                    "meta" => {
                        if node.attribute("property") == Some("title-type")
                            && node.text().map(str::trim) == Some("main")
                            && let Some(refined) = node.attribute("refines").and_then(|id| id.strip_prefix('#'))
                        {
                            main_title_id.get_or_insert(refined.to_string());
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

            package.metadata.date = select_publication_date(dates);
            package.metadata.title = titles
                .iter()
                .find(|(id, _)| id.is_some() && *id == main_title_id)
                .or_else(|| titles.first())
                .map(|(_, text)| text.clone());

            // EPUB 3 marks the cover with a manifest property; EPUB 2 points at it
            // from a meta element. Either way the item has to be an image: some
            // producers point the meta at the cover XHTML page instead.
            let cover_from_property = manifest
                .values()
                .find(|item| item.has_property("cover-image"))
                .filter(|item| item.is_image());
            let cover_from_meta = root
                .descendants()
                .find(|node| node.tag_name().name() == "meta" && node.attribute("name") == Some("cover"))
                .and_then(|node| node.attribute("content"))
                .and_then(|id| manifest.get(id))
                .filter(|item| item.is_image());
            if let Some(item) = cover_from_property.or(cover_from_meta)
                && let Ok(path) = item.resolved_path()
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

    if let Some(subject) = epub_metadata.subjects.first() {
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
        let xml = format!(
            r#"<package xmlns:dc="http://purl.org/dc/elements/1.1/"><metadata><dc:title>Book</dc:title></metadata><manifest>{siblings}</manifest></package>"#
        );
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

    fn parse(xml: &str) -> EpubPackageDocument {
        let mut budget = SecurityBudget::with_defaults();
        parse_opf(xml, "OEBPS", &mut budget).expect("OPF should parse").0
    }

    #[test]
    fn should_keep_the_first_title_and_every_creator_and_subject() {
        let package = parse(
            r#"<package xmlns="http://www.idpf.org/2007/opf" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:opf="http://www.idpf.org/2007/opf" unique-identifier="isbn">
  <metadata>
    <dc:title>Main Title</dc:title>
    <dc:title>Subtitle</dc:title>
    <dc:creator opf:role="aut">Alice Author</dc:creator>
    <dc:creator opf:role="ill">Ivan Illustrator</dc:creator>
    <dc:date opf:event="modification">2020-02-02</dc:date>
    <dc:date opf:event="publication">1999-01-01</dc:date>
    <dc:identifier id="uuid">urn:uuid:1</dc:identifier>
    <dc:identifier id="isbn">9780000000000</dc:identifier>
    <dc:subject>Fiction</dc:subject>
    <dc:subject>Adventure</dc:subject>
  </metadata>
  <collection role="series"><metadata><title>Series Name</title></metadata></collection>
</package>"#,
        );
        let metadata = package.metadata;
        assert_eq!(metadata.title.as_deref(), Some("Main Title"));
        assert_eq!(metadata.creators, vec!["Alice Author", "Ivan Illustrator"]);
        assert_eq!(metadata.date.as_deref(), Some("1999-01-01"));
        assert_eq!(metadata.identifier.as_deref(), Some("9780000000000"));
        assert_eq!(metadata.subjects, vec!["Fiction", "Adventure"]);
    }

    #[test]
    fn should_prefer_the_title_refined_as_main() {
        let package = parse(
            r##"<package xmlns="http://www.idpf.org/2007/opf" xmlns:dc="http://purl.org/dc/elements/1.1/" version="3.0">
  <metadata>
    <dc:title id="sub">A Subtitle</dc:title>
    <dc:title id="main">The Main Title</dc:title>
    <meta refines="#main" property="title-type">main</meta>
    <meta refines="#sub" property="title-type">subtitle</meta>
  </metadata>
</package>"##,
        );
        assert_eq!(package.metadata.title.as_deref(), Some("The Main Title"));
    }

    #[test]
    fn should_use_the_modification_date_only_when_no_other_date_exists() {
        let package = parse(
            r#"<package xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:opf="http://www.idpf.org/2007/opf"><metadata>
    <dc:date opf:event="modification">2020-02-02</dc:date>
</metadata></package>"#,
        );
        assert_eq!(package.metadata.date.as_deref(), Some("2020-02-02"));
    }

    #[test]
    fn should_read_oeb12_capitalised_dublin_core_elements() {
        let package = parse(
            r#"<package xmlns:dc="http://purl.org/dc/elements/1.0/"><metadata><dc-metadata>
    <dc:Title>Old Book</dc:Title><dc:Creator>Old Author</dc:Creator>
</dc-metadata></metadata></package>"#,
        );
        assert_eq!(package.metadata.title.as_deref(), Some("Old Book"));
        assert_eq!(package.metadata.creators, vec!["Old Author"]);
    }

    #[test]
    fn should_find_an_epub3_cover_image_property() {
        let package = parse(
            r#"<package xmlns:dc="http://purl.org/dc/elements/1.1/"><metadata><dc:title>T</dc:title></metadata>
<manifest>
  <item id="cover" href="images/cover.jpg" media-type="image/jpeg" properties="cover-image"/>
  <item id="c1" href="c1.xhtml" media-type="application/xhtml+xml"/>
</manifest></package>"#,
        );
        assert_eq!(
            package.metadata.cover_image_href.as_deref(),
            Some("OEBPS/images/cover.jpg")
        );
    }

    #[test]
    fn should_ignore_a_cover_meta_that_points_at_an_xhtml_page() {
        let package = parse(
            r#"<package xmlns:dc="http://purl.org/dc/elements/1.1/"><metadata><dc:title>T</dc:title><meta name="cover" content="coverpage"/></metadata>
<manifest>
  <item id="coverpage" href="cover.xhtml" media-type="application/xhtml+xml"/>
</manifest></package>"#,
        );
        assert_eq!(package.metadata.cover_image_href, None);
    }
}
