# Decision records

Standing decisions about how AOA is built, one file each. Every record here
documents a decision that was already made and already acted on somewhere in the
repository; the record names that place so a reader can check the decision
against the code rather than against a memory of a conversation.

Scan this index before proposing new work. A proposal that re-opens a decision
recorded here has to say which record it overturns and why; a proposal that
duplicates one is already built.

| # | Decision | Status |
|---|----------|--------|
| [0001](0001-crate-layer-assignment.md) | Every library crate gets exactly one architectural layer, and CLAUDE.md is where the assignment lives | Accepted |
| [0002](0002-trace-schema-ownership.md) | `aoa-trace` owns the trace wire format; every other crate reads it from there | Accepted |
| [0003](0003-held-out-provenance.md) | Held-out provenance is load-bearing evidence: an unprovable held-out claim demotes the repository rather than the standard | Accepted |

## What belongs here

A decision belongs in this directory when reversing it would change more than
one crate, or when the reason for it is not visible from the code that
implements it. Layer assignment qualifies on both counts: the layer list reads
as documentation but is enforced by a test, and its dependency direction is the
answer to "where does this new code go".

Routine choices — a function's name, an error type's shape, which of two equal
crates hosts a helper — do not. They live in the code and in the commit that
introduced them.

## Format

Each record states the decision, the context that forced it, its consequences,
and where it is recorded or enforced today. New records take the next number and
get a row in the table above; `crates/aoa/tests/decision_records.rs` fails the
workspace build if a record is added without one, or if the table links a file
that does not exist.
