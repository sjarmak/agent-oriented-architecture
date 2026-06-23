//! Offline external-outcome corpus → construct-validity wiring (R9c follow-up).
//!
//! The apparatus in [`crate::correlation`] / [`crate::construct`] can flip a
//! metric from Advisory to Gating, but only when fed real `(metric, outcome)`
//! pairs. This module supplies those pairs **offline**, from a synthetic corpus
//! of mined repos, with no clone, no network, and no forge API. It is the bridge
//! the apparatus was waiting on, exercised end-to-end against committed fixtures.
//!
//! The signal is the **revert rate** per repo: of the commits mined from a
//! repo's history, the fraction that a later commit reverts. This is the
//! least-noisy, forge-independent outcome named in the R9c plan. Each repo is
//! ONE observation — the per-task population would blow past
//! [`crate::MAX_EXACT_N`], so the corpus pre-aggregates to a per-repo rate, which
//! lands a 5–10 repo corpus inside the exact-permutation window.
//!
//! [`mine_reverts`] is the real, mechanical git miner written behind the same
//! reducer interface. It parses `git log --grep="This reverts commit <sha>"`
//! output via [`parse_reverted_shas`] — a pure string transform unit-tested
//! against captured log text, never a live clone. A future live session supplies
//! a real clone path and a command runner; nothing here runs git itself.

use serde::{Deserialize, Serialize};

use crate::construct::{
    build_report, CorrelationReport, ExternalOutcome, GatingThresholds, MetricOrientation,
    OutcomeCorrelation, GATING_CANDIDATES,
};
use crate::correlation::{spearman, CorrelationError};

/// One commit mined from a repo's history, with its outcome flags. `reverted`
/// is the primary revert signal; `churn` is the post-merge line-churn proxy
/// carried for the registered churn hypothesis (mined by the same pass, see
/// [`mine_reverts`]) — it is reported by [`Repo::mean_churn`] but does not drive
/// the revert correlation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MinedCommit {
    /// The mined commit SHA (a codeprobe `ground_truth_commit`).
    pub sha: String,
    /// Whether a later commit reverts this one (git's `This reverts commit …`).
    pub reverted: bool,
    /// Post-merge line churn over the oracle files in the window (proxy outcome).
    pub churn: u64,
}

/// One repo's worth of mined outcomes paired with the structure-measure counts
/// `aoa-audit` reports for it. A corpus row; one row = one correlation
/// observation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Repo {
    pub repo_id: String,
    /// The repo's structure-measure counts, keyed by the [`GATING_CANDIDATES`]
    /// metric name (e.g. `navigability_anchor_absence`). The `x` of each pair.
    pub structure_counts: std::collections::BTreeMap<String, u64>,
    /// The commits mined from this repo's history.
    pub mined_commits: Vec<MinedCommit>,
}

impl Repo {
    /// Fraction of mined commits that a later commit reverts. The `y` of each
    /// pair. `None` when no commits were mined (the rate is undefined, not zero).
    pub fn revert_rate(&self) -> Option<f64> {
        let total = self.mined_commits.len();
        if total == 0 {
            return None;
        }
        let reverted = self.mined_commits.iter().filter(|c| c.reverted).count();
        Some(reverted as f64 / total as f64)
    }

    /// Mean post-merge churn over the mined commits. `None` when none were mined.
    /// Carried for the registered churn hypothesis; not used by the revert path.
    pub fn mean_churn(&self) -> Option<f64> {
        let total = self.mined_commits.len();
        if total == 0 {
            return None;
        }
        let sum: u64 = self.mined_commits.iter().map(|c| c.churn).sum();
        Some(sum as f64 / total as f64)
    }
}

/// A synthetic external-outcome corpus: the offline stand-in for a set of cloned,
/// mined repos. Deserialized from a committed JSON fixture.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Corpus {
    pub repos: Vec<Repo>,
}

impl Corpus {
    /// Reduce the corpus to `(structure_count, revert_rate)` observation pairs
    /// for one metric — the input [`spearman`] consumes. ONE pair per repo.
    ///
    /// A repo is skipped only when it cannot contribute a defined observation:
    /// no commits were mined (no revert rate) or the metric is absent from its
    /// structure counts. Mechanical: a count lookup and a fraction, no judgment.
    pub fn observation_pairs(&self, metric: &str) -> Vec<(f64, f64)> {
        self.repos
            .iter()
            .filter_map(|repo| {
                let x = *repo.structure_counts.get(metric)? as f64;
                let y = repo.revert_rate()?;
                Some((x, y))
            })
            .collect()
    }
}

