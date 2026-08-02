//! Layout detection types and (when the layout-detection capability is enabled) inference
//! via RT-DETR and, on the ORT engine, YOLO/TATR/SLANeXT/PP-DocLayout-V3.
//!
//! The `types` submodule is always available under the `layout-types` feature (pure-Rust,
//! no ORT dependency). The inference submodules (`engine`, `models`, etc.) require the
//! layout-detection capability, available in two variants (mirrors `crate::doc_orientation`):
//! - `layout-detection` — ONNX Runtime. All models: RT-DETR, YOLO, TATR, SLANeXT,
//!   PP-DocLayout-V3, table classifier.
//! - `layout-tract` — pure-Rust `tract` engine (no-ORT targets, e.g. Android x86_64
//!   emulator). Only the engine-neutral models: RT-DETR (detection) and the table
//!   classifier (wired/wireless). Table STRUCTURE recognition (TATR, SLANeXT) and
//!   PP-DocLayout-V3 are ORT-only — see `crate::layout::session` and the `models`
//!   submodule doc comments for why — and simply do not compile under `layout-tract`.
//!
//! The `layout_detection` cfg (set by `build.rs`) is true whenever either variant is
//! active, so engine-neutral capability sites need not enumerate both. Consumer code
//! that only works on the ORT engine (table structure recognition, YOLO custom models)
//! still gates on the literal `layout-detection` feature.
//!
//! The ONNX/tract session is cached globally so that repeated extractions (e.g. batch
//! processing) pay model-load cost only once.

/// Layout detection result types (pure-Rust, available under `layout-types`).
pub mod types;

#[cfg(layout_detection)]
/// High-level layout detection engine wrapping model loading and inference.
pub mod engine;
#[cfg(layout_detection)]
/// Error types for layout detection failures.
pub mod error;
#[cfg(layout_detection)]
pub(crate) mod inference_timings;
#[cfg(all(layout_detection, not(target_arch = "wasm32")))]
/// Model downloading and caching (Hugging Face Hub). Not available on `wasm32` — the JS
/// host supplies model bytes directly, see `crate::layout::engine::LayoutEngine::from_rtdetr_bytes`.
pub(crate) mod model_manager;
#[cfg(layout_detection)]
/// Model implementations for layout detection (RT-DETR, and ORT-only: YOLO, TATR, SLANeXT,
/// PP-DocLayout-V3, table classifier).
pub mod models;
#[cfg(layout_detection)]
/// Postprocessing heuristics for raw model detections. NMS (ORT-only, YOLO) lives under
/// `feature = "layout-detection"`; RT-DETR is NMS-free.
pub mod postprocessing;
#[cfg(layout_detection)]
/// Image preprocessing (resize, normalization) for layout model input. Letterbox
/// preprocessing (ORT-only, YOLO/YOLOX) lives under `feature = "layout-detection"`.
pub mod preprocessing;
#[cfg(feature = "layout-detection")]
/// ONNX Runtime session creation and configuration helpers. ORT-only — used by the
/// bare-`ort::Session` models (YOLO, TATR, SLANeXT); the seam-based models (RT-DETR,
/// table classifier, PP-DocLayout-V3) go through `crate::inference` instead.
pub mod session;

pub use types::{BBox, DetectionResult, LayoutClass, LayoutDetection};

#[cfg(layout_detection)]
pub use engine::{CustomModelVariant, DetectTimings, LayoutEngine, LayoutEngineConfig, ModelBackend};
#[cfg(layout_detection)]
pub use error::LayoutError;
#[cfg(all(layout_detection, not(target_arch = "wasm32")))]
pub use model_manager::LayoutModelManager;
#[cfg(layout_detection)]
pub use models::LayoutModel;
#[cfg(layout_detection)]
pub use models::rtdetr::RtDetrModel;
#[cfg(all(layout_detection, feature = "pdf"))]
pub use models::table_classifier::{TableClassifier, TableType};
#[cfg(feature = "layout-detection")]
pub use models::yolo::{YoloModel, YoloVariant};

#[cfg(all(feature = "layout-detection", feature = "pdf"))]
use std::ops::{Deref, DerefMut};
#[cfg(all(feature = "layout-detection", feature = "pdf"))]
use std::sync::{Condvar, Mutex, MutexGuard};

