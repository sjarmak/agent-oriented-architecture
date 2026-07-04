//! `aoa audit --self`: the R14 lint-thyself gate.
//!
//! The toolkit turns its context-budget lint on itself: an applied migration
//! writes files an agent will read, so the migration manifest's entries are
//! exactly the toolkit's own added context. This command measures each written
//! file's tokens (pinned reference encoding) before the migration — the
//! archived original for an overwrite, nothing for a create — vs after, and
//! flags a **context regression** when the median rose without demonstrated
//! held-out gain.
//!
//! Held-out gain is demonstrated only by a `--baseline`/`--migrated` run pair
//! (the same inputs as `aoa eval compare`) that the reward-hacking gap gate
//! labels `good` — held-out rose AND the gap held or shrank. A `not_good` pair
//! (the gap widened) is reward-hack-shaped evidence and cannot justify a
//! context rise. Without the pair the evidence is reported absent — and an
//! undemonstrated gain cannot justify a rise either, so a rise is still
//! flagged. Comparison warnings (e.g. `zero_canary_leak_shape`) are surfaced
//! in both registers, never dropped. A repo with no applied migration has
//! nothing to measure and says so (absent input, never fabricated).
//!
//! Exit code: 1 when a regression is flagged, 0 otherwise.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Serialize;

use aoa_gap::{CompareWarning, Label};
use aoa_migrate::ManifestEntry;

use crate::cli::AuditArgs;
use crate::commands::eval::load_run;
use crate::commands::fsutil::{read_to_string_capped, MAX_JSON_BYTES};
use crate::output::{print_human, print_json};

/// Exit code returned when a context regression is flagged.
const REGRESSION_EXIT_CODE: i32 = 1;

/// One migration-written file's context-token measurement.
#[derive(Debug, Serialize)]
struct FileTokens {
    path: String,
    /// Tokens the file contributed before the migration: the archived
    /// original's count for an overwrite, 0 for a created file (it did not
    /// exist).
    before_tokens: usize,
    after_tokens: usize,
}

/// The held-out leg as supplied (or not) by the operator.
#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum HeldOutEvidence {
    /// No `--baseline`/`--migrated` run pair supplied: no held-out gain is
    /// demonstrated.
    Absent,
    Present {
        held_out_delta: f64,
        gap_delta: f64,
        /// The gap gate's own verdict on the pair — the same label `aoa eval
        /// compare` prints. Only `good` demonstrates a gain.
        label: Label,
        /// Non-fatal comparison advisories (e.g. a leak-shaped gain with no
        /// canary to adjudicate it), surfaced rather than dropped.
        warnings: Vec<CompareWarning>,
    },
}

impl HeldOutEvidence {
    /// Whether the evidence demonstrates a held-out gain by the toolkit's own
    /// gap gate: only a pair labelled `good` (held-out rose AND the
    /// reward-hacking gap held or shrank) justifies a context rise.
    fn demonstrates_gain(&self) -> bool {
        matches!(
            self,
            HeldOutEvidence::Present {
                label: Label::Good,
                ..
            }
        )
    }
}

/// The self-audit result — the JSON register.
#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum SelfAuditView {
    /// No applied migration is recorded: there is no before/after to measure.
    NoMigration { manifest: String },
    Measured {
        fixes_applied: Vec<String>,
        files: Vec<FileTokens>,
        median_before_tokens: f64,
        median_after_tokens: f64,
        held_out: HeldOutEvidence,
        context_regression: bool,
    },
}

/// Run the self-audit and render it in the requested register.
pub fn run(args: &AuditArgs) -> Result<i32> {
    let view = measure(args)?;
    if args.json {
        print_json(&view)?;
    } else {
        print_human(&render_human(&view));
    }
    Ok(match view {
        SelfAuditView::Measured {
            context_regression: true,
            ..
        } => REGRESSION_EXIT_CODE,
        _ => 0,
    })
}

