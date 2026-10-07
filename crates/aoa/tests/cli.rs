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

#[cfg(unix)]
fn make_fifo(path: &Path) {
    rustix::fs::mknodat(
        rustix::fs::CWD,
        path,
        rustix::fs::FileType::Fifo,
        rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
        0,
    )
    .expect("make a fifo");
}

#[test]
fn the_temporary_directory_sits_under_no_git_marker() {
    let temp = std::env::temp_dir();
    let temp = temp
        .canonicalize()
        .unwrap_or_else(|err| panic!("temporary directory {}: {err}", temp.display()));
    let markers: Vec<PathBuf> = temp
        .ancestors()
        .map(|dir| dir.join(".git"))
        .filter(|marker| marker.symlink_metadata().is_ok())
        .collect();
    assert!(
        markers.is_empty(),
        "the CLI fixtures are created under {} and the repository-root resolver treats every \
         ancestor .git marker as a repository, so {markers:?} would make every fixture a nested \
         checkout; point TMPDIR at a directory with no .git above it",
        temp.display()
    );
}

#[cfg(unix)]
#[path = "cli_sections/audit_git_environment.rs"]
mod audit_git_environment;
#[path = "cli_sections/audit_observe_lint.rs"]
mod audit_observe_lint;
#[path = "cli_sections/audit_self.rs"]
mod audit_self;
#[path = "cli_sections/behavioral_signal.rs"]
mod behavioral_signal;
#[path = "cli_sections/checkbox_baseline.rs"]
mod checkbox_baseline;
#[cfg(unix)]
#[path = "cli_sections/cli_git_environment.rs"]
mod cli_git_environment;
#[cfg(unix)]
#[path = "cli_sections/cli_operator_git_config.rs"]
mod cli_operator_git_config;
#[path = "cli_sections/core.rs"]
mod core;
#[path = "cli_sections/enforce.rs"]
mod enforce;
#[path = "cli_sections/enforce_liveness.rs"]
mod enforce_liveness;
#[path = "cli_sections/enforce_nested_repository.rs"]
mod enforce_nested_repository;
#[path = "cli_sections/enforce_relative_target.rs"]
mod enforce_relative_target;
#[path = "cli_sections/eval_run.rs"]
mod eval_run;
#[path = "cli_sections/eval_run_graph_coverage.rs"]
mod eval_run_graph_coverage;
#[path = "cli_sections/eval_run_trace_db.rs"]
mod eval_run_trace_db;
#[path = "cli_sections/eval_run_traces.rs"]
mod eval_run_traces;
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
#[path = "cli_sections/lint_discovery.rs"]
mod lint_discovery;
#[path = "cli_sections/migrate.rs"]
mod migrate;
#[path = "cli_sections/ownership.rs"]
mod ownership;
#[path = "cli_sections/policy_observe.rs"]
mod policy_observe;
#[path = "cli_sections/r0b.rs"]
mod r0b;
#[path = "cli_sections/readiness_graph_coverage.rs"]
mod readiness_graph_coverage;
#[path = "cli_sections/report.rs"]
mod report;
