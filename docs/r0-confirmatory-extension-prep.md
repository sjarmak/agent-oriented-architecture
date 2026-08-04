# R0 confirmatory extension: offline preparation and dry-run cost

Prepared on 2026-08-04 for `aoa-h4q5`. This preparation chain performed only
offline planning: it ran no agent trial and inspected no trial outcome. Live
execution still requires explicit user authorization.

> **Held-out correction (2026-08-04):** `aoa-j7lf` determined that HTTPie is
> partially compromised and must be demoted from this confirmatory design. The
> operational extension is now 2 repositories, 28 dual tasks, 168 trials, and
> approximately `$43.29`; it is unrunnable under `aoa-6anq` until that
> preregistration is amended. See
> [R0 HTTPie held-out reserve verdict](r0-httpie-held-out-verdict.md).

## Original fixed design (superseded)

The original extension plan used all three preregistered reserve repositories
with two arms and `K=3`. Models, conventions, `min_holdout=7`,
`min_effect_size=0.0`, and the alternative repository/harness weights of
`0.75/1.25` remain unchanged.

Preparation used the existing scripts under
`/home/ds/projects/codeprobe/runs/r0-campaign/bin/`. Migration was completed
before mining so the migrated states remained blind to the held-out tasks.

| Repository | Pinned baseline SHA | Fix | Shipped dual tasks | Quarantined |
| --- | --- | --- | ---: | ---: |
| httpie | `5b604c37c6c67e18e7c3e9aee6c88a8c22b98345` | `dead-imports-python` | 14 | 6 |
| sqlparse | `f80af6a4007f11ada847218df8c29dc859238290` | `dead-imports-python` | 16 | 2 |
| websockets | `ff4869ba468129f3e85b08c2a8a03ec45cf26537` | `dead-imports-python` | 12 | 8 |

Every shipped task records `verification_mode=dual`, uses the comprehension
scorer path that emits `scorer_family=dual_composite`, has consensus from
`regex_import_graph` and `python_ast_graph`, and records static-analysis
enrichment. The shipped and quarantined counts above come from each
repository's `mine.json` consensus record.

For each repository, the following offline artifacts are present under
`/home/ds/projects/codeprobe/runs/r0-campaign/<repo>/`:

- `prep.json` and `mine.json`
- `index.scip` and `index.aoa.json`
- `index-migrated.scip` and `index-migrated.aoa.json`

The six `.codeprobe/experiment.json` manifests under `seed1..3/expA,expB` for
each repository are also intentional offline outputs of the existing
`build_experiments.sh` pipeline. No experiment represented by those manifests
was started by this preparation chain.

## Original three-repository dry-run size and cost (superseded)

The real mined corpus contains 42 tasks. The fixed design therefore requires:

```text
42 tasks x 2 arms x 3 seeds = 252 trials
```

Codeprobe's no-agent estimator was run once per repository over the freshly
mined corpus with six repeats, mechanically encoding `2 arms x K=3` without
building or changing campaign arm assignments:

| Repository | Tasks | Trials | Codeprobe estimated range |
| --- | ---: | ---: | ---: |
| httpie | 14 | 84 | `$1.68-$12.60` |
| sqlparse | 16 | 96 | `$1.92-$14.40` |
| websockets | 12 | 72 | `$1.44-$10.80` |
| **Total** | **42** | **252** | **`$5.04-$37.80`** |

The authorizable planning estimate requested by `aoa-6anq`, using its fixed
measured-rate assumption, is:

```text
252 trials x $0.2577/trial = $64.9404, rounded to $64.94
```

The inherited planning note displays `630 trials at $62.35`, which is
arithmetically inconsistent with its fixed rate. The rate corresponds to
`$162.35 / 630 = $0.2577/trial`; the missing leading `1` is treated as a
transcription error, not as authorization to retune the preregistered rate.

## Integrity boundary

The five completed repositories and their campaign evidence were not touched.
The protected file remains:

```text
/home/ds/projects/codeprobe/runs/r0-file-read-seed1/out/falsification.k3.json
SHA-256 8c5471a321c5e0cb9fbc570969eaaa1df60f5e1c0d44fe1cb17f66bc37992c1b
```

The shorter hash recorded in `aoa-6anq` omits the final hexadecimal digit;
the full digest above is the value verified on disk. This preparation did not
execute a seed, run a live agent, build the eight-repository arm assignments,
or change a convention.

## Pre-existing HTTPie run evidence

Review of the hidden `.codeprobe` directories found a partial live baseline run
under `httpie/seed1/expA` that the pre-filing survey had missed. Its files have
timestamps from 2026-07-04 22:02-22:29 EDT, one month before this preparation,
and include seven `agent_output.txt` files and seven `scoring.json` files. They
therefore do not contradict the narrower claim that this 2026-08-04 chain ran
no agents, but they do mean the original seed scaffolding was not wholly empty.

The pre-existing directory was preserved, not deleted, at:

```text
/home/ds/projects/codeprobe/runs/r0-campaign/quarantine-20260804-pre-confirmatory/httpie-seed1-expA-baseline
```

The quarantine's `PROVENANCE.md` records its source, timestamps, and inventory.
The experiment manifests remain in place. The earlier baseline run partially
compromises HTTPie and demotes it from the confirmatory reserve set. The
evidence, exact affected task subset, migration blindness analysis, and
corrected two-repository cost are recorded in the
[R0 HTTPie held-out reserve verdict](r0-httpie-held-out-verdict.md).
