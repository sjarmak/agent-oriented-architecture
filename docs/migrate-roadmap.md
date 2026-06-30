# Migrate Roadmap — the R0-gated `CodeFix` candidate surface

`aoa-migrate` ships **two** fixes today: `NavigabilityAnchorFix` and the three
language adapters of `DeadImportFix`. The rest of the vision's MIGRATE pillar
(pillar 4, opt-in) is unbuilt **by design** — gated behind a live R0 `proceed`
verdict (`aoa-dhk.1`). This document is the durable map of that gated surface:
each unbuilt migration enumerated as a `CodeFix`-trait candidate, so the moment
R0 proceeds the surface can grow **one fix at a time**, each earning admission on
its own evidence.

> Companion to `docs/r0_runbook.md`. The runbook defines the gate; this doc
> defines what is allowed to grow once the gate opens.

## Why the surface is gated (read first)

MIGRATE is the **highest-reversal-cost layer**: a fix mutates a real checkout.
The `/converge` phasing makes Wave-1 migrate scope strictly conditional on R0
`proceed`, because the premortem's dominant failure is *baking unproven
affordances into the layer that is hardest to walk back*. A fix that has not
shown — on the held-out oracle — that it helps is a liability, not a feature.

So this doc **enumerates and scopes**; it does not authorize building. No
candidate here becomes code until (a) R0 returns `proceed` and (b) the candidate
clears the admission gate below. Adding a `CodeFix` impl before both hold is the
exact mistake the gate exists to prevent.

## The `CodeFix` contract (what a candidate must fit)

Every candidate is realized as one `impl CodeFix` (`crates/aoa-migrate/src/fix.rs`).
The trait shape constrains every candidate:

- **`plan(&self, repo) -> Vec<PlannedChange>` is read-only.** A fix is a
  *planner*: it reads the checkout and returns the changes it *would* make,
  writing nothing. The reversible apply/rollback is the engine's job
  (`apply.rs`), not the fix's.
- **`eligibility_note()`** records the R0 precondition under which the fix is a
  construct-valid *code-layer* treatment. It is carried per-fix into the manifest
  (`FixEligibility`), because preconditions differ (the navigability fix's
  "no README auto-injection" is meaningless for a compiler-backed fix).
- **`provenance()`** is optional environment provenance — the reproducibility
  *verification* half. A pure function of the tree records none; a
  toolchain-backed fix records what ran.
- Registration is a single line in **`all_fixes()`** — the one source of truth
  for which migrations exist. Growing the surface = adding one entry there.

A candidate that cannot be expressed as a read-only planner emitting reversible
`PlannedChange`s does not belong in this crate.

## The two shipped fixes (the baseline)

| Fix id | PRD | Audit signal driven down | Languages | Leak-channel guardrail |
|--------|-----|--------------------------|-----------|------------------------|
| `navigability-anchor` | — (pre-R5 best-practice) | `NavigabilityAnchor` | **language-agnostic** (pure function of the directory tree) | README content is a tree index, blind to file bodies → cannot transcribe a held-out answer. Ineligible for harnesses that auto-inject READMEs into agent context (`NAVIGABILITY_ELIGIBILITY`). |
| `dead-imports` / `dead-imports-python` / `dead-imports-typescript` | — | `UnusedImportProxy` | **per-language adapter** (rustc, ruff F401, ESLint) | strictly-subtractive; runs in an isolated copy; one unused-import lint class only. Adapter declares its own unchecked preconditions (cfg-gated, `TYPE_CHECKING`). |

These set the two shapes every candidate falls into: **language-agnostic
structural/policy generation** (like the anchor) or **per-language tool-backed
edit** (like dead-imports). The shape decides whether the cross-language adapter
problem (below) applies.

## Admission gate — how a candidate earns its place (R10/R14)

A candidate is *enumerated* here. It is *admitted* (turned on in `all_fixes()`)
only when both hold:

