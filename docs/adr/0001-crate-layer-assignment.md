# 0001 — Every library crate gets exactly one architectural layer

**Status:** Accepted. Recorded here in 2026-08 from the decision as it already
stands in `CLAUDE.md` and `crates/aoa/tests/architecture_doc.rs`.

## Context

AOA is a workspace of sixteen library crates plus a CLI composition root. A
contributor deciding where new code goes has to answer two questions: which
crate owns this reason to change, and may that crate depend on what the code
needs. Neither question has an answer the compiler can give — `cargo build`
accepts any acyclic dependency graph, and clippy does not read prose.

Left to itself the graph drifts. `aoa-falsify-build` was split out of
`aoa-falsify`, never added to the documented list, and became the only crate in
the workspace with no declared layer at all. Nothing failed.

## Decision

`CLAUDE.md`'s "Architecture Overview" assigns every library crate to exactly one
of four layers — inputs, measurement, decisions and reporting, controlled
changes and enforcement — and that assignment is the authoritative answer to
"where does this new code go". The intended dependency direction is inputs →
measurement → decisions → CLI, and library crates must not depend on CLI
concerns.

Membership in that list is enforced: `crates/aoa/tests/architecture_doc.rs`
fails the workspace tests when a crate under `crates/` has no layer, or when a
layer names a crate that no longer exists.

## Consequences

The document is a contract rather than a comment, so adding a crate is
incomplete until `CLAUDE.md` names it. The test parses the bullet block by
anchoring on the prose above it, which means editing that sentence is a
breaking change to the test and not merely a wording choice.

Enforcement is deliberately membership only, not edge direction. Two
pre-existing inversions would fail a layer-order check today: `aoa-bench` sits
in inputs but is pulled into measurement (`aoa-ynqcn`), and `aoa-recommend`
sits in decisions but depends on `aoa-migrate` in controlled changes
(`aoa-4s25v`). Shipping the order check before those edges are settled would
have required an exception list, which is a slower path to a weaker rule. Those
two beads extend the test once their edges are resolved.

Two layer placements that read as surprising are settled and recorded in
`CLAUDE.md` rather than re-argued per reader: `aoa-corpus` is measurement, not
inputs, because it mines revert history in order to join those outcomes onto
`aoa-construct`'s classification; `aoa-falsify-build` sits with decisions
because it depends on `aoa-falsify` for the shape it produces.

## Where this lives

- `CLAUDE.md`, "Architecture Overview" — the layer assignment itself.
- `crates/aoa/tests/architecture_doc.rs` — the enforcement, and the rationale
  for its deliberately narrow scope.
