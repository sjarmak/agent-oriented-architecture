//! Canonical, content-addressed evidence emitted by `aoa eval experiment`.

use std::fmt;

use aoa_domain::HeldOutProvenance;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

const SCHEMA_VERSION: &str = "measurement-observation-v1";
const HASH_DOMAIN: &[u8] = b"aoa.measurement-observation.v1\0";

pub const TRACE_LOCALITY_DEFINITION_VERSION: &str = "trace-locality-v1";
pub const TRACE_REACH_DEPTH_DEFINITION_VERSION: &str = "trace-reach-depth-v1";
pub const RETRIEVAL_LOCALITY_DEFINITION_VERSION: &str = "retrieval-locality-v1";
pub const INVARIANT_DISCOVERABILITY_DEFINITION_VERSION: &str = "invariant-discoverability-v1";
pub const MUTATION_SURFACE_DEFINITION_VERSION: &str = "mutation-surface-v1";
pub const EDIT_LOCALITY_DEFINITION_VERSION: &str = "edit-locality-v1";

/// A validated lowercase hexadecimal SHA-256 digest.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Sha256Digest(String);

impl Sha256Digest {
    pub fn parse(value: &str) -> Result<Self, ObservationError> {
        if value.len() != 64
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(ObservationError::InvalidSha256);
        }
        Ok(Self(value.to_string()))
    }

    pub fn of_bytes(bytes: &[u8]) -> Self {
        Self(format!("{:x}", Sha256::digest(bytes)))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Sha256Digest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GitHashAlgorithm {
    Sha1,
    Sha256,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GitObjectId {
    pub algorithm: GitHashAlgorithm,
    pub hex: String,
}

impl GitObjectId {
    pub fn parse(algorithm: GitHashAlgorithm, value: &str) -> Result<Self, ObservationError> {
        let expected = match algorithm {
            GitHashAlgorithm::Sha1 => 40,
            GitHashAlgorithm::Sha256 => 64,
        };
        if value.len() != expected
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(ObservationError::InvalidGitObjectId {
                algorithm,
                expected,
            });
        }
        Ok(Self {
            algorithm,
            hex: value.to_string(),
        })
    }

    pub fn validate(&self) -> Result<(), ObservationError> {
        Self::parse(self.algorithm, &self.hex).map(|_| ())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArmIdentity {
    Repo,
    Harness,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactDigestSetV1 {
    pub scoring: Option<Sha256Digest>,
    pub oracle: Option<Sha256Digest>,
    pub trace: Option<Sha256Digest>,
    pub index: Option<Sha256Digest>,
    pub config: Option<Sha256Digest>,
    pub calibration: Option<Sha256Digest>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CalibrationMethod {
    ExternalOutcomeCorrelation,
    ConsensusVerifiedAnswerOracle,
    HumanAdjudicatedBenchmark,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CalibrationConclusion {
    Calibrated,
    NotCalibrated,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CalibrationEvidenceV1 {
    pub method: CalibrationMethod,
    pub protocol_version: String,
    pub corpus_sha256: Sha256Digest,
    pub sample_size: u64,
    pub criteria: Vec<String>,
    pub conclusion: CalibrationConclusion,
}

impl CalibrationEvidenceV1 {
    pub fn validate(&self) -> Result<(), ObservationError> {
        if self.protocol_version.trim().is_empty()
            || self.sample_size == 0
            || self.criteria.is_empty()
            || self
                .criteria
                .iter()
                .any(|criterion| criterion.trim().is_empty())
        {
            return Err(ObservationError::IncompleteCalibration);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetricValueV1<T> {
    pub definition_version: String,
    pub value: T,
}

impl<T> MetricValueV1<T> {
    pub fn new(definition_version: impl Into<String>, value: T) -> Self {
        Self {
            definition_version: definition_version.into(),
            value,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnswerMetricsV1 {
    pub trace_locality: MetricValueV1<f64>,
    pub trace_reach_depth: MetricValueV1<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EditMetricsV1 {
    pub retrieval_locality: MetricValueV1<f64>,
    pub invariant_discoverability: MetricValueV1<f64>,
    pub mutation_surface: MetricValueV1<u32>,
    pub edit_locality: MetricValueV1<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "task_shape", content = "values", rename_all = "snake_case")]
pub enum MeasurementMetricsV1 {
    Answer(AnswerMetricsV1),
    Edit(EditMetricsV1),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExclusionReasonV1 {
    ArmArtifactMissing,
    ArtifactMissing,
    ArtifactMalformed,
    CalibrationMissing,
    CalibrationMalformed,
    CalibrationNotEstablished,
    MetricEvidenceMissing,
    MetricComputationFailed,
    HeldOutOutcomeMissing,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum MeasurementStateV1 {
    Measured {
        held_out_success: bool,
        held_out_provenance: HeldOutProvenance,
        calibration: CalibrationEvidenceV1,
        metrics: Box<MeasurementMetricsV1>,
    },
    Excluded {
        reason: ExclusionReasonV1,
    },
}

/// One arm's evidence for one original task under one fixed seed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MeasurementObservationV1 {
    pub id: Sha256Digest,
    pub schema_version: String,
    pub repo_id: String,
    pub repo_commit: GitObjectId,
    pub original_task_id: String,
    pub seed: u64,
    pub arm: ArmIdentity,
    pub evidence: ArtifactDigestSetV1,
    pub state: MeasurementStateV1,
}

#[derive(Serialize)]
struct IdentityPayload<'a> {
    schema_version: &'a str,
    repo_id: &'a str,
    repo_commit: &'a GitObjectId,
    original_task_id: &'a str,
    seed: u64,
    arm: ArmIdentity,
    evidence: &'a ArtifactDigestSetV1,
    state: &'a MeasurementStateV1,
}

impl MeasurementObservationV1 {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        repo_id: String,
        repo_commit: GitObjectId,
        original_task_id: String,
        seed: u64,
        arm: ArmIdentity,
        evidence: ArtifactDigestSetV1,
        state: MeasurementStateV1,
    ) -> Result<Self, ObservationError> {
        let mut observation = Self {
            id: Sha256Digest::of_bytes(&[]),
            schema_version: SCHEMA_VERSION.to_string(),
            repo_id,
            repo_commit,
            original_task_id,
            seed,
            arm,
            evidence,
            state,
        };
        observation.recompute_id()?;
        Ok(observation)
    }

    pub fn recompute_id(&mut self) -> Result<(), ObservationError> {
        validate_state(&self.state)?;
        let payload = IdentityPayload {
            schema_version: &self.schema_version,
            repo_id: &self.repo_id,
            repo_commit: &self.repo_commit,
            original_task_id: &self.original_task_id,
            seed: self.seed,
            arm: self.arm,
            evidence: &self.evidence,
            state: &self.state,
        };
        let canonical =
            serde_jcs::to_vec(&payload).map_err(|error| ObservationError::Canonical {
                message: error.to_string(),
            })?;
        let mut hasher = Sha256::new();
        hasher.update(HASH_DOMAIN);
        hasher.update(canonical);
        self.id = Sha256Digest(format!("{:x}", hasher.finalize()));
        Ok(())
    }

    pub fn verify_id(&self) -> Result<(), ObservationError> {
        let mut expected = self.clone();
        expected.recompute_id()?;
        if expected.id == self.id {
            Ok(())
        } else {
            Err(ObservationError::IdentityMismatch)
        }
    }
}

fn validate_state(state: &MeasurementStateV1) -> Result<(), ObservationError> {
    let metrics = match state {
        MeasurementStateV1::Measured { metrics, .. } => metrics,
        MeasurementStateV1::Excluded { .. } => return Ok(()),
    };
    let finite = match &**metrics {
        MeasurementMetricsV1::Answer(values) => values.trace_locality.value.is_finite(),
        MeasurementMetricsV1::Edit(values) => {
            values.retrieval_locality.value.is_finite()
                && values.invariant_discoverability.value.is_finite()
                && values.edit_locality.value.is_finite()
        }
    };
    if finite {
        Ok(())
    } else {
        Err(ObservationError::NonFiniteMetric)
    }
}

#[derive(Debug, Error)]
pub enum ObservationError {
    #[error("SHA-256 digest must be 64 lowercase hexadecimal characters")]
    InvalidSha256,
    #[error("{algorithm:?} object id must be {expected} lowercase hexadecimal characters")]
    InvalidGitObjectId {
        algorithm: GitHashAlgorithm,
        expected: usize,
    },
    #[error("measurement metric values must be finite")]
    NonFiniteMetric,
    #[error("failed to encode RFC 8785 canonical observation identity: {message}")]
    Canonical { message: String },
    #[error("measurement observation id does not match its canonical content")]
    IdentityMismatch,
    #[error(
        "calibration evidence requires a protocol version, positive sample size, and criteria"
    )]
    IncompleteCalibration,
}
