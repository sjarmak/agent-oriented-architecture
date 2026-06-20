# R0 Falsification Runbook — the Wave-1 gate

R0 asks one question before the AOA Toolkit earns a Wave 1: **is the
held-out improvement attributable to the AOA repo migration (the layer AOA
claims), or to the agent/harness?** If swapping the harness moves held-out
success as much as the migration does, AOA is targeting the wrong layer and the
project should `pivot` rather than `proceed`.

This is run as a **codeprobe experiment** with paired config arms, post-processed
by AOA into the falsification gate. The gate's verdict —
`proceed | pivot | inconclusive` — **is the documented Wave-1 gate.**

> Scope note: this runbook + the `aoa eval experiment` builder + `aoa falsify`
> are the machinery and the smoke. Running it at scale on **≥5 calibrated repos
> with live, non-deterministic agent runs** is tracked separately as bead
> `aoa-dhk.1`. Non-deterministic agent runs are *expected* to often land on
> `inconclusive` (the K≥3 determinism precondition) — that is a documented
> outcome, not a bug.

## The two deltas

| Delta | Codeprobe arm | What is varied | Fixed | Held-out source |
|-------|---------------|----------------|-------|-----------------|
| **repo-delta** | `repo_arm` | the repo (baseline → AOA-migrated) | harness | ARTIFACT leg |
| **harness-delta** | `harness_arm` | the agent/harness | repo (baseline) | ARTIFACT leg |

Both arms run the **same mined tasks** and both contribute their **held-out
(ARTIFACT) leg** — the contamination-free mined oracle. This is *not* the r0b
artifact-vs-direct mapping: r0b compares two legs of one run; R0 compares the
held-out leg of two different config arms. R0 `proceed` requires
`repo-delta ≥ harness-delta` on a strict majority of ≥5 eligible repos, hardened
by R0' (determinism, convention-invariance, eligibility, power).

## What counts as the AOA migration (the repo-delta treatment) — read before running

R0 is only interpretable if the repo-delta arm varies **the layer AOA actually
claims**, and varies *nothing the harness arm also varies*. AOA's claimed layer
is the **code's own infrastructure best-practices**: the structure, naming,
organization, and modularity of the codebase, and the navigable in-repo
documentation the agent builds on. The migration is the observability-guardrail
intervention — audit adherence to those best-practices, then apply (where
permitted) the changes that nudge the infrastructure back toward them.

Three guardrails make the treatment construct-valid; violating any one
**confounds the gate** and a `proceed` becomes meaningless:

1. **Code-layer, not prompt-layer.** The treatment must change what the
   compiler, tests, symbol graph, and file-navigation see — code structure,
   module boundaries, names, types, rustdoc/docstrings, dead-code removal. It
   **must not** be agent-instruction files (`CLAUDE.md`/`AGENTS.md`) or any
   prompt/system-context material: those are *harness* inputs, and putting them
   in the repo arm makes repo-delta and harness-delta measure the same thing.
   The repo dimension is realized by the **repo state the tasks run against**
   (baseline checkout vs migrated checkout) — **not** by any codeprobe
   `add-config` flag (those flags — `--model`, `--mcp-config`, `--allowed-tools`,
   `--instruction-variant`, `--preamble` — are all harness knobs and belong to
   the harness arm only).
2. **Authored blind to the held-out oracle.** Because a migration can touch
   prose (docstrings, READMEs), it can leak held-out task answers — an
   asymmetric contamination that inflates repo-delta only. The migrated state
   must be produced without sight of the mined held-out task set. Prefer
   tool-driven, mechanical migrations over free-hand edits for exactly this
   reason.
3. **Independent of the success criterion.** "Migrated" must be defined by a
   pre-registered best-practices spec, not *only* by whether AOA's own
   audit/lint passes — otherwise the gate confirms AOA's prior (Goodhart) rather
   than an independent fact. Use AOA's audit to *verify* the operator hit the
   spec, not to *define* it.

> Capability note: as of this writing AOA's `audit` surfaces only
> enforcement-plane gaps (CI / pre-commit / writable-mutation surface) and
> `lint-context` covers the context-file closure. Auditing and **executing**
> code-structure best-practices (naming / modularity / organization /
> doc-navigability) is the tool/skill/hook capability that produces this
> treatment reproducibly, and it is the precondition tracked in `aoa-dhk.1`'s
> blocker. Until it exists, the migrated arm is hand-authored per the guardrails
> above and the gate will mostly, honestly, return `inconclusive`.

## Pipeline

```
codeprobe experiment init/add-config + run (≥2 arms, K seeds)   # you run this — needs a live agent
        │  runs/<arm>/<task_id>/scoring.json (dual_composite)
        │  reports/aggregate.json (bias_warnings)
        ▼
aoa eval run   --codeprobe-run runs/<arm> ...    # per-arm process-metric records (optional, AC1)
        ▼
aoa eval experiment --manifest m.json --tasks T  # joins the arms → FalsifyInput (+ build report)
        ▼
aoa falsify --repos falsify_input.json \
            --build-meta falsify_input.build.json \
            --bias-warnings reports/aggregate.json
        ▼
falsification.json  { verdict, repo_delta, harness_delta, ... }   ← the Wave-1 gate
```

