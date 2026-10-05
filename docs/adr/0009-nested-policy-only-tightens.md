# 0009 — A nested repository's policy may only tighten: a write is refused if any enclosing governed repository refuses it

**Status:** Accepted. Recorded here in 2026-10 from aoa-msnkt, which asked
whether in-tree content may choose its own governing policy by creating a
nested repository. The project lead ruled on 2026-10-05 that it may not.

## Context

`aoa enforce check` resolves a pending write to the repository git would use
for the session's working directory and applied that repository's
`aoa-policy.yaml`, and only that one. aoa-wl3ye, aoa-309pf and aoa-hz7lt settled
how that root is chosen. None of them asked whether the chosen root's policy
should be the only one consulted.

It was the only one, and that let the governed party pick its governor. A
writer inside a governed repository could:

- run `git init` in a subdirectory and write an `aoa-policy.yaml` there;
- delete an outer submodule's `.git` marker, so an inner one became the root;
- replace a submodule path with a symlink to a directory holding a copy of its
  `.git` file.

In each case the resolver correctly answered "the nested repository", the
nested policy said `reproduction_required: false` with no protected paths, and
every refusal the enclosing repository declared stopped applying beneath that
directory. The test
`enforce_check_applies_the_policy_of_a_nested_repository_named_one_space_not_its_parent`
pinned this as intended behaviour. Codex raised the submodule variants on
67bcfdb and they were ruled non-blocking there because they were no wider than
`git init`. That was true and was the problem: `git init` was already an
escape.

`aoa-enforce` exists to bound what an agent may write inside a governed
repository. A rule under which one command moves the bound is not a boundary.

## Decision

A write is checked against the policy of every governed repository that
encloses the session's repository, from the innermost (the root the resolver
chooses, unchanged) outward, and it is refused if any of them refuses.

- The walk is `governing_roots` in `crates/aoa/src/commands/enforce.rs`. It
  asks `resolve_repository_root` for the repository above each root in turn, so
  the enclosing roots are found by the same resolver, with the same refusals,
  as the innermost one.
- Each root's policy is evaluated against the target expressed relative to
  that root. A parent that protects `nested/src/**` refuses a write to
  `src/lib.rs` made from inside `nested`; a parent that protects `src/**` does
  not, because that pattern names the parent's own `src`.
- A root with no `aoa-policy.yaml` contributes nothing. The default
  reproduction requirement for a repository with no policy file applies to the
  innermost root only, as before.
- An enclosing policy's `reproduction_required` holds even when the nested
  policy switches it off. The evidence is still read from the session's own
  live log.
- A target the innermost repository does not contain is still checked against
  every enclosing root that does contain it. Before this record such a write
  was out of scope entirely, so a session in `nested` could reach the parent's
  protected files by spelling them `../src/lib.rs`.
- The decision is allow or block, so "refuse if any refuses" needs no merge of
  policy files. A nested policy can add a refusal and can never lift one.
- The block message names the root whose policy refused, as
  `(enforced for <root>)`, and the `write.blocked` span carries it as
  `policy_root`. Spans are still written to the session's own repository; an
  enclosing repository's `.aoa/` is never created by a session it merely
  encloses.

## Consequences

**The walk stops where the resolver stops.** The resolver strips
`GIT_CEILING_DIRECTORIES` (record 0007) and walks markers to the filesystem
root, so the walk does too. An enclosing repository that git refuses (a broken
marker, a mismatched backlink, a bare repository) refuses the write rather than
being skipped. Skipping it would let a writer disable an outer policy by
damaging the outer marker. The cost is that a checkout sitting under a broken
repository denies every guarded write until the outer repository is repaired.

**Checkouts nested for convenience are governed twice.** A linked worktree or
clone kept under another governed checkout now also answers to that checkout's
policy, with its paths spelled from the outer root. Patterns anchored at the
outer root's own tree do not match it; an outer reproduction requirement does
apply.

**Every enclosing level costs one more root resolution per check.** The common
case is one extra marker walk that finds nothing.

## What this leaves open

- **The outermost repository's own `aoa-policy.yaml`.** An agent that can edit
  the outermost governed policy can loosen it. Enforcement cannot bound the file
  that defines it without a second authority; that change belongs to review.
- **A session whose repository is not inside the governed one.** The walk
  starts from the session's repository, not from the target. A session working
  in an unrelated repository that writes into a governed one by absolute path is
  out of scope, as it was before (aoa-7g14y.1 chose that scope so the gate does
  not cover the whole machine).
- **A nested repository below the session's root.** A session in the parent
  writing into `nested/` is checked against the parent's policy and any above
  it. The nested repository's own policy is not consulted, so the refusals it
  adds bind only sessions working inside it.

## Where it is enforced

`crates/aoa/tests/cli_sections/enforce_nested_repository.rs` holds the rule at
the CLI boundary: an outer refusal holds inside a nested repository and inside
a submodule, for a target outside the nested repository, and for the
reproduction requirement; a nested policy still adds a refusal; an enclosing
repository with no policy contributes nothing; an enclosing repository git
refuses denies the write. Each refusal test proves the planted target was not
modified.
