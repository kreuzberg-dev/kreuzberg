use std::hint::black_box;
use std::path::{Path, PathBuf};
use std::time::Duration;

use criterion::{Criterion, criterion_group, criterion_main};
use image::RgbImage;
use sha2::{Digest, Sha256};
use xberg::core::config::acceleration::{AccelerationConfig, ExecutionProviderType};
use xberg::layout::{DetectionResult, LayoutEngine, LayoutEngineConfig};

const RENDER_DPI: i32 = 150;
const INFERENCE_TOLERANCE: f32 = 1e-3;
const EXISTING_GROUPING: &[usize] = &[1, 1, 1, 1, 6, 1, 1, 1];
const MICROBATCH_GROUPING: &[usize] = &[8, 5];

struct B8Document {
    relative_path: &'static str,
    page_count: usize,
    sha256: &'static str,
}

const B8_DOCUMENTS: &[B8Document] = &[
    B8Document {
        relative_path: "pdf/fake_memo.pdf",
        page_count: 1,
        sha256: "f54e68281e2018cc3200626aa3cc970c63b4ff2f3afb89a4204207a66f5a2aeb",
    },
    B8Document {
        relative_path: "pdf/tiny.pdf",
        page_count: 1,
        sha256: "167ca200dae8b2f947fe68df0b1e9a8b87a197b47a55dc6bffda6c46784a7e9f",
    },
    B8Document {
        relative_path: "pdf/embedded_images_tables.pdf",
        page_count: 1,
        sha256: "00cf5fc3c334076f982b88d4e8f8bf1ecdf8273ce2b1de9d79f462b1c0a54e65",
    },
    B8Document {
        relative_path: "pdf/google_doc_document.pdf",
        page_count: 1,
        sha256: "69f6b7f493b1bc55d518942976cbeadc4ec0a36f6d8a6dc24feffc516d35b2c9",
    },
    B8Document {
        relative_path: "pdf/681693.pdf",
        page_count: 6,
        sha256: "133592c082044b981bb96947ccf8141c66d279375023a823fa3e681c3c8b17e5",
    },
    B8Document {
        relative_path: "pdf/ft_ACN_2009_page_102_t0.pdf",
        page_count: 1,
        sha256: "f8af9cee9feb9c6fb3ab0860b3ad9189dfb776111f95c3506feb4e1f69e87123",
    },
    B8Document {
        relative_path: ".corpus-cache/pdf/pb_fqr-retail-blackrock-global-allocation-fund-inc_page4.pdf",
        page_count: 1,
        sha256: "f38625341b3bc0d57c5d355ddd0d23e45439223dcdfc17bc84b9bb5f9d61d5dd",
    },
    B8Document {
        relative_path: ".corpus-cache/pdf/pb_sample_page_16_page1.pdf",
        page_count: 1,
        sha256: "996d0a61481f2ff5adfc60ff6ea9b62e778c99ae03838b21bcd65fdd03febfc7",
    },
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PageKey {
    document_index: usize,
    page_index: usize,
}

struct RenderedPage {
    key: PageKey,
    image: RgbImage,
}

struct KeyedDetection {
    key: PageKey,
    result: DetectionResult,
}

fn test_documents_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test_documents")
}

fn load_b8_pages() -> Vec<RenderedPage> {
    let root = test_documents_root();
    let mut pages = Vec::with_capacity(EXISTING_GROUPING.iter().sum());

    for (document_index, document) in B8_DOCUMENTS.iter().enumerate() {
        let path = root.join(document.relative_path);
        let pdf_bytes = std::fs::read(&path)
            .unwrap_or_else(|error| panic!("failed to read pinned B8 PDF {}: {error}", path.display()));
        let actual_hash = hex::encode(Sha256::digest(&pdf_bytes));
        assert_eq!(
            actual_hash,
            document.sha256,
            "pinned B8 PDF changed: {}",
            path.display()
        );
        let actual_page_count = xberg::pdf::render::pdf_page_count(&pdf_bytes, None)
            .unwrap_or_else(|error| panic!("failed to count pages in {}: {error}", path.display()));
        assert_eq!(
            actual_page_count,
            document.page_count,
            "pinned B8 page count changed: {}",
            path.display()
        );

        for page_index in 0..document.page_count {
            let png = xberg::pdf::render::render_pdf_page_to_png(&pdf_bytes, page_index, Some(RENDER_DPI), None)
                .unwrap_or_else(|error| {
                    panic!(
                        "failed to render pinned B8 PDF {} page {}: {error}",
                        path.display(),
                        page_index + 1
                    )
                });
            let image = image::load_from_memory(&png)
                .unwrap_or_else(|error| {
                    panic!(
                        "failed to decode rendered B8 PDF {} page {}: {error}",
                        path.display(),
                        page_index + 1
                    )
                })
                .into_rgb8();
            pages.push(RenderedPage {
                key: PageKey {
                    document_index,
                    page_index,
                },
                image,
            });
        }
    }

    pages
}

