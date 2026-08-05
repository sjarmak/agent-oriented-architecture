use std::collections::BTreeSet;

use aoa_gap::{ExposureStatus, SubjectKey};

fn subject(name: &str) -> SubjectKey {
    SubjectKey {
        repo_id: "httpie".to_string(),
        baseline_commit: "5b604c37".to_string(),
        oracle_target_symbol: name.to_string(),
        question_family: "dependency_analysis".to_string(),
    }
}

#[test]
fn partial_exposure_carries_the_exact_spent_subjects() {
    let spent = BTreeSet::from([
        subject("httpie.compat.func"),
        subject("httpie.__main__.main"),
    ]);
    let status = ExposureStatus::PartiallyExposed {
        subjects: spent.clone(),
    };

    assert!(matches!(
        &status,
        ExposureStatus::PartiallyExposed { subjects } if subjects == &spent
    ));
    assert!(!status.is_unexposed());
}

#[test]
fn unexposed_is_the_only_admissible_status() {
    assert!(ExposureStatus::Unexposed.is_unexposed());
    assert!(!ExposureStatus::Exposed.is_unexposed());
}
