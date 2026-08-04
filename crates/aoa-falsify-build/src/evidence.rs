//! Content-addressed artifact loading and per-arm observation assembly.

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result as AnyResult};
use aoa_bench::{
    load_task, scoring_path, transcript_path, AnswerMetricsV1, ArmIdentity, ArtifactDigestSetV1,
    CalibrationConclusion, CalibrationEvidenceV1, ExclusionReasonV1, MeasurementMetricsV1,
    MeasurementObservationV1, MeasurementStateV1, MetricValueV1, Sha256Digest,
    TRACE_LOCALITY_DEFINITION_VERSION, TRACE_REACH_DEPTH_DEFINITION_VERSION,
};

use crate::answer::AnswerContext;
use crate::manifest::{RepoManifest, TaskShape};

/// Per-artifact and aggregate task-evidence read bound.
const MAX_EVIDENCE_BYTES: u64 = 128 * 1024 * 1024;

pub(crate) struct LoadedArtifact<T> {
    pub(crate) digest: Option<Sha256Digest>,
    pub(crate) value: Option<T>,
    pub(crate) reason: Option<ExclusionReasonV1>,
    pub(crate) diagnostic: Option<String>,
}

pub(crate) fn read_artifact(path: &Path) -> LoadedArtifact<Vec<u8>> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_file() => metadata,
        Ok(_) => {
            return LoadedArtifact {
                digest: None,
                value: None,
                reason: Some(ExclusionReasonV1::ArtifactMalformed),
                diagnostic: Some(format!("{} is not a regular file", path.display())),
            };
        }
        Err(error) => {
            return LoadedArtifact {
                digest: None,
                value: None,
                reason: Some(ExclusionReasonV1::ArtifactMissing),
                diagnostic: Some(format!("cannot read {}: {error}", path.display())),
            };
        }
    };
    if metadata.len() > MAX_EVIDENCE_BYTES {
        return LoadedArtifact {
            digest: None,
            value: None,
            reason: Some(ExclusionReasonV1::ArtifactMalformed),
            diagnostic: Some(format!(
                "{} exceeds {MAX_EVIDENCE_BYTES} byte evidence cap",
                path.display()
            )),
        };
    }
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(error) => {
            return LoadedArtifact {
                digest: None,
                value: None,
                reason: Some(ExclusionReasonV1::ArtifactMissing),
                diagnostic: Some(format!("cannot open {}: {error}", path.display())),
            };
        }
    };
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    if let Err(error) = std::io::Read::by_ref(&mut file)
        .take(MAX_EVIDENCE_BYTES + 1)
        .read_to_end(&mut bytes)
    {
        return LoadedArtifact {
            digest: None,
            value: None,
            reason: Some(ExclusionReasonV1::ArtifactMalformed),
            diagnostic: Some(format!("cannot read {}: {error}", path.display())),
        };
    }
    if bytes.len() as u64 > MAX_EVIDENCE_BYTES {
        return LoadedArtifact {
            digest: None,
            value: None,
            reason: Some(ExclusionReasonV1::ArtifactMalformed),
            diagnostic: Some(format!(
                "{} exceeds {MAX_EVIDENCE_BYTES} byte evidence cap",
                path.display()
            )),
        };
    }
    LoadedArtifact {
        digest: Some(Sha256Digest::of_bytes(&bytes)),
        value: Some(bytes),
        reason: None,
        diagnostic: None,
    }
}

pub(crate) fn read_calibration(path: &Path) -> LoadedArtifact<CalibrationEvidenceV1> {
    let raw = read_artifact(path);
    let Some(bytes) = raw.value else {
        return LoadedArtifact {
            digest: raw.digest,
            value: None,
            reason: Some(match raw.reason {
                Some(ExclusionReasonV1::ArtifactMissing) => ExclusionReasonV1::CalibrationMissing,
                _ => ExclusionReasonV1::CalibrationMalformed,
            }),
            diagnostic: raw.diagnostic,
        };
    };
    match serde_json::from_slice::<CalibrationEvidenceV1>(&bytes) {
        Ok(value) => match value.validate() {
            Ok(()) => LoadedArtifact {
                digest: raw.digest,
                value: Some(value),
                reason: None,
                diagnostic: None,
            },
            Err(error) => LoadedArtifact {
                digest: raw.digest,
                value: None,
                reason: Some(ExclusionReasonV1::CalibrationMalformed),
                diagnostic: Some(error.to_string()),
            },
        },
        Err(error) => LoadedArtifact {
            digest: raw.digest,
            value: None,
            reason: Some(ExclusionReasonV1::CalibrationMalformed),
            diagnostic: Some(format!(
                "invalid calibration artifact {}: {error}",
                path.display()
            )),
        },
    }
}

