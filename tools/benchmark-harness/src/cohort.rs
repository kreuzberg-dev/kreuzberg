//! Exact, ordered benchmark cohort manifests.

use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::corpus::{CorpusDocument, corpus_from_fixture_manager};
use crate::fixture::FixtureManager;
use crate::{Error, Result};

const COHORT_SCHEMA_VERSION: u32 = 1;

/// A reproducible ordered fixture selection and its fixed native batch size.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CohortManifest {
    pub schema_version: u32,
    pub name: String,
    pub batch_size: usize,
    /// Whether every fixture in this cohort is expected to exercise OCR. Absent in legacy
    /// manifests (defaults to `false`); the workflow reads it to decide the `--ocr` flag rather
    /// than matching on the cohort name.
    #[serde(default)]
    pub ocr_enabled: bool,
    pub fixtures: Vec<PathBuf>,
}

impl CohortManifest {
    /// Load and validate a cohort manifest.
    pub fn from_file(path: &Path) -> Result<Self> {
        let raw = std::fs::read_to_string(path)?;
        let manifest: Self = serde_json::from_str(&raw)?;
        manifest.validate(path)?;
        Ok(manifest)
    }

    /// Load the manifest's fixtures in exactly the declared order.
    pub fn load_fixtures(&self, fixture_root: &Path, manifest_path: &Path) -> Result<FixtureManager> {
        if !fixture_root.is_dir() {
            return Err(Error::Config(format!(
                "cohort fixture root must be a directory: {}",
                fixture_root.display()
            )));
        }

        let mut manager = FixtureManager::new();
        for relative in &self.fixtures {
            let resolved = fixture_root.join(relative);
            manager.load_fixture(&resolved).map_err(|error| {
                Error::Config(format!(
                    "cohort '{}' fixture '{}' from {} failed to load: {error}",
                    self.name,
                    relative.display(),
                    manifest_path.display()
                ))
            })?;
        }
        Ok(manager)
    }

    /// Load the manifest's fixtures as corpus documents in exactly the declared order.
    pub fn load_corpus(&self, fixture_root: &Path, manifest_path: &Path) -> Result<Vec<CorpusDocument>> {
        let manager = self.load_fixtures(fixture_root, manifest_path)?;
        Ok(corpus_from_fixture_manager(&manager))
    }

