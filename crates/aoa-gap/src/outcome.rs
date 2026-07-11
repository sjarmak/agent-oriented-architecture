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
//!
//! The Factory checkbox-baseline column (aoa-d6t.27) rides the same corpus:
//! each [`Repo`] optionally carries the [`CheckboxBaseline`] scored for it, and
//! [`build_report_from_corpus`] correlates the achieved Factory level against
//! the same per-repo outcome, side by side with the gating candidates — never
//! as one of them. A checkbox score cannot gate anything, and a null
//! correlation for the column is a reportable study result, not a failure.

use std::collections::HashSet;
use std::path::Path;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::checkbox_baseline::CheckboxBaseline;
use crate::construct::{
    build_report, ConstructValidityReport, CorrelationReport, ExternalOutcome, GatingThresholds,
    MetricOrientation, OutcomeCorrelation, GATING_CANDIDATES,
};
use crate::correlation::{spearman, CorrelationError};
use crate::error::GapError;

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
/// observation. (`PartialEq` only: the baseline column carries `f64` pass
/// fractions.)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Repo {
    pub repo_id: String,
    /// The repo's structure-measure counts, keyed by the [`GATING_CANDIDATES`]
    /// metric name (e.g. `navigability_anchor_absence`). The `x` of each pair.
    pub structure_counts: std::collections::BTreeMap<String, u64>,
    /// The commits mined from this repo's history.
    pub mined_commits: Vec<MinedCommit>,
    /// The Factory checkbox-baseline feature column, produced for this repo by
    /// [`crate::score_repo`] at corpus-assembly time and carried as-is (level,
    /// per-pillar fractions, pinned criteria version). `None` when the repo was
    /// never scored — it then contributes no baseline observation, the same
    /// skip discipline as a metric absent from `structure_counts`.
    #[serde(default)]
    pub checkbox_baseline: Option<CheckboxBaseline>,
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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

    /// Reduce the corpus to `(factory_level, revert_rate)` observation pairs —
    /// the checkbox-baseline column against the same outcome the AOA metrics
    /// correlate with, so the two correlations are side-by-side comparable.
    /// Same skip discipline as [`Corpus::observation_pairs`]: a repo without a
    /// scored baseline or without a defined revert rate contributes nothing.
    pub fn checkbox_level_pairs(&self) -> Vec<(f64, f64)> {
        self.repos
            .iter()
            .filter_map(|repo| {
                let x = f64::from(repo.checkbox_baseline.as_ref()?.level);
                let y = repo.revert_rate()?;
                Some((x, y))
            })
            .collect()
    }
}

/// A gating-candidate metric's correlation could not be computed against the
/// corpus, and the failure is a real data problem the caller must fix — not a
/// benign "no external tie". Names the offending metric so the culprit corpus
/// row is obvious; the underlying [`CorrelationError`] (which carries the `n`,
/// cap, or non-finite value) is preserved as the [`std::error::Error::source`].
///
/// Only the *hard* correlation errors reach here: [`CorrelationError::SampleTooLarge`]
/// (the corpus carries more than [`crate::MAX_EXACT_N`] repos for this metric —
/// a precondition the 5–10-repo corpus is designed to satisfy) and
/// [`CorrelationError::NonFiniteObservation`]. The "not computable on this
/// corpus" outcomes ([`CorrelationError::TooFewObservations`],
/// [`CorrelationError::ZeroVariance`]) are absorbed upstream in
/// [`revert_correlations`] and never surface as this error.
#[derive(Debug, Clone, PartialEq, Error)]
#[error("metric {metric}: {source}")]
pub struct CorpusMetricError {
    /// The [`GATING_CANDIDATES`] metric name whose correlation failed.
    pub metric: String,
    /// The underlying rank-correlation error, carried verbatim. thiserror
    /// treats a field named `source` as the error source automatically.
    pub source: CorrelationError,
}