/// Correlate every gating-candidate metric against the corpus's revert rate and
/// build the reproducible construct-validity artifact.
///
/// For each candidate, the per-repo `(structure_count, revert_rate)` pairs are
/// run through [`spearman`]. A well-defined correlation becomes a single
/// [`OutcomeCorrelation`] against [`ExternalOutcome::RevertRate`]; a correlation
/// that cannot be computed (too few repos, or a metric/outcome that never varies
/// across the corpus) yields NO correlation, so the metric honestly stays
/// Advisory rather than being handed a fabricated tie. The classification itself
/// is [`build_report`]'s job under the supplied thresholds.
pub fn build_report_from_corpus(
    data_source: impl Into<String>,
    corpus: &Corpus,
    thresholds: &GatingThresholds,
) -> Result<crate::construct::ConstructValidityReport, CorrelationError> {
    let mut reports = Vec::with_capacity(GATING_CANDIDATES.len());
    for (metric, orientation) in GATING_CANDIDATES {
        reports.push(correlate_metric(metric, *orientation, corpus)?);
    }
    Ok(build_report(data_source, &reports, thresholds))
}

/// Build one metric's [`CorrelationReport`] from the corpus's revert rate.
///
/// `Err` only on errors that signal a real problem with the data the caller must
/// see. A correlation that is simply *not computable* on this corpus —
/// [`CorrelationError::TooFewObservations`] or [`CorrelationError::ZeroVariance`]
/// — is not an error here: it means "no external tie", so the report carries no
/// correlations and the metric stays Advisory.
fn correlate_metric(
    metric: &str,
    orientation: MetricOrientation,
    corpus: &Corpus,
) -> Result<CorrelationReport, CorrelationError> {
    let pairs = corpus.observation_pairs(metric);
    let correlations = match spearman(&pairs) {
        Ok(rc) => vec![OutcomeCorrelation {
            outcome: ExternalOutcome::RevertRate,
            coefficient: rc.coefficient,
            n: rc.n,
            p_value: rc.p_value,
        }],
        Err(CorrelationError::TooFewObservations(_) | CorrelationError::ZeroVariance) => Vec::new(),
        Err(other) => return Err(other),
    };
    Ok(CorrelationReport {
        metric: metric.to_string(),
        orientation,
        correlations,
    })
}

/// Parse the reverted-commit SHAs out of `git log` output of the form emitted by
/// `git log --grep="This reverts commit <sha>" --format=...`.
///
/// Pure mechanical extraction: each `This reverts commit <40-hex>.` line names
/// one SHA that was reverted. The full-40-hex form is git's canonical body line
/// (`git revert` writes it verbatim), so matching it is exact, not a heuristic.
/// Returned in first-seen order with duplicates removed.
pub fn parse_reverted_shas(git_log: &str) -> Vec<String> {
    const MARKER: &str = "This reverts commit ";
    let mut seen = Vec::new();
    for line in git_log.lines() {
        let Some(rest) = line.trim().strip_prefix(MARKER) else {
            continue;
        };
        let sha: String = rest.chars().take_while(|c| c.is_ascii_hexdigit()).collect();
        if sha.len() == 40 && !seen.contains(&sha) {
            seen.push(sha);
        }
    }
    seen
}

/// The exact git command the live miner runs to surface reverts in a clone. Built
/// here (not run) so a future live session and the parser test share one source
/// of truth for the command shape. `dir` is the scratch clone path a live session
/// supplies; this crate never executes it. The format is the commit body alone —
/// the canonical `This reverts commit <sha>` line lives there and is all
/// [`parse_reverted_shas`] reads.
#[must_use]
pub fn revert_log_command(dir: &str) -> Vec<String> {
    vec![
        "git".into(),
        "-C".into(),
        dir.into(),
        "log".into(),
        "--grep=This reverts commit".into(),
        "--format=%b".into(),
    ]
}

/// Runs a command and returns its stdout. The injection point that keeps this
/// module offline: tests pass a closure over captured text; a live session passes
/// a real subprocess runner. Returns the error string on failure so the caller
/// can surface a real failure rather than masking it.
pub type GitRunner<'a> = dyn Fn(&[String]) -> Result<String, String> + 'a;

