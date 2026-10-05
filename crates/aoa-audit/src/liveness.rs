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
//! Silence is emphatically not a pass. [`crate::audit`] raises it as a Tier-1
//! finding, the same rule the metrics side adopted after aoa-xo8y0: an absent
//! measurement is missing evidence, not a benign zero.
//!
//! The fourth state exists because an absent `.aoa/traces` is not a measurement
//! of anything. Registration and telemetry live on opposite sides of the
//! `.gitignore`: `.claude/settings.json` and the wrapper are tracked, `.aoa/` is
//! ignored, and `aoa observe --enforce` provisions the traces directory as part
//! of installing. So a tree with a registered plane and no traces directory is
//! one that git handed a registration into and in which no local install ever
//! ran — the state of every clean checkout, every CI runner, and every fresh
//! worktree. Reading it as silence asserted that a plane had stopped emitting on
//! the strength of never having been watched, and made the repository's own
//! `--fail-on tier1` self-audit unpassable by any checkout (aoa-rsixa).
//!
//! Unobserved is still not a pass either: it raises the same finding, at Tier-3,
//! the tier this crate reserves for what it asserts without a measurement to
//! back it. What it cannot do is distinguish "no session has run here" from
//! "every hook here failed before it could write" — both leave exactly the same
//! empty tree, and [`crate::hook_set`] answers the parts of that question a
//! repository's own files can answer.

use std::fs::DirEntry;
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::observe::TRACES_SUBDIR;
use crate::planes::{runtime_hooks, RuntimeHooks};
use crate::tier::Tier;

/// The filename shape the enforcement hooks append to, one per session. Owned by
/// `aoa_enforce::live_log`, which sits a layer above this crate and so cannot be
/// depended on from here; `crate::observe` already encodes the same lane when it
/// refuses to let a whole-trace write land on a live log.
const LIVE_LOG_PREFIX: &str = "live-";
const LIVE_LOG_EXTENSION: &str = ".jsonl";

/// Whether this repository's runtime enforcement plane is producing records.
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
    ForeignHooks,
    /// The hook set is installed and this tree holds no enforcement telemetry at
    /// all: `<repo>/.aoa/traces` does not exist. The module doc derives why that
    /// is the state of every clean checkout.
    ///
    /// Not evidence that a running plane fell silent, and not evidence that one
    /// is healthy: nothing here has been watched.
    InstalledUnobserved,
    /// The hook set is installed, its telemetry directory exists, and no
    /// enforcement record reached the live log. The plane reads as present from
    /// every configuration surface and enforces nothing.
    InstalledButSilent {
        silence: Silence,
    },
    /// The hook set is installed and the live log holds records.
    Enforcing {
        /// Live logs contributing spans within the window.
        live_logs: usize,
        /// Spans counted within the window, one per committed record line.
        spans: u64,
    },
}

impl EnforcementLiveness {
    /// The punch-list finding this state raises, if any.
    ///
    /// The tier is decided here, by the state, rather than by the plane: what
    /// separates these two findings is not which plane they are about but what
    /// evidence the audit is holding. A silence is measured — the telemetry
    /// directory is there and empty — and lands on the evidence-backed tier. An
    /// unobserved plane is an assertion with no measurement under it, which is
    /// what [`Tier::Tier3`] is for.
    ///
    /// Both raise a finding: dropping the unobserved one would put an unwatched
    /// plane on the pass side of the ledger, which is the reading this module
    /// exists to prevent.
    #[must_use]
    pub(crate) fn finding(&self) -> Option<LivenessFinding> {
        match self {
            EnforcementLiveness::NotInstalled
            | EnforcementLiveness::ForeignHooks
            | EnforcementLiveness::Enforcing { .. } => None,
            EnforcementLiveness::InstalledUnobserved => Some(LivenessFinding {
                tier: Tier::Tier3,
                headline: "installed but never observed running",
                reason: UNOBSERVED_REASON,
                cost_unit: "unobserved plane",
            }),
            EnforcementLiveness::InstalledButSilent { silence } => Some(LivenessFinding {
                tier: Tier::Tier1,
                headline: "installed but silent",
                reason: silence.reason(),
                cost_unit: "silent plane",
            }),
        }
    }

