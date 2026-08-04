# R0 HTTPie held-out reserve verdict

Determined on 2026-08-04 for `aoa-j7lf`, from on-disk artifacts and Git
history only. This investigation did not execute an experiment, start a seed,
or open any `agent_output.txt` or `scoring.json` payload.

## Verdict

**HTTPie IS PARTIALLY COMPROMISED.** Seven of the 14 dual tasks mined on
2026-08-04 ask the same repository questions as the seven HTTPie baseline
trials attempted on 2026-07-04. The other seven current tasks have no July
scored record. HTTPie is nevertheless demoted from the confirmatory reserve
set: selecting only the surviving half after discovering the prior outcomes
would violate `aoa-6anq`'s fixed rule to take the pre-outcome reserve whole and
not select tasks or repositories after outcomes exist. It is not replaced.

## Provenance and chronology

The preserved run is at
`/home/ds/projects/codeprobe/runs/r0-campaign/quarantine-20260804-pre-confirmatory/httpie-seed1-expA-baseline`.
Its directory name and former path, recorded in the adjacent `PROVENANCE.md`,
identify repository `httpie`, seed 1, experiment A, configuration `baseline`.
The read-only checkpoint table also records `config_name=baseline` and
`repeat_index=0` for every row. The directory contains 24 resolved
instructions and seven pairs of `agent_output.txt`/`scoring.json`. Its
checkpoint timestamps and file mtimes run from 2026-07-04 22:02:35 through
22:29:20 EDT; six rows are completed and the seventh is a recorded max-turns
failure.

This was part of the original July 4 R0 campaign episode, not a separate
confirmatory run. It occupied the standard
`runs/r0-campaign/httpie/seed1/expA/.codeprobe/runs/baseline` path. The first
HTTPie run state was created at 22:02:10 EDT, less than one second after the
last `requests` campaign `scoring.json` mtime at 22:02:09 EDT. The earlier
`runs/r0-campaign/quarantine-20260728-pre-k3` also contains seed/experiment run
directories with July 4 mtimes, including `seed1-expB-runs` and
`seed3-expB-runs` at 21:23. The HTTPie directory was residue from that same
episode that the July 28 quarantine missed; commit
`8cfdaa179527d8e25acd8e74e07db744427d7251` preserved it in the dated HTTPie
quarantine on August 4.

## Task overlap

The current corpus is fixed to baseline commit
`5b604c37c6c67e18e7c3e9aee6c88a8c22b98345` and recorded in
`/home/ds/projects/codeprobe/runs/r0-campaign/httpie/mine.json`, whose mtime is
2026-08-04 07:26:49 EDT. The following seven current tasks reproduce the July
question and target. The five dependency-analysis identifiers changed when
the miner added the file-read evidence requirement; their question bodies are
otherwise the same. The two import-chain identifiers are unchanged.

| July 4 task | August 4 dual task | Subject |
| --- | --- | --- |
| `comprehension-dependency_analysis-010-a2f47e78` | `comprehension-dependency_analysis-005-dfd06244` | `httpie.__main__.main` |
| `comprehension-dependency_analysis-011-e3b47d5a` | `comprehension-dependency_analysis-006-656a286e` | `httpie.cli.nested_json.parse.parse` |
| `comprehension-dependency_analysis-012-5bb7e8a0` | `comprehension-dependency_analysis-007-4773b9a9` | `httpie.compat.func` |
| `comprehension-dependency_analysis-013-eee4c76e` | `comprehension-dependency_analysis-008-40c6f3a6` | `httpie.core.program` |
| `comprehension-dependency_analysis-014-bb7b0060` | `comprehension-dependency_analysis-009-540190d3` | `httpie.manager.core.program` |
| `comprehension-import_chain-000-4c3807ce` | `comprehension-import_chain-000-4c3807ce` | `httpie.cookies` |
| `comprehension-import_chain-002-474b5bdb` | `comprehension-import_chain-002-474b5bdb` | `httpie.compat` |

The current instruction sources are under
`/home/ds/projects/r0-repos/httpie/.codeprobe/tasks/<August-task-id>/instruction.md`;
the July sources are the corresponding
`<July-task-id>/instruction.resolved.md` files in the quarantine. The seven
current tasks without a July scored record are import-chain questions for
`httpie.utils` and `httpie.encoding`, plus five transitive-dependency
questions. Thus the compromised subset is exactly 7/14 current tasks, and the
unexposed subset is exactly 7/14.

## Outcome use and migration blindness

The July outcome files chronologically predate the migrated clone, so a
complete claim that no operator had ever seen them cannot be proved from the
available artifacts. The one missing artifact that would settle that human
access question is a complete, timestamped file-access or operator-session
audit log covering the HTTPie run from 2026-07-04 22:02 EDT through the
2026-08-04 migration. No such log is present.

The durable artifact shows no answer-derived change, but it cannot establish
the operator's blindness. The current migrated clone was freshly cloned and
changed at 2026-08-04 07:26:38 EDT. Its reflog and `prep.json` identify commit
`7530c5c902b22d52a136f0b0f37e1fcf6ce6c830`; that commit deletes only an
unused `pathlib.Path` import from
`extras/packaging/linux/scripts/hooks/hook-pip.py`. The contemporaneous
`migrate_plan.json` and `migrate_apply_dead-imports-python.json` record the
sole `dead-imports-python` fix, mechanically selected by isolated Ruff 0.15.8.
Mining followed eleven seconds later. None of the seven overlapping task
subjects targets the changed file or module. That proves the migration was a
mechanical source-only transform completed before the current corpus was mined
and that its diff contains no answer-derived change. It does not prove the
authorship condition at `docs/r0_runbook.md:60`, which requires production
without sight of the held-out oracle. Without the missing access log, that
blindness guardrail is unverified for HTTPie and cannot be claimed to survive.

Tracked AOA history contains no HTTPie design/report record before commit
`69681f8ce07740324f058445040e76b1041ca3d2` at 2026-08-04 07:31 EDT, which
recorded the offline preparation and treated HTTPie as unused. Commit
`8cfdaa179527d8e25acd8e74e07db744427d7251` at 10:04 EDT is the first tracked
record to acknowledge the July run. The later review-triage note on `aoa-h4q5`
records that one scoring payload was inspected after the migration and report
were already committed. There is therefore evidence of later outcome access,
but no durable evidence that an outcome informed the migrated clone or the
already-fixed design. That narrow migration finding does not restore HTTPie's
held-out status: seven confirmatory task outcomes already existed.

## Consequence for authorization

> HTTPie is partially compromised (7 of its 14 current dual tasks repeat July
> 4 baseline trials) and is demoted without replacement. The confirmatory
> extension therefore drops from 3 repos / 42 dual tasks / 252 trials / about
> $64.94 to 2 repos / 28 dual tasks / 168 trials / about $43.29. The remaining
> codeprobe dry-run range is $3.36-$25.20. This reduced design does not satisfy
> `aoa-6anq` as preregistered: `min_holdout=7` per retained repo, `K=3`, weights
> 0.75/1.25, and the fixed decision mapping remain unchanged, but the specified
> three-repository/eight-repository-manifest selection rule cannot be executed.
> No live run is authorized until the preregistration is explicitly amended.

The corrected fixed-rate calculation is `168 x $0.2577 = $43.2936`, rounded to
`$43.29`. The original 252-trial calculation remains useful only as the
historical cost of the now-rejected three-repository plan.