/// Mine the reverted SHAs from a real clone at `dir`, using the injected `run`.
///
/// This is the live miner, written behind the same boundary the fixtures use but
/// **never executed against a clone in this crate** — every call site here passes
/// a runner over captured text. A future live session supplies a runner that
/// shells out to git; the offline clamp is enforced by never wiring a real
/// subprocess runner into any test or default.
pub fn mine_reverts(dir: &str, run: &GitRunner<'_>) -> Result<Vec<String>, String> {
    let cmd = revert_log_command(dir);
    let stdout = run(&cmd)?;
    Ok(parse_reverted_shas(&stdout))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::construct::MetricMode;

    const CONFIRMING: &str =
        include_str!("../tests/fixtures/external_outcome_corpus_confirming.json");
    const NOISE: &str = include_str!("../tests/fixtures/external_outcome_corpus_noise.json");

    fn corpus(json: &str) -> Corpus {
        serde_json::from_str(json).expect("corpus fixture parses")
    }

    // --- F2: the reducer, against hand-computed pairs ---

    #[test]
    fn revert_rate_is_reverted_over_total() {
        let repo = Repo {
            repo_id: "r".into(),
            structure_counts: Default::default(),
            mined_commits: vec![
                MinedCommit {
                    sha: "a".into(),
                    reverted: true,
                    churn: 1,
                },
                MinedCommit {
                    sha: "b".into(),
                    reverted: false,
                    churn: 2,
                },
                MinedCommit {
                    sha: "c".into(),
                    reverted: true,
                    churn: 3,
                },
                MinedCommit {
                    sha: "d".into(),
                    reverted: false,
                    churn: 4,
                },
            ],
        };
        assert_eq!(repo.revert_rate(), Some(0.5)); // 2 of 4
        assert_eq!(repo.mean_churn(), Some(2.5)); // (1+2+3+4)/4
    }

    #[test]
    fn empty_repo_has_no_rate() {
        let repo = Repo {
            repo_id: "r".into(),
            structure_counts: Default::default(),
            mined_commits: Vec::new(),
        };
        assert_eq!(repo.revert_rate(), None);
        assert_eq!(repo.mean_churn(), None);
    }

    #[test]
    fn observation_pairs_match_hand_computed_values() {
        // The confirming fixture is engineered so navigability_anchor_absence
        // (structure count) rises in lockstep with the revert rate.
        let c = corpus(CONFIRMING);
        let pairs = c.observation_pairs("navigability_anchor_absence");
        assert_eq!(
            pairs,
            vec![
                (0.0, 0.0),  // 0 anchor-absences, 0/4 reverted
                (1.0, 0.2),  // 1, 1/5 reverted
                (2.0, 0.4),  // 2, 2/5 reverted
                (3.0, 0.5),  // 3, 2/4 reverted
                (4.0, 0.75), // 4, 3/4 reverted
                (5.0, 1.0),  // 5, 4/4 reverted
            ]
        );
    }

    #[test]
    fn observation_pairs_skip_repos_missing_the_metric() {
        // The skip branch fires only when a repo lacks the metric in its
        // structure_counts. Build a corpus where exactly one of three repos is
        // missing "module_size_outliers" — and that repo still has a defined
        // revert rate, so the drop is attributable to the absent metric alone,
        // not to a missing outcome.
        let with_metric = |id: &str, count: u64| Repo {
            repo_id: id.into(),
            structure_counts: std::collections::BTreeMap::from([(
                "module_size_outliers".to_string(),
                count,
            )]),
            mined_commits: vec![MinedCommit {
                sha: "x".into(),
                reverted: true,
                churn: 1,
            }],
        };
        let without_metric = Repo {
            repo_id: "missing".into(),
            structure_counts: std::collections::BTreeMap::new(),
            mined_commits: vec![MinedCommit {
                sha: "y".into(),
                reverted: false, // defined revert rate (0.0), so the metric is the only reason to skip
                churn: 1,
            }],
        };
        let c = Corpus {
            repos: vec![with_metric("r1", 1), without_metric, with_metric("r2", 2)],
        };
        let pairs = c.observation_pairs("module_size_outliers");
        // The metric-less middle repo is dropped: 2 pairs from 3 repos, in order.
        assert!(
            pairs.len() < c.repos.len(),
            "skip branch must drop the metric-less repo"
        );
        assert_eq!(pairs, vec![(1.0, 1.0), (2.0, 1.0)]);
    }

    // --- F3: end-to-end wiring, known-answer ---

    #[test]
    fn confirming_corpus_flips_navigability_to_gating() {
        let c = corpus(CONFIRMING);
        let report = build_report_from_corpus(
            "fixture: external_outcome_corpus_confirming.json",
            &c,
            &GatingThresholds::default(),
        )
        .expect("well-defined corpus");

        let nav = report
            .metrics
            .iter()
            .find(|m| m.metric == "navigability_anchor_absence")
            .expect("candidate present");
        // Perfect positive monotone over 6 repos: rho = 1.0, p = 2/720 < 0.05,
        // |rho| >= 0.3, n = 6 >= 5. LowerIsBetter metric + RevertRate (a harm)
        // means a confirming tie is POSITIVE, which is exactly what we built.
        assert_eq!(
            nav.mode,
            MetricMode::Gating,
            "confirming corpus must gate it"
        );
        let corr = &nav.correlations[0];
        assert_eq!(corr.outcome, ExternalOutcome::RevertRate);
        assert!(corr.coefficient > 0.9, "rho={}", corr.coefficient);
        assert!(corr.p_value <= 0.05, "p={}", corr.p_value);
        assert_eq!(corr.n, 6);
    }

    #[test]
    fn noise_corpus_keeps_every_metric_advisory() {
        let c = corpus(NOISE);
        let report = build_report_from_corpus(
            "fixture: external_outcome_corpus_noise.json",
            &c,
            &GatingThresholds::default(),
        )
        .expect("well-defined corpus");
        for m in &report.metrics {
            assert_eq!(
                m.mode,
                MetricMode::Advisory,
                "{} must stay advisory on noise",
                m.metric
            );
        }
    }

    #[test]
    fn confirming_artifact_round_trips_through_serde() {
        let c = corpus(CONFIRMING);
        let report = build_report_from_corpus("fixture", &c, &GatingThresholds::default())
            .expect("well-defined");
        let json = serde_json::to_string(&report).expect("serializes");
        let back: crate::construct::ConstructValidityReport =
            serde_json::from_str(&json).expect("round-trips");
        assert_eq!(back, report);
    }

    /// The committed artifact is the reproducible deliverable: re-running the
    /// pipeline over the confirming corpus reproduces it exactly. Mirrors the
    /// `criterion_6_artifact_reproduces` discipline on the no-data determination.
    const REPORT_ARTIFACT: &str =
        include_str!("../tests/fixtures/external_outcome_corpus_report.json");

    #[test]
    fn confirming_artifact_reproduces_committed_fixture() {
        let c = corpus(CONFIRMING);
        let report = build_report_from_corpus(
            "fixture: external_outcome_corpus_confirming.json (offline synthetic corpus; \
revert rate per repo over mined commits, paired with aoa-audit structure counts)",
            &c,
            &GatingThresholds::default(),
        )
        .expect("well-defined");
        let from_disk: crate::construct::ConstructValidityReport =
            serde_json::from_str(REPORT_ARTIFACT).expect("artifact parses");
        assert_eq!(
            from_disk, report,
            "committed artifact must match the pipeline output"
        );
    }

    // --- F4: the real miner's parser, against captured git-log text ---

    #[test]
    fn parse_reverted_shas_extracts_canonical_revert_lines() {
        let log = "\
deadbeefdeadbeefdeadbeefdeadbeefdeadbeef
Revert \"add widget\"

This reverts commit 1111111111111111111111111111111111111111.
cafebabecafebabecafebabecafebabecafebabe
Some unrelated body mentioning reverts loosely
This reverts commit 2222222222222222222222222222222222222222 (cherry-picked).
";
        assert_eq!(
            parse_reverted_shas(log),
            vec![
                "1111111111111111111111111111111111111111".to_string(),
                "2222222222222222222222222222222222222222".to_string(),
            ]
        );
    }

    #[test]
    fn parse_reverted_shas_ignores_non_canonical_and_dedupes() {
        let log = "\
This reverts commit short.
This reverts commit 3333333333333333333333333333333333333333.
This reverts commit 3333333333333333333333333333333333333333.
prose: this reverts commit nothing in particular
";
        assert_eq!(
            parse_reverted_shas(log),
            vec!["3333333333333333333333333333333333333333".to_string()]
        );
    }

    #[test]
    fn revert_log_command_shape_is_stable() {
        // The miner and a future live session share this command shape.
        let cmd = revert_log_command("/scratch/clone");
        assert_eq!(cmd[0], "git");
        assert_eq!(cmd[1], "-C");
        assert_eq!(cmd[2], "/scratch/clone");
        assert!(cmd.iter().any(|a| a.contains("This reverts commit")));
    }

    #[test]
    fn mine_reverts_uses_injected_runner_over_captured_text() {
        // OFFLINE: the runner returns captured log text; no git is executed.
        let captured = "This reverts commit \
4444444444444444444444444444444444444444.\n"
            .to_string();
        let run = |_cmd: &[String]| -> Result<String, String> { Ok(captured.clone()) };
        let reverted = mine_reverts("/scratch/clone", &run).expect("runner ok");
        assert_eq!(
            reverted,
            vec!["4444444444444444444444444444444444444444".to_string()]
        );
    }

    #[test]
    fn mine_reverts_propagates_runner_failure() {
        let run = |_cmd: &[String]| -> Result<String, String> { Err("git not found".into()) };
        assert_eq!(
            mine_reverts("/scratch/clone", &run),
            Err("git not found".to_string())
        );
    }
}
