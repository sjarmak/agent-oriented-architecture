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

## Pipeline

```
codeprobe experiment (≥2 arms, K seeds)          # you run this — needs a live agent
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
codeprobe experiment init   --name r0 --path runs/r0
codeprobe experiment add-config baseline      --path runs/r0   # baseline repo, baseline harness
codeprobe experiment add-config aoa_migrated  --path runs/r0   # AOA-migrated repo  → repo_arm
codeprobe experiment add-config harness_swap  --path runs/r0   # swapped harness    → harness_arm
codeprobe experiment run       --path runs/r0                  # live agent over the SAME mined tasks
codeprobe experiment aggregate --path runs/r0                  # writes reports/aggregate.json
```

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
