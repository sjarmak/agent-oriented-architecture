# 0012: An answer-only codeprobe trial is read from a private copy of the run's `trace.db`

**Status:** Accepted. Recorded in 2026-10 from aoa-m12mo, the second review
round of aoa-vmj78. The first round (aoa-vmj78, ruling (a)) decided that a
transcript holding no agent events falls back to codeprobe's run-wide trace
database and fails loudly when that database is absent, unreadable or not the
schema this reader knows. The second round found the reader opening the
database in place, accepting an `events` table without its primary key,
capping the database file but not its write-ahead log, loading rows without a
bound, and deriving no database for a run dir spelled `.`; the project lead
ruled on 2026-10-06 that each is fixed as recorded here.

## Context

codeprobe runs `claude -p` and writes each trial's `agent_output.txt`. Current
codeprobe writes only the agent's extracted final answer there, so the
transcript the shim was built to walk observes no tool call. codeprobe's
telemetry adapter ingests every trial's tool calls into one SQLite database,
`<runs_dir>/trace.db`, beside the per-config run directories, keyed by the
config label that is also the run directory's name.

That database belongs to codeprobe. It is written in WAL mode, so a reader
that opens it in place creates `-wal` and `-shm` sidecars beside it, or fails
outright when the directory denies writes. Rows codeprobe has written but not
yet checkpointed live only in the write-ahead log, so a reader that copies the
database file alone misses them. The file is attacker-shaped input: its size,
its schema and its row count are all unbounded from this crate's point of view.

## Decision

The transcript is still tried first. A trial whose transcript parses to at
least one agent event is read from the transcript and reports
`trace_source: stream_json`. Only `ShimError::NoAgentEvents` falls through to
the database, and a transcript that fails for any other reason fails the trial.

The database is never opened in place. `trace.db` and, when present,
`trace.db-wal` are copied into a private temporary directory and the copy is
opened with `SQLITE_OPEN_READ_ONLY` and `PRAGMA query_only`. Nothing is
created beside the source, and a run directory of mode 555 holding a database
of mode 444 is read and left byte-identical. Copying the write-ahead log is
what lets uncheckpointed rows be read; SQLite replays it against the copy.

One byte cap, `MAX_TRACE_DB_BYTES` (1 GiB), covers the database and its
write-ahead log together, and it is enforced twice: against the two files'
sizes before any byte is copied, and against the bytes actually copied, so a
file that grows under the copy still fails. Rows are loaded with
`LIMIT cap + 1` and more than the span cap fails `ShimError::TooManySpans`
rather than truncating the trace.

The reader accepts exactly codeprobe's schema version 1, which `TRACE_DB_SCHEMA`
exports as DDL and the unit tests hold to the check:

- `schema_migrations(version INTEGER PRIMARY KEY, applied_at REAL NOT NULL)`
  holding exactly the row `version = 1`;
- `events` with the thirteen columns `run_id`, `config`, `task_id`,
  `event_seq`, `ts`, `event_type`, `tool_name`, `tool_input`, `tool_output`,
  `duration_ms`, `input_tokens`, `output_tokens`, `bytes_written`, their
  declared types and nullability, and the composite primary key
  `(run_id, config, task_id, event_seq)` in that order.

Column names, types, nullability and primary-key ordinals are all compared
through `pragma_table_info`, and any difference is `TraceDbError::Schema`
naming what was found and what is accepted. A trial is the rows whose `config`
is the run directory's name and whose `task_id` is the trial's; rows from more
than one `run_id` are `AmbiguousRun`, and no rows at all is `NoEvents`. Three
event types are read: `tool_use` becomes a span through the same mapping the
transcript walk uses, `trace_truncated` is `Truncated`, and anything else is
`MalformedEvent`.

The run directory is canonicalized before its parent and name are taken, so
`--codeprobe-run .` inside a config directory, `..` inside a trial, and a
symbolic link to the directory all locate the same database. The root
directory locates none, and a run directory that cannot be resolved is an
error, not an absent database.

### What the database cannot say

codeprobe's adapter records tool calls, not tool results. A trial read from
`trace.db` therefore keeps every write at `write.attempt`: nothing can settle
it to `write.committed`, `write.blocked` or `write.failed`, so such a trial
contributes no edits to `F_edit`. Each record names its source in
`trace_source` so a consumer can tell the two provenances apart.

## Consequences

- No read of a codeprobe run leaves a sidecar behind, and a run archived
  read-only is still scorable.
- The private copy costs the database's size in temporary space and in time,
  bounded by the cap.
- A copy taken while codeprobe is still writing may be torn between the
  database file and its log. SQLite reports such a copy as malformed or reads
  the frames that were complete, and either outcome fails or scores one
  trial, never the run. Reading a run that is still being written is not a
  supported use.
- The row bound counts every event row for the trial, including rows that
  produce no span, so a trial near the span cap can fail on rows that would
  not have become spans.
- A codeprobe schema bump fails every fallback read until this reader is
  taught the new version. That is the first round's loud-failure ruling and
  is intended.

## Where this lives

- `crates/aoa-codeprobe-shim/src/trace_db.rs` holds the copy, the open, the
  schema check, the bounded load and the trial build; `src/spans.rs` is the
  span builder both sources feed; `src/error.rs` states `TraceDbError`.
- `crates/aoa-bench/src/codeprobe_run.rs` `trace_db_location` resolves the
  database and config from the run directory.
- `crates/aoa/src/commands/eval_run.rs` tries the transcript, then the
  database, and reports `trace_source`.
- `crates/aoa-codeprobe-shim/tests/trace_db.rs` and
  `crates/aoa/tests/cli_sections/eval_run_trace_db.rs` prove the sidecar,
  read-only, cap, schema and path-spelling behaviour at the public boundaries.
