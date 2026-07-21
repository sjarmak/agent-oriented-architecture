use std::io::Write;
use std::path::{Component, Path, PathBuf};

use aoa_trace::{validate_trace, Trace, TraceReport};

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

/// Install trace logging for `repo`. This is a zero-write install with respect
/// to tracked files: it only creates the explicitly-ignored `.aoa/` tree.
///
/// Concretely it creates `<repo>/.aoa/traces/` and writes a `<repo>/.aoa/.gitignore`
/// containing `*`, so every artifact the instrumentation later emits is ignored
/// even in a repo with no top-level ignore for `.aoa/`. No tracked file is
/// touched.
pub fn observe(repo: &Path) -> Result<ObserveOutcome, AuditError> {
    let traces_dir = repo.join(TRACES_SUBDIR);
    std::fs::create_dir_all(&traces_dir).map_err(|source| AuditError::Io {
        path: traces_dir.clone(),
        source,
    })?;

    let gitignore = repo.join(".aoa").join(".gitignore");
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

    // Symlink boundary: even a legal single-component name can be a symlink
    // already sitting in the trace dir and pointing outside it. `std::fs::write`
    // follows symlinks, so writing through one would clobber whatever it targets.
    // Refuse rather than follow — `symlink_metadata` does not itself follow.
    if std::fs::symlink_metadata(&path).is_ok_and(|meta| meta.file_type().is_symlink()) {
        return Err(AuditError::UnsafeTraceName {
            name: name.to_string(),
        });
    }

    // Write through the versioned envelope so the file carries the wire-format
    // version and survives the read-side version check below.
    let json = aoa_trace::to_envelope_json_pretty(trace).map_err(|source| AuditError::Io {
        path: path.clone(),
        source: std::io::Error::new(std::io::ErrorKind::InvalidData, source),
    })?;
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
