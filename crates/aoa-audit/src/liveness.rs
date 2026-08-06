//! Whether the installed runtime enforcement plane is actually emitting records.
//!
//! [`crate::planes`] answers whether the hook set is *installed*. That is not the
//! same question as whether it *runs*, and the two states used to be
//! indistinguishable from every surface AOA exposed: `settings.json` reads
//! identically, the version stamp reads identically, and an absent live log
//! reads as no-activity rather than as broken instrumentation. AOA's own repo sat
//! in the silent state for the life of the enforce hook set — five hooks
//! installed, every one invoking a binary on no session's PATH, `.aoa/traces/`
//! never created — and nothing here or anywhere else said so (aoa-dpluh).
//!
//! So this module reports three distinct states, never two:
//! [`EnforcementLiveness::NotInstalled`],
//! [`EnforcementLiveness::InstalledButSilent`], and
//! [`EnforcementLiveness::Enforcing`]. Silence carries a [`Silence`] reason
//! because the ways of being silent are different defects with different fixes:
//! an absent `.aoa/traces` means the telemetry install never ran, an empty one
//! means it ran and the hooks did not, and logs holding zero records mean a
//! session opened one and wrote nothing into it. Collapsing those into one blank
//! hands an operator a symptom with no direction.
//!
//! Silence is emphatically not a pass. [`crate::audit`] raises it as a Tier-1
//! finding, the same rule the metrics side adopted after aoa-xo8y0: an absent
//! measurement is missing evidence, not a benign zero.

use std::fs::DirEntry;
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::observe::TRACES_SUBDIR;
use crate::planes::runtime_hook_present;

/// The filename shape the enforcement hooks append to, one per session. Owned by
/// `aoa_enforce::live_log`, which sits a layer above this crate and so cannot be
/// depended on from here; `crate::observe` already encodes the same lane when it
/// refuses to let a whole-trace write land on a live log.
const LIVE_LOG_PREFIX: &str = "live-";
const LIVE_LOG_EXTENSION: &str = ".jsonl";

/// Whether this repository's runtime enforcement plane is producing records.
///
/// Three states, deliberately not a boolean: the pair that matters is
/// `Enforcing` vs `InstalledButSilent`, and a boolean "installed" answers
/// neither.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "kebab-case")]
pub enum EnforcementLiveness {
    /// The hook set is not installed. There is nothing to be silent about; the
    /// missing plane is reported as a missing plane.
    ///
    /// Also the default, which is what a report predating this field
    /// deserializes to: a producer that could not measure liveness must never
    /// read as enforcing.
    #[default]
    NotInstalled,
    /// The hook set is installed and no enforcement record reached the live log.
    /// The plane reads as present from every configuration surface and enforces
    /// nothing.
    InstalledButSilent { silence: Silence },
    /// The hook set is installed and the live log holds records.
    Enforcing {
        /// Live logs contributing spans within the window.
        live_logs: usize,
        /// Spans counted within the window, one per committed record line.
        spans: u64,
    },
}

impl EnforcementLiveness {
    /// Whether the plane is demonstrably running. `false` for both silence and
    /// absence — a caller asking this question must not have to remember that
    /// two of the three states are negative.
    #[must_use]
    pub fn is_enforcing(&self) -> bool {
        matches!(self, EnforcementLiveness::Enforcing { .. })
    }

    /// The silence reason, when the plane is installed and silent.
    #[must_use]
    pub fn silence(&self) -> Option<Silence> {
        match self {
            EnforcementLiveness::InstalledButSilent { silence } => Some(*silence),
            _ => None,
        }
    }

    /// One line naming the state, for the human register. The silent state
    /// shouts: it is the one an operator has been reading as healthy.
    #[must_use]
    pub fn render_line(&self) -> String {
        match self {
            EnforcementLiveness::NotInstalled => {
                "enforcement plane: not installed (no runtime hook set)".to_string()
            }
            EnforcementLiveness::InstalledButSilent { silence } => format!(
                "enforcement plane: INSTALLED BUT SILENT — {}; this repo is NOT enforcing",
                silence.reason()
            ),
            EnforcementLiveness::Enforcing { live_logs, spans } => format!(
                "enforcement plane: enforcing ({spans} span(s) across {live_logs} live log(s))"
            ),
        }
    }
}

