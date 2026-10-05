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
- **The CLI is the one asymmetry.** `aoa` depends on all twenty libraries, so
  drawing every one of its edges would say nothing. Its arrows are drawn where
  they carry meaning — where the composition root is what joins two crates that
  do not know each other — and are checked for truth, never exhaustiveness.
- **Every element that claims to be code stands for a crate.** It links one, or
  it groups elements that do. An element claims to be code by sitting inside the
  `aoa` system, or by being declared `container` or `component` anywhere in the
  file. Naming a crate in an element's *title* is not standing for it: the title
  is what the diagram renders and no test reads it, so
  `recommendGhost = component 'aoa-recommend'` with no link resolved to the
  outside world, had its arrows dropped unchecked, and drew the exact
  `aoa-recommend -> aoa-migrate` dependency this record exists to remove — past
  a green suite. That is the grouping rule's defect one level down, found the
  same way, by a reviewer building the claim that could not fail. An element
  with no code behind it is an actor or an `externalSystem`.

  The two halves of "claims to be code" are both load-bearing, and the second
  was learned the hard way *twice*. Stated only over what sits inside the system
  boundary, the rule was satisfied by moving the ghost one line out of `aoa` and
  deleting two levels of indentation: still a `component`, still rendered, still
  drawing its arrow, and green again. A rule about a claim has to be about the
  claim, not about where in the file it was written.
- **No arrow may be drawn at a grouping that stands for several crates.** A
  container with exactly one crate under it resolves to that crate, so an arrow
  there is checked like any other; one over several — `aoa.substrate`,
  `aoa.measure` — names neither end of what it claims, and there is nothing to
  check it against. Left unchecked it would be the one shape of claim that can
  sit in a published diagram, read as authoritative, and never be wrong enough
  to fail. Both reviewers of this change found the same gap independently, and
  each demonstrated it with `aoa.substrate -> aoa.migrate`, which passed every
  test before the rule existed. Draw the arrow between the components that have
  the dependency.
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

This record originally listed a second limit, **LikeC4's own validator**: it ran
only in the Pages workflow, which triggers on push to main, so a model this test
parses but LikeC4 rejects reached main before anything said so. Closing it meant
choosing between an unpinned network fetch on every pull request and a pinned
version to maintain, which is why it was deferred rather than folded in.

aoa-vvvtx made that choice. `.github/workflows/rust-ci.yml` now carries a
`likec4` job, so `likec4 validate architecture` runs on every pull request, and
a model LikeC4 rejects fails that pull request instead of the deploy after it.
It is its own job rather than a step in `ci` because it needs a newer Node than
that job's ESLint install: the pinned release declares the Node it supports in
its `engines` field, and both workflows have to provide it.

The version is pinned to one exact release rather than floating on `@latest`:

- A gate whose verdict depends on when it ran is not a gate. Floating lets an
  upstream release change what "valid" means between a pull request's run and
  the merge run, and turn every open pull request red with no commit here.
- `npx -y likec4@latest` fetches and executes whatever was published minutes
  ago, on every pull request. A pin makes a new likec4 arrive as a diff somebody
  reads.
- It is already the convention for CI tools in that workflow (`ruff`,
  `cargo-llvm-cov`). The cost is a bump commit, which both already pay.

`likec4-pages.yml` runs the same release in all three places it runs likec4: the
`export json` step names it, and the two `likec4/actions` steps take it through
the action's `likec4-version` input instead of the likec4 bundled with the
action. That is what lets the gate predict the deploy. To bump, change every
occurrence in both workflows in one commit;
`crates/aoa/tests/likec4_pin.rs` fails the workspace tests if any of them names
a different or a moving version, if an action step drops the input, or if the
validating step is removed, made conditional, or path-filtered.

What the pin does not reach: `npx` still resolves the dependency tree below
likec4 on every run, since there is no lockfile for it as there is for the
vendored ESLint, and `likec4/actions@v1` is itself a moving tag whose wrapper
code can change under the pinned likec4.

That job's reach is wider than this test's, not narrower: LikeC4 reads all four
sources under `architecture/`, while the parser here opens only `model.c4` and
`views.c4`. A dangling `instanceOf` in `deployment.c4` passes every assertion
above and fails `likec4 validate`. Neither subsumes the other: LikeC4 checks
that the model is well-formed and says nothing about whether its arrows match
the crates on disk, which is the only thing this test checks.