/// Measure the applied migration's context-token footprint and decide the
/// regression flag.
fn measure(args: &AuditArgs) -> Result<SelfAuditView> {
    let manifest_file = aoa_migrate::manifest_path(&args.repo);
    if !manifest_file.exists() {
        return Ok(SelfAuditView::NoMigration {
            manifest: manifest_file.display().to_string(),
        });
    }
    let manifest = aoa_migrate::read_manifest(&args.repo).with_context(|| {
        format!(
            "failed to read migration manifest in {}",
            args.repo.display()
        )
    })?;
    if manifest.entries.is_empty() {
        anyhow::bail!(
            "migration manifest {} records no entries; nothing to measure",
            manifest_file.display()
        );
    }

    let repo_root = args
        .repo
        .canonicalize()
        .with_context(|| format!("failed to resolve the repo root {}", args.repo.display()))?;
    let encoder =
        aoa_budget::reference_encoder().context("failed to load the reference tokenizer")?;
    let count = |text: &str| aoa_budget::count_tokens(&encoder, text);
    let files = manifest
        .entries
        .iter()
        .map(|entry| file_tokens(&count, &args.repo, &repo_root, entry))
        .collect::<Result<Vec<_>>>()?;

    let held_out = match (&args.baseline, &args.migrated) {
        (Some(baseline), Some(migrated)) => {
            let outcome = aoa_gap::compare(&load_run(baseline)?, &load_run(migrated)?)
                .context("reward-hacking gap comparison failed")?;
            HeldOutEvidence::Present {
                held_out_delta: outcome.held_out_delta,
                gap_delta: outcome.gap_delta,
                label: outcome.label,
                warnings: outcome.warnings,
            }
        }
        // clap enforces the pairing (`requires`), so anything else is neither.
        _ => HeldOutEvidence::Absent,
    };

    let median_before_tokens = median(files.iter().map(|f| f.before_tokens));
    let median_after_tokens = median(files.iter().map(|f| f.after_tokens));
    let context_regression = regression(median_before_tokens, median_after_tokens, &held_out);

    Ok(SelfAuditView::Measured {
        fixes_applied: manifest.fixes_applied,
        files,
        median_before_tokens,
        median_after_tokens,
        held_out,
        context_regression,
    })
}

/// Count one manifest entry's before/after context tokens. A missing written
/// file or archive is a hard error: the manifest says it exists, so its absence
/// is a corrupted migration record, not a measurable state.
fn file_tokens(
    count: &impl Fn(&str) -> usize,
    repo: &Path,
    repo_root: &Path,
    entry: &ManifestEntry,
) -> Result<FileTokens> {
    let read = |path: &Path| {
        let resolved = resolve_in_repo(repo, repo_root, path)?;
        read_to_string_capped(&resolved, MAX_JSON_BYTES)
            .with_context(|| format!("failed to read migration-written file {}", path.display()))
    };
    match entry {
        ManifestEntry::Created { path } => Ok(FileTokens {
            path: path.display().to_string(),
            before_tokens: 0,
            after_tokens: count(&read(path)?),
        }),
        ManifestEntry::Modified { path, archive } => Ok(FileTokens {
            path: path.display().to_string(),
            before_tokens: count(&read(archive)?),
            after_tokens: count(&read(path)?),
        }),
    }
}

/// Resolve one manifest entry path against the audited repo root.
///
/// Apply records paths as `<apply-time repo>/<rel>` verbatim, so a manifest
/// written with a relative `--repo` (the default is `.`) only reads correctly
/// from the apply-time cwd. Re-anchor instead of trusting the cwd: strip the
/// audit-time repo prefix when the entry carries it (same-`--repo`
/// invocations), then join the remainder to the audit-time repo — an absolute
/// entry passes through `join` unchanged. Apply validated that every target
/// stays inside the repo, so a resolved path escaping it is a corrupted (or
/// hostile) migration record: a hard error, never a read of arbitrary
/// reachable files on the manifest's say-so.
fn resolve_in_repo(repo: &Path, repo_root: &Path, path: &Path) -> Result<PathBuf> {
    let joined = repo.join(path.strip_prefix(repo).unwrap_or(path));
    let resolved = joined.canonicalize().with_context(|| {
        format!(
            "failed to resolve migration-written file {}",
            path.display()
        )
    })?;
    if !resolved.starts_with(repo_root) {
        anyhow::bail!(
            "migration manifest entry {} resolves outside the repo {}; \
             refusing a corrupted migration record",
            path.display(),
            repo_root.display()
        );
    }
    Ok(resolved)
}

/// Median of the values: middle element for odd counts, mean of the two middle
/// elements for even counts. Deterministic arithmetic; the caller guarantees a
/// non-empty input (an entry-less manifest is rejected up front).
fn median(values: impl Iterator<Item = usize>) -> f64 {
    let mut sorted: Vec<usize> = values.collect();
    sorted.sort_unstable();
    let n = sorted.len();
    if n % 2 == 1 {
        sorted[n / 2] as f64
    } else {
        (sorted[n / 2 - 1] + sorted[n / 2]) as f64 / 2.0
    }
}

/// The R14 regression rule: flagged iff the median added-context tokens rose
/// AND no held-out gain is demonstrated. Absent evidence is no demonstrated
/// gain, and a pair the gap gate labels `not_good` (held-out rose but the
/// reward-hacking gap widened) is not one either — the toolkit does not get to
/// keep a context rise on an unmeasured promise or on reward-hack-shaped
/// evidence its own comparison gate rejects.
fn regression(median_before: f64, median_after: f64, evidence: &HeldOutEvidence) -> bool {
    median_after > median_before && !evidence.demonstrates_gain()
}

