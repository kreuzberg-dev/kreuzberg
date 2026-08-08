use std::collections::VecDeque;
use std::fs::File;
use std::io::Write;

use tempfile::tempdir;

use super::*;
#[cfg(not(target_arch = "wasm32"))]
use crate::core::config::concurrency::LayoutBatchWorkload;

#[tokio::test]
async fn extract_bytes_input_returns_envelope() {
    let config = ExtractionConfig::default();
    let output = crate::engine::Engine::new_default()
        .extract(ExtractInput::from_bytes(b"hello".to_vec(), "text/plain", None), &config)
        .await
        .unwrap();

    assert_eq!(output.results.len(), 1);
    assert_eq!(output.summary.inputs, 1);
    assert_eq!(output.summary.results, 1);
    assert_eq!(output.results[0].content.trim(), "hello");
}

#[tokio::test]
async fn extract_local_uri_returns_envelope() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("doc.txt");
    File::create(&path).unwrap().write_all(b"hello path").unwrap();

    let config = ExtractionConfig::default();
    let output = crate::engine::Engine::new_default()
        .extract(ExtractInput::from_uri(path.to_string_lossy()), &config)
        .await
        .unwrap();

    assert_eq!(output.results.len(), 1);
    assert_eq!(output.results[0].content.trim(), "hello path");
}

#[tokio::test]
async fn extract_file_uri_returns_envelope() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("doc.txt");
    File::create(&path).unwrap().write_all(b"hello file uri").unwrap();

    let config = ExtractionConfig::default();
    let output = crate::engine::Engine::new_default()
        .extract(ExtractInput::from_uri(format!("file://{}", path.display())), &config)
        .await
        .unwrap();

    assert_eq!(output.results.len(), 1);
    assert_eq!(output.results[0].content.trim(), "hello file uri");
}

#[tokio::test]
async fn extract_rejects_local_path_when_policy_disallows_it() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("doc.txt");
    File::create(&path).unwrap().write_all(b"hello local policy").unwrap();

    let mut config = ExtractionConfig::default();
    config.url.allow_local_file_inputs = false;
    let error = crate::engine::Engine::new_default()
        .extract(ExtractInput::from_uri(path.to_string_lossy()), &config)
        .await
        .unwrap_err();

    assert!(error.to_string().contains("local filesystem path inputs are disabled"));
}

#[tokio::test]
async fn extract_rejects_non_local_file_uri_host() {
    let config = ExtractionConfig::default();
    let error = crate::engine::Engine::new_default()
        .extract(ExtractInput::from_uri("file://evilhost/tmp/doc.txt"), &config)
        .await
        .unwrap_err();

    assert!(error.to_string().contains("unsupported non-local file URI host"));
}

#[tokio::test]
async fn extract_file_uri_accepts_localhost_host() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("doc.txt");
    File::create(&path)
        .unwrap()
        .write_all(b"hello localhost file uri")
        .unwrap();

    let config = ExtractionConfig::default();
    let output = crate::engine::Engine::new_default()
        .extract(
            ExtractInput::from_uri(format!("file://localhost{}", path.display())),
            &config,
        )
        .await
        .unwrap();

    assert_eq!(output.results.len(), 1);
    assert_eq!(output.results[0].content.trim(), "hello localhost file uri");
}

#[tokio::test]
async fn extract_rejects_unsupported_scheme() {
    let config = ExtractionConfig::default();
    let error = crate::engine::Engine::new_default()
        .extract(ExtractInput::from_uri("s3://bucket/file.txt"), &config)
        .await
        .unwrap_err();

    assert!(error.to_string().contains("unsupported URI scheme"));
}

