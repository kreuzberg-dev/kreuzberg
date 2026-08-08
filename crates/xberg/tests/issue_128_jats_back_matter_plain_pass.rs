//! Regression tests for issue #128: JATS back matter and block content.
//!
//! The plain structural pass (`build_jats_internal_document`) previously only
//! emitted headings/paragraphs for a narrow set of `<back>` children. This
//! verifies that acknowledgments, appendices, glossary terms, footnotes, and
//! general `<sec>`/`<p>`/`<list>` block content inside `<back>` are actually
//! walked and emitted, not silently dropped.

#[cfg(all(test, feature = "xml"))]
mod issue_128_jats_back_matter_plain_pass {
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

    /// `<ack>`, `<app-group>/<app>`, `<glossary>` (including bare `<term>` text),
    /// `<fn-group>`, and a generic `<sec>` with `<p>`/`<list>` inside `<back>` must
    /// all be walked and their text emitted into the document content.
    #[tokio::test]
    async fn should_emit_all_back_matter_sections_and_block_content() {
        let jats = r#"<?xml version="1.0" encoding="UTF-8"?>
<article>
  <front>
    <article-meta>
      <article-title>Back Matter Test</article-title>
    </article-meta>
  </front>
  <body>
    <p>Main body paragraph.</p>
  </body>
  <back>
    <ack>
      <title>Acknowledgments</title>
      <p>We thank the reviewers for their helpful comments.</p>
    </ack>
    <app-group>
      <app id="app1">
        <title>Appendix A</title>
        <p>Supplementary derivation details go here.</p>
      </app>
    </app-group>
    <glossary>
      <title>Glossary</title>
      <def-list>
        <def-item>
          <term>API</term>
          <def>
            <p>Application Programming Interface</p>
          </def>
        </def-item>
      </def-list>
    </glossary>
    <fn-group>
      <fn id="fn1">
        <label>1</label>
        <p>This is a footnote explaining the methodology.</p>
      </fn>
    </fn-group>
    <sec>
      <title>Extra Notes</title>
      <p>Extra plain paragraph in a back-matter section.</p>
      <list list-type="bullet">
        <list-item><p>First bullet item.</p></list-item>
        <list-item><p>Second bullet item.</p></list-item>
      </list>
    </sec>
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

        assert!(content.contains("Main body paragraph."), "body content missing");

        assert!(content.contains("Acknowledgments"), "ack title missing: {content}");
        assert!(
            content.contains("We thank the reviewers for their helpful comments."),
            "ack paragraph missing: {content}"
        );

        assert!(content.contains("Appendix A"), "appendix title missing: {content}");
        assert!(
            content.contains("Supplementary derivation details go here."),
            "appendix paragraph missing: {content}"
        );

        assert!(content.contains("Glossary"), "glossary title missing: {content}");
        assert!(content.contains("API"), "glossary term text missing: {content}");
        assert!(
            content.contains("Application Programming Interface"),
            "glossary definition missing: {content}"
        );

        assert!(
            content.contains("This is a footnote explaining the methodology."),
            "footnote text missing: {content}"
        );

        assert!(
            content.contains("Extra Notes"),
            "back-matter sec title missing: {content}"
        );
        assert!(
            content.contains("Extra plain paragraph in a back-matter section."),
            "back-matter sec paragraph missing: {content}"
        );
        assert!(content.contains("First bullet item."), "list item 1 missing: {content}");
        assert!(
            content.contains("Second bullet item."),
            "list item 2 missing: {content}"
        );
    }

    /// A minimal `<back>` containing only `<ack>` must not regress: the
    /// acknowledgment text is still present alongside the body content.
    #[tokio::test]
    async fn should_emit_acknowledgments_alongside_body_content() {
        let jats = r#"<?xml version="1.0" encoding="UTF-8"?>
<article>
  <front>
    <article-meta>
      <article-title>Ack Only Test</article-title>
    </article-meta>
  </front>
  <body>
    <p>Body text here.</p>
  </body>
  <back>
    <ack>
      <p>Funding was provided by the National Test Foundation.</p>
    </ack>
  </back>
</article>"#;

        let extractor = JatsExtractor;
        let config = plain_config();
        let input = ExtractInput::from_bytes(jats.as_bytes().to_vec(), "application/x-jats+xml", None);
        let extraction = extractor
            .extract(input, &config)
            .await
            .expect("JATS extraction should succeed");

        assert!(extraction.content.contains("Body text here."));
        assert!(
            extraction
                .content
                .contains("Funding was provided by the National Test Foundation.")
        );
    }
}
