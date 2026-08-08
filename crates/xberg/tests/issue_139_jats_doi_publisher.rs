//! Regression tests for issue #139: JATS citation DOI and publisher fields.
//!
//! `<element-citation>`/`<mixed-citation>` entries inside `<ref-list>`/`<ref>` were
//! missing DOI and publisher information. `<pub-id pub-id-type="doi">` must be
//! extracted as the citation's DOI, and `<publisher-name>`/`<publisher-loc>` must be
//! extracted as publisher fields, ending up in the formatted citation text the
//! extractor already builds for each reference-list entry.

#[cfg(all(test, feature = "xml"))]
mod issue_139_jats_doi_publisher {
    use xberg::ExtractInput;
    use xberg::core::config::ExtractionConfig;
    use xberg::extractors::JatsExtractor;
    use xberg::plugins::DocumentExtractor;

    fn plain_config() -> ExtractionConfig {
        ExtractionConfig {
            output_format: xberg::OutputFormat::Plain,
            include_document_structure: false,
            ..ExtractionConfig::default()
        }
    }

    /// A journal-article `<element-citation>` with a `<pub-id pub-id-type="doi">`
    /// must surface the exact DOI string in the rendered citation text.
    #[tokio::test]
    async fn should_extract_exact_doi_from_element_citation() {
        let jats = r#"<?xml version="1.0" encoding="UTF-8"?>
<article>
  <front>
    <article-meta>
      <article-title>Citation DOI Test</article-title>
    </article-meta>
  </front>
  <body>
    <p>See prior work <xref ref-type="bibr" rid="ref1">1</xref>.</p>
  </body>
  <back>
    <ref-list>
      <ref id="ref1">
        <element-citation publication-type="journal">
          <person-group person-group-type="author">
            <name>
              <surname>Author</surname>
              <given-names>First</given-names>
            </name>
          </person-group>
          <article-title>Original Research</article-title>
          <source>Journal Name</source>
          <year>2020</year>
          <pub-id pub-id-type="doi">10.1000/xyz123</pub-id>
        </element-citation>
      </ref>
    </ref-list>
  </back>
</article>"#;

        let extractor = JatsExtractor;
        let config = plain_config();
        let input = ExtractInput::from_bytes(jats.as_bytes().to_vec(), "application/x-jats+xml", None);
        let extraction = extractor
            .extract(input, &config)
            .await
            .expect("JATS extraction should succeed");

        assert!(
            extraction.content.contains("DOI: 10.1000/xyz123"),
            "expected exact DOI text in citation, got: {}",
            extraction.content
        );
        assert!(extraction.content.contains("Original Research"));
        assert!(extraction.content.contains("Journal Name"));
    }

    /// A book-type `<element-citation>` with `<publisher-name>`/`<publisher-loc>` must
    /// surface the exact publisher location and name in the rendered citation text.
    #[tokio::test]
    async fn should_extract_exact_publisher_name_and_location_from_element_citation() {
        let jats = r#"<?xml version="1.0" encoding="UTF-8"?>
<article>
  <front>
    <article-meta>
      <article-title>Citation Publisher Test</article-title>
    </article-meta>
  </front>
  <body>
    <p>See the reference book <xref ref-type="bibr" rid="ref1">1</xref>.</p>
  </body>
  <back>
    <ref-list>
      <ref id="ref1">
        <element-citation publication-type="book">
          <person-group person-group-type="author">
            <name>
              <surname>Writer</surname>
              <given-names>Bookish</given-names>
            </name>
          </person-group>
          <source>Comprehensive Guide to Testing</source>
          <year>2019</year>
          <publisher-loc>Springfield</publisher-loc>
          <publisher-name>Test Press</publisher-name>
        </element-citation>
      </ref>
    </ref-list>
  </back>
</article>"#;

        let extractor = JatsExtractor;
        let config = plain_config();
        let input = ExtractInput::from_bytes(jats.as_bytes().to_vec(), "application/x-jats+xml", None);
        let extraction = extractor
            .extract(input, &config)
            .await
            .expect("JATS extraction should succeed");

        assert!(
            extraction.content.contains("Springfield: Test Press."),
            "expected exact publisher location and name in citation, got: {}",
            extraction.content
        );
        assert!(extraction.content.contains("Comprehensive Guide to Testing"));
    }

    /// A citation with both DOI and publisher fields must surface both exactly,
    /// together with the existing author/title/year/source fields.
    #[tokio::test]
    async fn should_extract_doi_and_publisher_together_alongside_existing_fields() {
        let jats = r#"<?xml version="1.0" encoding="UTF-8"?>
<article>
  <front>
    <article-meta>
      <article-title>Full Citation Test</article-title>
    </article-meta>
  </front>
  <body>
    <p>Reference here <xref ref-type="bibr" rid="ref1">1</xref>.</p>
  </body>
  <back>
    <ref-list>
      <ref id="ref1">
        <element-citation publication-type="journal">
          <person-group person-group-type="author">
            <name>
              <surname>Smith</surname>
              <given-names>Jane</given-names>
            </name>
          </person-group>
          <article-title>A Complete Study</article-title>
          <source>International Journal of Examples</source>
          <year>2022</year>
          <volume>7</volume>
          <fpage>10</fpage>
          <lpage>20</lpage>
          <publisher-loc>Metropolis</publisher-loc>
          <publisher-name>Example Publishing</publisher-name>
          <pub-id pub-id-type="doi">10.9999/full-test</pub-id>
        </element-citation>
      </ref>
    </ref-list>
  </back>
</article>"#;

        let extractor = JatsExtractor;
        let config = plain_config();
        let input = ExtractInput::from_bytes(jats.as_bytes().to_vec(), "application/x-jats+xml", None);
        let extraction = extractor
            .extract(input, &config)
            .await
            .expect("JATS extraction should succeed");

        let content = &extraction.content;
        assert!(
            content.contains("Smith Jane"),
            "expected exact author name, got: {content}"
        );
        assert!(content.contains("A Complete Study"));
        assert!(content.contains("International Journal of Examples"));
        assert!(
            content.contains("2022;7:10-20"),
            "expected exact year/volume/pages, got: {content}"
        );
        assert!(
            content.contains("Metropolis: Example Publishing."),
            "expected exact publisher location and name, got: {content}"
        );
        assert!(
            content.contains("DOI: 10.9999/full-test."),
            "expected exact DOI text, got: {content}"
        );
    }
}
