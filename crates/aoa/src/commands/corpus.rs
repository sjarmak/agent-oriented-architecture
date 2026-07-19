//! Live external-outcome corpus driver (aoa-x0a.4).
//!
//! The offline apparatus in `aoa_gap::outcome` correlates AOA structure measures
//! against a per-repo revert rate, but only when handed a real [`Corpus`]. That
//! crate is deliberately offline — it never clones, never shells git, and wires
//! no real subprocess runner into any default. This module is the app-layer
//! driver that supplies the missing live half: it runs the real `aoa-audit` over
//! a cloned repo for the structure counts, shells git for the revert signal, and
//! assembles the [`Corpus`] the offline builder consumes. The offline clamp holds
//! because everything that touches the network or a subprocess lives here, in the
//! binary, never back in `aoa-gap`.
//!
//! [`structure_counts`] reduces a real audit run to the `(metric → count)` map a
//! corpus [`Repo`] carries; [`mine_corpus`] is the `aoa gap mine-corpus` handler
//! that discovers codeprobe tasks, mines the operator's local clones for the
//! revert signal, and writes the reproducible construct-validity report. Cloning
//! is an operator step (documented, network); this command reads only local
//! clones, so the whole pipeline is exercised in tests with an injected runner.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{anyhow, bail, Context, Result};

use aoa_audit::{structure_measurements, AuditConfig, AuditError, FindingKind, StructureMeasure};
use aoa_bench::{is_task_dir, load_task};
use aoa_gap::{
    build_report_from_corpus, mine_reverts, Corpus, GatingThresholds, GitRunner, MetricName,
    MinedCommit, Repo,
};

use crate::cli::MineCorpusArgs;
use crate::commands::fsutil::MAX_TASK_DIRS;
use crate::commands::git;
use crate::output::{print_human, print_json};

/// The audit structure measures that are orientation-safe corpus inputs.
///
/// Each returned metric is a `LowerIsBetter` "count of a bad thing" (an absence,
/// an outlier count, a proxy count) whose magnitude pairs sign-consistently with
/// a harm outcome like the revert rate — the property the R9c correlation relies
/// on. The mapping mirrors `aoa_recommend`'s private `join`, but **narrowed on
/// purpose**:
///
/// * `VerificationReachability` and `InvariantDiscoverability` carry no
///   gating-candidate metric (`join` maps both to `None`); the trace-derived
///   `invariant_discoverability` candidate is a different, `HigherIsBetter`
///   signal, not this absence count.
/// * `ContextBudget` and `MutationSurface` *are* joined by `recommend` (to
///   `budget_adherence` / `mutation_surface`), but `recommend` reads only a
///   metric's mode, never its orientation. The corpus correlation is
///   orientation-critical, and a `ContextBudget` overflow count against the
///   `HigherIsBetter` `budget_adherence` candidate would invert the sign. Those
///   metrics come from the trace, not a static repo audit, so they are excluded
///   here rather than fed in backwards.
///
/// The match stays exhaustive: adding a `FindingKind` forces the orientation
/// decision here (compile error) instead of silently dropping or mis-signing a
/// new measure. The metric *name* itself is not retyped — it is sourced from
/// the canonical [`FindingKind::metric_name`] map, so a rename lands in one
/// place. This function contributes only the corpus-specific include/exclude
/// *policy* on top of that map. The typed [`MetricName`] return makes every
/// `Some` name a registered gating candidate by construction.
fn structure_metric(kind: FindingKind) -> Option<MetricName> {
    match kind {
        // Orientation-safe structure measures: the canonical metric name.
        FindingKind::NavigabilityAnchor
        | FindingKind::ModuleSizeOutlier
        | FindingKind::UnusedImportProxy
        | FindingKind::BuildDeterminism
        | FindingKind::DevEnvironmentDeclaration
        | FindingKind::TaskDiscoverySurface
        | FindingKind::GeneratedArtifactProtection
        | FindingKind::WriteSafetyZone => kind.metric_name(),
        // Orientation-unsafe or non-structure — see the doc comment.
        FindingKind::ContextBudget
        | FindingKind::MutationSurface
        | FindingKind::MissingPlane
        | FindingKind::VerificationReachability
        | FindingKind::InvariantDiscoverability => None,
    }
}

