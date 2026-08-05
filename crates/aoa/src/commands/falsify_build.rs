//! IO and presentation shell for `aoa eval experiment`.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{bail, Context, Result};
use aoa_bench::Sha256Digest;
use aoa_falsify_build::{build, BuildReport, Manifest};

use crate::cli::ExperimentArgs;
use crate::commands::fsutil::load_json_capped;
use crate::output::{escape_terminal, print_human, print_json};

fn build_report_path(out: &Path) -> PathBuf {
    out.with_extension("build.json")
}

fn observations_path(out: &Path) -> PathBuf {
    out.with_extension("observations.jsonl")
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    static NONCE: AtomicU64 = AtomicU64::new(0);
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .context("output path must have a UTF-8 file name")?;
    let temporary = parent.join(format!(
        ".{name}.{}.{}.tmp",
        std::process::id(),
        NONCE.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| -> Result<()> {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .with_context(|| format!("failed to create {}", temporary.display()))?;
        file.write_all(bytes)
            .with_context(|| format!("failed to write {}", temporary.display()))?;
        file.sync_all()
            .with_context(|| format!("failed to sync {}", temporary.display()))?;
        std::fs::rename(&temporary, path)
            .with_context(|| format!("failed to install {}", path.display()))
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

pub(crate) fn run(args: &ExperimentArgs) -> Result<i32> {
    if let Some(threshold) = args.min_pair_yield {
        if !threshold.is_finite() || !(0.0..=1.0).contains(&threshold) {
            bail!("--min-pair-yield must be a finite fraction in [0, 1]");
        }
    }
    let manifest: Manifest = load_json_capped(&args.manifest, "manifest")?;
    let base_dir = args.manifest.parent().unwrap_or_else(|| Path::new("."));
    let (input, report, observations) = build(&manifest, &args.tasks, base_dir)?;
    let input_json = serde_json::to_string_pretty(&input)?;

    let mut observations_jsonl = Vec::new();
    for observation in &observations {
        serde_json::to_writer(&mut observations_jsonl, observation)?;
        observations_jsonl.push(b'\n');
    }
    let observation_path = observations_path(&args.out);
    let report = report.with_artifacts(
        args.out.display().to_string(),
        observation_path.display().to_string(),
        Sha256Digest::of_bytes(&observations_jsonl).to_string(),
    );
    let report_path = build_report_path(&args.out);
    let report_json = serde_json::to_string_pretty(&report)?;
    write_atomic(&args.out, input_json.as_bytes())?;
    write_atomic(&observation_path, &observations_jsonl)?;
    write_atomic(&report_path, format!("{report_json}\n").as_bytes())?;

    if args.json {
        print_json(&report)?;
    } else {
        print_human(&render_human(&report, &report_path));
    }
    enforce_pair_yield(args.min_pair_yield, &report)?;
    Ok(0)
}

fn enforce_pair_yield(threshold: Option<f64>, report: &BuildReport) -> Result<()> {
    let Some(threshold) = threshold else {
        return Ok(());
    };
    let low_yield = report
        .repos
        .iter()
        .map(|repo| {
            (
                &repo.repo_id,
                repo.identical_pairs,
                repo.candidate_pairs,
                repo.pair_yield,
            )
        })
        .chain(
            report
                .dropped_repos
                .iter()
                .map(|repo| (&repo.repo_id, 0, repo.candidate_pairs, repo.pair_yield)),
        )
        .find(|(_, _, _, pair_yield)| *pair_yield < threshold);
    if let Some((repo_id, admitted, candidates, pair_yield)) = low_yield {
        bail!(
            "pair-yield preflight failed: {} admitted {}/{} pairs ({:.3}), below {:.3}",
            repo_id,
            admitted,
            candidates,
            pair_yield,
            threshold
        );
    }
    Ok(())
}

fn render_human(report: &BuildReport, report_path: &Path) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    let _ = writeln!(
        out,
        "aoa eval experiment: built {} repo(s), {} identical pair(s) -> {}",
        report.repo_count,
        report.total_identical_pairs,
        escape_terminal(&report.out_path),
    );
    for repo in &report.repos {
        let _ = writeln!(
            out,
            "  {:<24} pairs={}/{} yield={:.3} holdout={} provenance={:?} confidence={:?} calibrated={} exposure={:?} eligible={}",
            escape_terminal(&repo.repo_id),
            repo.identical_pairs,
            repo.candidate_pairs,
            repo.pair_yield,
            repo.holdout_size,
            repo.native_span,
            repo.confidence,
            repo.calibrated,
            repo.exposure,
            repo.eligible,
        );
        render_exclusions(&mut out, &repo.excluded_tasks);
    }
    for repo in &report.dropped_repos {
        let _ = writeln!(
            out,
            "  {:<24} DROPPED: no identical pairs (pairs=0/{} yield={:.3})",
            escape_terminal(&repo.repo_id),
            repo.candidate_pairs,
            repo.pair_yield,
        );
        render_exclusions(&mut out, &repo.excluded_tasks);
    }
    if report.convention_inputs_degraded {
        let _ = writeln!(
            out,
            "  convention_inputs_degraded=true -> the verdict will abstain (inconclusive)",
        );
    }
    let _ = writeln!(
        out,
        "  build report: {}",
        escape_terminal(&report_path.display().to_string())
    );
    out
}

fn render_exclusions(out: &mut String, exclusions: &[aoa_falsify_build::ExcludedTask]) {
    use std::fmt::Write as _;
    for exclusion in exclusions {
        let _ = writeln!(
            out,
            "      excluded {}: {}",
            escape_terminal(&exclusion.task_id),
            escape_terminal(&exclusion.reason)
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aoa_falsify_build::{DroppedRepo, ExcludedTask, TaskShape};

    #[test]
    fn build_report_path_swaps_extension() {
        assert_eq!(
            build_report_path(Path::new("result.json")),
            PathBuf::from("result.build.json")
        );
    }

    #[test]
    fn human_report_escapes_external_exclusion_diagnostics() {
        let report = BuildReport {
            out_path: "out\ninjected.json".to_string(),
            observations_path: "out.observations.jsonl".to_string(),
            observations_sha256: "a".repeat(64),
            observation_count: 1,
            observation_ids: vec!["b".repeat(64)],
            repo_count: 0,
            total_identical_pairs: 0,
            task_shape: TaskShape::Answer,
            convention_inputs_degraded: false,
            repos: Vec::new(),
            dropped_repos: vec![DroppedRepo {
                repo_id: "repo".to_string(),
                candidate_pairs: 1,
                pair_yield: 0.0,
                excluded_tasks: vec![ExcludedTask {
                    task_id: "task".to_string(),
                    reason: "boom\u{1b}[31mRED".to_string(),
                }],
            }],
            notes: Vec::new(),
        };

        let rendered = render_human(&report, Path::new("out.build\ninjected.json"));

        assert!(!rendered.contains('\u{1b}'));
        assert!(rendered.contains(r"\u{1b}"));
        assert!(!rendered.contains("out\ninjected"));
        assert!(!rendered.contains("out.build\ninjected"));
        assert!(rendered.contains(r"out\ninjected.json"));
        assert!(rendered.contains(r"out.build\ninjected.json"));
    }

    #[test]
    fn run_rejects_oversized_manifest() {
        let dir = tempfile::tempdir().unwrap();
        let manifest = dir.path().join("manifest.json");
        let file = std::fs::File::create(&manifest).unwrap();
        file.set_len(crate::commands::fsutil::MAX_JSON_BYTES + 1)
            .unwrap();
        let args = ExperimentArgs {
            manifest,
            tasks: dir.path().join("tasks"),
            out: dir.path().join("out.json"),
            min_pair_yield: None,
            json: false,
        };
        let error = run(&args).unwrap_err();
        assert!(format!("{error:#}").contains("exceeds"));
    }
}
