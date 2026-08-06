# 0003 — Held-out provenance is load-bearing evidence

**Status:** Accepted. Recorded here in 2026-08 from the decision as it already
stands in `CLAUDE.md`'s conventions and in the R0 campaign records under
`docs/`.

## Context

Every score AOA reports is scored against a hidden version of the task the agent
never saw. The gap between visible-test performance and held-out performance is
the toolkit's primary signal, and it is only a signal while "the agent never saw
this" is true. That claim is about history, not about code: it depends on which
repositories were reserved before outcomes existed, and on whether a given
artifact was produced before or after the agent had been run against the same
questions.

History is exactly the thing that is convenient to reinterpret once results are
in. The pressure is always in one direction — toward keeping the tasks that
still look clean.

## Decision

Held-out provenance and anti-leakage checks are treated as load-bearing and are
not weakened to make an experiment pass. When held-out status cannot be proven,
the repository or artifact is demoted; the standard is not relaxed to fit it.

Two rules follow from that and are already applied:

- The pre-outcome reserve is taken whole. Selecting the surviving subset of a
  repository's tasks after discovering that prior outcomes exist is not
  permitted, because that selection is made with knowledge of the outcomes.
- `unknown` is a recorded evidence result, not a gap to be filled with the most
  plausible value.

## Consequences

The rules cost real experimental power, and have. HTTPie was demoted from the
R0 confirmatory reserve on 2026-08-04 after seven of its fourteen mined dual
tasks were found to ask the same repository questions as seven baseline trials
attempted a month earlier. Half the tasks had no prior scored record and would
have survived a narrower reading; taking only those would have violated the
whole-reserve rule. HTTPie was demoted and not replaced.

Likewise, the codeprobe commit that produced every completed R0 arm is not
recoverable from the preserved evidence: the artifacts record a codeprobe
version but no commit, checkout path, or dirty-tree state. Git reflogs bound
what `main` pointed at during the outcome windows, but cannot prove the process
ran from that checkout. That is recorded as `unknown` with forensic bounds
rather than resolved into a retroactive pin.

The practical consequence for a contributor: a change that makes a held-out
check pass by loosening what it checks is a defect even when the resulting
numbers improve, and especially then.

## Where this lives

- `CLAUDE.md`, "Conventions & Patterns" — the standing rule.
- `docs/r0-httpie-held-out-verdict.md` — the demotion and its reasoning.
- `docs/r0-codeprobe-execution-provenance.md` — `unknown` as an evidence
  result, with the bounds that support it.
- `docs/r0-confirmatory-extension-prep.md` — the reserve as it stands after the
  demotion.
