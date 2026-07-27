use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

#[cfg(not(unix))]
use aoa_trace::validate_trace;
use aoa_trace::{Trace, TraceReport};

use crate::error::AuditError;

/// Where telemetry traces are written, relative to a repo root.
const TRACES_SUBDIR: &str = ".aoa/traces";

/// The extension every whole-trace file carries. Distinguishes this lane from
/// the `.jsonl` live logs that share the traces directory.
const TRACE_EXTENSION: &str = ".json";

/// The result of installing trace telemetry. Tells the caller exactly where
/// traces will be written and where the ignore guard lives.
#[derive(Debug, Clone)]
pub struct ObserveOutcome {
    /// Absolute directory traces are written to (`<repo>/.aoa/traces`).
    pub traces_dir: PathBuf,
    /// The `.gitignore` written under `.aoa/` that ignores everything beneath it.
    pub gitignore: PathBuf,
}

impl ObserveOutcome {
    /// The path a trace named `name` would be written to, once `name` is
    /// confirmed to be a single safe filename component.
    ///
    /// Returns [`AuditError::UnsafeTraceName`] for anything that could escape
    /// the installed `.aoa/traces` boundary: an absolute path (which would
    /// replace the base outright), a `.`/`..` component, a multi-component or
    /// separator-bearing name, or an empty name. Also rejects any name outside
    /// this lane's `<stem>.json` shape, which keeps the write off the
    /// `live-<session>.jsonl` lane (see [`validate_trace_name`]).
    pub fn trace_path(&self, name: &str) -> Result<PathBuf, AuditError> {
        validate_trace_name(name)?;
        Ok(self.traces_dir.join(name))
    }
}

/// Accept a trace filename only if it is exactly one [`Component::Normal`]
/// carrying a non-empty stem and the `.json` extension.
///
/// The component rule is the containment invariant for the write path: joined
/// onto `.aoa/traces`, a single normal component always lands directly inside
/// it, whereas an absolute path replaces the base, `..` walks out of it, and a
/// multi-component name reaches into sub-trees. The explicit separator/NUL
/// guard closes the platform gap where `Path::components` folds a trailing
/// separator into a lone `Normal` and where `\` is an ordinary character on
/// Unix but a separator elsewhere.
///
/// The `.json` rule is the *lane* invariant. Two file shapes share
/// `.aoa/traces`: whole traces (`<stem>.json`, this function's caller) and the
/// append-only live logs (`live-<session>.jsonl`) that the `aoa enforce` hooks
/// extend under their own write lock. Without an extension check a caller could
/// hand `write_trace` a live-log name and clobber an active log from entirely
/// outside that lock. Requiring `.json` — which `.jsonl` does not satisfy —
/// confines this lane to files the live-log writer never touches, and matches
/// the reader contract in `aoa_observe_shim::corpus`, which only ingests a
/// non-`.jsonl` file when it ends in `.json`.
fn validate_trace_name(name: &str) -> Result<(), AuditError> {
    let mut components = Path::new(name).components();
    let single_normal =
        matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none();
    let in_json_lane = name
        .strip_suffix(TRACE_EXTENSION)
        .is_some_and(|stem| !stem.is_empty());

    if single_normal
        && in_json_lane
        && !name.contains('/')
        && !name.contains('\\')
        && !name.contains('\0')
    {
        Ok(())
    } else {
        Err(AuditError::UnsafeTraceName {
            name: name.to_string(),
        })
    }
}

/// Is `node` a symlink, without following it? The single spelling of that
/// question in this module — every symlink guard below goes through here.
///
/// Deliberately not [`Path::is_symlink`]: that helper folds *every* lstat
/// failure into `false`, so a guard built on it fails OPEN on
/// `EACCES`/`ENOTDIR`/`ELOOP`. Nothing is currently exploitable through that gap
/// — the `create_dir_all`/`fs::write` that follows hits the same condition and
/// errors — but a security check whose fallthrough is invisible is one refactor
/// away from being a real hole. Here, only `NotFound` means "absent, safe to
/// create"; any other error surfaces as [`AuditError::Io`].
fn is_symlink_nofollow(node: &Path) -> Result<bool, AuditError> {
    match std::fs::symlink_metadata(node) {
        Ok(meta) => Ok(meta.file_type().is_symlink()),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(AuditError::Io {
            path: node.to_path_buf(),
            source,
        }),
    }
}

