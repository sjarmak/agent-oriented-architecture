# 0009: `fix_oversized` replaces two files with two renames, archive first

**Status:** Accepted. Recorded in 2026-10 from aoa-a03ih, the second review
round of aoa-4xshs. The review found that the previous change described the
replacement of the context file and its archive as a single step. It is two
renames, and no filesystem call makes two renames atomic together. The project
lead ruled on 2026-10-05 that the window between them is accepted and that what
holds inside it has to be stated and tested.

## Context

`aoa_budget::fix_oversized` brings an over-budget context file under its
ceiling. It writes the full original body to a sibling `<stem>.archive.md` and
rewrites the context file as a summary that names the archive.

Each file is replaced by writing a temporary file beside it, giving that file
the mode, owner and attributes of the one it replaces, and renaming it into
place. One rename is atomic. The two renames are separate system calls, so a
failure or a kill can land between them.

## Decision

Both temporary files are prepared before either is placed, so every failure
that can be found early (a file that cannot be written, a directory that cannot
hold a new file) is found while nothing has changed.

The archive is placed first and the context file second. The order is the
decision: between the two renames the context file still holds the full
original body, and the archive holds the same body. No state in that window is
missing content.

The window itself is accepted. `fix_oversized` does not try to roll the archive
back when the context file cannot be placed, because a rollback is a third
rename with a window of its own.

## Consequences

Inside the window, meaning after the archive is placed and before the context
file is placed:

- A failure to place the context file returns an error and leaves the context
  file untouched. The archive already holds the original body. The same is true
  when the archive is in place and the sync of its directory then fails.
- Running `fix_oversized` again reaches the result an uninterrupted run would
  have reached. The archive is written whole each time and never appended to,
  so the second run cannot duplicate its content.
- An archive that stood before the run is replaced by the first rename. An
  uninterrupted run replaces it as well, so the window loses nothing a complete
  run would have kept.

A temporary file is removed when its replacement is refused. When it can be
neither placed nor removed, the returned error is
`BudgetError::TempFileLeftBehind` and names the file.

What this record does not cover:

- A process killed inside the window cannot remove the context file's temporary
  file. It stays in that file's directory under a `.aoa-budget-` prefix. It is
  not linked from any context file, so it does not enter a closure, and nothing
  removes it on the next run.
- A run that fails after both files are placed (the sync of the context file's
  directory, or the recount that follows the write) returns an error with the
  summary already in place. Running `fix_oversized` on a context file that is
  already a summary archives the summary over the original. That is a defect
  outside this decision and is tracked as aoa-rqnb8.
- Two files with more than one error. When the archive is refused and its own
  temporary file is left behind, a failure to remove the context file's
  temporary file as well is not reported; the error names the first.

## Where this lives

- `crates/aoa-budget/src/fix.rs` holds the order: `fix_placing` prepares both
  replacements, then places the archive, then the context file.
- `crates/aoa-budget/src/replace.rs` holds one replacement. `Placing` is the
  crate-private pair of steps (rename, directory sync) that the tests fail one
  at a time; `fix_oversized` always passes the real pair.
- The unit tests in `crates/aoa-budget/src/fix.rs` fail the second rename and
  the first directory sync, check that both files hold the original body, and
  compare a rerun against a tree fixed in one run.
