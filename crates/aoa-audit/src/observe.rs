use std::path::{Component, Path, PathBuf};

use aoa_trace::{validate_trace, Trace, TraceReport};

use crate::error::AuditError;

/// Where telemetry traces are written, relative to a repo root.
const TRACES_SUBDIR: &str = ".aoa/traces";

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
    /// separator-bearing name, or an empty name.
    pub fn trace_path(&self, name: &str) -> Result<PathBuf, AuditError> {
        validate_trace_name(name)?;
        Ok(self.traces_dir.join(name))
    }
}

/// Accept a trace filename only if it is exactly one [`Component::Normal`].
///
/// This is the containment invariant for the write path: joined onto
/// `.aoa/traces`, a single normal component always lands directly inside it,
/// whereas an absolute path replaces the base, `..` walks out of it, and a
/// multi-component name reaches into sub-trees. The explicit separator/NUL
/// guard closes the platform gap where `Path::components` folds a trailing
/// separator into a lone `Normal` and where `\` is an ordinary character on
/// Unix but a separator elsewhere.
fn validate_trace_name(name: &str) -> Result<(), AuditError> {
    let mut components = Path::new(name).components();
    let single_normal =
        matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none();

    if single_normal && !name.contains('/') && !name.contains('\\') && !name.contains('\0') {
        Ok(())
    } else {
        Err(AuditError::UnsafeTraceName {
            name: name.to_string(),
        })
    }
}

/// Refuse a single install-path node that already exists as a symlink.
///
/// Spelled with `symlink_metadata` rather than [`Path::is_symlink`] on purpose:
/// that helper folds *every* lstat failure into `false`, so a guard built on it
/// fails OPEN on `EACCES`/`ENOTDIR`/`ELOOP`. Nothing is currently exploitable
/// through that gap — the `create_dir_all`/`fs::write` that follows hits the same
/// condition and errors — but a security check whose fallthrough is invisible is
/// one refactor away from being a real hole. Here, only `NotFound` means
/// "absent, safe to create"; any other error surfaces as [`AuditError::Io`].
fn reject_symlink(node: &Path) -> Result<(), AuditError> {
    match std::fs::symlink_metadata(node) {
        Ok(meta) if meta.file_type().is_symlink() => Err(AuditError::UnsafeInstallPath {
            path: node.to_path_buf(),
        }),
        Ok(_) => Ok(()),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(AuditError::Io {
            path: node.to_path_buf(),
            source,
        }),
    }
}

/// Refuse when any node of the installed `.aoa/traces` path already exists as a
/// symlink.
///
/// This is the install-path half of the containment invariant. `create_dir_all`
/// and `fs::write` both FOLLOW symlinks, so a `.aoa` or `.aoa/traces` planted as
/// a link relocates every subsequent write outside the repo while each trace
/// name still passes [`validate_trace_name`] as a legal single component.
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
/// SCOPE — this defends against a symlink planted *before* the call, not against
/// a co-resident attacker racing one in. It is check-then-act: the lstats finish,
/// then the caller runs `create_dir_all`/`fs::write` as separate syscalls. A swap
/// landing in that window still escapes, and `create_dir_all` makes it worse than
/// it looks, since its fallback accepts "mkdir failed but `path.is_dir()`" as
/// success and `is_dir` follows links. Closing that needs `openat`/`O_NOFOLLOW`
/// rather than lstat, which is a dependency decision, not a patch — tracked as
/// aoa-zb48. Do not read this guard as more than it is.
fn reject_symlinked_install_path(traces_dir: &Path) -> Result<(), AuditError> {
    let depth = Path::new(TRACES_SUBDIR).components().count();
    traces_dir
        .ancestors()
        .take(depth)
        .try_for_each(reject_symlink)
}

/// Install trace logging for `repo`. This is a zero-write install with respect
/// to tracked files: it only creates the explicitly-ignored `.aoa/` tree.
///
/// Concretely it creates `<repo>/.aoa/traces/` and writes a `<repo>/.aoa/.gitignore`
/// containing `*`, so every artifact the instrumentation later emits is ignored
/// even in a repo with no top-level ignore for `.aoa/`. No tracked file is
/// touched.
pub fn observe(repo: &Path) -> Result<ObserveOutcome, AuditError> {
    let traces_dir = repo.join(TRACES_SUBDIR);
    reject_symlinked_install_path(&traces_dir)?;
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
pub fn write_trace(
    outcome: &ObserveOutcome,
    name: &str,
    trace: &Trace,
) -> Result<(PathBuf, TraceReport), AuditError> {
    let path = outcome.trace_path(name)?;

    // Serialize BEFORE the symlink checks, not between them and the write. The
    // guard is check-then-act either way (see `reject_symlinked_install_path`),
    // but interposing this work would stretch the race window by however long
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
    reject_symlinked_install_path(&outcome.traces_dir)?;

    // Symlink boundary: even a legal single-component name can be a symlink
    // already sitting in the trace dir and pointing outside it. `std::fs::write`
    // follows symlinks, so writing through one would clobber whatever it targets.
    // Reported as an unsafe NAME, not an unsafe install path: the offending node
    // is the caller's `name`, and callers already handle that variant.
    if path.is_symlink() {
        return Err(AuditError::UnsafeTraceName {
            name: name.to_string(),
        });
    }

    std::fs::write(&path, json).map_err(|source| AuditError::Io {
        path: path.clone(),
        source,
    })?;

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
            "a",
            "..foo",
            "foo..bar",
            "session_2026.json",
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