/// Refuse a single install-path node that already exists as a symlink.
fn reject_symlink(node: &Path) -> Result<(), AuditError> {
    if is_symlink_nofollow(node)? {
        return Err(AuditError::UnsafeInstallPath {
            path: node.to_path_buf(),
        });
    }
    Ok(())
}

/// Refuse when any node of the installed `.aoa/traces` path already exists as a
/// symlink.
///
/// The install-path half of the containment invariant; see
/// [`AuditError::UnsafeInstallPath`] for what it defends against.
///
/// `Path::ancestors` yields `.aoa/traces` then `.aoa` then the repo root, so
/// taking as many as `TRACES_SUBDIR` has components covers exactly the nodes
/// this install creates — and stays right if that constant gains a level. Each
/// node is lstat'd in its own right, so the check does not depend on visit
/// order: lstat of `.aoa/traces` through a symlinked `.aoa` reports the
/// *target's* real dir and passes, but the separate lstat of `.aoa` still
/// catches it.
///
/// Callers above the repo root are out of scope: a symlinked ancestor of `repo`
/// itself is the caller's choice of root, not an escape from it.
///
/// This exported check remains useful to callers that only need to diagnose a
/// path already containing a link, but it is deliberately not the authorization
/// check for a later mutation. On Unix, [`observe`] and [`write_trace`] instead
/// acquire each directory with `openat(O_NOFOLLOW | O_DIRECTORY)` and perform
/// the mutation relative to the acquired descriptor.
pub fn reject_symlinked_trace_dir(traces_dir: &Path) -> Result<(), AuditError> {
    let depth = Path::new(TRACES_SUBDIR).components().count();
    traces_dir
        .ancestors()
        .take(depth)
        .try_for_each(reject_symlink)
}

#[cfg(unix)]
mod unix {
    use super::*;
    use aoa_trace::{validate_trace_value, TraceError};
    use rustix::fs::{self, AtFlags, FileType, Mode, OFlags};
    use rustix::io::Errno;
    use std::fs::File;
    use std::os::fd::{AsFd, OwnedFd};

    const DIR_MODE: Mode = Mode::RWXU;
    const FILE_MODE: Mode = Mode::RUSR
        .union(Mode::WUSR)
        .union(Mode::RGRP)
        .union(Mode::WGRP)
        .union(Mode::ROTH)
        .union(Mode::WOTH);

    fn io(path: &Path, source: Errno) -> AuditError {
        AuditError::Io {
            path: path.to_path_buf(),
            source: source.into(),
        }
    }

    fn is_symlink_at(parent: impl AsFd, name: &str) -> bool {
        fs::statat(parent, name, AtFlags::SYMLINK_NOFOLLOW)
            .map(|stat| FileType::from_raw_mode(stat.st_mode) == FileType::Symlink)
            .unwrap_or(false)
    }

    fn map_nofollow_error(parent: impl AsFd, name: &str, path: &Path, source: Errno) -> AuditError {
        if source == Errno::LOOP || is_symlink_at(parent, name) {
            AuditError::UnsafeInstallPath {
                path: path.to_path_buf(),
            }
        } else {
            io(path, source)
        }
    }

    fn open_dir_at(parent: impl AsFd, name: &str, path: &Path) -> Result<OwnedFd, AuditError> {
        fs::openat(
            parent.as_fd(),
            name,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW,
            Mode::empty(),
        )
        .map_err(|source| map_nofollow_error(parent, name, path, source))
    }

