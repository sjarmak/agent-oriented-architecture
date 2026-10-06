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
| [0004](0004-environment-dependent-test-skips.md) | A test skips on a precondition CI can satisfy; it is ignored on one CI can never satisfy | Accepted |
| [0005](0005-enforcement-liveness-in-a-checkout.md) | A checkout with no telemetry is unobserved, not silent: the audit gates only on measurements it holds | Accepted |
| [0006](0006-architecture-model-conformance.md) | `architecture/model.c4` is a maintained contract: every arrow is a real dependency, and every dependency between library crates is an arrow | Accepted |
| [0007](0007-git-environment-and-config-for-subprocesses.md) | A git subprocess inherits only what can make git refuse, loudly: the shared strip, why the resolver inherits machine config, and why the data readers do not | Accepted |
| [0008](0008-path-trust-crate.md) | `aoa-path-trust` owns the filesystem and git trust primitives; `aoa-trace` owns the trace format and nothing else | Accepted |
| [0009](0009-nested-policy-only-tightens.md) | A nested repository's policy may only tighten: a write is refused if any enclosing governed repository's policy refuses it | Accepted |
| [0010](0010-fix-oversized-two-file-replacement.md) | `fix_oversized` replaces two files with two placements, archive first: the window between them is accepted and loses no content, and an archive that already stands refuses the run | Accepted |
| [0011](0011-windows-filesystem-residuals.md) | Windows keeps five sites by path where Unix acts through a held handle: the Unix arms are the tested specification, the Windows arms stay by-path until CI runs Windows | Accepted |

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