    pub(crate) fn missing_plane_note(&self) -> Option<&'static str> {
        matches!(self, EnforcementLiveness::ForeignHooks).then_some(FOREIGN_HEADLINE)
    }

    /// One line naming the state, for the human register. The silent state
    /// shouts: it is the one an operator has been reading as healthy.
    #[must_use]
    pub fn render_line(&self) -> String {
        match self {
            EnforcementLiveness::NotInstalled => {
                "enforcement plane: not installed (no runtime hook set)".to_string()
            }
            EnforcementLiveness::ForeignHooks => {
                format!("enforcement plane: {FOREIGN_HEADLINE} — {FOREIGN_REASON}")
            }
            EnforcementLiveness::InstalledUnobserved => format!(
                "enforcement plane: installed but NEVER OBSERVED RUNNING — {UNOBSERVED_REASON}; \
                 this repo is not known to be enforcing"
            ),
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

/// Why an installed plane reads as unobserved. Stated once, so the punch-list
/// title and the human line cannot describe the same state differently.
const UNOBSERVED_REASON: &str = "no .aoa/traces directory exists, so this tree \
                                 carries no enforcement telemetry to read";

const FOREIGN_HEADLINE: &str = "agent hooks present, AOA enforcement not installed";

const FOREIGN_REASON: &str = ".claude/settings.json configures hooks that are not AOA's enforce \
                              hook set, and AOA does not observe them";

/// What a liveness state contributes to the audit's punch-list.
///
/// Carried as one value rather than as separate tier and text accessors: the
/// tier and the phrase justifying it are the same judgment, and splitting them
/// is how a Tier-1 headline ends up over Tier-3 evidence.
///
/// Crate-internal: it is consumed by [`crate::audit`] and flattened into
/// [`crate::PunchItem`], which is the public shape. Widen it when something
/// outside this crate needs the tier and reason without going through a punch
/// item, not before.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct LivenessFinding {
    /// The evidence tier this finding is reported at.
    pub(crate) tier: Tier,
    /// The state's headline, read after `enforcement plane `.
    pub(crate) headline: &'static str,
    /// Why the plane reads that way.
    pub(crate) reason: &'static str,
    /// The unit its measured cost of 1 is counted in.
    pub(crate) cost_unit: &'static str,
}

/// Why an installed plane counts as silent.
///
/// Separate variants because they are separate defects, and every one of them is
/// read off a telemetry directory that exists — an absent one is
/// [`EnforcementLiveness::InstalledUnobserved`] and carries no `Silence` at all,
/// because there was nothing there to find quiet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Silence {
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
    match runtime_hooks(repo) {
        RuntimeHooks::Missing => return EnforcementLiveness::NotInstalled,
        RuntimeHooks::ForeignOnly => return EnforcementLiveness::ForeignHooks,
        RuntimeHooks::Installed => {}
    }
    let silent = |silence| EnforcementLiveness::InstalledButSilent { silence };

    let entries = match std::fs::read_dir(repo.join(TRACES_SUBDIR)) {
        Ok(entries) => entries,
        // Absent, not silent: nothing here was ever watched (aoa-rsixa).
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return EnforcementLiveness::InstalledUnobserved
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
                "PostToolUse":[
                    {"matcher":"Bash","hooks":[{"command":"aoa enforce record"}]},
                    {"matcher":"Write|Edit|MultiEdit|NotebookEdit","hooks":[{"command":"aoa enforce commit"}]}
                ],
                "PreToolUse":[{"matcher":"Write|Edit|MultiEdit|NotebookEdit","hooks":[{"command":"aoa enforce check"}]}],
                "PostToolUseFailure":[{"matcher":"Write|Edit|MultiEdit|NotebookEdit","hooks":[{"command":"aoa enforce fail"}]}],
                "PermissionDenied":[{"matcher":"Write|Edit|MultiEdit|NotebookEdit","hooks":[{"command":"aoa enforce deny"}]}]
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

    /// The silent state carrying `silence`. Assertions compare the whole state
    /// rather than a reason pulled out of it, so a result that is silent for the
    /// right reason but in the wrong variant cannot pass.
    fn silent(silence: Silence) -> EnforcementLiveness {
        EnforcementLiveness::InstalledButSilent { silence }
    }

    fn record_line(seq: u64) -> String {
        format!(r#"{{"type":"test.run","source":"native","seq":{seq},"attributes":{{}}}}"#)
    }

    /// Criterion (e): four separate facts, four separate answers. Reported as
    /// one sequence because the point is that they differ — asserting each in
    /// isolation would pass just as well against a single collapsed variant.
    ///
    /// The first step is the one that moved (aoa-rsixa): an absent traces
    /// directory is not the weakest kind of silence, it is the absence of the
    /// measurement silence is read from.
    #[test]
    fn absent_empty_and_populated_traces_dirs_are_four_distinct_facts() {
        let repo = installed_repo();

        assert_eq!(
            enforcement_liveness(repo.path(), None),
            EnforcementLiveness::InstalledUnobserved,
            "an absent traces dir is not silence: nothing here was ever watched"
        );

        let traces = traces_dir(repo.path());
        assert_eq!(
            enforcement_liveness(repo.path(), None),
            silent(Silence::NoLiveLogs),
            "an existing but empty traces dir is not an absent one"
        );

        std::fs::write(traces.join("live-s1.jsonl"), "").unwrap();
        assert_eq!(
            enforcement_liveness(repo.path(), None),
            silent(Silence::LiveLogsEmpty),
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
            enforcement_liveness(repo.path(), None),
            silent(Silence::NoLiveLogs)
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
            enforcement_liveness(repo.path(), None),
            silent(Silence::LiveLogsEmpty)
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
            enforcement_liveness(repo.path(), Some(future)),
            silent(Silence::NoneInWindow)
        );

        let past = SystemTime::now() - Duration::from_secs(3_600);
        assert_eq!(
            enforcement_liveness(repo.path(), Some(past)),
            EnforcementLiveness::Enforcing {
                live_logs: 1,
                spans: 1
            }
        );
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

    /// The wire form is what a downstream consumer keys on, so every state has
    /// to be its own tag and the silence reason has to survive.
    #[test]
    fn the_wire_form_carries_the_state_and_its_reason() {
        assert_eq!(
            serde_json::to_value(silent(Silence::NoLiveLogs)).unwrap(),
            serde_json::json!({"state":"installed-but-silent","silence":"no-live-logs"})
        );
        assert_eq!(
            serde_json::to_value(EnforcementLiveness::NotInstalled).unwrap(),
            serde_json::json!({"state":"not-installed"})
        );
        assert_eq!(
            serde_json::to_value(EnforcementLiveness::InstalledUnobserved).unwrap(),
            serde_json::json!({"state":"installed-unobserved"}),
            "the unobserved state is its own tag and carries no silence reason"
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

    /// The tier each state's finding is reported at, asserted as one table
    /// because the whole decision is the contrast between the two rows that
    /// raise something. A measured silence is evidence-backed; an unobserved
    /// plane is the same claim with nothing under it.
    #[test]
    fn the_finding_tier_follows_the_evidence_not_the_plane() {
        assert_eq!(
            EnforcementLiveness::InstalledButSilent {
                silence: Silence::NoLiveLogs,
            }
            .finding()
            .map(|finding| finding.tier),
            Some(Tier::Tier1)
        );
        assert_eq!(
            EnforcementLiveness::InstalledUnobserved
                .finding()
                .map(|finding| finding.tier),
            Some(Tier::Tier3),
            "nothing was measured, so the finding cannot claim to be measured"
        );
        assert_eq!(EnforcementLiveness::NotInstalled.finding(), None);
        assert_eq!(
            EnforcementLiveness::Enforcing {
                live_logs: 1,
                spans: 1
            }
            .finding(),
            None
        );
    }

    #[test]
    fn foreign_hooks_are_a_state_of_their_own_and_annotate_the_missing_plane() {
        let repo = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(repo.path().join(".claude")).unwrap();
        std::fs::write(
            repo.path().join(".claude/settings.json"),
            r#"{"hooks":{"PreToolUse":[{"hooks":[{"command":"./tools/guard.sh"}]}]}}"#,
        )
        .unwrap();

        let liveness = enforcement_liveness(repo.path(), None);
        assert_eq!(liveness, EnforcementLiveness::ForeignHooks);
        assert_eq!(
            serde_json::to_value(&liveness).unwrap(),
            serde_json::json!({"state": "foreign-hooks"})
        );

        assert_eq!(liveness.finding(), None);
        assert_eq!(
            liveness.missing_plane_note(),
            Some("agent hooks present, AOA enforcement not installed")
        );
        assert_eq!(EnforcementLiveness::NotInstalled.missing_plane_note(), None);

        let line = liveness.render_line();
        assert!(
            line.contains("agent hooks present, AOA enforcement not installed"),
            "{line}"
        );
    }

    /// An unobserved plane still reaches the punch-list and the human line, and
    /// neither may read as health. Dropping it there was the tempting fix for
    /// aoa-rsixa and would have put an unwatched plane on the pass side.
    #[test]
    fn an_unobserved_plane_is_never_rendered_as_healthy() {
        let line = EnforcementLiveness::InstalledUnobserved.render_line();
        assert!(
            line.contains("NEVER OBSERVED RUNNING") && line.contains("not known to be enforcing"),
            "the unobserved line must not read as a pass: {line}"
        );
        assert!(
            EnforcementLiveness::InstalledUnobserved.finding().is_some(),
            "an unobserved plane still raises a finding; only its tier moved"
        );
    }

    /// A default-constructed value is what a report predating this field
    /// deserializes to. It must land on the conservative side: never enforcing.
    #[test]
    fn the_default_state_is_never_enforcing() {
        assert_eq!(
            EnforcementLiveness::default(),
            EnforcementLiveness::NotInstalled
        );
    }
}
