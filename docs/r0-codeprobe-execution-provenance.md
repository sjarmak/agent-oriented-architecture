# R0 codeprobe execution-basis provenance

Recorded on 2026-08-05 under `aoa-32n55`. This is the AOA-side execution-basis
record for the preserved R0 campaign and the halted confirmatory extension. It
does not change an admission result, vote, score, or verdict.

## Finding

The exact codeprobe commit that produced every completed arm is **not
recoverable from the available evidence**. The preserved campaign's aggregate
envelopes identify codeprobe version `0.13.0`; the halted extension records no
codeprobe version. Neither evidence set records a Git commit, checkout path,
dirty-tree state, or immutable package identity. Git reflogs show what `main`
pointed to while the outcome artifacts were written, but they cannot prove that
the process ran from `main`, from that checkout, or from a clean tree. The
contemporaneous `main` values below are therefore forensic bounds, not
retroactive pins.

Outcome windows are the minimum and maximum mtimes of `scoring.json` files in
each run root, in America/New_York (`-04:00`). They exclude later aggregation
and AOA post-processing writes. `unknown` is an evidence result, not a guessed
SHA.

## Preserved five-repository campaign

The campaign used three configurations per repository and seed: `baseline`,
`aoa_migrated` (the repo arm below), and `harness_swap` (the harness arm
below). The 15 completed baseline configurations contain 210 outcome artifacts.
They are part of the execution-basis record even though the falsification
comparison names only the two changed arms.

### Baseline configurations

| Repository | Seed | Configuration | Completion | Outcome window | Producing codeprobe SHA | Contemporaneous codeprobe `main` observation |
| --- | ---: | --- | --- | --- | --- | --- |
| requests | 1 | baseline | complete (14/14) | 2026-07-04 21:24:06–21:27:21 | unknown | `cc3a352663efff20771923dc21e064bd6229974a` |
| requests | 2 | baseline | complete (14/14) | 2026-07-04 21:36:39–21:40:26 | unknown | `cc3a352663efff20771923dc21e064bd6229974a` |
| requests | 3 | baseline | complete (14/14) | 2026-07-04 21:47:55–21:50:47 | unknown | `cc3a352663efff20771923dc21e064bd6229974a` |
| isort | 1 | baseline | complete (14/14) | 2026-07-28 14:24:31–14:29:32 | unknown | `a5cee977639c2f33a5494429c35fe7334232f44d` |
| isort | 2 | baseline | complete (14/14) | 2026-07-28 11:24:38–11:29:34 | unknown | `f86980147a910bb24eef8c2f942b2ca088337ce6` |
| isort | 3 | baseline | complete (14/14) | 2026-07-28 14:41:56–14:47:27 | unknown | `f6e4162051f02c9c12b2ced8cb06364cb8c64d7d` |
| marshmallow | 1 | baseline | complete (7/7) | 2026-07-28 17:13:58–17:16:15 | unknown | `f6e4162051f02c9c12b2ced8cb06364cb8c64d7d` |
| marshmallow | 2 | baseline | complete (7/7) | 2026-07-28 17:25:16–17:34:20 | unknown | `f6e4162051f02c9c12b2ced8cb06364cb8c64d7d` |
| marshmallow | 3 | baseline | complete (7/7) | 2026-07-28 18:42:44–18:44:16 | unknown | `f6e4162051f02c9c12b2ced8cb06364cb8c64d7d` |
| rich | 1 | baseline | complete (19/19) | 2026-07-28 18:52:38–18:58:07 | unknown | `f6e4162051f02c9c12b2ced8cb06364cb8c64d7d` |
| rich | 2 | baseline | complete (19/19) | 2026-07-28 19:13:46–19:18:34 | unknown | `f6e4162051f02c9c12b2ced8cb06364cb8c64d7d` |
| rich | 3 | baseline | complete (19/19) | 2026-07-28 22:17:32–22:23:00 | unknown | `f6e4162051f02c9c12b2ced8cb06364cb8c64d7d` |
| gunicorn | 1 | baseline | complete (16/16) | 2026-07-28 18:52:24–19:02:58 | unknown | `f6e4162051f02c9c12b2ced8cb06364cb8c64d7d` |
| gunicorn | 2 | baseline | complete (16/16) | 2026-07-28 22:12:18–22:23:07 | unknown | `f6e4162051f02c9c12b2ced8cb06364cb8c64d7d` |
| gunicorn | 3 | baseline | complete (16/16) | 2026-07-28 22:49:11–23:00:36 | unknown | `f6e4162051f02c9c12b2ced8cb06364cb8c64d7d` |

