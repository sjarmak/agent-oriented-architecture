use aoa_bench::{
    AnswerMetricsV1, ArmIdentity, ArtifactDigestSetV1, CalibrationConclusion,
    CalibrationEvidenceV1, CalibrationMethod, ExclusionReasonV1, GitHashAlgorithm, GitObjectId,
    MeasurementObservationV1, MeasurementStateV1, MetricValueV1, Sha256Digest,
};
use aoa_gap::HeldOutProvenance;

fn digest(byte: char) -> Sha256Digest {
    Sha256Digest::parse(&byte.to_string().repeat(64)).expect("valid digest")
}

fn measured() -> MeasurementObservationV1 {
    MeasurementObservationV1::new(
        "sample/repo".to_string(),
        GitObjectId::parse(GitHashAlgorithm::Sha1, &"a".repeat(40)).expect("valid oid"),
        "original-task-id".to_string(),
        7,
        ArmIdentity::Repo,
        ArtifactDigestSetV1 {
            scoring: Some(digest('1')),
            oracle: Some(digest('2')),
            trace: Some(digest('3')),
            index: Some(digest('4')),
            config: Some(digest('5')),
            calibration: Some(digest('6')),
        },
        MeasurementStateV1::Measured {
            held_out_success: true,
            held_out_provenance: HeldOutProvenance::NativeComposed,
            calibration: CalibrationEvidenceV1 {
                method: CalibrationMethod::ExternalOutcomeCorrelation,
                protocol_version: "r11".to_string(),
                corpus_sha256: digest('7'),
                sample_size: 20,
                criteria: vec!["rho-significant".to_string()],
                conclusion: CalibrationConclusion::Calibrated,
            },
            metrics: Box::new(aoa_bench::MeasurementMetricsV1::Answer(AnswerMetricsV1 {
                trace_locality: MetricValueV1::new("trace-locality-v1", 0.75),
                trace_reach_depth: MetricValueV1::new("trace-reach-depth-v1", 2),
            })),
        },
    )
    .expect("observation")
}

#[test]
fn identical_content_reproduces_the_same_domain_separated_id() {
    let left = measured();
    let right = measured();
    assert_eq!(left.id, right.id);
    assert_eq!(left.id.as_str().len(), 64);
}

#[test]
fn every_identity_and_evidence_axis_changes_the_id() {
    let baseline = measured();

    let mut changed = measured();
    changed.original_task_id = "another-task".to_string();
    changed.recompute_id().expect("recompute");
    assert_ne!(changed.id, baseline.id);

    for field in [
        "scoring",
        "oracle",
        "trace",
        "index",
        "config",
        "calibration",
    ] {
        let mut changed = measured();
        let replacement = Some(digest('9'));
        match field {
            "scoring" => changed.evidence.scoring = replacement,
            "oracle" => changed.evidence.oracle = replacement,
            "trace" => changed.evidence.trace = replacement,
            "index" => changed.evidence.index = replacement,
            "config" => changed.evidence.config = replacement,
            "calibration" => changed.evidence.calibration = replacement,
            _ => unreachable!(),
        }
        changed.recompute_id().expect("recompute");
        assert_ne!(changed.id, baseline.id, "{field} must affect identity");
    }

    let mut changed = measured();
    if let MeasurementStateV1::Measured { metrics, .. } = &mut changed.state {
        if let aoa_bench::MeasurementMetricsV1::Answer(metrics) = &mut **metrics {
            metrics.trace_locality.definition_version = "trace-locality-v2".to_string();
        }
    }
    changed.recompute_id().expect("recompute");
    assert_ne!(
        changed.id, baseline.id,
        "metric definition version must affect identity"
    );

    let mut changed = measured();
    changed.evidence.config = Some(digest('8'));
    changed.recompute_id().expect("recompute");
    assert_ne!(changed.id, baseline.id);

    let mut changed = measured();
    changed.state = MeasurementStateV1::Excluded {
        reason: ExclusionReasonV1::ArtifactMalformed,
    };
    changed.recompute_id().expect("recompute");
    assert_ne!(changed.id, baseline.id);
}

#[test]
fn deserialized_content_cannot_keep_a_stale_id() {
    let mut observation = measured();
    observation.original_task_id = "tampered".to_string();
    assert!(observation.verify_id().is_err());
}

#[test]
fn calibration_evidence_must_be_complete() {
    let evidence = CalibrationEvidenceV1 {
        method: CalibrationMethod::HumanAdjudicatedBenchmark,
        protocol_version: String::new(),
        corpus_sha256: digest('7'),
        sample_size: 0,
        criteria: Vec::new(),
        conclusion: CalibrationConclusion::Calibrated,
    };
    assert!(evidence.validate().is_err());
}

#[test]
fn non_finite_metric_values_are_refused_before_hashing() {
    let mut candidate = measured();
    candidate.state = MeasurementStateV1::Measured {
        held_out_success: true,
        held_out_provenance: HeldOutProvenance::NativeComposed,
        calibration: CalibrationEvidenceV1 {
            method: CalibrationMethod::ExternalOutcomeCorrelation,
            protocol_version: "r11".to_string(),
            corpus_sha256: digest('7'),
            sample_size: 20,
            criteria: vec!["rho-significant".to_string()],
            conclusion: CalibrationConclusion::Calibrated,
        },
        metrics: Box::new(aoa_bench::MeasurementMetricsV1::Answer(AnswerMetricsV1 {
            trace_locality: MetricValueV1::new("trace-locality-v1", f64::NAN),
            trace_reach_depth: MetricValueV1::new("trace-reach-depth-v1", 2),
        })),
    };

    assert!(candidate.recompute_id().is_err());
}
