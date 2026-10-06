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

    let configured = match configured_worktree(git_dir)? {
        ConfiguredWorktree::Stated(configured) => configured,
        ConfiguredWorktree::Unstated => {
            return Ok(Some(SubmoduleRejection::WorktreeUnstated {
                git_dir: git_dir.to_path_buf(),
            }));
        }
        ConfiguredWorktree::Unreadable(rejection) => {
            return Ok(Some(SubmoduleRejection::Command(rejection)));
        }
    };
    if configured_worktree_is(git_dir, &configured, candidate)? {
        Ok(None)
    } else {
        Ok(Some(SubmoduleRejection::WorktreeMismatch {
            git_dir: git_dir.to_path_buf(),
            candidate: candidate.to_path_buf(),
            configured,
        }))
    }
}

fn configured_worktree_is(
    git_dir: &Path,
    configured: &str,
    candidate: &Path,
) -> Result<bool, RepositoryRootError> {
    let named = git_dir.join(configured);
    match named.canonicalize() {
        Ok(worktree) => Ok(worktree == candidate),
        Err(absent) if absent.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(RepositoryRootError::Unresolvable {
            path: named,
            source,
        }),
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

enum ConfiguredWorktree {
    Stated(String),
    Unstated,
    Unreadable(GitCommandRejection),
}

const GIT_CONFIG_KEY_UNSET: i32 = 1;

fn configured_worktree(git_dir: &Path) -> Result<ConfiguredWorktree, RepositoryRootError> {
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
    if output.status.code() == Some(GIT_CONFIG_KEY_UNSET) {
        return Ok(ConfiguredWorktree::Unstated);
    }
    if !output.status.success() {
        return Ok(ConfiguredWorktree::Unreadable(GitCommandRejection::new(
            CORE_WORKTREE,
            output.status,
            &output.stderr,
        )));
    }
    git_reported_path(&output.stdout, CORE_WORKTREE)
        .map(|worktree| ConfiguredWorktree::Stated(worktree.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git_dir_with_config(config: &str) -> (tempfile::TempDir, PathBuf) {
        let fixture = tempfile::tempdir().unwrap();
        let git_dir = fixture.path().canonicalize().unwrap().join("modules/sub");
        std::fs::create_dir_all(&git_dir).unwrap();
        std::fs::write(git_dir.join("config"), config).unwrap();
        (fixture, git_dir)
    }

    #[test]
    fn a_config_that_states_the_worktree_reports_it() {
        let (_fixture, git_dir) = git_dir_with_config("[core]\n\tworktree = ../../../sub\n");

        let configured = configured_worktree(&git_dir).unwrap();

        assert!(
            matches!(&configured, ConfiguredWorktree::Stated(worktree) if worktree == "../../../sub"),
        );
    }

    #[test]
    fn a_config_without_the_key_is_unstated() {
        let (_fixture, git_dir) = git_dir_with_config("[core]\n\tbare = false\n");

        assert!(matches!(
            configured_worktree(&git_dir).unwrap(),
            ConfiguredWorktree::Unstated
        ));
    }

    #[test]
    fn a_config_git_cannot_read_is_a_command_rejection_not_an_unstated_worktree() {
        let (_fixture, git_dir) = git_dir_with_config("[[[\n");

        let configured = configured_worktree(&git_dir).unwrap();

        let ConfiguredWorktree::Unreadable(rejection) = configured else {
            panic!("a malformed config must be reported as a rejection");
        };
        let message = rejection.to_string();
        assert!(message.contains(CORE_WORKTREE), "{message}");
        assert!(message.contains("bad config"), "{message}");
    }

    #[test]
    fn a_configured_worktree_that_does_not_exist_is_a_mismatch() {
        let (_fixture, git_dir) = git_dir_with_config("");
        let candidate = git_dir.parent().unwrap().to_path_buf();

        assert!(!configured_worktree_is(&git_dir, "../gone", &candidate).unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn a_configured_worktree_that_cannot_be_resolved_is_an_error_naming_the_path() {
        let (_fixture, git_dir) = git_dir_with_config("");
        let modules = git_dir.parent().unwrap();
        std::os::unix::fs::symlink("looped", modules.join("looped")).unwrap();

        let refusal = configured_worktree_is(&git_dir, "../looped", modules).unwrap_err();

        let RepositoryRootError::Unresolvable { path, source } = &refusal else {
            panic!("{refusal}");
        };
        assert_eq!(path, &git_dir.join("../looped"));
        assert_eq!(
            source.raw_os_error(),
            Some(rustix::io::Errno::LOOP.raw_os_error())
        );
    }
}
