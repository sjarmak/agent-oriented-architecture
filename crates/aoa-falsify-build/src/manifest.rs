//! The operator-authored build manifest.
//!
//! `deny_unknown_fields` on every boundary: a misspelled key
//! (`min_effect_szie`) must fail loud, not silently leave the real field at its
//! default (0.0 disables the effect-size floor).

use std::path::PathBuf;

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
    /// Determinism replication count (>= 3); each repo must supply this many runs.
    pub(crate) k_runs: u32,
    /// Power precondition: minimum per-repo held-out size.
    pub(crate) min_holdout_size: u32,
    /// Power precondition: minimum aggregate effect size. Defaults to `0.0`,
    /// which disables the effect-size floor (every effect clears `>= 0.0`) —
    /// matching `aoa_falsify::FalsifyConfig::default`. Set it explicitly to make
    /// the power precondition bite.
    #[serde(default)]
    pub(crate) min_effect_size: f64,
    pub(crate) repos: Vec<RepoManifest>,
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

    #[test]
    fn confidence_decl_maps_to_metrics_confidence() {
        assert_eq!(Confidence::from(ConfidenceDecl::High), Confidence::High);
        assert_eq!(Confidence::from(ConfidenceDecl::Low), Confidence::Low);
    }

    #[test]
    fn unknown_fields_are_rejected_at_every_boundary() {
        let cases = [
            (
                r#"{"k_runs":3,"min_holdout_size":1,"min_effect_szie":0.5,"repos":[]}"#.to_string(),
                "min_effect_szie",
            ),
            (
                format!(
                    r#"{{"k_runs":3,"min_holdout_size":1,"repos":[{REPO_PREFIX}
                    "calibratd":"x","runs":[]}}]}}"#
                ),
                "calibratd",
            ),
            (
                format!(
                    r#"{{"k_runs":3,"min_holdout_size":1,"repos":[{REPO_PREFIX}
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
            r#"{{"k_runs":3,"min_holdout_size":1,"repos":[{REPO_PREFIX}
            "calibrated_basis":"R11 scope note","runs":[]}}]}}"#
        );
        let error = serde_json::from_str::<Manifest>(&json).unwrap_err();
        assert!(error.to_string().contains("calibrated_basis"));
    }
}
