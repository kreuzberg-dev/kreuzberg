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

/// Argmax over the last `classes`-sized row of a flat logits buffer.
///
/// Errors instead of panicking when the buffer is empty or smaller than one
/// row: ONNX output shapes are model-controlled input.
pub(crate) fn argmax_last_row(shape: &[i64], data: &[f32]) -> Result<usize, LayoutError> {
    let classes = *shape.last().unwrap_or(&0) as usize;
    if classes == 0 || data.len() < classes {
        return Err(LayoutError::InvalidOutput(format!(
            "logits buffer of {} values cannot hold a row of {classes}",
            data.len()
        )));
    }
    let row = &data[data.len() - classes..];
    Ok(row
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(b.1))
        .map(|(i, _)| i)
        .unwrap_or(0))
}

#[cfg(test)]
mod argmax_tests {
    use super::argmax_last_row;

    #[test]
    fn picks_the_last_row_maximum() {
        // Two rows of three classes; the last row's max is index 1.
        let data = [9.0, 0.0, 0.0, 0.1, 5.0, 0.2];
        assert_eq!(argmax_last_row(&[2, 3], &data).unwrap(), 1);
    }

    #[test]
    fn empty_output_errors_instead_of_panicking() {
        assert!(argmax_last_row(&[0], &[]).is_err());
        assert!(argmax_last_row(&[1, 4], &[0.0]).is_err());
    }
}
