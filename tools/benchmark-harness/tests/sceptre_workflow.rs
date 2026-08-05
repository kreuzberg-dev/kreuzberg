use std::path::Path;

fn benchmark_workflow() -> String {
    let harness_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(harness_root.join("../../.github/workflows/benchmarks.yaml")).unwrap()
}

#[test]
fn workflow_prewarm_selects_explicit_sceptre_engines() {
    let workflow = benchmark_workflow();
    assert!(workflow.contains("Pre-warm Sceptre ORT models"));
    assert!(workflow.contains("--features all,xberg/sceptre-ocr-tract"));
    assert!(workflow.contains(r#"--ocr-backend-options '{"model":{"backend":"ort"}}'"#));
    assert!(workflow.contains("Pre-warm Sceptre tract models"));
    assert!(workflow.contains(r#"--ocr-backend-options '{"model":{"backend":"tract"}}'"#));
}

#[test]
fn tract_workflow_is_bounded_to_structured_markdown_single_and_batch() {
    let workflow = benchmark_workflow();
    let tract_job = workflow.split("  sceptre-tract-diagnostic:").nth(1).unwrap();
    let tract_job = tract_job.split("\n  bench-docling:").next().unwrap();

    assert!(tract_job.contains("mode: [single-file, batch]"));
    assert!(tract_job.contains("OUTPUT_FORMAT: markdown"));
    assert!(tract_job.contains("cohorts/ocr-images-structured.json"));
    assert!(!tract_job.contains("output_format: [markdown, plaintext]"));
}