#[cfg(all(
    feature = "layout-detection",
    any(feature = "pdf", feature = "ocr", feature = "ocr-wasm")
))]
use crate::core::config::layout::LayoutDetectionConfig;
#[cfg(all(
    feature = "layout-detection",
    any(feature = "pdf", feature = "ocr", feature = "ocr-wasm")
))]
use crate::model_cache::{ModelCache, ModelLease};

/// Bound retained layout sessions by model size and primary-path concurrency.
///
/// Layout detection and the primary TATR path retain two sessions for batch
/// overlap. Large optional SLANet models retain one session per variant, and
/// additional callers wait for an RAII lease to keep RSS bounded.
#[cfg(all(
    feature = "layout-detection",
    any(feature = "pdf", feature = "ocr", feature = "ocr-wasm")
))]
const LAYOUT_ENGINE_POOL_CAPACITY: usize = 2;
#[cfg(all(feature = "layout-detection", feature = "pdf"))]
const TATR_MODEL_POOL_CAPACITY: usize = 2;
#[cfg(all(feature = "layout-detection", feature = "pdf"))]
const SLANET_MODEL_POOL_CAPACITY: usize = 1;
#[cfg(all(feature = "layout-detection", feature = "pdf"))]
const TABLE_CLASSIFIER_POOL_CAPACITY: usize = 1;

/// Global cached layout engine.
///
/// Used by the image extractor (layout-detection + ocr/ocr-wasm) and the
/// PDF extractor (layout-detection + pdf).
#[cfg(all(
    feature = "layout-detection",
    any(feature = "pdf", feature = "ocr", feature = "ocr-wasm")
))]
static CACHED_ENGINE: ModelCache<LayoutEngine> = ModelCache::with_capacity(LAYOUT_ENGINE_POOL_CAPACITY);

/// Global cached TATR table structure recognition model.
#[cfg(all(feature = "layout-detection", feature = "pdf"))]
static CACHED_TATR: ModelCache<ConfiguredModel<models::tatr::TatrModel>> =
    ModelCache::with_capacity(TATR_MODEL_POOL_CAPACITY);

#[cfg(all(feature = "layout-detection", feature = "pdf"))]
type ModelAccelerationKey = Option<crate::core::config::acceleration::AccelerationConfig>;

#[cfg(all(feature = "layout-detection", feature = "pdf"))]
#[derive(Clone, PartialEq)]
struct ModelSessionKey {
    acceleration: ModelAccelerationKey,
    thread_budget: usize,
}

#[cfg(all(feature = "layout-detection", feature = "pdf"))]
pub(crate) struct ConfiguredModel<T> {
    session_key: ModelSessionKey,
    model: T,
}

#[cfg(all(feature = "layout-detection", feature = "pdf"))]
impl<T> ConfiguredModel<T> {
    fn matches_session(&self, session_key: &ModelSessionKey) -> bool {
        self.session_key == *session_key
    }
}

#[cfg(all(feature = "layout-detection", feature = "pdf"))]
impl<T> Deref for ConfiguredModel<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.model
    }
}

#[cfg(all(feature = "layout-detection", feature = "pdf"))]
impl<T> DerefMut for ConfiguredModel<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.model
    }
}

#[cfg(all(feature = "layout-detection", feature = "pdf"))]
pub(crate) type TatrLease = ModelLease<'static, ConfiguredModel<models::tatr::TatrModel>>;
#[cfg(all(feature = "layout-detection", feature = "pdf"))]
pub(crate) type SlanetLease = ModelLease<'static, ConfiguredModel<models::slanet::SlanetModel>>;
#[cfg(all(feature = "layout-detection", feature = "pdf"))]
pub(crate) type TableClassifierLease = ModelLease<'static, ConfiguredModel<models::table_classifier::TableClassifier>>;

#[cfg(all(feature = "layout-detection", feature = "pdf"))]
#[derive(Clone, Copy)]
enum AvailabilityState {
    Unknown,
    Loading,
    Available,
    Failed,
}

#[cfg(all(feature = "layout-detection", feature = "pdf"))]
struct ModelAvailability {
    state: Mutex<AvailabilityState>,
    changed: Condvar,
}

#[cfg(all(feature = "layout-detection", feature = "pdf"))]
struct AvailabilityAttempt<'a> {
    availability: &'a ModelAvailability,
    completed: bool,
}

#[cfg(all(feature = "layout-detection", feature = "pdf"))]
impl ModelAvailability {
    const fn new() -> Self {
        Self {
            state: Mutex::new(AvailabilityState::Unknown),
            changed: Condvar::new(),
        }
    }

