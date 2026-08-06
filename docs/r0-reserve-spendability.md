# R0 reserve spendability after the zero-valid-results ruling

Determined on 2026-08-06 for `aoa-p0g5l`, from on-disk run artifacts, the AOA
exposure gate, and closed decision records. This investigation executed no
experiment, started no seed, spent no repository, and mutated no run directory.
Unlike `aoa-j7lf`, it does open `scoring.json` payloads, which is what makes its
numeric claims checkable.

## The answer, first

**The spendable R0 reserve pool is empty.** No repository is available for the
sqlparse + websockets confirmatory extension to draw on as a genuinely held-out
subject. `dec-su1` returns nothing to the pool, because its precondition is not
satisfied by any run in the campaign's history.

`aoa-f1q3` cannot select a repository under its current framing. It needs a
human decision first; the options are enumerated under
[What aoa-f1q3 may actually do](#what-aoa-f1q3-may-actually-do).

## Governing records

| Record | Status | What it fixes |
| --- | --- | --- |
| [ADR 0003](adr/0003-held-out-provenance.md) | Accepted | Held-out provenance is load-bearing; the pre-outcome reserve is taken whole; demotion, not relaxation, when held-out status cannot be proven |
| `dec-su1` | Ruled (b), 2026-08-06 | A **zero-valid-results** run does not consume a reserved repo; it returns to the pool. Standing accounting rule |
| `dec-oizk` | Ruled (a), 2026-08-06 | Discard the broken run's results and rerun clean, **despite 79/80 grading correct** |
| [`aoa-j7lf`](r0-httpie-held-out-verdict.md) | Closed | HTTPie is partially compromised (7/14) and demoted without replacement |

ADR 0003 is treated as governing throughout. Nothing below overrides it, and
nothing below needs to: the record and the ADR agree.

## The load-bearing correction

The premise that motivated this question is that the 2026-08-04 confirmatory run
on sqlparse and websockets produced nothing, so `dec-su1` returns both
repositories to the pool. **That premise is false, and it was already corrected
in front of Stephanie before she ruled.** The `dec-oizk` entry originally told
her the run "produced nothing" and that "there is no result in there to leak";
it was retitled and re-published against the real fact after a read-only
verifier enumerated all 80 payloads. Her ruling answers the corrected question.

All 80 `scoring.json` payloads under `{sqlparse,websockets}` in the
`r0-campaign` run directory were enumerated directly for this record:

| | sqlparse | websockets | combined |
| --- | ---: | ---: | ---: |
| Trials | 56 | 24 | 80 |
| `score` / `score_direct` / `reward` | 0.0 on all | 0.0 on all | 0.0 on all |
| `score_artifact` non-zero | 56 | 23 | 79 |
| `passed_artifact = true` | 56 | 23 | 79 |
| Mean non-zero `score_artifact` | 0.9106 | 0.9876 | 0.9330 |
| `status` | `completed` on all | `completed` on all | `completed` on all |

Every payload records `scorer_family = dual_composite` and
`scoring_policy = min`. The top-level zero is the `min` policy collapsing a
**passing** artifact leg against the direct leg broken by `aoa-oer4`. It is an
artifact of the scoring policy, not an absence of grading.

AOA's own instrument reaches the same conclusion without being asked to. The
exposure gate tallies the held-out outcome from the independent artifact leg,
"never its top-level composite, which a failed direct leg can lower"
(`crates/aoa-bench/src/exposure.rs`, landed under `aoa-d2awk`). Running it:

```text
sqlparse   @ f80af6a4...: exposed (16/16)
  trials: 56; held-out passed=56 failed=0; errored=0; unscored=0
websockets @ ff4869ba...: exposed (12/12)
  trials: 24; held-out passed=23 failed=1; errored=0; unscored=0
```

`errored=0; unscored=0` on both. Every one of the 80 trials produced a
determinate held-out outcome. Measured by the gate that decides admission, this
is the cleanest-graded run in the entire campaign: every one of the five
completed campaign repositories carries errored trials (22, 6, 8, 53, and 21
respectively), and these two carry none.

Recorded spend on the run, re-derived here by summing
`diagnostics.token_cost_usd` across all 80 payloads: `$10.348696`. The
frequently quoted `$8.32` sums only the 72 rows in the five `results.json`
files and drops `sqlparse/seed2/expB`, an 8-trial arm worth `$2.030524` that
halted before writing its results file.

## Every repository ever reserved or drawn for R0

Verdicts below are stated in two independent columns, because the two questions
are not the same one. *Consumed* answers `dec-su1`'s reserve-accounting
question. *Held out* answers ADR 0003's provenance question. A repository is
spendable only if it is intact on **both**.

| Repository | Baseline commit | Run that touched it | Valid results obtained? | `dec-su1`: consumed? | ADR 0003: still held out? | Spendable |
| --- | --- | --- | --- | --- | --- | --- |
| requests | `23953c0c` | R0 campaign, 2026-07-04 21:24 to 07-28 23:25 EDT; 126 trials | Yes (51 passed, 22 failed, 53 errored) | Consumed | No, exposed 14/14 | **No** |
| isort | `fd8bd075` | R0 campaign, 2026-07-28 08:08 to 23:25 EDT; 155 trials | Yes (112 passed, 37 failed, 6 errored) | Consumed | No, exposed 14/14 | **No** |
| marshmallow | `cd3cda8b` | R0 campaign, 2026-07-28 17:13 to 23:25 EDT; 63 trials | Yes (47 passed, 8 failed, 8 errored) | Consumed | No, exposed 7/7 | **No** |
| gunicorn | `a8283bbf` | R0 campaign, 2026-07-28 18:52 to 23:25 EDT; 144 trials | Yes (103 passed, 19 failed, 22 errored) | Consumed | No, exposed 16/16 | **No** |
| rich | `9d8f9a37` | R0 campaign, 2026-07-28 18:52 to 23:25 EDT; 171 trials | Yes (125 passed, 25 failed, 21 errored) | Consumed | No, exposed 19/19 | **No** |
| httpie | `5b604c37` | July 4 baseline, 2026-07-04 22:02 to 22:29 EDT; 7 trials, quarantined | Yes (4 passed, 3 failed) | Consumed | No, partially exposed 7/14; **demoted whole** per `aoa-j7lf` | **No** |
| sqlparse | `f80af6a4` | Halted confirmatory run, 2026-08-04 15:37 to 16:17 EDT; 56 trials | **Yes**, 56/56 held-out passed, 0 errored, 0 unscored | Consumed | No, exposed 16/16 | **No** |
| websockets | `ff4869ba` | Halted confirmatory run, 2026-08-04 15:55 to 16:00 EDT; 24 trials | **Yes**, 23/24 held-out passed, 0 errored, 0 unscored | Consumed | No, exposed 12/12 | **No** |

The five campaign repositories are listed because they are what the reserve was
held back from. They were spent as designed and are not candidates.

## Applying dec-su1 mechanically

`dec-su1` reads: a **zero-valid-results** run does not consume a reserved repo;
consumption is tied to obtaining valid results, not to having attempted a run.

Applied literally, it changes nothing about the current pool:

- For the five campaign repositories and for httpie, valid results were
  obtained. `dec-su1`'s exemption does not reach them. They stay consumed.
- For sqlparse and websockets, valid results were **also** obtained: 79 of 80
  trials produced a passing held-out outcome, with zero errored and zero
  unscored trials. `dec-su1`'s precondition is not met, so its exemption does
  not reach them either. They stay consumed.

There is no run in the campaign's history to which `dec-su1` applies. The rule
is sound and now stands as standing accounting policy; it simply has no subject
here.

### The reading this record rejects

There is a second reading available: `dec-oizk` orders the broken run's results
discarded, so once discarded there are no results, so the run becomes a
zero-results run retroactively and `dec-su1` returns both repositories to the
pool.

That reading is rejected, on three independent grounds.

1. **The ruling forecloses it.** `dec-oizk` was ruled *(a)* "discard the broken
   run's results and rerun clean, **despite 79/80 grading correct**." The word
   *despite* concedes that the results exist. A rule keyed on a run having
   produced no valid results cannot be satisfied by a run the ruling itself
   describes as 79/80 correct.

2. **Discarding is not un-running.** Discarding a result is a decision about
   which evidence to *use*. It does not un-run the agent, un-write the 80
   `agent_output.txt` files, or un-see the tasks. Held-out status is a claim
   about history, and history did not change when the ruling landed.

3. **ADR 0003 names this exact move.** "History is exactly the thing that is
   convenient to reinterpret once results are in. The pressure is always in one
   direction, toward keeping the tasks that still look clean." Reading a discard
   order backwards into the factual record, to recover two repositories the
   campaign needs, is that pressure operating. The ADR's response is demotion,
   not reinterpretation.

## HTTPie

Cited, not re-derived. `aoa-j7lf` is closed and its verdict stands:

> **HTTPie IS PARTIALLY COMPROMISED.** Seven of the 14 dual tasks mined on
> 2026-08-04 ask the same repository questions as the seven HTTPie baseline
> trials attempted on 2026-07-04. [...] HTTPie is nevertheless demoted from the
> confirmatory reserve set: selecting only the surviving half after discovering
> the prior outcomes would violate `aoa-6anq`'s fixed rule to take the
> pre-outcome reserve whole and not select tasks or repositories after outcomes
> exist. It is not replaced.

The full evidence, task overlap table, and migration-blindness analysis are in
[the verdict](r0-httpie-held-out-verdict.md). Nothing in this record reopens,
qualifies, or narrows it. The exposure gate independently reports httpie as
partially exposed at 7/14 from the quarantined July 4 residue, which agrees with
the verdict.

`dec-su1` does not revive httpie. Its July 4 run obtained valid results (4
passed, 3 failed), so the exemption does not apply; and even if it did, the
whole-reserve rule demotes the repository rather than the touched subset.

## Consistency with ADR 0003

This record does not contradict ADR 0003, and no escalation on that ground is
required.

The two would collide only if `dec-su1` were read as restoring held-out status.
It is not, and it does not have to be: exposure classification in
`crates/aoa-bench/src/exposure.rs` is score-independent. `classify` marks a
subject exposed when a trial artifact exists for it under that repository's
admitted corpus, whatever the trial scored. So the 16/16 and 12/12 verdicts hold
irrespective of how `dec-su1` is read, and the accounting question and the
provenance question can be answered separately without either overriding the
other.

That separation is the reason the enumeration above carries two verdict columns.
A future ruling that changes reserve accounting still cannot move the held-out
column; only history can.

## What aoa-f1q3 may actually do

**Repositories the confirmatory extension may draw from as held-out subjects:
none.** All eight are exposed or demoted, and no ninth repository has been
reserved.

`dec-oizk` directs a clean rerun drawing on the reserve pool. That instruction
cannot be executed as written, because the pool it draws on is empty. This is
the blocking finding `aoa-f1q3` was waiting on, and it is a human decision, not
an engineering one. Three paths are available, and they are not equivalent:

1. **Reserve and mine a new repository.** Restores a genuine held-out subject
   and satisfies both ADR 0003 and the preregistration. Requires a repository
   never drawn for R0, mined at a pre-outcome commit, with the migration
   completed before mining as the existing protocol requires. Costs the mining
   and preparation work, and changes the manifest arithmetic in
   [the extension prep](r0-confirmatory-extension-prep.md), whose
   seven-repository threshold assumed sqlparse and websockets.

2. **Rerun sqlparse and websockets as acknowledged-exposed.** Cheap and
   immediate, and the corrected `$10.348696` spend leaves `$32.94` under the
   `$43.29` ceiling. But it does not measure what R0 is for: the
   visible/held-out gap is only a signal while the agent has not seen the task,
   and here it has seen every one. Whatever such a rerun produces, it cannot be
   reported as a held-out result, and ADR 0003 forbids relaxing the check to let
   it be.

3. **Retire the confirmatory extension.** Report the campaign's five-repository
   result on its own terms, with the reserve exhausted and the reason recorded.
   This is the option that costs experimental power and keeps the standard,
   which is the trade ADR 0003 says has already been made once, for HTTPie, and
   was made in that direction.

Path 2 is the one to be most careful about, because it is the cheapest and it
looks like progress. It is the shape of change ADR 0003 calls a defect "even
when the resulting numbers improve, and especially then."

## Reproducing this record

`CODEPROBE_ROOT` defaults to `~/projects/codeprobe`; export a different value
first if your codeprobe clone is elsewhere.

```bash
export CODEPROBE_ROOT="${CODEPROBE_ROOT:-$HOME/projects/codeprobe}"
cargo build --bin aoa
./target/debug/aoa eval exposure scan \
  --runs "$CODEPROBE_ROOT/runs/r0-campaign"
```

The `aoa-bench` test `real_r0_campaign_matches_documented_exposure_and_held_out_provenance`
asserts these same tallies against the campaign. It reads the run directory from
`AOA_R0_CAMPAIGN_RUNS`, and it is `#[ignore]`d so that a run without that campaign
reports `1 ignored` rather than a green `ok` that scanned nothing. To run it:

```bash
: "${CODEPROBE_ROOT:?export CODEPROBE_ROOT first (see above)}"
AOA_R0_CAMPAIGN_RUNS="$CODEPROBE_ROOT/runs/r0-campaign" \
  cargo test -p aoa-bench --test exposure_scan -- --ignored
```

The per-repository trial counts, held-out pass/fail/errored/unscored tallies,
causing run paths, and mtime ranges in this record are that command's output.
The `score_artifact` distribution and the `$10.348696` spend come from reading
the 80 `scoring.json` payloads under `{sqlparse,websockets}` in that same
`r0-campaign` run directory.
