# R0 confirmatory extension: authorized two-repository amendment

Originally prepared on 2026-08-04 for `aoa-h4q5`. The preparation chain
performed only offline planning: it ran no agent trial and inspected no trial
outcome. Stephanie authorized the amendment below on 2026-08-04 in response to
escalation `gc-720157` (relayed as `gc-722425`).

## Authorized amendment (2026-08-04)

HTTPie is excluded from the confirmatory reserve because the held-out
disqualification verdict landed in commit `21d3373`. Commit `e5948d1`
subsequently qualified this preparation document's migration-blindness summary;
it did not land or alter the verdict. This amendment does not reinterpret or
qualify that verdict; the complete evidence remains in the
[R0 HTTPie held-out reserve verdict](r0-httpie-held-out-verdict.md).

The confirmatory extension is amended to the two untouched reserve repositories:

| Repository | Dual tasks | Seeds | Arms | Trials |
| --- | ---: | ---: | ---: | ---: |
| sqlparse | 16 | 3 | 2 | 96 |
| websockets | 12 | 3 | 2 | 72 |
| **Total** | **28** | **3** | **2** | **168** |

The five preserved campaign repositories vote `2 proceed / 3 pivot` under the
pre-registered alternative weights. Adding sqlparse and websockets produces a
seven-repository manifest. A strict majority of seven is four, so **both reserve
repositories must cast proceed votes** to move the campaign from two to four
proceed votes. A per-repository delta tie counts as a proceed vote because the
vote predicate is `repo_delta >= harness_delta`. More generally, failure to meet
the robust-proceed requirements maps away from `proceed`: a completed base tally
with a minority or exact tie of proceed votes maps to `pivot`, while unmet
evidence or hardening preconditions map to `inconclusive`. The unanimous-reserve
bar is unchanged.

Both sqlparse and websockets are recorded as `native_composed` held-out
provenance in `runs/r0-campaign/out/falsify_input.k1.build.json`. They are
eligible to vote under the `(external | native_composed)` predicate landed in
commit `4f9107c`; the seven-repository threshold above is computed under that
rule.

Execution retains the fixed protocol: repo arm `claude-sonnet-4-6` on migrated
state, harness arm `claude-haiku-4-5` on baseline, Bash disallowed uniformly,
`K=3`, `min_holdout=7`, `min_effect_size=0.0`, weights `0.75/1.25`, and the
fixed `proceed | pivot | inconclusive` decision mapping. Seed 1 is the admission
gate for both repositories and must achieve pair yield `>= 0.80` in each before
seeds 2 and 3 run. The authorized planning ceiling is:

```text
168 trials x $0.2577/trial = $43.2936, rounded to $43.29
```

The changed power calculation relative to the original preregistration is part
of the authorization. The sections below preserve the superseded preparation
record for provenance; they are not executable campaign instructions.

## Original fixed design (superseded)

The original extension plan used all three preregistered reserve repositories
with two arms and `K=3`. Models, conventions, `min_holdout=7`,
`min_effect_size=0.0`, and the alternative repository/harness weights of
`0.75/1.25` remain unchanged.

Preparation used the existing scripts under
`/home/ds/projects/codeprobe/runs/r0-campaign/bin/`. Migration was completed
before the current corpus was mined and used mechanical source-only
transforms. For sqlparse and websockets, which have no pre-existing trial
outcomes, that ordering preserved operator blindness to the held-out tasks.
For HTTPie, the migrated diff contains no answer-derived change, but operator
blindness is unverified because seven equivalent July task outcomes predated
the migration and no timestamped access or operator-session audit log exists.
See the
[HTTPie migration-blindness finding](r0-httpie-held-out-verdict.md#outcome-use-and-migration-blindness).

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
