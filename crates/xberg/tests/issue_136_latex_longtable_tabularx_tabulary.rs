//! Regression tests for issue #136: the LaTeX extractor only recognized the
//! base `tabular` environment and silently dropped `longtable`, `tabularx`,
//! and `tabulary` tables (they were treated as generic unknown environments
//! and their `&`/`\\` grid was flattened into a run of plain-text lines).

#![cfg(feature = "office")]

use xberg::ExtractInput;
use xberg::OutputFormat;
use xberg::core::config::ExtractionConfig;
use xberg::extractors::latex::LatexExtractor;
use xberg::plugins::DocumentExtractor;

async fn extract_markdown(latex: &str) -> String {
    let extractor = LatexExtractor;
    let config = ExtractionConfig {
        output_format: OutputFormat::Markdown,
        include_document_structure: false,
        ..ExtractionConfig::default()
    };
    let input = ExtractInput::from_bytes(latex.as_bytes().to_vec(), "text/x-tex", None);
    let result = extractor
        .extract(input, &config)
        .await
        .expect("LaTeX extraction should not fail");
    // `result.content` is always the plain-text rendering regardless of
    // `output_format` (see `derive_extraction_result`); the GFM markdown
    // pipe-table text requested via `OutputFormat::Markdown` lives in
    // `formatted_content`.
    result
        .formatted_content
        .expect("markdown output_format should populate formatted_content")
}

#[tokio::test]
async fn should_render_longtable_rows_as_markdown_table() {
    let latex = r#"\documentclass{article}
\begin{document}
\begin{longtable}{|l|l|}
\hline
Name & Age \\
\hline
\endhead
Alice & 30 \\
Bob & 25 \\
\hline
\end{longtable}
\end{document}"#;

    let content = extract_markdown(latex).await;
    let expected = "| Name | Age |\n| --- | --- |\n| Alice | 30 |\n| Bob | 25 |\n";
    assert!(
        content.contains(expected),
        "expected longtable to render as a markdown table.\nExpected substring:\n{}\nActual content:\n{}",
        expected,
        content
    );
}

#[tokio::test]
async fn should_render_tabularx_rows_as_markdown_table() {
    let latex = r#"\documentclass{article}
\usepackage{tabularx}
\begin{document}
\begin{tabularx}{\textwidth}{|l|X|}
\hline
Key & Description \\
\hline
Height & Tall building \\
\hline
\end{tabularx}
\end{document}"#;

    let content = extract_markdown(latex).await;
    let expected = "| Key | Description |\n| --- | --- |\n| Height | Tall building |\n";
    assert!(
        content.contains(expected),
        "expected tabularx to render as a markdown table.\nExpected substring:\n{}\nActual content:\n{}",
        expected,
        content
    );
}

#[tokio::test]
async fn should_render_tabulary_rows_as_markdown_table() {
    let latex = r#"\documentclass{article}
\usepackage{tabulary}
\begin{document}
\begin{tabulary}{\textwidth}{|L|C|R|}
\hline
Left & Center & Right \\
\hline
a & b & c \\
\hline
\end{tabulary}
\end{document}"#;

    let content = extract_markdown(latex).await;
    let expected = "| Left | Center | Right |\n| --- | --- | --- |\n| a | b | c |\n";
    assert!(
        content.contains(expected),
        "expected tabulary to render as a markdown table.\nExpected substring:\n{}\nActual content:\n{}",
        expected,
        content
    );
}

#[tokio::test]
async fn should_treat_longtable_endhead_and_endfoot_as_structural_not_cell_content() {
    let latex = r#"\documentclass{article}
\begin{document}
\begin{longtable}{|l|l|}
\caption{A long table} \label{tab:long} \\
\hline
Name & Age \\
\hline
\endfirsthead
\hline
Name & Age \\
\hline
\endhead
\hline
\endfoot
Alice & 30 \\
\hline
\endlastfoot
\end{longtable}
\end{document}"#;

    let content = extract_markdown(latex).await;

    assert!(
        !content.contains("endhead") && !content.contains("endfoot") && !content.contains("endfirsthead"),
        "longtable page-break markers must not leak into cell content:\n{}",
        content
    );
    assert!(
        content.contains("| Name | Age |"),
        "expected header row to be parsed as table cells:\n{}",
        content
    );
    assert!(
        content.contains("| Alice | 30 |"),
        "expected body row to be parsed as table cells:\n{}",
        content
    );
}
