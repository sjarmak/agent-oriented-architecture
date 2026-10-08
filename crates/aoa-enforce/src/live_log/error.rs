//! The live log's failure vocabulary: one enum every operation in the module
//! returns, and which callers may match on.
//!
//! # Why this is typed rather than `anyhow`
//!
//! The module used to thread `anyhow` through every submodule. Nothing
//! public named an `anyhow` type by hand, but [`LiveLog::read_spans`],
//! [`LiveLog::append`] and [`LiveLog::append_with`] all returned
//! `anyhow::Result`, so the workspace's thiserror convention was in fact broken
//! where a caller could see it. The one behaviour the crate needed from
//! `anyhow` — recognising "the log does not exist yet" — was a downcast through
//! the error chain; that is a `match` now.
//!
//! [`LiveLog::read_spans`]: super::LiveLog::read_spans
//! [`LiveLog::append`]: super::LiveLog::append
//! [`LiveLog::append_with`]: super::LiveLog::append_with
//!
//! # How the variants were triaged
//!
//! The three invariant submodules and the facade above them stated 21 distinct
//! contexts; twelve variants carry them. Two groups are the same failure
//! wearing a different noun and are deliberately merged; the merge is recorded
//! here so a later reader can tell a decision from an oversight.
//!
//! - [`LiveLogError::Io`] covers all eight syscall verbs — `open`, `create`,
//!   `stat`, `read`, `seek`, `flock`, the append `write_all`, and the `set_len`
//!   that repairs a torn tail. Every one of them already rendered
//!   `failed to <verb> <path>` over an [`std::io::Error`], and the operator's
//!   remedy is whatever the underlying errno says, not which of our calls made
//!   it. The verb is now the [`IoAction`] field, and the messages are unchanged
//!   byte-for-byte.
//! - [`LiveLogError::MalformedLogPath`] covers the four `log_parts` faults. They
//!   are one condition — the path handed in is not `<repo>/.aoa/traces/<name>`
//!   — reported once per missing component, and the component is now a field.
//!   They are unreachable from `LiveLog::for_session`, which builds the path;
//!   they guard the module-internal callers that take one apart.
//!
//! What was NOT merged, and why: the remaining faults each send an operator
//! somewhere different. A refused symlink and a non-regular file are both
//! containment refusals but name different plants, and only the latter can
//! report what it found. A lock timeout points at another wedged hook, a torn
//! tail at a killed writer, an oversized line at a host passing an unbounded
//! path through its hook payload, and sequence exhaustion at a hand-edited log
//! — collapsing any of them would name the failure without naming the remedy.
//! [`LiveLogError::CorruptSpanLine`] and [`LiveLogError::SerializeSpan`] look
//! alike and are not: one is a corrupt file, the other a defect in the span we
//! were about to write.
//!
//! # Message shape
//!
//! A variant wrapping a foreign error puts it in `#[source]` and does not inline
//! it in its own `Display`, so a host rendering the chain (`{:#}` under
//! `anyhow`, which is what the CLI does) reads `context: cause` exactly as it
//! did before.

use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use thiserror::Error;

/// Result alias for every fallible live-log operation.
pub type Result<T> = std::result::Result<T, LiveLogError>;