1. **R0 is `proceed`.** The pillar is live at all.
2. **`aoa eval --compare` returns `Label::Good` for the fix.** That label is
   exactly (`crates/aoa-gap/src/compare.rs`):

   ```
   held_out_delta > 0.0  &&  gap_delta <= 0.0
   ```

   i.e. the fix **improves held-out pass** *and* **holds-or-reduces the
   reward-hacking gap**. A fix that lifts a visible metric while widening the gap
   is `NotGood` and stays out — that is the anti-Goodhart clamp. Each candidate's
   audit signal is the *symptom* it addresses, never the definition of its
   success; success is the held-out delta under a non-widening gap.

Each candidate is therefore a **hypothesis**, not a commitment: "removing this
finding improves held-out without gaming it." Admission is empirical, one fix at
a time, never assumed.

A second, fix-specific gate stacks on top: the **leak-channel guardrail**
(`docs/r0_runbook.md` guardrail 2). Any candidate that emits **prose** an agent
reads (templates, provenance headers, CI comments) must be authored **blind to
the held-out oracle**, or it can leak a task answer into the repo. Purely
structural candidates (CODEOWNERS from the mutation graph, planes from the policy
file) carry no prose leak channel and clear this trivially. The per-candidate
tables below flag which kind each is.

## The candidate surface

Each subsection is one unbuilt migration. Fields: **PRD requirement** it
realizes · **eligibility note** (when it may run) · **audit signal** it drives
down · **construct-validity guardrail** (how admission is judged) ·
**per-language applicability**.

### R5 — policy-file → three-plane generation

- **Candidate fix id:** `policy-planes`
- **PRD:** R5 (single `aoa-policy.yaml` → runtime + pre-commit + CI planes).
- **What it does:** `aoa-policy` already *compiles* a policy to plane artifacts
  (`ci_workflow`, `codeowners`, `precommit_config`) as a library. The migration
  is the missing step: **write those artifacts into the repo** as reversible
  `PlannedChange`s (a `.pre-commit-config.yaml`, a CI workflow file, the runtime
  hook config) when a plane is absent.
- **Eligibility note:** runs only when an `aoa-policy.yaml` exists and a declared
  plane's artifact is missing or stale. Generated artifacts must not collide with
  an operator's hand-authored CI/pre-commit config — overwrite is opt-in, and the
  manifest archives the original (engine guarantee).
- **Audit signal:** `MissingPlane` ("a required enforcement plane is absent").
- **Construct-validity guardrail:** structural (templated from the declared
  policy) — **no prose leak channel**. Admitted under the standard `Label::Good`
  gate. Construct-valid only if installing a plane changes what the
  compiler/tests/hooks see, not the agent's instruction context.
- **Per-language:** **mostly language-agnostic** (YAML/CI/CODEOWNERS). The one
  language-sensitive seam is pre-commit *hook commands* (`cargo fmt` vs `ruff` vs
  `eslint`); those belong in the policy file's declaration, not hard-coded in the
  fix.

### R6 — generated-artifact marking (the MIGRATE counterpart)

- **Candidate fix id:** `generated-artifact-mark`
- **PRD:** R6 (`.gitattributes` `linguist-generated -diff` + an agent-readable
  provenance header: "generated from X, edit X instead").
- **What it does:** for each declared generated path, **write** the
  `.gitattributes` override and **inject** the provenance header into the derived
  file. This is the *generation* side of R6.
- **⚠ Distinct from `aoa-hal.4`.** hal.4 builds the **ENFORCE-plane** R6 gate:
  a read-only `write.blocked` span when an agent *attempts* to write a generated
  path. It ships in Wave 1N, **un-gated**, because it mutates nothing — it only
  blocks. *This* candidate is the **MIGRATE-plane** R6: it *mutates the tree*
  (writes `.gitattributes`, edits headers) and therefore **stays R0-gated**.
  Same requirement, two planes, two gating regimes — do not conflate them.