/// Reduce a real structure measurement over `repo` to the corpus structure-count
/// map: the `(gating-candidate name → measured count)` pairs one [`aoa_gap::Repo`]
/// carries as its `x` values.
///
/// Uses [`aoa_audit::structure_measurements`], not the punch list, so a *clean*
/// repo contributes a real `0` (a repo that pins its build reports
/// `build_determinism_absence = 0`) rather than being dropped. That is what lets a
/// measure vary across a corpus and earn a confirming correlation. A metric is
/// omitted only when its probe is genuinely [`StructureMeasure::Unmeasurable`] on
/// this repo (too few files for a module-size median, no Rust sources for the
/// unused-import proxy); `observation_pairs` then drops that repo for that metric,
/// honestly recording no data rather than a fabricated `0`.
pub fn structure_counts(repo: &Path) -> Result<BTreeMap<String, u64>, AuditError> {
    let measures = structure_measurements(repo, AuditConfig::default().size_outlier_k)?;
    let mut counts = BTreeMap::new();
    for (kind, measure) in measures {
        if let (Some(name), StructureMeasure::Measured(value)) = (structure_metric(kind), measure) {
            counts.insert(name.to_string(), value);
        }
    }
    Ok(counts)
}

/// One repo's mining inputs: the operator's local clone and the distinct
/// codeprobe `ground_truth_commit`s mined from it. One `RepoInput` becomes one
/// corpus [`Repo`] — one correlation observation.
struct RepoInput {
    repo_id: String,
    clone: PathBuf,
    commits: Vec<String>,
}

/// Execute a git command line and return its stdout. The concrete [`GitRunner`]
/// the live driver injects into the otherwise-offline [`mine_reverts`]: it shells
/// out to a *local* clone (no network — the clone is an operator step). Non-zero
/// exit and spawn failure both surface as the runner's opaque `Err(String)`,
/// which `mine_reverts` wraps into a typed error at the boundary.
fn run_git(cmd: &[String]) -> std::result::Result<String, String> {
    let (prog, args) = cmd
        .split_first()
        .ok_or_else(|| "empty git command".to_string())?;
    let mut command = Command::new(prog);
    command.args(args);
    // Strict decode: git output here (revert logs, OIDs) must be valid UTF-8;
    // garbage is a real error, not something to paper over lossily.
    let stdout = git::checked(command, &cmd.join(" "))?;
    String::from_utf8(stdout).map_err(|e| format!("git output was not UTF-8: {e}"))
}

/// Resolve `sha` to the canonical full 40-hex OID of a commit in `clone`,
/// validating presence (NET-2) in the same step.
///
/// Canonicalization is load-bearing, not cosmetic: a `ground_truth_commit` may be
/// abbreviated or upper-case, but `parse_reverted_shas` only ever returns git's
/// canonical lower-case 40-hex form, and revert matching is an exact-string set
/// membership. Comparing a raw abbreviated/upper-case SHA against that set would
/// silently never match, counting a genuinely-reverted commit as not-reverted and
/// biasing the revert rate down. Resolving here forces both sides into the same
/// form. `--verify … ^{commit}` also fails loud on a shallow/stale clone that
/// lacks the commit, so the operator re-clones with full history.
fn resolve_commit(clone: &Path, sha: &str) -> Result<String> {
    let mut command = Command::new("git");
    command.arg("-C").arg(clone).args([
        "rev-parse",
        "--verify",
        "--quiet",
        &format!("{sha}^{{commit}}"),
    ]);
    // `spawn`, not `checked`: `--verify --quiet` exits non-zero with EMPTY
    // stderr when the ref is absent, so a generic stderr-folding error would be
    // blank. Here that non-zero exit is a domain signal — the commit is not in
    // the clone — with its own operator guidance below.
    let output = git::spawn(command, &format!("git rev-parse in {}", clone.display()))
        .map_err(|e| anyhow!(e))?;
    if !output.status.success() {
        bail!(
            "ground_truth_commit {sha} is not present in clone {} — clone with full \
history (not --depth) before mining",
            clone.display()
        );
    }
    let oid = String::from_utf8(output.stdout)
        .context("git rev-parse output was not UTF-8")?
        .trim()
        .to_string();
    if oid.len() != 40 || !oid.bytes().all(|b| b.is_ascii_hexdigit()) {
        bail!(
            "git rev-parse resolved {sha:?} to an unexpected OID {oid:?} in {}",
            clone.display()
        );
    }
    Ok(oid)
}

/// Reject a `repo_id` that is not a single safe path segment before it is joined
/// onto the clones directory. `repo_id` is `task.repo`, read verbatim from
/// operator-supplied `metadata.json`/`task.toml`; a value like `../../etc` or an
/// absolute path would, through `Path::join`, point the git subprocess at a
/// directory outside the clones tree.
fn validate_repo_id(repo_id: &str) -> Result<()> {
    if repo_id.is_empty()
        || repo_id == "."
        || repo_id == ".."
        || repo_id.contains('/')
        || repo_id.contains('\\')
    {
        bail!("unsafe repo id {repo_id:?}: expected a single path segment (from codeprobe `repo`)");
    }
    Ok(())
}

