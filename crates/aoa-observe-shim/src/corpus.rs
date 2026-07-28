//! The accumulated trace corpus under `<repo>/.aoa/traces/`.
//!
//! Two file shapes accumulate there, both observe-captured:
//!
//! - `live-<session>.jsonl` — appended live by the `aoa enforce` hooks, one
//!   span per line;
//! - `<name>.json` — whole validated traces landed via
//!   `aoa_audit::write_trace` (the instrumented-harness path).
//!
//! Only the `.json` shape carries the versioned trace envelope, and the
//! wire-format version is checked here on ingest. The `.jsonl` live-log lane has
//! no enclosing envelope to stamp (each line is a bare span) and is AOA-owned
//! rather than codeprobe-produced, so it is intentionally outside the
//! wire-version guard.
//!
//! [`load_corpus`] turns both into one stream of validated
//! [`ObservedSession`]s. A landed edit ([`held_out_edits`]) makes a session a
//! candidate for behavioral measurement, but not a complete metric
//! observation: the audit also requires same-task invariant and
//! accepted-solution context. File count alone never promotes metric modes.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use aoa_codeprobe_shim::{read_capped, read_capped_framed};
use aoa_trace::{validate_trace_value, Trace};
use serde_json::Value;

use crate::backend::parse_live_log;
use crate::error::ObserveShimError;

/// Where observe accumulates traces, relative to a repo root. Mirrors the
/// constant `aoa_audit::observe` provisions.
const TRACES_SUBDIR: &str = ".aoa/traces";

/// Largest corpus file read into memory. Live logs and trace files are small
/// by nature; the cap only trips pathological or hostile input (same bound as
/// the aoa-trace file validator).
const MAX_CORPUS_FILE_BYTES: u64 = 16 * 1024 * 1024;

/// One observe-captured session: a validated trace and where it came from.
#[derive(Debug, Clone, PartialEq)]
pub struct ObservedSession {
    /// The session identifier: the `live-<session>.jsonl` session token, or
    /// the file stem for a whole-trace `.json` file.
    pub session_id: String,
    /// The corpus file the trace was read from.
    pub path: PathBuf,
    pub trace: Trace,
}

/// The repo's accumulated, validated trace corpus.
#[derive(Debug, Clone, PartialEq)]
pub struct TraceCorpus {
    /// Every ingested session, ordered by file name (deterministic).
    pub sessions: Vec<ObservedSession>,
    /// Entries under the traces dir that are not corpus files (unknown
    /// extension, subdirectory, symlink). Recorded so nothing is silently
    /// ignored; they contribute no observation.
    pub skipped: Vec<PathBuf>,
}

impl TraceCorpus {
    /// Sessions carrying held-out edited-file truth. This is a candidate count,
    /// not the number of complete metric observations.
    ///
    /// The audit layer consumes the actual edit sets and counts only
    /// sessions that also have enough evidence to compute all four metrics.
    /// A session that only attempted a write holds nothing out.
    pub fn landed_edit_sessions(&self) -> usize {
        self.sessions
            .iter()
            .filter(|s| !held_out_edits(&s.trace).is_empty())
            .count()
    }
}

/// Load and validate the trace corpus accumulated under `<repo>/.aoa/traces/`.
///
/// A repo that was never observed (no traces directory) yields an empty corpus
/// — zero observations is a defined greenfield state, not an error. A corpus
/// file that fails to read, parse, or validate is an error naming the file:
/// the corpus feeds the behavioral-signal precondition, so a silent skip would
/// under-count the repo.
///
/// Symlinked entries are skipped (recorded on [`TraceCorpus::skipped`]), so a
/// crafted traces dir cannot pull out-of-tree files into the corpus.
pub fn load_corpus(repo: &Path) -> Result<TraceCorpus, ObserveShimError> {
    let dir = repo.join(TRACES_SUBDIR);
    // Refuse links in the AOA-owned directory chain before traversal. Checking
    // only each `read_dir` entry leaves the directory itself able to redirect
    // the entire corpus out of the caller-selected repository.
    for node in [repo.join(".aoa"), dir.clone()] {
        match std::fs::symlink_metadata(&node) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(ObserveShimError::Io {
                    path: node,
                    source: std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "refusing symlinked trace corpus directory",
                    ),
                });
            }
            Ok(_) => {}
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
                return Ok(TraceCorpus {
                    sessions: Vec::new(),
                    skipped: Vec::new(),
                });
            }
            Err(source) => return Err(ObserveShimError::Io { path: node, source }),
        }
    }
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Ok(TraceCorpus {
                sessions: Vec::new(),
                skipped: Vec::new(),
            })
        }
        Err(source) => return Err(ObserveShimError::Io { path: dir, source }),
    };

    let mut sessions = Vec::new();
    let mut skipped = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| ObserveShimError::Io {
            path: dir.clone(),
            source,
        })?;
        let path = entry.path();
        // `DirEntry::file_type` does NOT follow symlinks: a symlinked file must
        // not pull an out-of-tree target into the corpus.
        let file_type = entry.file_type().map_err(|source| ObserveShimError::Io {
            path: path.clone(),
            source,
        })?;
        if !file_type.is_file() {
            skipped.push(path);
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        match session_id_for(&name) {
            Some(session_id) => sessions.push(ingest(&path, session_id, &name)?),
            None => skipped.push(path),
        }
    }
    sessions.sort_by(|a, b| a.path.cmp(&b.path));
    skipped.sort();
    Ok(TraceCorpus { sessions, skipped })
}