    fn validate(&self, path: &Path) -> Result<()> {
        if self.schema_version != COHORT_SCHEMA_VERSION {
            return Err(Error::Config(format!(
                "unsupported cohort schema_version {} in {}; expected {}",
                self.schema_version,
                path.display(),
                COHORT_SCHEMA_VERSION
            )));
        }
        if self.name.trim().is_empty() {
            return Err(Error::Config(format!(
                "cohort name must not be empty in {}",
                path.display()
            )));
        }
        if self.batch_size == 0 {
            return Err(Error::Config(format!(
                "cohort batch_size must be greater than zero in {}",
                path.display()
            )));
        }
        if self.fixtures.is_empty() {
            return Err(Error::Config(format!(
                "cohort fixtures must not be empty in {}",
                path.display()
            )));
        }
        if !self.fixtures.len().is_multiple_of(self.batch_size) {
            return Err(Error::Config(format!(
                "cohort '{}' contains {} fixtures, which is not divisible by fixed batch_size {}",
                self.name,
                self.fixtures.len(),
                self.batch_size
            )));
        }

        let mut seen = HashSet::with_capacity(self.fixtures.len());
        for fixture in &self.fixtures {
            let valid_relative = !fixture.as_os_str().is_empty()
                && !fixture.is_absolute()
                && fixture
                    .components()
                    .all(|component| matches!(component, Component::Normal(_)));
            if !valid_relative {
                return Err(Error::Config(format!(
                    "cohort fixture paths must be normalized relative paths without '..': {}",
                    fixture.display()
                )));
            }
            if fixture.extension().and_then(|extension| extension.to_str()) != Some("json") {
                return Err(Error::Config(format!(
                    "cohort fixture path must name a JSON descriptor: {}",
                    fixture.display()
                )));
            }
            if !seen.insert(fixture.clone()) {
                return Err(Error::Config(format!(
                    "cohort fixture path is duplicated: {}",
                    fixture.display()
                )));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_manifest(dir: &Path, value: serde_json::Value) -> PathBuf {
        let path = dir.join("cohort.json");
        std::fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        path
    }

    #[test]
    fn validates_ordered_fixed_size_cohort() {
        let temp = tempfile::tempdir().unwrap();
        let path = write_manifest(
            temp.path(),
            serde_json::json!({
                "schema_version": 1,
                "name": "ordered",
                "batch_size": 2,
                "fixtures": ["b.json", "a.json"]
            }),
        );
        let manifest = CohortManifest::from_file(&path).unwrap();
        assert_eq!(manifest.fixtures, [PathBuf::from("b.json"), PathBuf::from("a.json")]);
    }

    #[test]
    fn native_pdf_fast_b8_is_loadable_without_ocr() {
        const BATCH_SIZE: usize = 8;

        let crate_root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let manifest_path = crate_root.join("cohorts/native-pdf-fast-b8.json");
        let manifest = CohortManifest::from_file(&manifest_path).unwrap();
        let fixtures = manifest
            .load_fixtures(&crate_root.join("fixtures"), &manifest_path)
            .unwrap();
        let expected_fixtures = [
            "pdf_tiny_memo.json",
            "pdf_tables.json",
            "pdf_embedded.json",
            "pdf_google_docs.json",
            "pdf/681693.json",
            "pdf/ft_ACN_2009_page_102_t0.json",
            "pdf/pb_fqr-retail-blackrock-global-allocation-fund-inc_page4.json",
            "pdf/pb_sample_page_16_page1.json",
        ]
        .map(PathBuf::from);

        assert_eq!(manifest.name, "native-pdf-fast-b8-v1");
        assert_eq!(manifest.batch_size, BATCH_SIZE);
        assert_eq!(manifest.fixtures, expected_fixtures);
        assert_eq!(fixtures.len(), BATCH_SIZE);
        let corpus = manifest
            .load_corpus(&crate_root.join("fixtures"), &manifest_path)
            .unwrap();
        let corpus_fixtures: Vec<_> = corpus
            .iter()
            .map(|doc| doc.fixture_path.strip_prefix(crate_root.join("fixtures")).unwrap())
            .collect();
        let expected_corpus_fixtures: Vec<_> = expected_fixtures.iter().map(PathBuf::as_path).collect();
        assert_eq!(corpus_fixtures, expected_corpus_fixtures);
        for (fixture_path, fixture) in fixtures.fixtures() {
            assert_eq!(fixture.file_type, "pdf");
            assert!(!fixture.requires_ocr());
            let fixture_dir = fixture_path.parent().unwrap();
            let document_path = fixture.resolve_document_path(fixture_dir);
            assert!(document_path.is_file());
            assert_eq!(fixture.file_size, std::fs::metadata(document_path).unwrap().len());
            for ground_truth_path in [
                fixture.resolve_ground_truth_path(fixture_dir).unwrap(),
                fixture.resolve_ground_truth_markdown_path(fixture_dir).unwrap(),
            ] {
                assert!(ground_truth_path.is_file());
                assert!(std::fs::metadata(ground_truth_path).unwrap().len() > 0);
            }
        }
    }

    #[test]
    fn image_cohorts_are_fully_eligible_for_sceptre() {
        use crate::adapter::OcrLanguagePolicy;

        let crate_root = Path::new(env!("CARGO_MANIFEST_DIR"));
        for cohort in ["ocr-images-structured", "ocr-images-fast"] {
            let manifest_path = crate_root.join(format!("cohorts/{cohort}.json"));
            let manifest = CohortManifest::from_file(&manifest_path).unwrap();
            let fixtures = manifest
                .load_fixtures(&crate_root.join("fixtures"), &manifest_path)
                .unwrap();

            assert!(manifest.ocr_enabled);
            assert_eq!(fixtures.len(), manifest.batch_size);
            assert!(fixtures.fixtures().iter().all(|(_, fixture)| {
                fixture.requires_ocr() && OcrLanguagePolicy::SceptrePerDocument.supports(fixture.ocr_language())
            }));
        }
    }

    #[test]
    fn ocr_pdf_fast_b4_is_loadable_with_quality_ground_truth() {
        const BATCH_SIZE: usize = 4;

        let crate_root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let manifest_path = crate_root.join("cohorts/ocr-pdf-fast-b4.json");
        let manifest = CohortManifest::from_file(&manifest_path).unwrap();
        let fixtures = manifest
            .load_fixtures(&crate_root.join("fixtures"), &manifest_path)
            .unwrap();
        let expected_fixtures = [
            "pdf_non_searchable.json",
            "pdf_ocr_test.json",
            "pdf_scanned_ocr.json",
            "pdf_image_only_german.json",
        ]
        .map(PathBuf::from);

        assert_eq!(manifest.name, "ocr-pdf-fast-b4-v1");
        assert_eq!(manifest.batch_size, BATCH_SIZE);
        assert_eq!(manifest.fixtures, expected_fixtures);
        assert_eq!(fixtures.len(), BATCH_SIZE);
        for (fixture_path, fixture) in fixtures.fixtures() {
            assert_eq!(fixture.file_type, "pdf");
            assert!(fixture.requires_ocr());
            let fixture_dir = fixture_path.parent().unwrap();
            let document_path = fixture.resolve_document_path(fixture_dir);
            assert!(document_path.is_file());
            assert_eq!(fixture.file_size, std::fs::metadata(document_path).unwrap().len());
            for ground_truth_path in [
                fixture.resolve_ground_truth_path(fixture_dir).unwrap(),
                fixture.resolve_ground_truth_markdown_path(fixture_dir).unwrap(),
            ] {
                assert!(ground_truth_path.is_file());
                assert!(std::fs::metadata(ground_truth_path).unwrap().len() > 0);
            }
        }
    }

    #[test]
    fn family_cohorts_load_with_resolvable_documents() {
        let crate_root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let fixtures_root = crate_root.join("fixtures");
        // (manifest file stem, manifest `name`, expected fixture count, ocr_enabled).
        let cohorts = [
            ("native-office-fast", "native-office-fast-v1", 16, false),
            ("native-markup-fast", "native-markup-fast-v1", 16, false),
            ("native-ebook-fast", "native-ebook-fast-v1", 6, false),
            ("native-email-fast", "native-email-fast-v1", 6, false),
            ("native-data-fast", "native-data-fast-v1", 12, false),
            ("ocr-images-fast", "ocr-images-fast-v1", 14, true),
            ("paddle-layout-quality-b4", "paddle-layout-quality-b4-v1", 12, true),
        ];
        for (stem, name, count, ocr_enabled) in cohorts {
            let manifest_path = crate_root.join(format!("cohorts/{stem}.json"));
            let manifest = CohortManifest::from_file(&manifest_path).unwrap();
            let fixtures = manifest.load_fixtures(&fixtures_root, &manifest_path).unwrap();

            assert_eq!(manifest.name, name, "{stem} name");
            assert_eq!(manifest.ocr_enabled, ocr_enabled, "{stem} ocr_enabled");
            assert_eq!(fixtures.len(), count, "{stem} fixture count");
            assert!(count.is_multiple_of(manifest.batch_size), "{stem} batch divisibility");

            let mut document_names = HashSet::new();
            for (fixture_path, fixture) in fixtures.fixtures() {
                let fixture_dir = fixture_path.parent().unwrap();
                let document_path = fixture.resolve_document_path(fixture_dir);
                assert!(
                    document_path.is_file(),
                    "{stem}: missing document {}",
                    document_path.display()
                );
                assert_eq!(
                    fixture.file_size,
                    std::fs::metadata(&document_path).unwrap().len(),
                    "{stem}: document size mismatch for {}",
                    document_path.display()
                );
                let basename = document_path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap()
                    .to_string();
                assert!(document_names.insert(basename), "{stem}: duplicate document basename");

                let ground_truth = fixture.resolve_ground_truth_path(fixture_dir).unwrap();
                assert!(
                    ground_truth.is_file(),
                    "{stem}: missing ground truth {}",
                    ground_truth.display()
                );
                assert!(
                    std::fs::metadata(&ground_truth).unwrap().len() > 0,
                    "{stem}: empty ground truth"
                );
            }
        }
    }

    #[test]
    fn rejects_partial_duplicate_and_parent_paths() {
        let temp = tempfile::tempdir().unwrap();
        for fixtures in [
            serde_json::json!(["a.json"]),
            serde_json::json!(["a.json", "a.json"]),
            serde_json::json!(["../a.json", "b.json"]),
        ] {
            let path = write_manifest(
                temp.path(),
                serde_json::json!({
                    "schema_version": 1,
                    "name": "invalid",
                    "batch_size": 2,
                    "fixtures": fixtures
                }),
            );
            assert!(CohortManifest::from_file(&path).is_err());
        }
    }
}
