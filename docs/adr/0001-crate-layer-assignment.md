# 0001 — Every library crate gets exactly one architectural layer

**Status:** Accepted. Recorded here in 2026-08 from the decision as it already
stands in `CLAUDE.md` and `crates/aoa/tests/architecture_doc.rs`.

## Context

AOA is a workspace of narrow library crates plus a CLI composition root. A
contributor deciding where new code goes has to answer two questions: which
crate owns this reason to change, and may that crate depend on what the code
needs. Neither question has an answer the compiler can give — `cargo build`
accepts any acyclic dependency graph, and clippy does not read prose.

Left to itself the graph drifts. `aoa-falsify-build` was split out of
`aoa-falsify`, never added to the documented list, and became the only crate in
the workspace with no declared layer at all. Nothing failed.

## Decision

`CLAUDE.md`'s "Architecture Overview" assigns every library crate to exactly one
layer, and that assignment is the authoritative answer to "where does this new
code go". The layers are ordered, dependencies run one way along that order, and
library crates must not depend on CLI concerns.

The layer list itself is not restated here. It changes as crates are added or
split, and a second copy would be a copy that drifts — this record fixes the
rule, `CLAUDE.md` holds the current assignment, and
`crates/aoa/tests/architecture_doc.rs` fails the workspace tests when the two
disagree. What that test checks, and where it deliberately stops, is documented
in its own module comment rather than duplicated here for the same reason.

## Consequences

The document is a contract rather than a comment, so adding a crate is
incomplete until `CLAUDE.md` names it. The test parses the bullet block by
anchoring on the prose above it, which means editing that sentence is a
breaking change to the test and not merely a wording choice.

Enforcement grew in scope rather than arriving whole, and the rule was not
weakened to let it. Where a known inversion could not be fixed at the moment the
check that catches it landed, it goes in an explicit, bead-tracked exception
list — one named edge that a reader can go read the reason for, rather than a
softened check that silently permits the whole class.

Placements that read as surprising to a newcomer — why `aoa-corpus` is
measurement rather than inputs, why `aoa-falsify-build` sits with decisions — are
argued in `CLAUDE.md` at the point of assignment. Settling them beside the
assignment is what stops them being re-argued per reader.

## Where this lives

- `CLAUDE.md`, "Architecture Overview" — the layer assignment itself, and the
  reasoning behind the placements that read as surprising.
- `crates/aoa/tests/architecture_doc.rs` — the enforcement, and the current
  scope of what it does and does not check.