/// Why [`build_report_from_corpus`] refused to build the side-by-side report.
/// The all-or-nothing contract of [`CorpusMetricError`] extends to the
/// checkbox-baseline column: a corpus whose column is broken yields no report,
/// with the refusal naming what is broken.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum CorpusJoinError {
    /// A gating-candidate metric hit a hard correlation error; see
    /// [`CorpusMetricError`] for which errors are hard vs absorbed.
    #[error(transparent)]
    Metric(#[from] CorpusMetricError),
    /// The checkbox-baseline column hit a hard correlation error (the same
    /// refuse set as the metrics: an oversized sample or a non-finite
    /// observation).
    #[error("checkbox baseline column: {0}")]
    Baseline(CorrelationError),
    /// Scored repos disagree on the criteria-table version. Levels from
    /// different tables are not one comparable feature column; correlating
    /// across them would fabricate a joint observation set. Carries every
    /// distinct version found (sorted) so the offending rows are findable.
    #[error("checkbox baseline criteria versions differ across the corpus: {0:?}")]
    MixedCriteriaVersions(Vec<String>),
    /// A repo carries a baseline scored for a different repo — a corpus-
    /// assembly bug that would silently misattribute the feature column.
    #[error("repo `{repo_id}` carries a checkbox baseline scored for `{baseline_repo_id}`")]
    BaselineRepoMismatch {
        repo_id: String,
        baseline_repo_id: String,
    },
}

/// The checkbox-baseline column correlated against the corpus outcome: the
/// pinned criteria version every scored repo shares, plus the level-vs-outcome
/// correlation. `correlations` is empty when the column is carried but no
/// correlation is computable on this corpus (fewer than three scored repos, or
/// a level that never varies) — honestly "no tie", never a fabricated one.
///
/// Deliberately NOT a [`CorrelationReport`]: the baseline is a study feature
/// column, not a gating candidate. It carries no orientation and is never
/// classified into a [`crate::MetricMode`], so a checkbox score cannot gate a
/// decision (see `crate::checkbox_baseline`) — a null result here IS the R0
/// headline.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CheckboxBaselineCorrelation {
    pub criteria_version: String,
    pub correlations: Vec<OutcomeCorrelation>,
}

/// The side-by-side study artifact built from one corpus: the construct-
/// validity report over the AOA gating candidates, joined with the Factory
/// checkbox-baseline column correlated against the same external outcome.
/// `checkbox_baseline` is `None` when no repo in the corpus carries the column
/// (there is then no criteria version to report, and no column to correlate).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CorpusReport {
    pub construct: ConstructValidityReport,
    pub checkbox_baseline: Option<CheckboxBaselineCorrelation>,
}

/// Correlate every gating-candidate metric AND the checkbox-baseline column
/// against the corpus's revert rate, building the reproducible side-by-side
/// study artifact: the construct-validity report plus the Factory baseline's
/// correlation against the same outcome.
///
/// For each candidate, the per-repo `(structure_count, revert_rate)` pairs are
/// run through [`spearman`]. A well-defined correlation becomes a single
/// [`OutcomeCorrelation`] against [`ExternalOutcome::RevertRate`]; a correlation
/// that cannot be computed (too few repos, or a metric/outcome that never varies
/// across the corpus) yields NO correlation, so the metric honestly stays
/// Advisory rather than being handed a fabricated tie. The classification itself
/// is [`build_report`]'s job under the supplied thresholds. The checkbox column
/// gets the identical treatment over its `(factory_level, revert_rate)` pairs —
/// minus the classification, which it must never receive.
///
/// **Contract — all-or-nothing, but diagnosable.** If any metric hits a *hard*
/// data error (its sample exceeds [`crate::MAX_EXACT_N`], or an observation is
/// non-finite), the whole report is refused: the caller gets no partial report,
/// which keeps "these metrics were validated together against this corpus"
/// honest. The refusal names the offending metric via [`CorpusMetricError`] so
/// the culprit corpus row is diagnosable rather than an anonymous "sample size
/// N exceeds cap". The baseline column joins the same contract through the
/// other [`CorpusJoinError`] variants: a hard correlation error on the column,
/// a criteria-version mix, or a misattributed baseline each refuse the report
/// by name. Because these are preconditions the corpus is built to satisfy,
/// they rarely fire — the point is a legible failure, not a hot path.
pub fn build_report_from_corpus(
    data_source: impl Into<String>,
    corpus: &Corpus,
    thresholds: &GatingThresholds,
) -> Result<CorpusReport, CorpusJoinError> {
    let mut reports = Vec::with_capacity(GATING_CANDIDATES.len());
    for (metric, orientation) in GATING_CANDIDATES {
        let report =
            correlate_metric(metric, *orientation, corpus).map_err(|source| CorpusMetricError {
                metric: (*metric).to_string(),
                source,
            })?;
        reports.push(report);
    }
    let checkbox_baseline = correlate_checkbox_baseline(corpus)?;
    Ok(CorpusReport {
        construct: build_report(data_source, &reports, thresholds),
        checkbox_baseline,
    })
}