/// Why an installed plane counts as silent.
///
/// Separate variants because they are separate defects: the first two say the
/// hooks never ran, and which one it is tells the operator whether `aoa observe`
/// ran at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Silence {
    /// `<repo>/.aoa/traces` does not exist: no session ever opened a live log,
    /// and the telemetry install itself may never have run.
    TracesDirectoryAbsent,
    /// `<repo>/.aoa/traces` exists but could not be read. Not evidence of
    /// enforcement, so it is reported as silence rather than swallowed.
    TracesDirectoryUnreadable,
    /// The traces directory exists and holds no live log at all.
    NoLiveLogs,
    /// Live logs exist and hold no committed span between them.
    LiveLogsEmpty,
    /// Spans exist, but none within the caller's window.
    NoneInWindow,
}

impl Silence {
    /// The phrase this silence reads as in a message.
    #[must_use]
    pub fn reason(self) -> &'static str {
        match self {
            Silence::TracesDirectoryAbsent => "no .aoa/traces directory exists",
            Silence::TracesDirectoryUnreadable => ".aoa/traces could not be read",
            Silence::NoLiveLogs => "no live log exists under .aoa/traces",
            Silence::LiveLogsEmpty => "every live log under .aoa/traces is empty",
            Silence::NoneInWindow => "no span was emitted within the requested window",
        }
    }
}

/// Report whether `repo`'s runtime enforcement plane has emitted a record,
/// optionally restricted to logs touched at or after `since`.
///
/// Reads only: it opens the live logs and never creates the traces directory, so
/// asking the question cannot manufacture the artifact that answers it.
///
/// `since` filters on each log's modification time rather than on a per-record
/// timestamp, because the span format carries none. That makes the window a
/// property of the *file*: a log last appended to before `since` contributes
/// nothing even if it holds records.
#[must_use]
pub fn enforcement_liveness(repo: &Path, since: Option<SystemTime>) -> EnforcementLiveness {
    if !runtime_hook_present(repo) {
        return EnforcementLiveness::NotInstalled;
    }
    let silent = |silence| EnforcementLiveness::InstalledButSilent { silence };

    let entries = match std::fs::read_dir(repo.join(TRACES_SUBDIR)) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return silent(Silence::TracesDirectoryAbsent)
        }
        Err(_) => return silent(Silence::TracesDirectoryUnreadable),
    };

    let mut logs = 0_usize;
    let mut total_spans = 0_u64;
    let mut window_logs = 0_usize;
    let mut window_spans = 0_u64;
    for entry in entries.flatten() {
        let Some(spans) = live_log_spans(&entry) else {
            continue;
        };
        logs += 1;
        total_spans += spans;
        if spans > 0 && touched_since(&entry, since) {
            window_logs += 1;
            window_spans += spans;
        }
    }

    if logs == 0 {
        silent(Silence::NoLiveLogs)
    } else if total_spans == 0 {
        silent(Silence::LiveLogsEmpty)
    } else if window_spans == 0 {
        silent(Silence::NoneInWindow)
    } else {
        EnforcementLiveness::Enforcing {
            live_logs: window_logs,
            spans: window_spans,
        }
    }
}

/// The number of committed spans in `entry`, or `None` when it is not a live log
/// at all.
///
/// Counted by line terminator, streaming: the append path writes exactly one
/// span per terminated line, an unterminated tail is a torn write that no reader
/// would accept, and a log large enough to matter is never held in memory. An
/// unreadable live log counts zero rather than aborting the audit — its silence
/// is the finding.
fn live_log_spans(entry: &DirEntry) -> Option<u64> {
    let name = entry.file_name().into_string().ok()?;
    if !name.starts_with(LIVE_LOG_PREFIX) || !name.ends_with(LIVE_LOG_EXTENSION) {
        return None;
    }
    if !entry.file_type().is_ok_and(|kind| kind.is_file()) {
        return None;
    }
    let Ok(file) = std::fs::File::open(entry.path()) else {
        return Some(0);
    };
    let mut reader = BufReader::new(file);
    let mut line = Vec::new();
    let mut spans = 0_u64;
    loop {
        line.clear();
        match reader.read_until(b'\n', &mut line) {
            // EOF, an unterminated tail (a torn write, not a span), or a read
            // error: nothing further can be counted either way.
            Ok(0) | Err(_) => break,
            Ok(_) if line.ends_with(b"\n") => spans += 1,
            Ok(_) => break,
        }
    }
    Some(spans)
}