    fn open_or_create_dir_at(
        parent: impl AsFd,
        name: &str,
        path: &Path,
    ) -> Result<OwnedFd, AuditError> {
        match fs::openat(
            parent.as_fd(),
            name,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW,
            Mode::empty(),
        ) {
            Ok(fd) => Ok(fd),
            Err(Errno::NOENT) => {
                match fs::mkdirat(parent.as_fd(), name, DIR_MODE) {
                    Ok(()) | Err(Errno::EXIST) => {}
                    Err(source) => return Err(io(path, source)),
                }
                open_dir_at(parent, name, path)
            }
            Err(source) => Err(map_nofollow_error(parent, name, path, source)),
        }
    }

    fn open_repo(repo: &Path) -> Result<OwnedFd, AuditError> {
        // `repo` is the caller-selected trust root. Symlinks above or at that
        // root remain allowed; every component created beneath it is opened
        // relative to this stable descriptor with `O_NOFOLLOW`.
        fs::open(repo, OFlags::RDONLY | OFlags::DIRECTORY, Mode::empty())
            .map_err(|source| io(repo, source))
    }

    fn open_installed_dirs(traces_dir: &Path) -> Result<(OwnedFd, OwnedFd, OwnedFd), AuditError> {
        let aoa_dir = traces_dir
            .parent()
            .ok_or_else(|| AuditError::UnsafeInstallPath {
                path: traces_dir.to_path_buf(),
            })?;
        let repo = aoa_dir
            .parent()
            .ok_or_else(|| AuditError::UnsafeInstallPath {
                path: traces_dir.to_path_buf(),
            })?;
        let repo_fd = open_repo(repo)?;
        let aoa_fd = open_dir_at(&repo_fd, ".aoa", aoa_dir)?;
        let traces_fd = open_dir_at(&aoa_fd, "traces", traces_dir)?;
        Ok((repo_fd, aoa_fd, traces_fd))
    }

    pub(super) fn observe(repo: &Path) -> Result<ObserveOutcome, AuditError> {
        observe_with_hook(repo, || {})
    }

    pub(super) fn observe_with_hook(
        repo: &Path,
        after_aoa_open: impl FnOnce(),
    ) -> Result<ObserveOutcome, AuditError> {
        let traces_dir = repo.join(TRACES_SUBDIR);
        let aoa_dir = repo.join(".aoa");
        let gitignore = aoa_dir.join(".gitignore");
        let repo_fd = open_repo(repo)?;
        let aoa_fd = open_or_create_dir_at(&repo_fd, ".aoa", &aoa_dir)?;

        after_aoa_open();

        let _traces_fd = open_or_create_dir_at(&aoa_fd, "traces", &traces_dir)?;
        let gitignore_fd = match fs::openat(
            &aoa_fd,
            ".gitignore",
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::NONBLOCK,
            FILE_MODE,
        ) {
            Ok(fd) => Some(fd),
            Err(Errno::EXIST) => None,
            Err(source) => {
                return Err(map_nofollow_error(
                    &aoa_fd,
                    ".gitignore",
                    &gitignore,
                    source,
                ))
            }
        };
        if let Some(fd) = gitignore_fd {
            File::from(fd)
                .write_all(b"*\n")
                .map_err(|source| AuditError::Io {
                    path: gitignore.clone(),
                    source,
                })?;
        } else {
            // Existing ignore files are never rewritten. Rewriting would let a
            // hardlink share the truncation with a tracked file. Open without
            // blocking, verify the acquired object is one ordinary inode, and
            // accept it only when it already carries the exact install bytes.
            let fd = fs::openat(
                &aoa_fd,
                ".gitignore",
                OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK,
                Mode::empty(),
            )
            .map_err(|source| map_nofollow_error(&aoa_fd, ".gitignore", &gitignore, source))?;
            let stat = fs::fstat(&fd).map_err(|source| io(&gitignore, source))?;
            if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile
                || stat.st_nlink != 1
                || stat.st_size != 2
            {
                return Err(AuditError::UnsafeInstallPath {
                    path: gitignore.clone(),
                });
            }
            let mut existing = [0_u8; 2];
            File::from(fd)
                .read_exact(&mut existing)
                .map_err(|source| AuditError::Io {
                    path: gitignore.clone(),
                    source,
                })?;
            if existing != *b"*\n" {
                return Err(AuditError::UnsafeInstallPath {
                    path: gitignore.clone(),
                });
            }
        }

        Ok(ObserveOutcome {
            traces_dir,
            gitignore,
        })
    }