/// Render the self-audit for the human register.
///
/// Entry paths and fix ids come verbatim from on-disk manifest JSON — the same
/// trust level as `falsification.json` — so both are `escape_debug`'d before
/// they reach the terminal.
fn render_human(view: &SelfAuditView) -> String {
    let mut out = String::new();
    match view {
        SelfAuditView::NoMigration { manifest } => {
            let _ = writeln!(
                out,
                "AOA self-audit (R14): no applied migration recorded ({manifest} absent); \
                 nothing to measure."
            );
        }
        SelfAuditView::Measured {
            fixes_applied,
            files,
            median_before_tokens,
            median_after_tokens,
            held_out,
            context_regression,
        } => {
            let fix_ids: Vec<String> = fixes_applied
                .iter()
                .map(|id| id.escape_debug().to_string())
                .collect();
            let _ = writeln!(
                out,
                "AOA self-audit (R14): {} file(s) written by fix(es) [{}]",
                files.len(),
                fix_ids.join(", "),
            );
            for f in files {
                let _ = writeln!(
                    out,
                    "  {}: {} -> {} tokens",
                    f.path.escape_debug(),
                    f.before_tokens,
                    f.after_tokens
                );
            }
            let _ = writeln!(
                out,
                "median context tokens: {median_before_tokens} -> {median_after_tokens}"
            );
            match held_out {
                HeldOutEvidence::Absent => {
                    let _ = writeln!(
                        out,
                        "held-out evidence: absent (no --baseline/--migrated run pair supplied)"
                    );
                }
                HeldOutEvidence::Present {
                    held_out_delta,
                    gap_delta,
                    label,
                    warnings,
                } => {
                    let _ = writeln!(
                        out,
                        "held-out evidence: delta {held_out_delta:+.4} (gap delta \
                         {gap_delta:+.4}), label {label:?}"
                    );
                    for warning in warnings {
                        let _ = writeln!(out, "held-out warning: {warning:?}");
                    }
                }
            }
            if *context_regression {
                let _ = writeln!(
                    out,
                    "verdict: context regression — median added-context tokens rose without \
                     demonstrated held-out gain"
                );
            } else {
                let _ = writeln!(out, "verdict: no context regression");
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn median_handles_odd_and_even_counts() {
        assert_eq!(median([3usize].into_iter()), 3.0);
        assert_eq!(median([9usize, 1, 3].into_iter()), 3.0);
        assert_eq!(median([4usize, 1, 3, 2].into_iter()), 2.5);
    }

    fn present(label: Label) -> HeldOutEvidence {
        HeldOutEvidence::Present {
            held_out_delta: 0.25,
            gap_delta: 0.0,
            label,
            warnings: vec![],
        }
    }

    #[test]
    fn regression_requires_a_rise_and_no_demonstrated_gain() {
        // Rose, no evidence: flagged.
        assert!(regression(0.0, 10.0, &HeldOutEvidence::Absent));
        // Rose, the gap gate labelled the pair not_good (held-out rose but the
        // reward-hacking gap widened): still flagged.
        assert!(regression(0.0, 10.0, &present(Label::NotGood)));
        // Rose, the gap gate labelled the pair good: justified.
        assert!(!regression(0.0, 10.0, &present(Label::Good)));
        // Did not rise: never flagged, evidence or not.
        assert!(!regression(10.0, 10.0, &HeldOutEvidence::Absent));
        assert!(!regression(10.0, 4.0, &HeldOutEvidence::Absent));
    }

    #[test]
    fn resolve_in_repo_reanchors_relative_entries_and_rejects_escapes() {
        let dir = std::env::temp_dir().join(format!("aoa-self-audit-{}", std::process::id()));
        let repo = dir.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::write(repo.join("README.md"), "anchor").unwrap();
        std::fs::write(dir.join("outside.md"), "outside").unwrap();
        let repo_root = repo.canonicalize().unwrap();

        // A `--repo .`-era entry resolves inside the audited repo, not the cwd.
        let resolved = resolve_in_repo(&repo, &repo_root, Path::new("./README.md")).unwrap();
        assert_eq!(resolved, repo_root.join("README.md"));

        // An entry already carrying the audit-time repo prefix is not doubled.
        let prefixed = repo.join("README.md");
        let resolved = resolve_in_repo(&repo, &repo_root, &prefixed).unwrap();
        assert_eq!(resolved, repo_root.join("README.md"));

        // An entry escaping the repo is refused, not read.
        let err = resolve_in_repo(&repo, &repo_root, &dir.join("outside.md")).unwrap_err();
        assert!(err.to_string().contains("outside the repo"), "{err}");

        std::fs::remove_dir_all(&dir).ok();
    }
}