    fn lock_state(&self) -> MutexGuard<'_, AvailabilityState> {
        self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn begin_default_attempt(&self) -> Option<AvailabilityAttempt<'_>> {
        let mut state = self.lock_state();
        loop {
            match *state {
                AvailabilityState::Unknown => {
                    *state = AvailabilityState::Loading;
                    return Some(AvailabilityAttempt {
                        availability: self,
                        completed: false,
                    });
                }
                AvailabilityState::Loading => {
                    state = self
                        .changed
                        .wait(state)
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                }
                AvailabilityState::Available => {
                    return Some(AvailabilityAttempt {
                        availability: self,
                        completed: false,
                    });
                }
                AvailabilityState::Failed => return None,
            }
        }
    }

    fn record_default_result(&self, succeeded: bool) -> bool {
        let mut state = self.lock_state();
        match (*state, succeeded) {
            (AvailabilityState::Available, _) | (AvailabilityState::Failed, _) => false,
            (_, true) => {
                *state = AvailabilityState::Available;
                self.changed.notify_all();
                false
            }
            (AvailabilityState::Unknown | AvailabilityState::Loading, false) => {
                *state = AvailabilityState::Failed;
                self.changed.notify_all();
                true
            }
        }
    }

    fn settled(&self) -> Option<bool> {
        match *self.lock_state() {
            AvailabilityState::Available => Some(true),
            AvailabilityState::Failed => Some(false),
            AvailabilityState::Unknown | AvailabilityState::Loading => None,
        }
    }
}

#[cfg(all(feature = "layout-detection", feature = "pdf"))]
impl AvailabilityAttempt<'_> {
    fn finish(mut self, succeeded: bool) -> bool {
        self.completed = true;
        self.availability.record_default_result(succeeded)
    }
}

#[cfg(all(feature = "layout-detection", feature = "pdf"))]
impl Drop for AvailabilityAttempt<'_> {
    fn drop(&mut self) {
        if !self.completed {
            self.availability.record_default_result(false);
        }
    }
}

#[cfg(all(feature = "layout-detection", feature = "pdf"))]
fn begin_model_attempt<'a>(
    availability: &'a ModelAvailability,
    acceleration: &ModelAccelerationKey,
) -> Option<Option<AvailabilityAttempt<'a>>> {
    if acceleration.is_some() {
        Some(None)
    } else {
        availability.begin_default_attempt().map(Some)
    }
}

#[cfg(all(feature = "layout-detection", feature = "pdf"))]
fn finish_model_attempt(attempt: Option<AvailabilityAttempt<'_>>, succeeded: bool) -> bool {
    match attempt {
        Some(attempt) => attempt.finish(succeeded),
        None => !succeeded,
    }
}

#[cfg(all(feature = "layout-detection", feature = "pdf"))]
static TATR_AVAILABILITY: ModelAvailability = ModelAvailability::new();

/// Convert a [`LayoutDetectionConfig`] into a [`LayoutEngineConfig`].
#[cfg(all(
    feature = "layout-detection",
    any(feature = "pdf", feature = "ocr", feature = "ocr-wasm")
))]
pub(crate) fn config_from_extraction(layout_config: &LayoutDetectionConfig) -> LayoutEngineConfig {
    LayoutEngineConfig {
        backend: ModelBackend::RtDetr,
        confidence_threshold: layout_config.confidence_threshold,
        apply_heuristics: layout_config.apply_heuristics,
        cache_dir: None,
        acceleration: layout_config.acceleration.clone(),
    }
}

/// Take the cached layout engine, or create a new one if the cache is empty.
///
/// The caller owns the engine for the duration of its work and should
/// return it via [`return_engine`] when done. This avoids holding the
/// global mutex during inference.
#[cfg(all(
    feature = "layout-detection",
    any(feature = "pdf", feature = "ocr", feature = "ocr-wasm")
))]
pub(crate) fn take_or_create_engine(
    layout_config: &LayoutDetectionConfig,
    thread_budget: usize,
) -> Result<ModelLease<'static, LayoutEngine>, LayoutError> {
    let desired_config = config_from_extraction(layout_config);
    let create_config = desired_config.clone();
    let thread_budget = thread_budget.max(1);
    CACHED_ENGINE.take_or_create_matching(
        |engine| engine.matches_config(&desired_config, thread_budget),
        || {
            crate::ort_discovery::ensure_ort_available();
            LayoutEngine::from_config_with_thread_budget(create_config, thread_budget)
        },
    )
}