- **Eligibility note:** runs only against paths the operator declared
  `generated_paths` in `aoa-policy.yaml`. The header injection must be idempotent
  (re-running does not stack headers) and must not run on a path that already
  carries a conflicting header.
- **Audit signal:** **none exists today.** There is no `FindingKind` for
  "declared-generated-but-unmarked". This candidate is paired with a **proposed
  new audit dimension** that counts unmarked generated paths; the fix is not
  admissible until that dimension exists to measure it (you cannot run
  `--compare` on a signal you do not emit).
- **Construct-validity guardrail:** the provenance header is **prose an agent
  reads** → carries the guardrail-2 leak channel. The header text must be a fixed
  template authored blind to the held-out oracle. `.gitattributes` itself is
  structural.
- **Per-language:** the `.gitattributes` glob is agnostic; the **provenance
  header comment syntax is language-specific** (`//` vs `#` vs `<!-- -->`). This
  is a genuine per-language adapter seam — see the cross-language note below.

### R7 — reproduction-before-mutation gate scaffolding

- **Candidate fix id:** `reproduction-gate-scaffold`
- **PRD:** R7 (block a `write.attempt` not preceded by a `test.run`).
- **What it does:** the *enforcement primitive* already ships (`aoa-enforce`,
  `aoa-hal.1`) and the live install path exists (`observe --enforce`,
  `aoa-hal.2`). The migration gap is **scaffolding the gate into a repo**: writing
  the `reproduction_required` toggle into `aoa-policy.yaml` and emitting the
  runtime-hook config so a fresh repo opts in by migration rather than by hand.
- **Eligibility note:** runs only when the repo has an agent-runtime config
  surface to install into and the operator has not already declared the toggle.
  Pure policy-enforcement install (ZFC-allowed) — it does not infer *whether* the
  operator wants the gate, only mechanizes turning it on once declared.
- **Audit signal:** `MissingPlane` (the runtime plane absent).
- **Construct-validity guardrail:** structural (config emission) — **no prose
  leak channel**. Standard `Label::Good` gate. Note R7 enforcement itself needs
  no construct-validity gate (it enforces operator policy, not an inferred
  recommendation — see `aoa-enforce` lib docs); but *installing it as a tree
  migration* is still a tree mutation and rides the R0 gate like every candidate.
- **Per-language:** **language-agnostic** (hook/policy config).

### R11 — `aoa init` ejectable scaffold

- **Candidate fix id:** `init-scaffold`
- **PRD:** R11 (ejectable agent-ready scaffold + skill/context-file templates).
- **What it does:** on a cold-start / greenfield repo, create the agent-ready
  scaffold — package navigability anchors, a starter `aoa-policy.yaml`, skill and
  context-file templates — as reversible creates.
- **Eligibility note:** runs only on a repo lacking the scaffold (create-only,
  never clobbering existing files — the engine already refuses to overwrite a
  `Create` target). Greenfield repos report `InsufficientData` for most audit
  signals (see `aoa-d6t.23`); this fix is the cold-start bridge, so its admission
  evidence comes from post-scaffold runs, not the empty baseline.
- **Audit signal:** `NavigabilityAnchor` + `MissingPlane` (a bare repo trips
  both). Overlaps the anchor and R5 candidates — `init-scaffold` is their
  cold-start composition, not a replacement.
- **Construct-validity guardrail:** the **skill/context-file templates are prose
  an agent reads** → guardrail-2 applies; templates authored blind to the
  held-out oracle, fixed content. The biggest leak-risk candidate in this doc;
  admit last and with the most scrutiny.
- **Per-language:** scaffold structure is agnostic; **templates are
  language-flavored** (a Rust starter differs from a Python one). Per-language
  adapter seam, like R6's header.

### R16 — ownership → CODEOWNERS

- **Candidate fix id:** `ownership-codeowners`
- **PRD:** R16 (inferred ownership → `CODEOWNERS`; pairs with R17 dual-register
  output, tracked in `aoa-hal.6`).