/// Digest every regular file below a task directory using a deterministic
/// filename-length/content-length framing. This covers every oracle input the
/// bench loader may consume without making relocation part of identity.
fn task_artifact_digest(task_dir: &Path) -> LoadedArtifact<()> {
    fn visit(dir: &Path, files: &mut Vec<PathBuf>) -> std::io::Result<()> {
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let file_type = entry.file_type()?;
            if file_type.is_dir() {
                visit(&entry.path(), files)?;
            } else if file_type.is_file() {
                files.push(entry.path());
            } else if file_type.is_symlink() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!(
                        "symlinked task artifact refused: {}",
                        entry.path().display()
                    ),
                ));
            }
        }
        Ok(())
    }

    let mut files = Vec::new();
    if let Err(error) = visit(task_dir, &mut files) {
        return LoadedArtifact {
            digest: None,
            value: None,
            reason: Some(ExclusionReasonV1::ArtifactMissing),
            diagnostic: Some(format!(
                "cannot enumerate task artifacts under {}: {error}",
                task_dir.display()
            )),
        };
    }
    files.sort();
    let mut framed = Vec::new();
    for path in files {
        let relative = path.strip_prefix(task_dir).expect("visited below root");
        let loaded = read_artifact(&path);
        let Some(bytes) = loaded.value else {
            return LoadedArtifact {
                digest: loaded.digest,
                value: None,
                reason: loaded.reason,
                diagnostic: loaded.diagnostic,
            };
        };
        let Some(name) = relative.to_str() else {
            return LoadedArtifact {
                digest: None,
                value: None,
                reason: Some(ExclusionReasonV1::ArtifactMalformed),
                diagnostic: Some(format!(
                    "task artifact path {:?} is not valid UTF-8",
                    relative.as_os_str()
                )),
            };
        };
        if framed
            .len()
            .saturating_add(name.len())
            .saturating_add(bytes.len())
            .saturating_add(16)
            > MAX_EVIDENCE_BYTES as usize
        {
            return LoadedArtifact {
                digest: None,
                value: None,
                reason: Some(ExclusionReasonV1::ArtifactMalformed),
                diagnostic: Some(format!(
                    "task artifact bundle under {} exceeds {MAX_EVIDENCE_BYTES} byte cap",
                    task_dir.display()
                )),
            };
        }
        framed.extend_from_slice(&(name.len() as u64).to_be_bytes());
        framed.extend_from_slice(name.as_bytes());
        framed.extend_from_slice(&(bytes.len() as u64).to_be_bytes());
        framed.extend_from_slice(&bytes);
    }
    LoadedArtifact {
        digest: Some(Sha256Digest::of_bytes(&framed)),
        value: Some(()),
        reason: None,
        diagnostic: None,
    }
}

pub(crate) struct RepoEvidence {
    pub(crate) calibration: LoadedArtifact<CalibrationEvidenceV1>,
    pub(crate) repo_config: LoadedArtifact<Vec<u8>>,
    pub(crate) harness_config: LoadedArtifact<Vec<u8>>,
    pub(crate) index: Option<LoadedArtifact<Vec<u8>>>,
}