/// Build one metric's [`CorrelationReport`] from the corpus's revert rate.
///
/// `Err` only on errors that signal a real problem with the data the caller must
/// see; the benign "not computable" outcomes are absorbed by
/// [`revert_correlations`].
fn correlate_metric(
    metric: &str,
    orientation: MetricOrientation,
    corpus: &Corpus,
) -> Result<CorrelationReport, CorrelationError> {
    Ok(CorrelationReport {
        metric: metric.to_string(),
        orientation,
        correlations: revert_correlations(&corpus.observation_pairs(metric))?,
    })
}

/// Correlate the checkbox-baseline column (the achieved Factory level) against
/// the corpus's revert rate, after validating that the scored repos form ONE
/// comparable column: every baseline scored for its own repo, all under the
/// same criteria version. `Ok(None)` when no repo carries the column at all.
fn correlate_checkbox_baseline(
    corpus: &Corpus,
) -> Result<Option<CheckboxBaselineCorrelation>, CorpusJoinError> {
    let scored: Vec<(&Repo, &CheckboxBaseline)> = corpus
        .repos
        .iter()
        .filter_map(|repo| repo.checkbox_baseline.as_ref().map(|b| (repo, b)))
        .collect();
    let Some((_, first)) = scored.first() else {
        return Ok(None);
    };
    for (repo, baseline) in &scored {
        if baseline.repo_id != repo.repo_id {
            return Err(CorpusJoinError::BaselineRepoMismatch {
                repo_id: repo.repo_id.clone(),
                baseline_repo_id: baseline.repo_id.clone(),
            });
        }
    }
    let mut versions: Vec<String> = scored
        .iter()
        .map(|(_, b)| b.criteria_version.clone())
        .collect();
    versions.sort_unstable();
    versions.dedup();
    if versions.len() > 1 {
        return Err(CorpusJoinError::MixedCriteriaVersions(versions));
    }
    let correlations =
        revert_correlations(&corpus.checkbox_level_pairs()).map_err(CorpusJoinError::Baseline)?;
    Ok(Some(CheckboxBaselineCorrelation {
        criteria_version: first.criteria_version.clone(),
        correlations,
    }))
}

/// Run one observation-pair set through [`spearman`] against
/// [`ExternalOutcome::RevertRate`], absorbing the benign "not computable on
/// this corpus" outcomes ([`CorrelationError::TooFewObservations`],
/// [`CorrelationError::ZeroVariance`]) as an empty correlation list — "no
/// external tie" — and propagating the hard data errors. Shared by the metric
/// and checkbox-baseline paths so the absorb/refuse discipline cannot drift
/// between them.
fn revert_correlations(pairs: &[(f64, f64)]) -> Result<Vec<OutcomeCorrelation>, CorrelationError> {
    match spearman(pairs) {
        Ok(rc) => Ok(vec![OutcomeCorrelation {
            outcome: ExternalOutcome::RevertRate,
            coefficient: rc.coefficient,
            n: rc.n,
            p_value: rc.p_value,
        }]),
        Err(CorrelationError::TooFewObservations(_) | CorrelationError::ZeroVariance) => {
            Ok(Vec::new())
        }
        Err(other) => Err(other),
    }
}

/// Parse the reverted-commit SHAs out of `git log` output of the form emitted by
/// `git log --grep="This reverts commit <sha>" --format=...`.
///
/// Pure mechanical extraction: each `This reverts commit <40-hex>.` line names
/// one SHA that was reverted. The full-40-hex form is git's canonical body line
/// (`git revert` writes it verbatim), so matching it is exact, not a heuristic.
/// Returned in first-seen order with duplicates removed — a [`HashSet`] tracks
/// membership (O(1) per line) while the returned `Vec` keeps the order.
pub fn parse_reverted_shas(git_log: &str) -> Vec<String> {
    const MARKER: &str = "This reverts commit ";
    let mut seen = HashSet::new();
    let mut ordered = Vec::new();
    for line in git_log.lines() {
        let Some(rest) = line.trim().strip_prefix(MARKER) else {
            continue;
        };
        let sha: String = rest.chars().take_while(|c| c.is_ascii_hexdigit()).collect();
        if sha.len() == 40 && seen.insert(sha.clone()) {
            ordered.push(sha);
        }
    }
    ordered
}

