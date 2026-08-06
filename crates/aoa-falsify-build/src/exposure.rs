//! Binds the persisted exposure ledger to the repo the gate is voting on.
//!
//! The ledger is the typed output of `aoa eval exposure scan --out`. Reading it
//! here is what makes the anti-leakage check load-bearing: the alternative this
//! replaced was a hand-typed word in the manifest, which asserted a verdict no
//! artifact had to agree with.

use std::path::Path;

use anyhow::{anyhow, bail, Context, Result as AnyResult};
use aoa_bench::{ExposureScan, GitObjectId};
use aoa_gap::ExposureStatus;

use crate::evidence::MAX_EVIDENCE_BYTES;

/// Shortest revision fragment accepted as an identity, matching git's own
/// default abbreviation length.
const MIN_BASELINE_ABBREV: usize = 7;

/// Derive `repo_id`'s exposure status from the persisted ledger at `scan_path`.
///
/// Every failure is an error rather than a degraded value: an absent, stale, or
/// mismatched ledger must never resolve toward eligibility, because `Unexposed`
/// is precisely the verdict that lets a repo vote.
pub(crate) fn resolve_exposure(
    scan_path: &Path,
    repo_id: &str,
    repo_commit: &GitObjectId,
) -> AnyResult<ExposureStatus> {
    let metadata = std::fs::symlink_metadata(scan_path).with_context(|| {
        format!(
            "repo {repo_id}: cannot read exposure ledger {}",
            scan_path.display()
        )
    })?;
    if !metadata.file_type().is_file() {
        bail!(
            "repo {repo_id}: exposure ledger {} is not a regular file",
            scan_path.display()
        );
    }
    if metadata.len() > MAX_EVIDENCE_BYTES {
        bail!(
            "repo {repo_id}: exposure ledger {} exceeds the {MAX_EVIDENCE_BYTES} byte evidence cap",
            scan_path.display()
        );
    }
    let bytes = std::fs::read(scan_path).with_context(|| {
        format!(
            "repo {repo_id}: cannot read exposure ledger {}",
            scan_path.display()
        )
    })?;
    let scan: ExposureScan = serde_json::from_slice(&bytes).with_context(|| {
        format!(
            "repo {repo_id}: exposure ledger {} is malformed",
            scan_path.display()
        )
    })?;

    // Exactly one entry, not the first match: `scan_exposure` rejects duplicate
    // repos, so two entries mean a ledger that was assembled by some other means
    // and it is no longer clear which verdict describes this repo. Resolving that
    // silently would be the same "a word decides" failure in a new place.
    let mut matching = scan.repos.iter().filter(|entry| entry.repo_id == repo_id);
    let entry = matching.next().ok_or_else(|| {
        anyhow!(
            "repo {repo_id}: exposure ledger {} has no exposure entry for it; \
             re-run `aoa eval exposure scan --out` against the runs root that \
             holds this repo's trials",
            scan_path.display()
        )
    })?;
    if matching.next().is_some() {
        bail!(
            "repo {repo_id}: exposure ledger {} carries more than one entry for it, \
             so no single measured verdict describes this repo",
            scan_path.display()
        );
    }
    // codeprobe records `prep.json`'s `baseline_sha`, conventionally abbreviated,
    // while the manifest pins a full object id — so this is prefix identification,
    // not string equality. The length floor is what keeps it an identity check: a
    // shorter fragment would match many revisions and admit a ledger scanned
    // against the wrong one.
    let baseline = entry.baseline_commit.to_ascii_lowercase();
    if baseline.len() < MIN_BASELINE_ABBREV || !baseline.bytes().all(|b| b.is_ascii_hexdigit()) {
        // Quoted and escaped: a display-hostile value must not reshape the
        // diagnostic it appears in.
        bail!(
            "repo {repo_id}: exposure ledger {} records baseline commit \"{}\", which cannot \
             identify a revision: at least {MIN_BASELINE_ABBREV} hex characters are required",
            scan_path.display(),
            entry.baseline_commit.escape_default()
        );
    }
    if !repo_commit.hex.starts_with(&baseline) {
        bail!(
            "repo {repo_id}: exposure ledger {} was scanned at baseline commit {} but the \
             manifest declares repo_commit {}; the ledger does not describe the revision \
             being measured",
            scan_path.display(),
            baseline,
            repo_commit.hex
        );
    }
    Ok(entry.status.clone())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use aoa_bench::{GitHashAlgorithm, GitObjectId};
    use aoa_gap::ExposureStatus;

    use super::resolve_exposure;

    const HEX: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const OTHER_HEX: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    fn commit(hex: &str) -> GitObjectId {
        GitObjectId::parse(GitHashAlgorithm::Sha1, hex).unwrap()
    }

    fn scan_json(repo_id: &str, baseline_commit: &str, status: &str) -> String {
        format!(
            r#"{{"repos":[{{"repo_id":"{repo_id}","baseline_commit":"{baseline_commit}",
            "total_subjects":2,"status":{status},"provenance":null}}]}}"#
        )
    }

    fn write_scan(dir: &Path, body: &str) -> std::path::PathBuf {
        let path = dir.join("exposure.json");
        std::fs::write(&path, body).unwrap();
        path
    }

    #[test]
    fn derives_the_status_of_the_matching_repo_and_commit() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_scan(dir.path(), &scan_json("sample/repo", HEX, r#""unexposed""#));

        let status = resolve_exposure(&path, "sample/repo", &commit(HEX)).unwrap();

        assert_eq!(status, ExposureStatus::Unexposed);
    }

    #[test]
    fn derives_a_partially_exposed_status_with_its_subjects() {
        let dir = tempfile::tempdir().unwrap();
        let status_json = r#"{"partially_exposed":{"subjects":[{"repo_id":"sample/repo",
            "baseline_commit":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "oracle_target_symbol":"pkg.app.main","question_family":"dependency_analysis"}]}}"#;
        let path = write_scan(dir.path(), &scan_json("sample/repo", HEX, status_json));

        let status = resolve_exposure(&path, "sample/repo", &commit(HEX)).unwrap();

        let ExposureStatus::PartiallyExposed { subjects } = status else {
            panic!("expected partially exposed, got {status:?}");
        };
        assert_eq!(subjects.len(), 1);
    }

    #[test]
    fn a_repo_absent_from_the_ledger_is_an_error_not_an_unexposed_default() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_scan(dir.path(), &scan_json("other/repo", HEX, r#""unexposed""#));

        let error = resolve_exposure(&path, "sample/repo", &commit(HEX)).unwrap_err();

        let error = format!("{error:#}");
        assert!(error.contains("sample/repo"), "got: {error}");
        assert!(
            error.contains("has no exposure entry for it"),
            "got: {error}"
        );
    }

    #[test]
    fn a_ledger_carrying_two_entries_for_one_repo_is_ambiguous_not_first_wins() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("exposure.json");
        std::fs::write(
            &path,
            format!(
                r#"{{"repos":[
                {{"repo_id":"sample/repo","baseline_commit":"{HEX}","total_subjects":2,
                 "status":"unexposed","provenance":null}},
                {{"repo_id":"sample/repo","baseline_commit":"{HEX}","total_subjects":2,
                 "status":"exposed","provenance":null}}]}}"#
            ),
        )
        .unwrap();

        let error = resolve_exposure(&path, "sample/repo", &commit(HEX)).unwrap_err();

        assert!(
            format!("{error:#}").contains("more than one entry"),
            "got: {error:#}"
        );
    }

    #[test]
    fn an_abbreviated_baseline_revision_identifies_the_pinned_commit() {
        // codeprobe records `prep.json`'s `baseline_sha`, which is conventionally
        // abbreviated, while the manifest pins a full object id. Requiring exact
        // equality would reject every real pairing and push operators back to
        // hand-editing — the very thing the ledger binding removes.
        let dir = tempfile::tempdir().unwrap();
        let path = write_scan(
            dir.path(),
            &scan_json("sample/repo", &HEX[..8], r#""unexposed""#),
        );

        let status = resolve_exposure(&path, "sample/repo", &commit(HEX)).unwrap();

        assert_eq!(status, ExposureStatus::Unexposed);
    }

    #[test]
    fn a_baseline_revision_too_short_to_identify_anything_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_scan(
            dir.path(),
            &scan_json("sample/repo", &HEX[..6], r#""unexposed""#),
        );

        let error = resolve_exposure(&path, "sample/repo", &commit(HEX)).unwrap_err();

        assert!(
            format!("{error:#}").contains("cannot identify"),
            "got: {error:#}"
        );
    }

    #[test]
    fn a_non_hex_baseline_revision_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_scan(
            dir.path(),
            &scan_json("sample/repo", "not-a-sha", r#""unexposed""#),
        );

        let error = resolve_exposure(&path, "sample/repo", &commit(HEX)).unwrap_err();

        assert!(
            format!("{error:#}").contains("cannot identify"),
            "got: {error:#}"
        );
    }

    #[test]
    fn an_abbreviation_of_another_revision_is_still_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_scan(
            dir.path(),
            &scan_json("sample/repo", &OTHER_HEX[..8], r#""unexposed""#),
        );

        let error = resolve_exposure(&path, "sample/repo", &commit(HEX)).unwrap_err();

        assert!(
            format!("{error:#}").contains("does not describe the revision being measured"),
            "got: {error:#}"
        );
    }

    #[test]
    fn a_ledger_scanned_at_a_different_baseline_commit_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_scan(
            dir.path(),
            &scan_json("sample/repo", OTHER_HEX, r#""unexposed""#),
        );

        let error = resolve_exposure(&path, "sample/repo", &commit(HEX)).unwrap_err();

        let error = format!("{error:#}");
        assert!(error.contains(OTHER_HEX), "got: {error}");
        assert!(error.contains(HEX), "got: {error}");
    }

    #[test]
    fn a_missing_or_malformed_ledger_fails_loud() {
        let dir = tempfile::tempdir().unwrap();

        let missing =
            resolve_exposure(&dir.path().join("absent.json"), "sample/repo", &commit(HEX))
                .unwrap_err();
        assert!(
            format!("{missing:#}").contains("absent.json"),
            "got: {missing:#}"
        );

        let path = write_scan(dir.path(), "{\"repos\":[{\"repo_id\":\"sample/repo\"}]}");
        let malformed = resolve_exposure(&path, "sample/repo", &commit(HEX)).unwrap_err();
        assert!(
            format!("{malformed:#}").contains("exposure.json"),
            "got: {malformed:#}"
        );
    }

    #[test]
    fn a_directory_in_place_of_the_ledger_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("scan_dir");
        std::fs::create_dir(&path).unwrap();

        let error = resolve_exposure(&path, "sample/repo", &commit(HEX)).unwrap_err();

        assert!(
            format!("{error:#}").contains("regular file"),
            "got: {error:#}"
        );
    }
}