fn first_evidence_problem(
    values: &[(&LoadedArtifact<Vec<u8>>, ExclusionReasonV1)],
) -> Option<(ExclusionReasonV1, String)> {
    values.iter().find_map(|(loaded, fallback)| {
        loaded.value.is_none().then(|| {
            (
                loaded.reason.unwrap_or(*fallback),
                loaded
                    .diagnostic
                    .clone()
                    .unwrap_or_else(|| "required evidence unavailable".to_string()),
            )
        })
    })
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn build_observation(
    repo: &RepoManifest,
    seed: u64,
    arm: ArmIdentity,
    run_dir: &Path,
    task_id: &str,
    outcome: Option<std::result::Result<bool, String>>,
    tasks_dir: &Path,
    repo_evidence: &RepoEvidence,
    answer_ctx: Option<&mut AnswerContext>,
    answer_context_error: Option<&str>,
) -> AnyResult<(MeasurementObservationV1, Option<String>)> {
    let scoring = read_artifact(&scoring_path(run_dir, task_id));
    let trace = read_artifact(&transcript_path(run_dir, task_id));
    let oracle = task_artifact_digest(&tasks_dir.join(task_id));
    let config = match arm {
        ArmIdentity::Repo => &repo_evidence.repo_config,
        ArmIdentity::Harness => &repo_evidence.harness_config,
    };
    let evidence = ArtifactDigestSetV1 {
        scoring: scoring.digest.clone(),
        oracle: oracle.digest.clone(),
        trace: trace.digest.clone(),
        index: repo_evidence
            .index
            .as_ref()
            .and_then(|loaded| loaded.digest.clone()),
        config: config.digest.clone(),
        calibration: repo_evidence.calibration.digest.clone(),
    };

    let excluded =
        |reason, diagnostic: String| (MeasurementStateV1::Excluded { reason }, diagnostic);
    let (state, diagnostic) = match outcome {
        None => excluded(
            ExclusionReasonV1::ArmArtifactMissing,
            format!("{arm:?} arm has no trial artifacts for task {task_id}"),
        ),
        Some(Err(diagnostic)) => excluded(ExclusionReasonV1::ArtifactMalformed, diagnostic),
        Some(Ok(held_out_success)) => {
            let required = [
                (&scoring, ExclusionReasonV1::ArtifactMissing),
                (&trace, ExclusionReasonV1::ArtifactMissing),
                (config, ExclusionReasonV1::ArtifactMissing),
            ];
            if let Some((reason, diagnostic)) = first_evidence_problem(&required) {
                excluded(reason, diagnostic)
            } else if oracle.value.is_none() {
                excluded(
                    oracle.reason.unwrap_or(ExclusionReasonV1::ArtifactMissing),
                    oracle
                        .diagnostic
                        .clone()
                        .unwrap_or_else(|| "oracle evidence unavailable".to_string()),
                )
            } else if let Some(reason) = repo_evidence.calibration.reason {
                excluded(
                    reason,
                    repo_evidence
                        .calibration
                        .diagnostic
                        .clone()
                        .unwrap_or_else(|| "calibration evidence unavailable".to_string()),
                )
            } else {
                let calibration = repo_evidence
                    .calibration
                    .value
                    .clone()
                    .expect("reason-free calibration has a value");
                if calibration.conclusion != CalibrationConclusion::Calibrated {
                    excluded(
                        ExclusionReasonV1::CalibrationNotEstablished,
                        "calibration artifact conclusion is not calibrated".to_string(),
                    )
                } else if repo.task_shape == TaskShape::Edit {
                    excluded(
                        ExclusionReasonV1::MetricEvidenceMissing,
                        "edit-shaped observation requires retrieval, invariant, mutation, and edit-locality evidence; degraded sentinels are forbidden".to_string(),
                    )
                } else if let Some(error) = answer_context_error {
                    excluded(ExclusionReasonV1::MetricEvidenceMissing, error.to_string())
                } else {
                    let Some(ctx) = answer_ctx else {
                        unreachable!("answer context or its error is present")
                    };
                    match ctx.observation_inputs(
                        task_id,
                        run_dir,
                        match arm {
                            ArmIdentity::Repo => "repo",
                            ArmIdentity::Harness => "harness",
                        },
                    ) {
                        Ok((trace_locality, trace_reach_depth)) => {
                            let task = load_task(tasks_dir.join(task_id)).with_context(|| {
                                format!("failed to load task {task_id} provenance")
                            })?;
                            (
                                MeasurementStateV1::Measured {
                                    held_out_success,
                                    held_out_provenance: task.held_out_provenance(),
                                    calibration,
                                    metrics: Box::new(MeasurementMetricsV1::Answer(
                                        AnswerMetricsV1 {
                                            trace_locality: MetricValueV1::new(
                                                TRACE_LOCALITY_DEFINITION_VERSION,
                                                trace_locality,
                                            ),
                                            trace_reach_depth: MetricValueV1::new(
                                                TRACE_REACH_DEPTH_DEFINITION_VERSION,
                                                trace_reach_depth,
                                            ),
                                        },
                                    )),
                                },
                                String::new(),
                            )
                        }
                        Err(error) => excluded(
                            ExclusionReasonV1::MetricComputationFailed,
                            format!("{error:#}"),
                        ),
                    }
                }
            }
        }
    };
    let observation = MeasurementObservationV1::new(
        repo.repo_id.clone(),
        repo.repo_commit.clone(),
        task_id.to_string(),
        seed,
        arm,
        evidence,
        state,
    )?;
    Ok((observation, (!diagnostic.is_empty()).then_some(diagnostic)))
}

#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::fs::symlink;

    use super::*;

    #[test]
    fn evidence_reader_refuses_a_symlink_without_hashing_its_target() {
        let dir = tempfile::tempdir().unwrap();
        let victim = dir.path().join("victim.json");
        let planted = dir.path().join("config.json");
        std::fs::write(&victim, br#"{"secret":"outside"}"#).unwrap();
        symlink(&victim, &planted).unwrap();

        let loaded = read_artifact(&planted);

        assert!(loaded.value.is_none());
        assert!(loaded.digest.is_none());
        assert_eq!(loaded.reason, Some(ExclusionReasonV1::ArtifactMalformed));
    }
}