/// Return a layout engine to the global cache for reuse by future extractions.
#[cfg(all(
    feature = "layout-detection",
    any(feature = "pdf", feature = "ocr", feature = "ocr-wasm")
))]
pub(crate) fn return_engine(engine: ModelLease<'static, LayoutEngine>) {
    drop(engine);
}

/// Take the cached TATR model, or create a new one if the cache is empty.
///
/// Returns `None` if the model cannot be loaded. A failed default-acceleration
/// load is cached to avoid repeated download attempts. Explicit acceleration
/// configurations remain retryable and log each construction failure.
#[cfg(all(feature = "layout-detection", feature = "pdf"))]
pub(crate) fn take_or_create_tatr(
    accel: Option<&crate::core::config::acceleration::AccelerationConfig>,
    thread_budget: usize,
) -> Option<TatrLease> {
    let session_key = ModelSessionKey {
        acceleration: accel.cloned(),
        thread_budget: thread_budget.max(1),
    };
    let attempt = begin_model_attempt(&TATR_AVAILABILITY, &session_key.acceleration)?;

    let create_key = session_key.clone();
    let result = CACHED_TATR.take_or_create_matching(
        |model| model.matches_session(&session_key),
        || {
            crate::ort_discovery::ensure_ort_available();
            let manager = LayoutModelManager::new(None);
            let model_path = manager.ensure_tatr_model()?;
            let model = models::tatr::TatrModel::from_file(
                &model_path.to_string_lossy(),
                create_key.acceleration.as_ref(),
                create_key.thread_budget,
            )?;
            Ok::<_, LayoutError>(ConfiguredModel {
                session_key: create_key,
                model,
            })
        },
    );

    match result {
        Ok(model) => {
            finish_model_attempt(attempt, true);
            Some(model)
        }
        Err(e) => {
            if finish_model_attempt(attempt, false) {
                tracing::warn!("TATR table structure model unavailable, table structure recognition disabled: {e}");
            }
            None
        }
    }
}

/// Return a TATR model to the global cache for reuse.
#[cfg(all(feature = "layout-detection", feature = "pdf"))]
pub(crate) fn return_tatr(model: TatrLease) {
    drop(model);
}

/// Global cached SLANeXT wired model.
#[cfg(all(feature = "layout-detection", feature = "pdf"))]
static CACHED_SLANET_WIRED: ModelCache<ConfiguredModel<models::slanet::SlanetModel>> =
    ModelCache::with_capacity(SLANET_MODEL_POOL_CAPACITY);

/// Global cached SLANeXT wireless model.
#[cfg(all(feature = "layout-detection", feature = "pdf"))]
static CACHED_SLANET_WIRELESS: ModelCache<ConfiguredModel<models::slanet::SlanetModel>> =
    ModelCache::with_capacity(SLANET_MODEL_POOL_CAPACITY);

/// Global cached SLANet-plus model.
#[cfg(all(feature = "layout-detection", feature = "pdf"))]
static CACHED_SLANET_PLUS: ModelCache<ConfiguredModel<models::slanet::SlanetModel>> =
    ModelCache::with_capacity(SLANET_MODEL_POOL_CAPACITY);

/// Global cached table classifier model.
#[cfg(all(feature = "layout-detection", feature = "pdf"))]
static CACHED_TABLE_CLASSIFIER: ModelCache<ConfiguredModel<models::table_classifier::TableClassifier>> =
    ModelCache::with_capacity(TABLE_CLASSIFIER_POOL_CAPACITY);

/// Tracks default-acceleration availability independently per SLANeXT variant.
#[cfg(all(feature = "layout-detection", feature = "pdf"))]
static SLANET_WIRED_AVAILABILITY: ModelAvailability = ModelAvailability::new();
#[cfg(all(feature = "layout-detection", feature = "pdf"))]
static SLANET_WIRELESS_AVAILABILITY: ModelAvailability = ModelAvailability::new();
#[cfg(all(feature = "layout-detection", feature = "pdf"))]
static SLANET_PLUS_AVAILABILITY: ModelAvailability = ModelAvailability::new();
#[cfg(all(feature = "layout-detection", feature = "pdf"))]
static TABLE_CLASSIFIER_AVAILABILITY: ModelAvailability = ModelAvailability::new();

