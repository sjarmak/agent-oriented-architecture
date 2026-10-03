# 0005 — A checkout with no telemetry is unobserved, not silent: the audit gates only on measurements it holds

**Status:** Accepted. Recorded here in 2026-08 alongside the split
`crates/aoa-audit/src/liveness.rs` now makes, after aoa-rsixa found the
repository's own CI self-audit gate failing on every clean checkout.

## Context

`aoa audit` answers whether this repository's runtime enforcement plane is
emitting records. Two halves of that question live on opposite sides of
`.gitignore`, and nothing said so:

- **Registration is tracked.** `.gitignore` force-includes
  `.claude/settings.json` and `.claude/hooks/aoa-enforce`, so every clone
  receives a fully registered hook set.
- **Telemetry is ignored.** `.aoa/` is ignored, and `aoa observe --enforce`
  creates `.aoa/traces/` as part of installing. No clone receives it.

So a clean checkout carries a registered plane and no telemetry whatsoever, and
that is not a transient condition — it is what a checkout *is*. The audit read
the absent traces directory as `Silence::TracesDirectoryAbsent` and raised the
Tier-1 finding "enforcement plane installed but silent". Because
`.github/workflows/rust-ci.yml` runs `audit --repo . --fail-on tier1`, the
repository's own dogfood gate could not be passed by any checkout, any CI
runner, or any fresh worktree. It exited 2 on a pristine `git archive` of `HEAD`.

The gate had never actually run: the liveness check landed in `aadcff4` after
the last Rust CI run (`67751cb`, 2026-07-29), which is 97 commits back. So this
was a gate about to go red for the first time, not one being tolerated.

The tempting reading — seed `.aoa/traces` in the repo or in CI — fabricates the
artifact that answers the question, and a seeded empty directory relocates the
Tier-1 to `Silence::NoLiveLogs` anyway. The other tempting reading, that
detecting installation from tracked files is itself the defect, does not hold:
answering "is the hook set registered" from the registration is exactly right.
What was wrong was treating the *absence of a measurement* as the measurement.

## Decision

The audit gates on measurements it holds, and an absent `.aoa/traces` is not
one. `EnforcementLiveness` therefore reports four states rather than three, and
the tier of the finding follows the evidence rather than the plane:

- **`InstalledUnobserved`** — registered, and this tree holds no telemetry at
  all. Raises a **Tier-3** finding. `crates/aoa-audit/src/tier.rs` defines
  Tier-3 as asserted-but-unsupported, which is precisely the standing of "this
  plane is not known to be running" when nothing was ever watched.
- **`InstalledButSilent`** — registered, the install's own traces directory
  exists, and it produced nothing. Raises a **Tier-1** finding, unchanged. The
  directory that exists and stays empty *is* the measurement.

This is the one punch item whose tier `EnforcementPlane::tier` does not decide.
Tier selection lives on the liveness state, in
`EnforcementLiveness::finding`, beside the state that determines it, so the
headline and the tier justifying it cannot drift apart.

**It partially reverses aoa-dpluh.** That record's rule was that an installed
plane emitting nothing is always Tier-1, and `README.md` said so publicly. Both
now hold only where a measurement exists.

## Consequences

**An unobserved plane is still a finding.** Dropping it from the punch-list
would put an unwatched plane on the pass side of the ledger, which is the
reading aoa-dpluh existed to prevent, and would have left
`--fail-on tier1` gating on nothing that a checkout can express: `Tier::Tier1`
has exactly one producer in the crate, `EnforcementPlane::tier`, so the CI
self-audit step is an enforcement-plane gate and nothing else. The finding stays
on `items`, so `aoa recommend` and `aoa report` still carry it, and the human
register still refuses to read as health.

**The residual risk, named.** An absent traces directory does not mean "no
session has run here". It means "no hook has ever succeeded here", which is a
superset containing the aoa-dpluh scenario itself — hooks registered, every one
invoking a binary nothing on the host could resolve, the directory never
created. The audit cannot separate those two from repository state, and after
this change it no longer pretends to: the Tier-3 finding and the human line
report the ambiguity rather than resolving it toward either answer.