#[tokio::test]
async fn extract_batch_collects_mixed_inputs() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("doc.txt");
    File::create(&path).unwrap().write_all(b"hello batch path").unwrap();

    let config = ExtractionConfig::default();
    let output = crate::engine::Engine::new_default()
        .extract_batch(
            vec![
                ExtractInput::from_bytes(b"hello batch bytes".to_vec(), "text/plain", None),
                ExtractInput::from_uri(path.to_string_lossy()),
            ],
            &config,
        )
        .await
        .unwrap();

    assert_eq!(output.results.len(), 2);
    assert_eq!(output.summary.inputs, 2);
    assert!(output.errors.is_empty());
}

#[tokio::test]
async fn extract_batch_collects_unsupported_scheme_error() {
    let config = ExtractionConfig::default();
    let output = crate::engine::Engine::new_default()
        .extract_batch(
            vec![
                ExtractInput::from_bytes(b"hello batch bytes".to_vec(), "text/plain", None),
                ExtractInput::from_uri("s3://bucket/doc.txt"),
            ],
            &config,
        )
        .await
        .unwrap();

    assert_eq!(output.results.len(), 1);
    assert_eq!(output.errors.len(), 1);
    assert_eq!(output.summary.inputs, 2);
    assert_eq!(output.summary.results, 1);
    assert_eq!(output.summary.errors, 1);
    assert_eq!(output.errors[0].index, 1);
    assert_eq!(output.errors[0].code, 1003);
    assert_eq!(output.errors[0].error_type, "unsupported_format");
}

#[tokio::test]
async fn extract_batch_applies_item_timeout() {
    let item = run_batch_item(0, "<test>".to_string(), Some(1), None, || async {
        std::future::pending::<()>().await;
        Ok(ExtractionResult::default())
    })
    .await;

    let error = item.result.unwrap_err();
    assert_eq!(error_code(&error), 1004);
    assert_eq!(error_type(&error), "timeout");
}

#[tokio::test]
async fn batch_scheduler_prioritizes_larger_inputs() {
    let directory = tempdir().unwrap();
    let large_path = directory.path().join("large.pdf");
    let mut large_file = File::create(&large_path).unwrap();
    large_file.write_all(&[0; 32]).unwrap();

    let pending = [
        (0, ExtractInput::from_bytes([0], "application/pdf", None), "small"),
        (1, ExtractInput::from_uri(large_path.to_string_lossy()), "large"),
        (2, ExtractInput::from_bytes([0; 8], "application/pdf", None), "medium"),
    ]
    .into_iter()
    .map(|(index, input, source)| (index, input, source.to_string()))
    .collect();

    let scheduled = prioritize_pending_batch_items(pending, &ExtractionConfig::default()).await;

    assert_eq!(
        scheduled.iter().map(|(index, _, _)| *index).collect::<Vec<_>>(),
        [1, 2, 0]
    );
}

#[tokio::test]
async fn batch_scheduler_respects_per_input_local_policy() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("large.pdf");
    File::create(&path).unwrap().write_all(&[0; 32]).unwrap();
    let mut denied = ExtractInput::from_uri(path.to_string_lossy());
    denied.config = Some(crate::core::config::FileExtractionConfig {
        url: Some(crate::core::config::UrlExtractionConfig {
            allow_local_file_inputs: false,
            ..Default::default()
        }),
        ..Default::default()
    });
    let pending = [
        (0, ExtractInput::from_bytes([0], "application/pdf", None), "small"),
        (1, denied, "denied"),
        (2, ExtractInput::from_bytes([0; 8], "application/pdf", None), "medium"),
    ]
    .into_iter()
    .map(|(index, input, source)| (index, input, source.to_string()))
    .collect();

    let scheduled = prioritize_pending_batch_items(pending, &ExtractionConfig::default()).await;

    assert_eq!(
        scheduled.iter().map(|(index, _, _)| *index).collect::<Vec<_>>(),
        [2, 1, 0]
    );
}

