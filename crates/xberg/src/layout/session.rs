use ort::session::{Session, builder::GraphOptimizationLevel};

use crate::core::config::acceleration::AccelerationConfig;
use crate::layout::error::LayoutError;

/// Build an optimized ORT session from an ONNX model file.
///
/// `thread_budget` controls the number of intra-op threads for this session.
/// Pass the result of [`crate::core::config::concurrency::resolve_thread_budget`]
/// to respect the user's `ConcurrencyConfig`.
///
/// When `accel` is `None` or `Auto`, uses platform defaults:
/// - macOS: CoreML (Neural Engine / GPU)
/// - Linux: CUDA (GPU)
/// - Others: CPU only
///
/// ORT silently falls back to CPU if the requested EP is unavailable.
pub(crate) fn build_session(
    path: &str,
    accel: Option<&AccelerationConfig>,
    thread_budget: usize,
) -> Result<Session, LayoutError> {
    let builder = Session::builder()?
        .with_optimization_level(GraphOptimizationLevel::All)
        .map_err(|e| LayoutError::Ort(ort::Error::new(e.message())))?
        .with_intra_threads(thread_budget)
        .map_err(|e| LayoutError::Ort(ort::Error::new(e.message())))?
        .with_inter_threads(1)
        .map_err(|e| LayoutError::Ort(ort::Error::new(e.message())))?;

    let builder = crate::ort_discovery::apply_execution_providers(builder, accel)
        .map_err(|e| LayoutError::Ort(ort::Error::new(e.message())))?;

    let mut builder = builder;
    let session = builder.commit_from_file(path)?;

    Ok(session)
}
