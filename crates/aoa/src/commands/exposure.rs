use std::fmt::Write as _;

use anyhow::Result;
use aoa_gap::ExposureStatus;

use crate::cli::ExposureScanArgs;
use crate::output::{print_human, print_json};

pub fn scan(args: &ExposureScanArgs) -> Result<i32> {
    let report = aoa_bench::scan_exposure(&args.runs)?;
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
