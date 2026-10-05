# 0004 — A test skips on a precondition CI can satisfy; it is ignored on one CI can never satisfy

**Status:** Accepted. Recorded here in 2026-08 from the split that already
stands in the workspace, after aoa-m1yqw made the R0 campaign test `#[ignore]`d
and left two conventions side by side with nothing saying which was which.

## Context

Some tests need something the machine may not have: a tool on `PATH`, a vendored
dependency tree, a directory of data produced elsewhere. There are two ways to
make such a test stop early, and they report differently.

`eprintln!("SKIP …")` followed by `return` leaves the test running. It passes,
and the summary counts it among the passes. libtest captures a passing test's
output, so the notice is invisible in a green run — nobody reading the run sees
that the test checked nothing.

`#[ignore]` stops the test from running at all. The summary says `1 ignored`,
which is a line a reader can act on, and the test is still reachable on demand
with `--ignored`.

The invisibility of the first form is the whole problem. It is harmless when the
test really runs somewhere that matters, and it is a lie when it does not.

## Decision

The convention is chosen by one question: **can CI ever satisfy this
precondition?**

- **Yes** — use `eprintln!("SKIP …")` + `return`, and make CI install whatever
  the test needs. CI is the run whose greenness is load-bearing; it exercises
  the test for real, so the skip is a local-dev affordance and its invisibility
  costs nothing. Skipping locally is then a statement about the developer's
  machine, not about the code.
- **No** — use `#[ignore]`, with a reason string naming what to set and how to
  run it. A printed notice here would report `ok` in CI for a test that scanned
  nothing, which is the failure mode worth spending a summary line to avoid.

The question is about the precondition, not about how inconvenient it is. A tool
that CI *could* install but currently does not is a `yes` with a missing CI
step, not a `no`.

## Consequences

Both conventions stay in the workspace, and the split is now the rule rather
than an accident:

- `crates/aoa-migrate/tests/imports_python.rs` and `imports_typescript.rs` probe
  for `ruff`, `node`, and the vendored ESLint. `.github/workflows/rust-ci.yml`
  installs all three, in both the `ci` and `coverage` jobs, so these tests run
  for real on every push and the printed notice never decides whether a green
  run means anything. The CI steps are load-bearing: dropping one would silently
  convert these tests into the case this record forbids.
- `crates/aoa-bench/tests/exposure_scan.rs` holds
  `real_r0_campaign_matches_documented_exposure_and_held_out_provenance`, which
  scans the R0 campaign codeprobe produced on an operator's machine. No runner
  can hold that input, so the test is `#[ignore]`d and names
  `AOA_R0_CAMPAIGN_RUNS` in its reason string.

- `crates/aoa-scip-graph/tests/walk_scope.rs`,
  `crates/aoa-budget/tests/closure_bounds.rs`,
  `crates/aoa-lint/tests/lint.rs` and
  `crates/aoa-audit/tests/context_root_containment.rs` hold tests that need the
  kernel to refuse the process a read: an unlistable directory in the first, a
  mode-000 file, a mode-000 directory a link points beneath and a mode-000
  directory a link steps back out of in the second, the same step back out in
  the third, and a mode-000 directory above the audit's context root in the
  fourth. A process running as root is refused none of them, so
  each test probes whether the seal took and prints the notice when it did not.
  CI satisfies the precondition without an install step, because the hosted
  runner executes the suite as an unprivileged user; the notice is for a
  developer running the tests as root in a container. Before aoa-wxfoc the
  first of these returned `ok` without a notice when the seal did not take.
  Moving the CI jobs into a root container would turn all of them into the forbidden
  case, and nothing in the workspace would notice: the registry test checks the
  tool installs, not the user the suite runs as.
- `crates/aoa-path-trust/src/nofollow.rs` and
  `crates/aoa-enforce/src/live_log/open.rs` each hold one unit test with the
  same precondition and the same answer.
  `an_unreadable_node_fails_closed_rather_than_reading_as_absent` needs the
  kernel to refuse an lstat beneath a mode-000 directory, and
  `a_trace_directory_that_cannot_be_created_reports_a_failed_open` needs it to
  refuse a mkdir in a mode-555 directory. Root is refused neither, so each
  probes whether the seal took and prints the notice when it did not. Before
  aoa-n7va7 both returned `ok` without a notice in that case. These are the
  first classified sites inside a `src/` tree; the registry walks every `.rs`
  file under `crates/`, so a unit test is counted the same way as an
  integration test.

- `crates/aoa-budget/tests/fix_replace.rs` holds two notices. The first is the
  same root-user case as the group above: the tests that expect a mode-444 file
  or a mode-555 directory to refuse a rewrite probe whether this process can
  write past that mode, and share one notice through a helper. The second belongs to the test that an
  archive keeps its extended attributes, which needs a temporary directory on a
  filesystem that stores `user.*` attributes; the test prints the notice when
  setting one answers "not supported". CI is expected to satisfy both without
  an install step, the first because the suite runs unprivileged and the second
  because the hosted runner's temporary directory is on its ext4 root disk. The
  second expectation is not checked by anything in the workspace: a runner
  whose temporary directory moved to a filesystem without user attributes
  would report `ok` for that one test.

The `exposure_scan.rs` test also prints a SKIP notice, which is not a third convention: the notice
sits *inside* the ignored test and reports an unset `AOA_R0_CAMPAIGN_RUNS` to
somebody who asked for the test by name with `--ignored`. `#[ignore]` has
already made the CI-honesty decision by then. A variable that *is* set must name
a real directory, so a typo fails rather than skipping.

A third case is what this record exists for. Answer the question above, add the
source to the registry in `crates/aoa/tests/environment_dependent_skips.rs` with
its per-convention counts, and record the reason here next to the cases already
described. That test fails on any skip site this record has not classified, so
the decision cannot be made by copying whichever nearby example the author
happened to read first.

It counts sites rather than blessing files, because a file listed once would
otherwise pre-approve every skip later added anywhere inside it — which is how a
registry stops describing the workspace without anything going red. It also
holds the three premises this record leans on but does not itself state: that
the ADR names every source the registry classifies, that every classified source
cites the ADR back, and that `.github/workflows/rust-ci.yml` still installs
`node`, the vendored ESLint, and `ruff`. That last one is the load-bearing half
of the `yes` branch. Delete the `ruff` step and the seven `imports_python.rs`
tests report `ok` in CI having checked nothing, with no other check in the
workspace noticing.

The scan is textual, so its reach is these two idioms spelled the way this
record spells them. A test that returns early with no notice at all is invisible
to it, and so is an attribute a macro emits. Neither is a gap to close by
widening the search until it guesses: a test that announces nothing is a defect
in that test, not an unclassified convention. What the enforcement guarantees is
that no *detectable* skip site is unclassified — and because a missed site is a
silent green run while a spurious match is a loud red gate, the matching is
deliberately biased toward the second. A match that is not a skip site at all
goes in the test's `NOT_A_SKIP` list, never into the registry, which would put a
classification into this record that nobody made.

## Where this lives

- `CLAUDE.md`, "Conventions & Patterns" — the standing rule.
- `crates/aoa/tests/environment_dependent_skips.rs` — the registry of classified
  sites and their counts, and the workspace tests that fail when an unclassified
  site appears, when the registry and this record stop describing the same set,
  when a citation of this record stops resolving, or when CI stops installing
  what the printed-notice sites depend on.
- `.github/workflows/rust-ci.yml` — the `node`, ESLint, and `ruff` installs that
  keep the `aoa-migrate` adapter tests on the `yes` side of the question.
