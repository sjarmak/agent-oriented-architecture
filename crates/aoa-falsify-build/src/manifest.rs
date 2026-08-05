//! The operator-authored build manifest.
//!
//! `deny_unknown_fields` on every boundary: a misspelled key
//! (`min_effect_szie`) must fail loud, not silently leave the real field at its
//! default (0.0 disables the effect-size floor).

use std::collections::BTreeSet;
use std::path::PathBuf;

use anyhow::bail;
use serde::{Deserialize, Serialize};

use aoa_bench::GitObjectId;
use aoa_gap::ExposureStatus;
use aoa_metrics::Confidence;

/// The whole build manifest.
///
/// Fields are crate-private: the manifest is a value the caller deserializes
/// and hands to [`crate::build`] whole, never one it reads field by field.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    /// Runs required per repo: 1 for pair-yield preflight, >= 3 for a verdict.
    pub(crate) k_runs: u32,
    /// Power precondition: minimum per-repo held-out size.
    pub(crate) min_holdout_size: u32,
    /// Power precondition: minimum aggregate effect size. Defaults to `0.0`,
    /// which disables the effect-size floor (every effect clears `>= 0.0`) —
    /// matching `aoa_falsify::FalsifyConfig::default`. Set it explicitly to make
    /// the power precondition bite.
    #[serde(default)]
    pub(crate) min_effect_size: f64,
    /// Pre-registered campaign inventory. The builder requires the repo IDs
    /// below to match this set exactly before reading any campaign evidence.
    pub(crate) expected_repo_ids: Vec<String>,
    pub(crate) repos: Vec<RepoManifest>,
}

impl Manifest {
    pub(crate) fn validate_repo_inventory(&self) -> anyhow::Result<()> {
        if let Some(repo_id) = first_duplicate(self.expected_repo_ids.iter().map(String::as_str)) {
            bail!(
                "manifest expected_repo_ids contains duplicate repo id: {}",
                diagnostic_repo_id(repo_id)
            );
        }
        if let Some(repo_id) = first_duplicate(self.repos.iter().map(|repo| repo.repo_id.as_str()))
        {
            bail!(
                "manifest repos contains duplicate repo id: {}",
                diagnostic_repo_id(repo_id)
            );
        }

        let expected: BTreeSet<_> = self.expected_repo_ids.iter().map(String::as_str).collect();
        let actual: BTreeSet<_> = self
            .repos
            .iter()
            .map(|repo| repo.repo_id.as_str())
            .collect();
        let missing = diagnostic_repo_ids(expected.difference(&actual).copied());
        let unexpected = diagnostic_repo_ids(actual.difference(&expected).copied());
        if !missing.is_empty() || !unexpected.is_empty() {
            bail!(
                "manifest repo inventory mismatch: missing [{missing}]; unexpected [{unexpected}]"
            );
        }
        Ok(())
    }

    /// Require the seed-1-only manifest used by the pair-yield budget preflight.
    pub fn validate_pair_yield_preflight(&self) -> crate::Result<()> {
        let invalid_repo = self.repos.iter().find(|repo| repo.runs.len() != 1);
        if self.k_runs == 1 && invalid_repo.is_none() {
            return Ok(());
        }

        let detail = invalid_repo.map_or_else(
            || format!("manifest declares k_runs={}", self.k_runs),
            |repo| {
                format!(
                    "repo {} supplies {} runs and manifest declares k_runs={}",
                    repo.repo_id,
                    repo.runs.len(),
                    self.k_runs
                )
            },
        );
        Err(crate::FalsifyBuildError::from_anyhow(anyhow::anyhow!(
            "--min-pair-yield is a seed-1 preflight and requires exactly one run per repo with \
             k_runs=1; {detail}. Use the seed-1 manifest from docs/r0_runbook.md Step 3"
        )))
    }
}

fn first_duplicate<'a>(values: impl Iterator<Item = &'a str>) -> Option<&'a str> {
    let mut seen = BTreeSet::new();
    values.into_iter().find(|value| !seen.insert(*value))
}

fn diagnostic_repo_ids<'a>(repo_ids: impl Iterator<Item = &'a str>) -> String {
    repo_ids
        .map(diagnostic_repo_id)
        .collect::<Vec<_>>()
        .join(", ")
}

