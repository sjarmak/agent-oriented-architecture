//! Where a trust root comes from: resolving a caller-supplied directory to the
//! Git worktree root that containment is then measured against.
//!
//! The rest of [`super`] answers "does this path stay under the root". This
//! answers "which root", and the two halves belong together: a host that gets
//! the containment checks for free would otherwise have to re-derive the anchor
//! they are relative to, and an anchor derived two ways is two answers.
//!
//! [`resolve_repository_root`] states the checks it makes and in what order.
//! Every one of them refuses; none of them widens what counts as a root.

use std::fmt;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};

/// Why a candidate directory yielded no repository trust root.
///
/// Every variant is a refusal or an inconclusive answer; none of them is a root.
/// A caller that cannot tell these apart still cannot mistake one for success.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum RepositoryRootError {
    /// The candidate was relative, so resolving it would consult the process
    /// directory — ambient state the anchor must not depend on.
    #[error("repository trust root must resolve from an absolute path, got {path}")]
    RelativeCandidate { path: PathBuf },

    /// A path did not resolve. Covers the candidate itself and the path Git
    /// reports back for it.
    #[error("failed to resolve {path}: {source}")]
    Unresolvable {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// The candidate resolved to something that is not a directory.
    #[error("{path} is not a directory")]
    NotADirectory { path: PathBuf },

    /// The marker itself is the thing being trusted, so a symlinked one is
    /// refused outright rather than followed to whatever it names.
    #[error("refusing repository root with symlinked marker at {marker}")]
    SymlinkedMarker { marker: PathBuf },

    /// The filesystem could not say whether a marker is there. Never read as
    /// absent: a check that fails open is not a check.
    #[error("failed to inspect {marker}: {source}")]
    Inspect {
        marker: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// A marker was found, but Git does not agree that its directory is the
    /// worktree root.
    #[error("Git repository validation failed for {marker}: {reason}")]
    NotAGitRoot { marker: PathBuf, reason: String },

    /// No ancestor carried a marker at all.
    #[error("{path} is not inside a Git repository")]
    NotInRepository { path: PathBuf },

    /// Git could not be run. Distinct from Git answering "no".
    #[error("failed to run git while validating {candidate}: {source}")]
    GitUnavailable {
        candidate: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// Git answered with bytes that are not a path we can compare.
    #[error("git returned a non-UTF-8 path for {field}")]
    NonUtf8GitPath { field: &'static str },

    /// A linked worktree's backlink could not be read.
    #[error("failed to read linked-worktree backlink at {path}: {source}")]
    Backlink {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// A linked worktree's backlink is not a path we can compare.
    #[error("linked-worktree backlink at {path} is not UTF-8")]
    NonUtf8Backlink { path: PathBuf },
}

/// Resolve `candidate` to the Git worktree root that governs it.
///
/// Canonicalization proves the directory exists and collapses aliases, then the
/// nearest non-symlink `.git` marker pins the answer to a real repository root
/// rather than an arbitrary caller-supplied path. A `.git` file is accepted for
/// linked worktrees, but only after its administrative directory and backlink
/// are confirmed to point back at this exact worktree.
///
/// The candidate must be absolute; see [`RepositoryRootError::RelativeCandidate`].
pub fn resolve_repository_root(candidate: &Path) -> Result<PathBuf, RepositoryRootError> {
    if !candidate.is_absolute() {
        return Err(RepositoryRootError::RelativeCandidate {
            path: candidate.to_path_buf(),
        });
    }
    let canonical =
        candidate
            .canonicalize()
            .map_err(|source| RepositoryRootError::Unresolvable {
                path: candidate.to_path_buf(),
                source,
            })?;
    if !canonical.is_dir() {
        return Err(RepositoryRootError::NotADirectory { path: canonical });
    }

    let mut nearest_rejection = None;
    for ancestor in canonical.ancestors() {
        let marker = ancestor.join(".git");
        match std::fs::symlink_metadata(&marker) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(RepositoryRootError::SymlinkedMarker { marker });
            }
            Ok(metadata) => match git_candidate_is_root(ancestor, metadata.is_dir())? {
                None => return Ok(ancestor.to_path_buf()),
                Some(rejection) => {
                    nearest_rejection.get_or_insert((marker, rejection));
                }
            },
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(source) => return Err(RepositoryRootError::Inspect { marker, source }),
        }
    }
    if let Some((marker, rejection)) = nearest_rejection {
        return Err(RepositoryRootError::NotAGitRoot {
            marker,
            reason: rejection.to_string(),
        });
    }
    Err(RepositoryRootError::NotInRepository { path: canonical })
}

/// Ask Git to validate the marker and require it to identify this exact
/// canonical directory as the worktree root.
fn git_candidate_is_root(
    candidate: &Path,
    marker_is_dir: bool,
) -> Result<Option<GitCandidateRejection>, RepositoryRootError> {
    git_candidate_is_root_with(candidate, marker_is_dir, &mut git_resolved_path)
}

fn git_candidate_is_root_with(
    candidate: &Path,
    marker_is_dir: bool,
    resolve: &mut impl FnMut(&Path, &'static str) -> Result<GitPathResolution, RepositoryRootError>,
) -> Result<Option<GitCandidateRejection>, RepositoryRootError> {
    let reported = match resolve(candidate, "--show-toplevel")? {
        Ok(path) => path,
        Err(rejection) => {
            return Ok(Some(GitCandidateRejection::Command(rejection)));
        }
    };
    if reported != candidate {
        return Ok(Some(GitCandidateRejection::RootMismatch {
            expected: candidate.to_path_buf(),
            reported,
        }));
    }
    if marker_is_dir {
        return Ok(None);
    }

    let git_dir = match resolve(candidate, "--git-dir")? {
        Ok(path) => path,
        Err(rejection) => {
            return Ok(Some(GitCandidateRejection::Command(rejection)));
        }
    };
    let common_dir = match resolve(candidate, "--git-common-dir")? {
        Ok(path) => path,
        Err(rejection) => {
            return Ok(Some(GitCandidateRejection::Command(rejection)));
        }
    };
    if git_dir.parent() != Some(common_dir.join("worktrees").as_path()) {
        return Ok(Some(GitCandidateRejection::LinkedWorktreeLayout {
            git_dir,
            common_dir,
        }));
    }
    if linked_worktree_points_back(candidate, &git_dir)? {
        Ok(None)
    } else {
        Ok(Some(GitCandidateRejection::BacklinkMismatch {
            candidate: candidate.to_path_buf(),
            git_dir,
        }))
    }
}

type GitPathResolution = std::result::Result<PathBuf, GitCommandRejection>;

#[derive(Debug)]
struct GitCommandRejection {
    field: &'static str,
    status: ExitStatus,
    stderr: String,
}

#[derive(Debug)]
enum GitCandidateRejection {
    Command(GitCommandRejection),
    RootMismatch {
        expected: PathBuf,
        reported: PathBuf,
    },
    LinkedWorktreeLayout {
        git_dir: PathBuf,
        common_dir: PathBuf,
    },
    BacklinkMismatch {
        candidate: PathBuf,
        git_dir: PathBuf,
    },
}

impl fmt::Display for GitCandidateRejection {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Command(rejection) => rejection.fmt(formatter),
            Self::RootMismatch { expected, reported } => write!(
                formatter,
                "Git check --show-toplevel reported {} instead of candidate {}",
                reported.display(),
                expected.display()
            ),
            Self::LinkedWorktreeLayout {
                git_dir,
                common_dir,
            } => write!(
                formatter,
                "Git linked-worktree layout is invalid: --git-dir reported {}, which is not directly under {}/worktrees from --git-common-dir",
                git_dir.display(),
                common_dir.display()
            ),
            Self::BacklinkMismatch { candidate, git_dir } => write!(
                formatter,
                "linked-worktree backlink at {}/gitdir does not point to {}/.git",
                git_dir.display(),
                candidate.display()
            ),
        }
    }
}

impl fmt::Display for GitCommandRejection {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "Git check {} failed with status {}",
            self.field, self.status
        )?;
        if !self.stderr.is_empty() {
            write!(formatter, ": {}", self.stderr)
        } else {
            write!(formatter, " (no stderr)")
        }
    }
}

