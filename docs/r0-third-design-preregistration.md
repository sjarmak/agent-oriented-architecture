# R0 confirmatory extension: third design preregistration

Written 2026-08-07 for `aoa-6da35`, after Stephanie ruled option 1 — reserve and
mine a repository never drawn for R0 — on 2026-08-06. Option 1 is enumerated in
[the reserve spendability record](r0-reserve-spendability.md#what-aoa-f1q3-may-actually-do),
which established that the spendable reserve pool is empty and that all eight
previously reserved or drawn repositories are exposed or demoted.

Every date and time in this document is `-04:00`, the offset `git log` reports
for the commits that carry it. A document dated ahead of its own commit is the
first thing a skeptic's `git` check turns up, so the two are kept in one clock.

This is the third design. The first used three reserve repositories; the second
was [the authorized two-repository amendment](r0-confirmatory-extension-prep.md)
over sqlparse and websockets. Both are superseded as executable plans and both
are preserved for provenance. [ADR 0003](adr/0003-held-out-provenance.md)
governs unchanged: a repository is genuinely held out or it is not a subject.

This is revision 2. Revision 1 was rejected in review for two defects in the
decision rule itself — a branch-B pass mark one vote too high, and a
falsification mapping that did not match what `decide` returns. Both are
corrected below, and the correction is the reason the preregistration is worth
having: a pass mark discovered to be wrong *after* mining could not be repaired
without being the post-hoc threshold adjustment this document exists to prevent.

**This commit contains selection criteria and the threshold rule only.** No
candidate repository is named, inspected, or alluded to anywhere below. The
application of these criteria to candidates lands in the next commit, and
`git log` is the evidence that the criteria predate the choice.

## Why the ordering is the artifact

Adjusting a threshold after seeing results is the hazard R0 exists to detect,
and doing it invalidates a confirmatory extension more thoroughly than having no
subject at all. Selection carries the same property: preferring a candidate
after inspecting its contents is the contamination the held-out design excludes.

Both are ordering claims, and an ordering claim asserted in prose asks to be
taken on trust. Splitting the artifact across two commits makes it checkable
instead. A skeptic reads `git log` and sees that the commit fixing the rule
carries no candidate, and that the commit naming candidates could not have moved
the rule. That is why this is two commits rather than one document with two
sections, and it is why they must not be squashed.

## Selection criteria

Written before any candidate has been looked at, and applied in the order given.

**C1 — never drawn for R0 in any prior run.** The repository must have no
persisted trial artifact under any R0 campaign root, including quarantine
subtrees. This is first because it is the only criterion whose failure cannot be
repaired: a subject that has been seen cannot be un-seen. Held-out status is a
claim about history, and history does not change when it becomes inconvenient.

**C2 — a pre-outcome baseline commit exists and is pinned to a full SHA.** The
repository is pinned to an exact revision before mining, recorded as a full
40-character SHA. Abbreviated SHAs and branch names are both rejected: the
[codeprobe execution-basis record](r0-codeprobe-execution-provenance.md) already
requires a full-SHA precondition for any rerun, and a moving branch makes the
mined corpus unreproducible.

**C3 — the migration completes before mining.** The AOA migration is applied to
the pinned baseline and completes cleanly *before* the task corpus is mined, as
the established protocol requires. Mining first and migrating second lets the
migration be shaped by knowledge of the tasks, which is the operator-blindness
failure the HTTPie verdict had to reason about at length and could not fully
exclude.

**C4 — the mined corpus clears the standing admission floor unchanged.** At
least `min_holdout=7` admitted dual tasks, and seed 1 reaching pair yield
`>= 0.80`. Both numbers are inherited, not chosen here. A repository that cannot
clear them is not admitted, and the floor is never lowered to admit it: the
runbook's standing instruction is to fix the evidence pipeline and rerun seed 1
rather than weaken admission.

**C5 — the fixed protocol applies unchanged.** Repo arm `claude-sonnet-4-6` on
migrated state, harness arm `claude-haiku-4-5` on baseline, Bash disallowed
uniformly, `K=3`, `min_effect_size=0.0`, all four preregistered conventions
including the alternative weights `0.75/1.25`, and the fixed
`proceed | pivot | inconclusive` decision mapping. A repository requiring any
change to this list is not a candidate, because an extension that alters the
protocol is not confirming the same hypothesis.

**C6 — every criterion is evaluated in writing, per candidate.** The application
commit records how each criterion was met or failed for each candidate examined,
including the candidates that were rejected. Recording only the winner would
leave the selection unfalsifiable.

C1 through C3 and C5 are checkable before any agent runs. C4 is the one
criterion whose satisfaction is not knowable until mining and seed 1 have
happened, which is why it is stated as an admission gate against a fixed floor
rather than as a selection preference. It can admit or exclude a candidate; it
can never be used to prefer one after the fact.

## The threshold, as a rule over repository count

### The predicate, read from the implementation

R0's verdict is not a mean over repositories. It is a per-repository vote
decided by strict majority, tallied in `crates/aoa-falsify/src/verdict.rs`:

```rust
let against = repos.len() - votes_for;
let verdict = if votes_for > against {
    Verdict::Proceed
} else {
    // A strict-majority loss and an exact tie both default to pivot.
    Verdict::Pivot
};
```

Two further constants in that file bind the rule. `MIN_ELIGIBLE_REPOS = 5`: a
manifest with fewer than five eligible repositories is `Inconclusive`, because a
majority is not meaningful below that. And a repository admitting zero pairs
under a convention casts no vote and makes the whole tally `Inconclusive` rather
than counting against.

So for a manifest of `M` eligible repositories, `proceed` requires

```text
votes_for > M - votes_for   <=>   votes_for >= floor(M / 2) + 1
```

`M` counts **eligible** repositories only. `partition` (`verdict.rs:253`) splits
the manifest on `is_eligible` and `decide` receives the eligible slice alone, so
an ineligible repository is not a vote against — it is not in the denominator at
all. That distinction does no work while every repository is eligible, and it
does all the work in
[the precondition section](#the-precondition-the-count-depends-on) below.

### Instantiating it for an extension, where the preserved five still vote

**This subsection assumes the five preserved campaign repositories are eligible
and vote.** That assumption is not free, and the section after it is where it
stops holding. Every number in this subsection is conditional on it.

The five preserved campaign repositories vote `2 proceed / 3 pivot` under the
alternative weights `0.75/1.25`, evaluated at run index 0. On this reading that
is the binding convention: under canonical equal weights the same five vote
`5-0` proceed, so canonical is already satisfied and is not what decides the
extension. R0' requires the verdict to hold across every preregistered
convention, so the threshold is derived on the convention that fails rather than
the one that passes.

Adding `N` new repositories gives `M = 5 + N`, and the preserved arm contributes
2 proceed votes that the extension does not re-litigate. The new repositories
must therefore supply

```text
n_new  >=  floor((5 + N) / 2) + 1  -  2  =  floor((5 + N) / 2) - 1
```

proceed votes out of `N`:

| New repos `N` | Manifest `M` | Strict majority | New proceed votes required | Out of | Feasible |
| ---: | ---: | ---: | ---: | ---: | :--- |
| 1 | 6 | 4 | 2 | 1 | **No** |
| 2 | 7 | 4 | 2 | 2 | Yes, unanimous |
| 3 | 8 | 5 | 3 | 3 | Yes, unanimous |
| 4 | 9 | 5 | 3 | 4 | Yes |
| 5 | 10 | 6 | 4 | 5 | Yes |

### Why this is the same principle, not a new one

The rule reproduces both previously authorized instantiations exactly, and that
is the check that it is inherited rather than invented for this design:

- The three-repository design gave an eight-repository manifest, and `aoa-6anq`
  recorded "with eight repos, strict majority requires 5 proceed votes", all
  three reserve repositories voting proceed. Row `N = 3` gives 5, and 3 of 3.
- The two-repository amendment gave a seven-repository manifest, and the
  extension prep recorded "a strict majority of seven is four, so both reserve
  repositories must cast proceed votes". Row `N = 2` gives 4, and 2 of 2.

Nothing above chooses a pass mark. The pass mark is a function of manifest size
under a rule fixed in `verdict.rs` before any of these designs existed. The only
free parameter is `N`, and `N` is a spend decision rather than a statistical
one.

### The feasibility boundary

**A single new repository cannot produce a proceed verdict, on either reading of
the exposure ledger.** Where the preserved five vote, `M = 6`, the strict
majority is 4, the preserved arm supplies 2, and one new repository supplies at
most 1: the maximum attainable is 3, and a tie is not available either, since
ties default to pivot. Where they do not vote, `M = 1`, which is below
`MIN_ELIGIBLE_REPOS = 5` and is `Inconclusive` before any outcome is read. The
verdict is decided against proceeding before the extension runs, whatever the
new repository does.

This is a property of the vote arithmetic and not of any candidate, which is why
it belongs in this commit rather than the next one. **The minimum feasible
design is `N = 2` where the preserved five vote, and `N = 5` where they do
not**, and each requires the proceed count the branch table below states.

## The precondition the count depends on

The table above assumes the five preserved repositories still vote. That
assumption does not currently hold, and it fails for two independent reasons.
Both were measured on 2026-08-07 against the preserved K=3 evidence. Neither is
inferred.

**1. The preserved evidence cannot be read by the current binary.**

```text
$ aoa falsify --repos runs/r0-file-read-seed1/out/falsify_input.k3.json \
              --build-meta runs/r0-file-read-seed1/out/falsify_input.k3.build.json
error: failed to parse falsify input .../falsify_input.k3.json:
       missing field `exposure` at line 9 column 7
```

The preserved file predates the exposure gate. Each of its five `eligibility`
records carries `confidence`, `native_span`, and `calibrated` and no `exposure`
field, while `Eligibility` in `crates/aoa-falsify/src/types.rs` declares
`exposure: ExposureStatus` under `deny_unknown_fields` with no default. So
`aoa-6anq`'s instruction to "preserve the original five-repository K=3 evidence
byte-for-byte and add the reserve repositories to one manifest" is not
executable as written. The evidence has to be re-emitted with an exposure value
for each repository, and *what that value is* decides whether the five vote at
all.

**2. Re-deriving exposure today makes all five ineligible.**

```text
$ aoa eval exposure scan --runs runs/r0-file-read-seed1
  gunicorn @ a8283bbf...: exposed (16/16)
  isort    @ fd8bd075...: exposed (14/14)
  ...
```

`is_eligible` in `crates/aoa-falsify/src/eligibility.rs` requires
`exposure.is_unexposed()`. Every preserved repository is `exposed` under a scan
of its own K=3 run root, because that run spent every one of its admitted
subjects. Ineligible repositories cast no vote and are not counted in `M`, so a
manifest built from a fresh scan holds only the new repositories, and
`MIN_ELIGIBLE_REPOS = 5` makes any `N < 5` `Inconclusive` on its own — before
any outcome is looked at.

**The two branches.**

| Branch | Preserved five | `M` | Required design | New proceed votes needed |
| --- | --- | --- | --- | --- |
| A — the build-time ledger is authoritative | Vote, `2 proceed / 3 pivot` | `5 + N` | `N >= 2` | 2 of 2 at `N = 2` |
| B — the current ledger is authoritative | Do not vote | `N` | `N >= 5` | 3 of 5 at `N = 5` |

Branch B's `M` is the point most easily got wrong, and getting it wrong inflates
the pass mark. Under branch B the preserved five are ineligible, so they are not
in the denominator and they contribute no votes to subtract: `M = N = 5`, strict
majority is `floor(5 / 2) + 1 = 3`, and the mark is 3 of 5. Carrying branch A's
`M = 5 + N` across to branch B would give a majority of 6 over a manifest of 10,
less the preserved arm's 2, or 4 of 5 — one vote too high, and credited to
repositories the same branch has just said do not vote.

Branch A says exposure is a claim about what the agent had seen *before* the run
being scored, so a repository's eligibility is fixed when its evidence is built
and a later scan cannot retroactively unseat a completed result. Branch B says
the gate means what it currently computes.

Under branch B the canonical convention is also a live constraint rather than a
settled one. `decide` computes the canonical tally first and treats it as the
base verdict every later precondition is applied to (`verdict.rs:124`). Where
the preserved five vote, canonical is already satisfied `5-0` and only the
alternative weights can bind; where they do not, the new repositories must reach
3 of 5 under canonical *before* any convention-invariance question is reached.

The branches differ by more than a factor of two in repositories to reserve and
mine, so this cannot be settled by whoever happens to build the manifest, nor by
which exposure ledger a build command happens to point at. It is a ruling, it
has to be recorded, and it has to be made *before* `N` is fixed. It also has the
shape ADR 0003 warns about: branch A is the cheaper reading and it is the one
that recovers evidence the campaign needs, which is the direction the ADR says
the pressure always runs. That is a reason to rule it explicitly and in writing,
not a reason to assume either answer here.

## What would falsify the confirmatory extension

Stated before any result exists and before any candidate has been chosen. The
mapping below is read from `decide` (`verdict.rs:97-158`) rather than from the
informal names of the three verdicts, because the two do not agree in the case
this design is most likely to land in.

**Falsified (`pivot`).** Two things must both hold. The three preconditions
`decide` checks before it reads the vote must pass — at least five eligible
repositories (`verdict.rs:105`), the power precondition (`verdict.rs:117`), and
no repository admitting zero identical-pair tasks under the canonical
convention (`verdict.rs:125`) — and then the **canonical** equal-weights tally
at run index 0 must fail to reach a strict majority, whether as a minority or as
an exact tie, since a tie defaults to pivot.

Both halves are load-bearing. A canonical minority is not on its own a `pivot`:
any of those three preconditions failing returns `Inconclusive` without the vote
being reached. And no other route to `pivot` exists — `decide` computes the
canonical base tally first (`verdict.rs:124`), and every precondition after it
can only downgrade a `Proceed` to `Inconclusive`. Per `aoa-6anq`'s fixed
decision mapping, pivot stops repository-layer expansion and prioritizes harness
work. It does not license a fourth design.

**Neither falsified nor confirmed (`inconclusive`).** A precondition fails
rather than the canonical vote. Each of these is a distinct path, and all of
them return `Inconclusive`:

- Fewer than five eligible repositories (`verdict.rs:105`).
- The power precondition: any eligible repository's held-out size below
  `min_holdout` (`verdict.rs:117`). The aggregate effect branch cannot fail
  here, since C5 fixes `min_effect_size=0.0`.
- A repository admitting zero identical-pair tasks, under the canonical
  convention (`verdict.rs:125`) or under any other (`verdict.rs:228`).
- **Determinism instability**: the tally at any run `1..K` differs from run 0
  (`verdict.rs:139`, `197-215`).
- **Convention-invariance flip**: canonical proceeds, but the verdict is not
  `Proceed` under one of the four preregistered conventions (`verdict.rs:147`).
- An admission-floor failure at seed 1, or unprovable held-out provenance on a
  new repository, either of which stops the manifest being built at all.

Inconclusive means no Wave 1 and no convention retuning. It is never silently
converted to `pivot`, and never to `proceed`.

**Confirmed (`proceed`).** The canonical tally reaches the strict majority and
every precondition above holds, including invariance across all four
conventions.

**The consequence worth stating in advance.** This design is derived on the
alternative weights `0.75/1.25`, the convention the preserved five fail. If the
canonical tally proceeds and the alternative weights then flip it, `decide`
returns `Inconclusive`, not `Pivot` (`verdict.rs:147`). The preregistered
consequence of that outcome is therefore the inconclusive one — no Wave 1, no
convention retuning — and specifically **not** the pivot consequence of stopping
repository-layer expansion.

These moves are excluded in advance, because each converts a pivot into a
proceed after the fact:

- Adding seeds beyond `K=3` to chase a vote. `aoa-6anq` recorded before the
  campaign ran that extra seeds cannot move the convention-invariance result,
  and the implementation agrees: `convention_invariant` tallies run index 0 only
  (`verdict.rs:227`). That is the exact scope of the claim. Extra seeds are not
  inert elsewhere — `determinism_satisfied` tallies runs `1..K` against run 0
  (`verdict.rs:204-212`), and one differing tally downgrades a `Proceed` to
  `Inconclusive`. Seeds can lose a proceed; they cannot win one.
- Dropping or substituting a repository once its outcomes are visible, or
  admitting only the surviving subset of a partially compromised one. That is
  the move the HTTPie verdict refused, at the cost of a repository.
- Retuning the `0.75/1.25` weights, or any other convention.
- Lowering `min_holdout`, or the `0.80` pair-yield floor.
- Reporting the aggregate weighted means, near-tied at roughly `0.6111` against
  `0.6088`, in place of the per-repository majority the verdict is defined by.

## What this commit does not decide

- **No candidate repository is named or inspected.** That is the next commit.
- **`N` is not fixed.** It cannot be until the exposure-ledger branch above is
  ruled, and it is a spend decision in any case.
- **Nothing was mined, no trial ran, and no spend occurred.** The two commands
  quoted above read artifacts that already existed and wrote nothing to any run
  directory.

---

# Application

Everything above this line was committed as
`6c8d7b5 docs: preregister the R0 third design's criteria and threshold rule (aoa-6da35)`
at **2026-08-07T00:24:32-04:00**, naming no candidate. Everything below was
committed afterwards. `git log --follow docs/r0-third-design-preregistration.md`
is the check that
[Why the ordering is the artifact](#why-the-ordering-is-the-artifact) asks for.

## C1 applied, exhaustively

C1 is the criterion that can be applied without proposing a candidate: it
defines the excluded set rather than selecting from the admitted one. Applying
it means enumerating every repository that has a persisted trial artifact under
any R0 run root, in any codeprobe checkout — not only the primary one.

### The exposure scan is not the instrument for this

`aoa eval exposure scan` answers "which repositories with a mined corpus are
exposed". That is a narrower question than C1's, and the gap matters twice.

It **errors rather than reporting an empty result** on a root that holds no
mined corpus: `scan_exposure` returns `NoExposureCorpora` when `load_corpora`
finds no `prep.json` + `mine.json` pair (`crates/aoa-bench/src/exposure.rs:135-140`).
A root with no corpus is exactly the case C1 has to distinguish — "never mined"
against "scanned and found nothing" — and a nonzero exit distinguishes neither.
It also **errors on a corpus-bearing root whose task files have since moved**,
which is the state of one R0 root below.

Of the eight R0 run roots the glob below reaches, the scan completes on three.
Revision 1 of this document reported the other roots as "none", which read as a
scan result and was not one. Three of the roots it could not scan do hold
persisted trial artifacts.

### What each root holds, measured

| Checkout | Run root | Mined corpus | Repositories with persisted artifacts | `exposure scan` |
| --- | --- | --- | --- | --- |
| `codeprobe` | `r0-campaign` | yes | 8 — the full set below | completes |
| `codeprobe` | `r0-file-read-seed1` | yes | 5 — gunicorn, isort, marshmallow, requests, rich | completes |
| `codeprobe` | `r0-preflight-current` | yes | the same 5 | completes |
| `codeprobe` | `r0-pilot` | no | none — two empty `experiment.json` scaffolds, no tasks, no trials | exits 1, no corpus |
| `codeprobe` | `r0-prompt-canary` | no | **requests** — 1 task, 8 files under `.codeprobe/runs` | exits 1, no corpus |
| `codeprobe-hrvq` | `r0` | no | **isort, sqlparse** — seeds 1–3, 825 files | exits 1, no corpus |
| `codeprobe-i83o` | `r0-campaign` | yes, 8 pairs | 8 — the full set below | exits 1, missing task file |
| `codeprobe-i83o` | `r0-pilot` | no | none — reports, logs, screening TSVs | exits 1, no corpus |

`codeprobe-i83o/runs/r0-campaign` is the root the scan cannot read at all:

```text
$ aoa eval exposure scan --runs "$CODEPROBE_ROOT-i83o/runs/r0-campaign"
error: failed to read .../r0-repos/httpie/.codeprobe/tasks/
       comprehension-dependency_analysis-010-a2f47e78/instruction.md:
       No such file or directory (os error 2)
```

### Two scan-independent enumerations, and they agree

Both run over every checkout at once and neither can exit early on a root it
cannot classify. The first enumerates repositories with a mined corpus; the
second reads the repository each persisted task names, which reaches the roots
that hold trials without a corpus:

```bash
: "${CODEPROBE_ROOT:?export CODEPROBE_ROOT first (see the runbook)}"
# Sibling checkouts share the prefix, so the glob covers every one of them.

# A — repositories with a mined corpus
find "$CODEPROBE_ROOT"*/runs -name mine.json -printf '%h\n' \
  | xargs -n1 basename | sort -u

# B — repositories named by persisted task metadata
find "$CODEPROBE_ROOT"*/runs -name metadata.json -path '*tasks*' \
  -exec jq -r '.repo // empty' {} + | sort -u
```

Both return the same eight names. `A` alone would miss the two roots that hold
trials but no corpus; `B` catches them, and adds no ninth repository.

The union is exactly eight repositories, each pinned at one baseline revision
across every root that contains it:

| Repository | Baseline commit | Status |
| --- | --- | --- |
| gunicorn | `a8283bbf5e1416d0dd13f994f71ec2761988aeab` | exposed (16/16) |
| httpie | `5b604c37c6c67e18e7c3e9aee6c88a8c22b98345` | partially exposed (7/14) |
| isort | `fd8bd075176d074af69aa6acae7ed89a6a89bb05` | exposed (14/14) |
| marshmallow | `cd3cda8b1a9ae740d439538cee7aa8faea58d6b9` | exposed (7/7) |
| requests | `23953c0c875219a715f081cf3de7c149a7629ccf` | exposed (14/14) |
| rich | `9d8f9a372cc5916fd4781fec207ced7ddac2f08f` | exposed (19/19) |
| sqlparse | `f80af6a4007f11ada847218df8c29dc859238290` | exposed (16/16) |
| websockets | `ff4869ba468129f3e85b08c2a8a03ec45cf26537` | exposed (12/12) |

The `Status` column is the exposure classification from the three roots the scan
completes on. The trials in `r0-prompt-canary` and `codeprobe-hrvq/runs/r0` do
not change it: they name requests, isort and sqlparse, all three already
`exposed` and already excluded. They are recorded because C1 asks whether a
repository was *drawn*, and a trial that no exposure ledger counts is still a
subject that has been seen.

**C1 therefore admits any repository outside this table**, and it excludes every
repository inside it — including httpie, whose 7-of-14 partial exposure is
excluded whole per the [HTTPie verdict](r0-httpie-held-out-verdict.md) rather
than reduced to its surviving tasks.

This agrees with the eight-repository enumeration in
[the reserve spendability record](r0-reserve-spendability.md#every-repository-ever-reserved-or-drawn-for-r0),
which reached it from run artifacts and closed decision records rather than from
the gate. Three independent derivations now, same set.

A separate caution for whoever selects next: the `r0-repos` directory alongside
the codeprobe checkouts holds working checkouts of many repositories beyond
these eight. A checkout is not a draw — none of them has a persisted trial
artifact under any R0 root, which is what C1 tests — but the directory is not
evidence of held-out status either, and C1 must be re-run against the
enumerations above at selection time rather than read off that listing.

## The threshold instantiated

From the rule in the criteria commit, with no new principle. Each branch uses
its own `M`:

| Branch | `N` | Manifest `M` | Strict majority | New repos that must vote proceed |
| --- | ---: | ---: | ---: | --- |
| A — preserved five vote | 2 | 7 | 4 | 2 of 2 — unanimous |
| B — preserved five ineligible | 5 | 5 | 3 | 3 of 5 |

Row A is numerically identical to the superseded two-repository amendment, which
is expected: the same rule over the same manifest size returns the same mark.

Row B is `M = N`: the preserved five are ineligible under this branch, so
`partition` drops them before `decide` sees the manifest and they are neither
votes for, votes against, nor denominator. `floor(5 / 2) + 1 = 3`, which is the
same 3 of 5
[the criteria commit's branch table](#the-precondition-the-count-depends-on)
states.

Illustrative cost at the inherited planning rate of `$0.2577/trial` and the
prior corpora's typical 14 dual tasks per repository, `tasks x 2 arms x 3
seeds`:

```text
Branch A, N=2:  28 tasks -> 168 trials -> $43.29
Branch B, N=5:  70 tasks -> 420 trials -> $108.23
```

These follow `N` and are unaffected by the `M` correction above. Both are
placeholders for codeprobe's exact dry-run, which cannot be produced until the
corpora are mined. They are shown because the gap between them is the decision,
not because either is authorized.

## No repository is chosen here, and why

This is a deliberate stop, not an unfinished step. Three reasons, in decreasing
order of how much they bind.

**1. The ruling that was made rests on arithmetic that does not hold.** Option 1
reads "reserve and mine **a** new repository", and Stephanie ruled it on
2026-08-06 against a record that described the change as affecting "the manifest
arithmetic". The criteria commit shows one new repository cannot reach proceed
under either branch. So the option as ruled is not executable, and its
executable forms cost either two repositories or five. Choosing between them is
a spend-scale decision, and `aoa-6anq` reserves spend authorization explicitly.
Selecting repositories here would present that choice as already made.

**2. `N` is not determined.** It follows from the exposure-ledger branch, which
is a ruling and not a build detail. Selecting repositories before `N` is fixed
means either selecting for the smaller count and topping up later — so that the
later additions are chosen with knowledge the first ones were not — or selecting
for the larger and discarding, which is selection after the fact wearing a
different hat.

**3. The pool and the tie-break are themselves criteria, and they are not
preregistered.** The criteria commit fixes admission (C1–C5) but not which
universe of repositories is considered, nor how to order admissible candidates
when there are more of them than `N`. Both decide the outcome of selection as
surely as C1 does. Writing them now and applying them in this same commit would
produce exactly the pattern the two-commit structure exists to rule out: a rule
published simultaneously with the choice it justifies. They belong in a
criteria commit of their own, before any candidate is examined.

## Disclosure

One candidate-shaped repository name was contacted: `git ls-remote` against
`pallets/click`, run to establish whether this environment had network access at
all before deciding what the application commit could contain. It returned a
HEAD SHA. No criterion was evaluated against it, it is not proposed as a
candidate, and it carries no standing from having been the string used in a
connectivity check.

**Its timing, since that is the only fact the ordering argument needs.** It was
run during the work on `aoa-6da35`, which began at `2026-08-06T23:11:35-04:00`
when the per-bead worktree was created, and before revision 1's application
commit at `2026-08-06T23:36:12-04:00`. No finer timestamp was recorded at the
time, so this document does **not** claim the contact followed the criteria
commit; it may have preceded it. What is checkable instead is the criteria
commit's own content:

```bash
git show 6c8d7b5:docs/r0-third-design-preregistration.md | grep -i click
```

which returns nothing. The criteria commit names no candidate and evaluates no
criterion against one, whenever the connectivity check happened. Revision 1's
criteria commit passed the same check before it was rewritten. It is recorded
here because a
candidate name that was touched and left unmentioned is the kind of omission
that makes a selection record unfalsifiable later.

## What has to happen next

Two rulings, then one bead:

1. **Which exposure ledger is authoritative for the preserved five** — the
   build-time ledger (branch A) or a current scan (branch B). This fixes `N` at
   `>= 2` or `>= 5`, and with it the pass mark at 2 of 2 or 3 of 5. The criteria
   commit sets out both readings and takes neither.
2. **Whether option 1 is still the ruling at its real cost**, now that it is two
   repositories or five rather than one. If it is not, the remaining options are
   unchanged and option 2 remains closed; that is a fresh decision, not a
   fallback available locally.

Then a criteria commit fixing the candidate pool and the ordering rule, and only
after it, the selection.

## Confirmation

Nothing was mined. No agent trial ran. No spend occurred. Every command in this
document reads artifacts that already existed, plus one `git ls-remote` that
transferred no repository content. No run directory was written to, and the
protected `falsification.k3.json` was not modified.
