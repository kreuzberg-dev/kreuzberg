//! Doctor command - probe configured backends and report what will actually
//! execute on this host.

use anyhow::{Context, Result};
use std::path::PathBuf;
use xberg::{DoctorCheck, ProbeStatus, doctor};

use super::config::load_config;
use crate::{WireFormat, style};

/// Execute the doctor command. Exits nonzero when any check fails, so the
/// command is usable as a setup gate in scripts and CI.
#[expect(
    clippy::print_stdout,
    reason = "the doctor report is the command's stdout result output"
)]
pub fn doctor_command(
    config_path: Option<PathBuf>,
    no_config_discovery: bool,
    format: WireFormat,
    clean: bool,
) -> Result<()> {
    let config = load_config(config_path, !no_config_discovery)?;

    // Clean first so the report reflects the post-clean cache state; the
    // cleanup result is a regular check, keeping one output shape per format.
    let cleaned = clean.then(xberg::doctor::clean_obsolete);
    let mut report = doctor(&config);
    if let Some(outcome) = cleaned {
        report.checks.push(if outcome.failed == 0 {
            DoctorCheck::pass(
                "cache.clean",
                format!("removed {} stray cache file(s)", outcome.removed),
            )
        } else {
            DoctorCheck::fail(
                "cache.clean",
                format!(
                    "removed {} stray cache file(s), failed to remove {}",
                    outcome.removed, outcome.failed
                ),
            )
        });
    }

    match format {
        WireFormat::Text => {
            println!("{}", style::header("Doctor"));
            println!("{}", style::dim("======"));
            for check in &report.checks {
                let marker = match check.status {
                    ProbeStatus::Pass => style::success("pass"),
                    ProbeStatus::Fail => style::label("FAIL"),
                    ProbeStatus::Skip => style::dim("skip"),
                };
                println!("{marker} {} — {}", check.name, check.message);
            }
        }
        WireFormat::Json => {
            println!(
                "{}",
                serde_json::to_string_pretty(&report).context("Failed to serialize doctor report to JSON")?
            );
        }
        WireFormat::Toon => {
            println!(
                "{}",
                serde_toon::to_string(&report).context("Failed to serialize doctor report to TOON")?
            );
        }
    }

    if !report.is_ok() {
        std::process::exit(1);
    }
    Ok(())
}
