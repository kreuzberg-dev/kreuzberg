//! In-memory job store for async extraction polling.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use moka::ops::compute::{CompResult, Op};
use moka::sync::Cache;

use crate::api::types::{JobState, JobStatus};
use crate::cancellation::CancellationToken;

/// Default time-to-live for completed/failed jobs (5 minutes).
const JOB_TTL: Duration = Duration::from_secs(300);

/// Maximum number of concurrent jobs held in the cache.
const MAX_CAPACITY: u64 = 10_000;

/// Maximum number of jobs in Pending or Running state at any one time.
pub const MAX_ACTIVE_JOBS: usize = 100;

/// Thread-safe in-memory store for async extraction jobs.
///
/// Uses [`moka::sync::Cache`] with built-in TTL eviction — no background
/// eviction task required. Entries are evicted automatically after 5 minutes.
/// Server restarts clear all active jobs.
#[derive(Clone)]
pub struct JobStore {
    jobs: Cache<String, JobStatus>,
    tokens: Cache<String, CancellationToken>,
    active: Arc<AtomicUsize>,
}

impl Default for JobStore {
    fn default() -> Self {
        Self::new()
    }
}

impl JobStore {
    /// Create a new empty job store with default TTL and capacity.
    pub fn new() -> Self {
        let jobs = Cache::builder()
            .max_capacity(MAX_CAPACITY)
            .time_to_live(JOB_TTL)
            .build();
        let tokens = Cache::builder()
            .max_capacity(MAX_CAPACITY)
            .time_to_live(JOB_TTL)
            .build();
        Self {
            jobs,
            tokens,
            active: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Return the number of jobs currently in Pending or Running state.
    pub fn active_count(&self) -> usize {
        self.active.load(Ordering::Relaxed)
    }

    /// Create a new job in the store and return its ID.
    ///
    /// This handles ID generation and initial state registration in one atomic step.
    pub fn create_job(&self) -> String {
        let job_id = generate_job_id();
        let now = now_rfc3339();
        self.active.fetch_add(1, Ordering::Relaxed);
        self.create(job_id.clone(), now);
        self.tokens.insert(job_id.clone(), CancellationToken::default());
        job_id
    }

    /// Return the cancellation token associated with a job, if it still exists.
    ///
    /// Pass the returned token's clone to the extraction call (via
    /// `ExtractionConfig::cancel_token`) so it observes cancellation at its
    /// next checkpoint.
    pub fn cancellation_token(&self, job_id: &str) -> Option<CancellationToken> {
        self.tokens.get(job_id)
    }

    /// Register a new job in `Pending` state. Returns its initial `JobStatus`.
    pub fn create(&self, job_id: String, timestamp: String) -> JobStatus {
        let status = JobStatus {
            job_id: job_id.clone(),
            state: JobState::Pending,
            created_at: timestamp.clone(),
            updated_at: timestamp,
            result: None,
            error: None,
        };
        self.jobs.insert(job_id, status.clone());
        status
    }

    /// Retrieve the current status of a job by ID.
    pub fn get(&self, job_id: &str) -> Option<JobStatus> {
        self.jobs.get(job_id)
    }

    /// Transition a job to the `Running` state.
    pub fn set_running(&self, job_id: &str, timestamp: String) {
        if let Some(mut status) = self.jobs.get(job_id) {
            status.state = JobState::Running;
            status.updated_at = timestamp;
            self.jobs.insert(job_id.to_string(), status);
        }
    }

    /// Mark a job as `Completed` and store its result.
    ///
    /// A no-op if the job was already cancelled, so a late-arriving result
    /// from a cancelled extraction cannot clobber the `Cancelled` state. The
    /// read-modify-write against the job entry and the `active` decrement are
    /// atomic with respect to concurrent `cancel`/`fail` calls on the same
    /// job ID (see [`JobStore::cancel`] for why this matters).
    pub fn complete(&self, job_id: &str, result: serde_json::Value, timestamp: String) {
        let outcome = self.jobs.entry_by_ref(job_id).and_compute_with(|entry| match entry {
            Some(entry) if entry.value().state == JobState::Cancelled => Op::Nop,
            Some(entry) => {
                let mut status = entry.into_value();
                status.state = JobState::Completed;
                status.result = Some(result);
                status.updated_at = timestamp;
                Op::Put(status)
            }
            None => Op::Nop,
        });

        if matches!(outcome, CompResult::ReplacedWith(_)) {
            self.active.fetch_sub(1, Ordering::Relaxed);
        }
    }

    /// Mark a job as `Failed` and store the error message.
    ///
    /// A no-op if the job was already cancelled, so a late-arriving error
    /// from a cancelled extraction cannot clobber the `Cancelled` state. See
    /// [`JobStore::complete`] for the atomicity guarantee.
    pub fn fail(&self, job_id: &str, error: String, timestamp: String) {
        let outcome = self.jobs.entry_by_ref(job_id).and_compute_with(|entry| match entry {
            Some(entry) if entry.value().state == JobState::Cancelled => Op::Nop,
            Some(entry) => {
                let mut status = entry.into_value();
                status.state = JobState::Failed;
                status.error = Some(error);
                status.updated_at = timestamp;
                Op::Put(status)
            }
            None => Op::Nop,
        });

        if matches!(outcome, CompResult::ReplacedWith(_)) {
            self.active.fetch_sub(1, Ordering::Relaxed);
        }
    }

    /// Cancel a pending or running job.
    ///
    /// Fires the job's [`CancellationToken`] so a running extraction observes
    /// it at its next checkpoint. Jobs that already reached a terminal state
    /// (`Completed`, `Failed`, or `Cancelled`) cannot be cancelled again.
    ///
    /// The state transition and the `active` decrement happen inside a single
    /// [`moka::Cache::entry_by_ref`] `and_compute_with` call, which moka
    /// serializes per key. This closes a race with the job's own background
    /// task calling [`JobStore::complete`]/[`JobStore::fail`] concurrently:
    /// without it, both sides could read the pre-transition state, both would
    /// decrement `active`, and the second decrement would underflow the
    /// counter (permanently blocking new submissions once `active_count()`
    /// wraps past `MAX_ACTIVE_JOBS`).
    pub fn cancel(&self, job_id: &str, timestamp: String) -> CancelOutcome {
        let outcome = self.jobs.entry_by_ref(job_id).and_compute_with(|entry| match entry {
            Some(entry) => {
                let mut status = entry.into_value();
                match status.state {
                    JobState::Pending | JobState::Running => {
                        status.state = JobState::Cancelled;
                        status.updated_at = timestamp;
                        Op::Put(status)
                    }
                    JobState::Completed | JobState::Failed | JobState::Cancelled => Op::Nop,
                }
            }
            None => Op::Nop,
        });

        match outcome {
            CompResult::StillNone(_) => CancelOutcome::NotFound,
            CompResult::Unchanged(entry) => CancelOutcome::Conflict(entry.into_value()),
            CompResult::ReplacedWith(entry) => {
                self.active.fetch_sub(1, Ordering::Relaxed);
                if let Some(token) = self.tokens.get(job_id) {
                    token.cancel();
                }
                CancelOutcome::Cancelled(entry.into_value())
            }
            CompResult::Inserted(_) | CompResult::Removed(_) => {
                unreachable!("cancel() only produces Nop/Put on an existing entry, never Remove or a fresh Insert")
            }
        }
    }
}

/// Outcome of a [`JobStore::cancel`] call.
#[derive(Debug, Clone)]
pub enum CancelOutcome {
    /// The job was pending or running and is now cancelled.
    Cancelled(JobStatus),
    /// The job already reached a terminal state and cannot be cancelled.
    Conflict(JobStatus),
    /// No job exists with this ID (unknown or expired).
    NotFound,
}

/// Generate a new unique job ID (UUID v4).
pub fn generate_job_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// Return the current time formatted as RFC 3339 (ISO 8601).
pub fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339()
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::api::types::JobState;

    #[test]
    fn test_create_job_is_pending() {
        let store = JobStore::new();
        let ts = "2026-05-01T12:00:00Z".to_string();
        let status = store.create("job-1".to_string(), ts.clone());
        assert_eq!(status.state, JobState::Pending);
        assert_eq!(status.job_id, "job-1");
        assert_eq!(status.created_at, ts);
    }

    #[test]
    fn test_get_existing_job() {
        let store = JobStore::new();
        store.create("job-2".to_string(), "2026-05-01T12:00:00Z".to_string());
        let got = store.get("job-2");
        assert!(got.is_some());
        assert_eq!(got.unwrap().job_id, "job-2");
    }

    #[test]
    fn test_get_missing_job_returns_none() {
        let store = JobStore::new();
        assert!(store.get("nope").is_none());
    }

    #[test]
    fn test_set_running_transitions_state() {
        let store = JobStore::new();
        store.create("job-3".to_string(), "2026-05-01T12:00:00Z".to_string());
        store.set_running("job-3", "2026-05-01T12:00:01Z".to_string());
        let status = store.get("job-3").unwrap();
        assert_eq!(status.state, JobState::Running);
    }

    #[test]
    fn test_complete_stores_result() {
        let store = JobStore::new();
        store.create("job-4".to_string(), "2026-05-01T12:00:00Z".to_string());
        store.complete(
            "job-4",
            serde_json::json!({"content": "hello"}),
            "2026-05-01T12:00:02Z".to_string(),
        );
        let status = store.get("job-4").unwrap();
        assert_eq!(status.state, JobState::Completed);
        assert!(status.result.is_some());
    }

    #[test]
    fn test_fail_stores_error() {
        let store = JobStore::new();
        store.create("job-5".to_string(), "2026-05-01T12:00:00Z".to_string());
        store.fail(
            "job-5",
            "OCR unavailable".to_string(),
            "2026-05-01T12:00:03Z".to_string(),
        );
        let status = store.get("job-5").unwrap();
        assert_eq!(status.state, JobState::Failed);
        assert_eq!(status.error.as_deref(), Some("OCR unavailable"));
    }

    #[test]
    fn test_cancel_pending_job() {
        let store = JobStore::new();
        let job_id = store.create_job();
        assert_eq!(store.active_count(), 1);

        match store.cancel(&job_id, "2026-05-01T12:00:01Z".to_string()) {
            CancelOutcome::Cancelled(status) => assert_eq!(status.state, JobState::Cancelled),
            other => panic!("expected Cancelled, got {other:?}"),
        }
        assert_eq!(store.get(&job_id).unwrap().state, JobState::Cancelled);
        assert_eq!(store.active_count(), 0);
    }

    #[test]
    fn test_cancel_running_job_fires_token() {
        let store = JobStore::new();
        let job_id = store.create_job();
        store.set_running(&job_id, "2026-05-01T12:00:01Z".to_string());
        let token = store.cancellation_token(&job_id).expect("token registered on create");
        assert!(!token.is_cancelled());

        match store.cancel(&job_id, "2026-05-01T12:00:02Z".to_string()) {
            CancelOutcome::Cancelled(status) => assert_eq!(status.state, JobState::Cancelled),
            other => panic!("expected Cancelled, got {other:?}"),
        }
        assert!(token.is_cancelled(), "cancelling a running job must fire its token");
    }

    #[test]
    fn test_cancel_completed_job_is_conflict() {
        let store = JobStore::new();
        let job_id = store.create_job();
        store.complete(
            &job_id,
            serde_json::json!({"content": "done"}),
            "2026-05-01T12:00:01Z".to_string(),
        );

        match store.cancel(&job_id, "2026-05-01T12:00:02Z".to_string()) {
            CancelOutcome::Conflict(status) => assert_eq!(status.state, JobState::Completed),
            other => panic!("expected Conflict, got {other:?}"),
        }
        assert_eq!(
            store.get(&job_id).unwrap().state,
            JobState::Completed,
            "a conflicting cancel must not alter the job's state"
        );
    }

    #[test]
    fn test_cancel_failed_job_is_conflict() {
        let store = JobStore::new();
        let job_id = store.create_job();
        store.fail(&job_id, "boom".to_string(), "2026-05-01T12:00:01Z".to_string());

        match store.cancel(&job_id, "2026-05-01T12:00:02Z".to_string()) {
            CancelOutcome::Conflict(status) => assert_eq!(status.state, JobState::Failed),
            other => panic!("expected Conflict, got {other:?}"),
        }
    }

    #[test]
    fn test_cancel_already_cancelled_job_is_conflict() {
        let store = JobStore::new();
        let job_id = store.create_job();
        store.cancel(&job_id, "2026-05-01T12:00:01Z".to_string());

        match store.cancel(&job_id, "2026-05-01T12:00:02Z".to_string()) {
            CancelOutcome::Conflict(status) => assert_eq!(status.state, JobState::Cancelled),
            other => panic!("expected Conflict, got {other:?}"),
        }
    }

    #[test]
    fn test_cancel_missing_job_returns_not_found() {
        let store = JobStore::new();
        assert!(matches!(
            store.cancel("nope", "2026-05-01T12:00:00Z".to_string()),
            CancelOutcome::NotFound
        ));
    }

    #[test]
    fn test_complete_after_cancel_is_noop() {
        let store = JobStore::new();
        let job_id = store.create_job();
        store.cancel(&job_id, "2026-05-01T12:00:01Z".to_string());

        store.complete(
            &job_id,
            serde_json::json!({"content": "late"}),
            "2026-05-01T12:00:02Z".to_string(),
        );

        let status = store.get(&job_id).unwrap();
        assert_eq!(
            status.state,
            JobState::Cancelled,
            "a late completion must not override a cancelled job"
        );
        assert!(status.result.is_none());
    }

    #[test]
    fn test_fail_after_cancel_is_noop() {
        let store = JobStore::new();
        let job_id = store.create_job();
        store.cancel(&job_id, "2026-05-01T12:00:01Z".to_string());

        store.fail(&job_id, "late error".to_string(), "2026-05-01T12:00:02Z".to_string());

        let status = store.get(&job_id).unwrap();
        assert_eq!(
            status.state,
            JobState::Cancelled,
            "a late failure must not override a cancelled job"
        );
        assert!(status.error.is_none());
    }

    #[test]
    fn test_generate_job_id_is_valid_uuid() {
        let id = generate_job_id();
        assert!(!id.is_empty());
        assert!(
            uuid::Uuid::parse_str(&id).is_ok(),
            "generated job ID must be a valid UUID: {id}"
        );
    }

    #[test]
    fn test_create_job_helper() {
        let store = JobStore::new();
        let id = store.create_job();
        assert!(!id.is_empty());
        let status = store.get(&id).expect("job must be created");
        assert_eq!(status.state, JobState::Pending);
    }
}

#[test]
fn test_create_job_concurrent_uniqueness() {
    use std::sync::Arc;
    use std::thread;

    let store = Arc::new(JobStore::new());
    let mut handles = vec![];

    for _ in 0..100 {
        let store_clone = Arc::clone(&store);
        handles.push(thread::spawn(move || store_clone.create_job()));
    }

    let mut job_ids = std::collections::HashSet::new();
    for handle in handles {
        let id = handle.join().unwrap();
        assert!(job_ids.insert(id.clone()), "Duplicate job ID generated: {}", id);
    }

    assert_eq!(job_ids.len(), 100);
}

/// Regression test for a race where `cancel()` and `complete()`/`fail()` on the
/// same job ID could both read the pre-transition state and both decrement
/// `active`, underflowing the `AtomicUsize` counter and permanently blocking
/// new job submissions. `entry_by_ref(..).and_compute_with(..)` serializes the
/// two calls per key, so exactly one of them observes the pending/running
/// state and performs the transition + decrement.
#[test]
fn test_concurrent_cancel_and_complete_never_double_decrements() {
    use std::sync::Arc;
    use std::thread;

    for _ in 0..200 {
        let store = Arc::new(JobStore::new());
        let job_id = store.create_job();

        let store_a = Arc::clone(&store);
        let job_id_a = job_id.clone();
        let cancel_thread = thread::spawn(move || {
            store_a.cancel(&job_id_a, "2026-05-01T12:00:01Z".to_string());
        });

        let store_b = Arc::clone(&store);
        let job_id_b = job_id.clone();
        let complete_thread = thread::spawn(move || {
            store_b.complete(
                &job_id_b,
                serde_json::json!({"content": "done"}),
                "2026-05-01T12:00:01Z".to_string(),
            );
        });

        cancel_thread.join().unwrap();
        complete_thread.join().unwrap();

        assert_eq!(
            store.active_count(),
            0,
            "active must be decremented exactly once by whichever of cancel/complete won the race, \
             never zero times (leak) or twice (underflow)"
        );

        let final_state = store.get(&job_id).unwrap().state;
        assert!(
            matches!(final_state, JobState::Cancelled | JobState::Completed),
            "the job must end in whichever terminal state won the race, not a corrupted mix: got {final_state:?}"
        );
    }
}
