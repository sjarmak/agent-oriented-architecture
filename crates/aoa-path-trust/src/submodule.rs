use std::fmt;
use std::path::{Path, PathBuf};

use super::git_environment::git_free_of_inherited_state;
use super::root::{
    git_reported_path, resolve_repository_root, GitCommandRejection, GitPathResolution,
    RepositoryRootError,
};

const CORE_WORKTREE: &str = "core.worktree";

#[derive(Debug)]
pub(super) enum SubmoduleRejection {
    Command(GitCommandRejection),
    NoSuperproject {
        git_dir: PathBuf,
        reason: String,
    },
    OutsideSuperprojectModules {
        git_dir: PathBuf,
        superproject_git_dir: PathBuf,
    },
    WorktreeUnstated {
        git_dir: PathBuf,
    },
    WorktreeMismatch {
        git_dir: PathBuf,
        candidate: PathBuf,
        configured: String,
    },
}

impl fmt::Display for SubmoduleRejection {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Command(rejection) => rejection.fmt(formatter),
            Self::NoSuperproject { git_dir, reason } => write!(
                formatter,
                "Git submodule layout is invalid: --git-dir reported {}, and no validated superproject encloses the marker: {reason}",
                git_dir.display()
            ),
            Self::OutsideSuperprojectModules {
                git_dir,
                superproject_git_dir,
            } => write!(
                formatter,
                "Git submodule layout is invalid: --git-dir reported {}, which is not under {}/modules from the superproject's --git-dir",
                git_dir.display(),
                superproject_git_dir.display()
            ),
            Self::WorktreeUnstated { git_dir } => write!(
                formatter,
                "Git submodule layout is invalid: {}/config states no {CORE_WORKTREE}",
                git_dir.display()
            ),
            Self::WorktreeMismatch {
                git_dir,
                candidate,
                configured,
            } => write!(
                formatter,
                "Git submodule layout is invalid: {CORE_WORKTREE} in {}/config is {configured}, which does not resolve to {}",
                git_dir.display(),
                candidate.display()
            ),
        }
    }
}

pub(super) fn submodule_rejection(
    candidate: &Path,
    git_dir: &Path,
    resolve: &mut impl FnMut(&Path, &'static str) -> Result<GitPathResolution, RepositoryRootError>,
) -> Result<Option<SubmoduleRejection>, RepositoryRootError> {
    let superproject = match enclosing_repository_root(candidate) {
        Ok(superproject) => superproject,
        Err(
            refusal @ (RepositoryRootError::NotAGitRoot { .. }
            | RepositoryRootError::NotInRepository { .. }),
        ) => {
            return Ok(Some(SubmoduleRejection::NoSuperproject {
                git_dir: git_dir.to_path_buf(),
                reason: refusal.to_string(),
            }));
        }
        Err(other) => return Err(other),
    };
    let superproject_git_dir = match resolve(&superproject, "--git-dir")? {
        Ok(path) => path,
        Err(rejection) => return Ok(Some(SubmoduleRejection::Command(rejection))),
    };
    let names_a_module = git_dir
        .strip_prefix(superproject_git_dir.join("modules"))
        .is_ok_and(|name| name.components().next().is_some());
    if !names_a_module {
        return Ok(Some(SubmoduleRejection::OutsideSuperprojectModules {
            git_dir: git_dir.to_path_buf(),
            superproject_git_dir,
        }));
    }

    let Some(configured) = configured_worktree(git_dir)? else {
        return Ok(Some(SubmoduleRejection::WorktreeUnstated {
            git_dir: git_dir.to_path_buf(),
        }));
    };
    let points_back = git_dir
        .join(&configured)
        .canonicalize()
        .is_ok_and(|worktree| worktree == candidate);
    if points_back {
        Ok(None)
    } else {
        Ok(Some(SubmoduleRejection::WorktreeMismatch {
            git_dir: git_dir.to_path_buf(),
            candidate: candidate.to_path_buf(),
            configured,
        }))
    }
}

fn enclosing_repository_root(candidate: &Path) -> Result<PathBuf, RepositoryRootError> {
    match candidate.parent() {
        Some(parent) => resolve_repository_root(parent),
        None => Err(RepositoryRootError::NotInRepository {
            path: candidate.to_path_buf(),
        }),
    }
}

fn configured_worktree(git_dir: &Path) -> Result<Option<String>, RepositoryRootError> {
    let output = git_free_of_inherited_state()
        .arg("-C")
        .arg(git_dir)
        .args(["config", "--file"])
        .arg(git_dir.join("config"))
        .args(["--get", CORE_WORKTREE])
        .output()
        .map_err(|source| RepositoryRootError::GitUnavailable {
            candidate: git_dir.to_path_buf(),
            source,
        })?;
    if !output.status.success() {
        return Ok(None);
    }
    git_reported_path(&output.stdout, CORE_WORKTREE).map(|worktree| Some(worktree.to_owned()))
}