- **What it does:** the mutation-gateway allowlist declared in `aoa-policy.yaml`
  already becomes the CODEOWNERS spine in `aoa-policy::codeowners`. The migration
  writes that `CODEOWNERS` file into the repo, narrowing who/what owns the
  state-changing surface.
- **Eligibility note:** runs only when `mutation_gateways` are declared and no
  conflicting `CODEOWNERS` exists (or overwrite is opt-in with archive). Ownership
  is *declared by the operator*, not inferred by AOA — the fix mechanizes the
  declared spine, it does not guess owners.
- **Audit signal:** `MutationSurface` (writable files reachable within the
  mutation-surface depth) — CODEOWNERS narrows that surface to declared gateways.
- **Construct-validity guardrail:** structural (generated from the declared
  allowlist) — **no prose leak channel**. Standard `Label::Good` gate.
- **Per-language:** **language-agnostic** (CODEOWNERS is a VCS-level file).

## Audit-signal coverage map

| `FindingKind` | Shipped fix | R0-gated candidate |
|---------------|-------------|--------------------|
| `NavigabilityAnchor` | `navigability-anchor` | `init-scaffold` (cold-start) |
| `UnusedImportProxy` | `dead-imports*` | — |
| `MissingPlane` | — | `policy-planes` (R5), `reproduction-gate-scaffold` (R7), `init-scaffold` (R11) |
| `MutationSurface` | — | `ownership-codeowners` (R16) |
| *(none — proposed)* | — | `generated-artifact-mark` (R6) **needs a new audit dimension first** |
| `ContextBudget` | — | **no candidate** — context-budget remediation is not yet a scoped migration |
| `ModuleSizeOutlier` | — | **no candidate** — module-splitting is a semantic edit, not a mechanical subtraction; out of scope until a construct-valid mechanical form exists |

Two gaps worth stating plainly: R6 cannot be admitted until its audit dimension
exists (no signal → no `--compare`), and two existing audit signals
(`ContextBudget`, `ModuleSizeOutlier`) have **no** migrate candidate because
neither yet reduces to a reversible, oracle-blind, mechanical change. Do not
invent one to "cover" the signal — an unmeasurable or semantic fix is worse than
none.

## Cross-language applicability

`NavigabilityAnchorFix` is language-agnostic because its output is a pure
function of the directory tree. `DeadImportFix` is **Rust-first with per-language
adapters** (Python, TypeScript). That split means: **the migrate pillar is
effectively single-language for any fix that must parse or emit
language-specific syntax.**

Among the candidates, the language-sensitive seams are:

- **R6 provenance header** — comment syntax per language (`//` / `#` / `<!-- -->`).
- **R11 templates** — language-flavored starter content.
- **R5 pre-commit commands** — language-specific tools (declared in policy, not
  hard-coded).

The rest (R5 plane structure, R7 config, R16 CODEOWNERS) are agnostic.

`aoa-mnz.6` (generic cross-language adapters) was **closed vacuous** — there is
no second-language fix today that justifies a shared adapter abstraction. Do not
build the abstraction speculatively (YAGNI; rule-of-three). **Revisit
per-language adapter design only when a second concrete language-specific fix is
justified** — at that point R6's header or R11's templates would be the trigger,
and the `DeadImportFix` `ImportAdapter` split is the proven prior art to port.

## Growing the surface — the operating procedure

When R0 returns `proceed`:

1. Pick **one** candidate (start with the lowest leak-risk structural ones:
   `ownership-codeowners`, `policy-planes`).
2. If it needs an audit dimension that does not exist (R6), build the **audit
   signal first** — you cannot `--compare` what you do not measure.
3. Implement it as one `impl CodeFix`, read-only `plan`, reversible via the
   existing engine; register it in `all_fixes()`.
4. Run `aoa eval --compare`; admit only on `Label::Good`. If `NotGood`, the fix
   stays unregistered — the enumeration entry remains, the code does not ship.
5. Repeat for the next candidate. Never batch.