/// Recursively collect the task dirs under `root`: every directory `load_task`
/// can read (one carrying `metadata.json` or `task.toml`). A task dir is a leaf —
/// discovery does not descend into it. Results are sorted for a deterministic,
/// reproducible corpus order.
fn discover_task_dirs(root: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    discover_into(root, &mut out)?;
    out.sort();
    Ok(out)
}

fn discover_into(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    // Same leaf test `load_task` guards on — shared so discovery and loading
    // cannot drift on what counts as a task dir.
    if is_task_dir(dir) {
        if out.len() >= MAX_TASK_DIRS {
            bail!("task tree exceeds the {MAX_TASK_DIRS} task-dir cap");
        }
        out.push(dir.to_path_buf());
        return Ok(());
    }
    let entries =
        std::fs::read_dir(dir).with_context(|| format!("reading task tree {}", dir.display()))?;
    for entry in entries {
        let entry = entry.with_context(|| format!("reading an entry under {}", dir.display()))?;
        // `DirEntry::file_type` does NOT follow symlinks: a symlinked directory
        // (or a cycle) inside an operator-supplied task tree cannot pull the walk
        // out of the tree or recurse forever — the whole-repo convention.
        let file_type = entry
            .file_type()
            .with_context(|| format!("stat-ing {}", entry.path().display()))?;
        if file_type.is_dir() {
            discover_into(&entry.path(), out)?;
        }
    }
    Ok(())
}

/// Assemble the corpus from per-repo inputs and an injected git runner. The
/// offline-testable core: `run` is a real subprocess in the live path and a
/// closure over captured text in tests, so the mining logic is exercised without
/// a clone. Each mined commit is flagged `reverted` when a later commit in the
/// clone reverts it; churn is carried as `0` — this pass correlates the revert
/// rate only (churn is a secondary, non-gating proxy the report never reads).
///
/// **Sequential by design.** The repos are independent (one `mine_reverts` git
/// subprocess plus one filesystem-only `structure_counts` audit each) and could
/// run concurrently, but this stays sequential deliberately: a corpus is a
/// handful of operator-supplied local clones, and the per-repo cost is a single
/// `git log` over one clone plus one audit pass — well under the tens-of-seconds
/// total where parallelism would pay for its complexity. Parallelizing would
/// require a `Send + Sync` bound on [`GitRunner`] (today `dyn Fn(..) + 'a`, so
/// the closure that captures a `Command` builder is neither) and a threaded
/// assembler; that is the exact lever to pull if a future corpus grows large
/// enough to measure a real wall-clock win. Until then, YAGNI.
fn build_corpus(inputs: &[RepoInput], run: &GitRunner<'_>) -> Result<Corpus> {
    let mut repos = Vec::with_capacity(inputs.len());
    for input in inputs {
        let reverted = mine_reverts(&input.clone, run)
            .map_err(anyhow::Error::from)
            .with_context(|| format!("mining reverts for {}", input.repo_id))?;
        let reverted: BTreeSet<&str> = reverted.iter().map(String::as_str).collect();
        let structure_counts = structure_counts(&input.clone)
            .with_context(|| format!("auditing structure of {}", input.repo_id))?;
        let mined_commits = input
            .commits
            .iter()
            .map(|sha| MinedCommit {
                sha: sha.clone(),
                reverted: reverted.contains(sha.as_str()),
                churn: 0,
            })
            .collect();
        repos.push(Repo {
            repo_id: input.repo_id.clone(),
            structure_counts,
            mined_commits,
            checkbox_baseline: None,
        });
    }
    Ok(Corpus { repos })
}