#[tokio::test]
async fn batch_scheduler_preserves_tie_order_and_remote_slots() {
    let pending = [
        (0, ExtractInput::from_bytes([0; 8], "application/pdf", None), "first"),
        (1, ExtractInput::from_uri("https://example.com/a.pdf"), "remote"),
        (2, ExtractInput::from_bytes([0; 32], "application/pdf", None), "large"),
        (3, ExtractInput::from_bytes([0; 8], "application/pdf", None), "second"),
    ]
    .into_iter()
    .map(|(index, input, source)| (index, input, source.to_string()))
    .collect();

    let scheduled = prioritize_pending_batch_items(pending, &ExtractionConfig::default()).await;

    assert_eq!(
        scheduled.iter().map(|(index, _, _)| *index).collect::<Vec<_>>(),
        [2, 1, 0, 3]
    );
}

#[test]
fn batch_scheduler_prioritizes_only_when_work_will_queue() {
    assert!(!should_prioritize_pending_batch_items(4, 1));
    assert!(!should_prioritize_pending_batch_items(4, 4));
    assert!(should_prioritize_pending_batch_items(5, 4));
}

#[test]
fn batch_scheduler_does_not_probe_disallowed_local_inputs() {
    let bare = ExtractInput::from_uri("/private/automount/doc.pdf");
    let file_uri = ExtractInput::from_uri("file:///private/automount/doc.pdf");
    let mut config = ExtractionConfig::default();
    config.url.allow_local_file_inputs = false;
    config.url.allow_file_uris = false;

    assert_eq!(local_batch_path(&bare, &config), None);
    assert_eq!(local_batch_path(&file_uri, &config), None);
}