`scripts/r0_experiment.sh` chains the AOA post-processing steps once the
codeprobe experiment has been run.

## Step 1 — stand up the codeprobe experiment (≥2 arms, same tasks)

```bash
cd /home/ds/projects/codeprobe
codeprobe experiment init runs/r0 --name r0
# add-config sets HARNESS knobs only (agent/model/mcp/tools/instruction/preamble).
# The repo dimension is the repo STATE the tasks run against (see the treatment
# guardrails above), NOT an add-config flag.
codeprobe experiment add-config runs/r0 --label baseline      # baseline repo state, baseline harness
codeprobe experiment add-config runs/r0 --label aoa_migrated  # migrated repo state, baseline harness → repo_arm
codeprobe experiment add-config runs/r0 --label harness_swap  # baseline repo state, swapped harness   → harness_arm
codeprobe run --config runs/r0 --dry-run                      # estimate cost/turns FIRST (no agent spawned)
codeprobe run --config runs/r0 --max-cost-usd <budget>        # live agent over the SAME mined tasks; cost-capped
codeprobe experiment status    runs/r0                        # per-config completion
codeprobe experiment aggregate runs/r0                        # writes reports/aggregate.json
```

> There is no `codeprobe experiment run`; execution is the top-level
> `codeprobe run --config <experiment-dir>`. Always `--dry-run` first and pass
> `--max-cost-usd` — a ≥5-repo × K≥3 × 3-arm campaign is many live agent
> sessions.

For determinism (K≥3), run the experiment **K times** into K dirs (one per seed),
or seed the agent K ways — each becomes one `runs[]` entry per repo in the
manifest below.

## Step 2 — author the build manifest

Per repo, you declare the two eligibility facts AOA will **not** fabricate, and
point each fixed-seed run at its two arm dirs. Arm paths are resolved relative to
the manifest file.

```json
{
  "k_runs": 3,
  "min_holdout_size": 20,
  "min_effect_size": 0.0,
  "repos": [
    {
      "repo_id": "org/widget",
      "confidence": "high",     // operator assertion: SCIP-grade index. REQUIRED — no default.
      "calibrated": true,       // operator assertion: scoring calibrated. REQUIRED — no default.
      "runs": [
        { "seed": 1, "repo_arm": "seed1/aoa_migrated", "harness_arm": "seed1/harness_swap" },
        { "seed": 2, "repo_arm": "seed2/aoa_migrated", "harness_arm": "seed2/harness_swap" },
        { "seed": 3, "repo_arm": "seed3/aoa_migrated", "harness_arm": "seed3/harness_swap" }
      ]
    }
    // ... ≥5 repos for a usable (non-`too_few_repos`) verdict
  ]
}
```

`confidence` and `calibrated` are **required** and must reflect reality: verify
`confidence: high` against `aoa eval run`'s `graph_quality` (it is `scip` only
for a real SCIP index). `native_span` is **derived** from each task's mined
oracle (held-out provenance) — never declared. A repo that is not
high-confidence **and** native-composed **and** calibrated is reported as
*excluded* and casts no vote (R-silent).

## Step 3 — build + gate

```bash
scripts/r0_experiment.sh \
  --tasks    /home/ds/projects/codeprobe/runs/r0/tasks \
  --manifest manifest.json \
  --aggregate /home/ds/projects/codeprobe/runs/r0/reports/aggregate.json \
  --out      out/
# → out/falsify_input.json, out/falsify_input.build.json, out/falsification.json
```

## Reading the verdict

`falsification.json`:

- **`verdict`**: `proceed` (R0 not falsified — migration is the right layer),
  `pivot` (falsified — harness explains the gain), or `inconclusive` (the gate
  abstained).
- **`precondition_unmet`** (present only when the verdict is *not* a real gate
  decision):
  - `too_few_repos` — fewer than 5 repos submitted. The smoke hits this. **A
    real R0' abstention has NO `precondition_unmet` field** — that is how you
    tell "we could not run the gate" apart from "the gate ran and abstained."
  - `convention_inputs_degraded` — per-task `edit_locality`/`mutation_depth`
    were not derivable (no per-repo symbol graph), so the R0'
    convention-invariance check could not be exercised. The gate abstains rather
    than assert a verdict the hardening cannot back. (Wiring real convention
    inputs is the `aoa-dhk.1` follow-up.)
- **`repo_delta` / `harness_delta`**: mean held-out success on each arm over
  eligible repos. Emitted even when abstaining, for transparency.
- **`bias_warnings`**: codeprobe's measurement-bias warnings, surfaced
  **alongside** the AOA verdict and never altering it. `bias_gate_invalidating:
  true` means a `no_independent_baseline` warning fired — codeprobe's own
  ranking is uninterpretable, so weight the comparison accordingly.

Exit code is `0` for a genuine gate verdict (including a real R0' `inconclusive`)
and non-zero when a `precondition_unmet` blocked the gate.
