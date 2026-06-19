use std::path::Path;

use crate::tier::EnforcementPlane;

/// The candidate paths probed for each enforcement plane. A plane is present if
/// any of its candidates exists; absent otherwise. These are purely structural
/// file-existence checks — the audit never reads or interprets their contents.
fn candidates(plane: EnforcementPlane) -> &'static [&'static str] {
    match plane {
        EnforcementPlane::RuntimeHook => &[".aoa/hooks.toml", ".claude/settings.json"],
        EnforcementPlane::PreCommit => &[".pre-commit-config.yaml", ".git/hooks/pre-commit"],
        EnforcementPlane::Ci => &[
            ".github/workflows",
            ".gitlab-ci.yml",
            ".circleci/config.yml",
        ],
    }
}

/// Whether `plane` is structurally present in `repo`.
fn present(repo: &Path, plane: EnforcementPlane) -> bool {
    candidates(plane).iter().any(|rel| repo.join(rel).exists())
}

/// Return the enforcement planes that are structurally absent from `repo`, in
/// declaration order. Each absent plane becomes a punch-list item.
pub fn missing_planes(repo: &Path) -> Vec<EnforcementPlane> {
    [
        EnforcementPlane::RuntimeHook,
        EnforcementPlane::PreCommit,
        EnforcementPlane::Ci,
    ]
    .into_iter()
    .filter(|plane| !present(repo, *plane))
    .collect()
}
