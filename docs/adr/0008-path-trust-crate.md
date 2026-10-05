# 0008 — `aoa-path-trust` owns the filesystem and git trust primitives

**Status:** Accepted. Recorded here in 2026-10 from aoa-9pylp, which asked
whether `aoa-trace` should be declared the workspace's shared-primitives floor
or give the path-trust boundary a crate of its own. The project lead ruled for
the split on 2026-10-05.

## Context

aoa-ulmk4 gave the path-trust boundary a single owner so that a bypass fixed
once is fixed for every consumer. It put that owner in `aoa-trace`, as the
module `path_trust`, because both consumers already depended on `aoa-trace` and
the crate already hosted `validate_single_component`.

That placement had a cost that grew with every change:

- `aoa-trace` is the trace wire format's owner (record 0002), and the format is
  its reason to change. Over 2026-10-05 the module gained the git environment
  strip, the repository-root resolver, the submodule trust root and the
  descriptor-based directory walker. Each one was a change to the trace crate
  that touched no trace type.
- The module needed `rustix` on Unix, so the trace crate carried a dependency
  no trace code used.
- `aoa_trace::dirfd::map_nofollow_error` took a `rustix::io::Errno`. A caller
  had to name the same `Errno` type, so the `rustix` major version was part of
  the trace crate's public API. The module documented the coupling and did not
  remove it.

The alternative was to declare `aoa-trace` the shared-primitives floor and say
so in its docs. That would have made the trace crate the answer to "where does
a primitive two crates need go", which is the drift record 0001 exists to
prevent.

## Decision

The primitives live in `crates/aoa-path-trust`, a library crate with no
internal dependency. `aoa-trace` does not depend on it and does not re-export
it. Every consumer imports from `aoa_path_trust` directly: `aoa-audit`,
`aoa-enforce` and the `aoa` CLI.

The crate holds filesystem and git trust primitives and nothing else:

- whether a name may be joined onto a trusted base
  (`validate_single_component`),
- the symlink-refusing join and its single-node checks (`safe_join_nofollow`,
  `reject_symlink`, `is_symlink_nofollow`),
- descriptor-based directory acquisition on Unix (`dirfd`),
- lexical and canonicalizing resolution (`normalize_lexically`,
  `resolve_canonicalizing`),
- the repository root and the linked-worktree check
  (`resolve_repository_root`, `linked_worktree_points_back`),
- the environment a git subprocess may inherit
  (`git_free_of_inherited_state`, whose rules are record 0007).

It may acquire another primitive of that kind: a rule about which path a write
may reach, which repository a command runs against, or what a git subprocess
may inherit. It may not acquire a trace type, a measurement, a finding, a
policy, or a dependency on another workspace crate. A helper that is merely
shared by two crates does not belong here on that ground alone. `aoa-domain`
stays vocabulary only and is not the home for these either.

No `rustix` type appears in the crate's public surface. `map_nofollow_error`
takes a `std::io::Error` and compares its raw OS error, and the descriptor
functions take and return `std::os::fd` types. `rustix` is an implementation
detail of this crate, and a consumer that also uses `rustix` picks its own
version.

The `dirfd` functions take the entry name as `impl AsRef<OsStr>`, not `&str`
(aoa-7g14y.2, 2026-10-05). `aoa-enforce` classifies a refused open of the live
log file through `map_nofollow_error`, and that file name reaches it as the
last component of a `Path`, which need not be UTF-8. Widening the parameter
here kept one classification for every consumer; a `&str` caller compiles
unchanged.

## Consequences

`aoa-trace` has no `rustix` dependency and its manifest lists only what the
trace format needs. A hardening of the trust boundary no longer rebuilds or
re-versions the trace crate.

`aoa-path-trust` sits in "Capture and inputs" ahead of `aoa-trace` in
`CLAUDE.md`'s layer list, so any later layer may depend on it. It is drawn as
its own container in `architecture/model.c4`, with the two library edges that
exist (`aoa-audit` and `aoa-enforce`).

The split moved code and tests without changing behaviour. The seven
integration tests that exercise the resolver and the git environment moved
with the module, and the module's unit tests moved inside it.

Left as it was:

- `aoa-audit`, `aoa-enforce` and `aoa-budget` still depend on `rustix`
  themselves, for descriptor-relative file operations of their own. This
  record removes the forced version coupling, not those uses.
- `aoa-enforce` kept private copies of the directory acquisition in
  `crates/aoa-enforce/src/live_log/open.rs` until aoa-7g14y.2 deleted them.
  It now acquires `.aoa` and `.aoa/traces` through `dirfd` and keeps only the
  final `openat` of the log file, which is a file open and not a directory
  acquisition.
- `aoa-budget` has its own descriptor walk in
  `crates/aoa-budget/src/boundary.rs`. It predates this record and was not
  folded in here.
- Nothing mechanical stops a crate from re-implementing one of these
  primitives. Single ownership is held by review and by this record.

## Where this lives

- `crates/aoa-path-trust/src/lib.rs`: the crate's surface and the question
  each primitive answers.
- `crates/aoa-path-trust/Cargo.toml`: the one `rustix` requirement, and no
  workspace crate under `[dependencies]`.
- `CLAUDE.md`, "Architecture Overview": the layer assignment and the reason
  for the separate crate.
- `architecture/model.c4`: the `aoa.trust.pathTrust` element and its edges.
- `crates/aoa/tests/architecture_doc.rs` and
  `crates/aoa/tests/architecture_model.rs`: fail the workspace tests if the
  crate loses its layer, gains a dependency on a later layer, or the model
  stops matching its manifest.