### Changed arms

| Repository | Seed | Arm | Outcome window | Producing codeprobe SHA | Contemporaneous codeprobe `main` observation |
| --- | ---: | --- | --- | --- | --- |
| requests | 1 | harness | 2026-07-04 21:27:49–21:32:48 | unknown | `cc3a352663efff20771923dc21e064bd6229974a` |
| requests | 1 | repo | 2026-07-04 21:33:06–21:36:15 | unknown | `cc3a352663efff20771923dc21e064bd6229974a` |
| requests | 2 | harness | 2026-07-04 21:40:50–21:44:42 | unknown | `cc3a352663efff20771923dc21e064bd6229974a` |
| requests | 2 | repo | 2026-07-04 21:45:01–21:47:37 | unknown | `cc3a352663efff20771923dc21e064bd6229974a` |
| requests | 3 | harness | 2026-07-04 21:51:12–21:57:52 | unknown | `cc3a352663efff20771923dc21e064bd6229974a` |
| requests | 3 | repo | 2026-07-04 21:58:09–22:02:09 | unknown | `cc3a352663efff20771923dc21e064bd6229974a` |
| isort | 2 | repo | 2026-07-28 11:24:04–11:41:27 | unknown | `f86980147a910bb24eef8c2f942b2ca088337ce6` |
| isort | 2 | harness | 2026-07-28 11:30:00–11:36:12 | unknown | `f86980147a910bb24eef8c2f942b2ca088337ce6` |
| isort | 1 | harness | 2026-07-28 14:30:11–14:36:40 | unknown | `a5cee977639c2f33a5494429c35fe7334232f44d` |
| isort | 1 | repo | 2026-07-28 14:36:53–14:41:31 | unknown | `main` moved from `a5cee977639c2f33a5494429c35fe7334232f44d` to `01145befbb03edd25be3d40d3256598a306fd6da`, reset to `4d16a6776be24d546fdcb2de08f2fcc7fd5db186`, then moved to `f6e4162051f02c9c12b2ced8cb06364cb8c64d7d` during the arm |
| isort | 3 | harness | 2026-07-28 14:47:53–16:18:00 | unknown | `f6e4162051f02c9c12b2ced8cb06364cb8c64d7d` |
| isort | 3 | repo | 2026-07-28 16:18:41–16:22:41 | unknown | `f6e4162051f02c9c12b2ced8cb06364cb8c64d7d` |
| marshmallow | 1 | harness | 2026-07-28 17:17:08–17:19:07 | unknown | `f6e4162051f02c9c12b2ced8cb06364cb8c64d7d` |
| marshmallow | 1 | repo | 2026-07-28 17:21:06–17:23:38 | unknown | `f6e4162051f02c9c12b2ced8cb06364cb8c64d7d` |
| marshmallow | 2 | harness | 2026-07-28 18:35:01–18:38:24 | unknown | `f6e4162051f02c9c12b2ced8cb06364cb8c64d7d` |
| marshmallow | 2 | repo | 2026-07-28 18:39:39–18:41:22 | unknown | `f6e4162051f02c9c12b2ced8cb06364cb8c64d7d` |
| marshmallow | 3 | harness | 2026-07-28 18:45:07–18:48:30 | unknown | `f6e4162051f02c9c12b2ced8cb06364cb8c64d7d` |
| marshmallow | 3 | repo | 2026-07-28 18:49:52–18:51:48 | unknown | `f6e4162051f02c9c12b2ced8cb06364cb8c64d7d` |
| rich | 1 | harness | 2026-07-28 18:58:42–19:07:40 | unknown | `f6e4162051f02c9c12b2ced8cb06364cb8c64d7d` |
| rich | 1 | repo | 2026-07-28 19:08:00–19:13:17 | unknown | `f6e4162051f02c9c12b2ced8cb06364cb8c64d7d` |
| rich | 2 | harness | 2026-07-28 19:19:45–22:10:39 | unknown | `f6e4162051f02c9c12b2ced8cb06364cb8c64d7d` |
| rich | 2 | repo | 2026-07-28 22:11:01–22:17:01 | unknown | `f6e4162051f02c9c12b2ced8cb06364cb8c64d7d` |
| rich | 3 | harness | 2026-07-28 22:23:44–22:34:17 | unknown | `f6e4162051f02c9c12b2ced8cb06364cb8c64d7d` |
| rich | 3 | repo | 2026-07-28 22:34:43–22:41:42 | unknown | `f6e4162051f02c9c12b2ced8cb06364cb8c64d7d` |
| gunicorn | 1 | harness | 2026-07-28 19:03:40–19:19:03 | unknown | `f6e4162051f02c9c12b2ced8cb06364cb8c64d7d` |
| gunicorn | 1 | repo | 2026-07-28 19:19:22–22:11:55 | unknown | `f6e4162051f02c9c12b2ced8cb06364cb8c64d7d` |
| gunicorn | 2 | harness | 2026-07-28 22:24:18–22:36:21 | unknown | `f6e4162051f02c9c12b2ced8cb06364cb8c64d7d` |
| gunicorn | 2 | repo | 2026-07-28 22:36:47–22:48:31 | unknown | `f6e4162051f02c9c12b2ced8cb06364cb8c64d7d` |
| gunicorn | 3 | harness | 2026-07-28 23:01:33–23:14:58 | unknown | `main` moved from `f6e4162051f02c9c12b2ced8cb06364cb8c64d7d` to `398463844963d058e29e0355f740a419916ad116` during the arm |
| gunicorn | 3 | repo | 2026-07-28 23:15:32–23:25:27 | unknown | `398463844963d058e29e0355f740a419916ad116` |