/// The session id a corpus file name maps to, or `None` for a non-corpus file.
fn session_id_for(name: &str) -> Option<String> {
    if let Some(session) = name
        .strip_prefix("live-")
        .and_then(|rest| rest.strip_suffix(".jsonl"))
    {
        return Some(session.to_string());
    }
    name.strip_suffix(".json").map(str::to_string)
}

/// Read, parse, and validate one corpus file into an [`ObservedSession`].
fn ingest(
    path: &Path,
    session_id: String,
    name: &str,
) -> Result<ObservedSession, ObserveShimError> {
    let live_log = name.ends_with(".jsonl");
    // The live log is read *framed*: the enforce hooks append to it while an
    // audit walks the corpus, so its last line can be half written. Framing is
    // confined to this lane — a whole-trace `.json` file is one JSON document
    // with no line structure to cut on.
    //
    // Framing rather than the shared lock `aoa enforce` takes on the same log:
    // an audit is a batch walk with nothing gated on it, so a snapshot that
    // stops one record short is correct and the next run picks the record up.
    // Waiting would put a whole-corpus traversal in front of the latency-
    // sensitive hooks writing that log, to buy a freshness this caller has no
    // use for.
    let read = if live_log {
        read_capped_framed
    } else {
        read_capped
    };
    let raw = read(path, MAX_CORPUS_FILE_BYTES).map_err(|source| ObserveShimError::Ingest {
        path: path.to_path_buf(),
        source,
    })?;

    let trace = if live_log {
        parse_live_log(&raw)
            .map_err(|source| ObserveShimError::Ingest {
                path: path.to_path_buf(),
                source,
            })?
            .trace
    } else {
        // Whole-trace `.json` files carry a versioned envelope. Parse it (a
        // malformed file stays a Schema error, as before), then `into_trace`
        // version-checks before unwrapping the spans — this is the codeprobe
        // ingest path, so the guard must fire here, not only in `validate_trace`.
        let envelope: aoa_trace::TraceEnvelope =
            serde_json::from_str(&raw).map_err(|source| ObserveShimError::Schema {
                path: path.to_path_buf(),
                source,
            })?;
        envelope
            .into_trace()
            .map_err(|source| ObserveShimError::InvalidTrace {
                path: path.to_path_buf(),
                source,
            })?
    };
    validate_trace_value(&trace).map_err(|source| ObserveShimError::InvalidTrace {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(ObservedSession {
        session_id,
        path: path.to_path_buf(),
        trace,
    })
}

/// The dev's real edits in an observed session: the `write.committed` span
/// targets. This is the contamination-free held-out ground truth the live
/// corpus exists to accumulate — the observed session captures the *present*
/// work, and the edit the dev actually landed is the hidden truth a later
/// evaluation scores against (codeprobe mines the same truth from *past*
/// commits).
///
/// Only a confirmed post-execution success qualifies. Every other write span is
/// excluded because none of them attests that anything landed: `write.attempt`
/// is intent recorded *before* the tool ran, and `write.failed`,
/// `write.denied`, and `write.blocked` are all explicit non-landings. Reading
/// intent as outcome is precisely how failed and cancelled mutations used to
/// contaminate this set.
pub fn held_out_edits(trace: &Trace) -> BTreeSet<String> {
    trace
        .spans
        .iter()
        .filter(|s| s.span_type.is_confirmed_mutation())
        .filter_map(|s| s.attributes.get("path").and_then(Value::as_str))
        .map(str::to_string)
        .collect()
}