/// `aoa gap mine-corpus`: build the live external-outcome corpus from codeprobe
/// tasks and operator-supplied clones, then write the reproducible R9c
/// CorrelationReport. Outcome is DATA — every metric honestly staying Advisory
/// (no confirming correlation in the real data) is a valid result, not a failure.
pub fn mine_corpus(args: &MineCorpusArgs, json: bool) -> Result<i32> {
    // 1. Discover + load tasks; group distinct ground_truth_commits by repo.
    let task_dirs = discover_task_dirs(&args.tasks)?;
    let mut by_repo: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut skipped_no_commit = 0usize;
    for dir in &task_dirs {
        let task = load_task(dir).with_context(|| format!("loading task {}", dir.display()))?;
        match task.ground_truth_commit {
            Some(sha) => {
                by_repo.entry(task.repo).or_default().insert(sha);
            }
            None => skipped_no_commit += 1,
        }
    }
    if by_repo.is_empty() {
        bail!(
            "no tasks with a ground_truth_commit found under {} ({} task dirs scanned)",
            args.tasks.display(),
            task_dirs.len()
        );
    }

    // 2. Resolve each repo's clone; canonicalize + validate every SHA against it.
    let mut inputs = Vec::with_capacity(by_repo.len());
    for (repo_id, raw_commits) in by_repo {
        validate_repo_id(&repo_id)?;
        let clone = args.clones.join(&repo_id);
        if !clone.is_dir() {
            bail!(
                "clone for repo `{repo_id}` not found at {} — clone it (full history) \
before mining",
                clone.display()
            );
        }
        // Resolve to canonical OIDs and dedup: two raw SHAs (e.g. abbreviated and
        // full) can name one commit, which must count as one mined observation.
        let mut commits: BTreeSet<String> = BTreeSet::new();
        for sha in &raw_commits {
            commits.insert(resolve_commit(&clone, sha)?);
        }
        inputs.push(RepoInput {
            repo_id,
            clone,
            commits: commits.into_iter().collect(),
        });
    }

    // 3. Assemble the corpus with the real (local) git runner.
    let runner = |cmd: &[String]| run_git(cmd);
    let corpus = build_corpus(&inputs, &runner)?;

    // 4. Correlate into the reproducible construct-validity report.
    let data_source = format!(
        "live codeprobe corpus: {} repos mined from {} ({} task dirs, {} without a \
ground_truth_commit skipped)",
        inputs.len(),
        args.tasks.display(),
        task_dirs.len(),
        skipped_no_commit
    );
    let report = build_report_from_corpus(data_source, &corpus, &GatingThresholds::default())
        .map_err(|e| anyhow!("building corpus report: {e}"))?;

    // 5. Write the artifact.
    let serialized = serde_json::to_string_pretty(&report).context("serializing corpus report")?;
    std::fs::write(&args.out, format!("{serialized}\n"))
        .with_context(|| format!("writing artifact to {}", args.out.display()))?;

    // 6. Report.
    if json {
        print_json(&report)?;
    } else {
        let mut s = String::new();
        let _ = writeln!(
            s,
            "corpus: {} repos, artifact -> {}",
            corpus.repos.len(),
            args.out.display()
        );
        for m in &report.construct.metrics {
            let n = m.correlations.first().map_or(0, |c| c.n);
            let _ = writeln!(s, "  {:<42} {:?} (n={n})", m.metric, m.mode);
        }
        print_human(&s);
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use aoa_gap::GATING_CANDIDATES;

    fn is_gating_candidate(name: &str) -> bool {
        GATING_CANDIDATES.iter().any(|(n, _)| n.as_str() == name)
    }

    #[test]
    fn orientation_unsafe_and_metricless_kinds_are_excluded() {
        // The two kinds recommend joins but whose orientation would flip the
        // corpus sign, plus the three that carry no corpus metric at all.
        for kind in [
            FindingKind::ContextBudget,
            FindingKind::MutationSurface,
            FindingKind::MissingPlane,
            FindingKind::VerificationReachability,
            FindingKind::InvariantDiscoverability,
        ] {
            assert_eq!(structure_metric(kind), None, "{kind:?} must be excluded");
        }
    }

    #[test]
    fn structure_counts_on_a_bare_repo_stays_within_candidates() {
        // A real audit run over a throwaway tree: whatever measures fire, every
        // key must be a registered candidate and no excluded name may leak in.
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("main.py"), "print('hi')\n").expect("write source");

        let counts = structure_counts(dir.path()).expect("audit runs on a bare tree");

        for name in counts.keys() {
            assert!(
                is_gating_candidate(name),
                "{name:?} is not a registered gating candidate"
            );
        }
        for excluded in [
            "invariant_discoverability",
            "budget_adherence",
            "mutation_surface",
        ] {
            assert!(
                !counts.contains_key(excluded),
                "excluded metric {excluded:?} leaked into the structure counts"
            );
        }
    }

    #[test]
    fn structure_counts_records_a_zero_for_a_clean_measure() {
        // A repo that pins its build surfaces build_determinism_absence = 0 — the
        // (0, y) corpus anchor — rather than dropping the metric (aoa-3a7).
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("Cargo.lock"), "").expect("lockfile");
        std::fs::write(dir.path().join("main.py"), "print('hi')\n").expect("source");

        let counts = structure_counts(dir.path()).expect("measurement runs");
        assert_eq!(
            counts.get("build_determinism_absence"),
            Some(&0),
            "a pinned build is a measured 0, not a dropped metric"
        );
    }

    #[test]
    fn build_corpus_flags_reverted_commits_and_audits_structure() {
        // OFFLINE: a fake runner returns captured revert-log text; no git runs.
        // A real audit still runs over the tiny clone tree for the structure
        // counts — that is filesystem-only, no network, no subprocess.
        let dir = tempfile::tempdir().expect("tempdir");
        let clone = dir.path().join("acme");
        std::fs::create_dir_all(&clone).expect("clone dir");
        std::fs::write(clone.join("main.py"), "print('hi')\n").expect("source");

        let reverted_sha = "1111111111111111111111111111111111111111";
        let kept_sha = "2222222222222222222222222222222222222222";
        let captured = format!("This reverts commit {reverted_sha}.\n");
        let run = |_cmd: &[String]| -> std::result::Result<String, String> { Ok(captured.clone()) };

        let inputs = vec![RepoInput {
            repo_id: "acme".into(),
            clone: clone.clone(),
            commits: vec![kept_sha.into(), reverted_sha.into()],
        }];
        let corpus = build_corpus(&inputs, &run).expect("corpus assembles");

        assert_eq!(corpus.repos.len(), 1);
        let repo = &corpus.repos[0];
        // One of two mined commits was reverted -> revert rate 0.5.
        assert_eq!(repo.revert_rate(), Some(0.5));
        let reverted_flag = |sha: &str| {
            repo.mined_commits
                .iter()
                .find(|c| c.sha == sha)
                .unwrap_or_else(|| panic!("{sha} mined"))
                .reverted
        };
        assert!(reverted_flag(reverted_sha), "the reverted sha is flagged");
        assert!(!reverted_flag(kept_sha), "the kept sha is not flagged");
        for name in repo.structure_counts.keys() {
            assert!(is_gating_candidate(name), "{name} is a candidate");
        }
    }

    #[test]
    fn discover_task_dirs_finds_task_leaves_without_descending_into_them() {
        let root = tempfile::tempdir().expect("tempdir");
        // A nested metadata.json task (codeprobe's .codeprobe/tasks/<t> shape).
        let meta_task = root.path().join("requests/seed1/.codeprobe/tasks/t1");
        std::fs::create_dir_all(&meta_task).expect("meta task dir");
        std::fs::write(meta_task.join("metadata.json"), "{}").expect("metadata");
        // A shallow task.toml task.
        let toml_task = root.path().join("rich/t2");
        std::fs::create_dir_all(&toml_task).expect("toml task dir");
        std::fs::write(toml_task.join("task.toml"), "").expect("task.toml");
        // A non-task dir with no manifest must not appear.
        std::fs::create_dir_all(root.path().join("empty/nested")).expect("empty dir");

        let found = discover_task_dirs(root.path()).expect("discovery");
        assert!(found.contains(&meta_task), "metadata.json task found");
        assert!(found.contains(&toml_task), "task.toml task found");
        assert_eq!(
            found.len(),
            2,
            "only the two task leaves, nothing else: {found:?}"
        );
    }

    #[test]
    fn validate_repo_id_rejects_traversal_and_accepts_a_plain_segment() {
        assert!(validate_repo_id("requests").is_ok());
        assert!(validate_repo_id("sqlparse").is_ok());
        for bad in ["", ".", "..", "../etc", "a/b", "/etc/passwd", "a\\b"] {
            assert!(
                validate_repo_id(bad).is_err(),
                "{bad:?} must be rejected as unsafe"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn discover_task_dirs_does_not_follow_symlinked_dirs() {
        // A symlink cycle inside the task tree must neither recurse forever nor
        // pull the walk out of the tree: file_type() does not follow symlinks.
        let root = tempfile::tempdir().expect("tempdir");
        let task = root.path().join("real/t1");
        std::fs::create_dir_all(&task).expect("task dir");
        std::fs::write(task.join("metadata.json"), "{}").expect("metadata");
        // A directory-cycle symlink and a symlink to an out-of-tree dir.
        std::os::unix::fs::symlink(root.path(), root.path().join("real/loop"))
            .expect("cycle symlink");
        let outside = tempfile::tempdir().expect("outside tempdir");
        std::fs::create_dir_all(outside.path().join("secret")).expect("outside task");
        std::fs::write(outside.path().join("secret/metadata.json"), "{}").expect("outside meta");
        std::os::unix::fs::symlink(outside.path(), root.path().join("escape"))
            .expect("escape symlink");

        let found = discover_task_dirs(root.path()).expect("discovery terminates");
        assert_eq!(
            found,
            vec![task],
            "only the in-tree task, no cycle, no escape"
        );
    }
}