**Deleting the evidence now downgrades the finding, and that is accepted.** Say
it plainly, because it is the sharpest consequence and the one an operator can
act on: a repository whose install *did* run and whose hooks emit nothing is a
Tier-1 measured silence that fails the gate, and `rm -rf .aoa` turns it into the
non-gating Tier-3 state. `.aoa/` is ignored and self-ignoring, so a routine
`git clean -xdf` reaches it too — this is available by accident, not only by
intent. Before this change deletion bought nothing, since an absent directory
was Tier-1 as well.

That prior protection was not a separable thing that could have been kept. It
*was* the behavior that made the gate unpassable by every checkout: one rule
produced both, and there is no repository state that tells "never installed
here" apart from "installed here, evidence since removed". Keeping the
protection means keeping a gate no CI run can pass, which is not a gate.

So the risk is accepted, on three conditions. The finding survives the deletion
— Tier-3, on `items`, and loud in the human register — so only the exit code
moves and a reader is still told. The behavior is pinned by
`deleting_the_telemetry_directory_downgrades_a_measured_silence` in
`crates/aoa/tests/cli_sections/enforce_liveness.rs`, so changing it later is a
deliberate act against a red test rather than a silent drift. And closing it
properly is tracked as aoa-zswh6, which needs evidence of an install that
survives a working-tree wipe — a different mechanism, not a different tier.

`hook_set_defect` answers the parts a repository's own files *can* answer — an
install that is behind, ahead, unstamped, or missing its wrapper — and the
wrapper is loud on stderr when it cannot resolve a binary. Neither is a
substitute for a session that actually ran, and neither gates: `hook_set_defect`
reaches the operator as a `warning:` line from
`crates/aoa/src/commands/audit.rs` and never touches `exit_code`. After this
change, nothing in the audit fails a run over a registered plane that has never
been seen to work. That is the true statement, and the one to hold against
aoa-zswh6.

**A wire break, accepted rather than papered over.** `Silence::TracesDirectoryAbsent`
is removed, because a `Silence` reason for a state that is not silence would be
a type-level lie. `AuditReport`'s hand-written `Deserialize` uses
`#[serde(default)]` on `enforcement_liveness`, which fires on a *missing* field
and not on a present-but-unknown tag, so a report persisted before this change
carrying `{"state":"installed-but-silent","silence":"traces-directory-absent"}`
now fails to deserialize. No in-tree consumer reads a persisted `AuditReport`.

**CI step ordering is now load-bearing.** The self-audit step passes only
because nothing before it creates `.aoa/`. Today that holds: it runs before
`cargo test`, `enforcement_liveness` is read-only by construction, and the tests
work in temporary directories. A step added ahead of it that runs `aoa observe`
against the checkout would provision an empty traces directory and turn the run
red with a Tier-1 `no-live-logs` finding — a correct verdict about a repository
nobody is enforcing in, and an unguessable diagnosis. Order the self-audit ahead
of anything that writes `.aoa/`.

## Where this lives

- `crates/aoa-audit/src/liveness.rs` — the liveness states, and
  `EnforcementLiveness::finding`, which assigns the tier from the state.
- `crates/aoa-audit/src/audit.rs`, `fn plane_items` — the one punch item whose
  tier comes from liveness rather than from the plane.
- `.gitignore` — the tracked-registration / ignored-telemetry split this record
  exists to name.
- `.github/workflows/rust-ci.yml`, the `Self-audit` step — the gate that could
  not be passed, and the ordering constraint above.
- `crates/aoa/tests/cli_sections/enforce_liveness.rs` — the boundary tests, in
  both directions: a checkout with no telemetry passes `--fail-on tier1` and
  still raises a Tier-3 finding, an installed plane whose traces directory
  exists and stays empty still fails it, and the accepted downgrade-by-deletion
  above is pinned rather than left to be rediscovered.
- aoa-zswh6 — the follow-up that would close the deletion gap with durable
  install evidence.