fn diagnostic_repo_id(repo_id: &str) -> String {
    format!(r#""{}""#, repo_id.escape_default())
}

/// One repo's operator assertions and its per-seed arm run dirs.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepoManifest {
    pub(crate) repo_id: String,
    /// Exact source revision whose task and graph evidence is being measured.
    pub(crate) repo_commit: GitObjectId,
    /// Operator assertion that the repo carries a SCIP-grade (high-confidence)
    /// index. REQUIRED — there is no safe default toward eligibility.
    pub(crate) confidence: ConfidenceDecl,
    /// Typed, content-addressed evidence backing calibration eligibility.
    pub(crate) calibration_artifact: PathBuf,
    /// Pre-admission result from `aoa eval exposure scan`. REQUIRED: absence
    /// must never default toward eligibility.
    pub(crate) exposure: ExposureStatus,
    /// Exact configuration bytes used by every repo-arm replication.
    pub(crate) repo_arm_config: PathBuf,
    /// Exact configuration bytes used by every harness-arm replication.
    pub(crate) harness_arm_config: PathBuf,
    /// The task shape this repo's trials carry. `answer` (comprehension tasks)
    /// computes real trace-locality/trace-reach convention inputs and REQUIRES
    /// `scip_index`; `edit` (the default) emits degraded sentinels until an
    /// edit-task pipeline exists.
    #[serde(default)]
    pub(crate) task_shape: TaskShape,
    /// Vendored SCIP JSON index for this repo (the `aoa eval run --scip-index`
    /// form), resolved relative to the manifest. Required for `answer` shape;
    /// rejected otherwise (it would silently do nothing).
    ///
    /// Pinned to the BASELINE-arm (pre-migration) repo state: one shared
    /// universe measures both arms symmetrically, and migration-added files are
    /// intentionally out-of-universe for both arms — see `docs/r0_runbook.md`
    /// § "Which SCIP index (pinned)".
    #[serde(default)]
    pub(crate) scip_index: Option<PathBuf>,
    pub(crate) runs: Vec<RunManifest>,
}

/// Declared task shape of a repo's trials. Spelled lowercase in the manifest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskShape {
    #[default]
    Edit,
    Answer,
}

/// One fixed-seed replication: the two arm run dirs over the same mined tasks.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunManifest {
    pub(crate) seed: u64,
    /// codeprobe config-label run dir for the AOA-migrated arm.
    pub(crate) repo_arm: PathBuf,
    /// codeprobe config-label run dir for the harness-swap arm.
    pub(crate) harness_arm: PathBuf,
}

/// Operator-declared index confidence. Spelled lowercase in the manifest.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfidenceDecl {
    High,
    Low,
}