#[tokio::test]
async fn batch_scheduler_restores_public_result_order_after_prioritizing() {
    let directory = tempdir().unwrap();
    let contents = ["small".to_string(), "large ".repeat(32), "medium medium".to_string()];
    let mut inputs = Vec::new();
    for (index, content) in contents.iter().enumerate() {
        let path = directory.path().join(format!("{index}.txt"));
        File::create(&path).unwrap().write_all(content.as_bytes()).unwrap();
        inputs.push(ExtractInput::from_uri(path.to_string_lossy()));
    }

    let config = ExtractionConfig {
        concurrency: Some(crate::core::config::ConcurrencyConfig { max_threads: Some(2) }),
        max_concurrent_extractions: Some(2),
        ..Default::default()
    };
    let output = crate::engine::Engine::new_default()
        .extract_batch(inputs, &config)
        .await
        .unwrap();

    assert!(output.errors.is_empty());
    assert_eq!(
        output
            .results
            .iter()
            .map(|document| document.content.trim())
            .collect::<Vec<_>>(),
        contents.iter().map(|content| content.trim()).collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn bounded_batch_scheduler_caps_in_flight_tasks() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    let active = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let pending = (0..8).collect::<VecDeque<_>>();
    let completed = run_bounded_batch_tasks(pending, 2, {
        let active = Arc::clone(&active);
        let peak = Arc::clone(&peak);
        move |index| {
            let active = Arc::clone(&active);
            let peak = Arc::clone(&peak);
            async move {
                let now = active.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(now, Ordering::SeqCst);
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                active.fetch_sub(1, Ordering::SeqCst);
                BatchItemResult {
                    index,
                    source: index.to_string(),
                    result: Ok(ExtractionResult::default()),
                }
            }
        }
    })
    .await
    .unwrap();

    assert_eq!(completed.len(), 8);
    assert_eq!(peak.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn bounded_batch_scheduler_preserves_completion_and_error_indices() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    let stage = Arc::new(AtomicUsize::new(0));
    let pending = (0..3).collect::<VecDeque<_>>();
    let completed = run_bounded_batch_tasks(pending, 3, {
        let stage = Arc::clone(&stage);
        move |index| {
            let stage = Arc::clone(&stage);
            async move {
                let prerequisite = match index {
                    0 => 2,
                    2 => 1,
                    _ => 0,
                };
                while stage.load(Ordering::SeqCst) < prerequisite {
                    tokio::task::yield_now().await;
                }
                stage.fetch_add(1, Ordering::SeqCst);
                let result = if index == 1 {
                    Err(XbergError::Other("indexed failure".to_string()))
                } else {
                    Ok(ExtractionResult::default())
                };
                BatchItemResult {
                    index,
                    source: index.to_string(),
                    result,
                }
            }
        }
    })
    .await
    .unwrap();

    assert_eq!(completed.iter().map(|item| item.index).collect::<Vec<_>>(), [1, 2, 0]);
    assert!(completed[0].result.is_err());
    assert_eq!(completed[0].source, "1");
}

#[test]
#[cfg(layout_detection)]
fn engine_batch_execution_plan_matches_layout_aware_resolution() {
    let config = ExtractionConfig {
        concurrency: Some(crate::core::config::ConcurrencyConfig { max_threads: Some(4) }),
        ..Default::default()
    };
    let non_layout = resolve_engine_batch_execution_plan_for(&config, LayoutBatchWorkload::None, 8);
    assert_eq!(non_layout.workers, 4);
    assert_eq!(non_layout.thread_budget, 1);
    let layout = resolve_engine_batch_execution_plan_for(&config, LayoutBatchWorkload::All, 8);
    assert_eq!(layout.workers, 1);
    assert_eq!(layout.thread_budget, 4);

    let explicit = ExtractionConfig {
        max_concurrent_extractions: Some(2),
        ..config
    };
    let layout_explicit = resolve_engine_batch_execution_plan_for(&explicit, LayoutBatchWorkload::All, 8);
    assert_eq!(layout_explicit.workers, 1);
    assert_eq!(layout_explicit.thread_budget, 4);
    let non_layout_explicit = resolve_engine_batch_execution_plan_for(&explicit, LayoutBatchWorkload::None, 8);
    assert_eq!(non_layout_explicit.workers, 2);
    assert_eq!(non_layout_explicit.thread_budget, 2);
}

#[test]
fn engine_batch_base_config_applies_plan_budget_once() {
    let base = Arc::new(ExtractionConfig {
        concurrency: Some(crate::core::config::ConcurrencyConfig { max_threads: Some(8) }),
        ..Default::default()
    });

    let adjusted = resolve_batch_base_config(&base, 2);
    assert_eq!(
        adjusted.concurrency.as_ref().and_then(|config| config.max_threads),
        Some(2)
    );
    assert!(!Arc::ptr_eq(&base, &adjusted));

    let reused = resolve_batch_base_config(&adjusted, 2);
    assert!(Arc::ptr_eq(&adjusted, &reused));
}

#[test]
fn engine_batch_execution_plan_clamps_explicit_zero_to_one() {
    let config = ExtractionConfig {
        max_concurrent_extractions: Some(0),
        ..Default::default()
    };

    assert_eq!(
        resolve_engine_batch_execution_plan_for(&config, LayoutBatchWorkload::None, 8).workers,
        1
    );
}

#[test]
fn engine_batch_execution_plan_without_layout_respects_input_count() {
    let config = ExtractionConfig {
        concurrency: Some(crate::core::config::ConcurrencyConfig { max_threads: Some(4) }),
        ..Default::default()
    };
    let inputs = vec![ExtractInput::default()];

    assert_eq!(resolve_engine_batch_execution_plan(&config, &inputs).workers, 1);
}

#[cfg(layout_detection)]
#[test]
fn engine_batch_classifies_all_markdown_pdfs_for_single_layout_worker() {
    let config = ExtractionConfig {
        concurrency: Some(crate::core::config::ConcurrencyConfig { max_threads: Some(8) }),
        layout: Some(Default::default()),
        use_layout_for_markdown: true,
        disable_ocr: true,
        ..Default::default()
    };
    let inputs = vec![ExtractInput::from_uri("document.pdf"); 4];

    assert_eq!(classify_layout_batch(&config, &inputs), LayoutBatchWorkload::All);
    let plan = resolve_engine_batch_execution_plan(&config, &inputs);
    assert_eq!(plan.workers, 1);
    assert_eq!(plan.thread_budget, 8);
}

#[cfg(layout_detection)]
#[test]
fn engine_batch_classifies_disabled_layout_as_none_when_ocr_is_disabled() {
    let config = ExtractionConfig {
        concurrency: Some(crate::core::config::ConcurrencyConfig { max_threads: Some(8) }),
        layout: Some(Default::default()),
        use_layout_for_markdown: false,
        disable_ocr: true,
        ..Default::default()
    };
    let inputs = vec![ExtractInput::from_uri("document.pdf"); 4];

    assert_eq!(classify_layout_batch(&config, &inputs), LayoutBatchWorkload::None);
    assert_eq!(resolve_engine_batch_execution_plan(&config, &inputs).workers, 4);
}

#[cfg(layout_detection)]
#[test]
fn engine_batch_classifies_partial_input_layout_override_as_mixed() {
    let config = ExtractionConfig {
        concurrency: Some(crate::core::config::ConcurrencyConfig { max_threads: Some(8) }),
        use_layout_for_markdown: true,
        disable_ocr: true,
        ..Default::default()
    };
    let layout_input = ExtractInput {
        config: Some(crate::core::config::FileExtractionConfig {
            layout: Some(Default::default()),
            ..Default::default()
        }),
        ..ExtractInput::from_uri("layout.pdf")
    };
    let inputs = vec![
        layout_input,
        ExtractInput::from_uri("plain.pdf"),
        ExtractInput::from_uri("plain.pdf"),
        ExtractInput::from_uri("plain.pdf"),
    ];

    assert_eq!(classify_layout_batch(&config, &inputs), LayoutBatchWorkload::Mixed);
    let plan = resolve_engine_batch_execution_plan(&config, &inputs);
    assert_eq!(plan.workers, 2);
    assert_eq!(plan.thread_budget, 4);
}

#[cfg(layout_detection)]
#[test]
fn engine_batch_classifies_ocr_capable_layout_as_mixed() {
    let config = ExtractionConfig {
        concurrency: Some(crate::core::config::ConcurrencyConfig { max_threads: Some(8) }),
        layout: Some(Default::default()),
        use_layout_for_markdown: false,
        disable_ocr: false,
        ..Default::default()
    };
    let inputs = vec![ExtractInput::from_uri("image.png"); 4];

    assert_eq!(classify_layout_batch(&config, &inputs), LayoutBatchWorkload::Mixed);
    assert_eq!(resolve_engine_batch_execution_plan(&config, &inputs).workers, 2);
}

#[cfg(layout_detection)]
#[test]
fn engine_batch_classifies_ordinary_batch_as_non_layout() {
    let config = ExtractionConfig {
        concurrency: Some(crate::core::config::ConcurrencyConfig { max_threads: Some(8) }),
        ..Default::default()
    };
    let inputs = vec![ExtractInput::from_uri("document.txt"); 4];

    assert_eq!(classify_layout_batch(&config, &inputs), LayoutBatchWorkload::None);
    assert_eq!(resolve_engine_batch_execution_plan(&config, &inputs).workers, 4);
}

#[cfg(all(layout_detection, feature = "url-ingestion"))]
#[test]
fn engine_batch_plan_ignores_shared_url_count_and_layout_overrides() {
    let config = ExtractionConfig {
        concurrency: Some(crate::core::config::ConcurrencyConfig { max_threads: Some(8) }),
        ..Default::default()
    };
    let shared = ExtractInput {
        config: Some(crate::core::config::FileExtractionConfig {
            layout: Some(Default::default()),
            ..Default::default()
        }),
        ..ExtractInput::from_uri("https://example.com/document.pdf")
    };
    assert!(shared_group_uri(&shared).is_some());

    let local = ExtractInput::from_uri("local.pdf");
    let all_plan = resolve_engine_batch_execution_plan(&config, &[shared, local.clone()]);
    assert_eq!(all_plan.workers, 2);
    assert_eq!(all_plan.thread_budget, 4);

    let pending = VecDeque::from([(1, local, "local.pdf".to_string())]);
    let pending_plan = resolve_pending_batch_execution_plan(&config, &pending);
    assert_eq!(pending_plan.workers, 1);
    assert_eq!(pending_plan.thread_budget, 8);
}

#[cfg(layout_detection)]
#[test]
fn engine_batch_concurrency_detects_per_input_layout_override() {
    let config = ExtractionConfig {
        concurrency: Some(crate::core::config::ConcurrencyConfig { max_threads: Some(4) }),
        ..Default::default()
    };
    let inputs = vec![ExtractInput {
        config: Some(crate::core::config::FileExtractionConfig {
            layout: Some(Default::default()),
            ..Default::default()
        }),
        ..Default::default()
    }];

    let plan = resolve_engine_batch_execution_plan(&config, &inputs);
    assert_eq!(plan.workers, 1);
    assert_eq!(plan.thread_budget, 4);
}

#[cfg(feature = "url-ingestion")]
#[tokio::test]
async fn url_markdown_page_runs_through_pipeline() {
    let config = ExtractionConfig::default();
    let links = vec![ExtractedUri {
        url: "https://example.com/next".to_string(),
        label: Some("next".to_string()),
        page: None,
        kind: UriKind::Hyperlink,
    }];

    let result = run_url_page_pipeline(
        "alpha beta gamma delta epsilon zeta eta theta".to_string(),
        true,
        "text/html; charset=utf-8",
        links,
        &config,
    )
    .await
    .unwrap();

    assert_eq!(result.mime_type, "text/markdown");
    assert_eq!(result.metadata.output_format.as_deref(), Some("plain"));
    assert_eq!(result.uris.as_ref().map(Vec::len), Some(1));
}

#[cfg(feature = "tree-sitter")]
#[tokio::test]
async fn extract_py_local_uri_returns_source_code_mime() {
    use crate::core::config::TreeSitterConfig;

    let dir = tempdir().unwrap();
    let path = dir.path().join("hello.py");
    File::create(&path)
        .unwrap()
        .write_all(b"def greet(name):\n    return f'Hello, {name}!'\n")
        .unwrap();

    let config = ExtractionConfig {
        tree_sitter: Some(TreeSitterConfig::default()),
        ..Default::default()
    };

    let output = crate::engine::Engine::new_default()
        .extract(ExtractInput::from_uri(path.to_string_lossy()), &config)
        .await
        .unwrap();

    assert_eq!(output.results.len(), 1, "expected one result");
    assert_eq!(
        output.results[0].mime_type, "text/x-source-code",
        "Python file must extract as text/x-source-code"
    );
    assert!(output.results[0].content.len() >= 5, "content must be non-trivial");
}

#[cfg(feature = "url-ingestion")]
#[test]
fn refine_downloaded_mime_type_passthrough_non_octet_stream() {
    let refined = refine_downloaded_mime_type("application/pdf", Some("document.py"), "http://example.com/document.py");
    assert_eq!(
        refined, "application/pdf",
        "explicit server MIME type must not be overridden by filename"
    );
}

#[cfg(all(feature = "url-ingestion", feature = "tree-sitter"))]
#[test]
fn refine_downloaded_mime_type_py_extension_resolves_to_source_code() {
    let refined = refine_downloaded_mime_type(
        "application/octet-stream",
        Some("hello.py"),
        "http://example.com/code/hello.py",
    );
    assert_eq!(
        refined, "text/x-source-code",
        "octet-stream with .py filename must resolve to text/x-source-code"
    );
}

#[cfg(feature = "url-ingestion")]
#[test]
fn refine_downloaded_mime_type_no_filename_returns_octet_stream() {
    let refined = refine_downloaded_mime_type("application/octet-stream", None, "http://example.com/download");
    assert_eq!(
        refined, "application/octet-stream",
        "no filename means no refinement; extract_bytes handles sniffing"
    );
}

/// Regression: a shared-URL batch result that maps to no input slot (e.g.
/// crawlberg drops a panicked task as an empty-URL pair) must NOT cause its
/// input to vanish. The sweep fills every unfilled slot with an error so
/// `results + errors == inputs` always holds.
#[cfg(all(feature = "tokio-runtime", feature = "url-ingestion"))]
#[test]
fn fill_dropped_shared_slots_reattaches_or_synthesizes_errors() {
    use std::collections::VecDeque;

    let shared_items = vec![
        SharedUrlItem {
            index: 0,
            source: "http://a/".into(),
            uri: "http://a/".into(),
            config: ExtractionConfig::default(),
        },
        SharedUrlItem {
            index: 1,
            source: "http://b/".into(),
            uri: "http://b/".into(),
            config: ExtractionConfig::default(),
        },
        SharedUrlItem {
            index: 2,
            source: "http://c/".into(),
            uri: "http://c/".into(),
            config: ExtractionConfig::default(),
        },
    ];
    let mut items: Vec<Option<BatchItemResult>> = vec![
        Some(BatchItemResult {
            index: 0,
            source: "http://a/".into(),
            result: Err(crate::XbergError::Other("a".into())),
        }),
        None,
        Some(BatchItemResult {
            index: 2,
            source: "http://c/".into(),
            result: Err(crate::XbergError::Other("c".into())),
        }),
    ];
    let mut unmatched = VecDeque::new();
    unmatched.push_back(crate::XbergError::Other("task panicked: boom".into()));

    fill_dropped_shared_slots(&shared_items, &mut items, unmatched);

    assert!(items.iter().all(Option::is_some), "every shared slot must be filled");
    let filled = items[1].as_ref().expect("slot 1 filled");
    assert_eq!(filled.index, 1);
    assert_eq!(filled.source, "http://b/");
    match &filled.result {
        Err(crate::XbergError::Other(message)) => {
            assert!(message.contains("task panicked: boom"), "got: {message}");
        }
        _ => panic!("expected the re-attached panic error in slot 1"),
    }
}

/// When no unmatched error was captured, the synthesized error names the URL.
#[cfg(all(feature = "tokio-runtime", feature = "url-ingestion"))]
#[test]
fn fill_dropped_shared_slots_synthesizes_when_no_captured_error() {
    use std::collections::VecDeque;

    let shared_items = vec![SharedUrlItem {
        index: 0,
        source: "http://x/".into(),
        uri: "http://x/".into(),
        config: ExtractionConfig::default(),
    }];
    let mut items: Vec<Option<BatchItemResult>> = vec![None];

    fill_dropped_shared_slots(&shared_items, &mut items, VecDeque::new());

    match &items[0].as_ref().expect("slot 0 filled").result {
        Err(crate::XbergError::Other(message)) => {
            assert!(
                message.contains("http://x/"),
                "synthesized error names the URL, got: {message}"
            );
        }
        _ => panic!("expected a synthesized error naming the URL"),
    }
}

#[cfg(all(feature = "tokio-runtime", feature = "url-ingestion"))]
#[tokio::test]
async fn shared_url_duration_includes_fetch_without_extending_conversion_timeout() {
    let config = ExtractionConfig {
        extraction_timeout_secs: Some(1),
        ..ExtractionConfig::default()
    };
    let shared = SharedUrlItem {
        index: 0,
        source: "http://example.com/".into(),
        uri: "http://example.com/".into(),
        config,
    };
    let batch_started = Instant::now() - std::time::Duration::from_millis(25);
    let conversion = async { Ok(ExtractionResult::single(ExtractedDocument::default())) };

    let item = finalize_shared_item(&shared, batch_started, conversion).await;

    let output = item.result.expect("immediate conversion remains within its timeout");
    assert_eq!(output.results.len(), 1);
    assert!(
        output.results[0].metadata.extraction_duration_ms.unwrap_or_default() >= 25,
        "duration must include time before conversion began"
    );
}
