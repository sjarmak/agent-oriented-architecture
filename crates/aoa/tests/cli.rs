use std::path::{Path, PathBuf};
use std::process::Command;

use aoa_construct::MIN_HELD_OUT_OBSERVATIONS;
use assert_cmd::prelude::*;
use predicates::prelude::*;
use serde_json::Value;
use tempfile::TempDir;

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
}

fn aoa() -> Command {
    Command::cargo_bin("aoa").expect("aoa binary builds")
}

#[path = "cli_sections/audit_observe_lint.rs"]
mod audit_observe_lint;
#[path = "cli_sections/audit_self.rs"]
mod audit_self;
#[path = "cli_sections/behavioral_signal.rs"]
mod behavioral_signal;
#[path = "cli_sections/checkbox_baseline.rs"]
mod checkbox_baseline;
#[path = "cli_sections/core.rs"]
mod core;
#[path = "cli_sections/enforce.rs"]
mod enforce;
#[path = "cli_sections/eval_run.rs"]
mod eval_run;
#[path = "cli_sections/experiment.rs"]
mod experiment;
#[path = "cli_sections/exposure.rs"]
mod exposure;
#[path = "cli_sections/falsify_policy.rs"]
mod falsify_policy;
#[path = "cli_sections/gap_recommend.rs"]
mod gap_recommend;
#[path = "cli_sections/input_boundaries.rs"]
mod input_boundaries;
#[path = "cli_sections/migrate.rs"]
mod migrate;
#[path = "cli_sections/ownership.rs"]
mod ownership;
#[path = "cli_sections/policy_observe.rs"]
mod policy_observe;
#[path = "cli_sections/r0b.rs"]
mod r0b;
#[path = "cli_sections/report.rs"]
mod report;