fn cpu_layout_engine() -> LayoutEngine {
    LayoutEngine::from_config(LayoutEngineConfig {
        acceleration: Some(AccelerationConfig {
            provider: ExecutionProviderType::Cpu,
            ..AccelerationConfig::default()
        }),
        ..LayoutEngineConfig::default()
    })
    .expect("failed to construct the CPU RT-DETR layout engine")
}

fn detect_grouping(engine: &mut LayoutEngine, pages: &[RenderedPage], grouping: &[usize]) -> Vec<KeyedDetection> {
    assert_eq!(
        grouping.iter().sum::<usize>(),
        pages.len(),
        "batch grouping must cover every rendered page"
    );

    let mut results = Vec::with_capacity(pages.len());
    let mut start = 0;
    for &batch_size in grouping {
        let end = start + batch_size;
        let batch = &pages[start..end];
        let images: Vec<&RgbImage> = batch.iter().map(|page| &page.image).collect();
        let detections = engine
            .detect_batch_for_benchmark(&images)
            .expect("RT-DETR batch inference failed");
        assert_eq!(
            detections.len(),
            batch.len(),
            "RT-DETR batch output cardinality changed"
        );
        results.extend(
            batch
                .iter()
                .zip(detections)
                .map(|(page, (result, _timings))| KeyedDetection { key: page.key, result }),
        );
        start = end;
    }
    results
}

fn assert_close(actual: f32, expected: f32, context: &str) {
    assert!(
        (actual - expected).abs() <= INFERENCE_TOLERANCE,
        "{context}: expected {expected}, got {actual}"
    );
}

fn assert_detection_parity(existing: &[KeyedDetection], microbatched: &[KeyedDetection]) {
    assert_eq!(existing.len(), microbatched.len());

    for (existing_page, microbatched_page) in existing.iter().zip(microbatched) {
        assert_eq!(existing_page.key, microbatched_page.key);
        assert_eq!(existing_page.result.page_width, microbatched_page.result.page_width);
        assert_eq!(existing_page.result.page_height, microbatched_page.result.page_height);
        assert_eq!(
            existing_page.result.detections.len(),
            microbatched_page.result.detections.len(),
            "detection count changed for page {:?}",
            existing_page.key
        );

        for (existing_detection, microbatched_detection) in existing_page
            .result
            .detections
            .iter()
            .zip(&microbatched_page.result.detections)
        {
            assert_eq!(existing_detection.class_name, microbatched_detection.class_name);
            assert_close(
                microbatched_detection.confidence,
                existing_detection.confidence,
                "detection confidence changed",
            );
            assert_close(
                microbatched_detection.bbox.x1,
                existing_detection.bbox.x1,
                "bbox x1 changed",
            );
            assert_close(
                microbatched_detection.bbox.y1,
                existing_detection.bbox.y1,
                "bbox y1 changed",
            );
            assert_close(
                microbatched_detection.bbox.x2,
                existing_detection.bbox.x2,
                "bbox x2 changed",
            );
            assert_close(
                microbatched_detection.bbox.y2,
                existing_detection.bbox.y2,
                "bbox y2 changed",
            );
        }
    }
}

fn bench_pdf_layout_microbatch(criterion: &mut Criterion) {
    let pages = load_b8_pages();
    let mut engine = cpu_layout_engine();

    let existing = detect_grouping(&mut engine, &pages, EXISTING_GROUPING);
    let microbatched = detect_grouping(&mut engine, &pages, MICROBATCH_GROUPING);
    assert_detection_parity(&existing, &microbatched);

    let mut group = criterion.benchmark_group("pdf_layout_microbatch_b8");
    group.bench_function("existing_1_1_1_1_6_1_1_1", |bencher| {
        bencher.iter(|| black_box(detect_grouping(&mut engine, black_box(&pages), EXISTING_GROUPING)))
    });
    group.bench_function("microbatch_8_5", |bencher| {
        bencher.iter(|| black_box(detect_grouping(&mut engine, black_box(&pages), MICROBATCH_GROUPING)))
    });
    group.finish();
}

criterion_group! {
    name = benches;
    config = Criterion::default()
        .sample_size(10)
        .warm_up_time(Duration::from_secs(5))
        .measurement_time(Duration::from_secs(30));
    targets = bench_pdf_layout_microbatch
}
criterion_main!(benches);
