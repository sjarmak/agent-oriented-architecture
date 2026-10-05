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
encloses the target, from the innermost outward, and it is refused if any of
them refuses.

- The walk is `governing_roots` in `crates/aoa/src/commands/enforce.rs`. It
  starts from the target, not from the session: the innermost root is the one
  `resolve_repository_root` gives for the target's nearest existing ancestor
  directory, and each further root is the one it gives for the directory above
  the last. Every root is found by the same resolver, with the same refusals,
  as the session's own. A session in the parent that writes into `nested/` is
  therefore held to the refusals `nested` adds.
- Scope is unchanged from aoa-7g14y.1. A target that lies outside every
  repository enclosing the session's own is out of scope: it is allowed, no
  policy is read for it and nothing is recorded. Starting the walk from the
  target adds roots beneath or beside the session's repository; it does not
  extend the gate to an unrelated repository reached by absolute path.
- Each root's policy is evaluated against the target expressed relative to
  that root. A parent that protects `nested/src/**` refuses a write to
  `src/lib.rs` made from inside `nested`; a parent that protects `src/**` does
  not, because that pattern names the parent's own `src`.
- A root with no `aoa-policy.yaml` entry contributes nothing. The default
  reproduction requirement for a repository with no policy file applies to the
  session's own repository only, as before.
- Only a genuinely absent entry counts as no policy. An `aoa-policy.yaml` that
  exists but cannot be read as a policy refuses the write, at every root in the
  walk including the innermost: a symlink (whether or not its target exists),
  an entry that is not a regular file, a file the process may not read, a
  malformed file. The file is opened relative to the root's descriptor without
  following links (`read_regular_file_nofollow` in `aoa-path-trust`), so a
  policy is never read through a link. Before this, a dangling link read as
  "no policy" and a live one let the governed party point the enclosing root at
  a file of its choosing.
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

**Policies govern the canonical location a write lands in.** The walk starts
from the target with its symlinks resolved, so a write through a linked
directory is checked where it arrives. With `/P/nested` a link to `/P/real`, a
write to `nested/src/lib.rs` is checked against `/P`'s policy as
`real/src/lib.rs`, and a policy protecting `real/src/**` refuses it. A link to
a repository outside `/P` is governed by that repository: the write is held to
that repository's policy and to those of the repositories enclosing it, not to
the ones enclosing `/P`. So a link created inside `/P` to somewhere outside it
moves the writes made through it out from under `/P`'s patterns for the
destination. That is not an escape of this rule, because nothing inside `/P`
is written, but it is a consequence a policy author has to know. What `/P`
keeps is the spelling: the path as written still lies in `/P`, so `/P`'s
patterns are matched against the alias (`linked/src/lib.rs`) and its
reproduction requirement still applies. That is the existing protected-alias
rule in `write_scope`, unchanged here.

**The walk stops where the resolver stops.** The resolver strips
`GIT_CEILING_DIRECTORIES` (record 0007) and walks `.git` entries to the
filesystem root, so the walk does too. An enclosing repository whose `.git`
entry git refuses (a broken marker, a mismatched backlink, a marker naming a
repository rooted elsewhere) refuses the write rather than being skipped.
Skipping it would let a writer disable an outer policy by damaging the outer
marker. The cost is that a checkout sitting under a broken repository denies
every guarded write until the outer repository is repaired. A bare repository
in an ancestor directory is not detected at all: it has no `.git` entry for
the resolver to find, so it neither governs the write nor refuses it.

**A write into a directory the resolver refuses is refused.** Because the walk
starts from the target, a target whose nearest existing directory is inside a
repository's `.git` directory, or inside a nested repository git will not
validate, is denied even when the session's own repository is sound. Before
this record such a write was evaluated against the session's policy alone.

**Checkouts nested for convenience are governed twice.** A linked worktree or
clone kept under another governed checkout now also answers to that checkout's
policy, with its paths spelled from the outer root. Patterns anchored at the
outer root's own tree do not match it; an outer reproduction requirement does
apply.

**Every enclosing level costs one more root resolution per check.** The common
case is one extra marker walk that finds nothing. A target in a repository
nested below the session's adds one resolution per nested level.

## What this leaves open

- **The outermost repository's own `aoa-policy.yaml`.** An agent that can edit
  the outermost governed policy can loosen it. Enforcement cannot bound the file
  that defines it without a second authority; that change belongs to review.
- **A session whose repository is not inside the governed one.** A session
  working in an unrelated repository that writes into a governed one by
  absolute path is out of scope, as it was before (aoa-7g14y.1 chose that scope
  so the gate does not cover the whole machine).
- **A swap between the check and the write.** The policy file is opened without
  following links, but the check runs before the tool does. A directory
  replaced by a link after the check and before the write is not seen.

## Where it is enforced

`crates/aoa/tests/cli_sections/enforce_nested_repository.rs` holds the rule at
the CLI boundary: an outer refusal holds inside a nested repository and inside
a submodule, for a target outside the nested repository, and for the
reproduction requirement; a nested policy still adds a refusal; an enclosing
repository with no policy contributes nothing; an enclosing repository git
refuses denies the write; a session in the parent is held to a refusal the
nested repository adds; a repository the session is not inside stays out of
scope; a write through a linked directory is refused at the location it lands
in, and one through a link to another repository by that repository's policy;
a symlinked, dangling or malformed policy refuses, including the session's own.
Each refusal test proves the planted target was not modified.