    pub(super) fn write_trace(
        outcome: &ObserveOutcome,
        name: &str,
        trace: &Trace,
    ) -> Result<(PathBuf, TraceReport), AuditError> {
        write_trace_with_hook(outcome, name, trace, || {})
    }

    pub(super) fn write_trace_with_hook(
        outcome: &ObserveOutcome,
        name: &str,
        trace: &Trace,
        after_traces_open: impl FnOnce(),
    ) -> Result<(PathBuf, TraceReport), AuditError> {
        let path = outcome.trace_path(name)?;
        let json = aoa_trace::to_envelope_json_pretty(trace).map_err(|source| AuditError::Io {
            path: path.clone(),
            source: std::io::Error::new(std::io::ErrorKind::InvalidData, source),
        })?;
        let (_repo_fd, _aoa_fd, traces_fd) = open_installed_dirs(&outcome.traces_dir)?;

        after_traces_open();

        let trace_fd = fs::openat(
            &traces_fd,
            name,
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW,
            FILE_MODE,
        )
        .map_err(|source| {
            if source == Errno::LOOP || is_symlink_at(&traces_fd, name) {
                AuditError::UnsafeTraceName {
                    name: name.to_string(),
                }
            } else if source == Errno::EXIST {
                AuditError::TraceExists { path: path.clone() }
            } else {
                io(&path, source)
            }
        })?;
        let mut file = File::from(trace_fd);
        file.write_all(json.as_bytes())
            .map_err(|source| AuditError::Io {
                path: path.clone(),
                source,
            })?;

        // Serialization is deterministic and cannot alter the in-memory trace.
        // Validate that same value without reopening the attacker-controlled
        // pathname after the descriptor-relative write.
        let report = validate_trace_value(trace).map_err(|source| {
            AuditError::Trace(TraceError::InvalidFile {
                path: path.clone(),
                source: Box::new(source),
            })
        })?;
        Ok((path, report))
    }
}

/// Install trace logging for `repo`. This is a zero-write install with respect
/// to tracked files: it only creates the explicitly-ignored `.aoa/` tree.
///
/// Concretely it creates `<repo>/.aoa/traces/` and writes a `<repo>/.aoa/.gitignore`
/// containing `*`, so every artifact the instrumentation later emits is ignored
/// even in a repo with no top-level ignore for `.aoa/`. No tracked file is
/// touched.
#[cfg(unix)]
pub fn observe(repo: &Path) -> Result<ObserveOutcome, AuditError> {
    unix::observe(repo)
}

/// Non-Unix fallback. Platforms without `openat` retain the existing explicit
/// no-follow checks; the fd-relative race guarantee is Unix-only.
#[cfg(not(unix))]
pub fn observe(repo: &Path) -> Result<ObserveOutcome, AuditError> {
    let traces_dir = repo.join(TRACES_SUBDIR);
    reject_symlinked_trace_dir(&traces_dir)?;
    std::fs::create_dir_all(&traces_dir).map_err(|source| AuditError::Io {
        path: traces_dir.clone(),
        source,
    })?;

    let gitignore = repo.join(".aoa").join(".gitignore");
    // The guard above covers the directories; the ignore file is a write target
    // in its own right, and `fs::write` through a link planted here would
    // truncate whatever it points at — including a tracked file in this repo.
    reject_symlink(&gitignore)?;
    std::fs::write(&gitignore, "*\n").map_err(|source| AuditError::Io {
        path: gitignore.clone(),
        source,
    })?;

    Ok(ObserveOutcome {
        traces_dir,
        gitignore,
    })
}

