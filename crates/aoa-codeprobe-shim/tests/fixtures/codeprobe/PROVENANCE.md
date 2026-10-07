# codeprobe-written trace.db

`trace.db` was written by codeprobe itself, not by the DDL the shim states in
`TRACE_DB_SCHEMA`, so `tests/trace_db.rs::a_database_codeprobe_wrote_reads_as_the_transcript_it_ingested`
checks the reader against the producer rather than against its own idea of
the producer.

- Producer: `~/projects/codeprobe` at commit
  `fd9ff15e01b69dd71ee539006fea3631c4062cb7` (2026-10-07). The working tree
  carried uncommitted edits under `src/codeprobe/cli/` and `tests/skills/`
  only; `src/codeprobe/trace/` was at the committed state.
- Input: `../agent_output.txt`, the sanitized stream-json transcript the
  transcript tests already read, so the two readers can be compared span for
  span.
- Command, run from this directory:

  ```
  python3 -I make_trace_db.py ~/projects/codeprobe/src ../agent_output.txt trace.db
  ```

  `make_trace_db.py` opens a `TraceRecorder` with run id `aoa-fixture-run`
  and ingests the transcript as config `baseline`, task `task-001`. It
  recorded eight events: seven `tool_use` rows (Grep, Read,
  `mcp__codegraph__codegraph_search`, WeirdCustomTool, Write, Edit, Bash)
  and one `result` row, plus `schema_migrations` version 1.
- The database is in WAL journal mode as codeprobe leaves it; the WAL was
  checkpointed into the main file on close, so no `-wal` or `-shm` sibling is
  checked in.

Regenerate with the same command when codeprobe changes its schema, and
record the new commit here. A reader that no longer accepts the regenerated
file is the failure this fixture exists to surface.