The July 28 transitions do not establish that execution semantics changed
during an arm; they establish that commit-time inference is not a valid
substitute for a recorded execution pin.

## Halted confirmatory extension

| Repository | Seed | Arm | Completion | Outcome window | Producing codeprobe SHA | Contemporaneous codeprobe `main` observation |
| --- | ---: | --- | --- | --- | --- | --- |
| sqlparse | 1 | harness | complete (16/16) | 2026-08-04 15:37:41–15:42:22 | unknown | `6f326c3bda9060e37aca0bee060652dde41ddd4e` |
| sqlparse | 1 | repo | complete (16/16) | 2026-08-04 15:42:46–15:55:05 | unknown | `6f326c3bda9060e37aca0bee060652dde41ddd4e` |
| websockets | 1 | harness | complete (12/12) | 2026-08-04 15:55:40–15:57:35 | unknown | `6f326c3bda9060e37aca0bee060652dde41ddd4e` |
| websockets | 1 | repo | complete (12/12) | 2026-08-04 15:58:04–16:00:32 | unknown | `6f326c3bda9060e37aca0bee060652dde41ddd4e` |
| sqlparse | 2 | harness | complete (16/16) | 2026-08-04 16:03:02–16:08:03 | unknown | `6f326c3bda9060e37aca0bee060652dde41ddd4e` |
| sqlparse | 2 | repo | partial (8/16) | 2026-08-04 16:08:28–16:17:58 | unknown | `6f326c3bda9060e37aca0bee060652dde41ddd4e` |

No sqlparse seed 3 arm and no websockets seed 2 or 3 arm produced a score.
Every preserved score in this halted run is `0.0`; the exposure and
instrument-void consequences remain as recorded in
[the confirmatory extension record](r0-confirmatory-extension-prep.md).

## Deliberate execution-basis amendment

The halted extension exposed the container `TASK_REPO_ROOT` scoring fault. The
codeprobe correction is deliberately adopted as an execution-basis amendment:

1. `d799a98a1c7e880e402d36803c11de26ad6d6c34` — preserves and stages the
   scorer workspace in containers and adds the regression coverage.
2. `53da421202f527e08364eb7886ad4ea19ab658f7` — simplifies that regression and
   is the minimum amended descendant adopted as the scoring-correction basis.

This amendment does not claim that either SHA produced the earlier trials.
Commit `9e38d2156cf158f549fe14118b3de1b9e1c6404a` later changed explicit task
timeout handling. A rerun may include that change only by naming its full SHA
as a separate, deliberate execution-basis choice; it must not silently follow
the moving `main` branch.

## Mandatory precondition for any R0 rerun

Before a dry run, seed, or agent starts, the operator must record one full
40-hex codeprobe commit for the rerun and verify that the executing checkout is
clean and exactly at that commit. The same pin applies to every repository,
seed, and arm in that rerun. The campaign record must retain:

- the full codeprobe SHA and whether it includes the `53da421` scoring
  amendment and the `9e38d21` timeout change;
- the absolute executing checkout path and a clean-tree assertion;
- the repository, seed, arm, and outcome-artifact time window produced under
  that pin; and
- any later basis change as a dated amendment made before affected trials run.

A branch name, tag, package version, filesystem path, or "current main" is not
a pin. If the recorded SHA does not match the executing checkout, execution
must stop. Existing unpinned artifacts remain usable only with the `unknown`
provenance finding above; their producer identity must not be backfilled from
commit timestamps.
