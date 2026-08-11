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

    /// A label also survives when the TeX alternative wins.
    #[tokio::test]
    async fn equation_label_tags_the_tex_math_path() {
        let jats = r#"<?xml version="1.0" encoding="UTF-8"?>
<article>
  <front>
    <article-meta><article-title>Math Test</article-title></article-meta>
  </front>
  <body>
    <disp-formula id="e1">
      <label>2</label>
      <tex-math>$$a^2$$</tex-math>
    </disp-formula>
  </body>
</article>"#;

        let extraction = extract(jats).await;
        assert_eq!(extraction.formulas.len(), 1);
        assert_eq!(extraction.formulas[0].latex, "a^2 \\tag{2}");
    }

    /// A label prefixes the fallback text when no math notation is present.
    #[tokio::test]
    async fn equation_label_prefixes_the_text_fallback() {
        let jats = r#"<?xml version="1.0" encoding="UTF-8"?>
<article>
  <front>
    <article-meta><article-title>Math Test</article-title></article-meta>
  </front>
  <body>
    <disp-formula id="e1"><label>3</label>a + b = c</disp-formula>
  </body>
</article>"#;

        let extraction = extract(jats).await;
        assert_eq!(extraction.formulas.len(), 1);
        assert_eq!(extraction.formulas[0].latex, "3 a + b = c");
    }

    /// Every `mml:math` sibling in one formula element is converted; none is
    /// silently dropped.
    #[tokio::test]
    async fn multiple_math_siblings_are_all_converted() {
        let jats = r#"<?xml version="1.0" encoding="UTF-8"?>
<article xmlns:mml="http://www.w3.org/1998/Math/MathML">
  <front>
    <article-meta><article-title>Math Test</article-title></article-meta>
  </front>
  <body>
    <disp-formula id="e1">
      <mml:math><mml:mi>a</mml:mi></mml:math>
      <mml:math><mml:mi>b</mml:mi></mml:math>
    </disp-formula>
  </body>
</article>"#;

        let extraction = extract(jats).await;
        assert_eq!(extraction.formulas.len(), 1);
        assert_eq!(extraction.formulas[0].latex, "a \\\\ b");
    }

    /// CDATA text inside a captured MathML subtree reaches the converter.
    #[tokio::test]
    async fn cdata_inside_mathml_is_preserved() {
        let jats = r#"<?xml version="1.0" encoding="UTF-8"?>
<article xmlns:mml="http://www.w3.org/1998/Math/MathML">
  <front>
    <article-meta><article-title>Math Test</article-title></article-meta>
  </front>
  <body>
    <disp-formula id="e1">
      <mml:math><mml:mtext><![CDATA[x < y]]></mml:mtext></mml:math>
    </disp-formula>
  </body>
</article>"#;

        let extraction = extract(jats).await;
        assert_eq!(extraction.formulas.len(), 1);
        assert!(
            extraction.formulas[0].latex.contains("x < y") || extraction.formulas[0].latex.contains("x &lt; y"),
            "CDATA content must survive; got: {}",
            extraction.formulas[0].latex
        );
    }

    /// A single-quoted attribute holding a double quote must not break the
    /// captured subtree.
    #[tokio::test]
    async fn single_quoted_attribute_with_double_quote_is_survivable() {
        let jats = r#"<?xml version="1.0" encoding="UTF-8"?>
<article xmlns:mml="http://www.w3.org/1998/Math/MathML">
  <front>
    <article-meta><article-title>Math Test</article-title></article-meta>
  </front>
  <body>
    <disp-formula id="e1">
      <mml:math alttext='say "hi"'><mml:mfrac><mml:mi>a</mml:mi><mml:mi>b</mml:mi></mml:mfrac></mml:math>
    </disp-formula>
  </body>
</article>"#;

        let extraction = extract(jats).await;
        assert_eq!(extraction.formulas.len(), 1);
        assert_eq!(extraction.formulas[0].latex, "\\frac{a}{b}");
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