/// Every way reading from or appending to the live span log can fail.
#[derive(Debug, Error)]
pub enum LiveLogError {
    /// A filesystem operation on the log or a directory above it failed. The
    /// operation is the field; see the triage note above for why these are one
    /// variant.
    #[error("failed to {action} {}", path.display())]
    Io {
        action: IoAction,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    // ---- containment: what the payload named must not redirect the write ----
    /// A symlink sat at a component of the log path. Refused rather than
    /// followed, so nothing outside `<base>/.aoa/traces/` can be appended to.
    #[error("refusing to follow a symlink at {}", path.display())]
    SymlinkRefused { path: PathBuf },

    #[error(transparent)]
    PathRefused(aoa_path_trust::PathTrustError),

    /// The log path resolved to something other than a regular file — a FIFO, a
    /// directory, a device node.
    #[error(
        "refusing to use {}: the span log must be a regular file, found {file_type:?}",
        path.display()
    )]
    NotRegularFile {
        path: PathBuf,
        file_type: std::fs::FileType,
    },

    /// The path handed in is not shaped like `<repo>/.aoa/traces/<name>`.
    #[error("span log has no {component}: {}", path.display())]
    MalformedLogPath {
        component: LogPathComponent,
        path: PathBuf,
    },

    // ---- bounded waiting ----
    /// The advisory lock was still held when the deadline passed. Reported
    /// rather than waited out: a hook runs in the agent's tool path, so
    /// inheriting another process's stall freezes the session.
    #[error(
        "timed out after {timeout:?} waiting for the span log lock on {}; \
         another aoa hook is holding it and may be wedged",
        path.display()
    )]
    LockTimeout { timeout: Duration, path: PathBuf },

    // ---- the log's own contents ----
    /// The log's final line has no terminating newline, so an append would
    /// splice this span onto a killed writer's fragment.
    #[error(
        "{} has no trailing newline, so its last line is a torn write; \
         appending would splice this span onto it",
        path.display()
    )]
    TornTail { path: PathBuf },

    /// A line already in the log is longer than the bounded tail read, so no
    /// sequence number can be derived from it.
    #[error("the last line of {} exceeds the {max_tail}-byte tail read", path.display())]
    TailLineTooLong { path: PathBuf, max_tail: u64 },

    /// The span we were about to write is longer than the bounded tail read.
    /// Refused before writing: the log would otherwise poison itself with a line
    /// no later append could read back.
    #[error(
        "refusing to append a {bytes}-byte span line to {}: it exceeds the \
         {max_tail}-byte tail read, so no later append could derive its \
         sequence number",
        path.display()
    )]
    SpanLineTooLong {
        bytes: u64,
        path: PathBuf,
        max_tail: u64,
    },

    /// The log's last span carries `u64::MAX`, so the next number would wrap to
    /// zero and make the log decreasing.
    #[error(
        "the last span in {} carries seq {seq}, the maximum; the next \
         sequence number would wrap to zero and make the log decreasing",
        path.display()
    )]
    SequenceExhausted { path: PathBuf, seq: u64 },

    /// A line in the log is not a deserializable span.
    #[error("corrupt span line in {}", path.display())]
    CorruptSpanLine {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },

    /// The span handed to `append_with` could not be serialized — a defect in
    /// the caller's span, not in the file.
    #[error("failed to serialize span")]
    SerializeSpan {
        #[source]
        source: serde_json::Error,
    },
}

impl LiveLogError {
    /// Whether this is "the log does not exist yet", which is the ordinary state
    /// before the first span is written. Every other failure — a symlink, a
    /// FIFO, a permission problem — stays an error.
    pub(super) fn is_not_found(&self) -> bool {
        matches!(
            self,
            LiveLogError::Io { source, .. } if source.kind() == std::io::ErrorKind::NotFound
        )
    }

    /// A [`LiveLogError::Io`] over `path` for `action`.
    pub(super) fn io(path: &Path, action: IoAction, source: std::io::Error) -> Self {
        LiveLogError::Io {
            action,
            path: path.to_path_buf(),
            source,
        }
    }
}

/// The filesystem operation a [`LiveLogError::Io`] was reporting. Renders as
/// the verb phrase in `failed to <verb> <path>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IoAction {
    Open,
    Create,
    Stat,
    Read,
    Seek,
    Lock,
    Append,
    RepairTornTail,
}

impl fmt::Display for IoAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Open => "open",
            Self::Create => "create",
            Self::Stat => "stat",
            Self::Read => "read",
            Self::Seek => "seek",
            Self::Lock => "lock",
            Self::Append => "append to",
            Self::RepairTornTail => "repair torn tail in",
        })
    }
}

/// Which component a malformed log path was missing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogPathComponent {
    TracesDir,
    AoaDir,
    RepoRoot,
    FileName,
}

impl fmt::Display for LogPathComponent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::TracesDir => "traces directory",
            Self::AoaDir => ".aoa directory",
            Self::RepoRoot => "repository root",
            Self::FileName => "file name",
        })
    }
}
