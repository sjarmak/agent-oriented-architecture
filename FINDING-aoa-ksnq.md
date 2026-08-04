Source: DEEP_AUDIT 2026-08-04, section 2 of .gc-reports/audit-2026-08-04.md. VERIFIED BY EXECUTION: External -> votes in R0 = false; NativeComposed -> true; while compute_gap(External) returns Ok(Some(1.0)), a CERTIFIED gap.

crates/aoa-falsify/src/eligibility.rs:13 requires matches!(e.native_span, HeldOutProvenance::NativeComposed).

Three sites disagree about what External means:
- crates/aoa-gap/src/provenance.rs:5  -- 'Only External and NativeComposed suites can certify a real gap.'
- crates/aoa-bench/src/provenance.rs:19-22 -- External short-circuits BEFORE the backend count, commented 'the commit is the stronger contamination-free anchor.'
- crates/aoa-bench/src/codeprobe_run.rs:383 -- an all-External set aggregates to External.

CONSEQUENCE, and this is why it is P1 rather than cosmetic: a repo of org-scale codeprobe tasks each carrying a real ground_truth_commit -- the STRONGEST anti-contamination anchor available -- aggregates to External and contributes ZERO votes, pushing R0 toward TooFewRepos. And because any_native is ordered ahead of External at codeprobe_run.rs:379-383, adding one WEAKER-anchored task (no commit, two mined backends) flips the whole repo from ineligible to eligible. That is backwards.

No test covers is_eligible with External; crates/aoa-falsify/tests/falsify_test.rs exercises only NativeComposed and SynthesizedFromVisible (:319, :330, :341). Either the predicate or the documented contract is wrong and nothing pins which.

This sits directly on the R0 chain, so resolve the CONTRACT first, then encode it. Do NOT weaken an anti-leakage check to make a run eligible.
FIX (whichever the contract says): if External should vote, predicate becomes matches!(..., NativeComposed | External). If it deliberately must not, say so in eligibility.rs AND add a ProvenanceExcluded reason to the report so a fully-External manifest fails LOUDLY instead of degrading to TooFewRepos.
TESTS: is_eligible with External, plus an all-External manifest asserting the loud failure mode.

REINVENTION_GATE: bd search --status all run for the finding terms; zero hits, control searches confirm search was live. Sibling findings from the same DEEP_AUDIT: aoa-zh3i, aoa-zgqi, aoa-luqt.