fn git_resolved_path(
    candidate: &Path,
    field: &'static str,
) -> Result<GitPathResolution, RepositoryRootError> {
    // Git's complete `rev-parse --local-env-vars` set, plus the two discovery
    // controls it omits. Any one of these must describe the candidate itself,
    // never ambient state inherited from the host.
    const REPOSITORY_ENV: [&str; 17] = [
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        "GIT_CONFIG",
        "GIT_CONFIG_PARAMETERS",
        "GIT_CONFIG_COUNT",
        "GIT_OBJECT_DIRECTORY",
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_IMPLICIT_WORK_TREE",
        "GIT_GRAFT_FILE",
        "GIT_INDEX_FILE",
        "GIT_NO_REPLACE_OBJECTS",
        "GIT_REPLACE_REF_BASE",
        "GIT_PREFIX",
        "GIT_SHALLOW_FILE",
        "GIT_COMMON_DIR",
        "GIT_CEILING_DIRECTORIES",
        "GIT_DISCOVERY_ACROSS_FILESYSTEM",
    ];
    let mut command = Command::new("git");
    for variable in REPOSITORY_ENV {
        command.env_remove(variable);
    }
    let output = command
        .arg("-C")
        .arg(candidate)
        .args(["rev-parse", "--path-format=absolute", field])
        .output()
        .map_err(|source| RepositoryRootError::GitUnavailable {
            candidate: candidate.to_path_buf(),
            source,
        })?;
    if !output.status.success() {
        return Ok(Err(GitCommandRejection {
            field,
            status: output.status,
            stderr: String::from_utf8_lossy(&output.stderr)
                .trim_end()
                .to_owned(),
        }));
    }

    let reported = std::str::from_utf8(&output.stdout)
        .map_err(|_| RepositoryRootError::NonUtf8GitPath { field })?
        .trim_end();
    let reported =
        Path::new(reported)
            .canonicalize()
            .map_err(|source| RepositoryRootError::Unresolvable {
                path: PathBuf::from(reported),
                source,
            })?;
    Ok(Ok(reported))
}