/// Take a cached SLANeXT model for the given variant, or create a new one.
#[cfg(all(feature = "layout-detection", feature = "pdf"))]
pub(crate) fn take_or_create_slanet(
    variant: &str,
    accel: Option<&crate::core::config::acceleration::AccelerationConfig>,
    thread_budget: usize,
) -> Option<SlanetLease> {
    let (cache, availability) = match variant {
        "slanet_wired" => (&CACHED_SLANET_WIRED, &SLANET_WIRED_AVAILABILITY),
        "slanet_wireless" => (&CACHED_SLANET_WIRELESS, &SLANET_WIRELESS_AVAILABILITY),
        "slanet_plus" => (&CACHED_SLANET_PLUS, &SLANET_PLUS_AVAILABILITY),
        _ => return None,
    };

    let session_key = ModelSessionKey {
        acceleration: accel.cloned(),
        thread_budget: thread_budget.max(1),
    };
    let attempt = begin_model_attempt(availability, &session_key.acceleration)?;

    let create_key = session_key.clone();
    let result = cache.take_or_create_matching(
        |model| model.matches_session(&session_key),
        || {
            crate::ort_discovery::ensure_ort_available();
            let manager = LayoutModelManager::new(None);
            let model_path = manager.ensure_slanet_model(variant)?;
            let model = models::slanet::SlanetModel::from_file(
                &model_path.to_string_lossy(),
                create_key.acceleration.as_ref(),
                create_key.thread_budget,
            )?;
            Ok::<_, LayoutError>(ConfiguredModel {
                session_key: create_key,
                model,
            })
        },
    );

    match result {
        Ok(model) => {
            finish_model_attempt(attempt, true);
            Some(model)
        }
        Err(e) => {
            if finish_model_attempt(attempt, false) {
                tracing::warn!(variant, "SLANeXT model unavailable: {e}");
            }
            None
        }
    }
}

/// Returns `true` if the TATR table structure model is loadable.
///
/// On first call, attempts to load TATR using default acceleration. Subsequent
/// calls return the settled default-acceleration result. This makes the check a
/// safe fail-fast guard before code paths that would otherwise perform the first
/// model load.
#[cfg(all(feature = "layout-detection", feature = "pdf"))]
pub(crate) fn is_tatr_available(
    acceleration: Option<&crate::core::config::acceleration::AccelerationConfig>,
    thread_budget: usize,
) -> bool {
    if acceleration.is_none()
        && let Some(result) = TATR_AVAILABILITY.settled()
    {
        return result;
    }
    if let Some(model) = take_or_create_tatr(acceleration, thread_budget) {
        return_tatr(model);
        true
    } else {
        false
    }
}

/// Returns `true` if the selected SLANeXT variant and acceleration are loadable.
///
/// The caller supplies the exact variant used by inference; explicit acceleration
/// checks bypass the default-path negative cache.
#[cfg(all(feature = "layout-detection", feature = "pdf"))]
pub(crate) fn is_slanet_available(
    variant: &str,
    acceleration: Option<&crate::core::config::acceleration::AccelerationConfig>,
    thread_budget: usize,
) -> bool {
    take_or_create_slanet(variant, acceleration, thread_budget).is_some()
}

/// Take a cached table classifier, or create a new one.
#[cfg(all(feature = "layout-detection", feature = "pdf"))]
pub(crate) fn take_or_create_table_classifier(
    accel: Option<&crate::core::config::acceleration::AccelerationConfig>,
    thread_budget: usize,
) -> Option<TableClassifierLease> {
    let session_key = ModelSessionKey {
        acceleration: accel.cloned(),
        thread_budget: thread_budget.max(1),
    };
    let attempt = begin_model_attempt(&TABLE_CLASSIFIER_AVAILABILITY, &session_key.acceleration)?;

    let create_key = session_key.clone();
    let result = CACHED_TABLE_CLASSIFIER.take_or_create_matching(
        |model| model.matches_session(&session_key),
        || {
            crate::ort_discovery::ensure_ort_available();
            let manager = LayoutModelManager::new(None);
            let model_path = manager.ensure_table_classifier()?;
            let model = models::table_classifier::TableClassifier::from_file_with_thread_budget(
                &model_path.to_string_lossy(),
                create_key.acceleration.as_ref(),
                create_key.thread_budget,
            )?;
            Ok::<_, LayoutError>(ConfiguredModel {
                session_key: create_key,
                model,
            })
        },
    );

    match result {
        Ok(model) => {
            finish_model_attempt(attempt, true);
            Some(model)
        }
        Err(e) => {
            if finish_model_attempt(attempt, false) {
                tracing::warn!("Table classifier unavailable: {e}");
            }
            None
        }
    }
}