/// The exact git command the live miner runs to surface reverts in a clone. Built
/// here (not run) so a future live session and the parser test share one source
/// of truth for the command shape. `dir` is the scratch clone path a live session
/// supplies; this crate never executes it. The format is the commit body alone —
/// the canonical `This reverts commit <sha>` line lives there and is all
/// [`parse_reverted_shas`] reads.
///
/// **Ref scope: `--all`, deliberately, not HEAD.** A bare `git log` walks only
/// HEAD-reachable history, but a mined `ground_truth_commit` need not live on
/// HEAD: the live driver validates each SHA with `git rev-parse --verify
/// <sha>^{commit}`, which resolves against the *entire* object database, so a
/// commit on an unmerged branch enters the corpus. Scanning reverts from HEAD
/// only would then miss a revert that lives on that same (or any other) unmerged
/// branch, silently counting a genuinely-reverted commit as kept and biasing the
/// revert rate *down* with no error. `--all` makes the revert scan span every ref
/// — symmetric with the full-object-DB reach of commit resolution — so a revert
/// anywhere in the ref graph is seen. The cost of the broader scope (a revert on
/// an abandoned topic branch counts) is the correct direction: a revert anywhere
/// is evidence the change was backed out, and the alternative is a silent, non-
/// obvious undercount. Enforced by [`tests::revert_log_command_shape_is_stable`].
///
/// Errs with [`GapError::RevertMiner`] when `dir` is not valid UTF-8: git's `-C`
/// argument is a string, and silently lossy-converting the path would point the
/// miner at the wrong directory.
pub fn revert_log_command(dir: &Path) -> Result<Vec<String>, GapError> {
    let dir = dir
        .to_str()
        .ok_or_else(|| GapError::RevertMiner(format!("clone path is not valid UTF-8: {dir:?}")))?;
    Ok(vec![
        "git".into(),
        "-C".into(),
        dir.into(),
        "log".into(),
        // Span every ref, not just HEAD — see the ref-scope note above.
        "--all".into(),
        "--grep=This reverts commit".into(),
        "--format=%b".into(),
    ])
}

/// Runs a command and returns its stdout. The injection point that keeps this
/// module offline: tests pass a closure over captured text; a live session passes
/// a real subprocess runner. The runner reports failure as an opaque message
/// string (whatever the subprocess surface produced); [`mine_reverts`] wraps it
/// into the typed [`GapError::RevertMiner`] at the boundary.
pub type GitRunner<'a> = dyn Fn(&[String]) -> Result<String, String> + 'a;

