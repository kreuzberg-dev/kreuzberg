//! Environment diagnostics: probe configured backends and report what will
//! actually execute on this host.
//!
//! `doctor` answers "is it my document or my environment?" before the first
//! document is processed. Each configured backend gets a `probe()` pass /
//! warn / fail / skip verdict with a one-line reason; config-level misconfigurations
//! and cache hygiene are reported alongside. Nothing is downloaded and no
//! billable API call is made.

mod cache;
mod config_lint;
#[cfg(all(layout_detection, not(target_arch = "wasm32")))]
mod layout;
#[cfg(not(all(layout_detection, not(target_arch = "wasm32"))))]
mod layout_unavailable;
mod ocr;

pub use cache::{CleanOutcome, clean_obsolete};

use crate::core::config::ExtractionConfig;
use serde::{Deserialize, Serialize};

/// Outcome of a single doctor check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProbeStatus {
    /// The backend or setting will work as configured.
    Pass,
    /// The check ran and found something actionable, but nothing is broken
    /// (e.g. stray cache files, stale model revisions). Never fails the report.
    Warn,
    /// The configured setup will not work (or will silently degrade) on this host.
    Fail,
    /// The check cannot run locally (e.g. model not cached, feature not compiled in);
    /// first real use decides, possibly after a download.
    Skip,
}

/// A single doctor verdict: what was checked, the outcome, and why.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DoctorCheck {
    /// Check identifier, e.g. `ocr.tesseract` or `layout.rtdetr`.
    pub name: String,
    /// Pass / warn / fail / skip verdict.
    pub status: ProbeStatus,
    /// One-line reason or detail (e.g. missing language, resolved path, error).
    pub message: String,
}

impl DoctorCheck {
    /// A [`ProbeStatus::Pass`] verdict: the checked backend or setting will
    /// work as configured on this host.
    pub fn pass(name: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            status: ProbeStatus::Pass,
            message: message.into(),
        }
    }

    /// A [`ProbeStatus::Warn`] verdict: the check ran and found something
    /// actionable, but nothing is broken. Never fails the report.
    pub fn warn(name: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            status: ProbeStatus::Warn,
            message: message.into(),
        }
    }

    /// A [`ProbeStatus::Fail`] verdict: the configured setup will not work (or
    /// will silently degrade) on this host.
    pub fn fail(name: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            status: ProbeStatus::Fail,
            message: message.into(),
        }
    }

    /// A [`ProbeStatus::Skip`] verdict: the check cannot run locally; first
    /// real use decides, possibly after a download.
    pub fn skip(name: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            status: ProbeStatus::Skip,
            message: message.into(),
        }
    }
}

/// Aggregate doctor report over all configured backends and settings.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DoctorReport {
    /// Individual check verdicts, in execution order.
    pub checks: Vec<DoctorCheck>,
}

impl DoctorReport {
    /// Whether no check failed. Warnings and skips do not count as failures,
    /// so the report stays usable as a scripting/CI gate.
    pub fn is_ok(&self) -> bool {
        self.checks.iter().all(|c| c.status != ProbeStatus::Fail)
    }
}

/// Probe the backends and settings in `config` and report what will actually
/// execute on this host.
///
/// Runs no downloads and no billable API calls. Backends that are not compiled
/// in or whose models are not cached report `Skip` rather than failing.
pub fn doctor(config: &ExtractionConfig) -> DoctorReport {
    let mut checks = Vec::new();
    checks.extend(config_lint::lint_config(config));
    checks.extend(ocr::probe_ocr(config));
    #[cfg(all(layout_detection, not(target_arch = "wasm32")))]
    checks.extend(layout::probe_layout(config));
    #[cfg(not(all(layout_detection, not(target_arch = "wasm32"))))]
    checks.extend(layout_unavailable::probe_layout(config));
    checks.extend(cache::check_cache(config));
    DoctorReport { checks }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_ok_only_without_failures() {
        let mut report = DoctorReport::default();
        assert!(report.is_ok());
        report.checks.push(DoctorCheck::pass("a", "fine"));
        report.checks.push(DoctorCheck::skip("b", "not cached"));
        report.checks.push(DoctorCheck::warn("c", "stray files"));
        assert!(report.is_ok());
        report.checks.push(DoctorCheck::fail("d", "broken"));
        assert!(!report.is_ok());
    }

    #[test]
    fn probe_status_serializes_lowercase() {
        let check = DoctorCheck::fail("ocr.vlm", "no key");
        let json = serde_json::to_value(&check).unwrap();
        assert_eq!(json["status"], "fail");
        let roundtrip: DoctorCheck = serde_json::from_value(json).unwrap();
        assert_eq!(roundtrip.status, ProbeStatus::Fail);
        assert_eq!(roundtrip.message, "no key");

        let warn = serde_json::to_value(DoctorCheck::warn("cache.xberg", "stray")).unwrap();
        assert_eq!(warn["status"], "warn");
    }
}