#[cfg(all(test, feature = "layout-detection", feature = "pdf"))]
mod availability_tests {
    use super::*;
    use crate::core::config::acceleration::{AccelerationConfig, ExecutionProviderType};
    use std::sync::Arc;
    use std::time::Duration;

    fn acceleration_key(device_id: u32) -> ModelAccelerationKey {
        Some(AccelerationConfig {
            provider: ExecutionProviderType::Cpu,
            device_id,
        })
    }

    #[test]
    fn later_failure_cannot_downgrade_success() {
        let availability = ModelAvailability::new();
        let succeeding = availability.begin_default_attempt().unwrap();
        succeeding.finish(true);

        let failing = availability.begin_default_attempt().unwrap();
        failing.finish(false);
        assert_eq!(availability.settled(), Some(true));
    }

    #[test]
    fn initial_default_load_is_single_flight() {
        let availability = Arc::new(ModelAvailability::new());
        let first = availability.begin_default_attempt().unwrap();
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (finished_tx, finished_rx) = std::sync::mpsc::channel();
        let waiting_availability = Arc::clone(&availability);
        let waiter = std::thread::spawn(move || {
            started_tx.send(()).unwrap();
            let attempt = waiting_availability.begin_default_attempt().unwrap();
            attempt.finish(false);
            finished_tx.send(()).unwrap();
        });

        started_rx.recv().unwrap();
        assert!(matches!(
            finished_rx.recv_timeout(Duration::from_millis(20)),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout)
        ));
        first.finish(true);
        finished_rx.recv().unwrap();
        waiter.join().unwrap();
        assert_eq!(availability.settled(), Some(true));
    }

    #[test]
    fn initial_failure_settles_default_path() {
        let availability = ModelAvailability::new();
        let attempt = availability.begin_default_attempt().unwrap();

        assert!(attempt.finish(false));
        assert_eq!(availability.settled(), Some(false));
        assert!(availability.begin_default_attempt().is_none());
    }

    #[test]
    fn explicit_acceleration_is_not_negative_cached() {
        let availability = ModelAvailability::new();
        availability.begin_default_attempt().unwrap().finish(false);
        let explicit = acceleration_key(1);

        assert!(matches!(begin_model_attempt(&availability, &explicit), Some(None)));
        assert!(matches!(begin_model_attempt(&availability, &explicit), Some(None)));
        assert!(!finish_model_attempt(None, true));
        assert!(finish_model_attempt(None, false));
    }

    fn session_key(acceleration: ModelAccelerationKey, thread_budget: usize) -> ModelSessionKey {
        ModelSessionKey {
            acceleration,
            thread_budget,
        }
    }

    #[test]
    fn configured_model_pool_does_not_cross_acceleration_keys() {
        let cache = ModelCache::with_capacity(1);
        let cpu = session_key(acceleration_key(0), 2);
        let other_device = session_key(acceleration_key(1), 2);
        drop(
            cache
                .take_or_create(|| {
                    Ok::<_, ()>(ConfiguredModel {
                        session_key: cpu,
                        model: 1,
                    })
                })
                .unwrap(),
        );

        let lease = cache
            .take_or_create_matching(
                |model| model.matches_session(&other_device),
                || {
                    Ok::<_, ()>(ConfiguredModel {
                        session_key: other_device.clone(),
                        model: 2,
                    })
                },
            )
            .unwrap();

        assert!(lease.matches_session(&other_device));
        assert_eq!(**lease, 2);
    }

    #[test]
    fn configured_model_pool_does_not_cross_thread_budgets() {
        let cache = ModelCache::with_capacity(1);
        let two_threads = session_key(acceleration_key(0), 2);
        let four_threads = session_key(acceleration_key(0), 4);
        drop(
            cache
                .take_or_create(|| {
                    Ok::<_, ()>(ConfiguredModel {
                        session_key: two_threads,
                        model: 1,
                    })
                })
                .unwrap(),
        );

        let lease = cache
            .take_or_create_matching(
                |model| model.matches_session(&four_threads),
                || {
                    Ok::<_, ()>(ConfiguredModel {
                        session_key: four_threads.clone(),
                        model: 2,
                    })
                },
            )
            .unwrap();

        assert!(lease.matches_session(&four_threads));
        assert_eq!(**lease, 2);
    }
}