impl From<ConfidenceDecl> for Confidence {
    fn from(d: ConfidenceDecl) -> Self {
        match d {
            ConfidenceDecl::High => Confidence::High,
            ConfidenceDecl::Low => Confidence::Low,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const REPO_PREFIX: &str = r#"{"repo_id":"r","confidence":"high","exposure":"unexposed",
        "repo_commit":{"algorithm":"sha1","hex":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"},
        "calibration_artifact":"calibration.json","repo_arm_config":"repo.json",
        "harness_arm_config":"harness.json","#;

    fn manifest_with_inventory(expected_repo_ids: &str, repos: &str) -> Manifest {
        serde_json::from_str(&format!(
            r#"{{"k_runs":3,"min_holdout_size":1,"expected_repo_ids":[{expected_repo_ids}],"repos":[{repos}]}}"#
        ))
        .unwrap()
    }

    fn manifest_with_runs(k_runs: u32, runs: &str) -> Manifest {
        serde_json::from_str(&format!(
            r#"{{"k_runs":{k_runs},"min_holdout_size":1,"expected_repo_ids":["r"],"repos":[{REPO_PREFIX}
            "runs":{runs}}}]}}"#
        ))
        .unwrap()
    }

    fn repo(repo_id: &str) -> String {
        format!(r#"{REPO_PREFIX}"runs":[]}}"#)
            .replace(r#""repo_id":"r""#, &format!(r#""repo_id":"{repo_id}""#))
    }

    #[test]
    fn confidence_decl_maps_to_metrics_confidence() {
        assert_eq!(Confidence::from(ConfidenceDecl::High), Confidence::High);
        assert_eq!(Confidence::from(ConfidenceDecl::Low), Confidence::Low);
    }

    #[test]
    fn repo_inventory_reports_missing_and_unexpected_ids() {
        let repos = format!("{},{}", repo("r"), repo("unexpected"));
        let manifest = manifest_with_inventory(r#""r","missing""#, &repos);

        let error = manifest.validate_repo_inventory().unwrap_err();

        assert_eq!(
            error.to_string(),
            "manifest repo inventory mismatch: missing [\"missing\"]; unexpected [\"unexpected\"]"
        );
    }

    #[test]
    fn repo_inventory_rejects_duplicate_expected_and_actual_ids() {
        let duplicate_expected = manifest_with_inventory(r#""r","r""#, &repo("r"));
        assert_eq!(
            duplicate_expected
                .validate_repo_inventory()
                .unwrap_err()
                .to_string(),
            "manifest expected_repo_ids contains duplicate repo id: \"r\""
        );

        let duplicate_actual =
            manifest_with_inventory(r#""r""#, &format!("{},{}", repo("r"), repo("r")));
        assert_eq!(
            duplicate_actual
                .validate_repo_inventory()
                .unwrap_err()
                .to_string(),
            "manifest repos contains duplicate repo id: \"r\""
        );
    }

    #[test]
    fn repo_inventory_accepts_existing_free_text_repo_ids() {
        let repo_id = "team name/../répo";
        let manifest = manifest_with_inventory(&format!(r#""{repo_id}""#), &repo(repo_id));

        manifest.validate_repo_inventory().unwrap();
    }

    #[test]
    fn repo_inventory_escapes_display_unsafe_ids_in_diagnostics() {
        let bidi = manifest_with_inventory(r#""safe\u202egpj.exe""#, &repo("actual"));
        let error = bidi.validate_repo_inventory().unwrap_err().to_string();
        assert!(
            !error.contains('\u{202e}'),
            "raw bidi override leaked: {error:?}"
        );
        assert!(error.contains(r#""safe\u{202e}gpj.exe""#), "got: {error}");

        let delimiter = manifest_with_inventory(r#""legit], unexpected [forged""#, &repo("actual"));
        let error = delimiter.validate_repo_inventory().unwrap_err().to_string();
        assert!(
            error.contains(r#""legit], unexpected [forged""#),
            "delimiter-shaped ID must remain quoted: {error}"
        );
    }

    #[test]
    fn pair_yield_preflight_requires_one_declared_and_supplied_run() {
        let one_run = r#"[{"seed":1,"repo_arm":"a","harness_arm":"b"}]"#;
        let two_runs = r#"[{"seed":1,"repo_arm":"a","harness_arm":"b"},
            {"seed":2,"repo_arm":"c","harness_arm":"d"}]"#;

        manifest_with_runs(1, one_run)
            .validate_pair_yield_preflight()
            .unwrap();
        for manifest in [
            manifest_with_runs(3, one_run),
            manifest_with_runs(1, two_runs),
        ] {
            let error = manifest.validate_pair_yield_preflight().unwrap_err();
            assert!(error
                .to_string()
                .contains("requires exactly one run per repo"));
        }
    }

    #[test]
    fn unknown_fields_are_rejected_at_every_boundary() {
        let cases = [
            (
                r#"{"k_runs":3,"min_holdout_size":1,"min_effect_szie":0.5,"expected_repo_ids":[],"repos":[]}"#.to_string(),
                "min_effect_szie",
            ),
            (
                format!(
                    r#"{{"k_runs":3,"min_holdout_size":1,"expected_repo_ids":["r"],"repos":[{REPO_PREFIX}
                    "calibratd":"x","runs":[]}}]}}"#
                ),
                "calibratd",
            ),
            (
                format!(
                    r#"{{"k_runs":3,"min_holdout_size":1,"expected_repo_ids":["r"],"repos":[{REPO_PREFIX}
                    "runs":[{{"seed":1,"repo_arm":"a","harness_arm":"b","sed":2}}]}}]}}"#
                ),
                "sed",
            ),
        ];
        for (json, key) in cases {
            let error = serde_json::from_str::<Manifest>(&json).unwrap_err();
            assert!(error.to_string().contains(key), "got: {error}");
        }
    }

    #[test]
    fn legacy_calibrated_basis_key_is_rejected() {
        let json = format!(
            r#"{{"k_runs":3,"min_holdout_size":1,"expected_repo_ids":["r"],"repos":[{REPO_PREFIX}
            "calibrated_basis":"R11 scope note","runs":[]}}]}}"#
        );
        let error = serde_json::from_str::<Manifest>(&json).unwrap_err();
        assert!(error.to_string().contains("calibrated_basis"));
    }
}
