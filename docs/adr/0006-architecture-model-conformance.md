# 0006 — `architecture/model.c4` is a maintained contract, and a test holds it to the crates

**Status:** Accepted. Recorded here in 2026-08 from aoa-enzj8, which found the
model asserting a dependency that aoa-4s25v had removed months earlier, three
more that never existed, and no component at all for `aoa-domain`.

## Context

The repository carries two architecture documents. CLAUDE.md's "Architecture
Overview" assigns every library crate to a layer, and
`crates/aoa/tests/architecture_doc.rs` fails the build when that assignment
stops matching the crates on disk (record 0001). `architecture/model.c4` is a
LikeC4 model of the same system, and it is the one that gets *published*: a
GitHub Actions workflow renders it to a website on every push to main.

Nothing read it. So it drifted, in every direction a diagram can:

- `aoa.measure.recommend -> aoa.migrate.migrator` claimed a production
  dependency that aoa-4s25v deleted when it made `aoa-recommend` take owned
  `AvailableFix` rows and moved the registry projection into the CLI.
- `metrics -> scipGraph`, `gap -> metrics` and `falsify -> metrics` claimed
  three more dependencies that the manifests have never declared. Each is a
  hand-off the composition root performs.
- The single arrow crossing into the write-gated half pointed the wrong way.
  The real edge is `aoa-migrate -> aoa-audit`, and the model did not draw it.
- `aoa-domain` had no element, from the day aoa-ynqcn created it as the bottom
  of the stack.

A wrong diagram is worse than no diagram: a reviewer who has none knows to read
the code, and one who trusts a published picture does not. `cargo build` does
not read a diagram, and neither does clippy, so nothing about the ordinary
workflow would ever have said any of this.

## Decision

`architecture/model.c4` is a **maintained contract**, not a snapshot, and
`crates/aoa/tests/architecture_model.rs` enforces it:

- **Membership.** Every crate under `crates/` is exactly one element, and no
  two elements claim the same crate.
- **Soundness.** Every relationship whose two endpoints both resolve to crates
  is a production dependency of the source on the target. `[dev-dependencies]`
  does not count, for the reason record 0001 already gives: it enters no
  consumer's build graph.
- **Completeness, between library crates.** Every production dependency between
  two library crates is drawn. Without this half, the cheap way to satisfy the
  soundness check is to delete the arrow rather than fix what it describes.
- **The CLI is the one asymmetry.** `aoa` depends on all eighteen libraries, so
  drawing every one of its edges would say nothing. Its arrows are drawn where
  they carry meaning — where the composition root is what joins two crates that
  do not know each other — and are checked for truth, never exhaustiveness.
- **`#conceptual`** marks a relationship that is deliberately not a code edge.
  Today there is one: R0's verdict gating whether `migrate` is worth trusting
  on a repository is read by an operator, not called by a crate. Each use is
  registered with a reason in the test, and the registration is deleted the
  moment the edge becomes real — the same discipline record 0001's
  `LAYER_EXCEPTIONS` follows.

Two documents, two contracts, and they are not variants of each other.
`architecture_doc.rs` owns layer membership and direction; this one owns
model-to-manifest soundness and completeness. A crate can sit in the right layer
and still be drawn with an edge it does not have — that is exactly what happened.

Containers in the model group by concern and do **not** mirror CLAUDE.md's
layers: `recommend` sits under measurement in the model and in decisions in the
layer list, and `policy`/`enforce` sit under gates rather than under controlled
changes. The model's header says so, because a reader who assumes a layer-for-
layer mapping will draw conclusions neither document supports.

## Consequences

Splitting a crate now means editing the diagram. That is the cost, and it is the
point: `aoa-falsify-build` was split out of `aoa-falsify` and never added to
CLAUDE.md's layer list, which is the omission record 0001 exists to prevent, and
the same omission is what left `aoa-domain` invisible here.

Restating a removed dependency is not free either. Three of the four wrong
arrows had to be re-drawn as CLI hand-offs rather than simply deleted, because
the thing each described does still happen — just in the composition root. The
test cannot tell whether an arrow's *label* is true, only whether its endpoints
are, so a fix that keeps a plausible label on a newly-drawn arrow can still
mislead. Labels are a review concern; the graph is a test concern.

Deliberately left unenforced, so that it is a stated limit rather than an
oversight:

- **`views.c4`'s dynamic views.** They are walkthroughs of what happens at run
  time, not claims about the build graph — `operator -> falsify` and `audit ->
  operator` are steps in the same file. Holding them to the dependency rule
  would be a category error and would force deleting accurate narrative. Their
  endpoints are checked; their steps are not. The one step that had gone
  factually wrong about the run itself (`recommend -> migrator` in
  `migrateFlow`) was routed through the CLI, which is what performs it.
- **LikeC4's own validator.** It runs in the Pages workflow, which triggers on
  push to main, so a model that this test parses but LikeC4 does not would reach
  main before anything said so. Adding `likec4 validate` to PR CI means either
  an unpinned network fetch on every pull request or a pinned version to
  maintain; filed as its own unit of work rather than folded in here.