fn linked_worktree_points_back(
    candidate: &Path,
    git_dir: &Path,
) -> Result<bool, RepositoryRootError> {
    const MAX_BACKLINK_BYTES: u64 = 4096;
    let backlink_path = git_dir.join("gitdir");
    let mut raw = Vec::new();
    File::open(&backlink_path)
        .and_then(|file| file.take(MAX_BACKLINK_BYTES + 1).read_to_end(&mut raw))
        .map_err(|source| RepositoryRootError::Backlink {
            path: backlink_path.clone(),
            source,
        })?;
    if raw.len() as u64 > MAX_BACKLINK_BYTES {
        return Ok(false);
    }
    let backlink = std::str::from_utf8(&raw)
        .map_err(|_| RepositoryRootError::NonUtf8Backlink {
            path: backlink_path,
        })?
        .trim_end();
    let backlink = Path::new(backlink);
    let backlink = if backlink.is_absolute() {
        backlink.to_path_buf()
    } else {
        git_dir.join(backlink)
    };
    // Either side failing to resolve is a mismatch. Comparing the two
    // `canonicalize().ok()` values instead reads as equal when *both* fail,
    // accepting a backlink that points nowhere as pointing back.
    let (Ok(resolved_backlink), Ok(marker)) = (
        backlink.canonicalize(),
        candidate.join(".git").canonicalize(),
    ) else {
        return Ok(false);
    };
    Ok(resolved_backlink == marker)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn init_git_repo(path: &Path) {
        let status = Command::new("git")
            .args(["init", "--quiet"])
            .arg(path)
            .status()
            .expect("git is available for repository-boundary tests");
        assert!(status.success(), "git init failed for test fixture");
    }

    fn commit_and_add_worktree(main: &Path, worktree: &Path) {
        init_git_repo(main);
        let status = Command::new("git")
            .arg("-C")
            .arg(main)
            .args([
                "-c",
                "user.name=AOA Test",
                "-c",
                "user.email=aoa@example.invalid",
                "commit",
                "--quiet",
                "--allow-empty",
                "-m",
                "fixture",
            ])
            .status()
            .unwrap();
        assert!(status.success(), "worktree fixture commit must succeed");
        let status = Command::new("git")
            .arg("-C")
            .arg(main)
            .args(["worktree", "add", "--quiet", "--detach"])
            .arg(worktree)
            .status()
            .unwrap();
        assert!(status.success(), "linked worktree must initialize");
    }

    fn candidate_fixture() -> (tempfile::TempDir, PathBuf) {
        let fixture = tempfile::tempdir().unwrap();
        let candidate = fixture.path().canonicalize().unwrap();
        (fixture, candidate)
    }

    fn rejection_message(
        candidate: &Path,
        marker_is_dir: bool,
        resolutions: Vec<(&'static str, GitPathResolution)>,
    ) -> String {
        let mut resolutions = resolutions.into_iter();
        let mut resolve = |_: &Path, field| {
            let (expected, resolution) = resolutions.next().expect("unexpected Git check");
            assert_eq!(field, expected);
            Ok(resolution)
        };
        git_candidate_is_root_with(candidate, marker_is_dir, &mut resolve)
            .unwrap()
            .expect("candidate must be rejected")
            .to_string()
    }

    fn rejected_git_path(field: &'static str, stderr: &str) -> GitPathResolution {
        let status = Command::new("git")
            .arg("--definitely-not-a-real-option")
            .output()
            .unwrap()
            .status;
        Err(GitCommandRejection {
            field,
            status,
            stderr: stderr.to_string(),
        })
    }

    #[test]
    fn git_candidate_reports_show_toplevel_failure() {
        let (_fixture, candidate) = candidate_fixture();
        let message = rejection_message(
            &candidate,
            true,
            vec![(
                "--show-toplevel",
                rejected_git_path(
                    "--show-toplevel",
                    "fatal: detected dubious ownership in repository",
                ),
            )],
        );
        assert!(message.contains("--show-toplevel"), "{message}");
        assert!(message.contains("dubious ownership"), "{message}");
    }

    #[test]
    fn git_candidate_reports_mismatched_toplevel() {
        let (_fixture, candidate) = candidate_fixture();
        let other = candidate.join("other");
        std::fs::create_dir(&other).unwrap();
        let message = rejection_message(
            &candidate,
            true,
            vec![("--show-toplevel", Ok(other.clone()))],
        );
        assert!(message.contains("--show-toplevel"), "{message}");
        assert!(message.contains(&other.display().to_string()), "{message}");
    }

    #[test]
    fn git_candidate_reports_git_dir_failure() {
        let (_fixture, candidate) = candidate_fixture();
        let message = rejection_message(
            &candidate,
            false,
            vec![
                ("--show-toplevel", Ok(candidate.clone())),
                (
                    "--git-dir",
                    rejected_git_path("--git-dir", "fatal: corrupt config"),
                ),
            ],
        );
        assert!(message.contains("--git-dir"), "{message}");
        assert!(message.contains("corrupt config"), "{message}");
    }

    #[test]
    fn git_candidate_reports_common_dir_failure() {
        let (_fixture, candidate) = candidate_fixture();
        let git_dir = candidate.join("admin/worktrees/fixture");
        let message = rejection_message(
            &candidate,
            false,
            vec![
                ("--show-toplevel", Ok(candidate.clone())),
                ("--git-dir", Ok(git_dir.clone())),
                (
                    "--git-common-dir",
                    rejected_git_path("--git-common-dir", "fatal: invalid common directory"),
                ),
            ],
        );
        assert!(message.contains("--git-common-dir"), "{message}");
        assert!(message.contains("invalid common directory"), "{message}");
    }

    #[test]
    fn git_candidate_reports_invalid_linked_worktree_layout() {
        let (_fixture, candidate) = candidate_fixture();
        let git_dir = candidate.join("admin/worktrees/fixture");
        let common_dir = candidate.join("different-admin");
        let message = rejection_message(
            &candidate,
            false,
            vec![
                ("--show-toplevel", Ok(candidate.clone())),
                ("--git-dir", Ok(git_dir.clone())),
                ("--git-common-dir", Ok(common_dir.clone())),
            ],
        );
        assert!(message.contains("linked-worktree layout"), "{message}");
        assert!(
            message.contains(&git_dir.display().to_string()),
            "{message}"
        );
        assert!(
            message.contains(&common_dir.display().to_string()),
            "{message}"
        );
    }

    #[test]
    fn git_candidate_reports_mismatched_linked_worktree_backlink() {
        let (_fixture, candidate) = candidate_fixture();
        let common_dir = candidate.join("admin");
        let git_dir = common_dir.join("worktrees/fixture");
        std::fs::create_dir_all(&git_dir).unwrap();
        std::fs::write(candidate.join(".git"), "gitdir: ignored\n").unwrap();
        std::fs::write(
            git_dir.join("gitdir"),
            candidate
                .join("missing-marker")
                .to_string_lossy()
                .as_bytes(),
        )
        .unwrap();
        let message = rejection_message(
            &candidate,
            false,
            vec![
                ("--show-toplevel", Ok(candidate.clone())),
                ("--git-dir", Ok(git_dir.clone())),
                ("--git-common-dir", Ok(common_dir)),
            ],
        );
        assert!(message.contains("backlink"), "{message}");
        assert!(
            message.contains(&git_dir.display().to_string()),
            "{message}"
        );
    }

    /// A backlink that resolves nowhere is a mismatch even when the candidate's
    /// own marker has gone missing too. Comparing the two `canonicalize()`
    /// results as `Option`s made this pair of failures compare *equal*, so a
    /// worktree whose backlink pointed nowhere was accepted as pointing back.
    #[test]
    fn treats_an_unresolvable_backlink_as_a_mismatch() {
        let (_fixture, candidate) = candidate_fixture();
        let common_dir = candidate.join("admin");
        let git_dir = common_dir.join("worktrees/fixture");
        std::fs::create_dir_all(&git_dir).unwrap();
        // Deliberately no `.git` marker beside the candidate, so both sides of
        // the comparison fail to resolve.
        std::fs::write(
            git_dir.join("gitdir"),
            candidate
                .join("missing-marker")
                .to_string_lossy()
                .as_bytes(),
        )
        .unwrap();
        let message = rejection_message(
            &candidate,
            false,
            vec![
                ("--show-toplevel", Ok(candidate.clone())),
                ("--git-dir", Ok(git_dir.clone())),
                ("--git-common-dir", Ok(common_dir)),
            ],
        );
        assert!(message.contains("backlink"), "{message}");
    }

    /// A `.git` that Git itself disowns is not a trust root, and asking must not
    /// leave anything behind in the directory that was asked about.
    #[test]
    fn rejects_an_existing_non_repository_directory() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join(".git")).unwrap();
        std::fs::write(dir.path().join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();

        let err = resolve_repository_root(dir.path())
            .expect_err("an arbitrary directory is not a trust root");
        let message = err.to_string();
        assert!(message.contains("--show-toplevel"), "{message}");
        assert!(message.contains("status"), "{message}");
        assert!(message.contains("not a git repository"), "{message}");

        let entries: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(entries, [".git"], "resolution must not write anything");
    }

    #[test]
    fn accepts_a_linked_worktree_git_file() {
        let fixture = tempfile::tempdir().unwrap();
        let main = fixture.path().join("main");
        let worktree = fixture.path().join("worktree");
        commit_and_add_worktree(&main, &worktree);

        assert_eq!(
            resolve_repository_root(&worktree).unwrap(),
            worktree.canonicalize().unwrap()
        );
    }

    #[test]
    fn accepts_relative_linked_worktree_metadata() {
        let fixture = tempfile::tempdir().unwrap();
        let main = fixture.path().join("main");
        let worktree = fixture.path().join("worktree");
        commit_and_add_worktree(&main, &worktree);

        let admin_dir = main.join(".git/worktrees/worktree");
        std::fs::write(
            worktree.join(".git"),
            "gitdir: ../main/.git/worktrees/worktree\n",
        )
        .unwrap();
        std::fs::write(admin_dir.join("gitdir"), "../../../../worktree/.git\n").unwrap();
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(&worktree)
                .args(["rev-parse", "--is-inside-work-tree"])
                .output()
                .unwrap()
                .status
                .success(),
            "Git must accept the relative linked-worktree metadata fixture"
        );

        assert_eq!(
            resolve_repository_root(&worktree).unwrap(),
            worktree.canonicalize().unwrap()
        );
    }

    #[test]
    fn ignores_a_nested_marker_redirecting_to_its_parent() {
        let repo = tempfile::tempdir().unwrap();
        init_git_repo(repo.path());
        let nested = repo.path().join("nested");
        std::fs::create_dir(&nested).unwrap();
        std::fs::write(
            nested.join(".git"),
            format!("gitdir: {}\n", repo.path().join(".git").display()),
        )
        .unwrap();

        assert_eq!(
            resolve_repository_root(&nested).unwrap(),
            repo.path().canonicalize().unwrap()
        );
    }

    /// The anchor may not be derived from ambient process state: a relative
    /// candidate would canonicalize against whatever directory the host happens
    /// to be in.
    #[test]
    fn refuses_a_relative_candidate() {
        let err = resolve_repository_root(Path::new("nested/candidate"))
            .expect_err("a relative candidate has no meaning without the process directory");
        assert!(err.to_string().contains("absolute"), "{err}");
    }

    /// The marker itself is the thing being trusted, so a symlinked one is
    /// refused outright rather than followed to whatever it names.
    #[cfg(unix)]
    #[test]
    fn refuses_a_symlinked_marker() {
        let fixture = tempfile::tempdir().unwrap();
        let repo = fixture.path().join("repo");
        let candidate = fixture.path().join("candidate");
        init_git_repo(&repo);
        std::fs::create_dir(&candidate).unwrap();
        std::os::unix::fs::symlink(repo.join(".git"), candidate.join(".git")).unwrap();

        let err = resolve_repository_root(&candidate)
            .expect_err("a symlinked marker is never a trust root");
        assert!(err.to_string().contains("symlinked marker"), "{err}");
    }
}