/// Write a trace through the observe-installed path and validate it in one
/// step. This is the instrumentation entrypoint: the instrumented harness hands
/// a [`Trace`] here and it lands under `.aoa/traces/`, already ordering-checked.
#[cfg(unix)]
pub fn write_trace(
    outcome: &ObserveOutcome,
    name: &str,
    trace: &Trace,
) -> Result<(PathBuf, TraceReport), AuditError> {
    unix::write_trace(outcome, name, trace)
}

/// Non-Unix fallback matching [`observe`]'s platform split.
#[cfg(not(unix))]
pub fn write_trace(
    outcome: &ObserveOutcome,
    name: &str,
    trace: &Trace,
) -> Result<(PathBuf, TraceReport), AuditError> {
    let path = outcome.trace_path(name)?;

    // Encode through the versioned envelope so the file carries the wire-format
    // version and survives the read-side version check below.
    //
    // Done BEFORE the symlink checks, not between them and the write. The guard
    // is check-then-act either way (see `reject_symlinked_trace_dir`), but
    // interposing this work would stretch the race window by however long
    // encoding takes. Keep the checks adjacent to the `fs::write` they protect.
    let json = aoa_trace::to_envelope_json_pretty(trace).map_err(|source| AuditError::Io {
        path: path.clone(),
        source: std::io::Error::new(std::io::ErrorKind::InvalidData, source),
    })?;

    // Re-check the install path on every write, not just at install. `observe`
    // runs once; writes happen in later processes over the lifetime of the repo,
    // so a link planted into `.aoa` after install would otherwise defeat the
    // install-time guard entirely. `ObserveOutcome`'s fields are public too, so
    // an outcome reaching here need never have come from `observe` at all.
    reject_symlinked_trace_dir(&outcome.traces_dir)?;

    // Symlink boundary: even a legal single-component name can be a symlink
    // already sitting in the trace dir and pointing outside it. `std::fs::write`
    // follows symlinks, so writing through one would clobber whatever it targets.
    // Reported as an unsafe NAME, not an unsafe install path: the offending node
    // is the caller's `name`, and callers already handle that variant.
    if is_symlink_nofollow(&path)? {
        return Err(AuditError::UnsafeTraceName {
            name: name.to_string(),
        });
    }

    // Create-new, never truncate. A trace file is a finished artifact, so an
    // existing path means a name collision, not an update: truncating it would
    // destroy a landed observation the corpus has already counted. `create_new`
    // makes that refusal atomic — the existence check and the create are one
    // syscall, so a concurrent writer cannot land between them.
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|source| match source.kind() {
            std::io::ErrorKind::AlreadyExists => AuditError::TraceExists { path: path.clone() },
            _ => AuditError::Io {
                path: path.clone(),
                source,
            },
        })?;
    file.write_all(json.as_bytes())
        .map_err(|source| AuditError::Io {
            path: path.clone(),
            source,
        })?;
    drop(file);

    let report = validate_trace(&path)?;
    Ok((path, report))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outcome() -> ObserveOutcome {
        ObserveOutcome {
            traces_dir: PathBuf::from("/repo/.aoa/traces"),
            gitignore: PathBuf::from("/repo/.aoa/.gitignore"),
        }
    }

    #[test]
    fn valid_single_component_names_round_trip() {
        let o = outcome();
        for name in [
            "run-1.json",
            "trace.json",
            "a.json",
            "..foo.json",
            "foo..bar.json",
            "session_2026.json",
            // Not in the live-log lane: the `live-` prefix alone is harmless,
            // only `live-<session>.jsonl` names an append-only log.
            "live-s1.json",
        ] {
            let path = o.trace_path(name).expect("valid name accepted");
            assert_eq!(path, o.traces_dir.join(name));
            assert_eq!(
                path.parent(),
                Some(o.traces_dir.as_path()),
                "valid name must stay directly inside the trace dir: {name:?}"
            );
        }
    }

    #[cfg(unix)]
    mod dirfd_races {
        use super::*;
        use aoa_trace::{Span, SpanSource, SpanType};
        use std::os::unix::fs::symlink;

        fn valid_trace() -> Trace {
            Trace {
                spans: vec![Span {
                    span_type: SpanType::FileRead,
                    source: SpanSource::Native,
                    seq: 0,
                    attributes: serde_json::Map::new(),
                }],
            }
        }

        #[test]
        fn observe_keeps_writes_on_acquired_directory_after_path_substitution() {
            let repo = tempfile::tempdir().expect("create repo");
            let outside = tempfile::tempdir().expect("create outside dir");
            let seeded = outside.path().join("seeded.txt");
            std::fs::write(&seeded, "original").expect("seed outside file");

            let displaced = repo.path().join(".aoa-displaced");
            let outcome = unix::observe_with_hook(repo.path(), || {
                std::fs::rename(repo.path().join(".aoa"), &displaced)
                    .expect("displace acquired .aoa");
                symlink(outside.path(), repo.path().join(".aoa"))
                    .expect("substitute .aoa with outside link");
            })
            .expect("dirfd-relative install succeeds safely");

            assert_eq!(outcome.traces_dir, repo.path().join(".aoa/traces"));
            assert_eq!(
                std::fs::read_to_string(&seeded).expect("read seeded file"),
                "original"
            );
            assert_eq!(
                std::fs::read_dir(outside.path())
                    .expect("read outside")
                    .count(),
                1,
                "install escaped through substituted .aoa"
            );
            assert_eq!(
                std::fs::read_to_string(displaced.join(".gitignore"))
                    .expect("read dirfd-relative gitignore"),
                "*\n"
            );
            assert!(displaced.join("traces").is_dir());
        }

        #[test]
        fn whole_trace_create_stays_on_acquired_directory_after_path_substitution() {
            let repo = tempfile::tempdir().expect("create repo");
            let outside = tempfile::tempdir().expect("create outside dir");
            let seeded = outside.path().join("seeded.txt");
            std::fs::write(&seeded, "original").expect("seed outside file");
            let outcome = observe(repo.path()).expect("install observe");

            let displaced = repo.path().join(".aoa-displaced");
            let (path, _) =
                unix::write_trace_with_hook(&outcome, "race.json", &valid_trace(), || {
                    std::fs::rename(repo.path().join(".aoa"), &displaced)
                        .expect("displace acquired .aoa");
                    symlink(outside.path(), repo.path().join(".aoa"))
                        .expect("substitute .aoa with outside link");
                })
                .expect("dirfd-relative trace create succeeds safely");

            assert_eq!(path, repo.path().join(".aoa/traces/race.json"));
            assert_eq!(
                std::fs::read_to_string(&seeded).expect("read seeded file"),
                "original"
            );
            assert_eq!(
                std::fs::read_dir(outside.path())
                    .expect("read outside")
                    .count(),
                1,
                "trace create escaped through substituted .aoa"
            );
            assert!(
                displaced.join("traces/race.json").is_file(),
                "trace was not created relative to the acquired traces fd"
            );
        }
    }

    #[test]
    fn escaping_names_are_rejected() {
        let o = outcome();
        for name in [
            "/etc/passwd",      // absolute path replaces the base outright
            "/tmp/x",           // absolute path
            "..",               // parent component
            "../x",             // parent traversal
            "../../etc/passwd", // deep parent traversal
            ".",                // current-dir component
            "a/b",              // multi-component
            "sub/trace.json",   // multi-component
            "dir/",             // trailing separator
            "a\\b",             // backslash separator (defensive)
            "",                 // empty
            "live-x.jsonl",     // the append-only live-log lane
            "trace.jsonl",      // any .jsonl, lane-owned regardless of prefix
            "run-1",            // no extension: outside the ingested lane
            "run-1.txt",        // wrong extension
            ".json",            // extension with an empty stem
        ] {
            let err = o
                .trace_path(name)
                .expect_err(&format!("must reject {name:?}"));
            assert!(
                matches!(err, AuditError::UnsafeTraceName { .. }),
                "wrong error for {name:?}: {err:?}"
            );
        }
    }
}