/// Mine the reverted SHAs from a real clone at `dir`, using the injected `run`.
///
/// This is the live miner, written behind the same boundary the fixtures use but
/// **never executed against a clone in this crate** — every call site here passes
/// a runner over captured text. A future live session supplies a runner that
/// shells out to git; the offline clamp is enforced by never wiring a real
/// subprocess runner into any test or default.
///
/// Errs with [`GapError::RevertMiner`] when the path is not valid UTF-8 (see
/// [`revert_log_command`]) or when the runner fails; the runner's message is
/// carried verbatim inside the variant.
pub fn mine_reverts(dir: &Path, run: &GitRunner<'_>) -> Result<Vec<String>, GapError> {
    let cmd = revert_log_command(dir)?;
    let stdout = run(&cmd).map_err(GapError::RevertMiner)?;
    Ok(parse_reverted_shas(&stdout))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::checkbox_baseline::{FACTORY_CRITERIA_SOURCE, FACTORY_CRITERIA_VERSION};
    use crate::construct::MetricMode;

    const CONFIRMING: &str =
        include_str!("../tests/fixtures/external_outcome_corpus_confirming.json");
    const NOISE: &str = include_str!("../tests/fixtures/external_outcome_corpus_noise.json");

    fn corpus(json: &str) -> Corpus {
        serde_json::from_str(json).expect("corpus fixture parses")
    }

    /// A hand-built baseline column value for join-logic tests. The join reads
    /// only `repo_id`, `criteria_version`, and `level`; the score detail vecs
    /// are irrelevant to it, so they stay empty here.
    fn baseline(repo_id: &str, level: u8) -> CheckboxBaseline {
        CheckboxBaseline {
            repo_id: repo_id.into(),
            criteria_version: FACTORY_CRITERIA_VERSION.to_string(),
            criteria_source: FACTORY_CRITERIA_SOURCE.to_string(),
            level,
            levels: Vec::new(),
            pillars: Vec::new(),
            criteria: Vec::new(),
        }
    }

    /// A one-commit repo with no structure counts (every metric absorbs as
    /// "too few observations") carrying a scored baseline column.
    fn scored_repo(id: &str, level: u8, reverted: bool) -> Repo {
        Repo {
            repo_id: id.into(),
            structure_counts: Default::default(),
            mined_commits: vec![MinedCommit {
                sha: format!("sha-{id}"),
                reverted,
                churn: 1,
            }],
            checkbox_baseline: Some(baseline(id, level)),
        }
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
            checkbox_baseline: None,
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
            checkbox_baseline: None,
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
            checkbox_baseline: None,
        };
        let without_metric = Repo {
            repo_id: "missing".into(),
            structure_counts: std::collections::BTreeMap::new(),
            mined_commits: vec![MinedCommit {
                sha: "y".into(),
                reverted: false, // defined revert rate (0.0), so the metric is the only reason to skip
                churn: 1,
            }],
            checkbox_baseline: None,
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
            .construct
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
        for m in &report.construct.metrics {
            assert_eq!(
                m.mode,
                MetricMode::Advisory,
                "{} must stay advisory on noise",
                m.metric
            );
        }
        // The noise fixture predates the checkbox column and carries none: the
        // report must say so (`None`), not fabricate an empty column.
        assert!(
            report.checkbox_baseline.is_none(),
            "a column-less corpus reports no baseline"
        );
    }

    #[test]
    fn boolean_absence_measure_can_gate_once_clean_repos_supply_a_zero_anchor() {
        // aoa-3a7: with aoa-audit's structure_measurements a clean repo now
        // contributes a real 0, so a boolean absence measure varies (0s and 1s)
        // and can reach Gating on a confirming corpus. Before the fix the corpus
        // only ever saw the 1s (clean repos were dropped), pinning it Advisory
        // regardless of the real relation. A binary predictor needs enough repos
        // to beat the tie-degenerate permutation p-value: 4-vs-4 clean separation
        // gives p = 2·(4!·4!)/8! ≈ 0.029 <= 0.05.
        let repo = |id: &str, absence: u64, reverted: usize| Repo {
            repo_id: id.into(),
            structure_counts: std::collections::BTreeMap::from([(
                "build_determinism_absence".to_string(),
                absence,
            )]),
            mined_commits: (0..10)
                .map(|i| MinedCommit {
                    sha: format!("{id}-{i}"),
                    reverted: i < reverted,
                    churn: 0,
                })
                .collect(),
            checkbox_baseline: None,
        };
        // Pinned builds (0) revert rarely; unpinned (1) revert often — a
        // confirming positive tie for the LowerIsBetter measure vs the RevertRate
        // harm, with clean separation between the two groups.
        let c = Corpus {
            repos: vec![
                repo("r0", 0, 0),
                repo("r1", 0, 1),
                repo("r2", 0, 2),
                repo("r3", 0, 3),
                repo("r4", 1, 7),
                repo("r5", 1, 8),
                repo("r6", 1, 9),
                repo("r7", 1, 10),
            ],
        };
        let report = build_report_from_corpus(
            "test: confirming boolean-absence corpus",
            &c,
            &GatingThresholds::default(),
        )
        .expect("well-defined corpus");
        let m = report
            .construct
            .metrics
            .iter()
            .find(|m| m.metric == "build_determinism_absence")
            .expect("candidate present");
        assert_eq!(
            m.mode,
            MetricMode::Gating,
            "a confirming boolean-absence corpus must gate; correlation={:?}",
            m.correlations.first()
        );
    }

    #[test]
    fn confirming_artifact_round_trips_through_serde() {
        let c = corpus(CONFIRMING);
        let report = build_report_from_corpus("fixture", &c, &GatingThresholds::default())
            .expect("well-defined");
        let json = serde_json::to_string(&report).expect("serializes");
        let back: CorpusReport = serde_json::from_str(&json).expect("round-trips");
        assert_eq!(back, report);
    }

    // --- aoa-d6t.27: the checkbox-baseline column joined into the corpus ---

    #[test]
    fn confirming_fixture_repos_carry_the_checkbox_baseline_column() {
        let c = corpus(CONFIRMING);
        let expected_levels = [5u8, 4, 3, 2, 1, 1];
        assert_eq!(c.repos.len(), expected_levels.len());
        for (repo, expected) in c.repos.iter().zip(expected_levels) {
            let b = repo
                .checkbox_baseline
                .as_ref()
                .unwrap_or_else(|| panic!("{} lacks the baseline column", repo.repo_id));
            assert_eq!(b.repo_id, repo.repo_id, "column scored for its own repo");
            assert_eq!(b.level, expected, "{} level", repo.repo_id);
            assert_eq!(b.criteria_version, FACTORY_CRITERIA_VERSION);
            assert_eq!(
                b.pillars.len(),
                crate::checkbox_baseline::PILLARS.len(),
                "per-pillar fractions ride the column"
            );
        }
    }

    #[test]
    fn checkbox_level_pairs_pair_level_with_revert_rate() {
        // Levels descend as the revert rate rises — the engineered
        // Factory-confirming direction of the synthetic fixture.
        let c = corpus(CONFIRMING);
        assert_eq!(
            c.checkbox_level_pairs(),
            vec![
                (5.0, 0.0),
                (4.0, 0.2),
                (3.0, 0.4),
                (2.0, 0.5),
                (1.0, 0.75),
                (1.0, 1.0),
            ]
        );
    }

    #[test]
    fn checkbox_level_pairs_skip_unscored_and_unmined_repos() {
        let unscored = Repo {
            checkbox_baseline: None,
            ..scored_repo("unscored", 3, true)
        };
        let unmined = Repo {
            mined_commits: Vec::new(),
            ..scored_repo("unmined", 4, true)
        };
        let c = Corpus {
            repos: vec![scored_repo("full", 2, true), unscored, unmined],
        };
        // Only the fully-observed repo contributes a pair.
        assert_eq!(c.checkbox_level_pairs(), vec![(2.0, 1.0)]);
    }

    #[test]
    fn confirming_corpus_carries_baseline_side_by_side_never_gating() {
        let c = corpus(CONFIRMING);
        let report = build_report_from_corpus("fixture", &c, &GatingThresholds::default())
            .expect("well-defined corpus");

        // The AOA-metric side is exactly the gating-candidate list — the
        // baseline column never enters it, so a checkbox score cannot gate.
        assert_eq!(report.construct.metrics.len(), GATING_CANDIDATES.len());
        for m in &report.construct.metrics {
            assert!(
                GATING_CANDIDATES.iter().any(|(name, _)| *name == m.metric),
                "{} is not a registered candidate",
                m.metric
            );
        }

        // The baseline rides beside it: one raw correlation against the same
        // outcome, no MetricMode anywhere in its shape.
        let baseline = report
            .checkbox_baseline
            .as_ref()
            .expect("scored corpus carries the column");
        assert_eq!(baseline.criteria_version, FACTORY_CRITERIA_VERSION);
        assert_eq!(baseline.correlations.len(), 1);
        let corr = &baseline.correlations[0];
        assert_eq!(corr.outcome, ExternalOutcome::RevertRate);
        assert_eq!(corr.n, 6);
        // Hand-computed tie-corrected rho over the pairs above: level ranks
        // (6, 5, 4, 3, 1.5, 1.5) vs rate ranks (1..=6) give cov = -17,
        // norms sqrt(17) and sqrt(17.5).
        let expected = -17.0 / (17.0f64 * 17.5).sqrt();
        assert!(
            (corr.coefficient - expected).abs() < 1e-9,
            "rho={}",
            corr.coefficient
        );
        assert!(corr.p_value <= 0.05, "p={}", corr.p_value);
    }

    #[test]
    fn mixed_criteria_versions_are_refused() {
        let mut c = corpus(CONFIRMING);
        c.repos[1]
            .checkbox_baseline
            .as_mut()
            .expect("fixture repo is scored")
            .criteria_version = "factory-official-v1".to_string();
        let err = build_report_from_corpus("fixture", &c, &GatingThresholds::default())
            .expect_err("levels from two criteria tables are not one column");
        assert_eq!(
            err,
            CorpusJoinError::MixedCriteriaVersions(vec![
                "factory-official-v1".to_string(),
                FACTORY_CRITERIA_VERSION.to_string(),
            ])
        );
    }

    #[test]
    fn baseline_scored_for_another_repo_is_refused() {
        let mut c = corpus(CONFIRMING);
        c.repos[0]
            .checkbox_baseline
            .as_mut()
            .expect("fixture repo is scored")
            .repo_id = "sample/zulu".to_string();
        let err = build_report_from_corpus("fixture", &c, &GatingThresholds::default())
            .expect_err("a misattributed column is a corpus-assembly bug");
        assert_eq!(
            err,
            CorpusJoinError::BaselineRepoMismatch {
                repo_id: "sample/alpha".to_string(),
                baseline_repo_id: "sample/zulu".to_string(),
            }
        );
    }

    #[test]
    fn under_three_scored_repos_absorbs_as_no_tie_not_error() {
        // Two scored repos: spearman refuses (< 3 observations), which is a
        // benign "not computable on this corpus" — the column is still carried,
        // with no fabricated correlation.
        let unscored = Repo {
            checkbox_baseline: None,
            ..scored_repo("r2", 1, true)
        };
        let c = Corpus {
            repos: vec![
                scored_repo("r0", 1, false),
                scored_repo("r1", 3, true),
                unscored,
            ],
        };
        let report = build_report_from_corpus("fixture", &c, &GatingThresholds::default())
            .expect("too-few-scored is absorbed, not an error");
        let baseline = report.checkbox_baseline.expect("column present");
        assert!(baseline.correlations.is_empty(), "no tie, none fabricated");
        assert_eq!(baseline.criteria_version, FACTORY_CRITERIA_VERSION);
    }

    #[test]
    fn constant_level_absorbs_as_no_tie_not_error() {
        let repos: Vec<Repo> = (0..4)
            .map(|i| scored_repo(&format!("r{i}"), 3, i % 2 == 0))
            .collect();
        let c = Corpus { repos };
        let report = build_report_from_corpus("fixture", &c, &GatingThresholds::default())
            .expect("zero variance is absorbed, not an error");
        let baseline = report.checkbox_baseline.expect("column present");
        assert!(baseline.correlations.is_empty(), "no tie, none fabricated");
    }

    #[test]
    fn oversized_baseline_column_is_refused_with_the_typed_error() {
        // 11 scored repos and NO structure counts: every metric absorbs as
        // too-few-observations, so the baseline column is the only place the
        // hard SampleTooLarge can fire — and it must, named as the baseline.
        let repos: Vec<Repo> = (0..11)
            .map(|i| scored_repo(&format!("r{i}"), (i % 5 + 1) as u8, i % 2 == 0))
            .collect();
        let c = Corpus { repos };
        let err = build_report_from_corpus("fixture", &c, &GatingThresholds::default())
            .expect_err("a baseline column past MAX_EXACT_N must refuse the report");
        assert_eq!(
            err,
            CorpusJoinError::Baseline(CorrelationError::SampleTooLarge { n: 11, cap: 10 })
        );
        assert!(
            err.to_string().contains("checkbox baseline"),
            "the error must name the baseline column: {err}"
        );
    }

    #[test]
    fn corpus_with_column_round_trips_through_serde() {
        let c = Corpus {
            repos: vec![scored_repo("r0", 4, false)],
        };
        let json = serde_json::to_string(&c).expect("serializes");
        let back: Corpus = serde_json::from_str(&json).expect("round-trips");
        assert_eq!(back, c);
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
revert rate per repo over mined commits, paired with aoa-audit structure counts and the \
Factory checkbox-baseline column)",
            &c,
            &GatingThresholds::default(),
        )
        .expect("well-defined");
        let from_disk: CorpusReport =
            serde_json::from_str(REPORT_ARTIFACT).expect("artifact parses");
        assert_eq!(
            from_disk, report,
            "committed artifact must match the pipeline output"
        );
    }

    // --- F3b: the report contract on a real data error vs a benign "no tie" ---

    #[test]
    fn sample_too_large_for_one_metric_names_it_instead_of_discarding() {
        // 11 repos all carrying ONLY "module_size_outliers" (an early candidate,
        // retrieval_locality, therefore sees 0 pairs -> TooFewObservations, which
        // must be ABSORBED as Advisory and NOT abort the loop). The offending
        // metric produces 11 observation pairs, one past MAX_EXACT_N=10, so
        // spearman returns SampleTooLarge. The report is refused all-or-nothing,
        // but the error must NAME the offending metric and its n — not surface a
        // bare "sample size 11 exceeds cap 10" with no clue which row is at fault.
        let repos: Vec<Repo> = (0..11)
            .map(|i| Repo {
                repo_id: format!("r{i}"),
                structure_counts: std::collections::BTreeMap::from([(
                    "module_size_outliers".to_string(),
                    i as u64,
                )]),
                mined_commits: vec![MinedCommit {
                    sha: format!("s{i}"),
                    reverted: i % 2 == 0,
                    churn: i as u64,
                }],
                checkbox_baseline: None,
            })
            .collect();
        let c = Corpus { repos };

        let err = build_report_from_corpus("fixture: oversized", &c, &GatingThresholds::default())
            .expect_err("a metric past MAX_EXACT_N must refuse the report");

        let CorpusJoinError::Metric(metric_err) = &err else {
            panic!("expected the metric variant, got {err:?}");
        };
        assert_eq!(
            metric_err.metric, "module_size_outliers",
            "the error must name the offending metric"
        );
        assert!(
            matches!(
                metric_err.source,
                CorrelationError::SampleTooLarge { n: 11, cap: 10 }
            ),
            "source must carry the typed SampleTooLarge with n and cap, got {:?}",
            metric_err.source
        );
        let msg = err.to_string();
        assert!(
            msg.contains("module_size_outliers") && msg.contains("sample size 11"),
            "Display must name metric and n: {msg}"
        );
    }

    #[test]
    fn zero_variance_metric_is_absorbed_as_advisory_not_error() {
        // A metric that never varies across the corpus yields ZeroVariance from
        // spearman. That is "no external tie", NOT a data error: the report must
        // still build (Ok), with the metric honestly Advisory — the absorb path
        // must not regress into the refuse path.
        let repos: Vec<Repo> = (0..4)
            .map(|i| Repo {
                repo_id: format!("r{i}"),
                structure_counts: std::collections::BTreeMap::from([(
                    "unused_import_proxy".to_string(),
                    5, // constant across every repo -> zero variance
                )]),
                mined_commits: vec![MinedCommit {
                    sha: format!("s{i}"),
                    reverted: i % 2 == 0, // revert rate varies; only x is constant
                    churn: 1,
                }],
                checkbox_baseline: None,
            })
            .collect();
        let c = Corpus { repos };

        let report = build_report_from_corpus("fixture: flat", &c, &GatingThresholds::default())
            .expect("a zero-variance metric is absorbed, not an error");
        let m = report
            .construct
            .metrics
            .iter()
            .find(|m| m.metric == "unused_import_proxy")
            .expect("candidate present");
        assert_eq!(
            m.mode,
            MetricMode::Advisory,
            "zero-variance metric stays advisory"
        );
        assert!(
            m.correlations.is_empty(),
            "no tie means no fabricated correlation"
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
    fn parse_reverted_shas_dedup_preserves_first_seen_order_at_scale() {
        // 100 distinct SHAs, each appearing twice. Vec::contains would be
        // O(n^2) here; the HashSet path is O(n). Order must stay first-seen.
        let shas: Vec<String> = (0..100).map(|i| format!("{i:040x}")).collect();
        let log: String = shas
            .iter()
            .chain(shas.iter())
            .map(|s| format!("This reverts commit {s}.\n"))
            .collect();
        assert_eq!(parse_reverted_shas(&log), shas);
    }

    #[test]
    fn revert_log_command_shape_is_stable() {
        // The miner and a future live session share this command shape.
        let cmd = revert_log_command(Path::new("/scratch/clone")).expect("UTF-8 path");
        assert_eq!(cmd[0], "git");
        assert_eq!(cmd[1], "-C");
        assert_eq!(cmd[2], "/scratch/clone");
        assert!(cmd.iter().any(|a| a.contains("This reverts commit")));
        // Ref scope is `--all`, not HEAD: a revert on an unmerged branch must be
        // seen (symmetric with the full-object-DB reach of commit resolution),
        // else the revert rate is silently undercounted.
        assert!(
            cmd.iter().any(|a| a == "--all"),
            "revert scan must span every ref (--all), got {cmd:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn revert_log_command_rejects_non_utf8_path() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;

        let bad = Path::new(OsStr::from_bytes(b"/scratch/\xff\xfe"));
        let err = revert_log_command(bad).expect_err("non-UTF-8 path must fail loudly");
        assert!(
            matches!(err, GapError::RevertMiner(_)),
            "expected RevertMiner, got {err:?}"
        );
        assert!(
            err.to_string().contains("not valid UTF-8"),
            "error must name the UTF-8 failure: {err}"
        );
    }

    #[test]
    fn mine_reverts_uses_injected_runner_over_captured_text() {
        // OFFLINE: the runner returns captured log text; no git is executed.
        let captured = "This reverts commit \
4444444444444444444444444444444444444444.\n"
            .to_string();
        let run = |_cmd: &[String]| -> Result<String, String> { Ok(captured.clone()) };
        let reverted = mine_reverts(Path::new("/scratch/clone"), &run).expect("runner ok");
        assert_eq!(
            reverted,
            vec!["4444444444444444444444444444444444444444".to_string()]
        );
    }

    #[test]
    fn mine_reverts_wraps_runner_failure_in_typed_error() {
        let run = |_cmd: &[String]| -> Result<String, String> { Err("git not found".into()) };
        let err = mine_reverts(Path::new("/scratch/clone"), &run)
            .expect_err("runner failure must propagate");
        assert_eq!(err, GapError::RevertMiner("git not found".into()));
        assert!(
            err.to_string().contains("git not found"),
            "display must carry the runner's message: {err}"
        );
    }
}