/// Whether this log was appended to within the window. A log whose modification
/// time cannot be read is treated as in-window: the alternative is dropping a
/// log that demonstrably holds records, which understates enforcement.
fn touched_since(entry: &DirEntry, since: Option<SystemTime>) -> bool {
    let Some(since) = since else {
        return true;
    };
    match entry.metadata().and_then(|metadata| metadata.modified()) {
        Ok(modified) => modified >= since,
        Err(_) => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// A repo whose hook set the plane check accepts. The liveness question only
    /// arises once the plane reads as installed.
    fn installed_repo() -> tempfile::TempDir {
        let repo = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(repo.path().join(".claude")).unwrap();
        std::fs::write(
            repo.path().join(".claude/settings.json"),
            r#"{"hooks":{
                "PostToolUse":[{"hooks":[
                    {"command":"aoa enforce record"},
                    {"command":"aoa enforce commit"}
                ]}],
                "PreToolUse":[{"hooks":[{"command":"aoa enforce check"}]}],
                "PostToolUseFailure":[{"hooks":[{"command":"aoa enforce fail"}]}],
                "PermissionDenied":[{"hooks":[{"command":"aoa enforce deny"}]}]
            }}"#,
        )
        .unwrap();
        repo
    }

    fn traces_dir(repo: &Path) -> std::path::PathBuf {
        let dir = repo.join(TRACES_SUBDIR);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn record_line(seq: u64) -> String {
        format!(r#"{{"type":"test.run","source":"native","seq":{seq},"attributes":{{}}}}"#)
    }

    /// Criterion (e): three separate facts, three separate answers. Reported as
    /// one sequence because the point is that they differ — asserting each in
    /// isolation would pass just as well against a single collapsed variant.
    #[test]
    fn absent_empty_and_populated_traces_dirs_are_three_distinct_facts() {
        let repo = installed_repo();

        assert_eq!(
            enforcement_liveness(repo.path(), None).silence(),
            Some(Silence::TracesDirectoryAbsent)
        );

        let traces = traces_dir(repo.path());
        assert_eq!(
            enforcement_liveness(repo.path(), None).silence(),
            Some(Silence::NoLiveLogs),
            "an existing but empty traces dir is not an absent one"
        );

        std::fs::write(traces.join("live-s1.jsonl"), "").unwrap();
        assert_eq!(
            enforcement_liveness(repo.path(), None).silence(),
            Some(Silence::LiveLogsEmpty),
            "a live log holding nothing is not the same as having no live log"
        );

        std::fs::write(
            traces.join("live-s1.jsonl"),
            format!("{}\n", record_line(0)),
        )
        .unwrap();
        assert_eq!(
            enforcement_liveness(repo.path(), None),
            EnforcementLiveness::Enforcing {
                live_logs: 1,
                spans: 1
            }
        );
    }

    /// The plane check gates the whole question: an uninstalled repo is not
    /// silent, whatever its traces dir looks like.
    #[test]
    fn an_uninstalled_plane_is_not_installed_rather_than_silent() {
        let repo = tempfile::tempdir().unwrap();
        traces_dir(repo.path());

        assert_eq!(
            enforcement_liveness(repo.path(), None),
            EnforcementLiveness::NotInstalled
        );
        assert!(!enforcement_liveness(repo.path(), None).is_enforcing());
    }

    /// Only `live-<session>.jsonl` files are the enforcement lane. A whole-trace
    /// `.json` artifact in the same directory is a different lane entirely, and
    /// counting it would report enforcement from a file no hook ever wrote —
    /// exactly the false pass this module exists to prevent.
    #[test]
    fn a_whole_trace_artifact_is_not_evidence_of_enforcement() {
        let repo = installed_repo();
        let traces = traces_dir(repo.path());
        std::fs::write(traces.join("run-1.json"), "{\"spans\":[]}\n").unwrap();
        std::fs::write(traces.join("notes.txt"), "not a log\n").unwrap();

        assert_eq!(
            enforcement_liveness(repo.path(), None).silence(),
            Some(Silence::NoLiveLogs)
        );
    }

    /// A torn final line is a write that never completed; counting it would
    /// report a record the log cannot read back.
    #[test]
    fn an_unterminated_tail_is_not_counted_as_a_record() {
        let repo = installed_repo();
        let traces = traces_dir(repo.path());
        std::fs::write(traces.join("live-torn.jsonl"), r#"{"type":"test.run""#).unwrap();

        assert_eq!(
            enforcement_liveness(repo.path(), None).silence(),
            Some(Silence::LiveLogsEmpty)
        );

        std::fs::write(
            traces.join("live-torn.jsonl"),
            format!("{}\n{}", record_line(0), r#"{"type":"test.run""#),
        )
        .unwrap();
        assert_eq!(
            enforcement_liveness(repo.path(), None),
            EnforcementLiveness::Enforcing {
                live_logs: 1,
                spans: 1
            },
            "the committed record counts; the torn tail does not"
        );
    }

    /// The window is the "since a given time" half of the surface: records that
    /// exist but predate the window are silence, not enforcement. A session
    /// asking "is the plane running *now*" must not be answered with last
    /// month's log.
    #[test]
    fn records_outside_the_window_are_silence_not_enforcement() {
        let repo = installed_repo();
        let traces = traces_dir(repo.path());
        std::fs::write(
            traces.join("live-old.jsonl"),
            format!("{}\n", record_line(0)),
        )
        .unwrap();

        let future = SystemTime::now() + Duration::from_secs(3_600);
        assert_eq!(
            enforcement_liveness(repo.path(), Some(future)).silence(),
            Some(Silence::NoneInWindow)
        );

        let past = SystemTime::now() - Duration::from_secs(3_600);
        assert!(enforcement_liveness(repo.path(), Some(past)).is_enforcing());
    }

    /// Records are summed across sessions: a repo with several live logs is one
    /// enforcing plane, not several partial answers.
    #[test]
    fn records_are_summed_across_session_logs() {
        let repo = installed_repo();
        let traces = traces_dir(repo.path());
        std::fs::write(
            traces.join("live-a.jsonl"),
            format!("{}\n{}\n", record_line(0), record_line(1)),
        )
        .unwrap();
        std::fs::write(traces.join("live-b.jsonl"), format!("{}\n", record_line(0))).unwrap();

        assert_eq!(
            enforcement_liveness(repo.path(), None),
            EnforcementLiveness::Enforcing {
                live_logs: 2,
                spans: 3
            }
        );
    }

    /// The wire form is what a downstream consumer keys on, so the three states
    /// have to be three distinct tags and the silence reason has to survive.
    #[test]
    fn the_wire_form_carries_the_state_and_its_reason() {
        let silent = EnforcementLiveness::InstalledButSilent {
            silence: Silence::NoLiveLogs,
        };
        assert_eq!(
            serde_json::to_value(&silent).unwrap(),
            serde_json::json!({"state":"installed-but-silent","silence":"no-live-logs"})
        );
        assert_eq!(
            serde_json::to_value(EnforcementLiveness::NotInstalled).unwrap(),
            serde_json::json!({"state":"not-installed"})
        );
        assert_eq!(
            serde_json::to_value(EnforcementLiveness::Enforcing {
                live_logs: 1,
                spans: 2
            })
            .unwrap(),
            serde_json::json!({"state":"enforcing","live_logs":1,"spans":2})
        );
    }

    /// A default-constructed value is what a report predating this field
    /// deserializes to. It must land on the conservative side: never enforcing.
    #[test]
    fn the_default_state_is_never_enforcing() {
        assert!(!EnforcementLiveness::default().is_enforcing());
    }
}
