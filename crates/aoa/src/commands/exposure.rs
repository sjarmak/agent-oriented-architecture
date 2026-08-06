use std::fmt::Write as _;

use anyhow::{Context, Result};
use aoa_domain::ExposureStatus;

use crate::cli::ExposureScanArgs;
use crate::output::{print_human, print_json};

pub fn scan(args: &ExposureScanArgs) -> Result<i32> {
    let report = aoa_bench::scan_exposure(&args.runs)?;
    if let Some(path) = &args.out {
        let bytes = serde_json::to_vec_pretty(&report).with_context(|| {
            format!(
                "failed to render the exposure ledger for {}",
                path.display()
            )
        })?;
        std::fs::write(path, bytes).with_context(|| {
            format!("failed to write the exposure ledger to {}", path.display())
        })?;
    }
    if args.json {
        print_json(&report)?;
    } else {
        let mut out = String::new();
        let _ = writeln!(
            out,
            "aoa eval exposure scan: {} repo(s)",
            report.repos.len()
        );
        for repo in &report.repos {
            let _ = writeln!(
                out,
                "  {} @ {}: {} ({}/{})",
                repo.repo_id,
                repo.baseline_commit,
                status_name(&repo.status),
                repo.exposed_subject_count(),
                repo.total_subjects,
            );
            if let Some(provenance) = &repo.provenance {
                for path in &provenance.causing_run_paths {
                    let _ = writeln!(out, "    run: {}", path.display());
                }
                let _ = writeln!(
                    out,
                    "    mtime (unix ms): {}..={}",
                    provenance.mtime_range.earliest_unix_ms, provenance.mtime_range.latest_unix_ms,
                );
                let _ = writeln!(
                    out,
                    "    trials: {}; held-out passed={} failed={}; errored={}; unscored={}",
                    provenance.trial_count,
                    provenance.held_out_passed,
                    provenance.held_out_failed,
                    provenance.errored_trials,
                    provenance.unscored_trials,
                );
            }
        }
        print_human(&out);
    }
    Ok(0)
}

fn status_name(status: &ExposureStatus) -> &'static str {
    match status {
        ExposureStatus::Unexposed => "unexposed",
        ExposureStatus::PartiallyExposed { .. } => "partially exposed",
        ExposureStatus::Exposed => "exposed",
    }
}
