//! Regression tests for issue #126: JATS `<fig>` figure captions and graphics.
//!
//! `<fig>` elements contain a `<label>`, a `<caption>` (with nested `<title>`/`<p>`),
//! and a `<graphic xlink:href="...">`. The extractor must associate the full caption
//! text (not just the label) and the graphic's href with the figure, instead of
//! dropping them or flattening them into unrelated surrounding text.

#[cfg(all(test, feature = "xml"))]
mod issue_126_jats_figure_captions_graphics {
    use xberg::ExtractInput;
    use xberg::core::config::ExtractionConfig;
    use xberg::extractors::JatsExtractor;
    use xberg::plugins::DocumentExtractor;
    use xberg::types::uri::UriKind;

    fn plain_config() -> ExtractionConfig {
        ExtractionConfig {
            output_format: xberg::OutputFormat::Plain,
            include_document_structure: false,
            ..ExtractionConfig::default()
        }
    }

    /// A `<fig>` with a label, a caption (title + paragraph), and a graphic href must
    /// surface the exact caption text and the exact graphic href, not just the label,
    /// and must not drop the graphic reference.
    #[tokio::test]
    async fn should_extract_exact_caption_text_and_graphic_href_from_fig() {
        let jats = r#"<?xml version="1.0" encoding="UTF-8"?>
<article xmlns:xlink="http://www.w3.org/1999/xlink">
  <front>
    <article-meta>
      <article-title>Figure Extraction Test</article-title>
    </article-meta>
  </front>
  <body>
    <sec id="s1">
      <title>Results</title>
      <p>Preceding paragraph text.</p>
      <fig id="fig1">
        <label>Figure 1</label>
        <caption>
          <title>Overview of the experimental setup</title>
          <p>Panel A shows the control group; panel B shows the treatment group.</p>
        </caption>
        <graphic xlink:href="images/fig1.png"/>
      </fig>
      <p>Following paragraph text.</p>
    </sec>
  </body>
</article>"#;

        let extractor = JatsExtractor;
        let config = plain_config();
        let input = ExtractInput::from_bytes(jats.as_bytes().to_vec(), "application/x-jats+xml", None);
        let extraction = extractor
            .extract(input, &config)
            .await
            .expect("JATS extraction should succeed");

        assert!(
            extraction.content.contains("Overview of the experimental setup"),
            "expected caption title text in content, got: {}",
            extraction.content
        );
        assert!(
            extraction
                .content
                .contains("Panel A shows the control group; panel B shows the treatment group."),
            "expected full caption paragraph text in content, got: {}",
            extraction.content
        );
        assert!(
            extraction.content.contains("images/fig1.png"),
            "expected graphic href in content, got: {}",
            extraction.content
        );

        // The caption/graphic must not be flattened into the surrounding prose:
        // the preceding and following paragraphs remain distinct, separate text.
        assert!(extraction.content.contains("Preceding paragraph text."));
        assert!(extraction.content.contains("Following paragraph text."));

        let uris = extraction.uris.expect("expected extracted URIs for the figure graphic");
        let image_uri = uris
            .iter()
            .find(|u| u.url == "images/fig1.png")
            .expect("expected an ExtractedUri for the figure graphic href");
        assert_eq!(image_uri.kind, UriKind::Image);
        assert_eq!(
            image_uri.label.as_deref(),
            Some(
                "Figure 1: Overview of the experimental setup Panel A shows the control group; panel B shows the treatment group."
            )
        );
    }

    /// Multiple figures in the same document must each keep their own caption and
    /// graphic href distinct — captions/hrefs must not bleed between figures.
    #[tokio::test]
    async fn should_keep_captions_and_graphics_distinct_across_multiple_figures() {
        let jats = r#"<?xml version="1.0" encoding="UTF-8"?>
<article xmlns:xlink="http://www.w3.org/1999/xlink">
  <front>
    <article-meta>
      <article-title>Multi-Figure Test</article-title>
    </article-meta>
  </front>
  <body>
    <fig id="fig1">
      <label>Figure 1</label>
      <caption><p>First figure caption.</p></caption>
      <graphic xlink:href="images/first.png"/>
    </fig>
    <fig id="fig2">
      <label>Figure 2</label>
      <caption><p>Second figure caption.</p></caption>
      <graphic xlink:href="images/second.png"/>
    </fig>
  </body>
</article>"#;

        let extractor = JatsExtractor;
        let config = plain_config();
        let input = ExtractInput::from_bytes(jats.as_bytes().to_vec(), "application/x-jats+xml", None);
        let extraction = extractor
            .extract(input, &config)
            .await
            .expect("JATS extraction should succeed");

        assert!(extraction.content.contains("First figure caption."));
        assert!(extraction.content.contains("Second figure caption."));
        assert!(extraction.content.contains("images/first.png"));
        assert!(extraction.content.contains("images/second.png"));

        let uris = extraction
            .uris
            .expect("expected extracted URIs for both figure graphics");
        assert_eq!(
            uris.iter().filter(|u| u.kind == UriKind::Image).count(),
            2,
            "expected exactly two image URIs, got: {:?}",
            uris
        );

        let first = uris
            .iter()
            .find(|u| u.url == "images/first.png")
            .expect("expected image URI for first figure");
        assert_eq!(first.label.as_deref(), Some("Figure 1: First figure caption."));

        let second = uris
            .iter()
            .find(|u| u.url == "images/second.png")
            .expect("expected image URI for second figure");
        assert_eq!(second.label.as_deref(), Some("Figure 2: Second figure caption."));
    }

    /// A `<fig>` with only a graphic and no caption must still surface the graphic href.
    #[tokio::test]
    async fn should_extract_graphic_href_when_fig_has_no_caption() {
        let jats = r#"<?xml version="1.0" encoding="UTF-8"?>
<article xmlns:xlink="http://www.w3.org/1999/xlink">
  <front>
    <article-meta>
      <article-title>Captionless Figure Test</article-title>
    </article-meta>
  </front>
  <body>
    <fig id="fig1">
      <graphic xlink:href="images/uncaptioned.png"/>
    </fig>
  </body>
</article>"#;

        let extractor = JatsExtractor;
        let config = plain_config();
        let input = ExtractInput::from_bytes(jats.as_bytes().to_vec(), "application/x-jats+xml", None);
        let extraction = extractor
            .extract(input, &config)
            .await
            .expect("JATS extraction should succeed");

        let uris = extraction.uris.expect("expected extracted URIs for the figure graphic");
        let image_uri = uris
            .iter()
            .find(|u| u.url == "images/uncaptioned.png")
            .expect("expected an ExtractedUri for the uncaptioned figure graphic");
        assert_eq!(image_uri.kind, UriKind::Image);
        assert_eq!(image_uri.label, None);
    }
}
