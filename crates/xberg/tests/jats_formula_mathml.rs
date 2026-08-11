//! JATS formula extraction: `disp-formula` / `inline-formula` content must
//! surface as LaTeX, not as flattened MathML text.
//!
//! Preference order inside a formula element: a `tex-math` child verbatim,
//! then the `mml:math` subtree through the shared MathML converter, then the
//! flattened text content.

#[cfg(all(test, feature = "xml"))]
mod jats_formula_mathml {
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

    async fn extract(jats: &str) -> xberg::ExtractedDocument {
        let extractor = JatsExtractor;
        let config = plain_config();
        let input = ExtractInput::from_bytes(jats.as_bytes().to_vec(), "application/x-jats+xml", None);
        extractor
            .extract(input, &config)
            .await
            .expect("JATS extraction should succeed")
    }

    /// A namespaced `mml:math` subtree converts to LaTeX and reaches the
    /// public `formulas` list.
    #[tokio::test]
    async fn disp_formula_mathml_converts_to_latex() {
        let jats = r#"<?xml version="1.0" encoding="UTF-8"?>
<article xmlns:mml="http://www.w3.org/1998/Math/MathML">
  <front>
    <article-meta><article-title>Math Test</article-title></article-meta>
  </front>
  <body>
    <disp-formula id="e1">
      <mml:math><mml:mfrac><mml:mi>a</mml:mi><mml:mi>b</mml:mi></mml:mfrac></mml:math>
    </disp-formula>
  </body>
</article>"#;

        let extraction = extract(jats).await;
        assert_eq!(
            extraction.formulas.len(),
            1,
            "expected one formula; got: {:?}",
            extraction.formulas
        );
        assert_eq!(extraction.formulas[0].latex, "\\frac{a}{b}");
        assert_eq!(extraction.formulas[0].page, None);
        assert_eq!(extraction.formulas[0].bbox, None);
    }

    /// When `alternatives` carries both `tex-math` and `mml:math`, the
    /// original TeX wins, with its delimiters stripped.
    #[tokio::test]
    async fn tex_math_is_preferred_over_mathml() {
        let jats = r#"<?xml version="1.0" encoding="UTF-8"?>
<article xmlns:mml="http://www.w3.org/1998/Math/MathML">
  <front>
    <article-meta><article-title>Math Test</article-title></article-meta>
  </front>
  <body>
    <disp-formula id="e1">
      <alternatives>
        <tex-math><![CDATA[\[ E = mc^2 \]]]></tex-math>
        <mml:math><mml:mi>E</mml:mi></mml:math>
      </alternatives>
    </disp-formula>
  </body>
</article>"#;

        let extraction = extract(jats).await;
        assert_eq!(extraction.formulas.len(), 1);
        assert_eq!(extraction.formulas[0].latex, "E = mc^2");
    }

    /// An equation label survives the MathML conversion as a LaTeX `\tag`.
    #[tokio::test]
    async fn equation_label_becomes_a_tag() {
        let jats = r#"<?xml version="1.0" encoding="UTF-8"?>
<article xmlns:mml="http://www.w3.org/1998/Math/MathML">
  <front>
    <article-meta><article-title>Math Test</article-title></article-meta>
  </front>
  <body>
    <disp-formula id="e1">
      <label>1.1</label>
      <mml:math><mml:mfrac><mml:mi>a</mml:mi><mml:mi>b</mml:mi></mml:mfrac></mml:math>
    </disp-formula>
  </body>
</article>"#;

        let extraction = extract(jats).await;
        assert_eq!(extraction.formulas.len(), 1);
        assert_eq!(extraction.formulas[0].latex, "\\frac{a}{b} \\tag{1.1}");
    }

    /// A formula with plain text content still extracts through the fallback.
    #[tokio::test]
    async fn plain_text_formula_falls_back_to_text() {
        let jats = r#"<?xml version="1.0" encoding="UTF-8"?>
<article>
  <front>
    <article-meta><article-title>Math Test</article-title></article-meta>
  </front>
  <body>
    <disp-formula id="e1">a + b = c</disp-formula>
  </body>
</article>"#;

        let extraction = extract(jats).await;
        assert_eq!(extraction.formulas.len(), 1);
        assert_eq!(extraction.formulas[0].latex, "a + b = c");
    }
}
