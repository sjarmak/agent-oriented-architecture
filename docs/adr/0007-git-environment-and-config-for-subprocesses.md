# 0007 — A git subprocess inherits only what can make git refuse, loudly

**Status:** Accepted. Recorded here in 2026-10 from aoa-j4xvf, which routed the
CLI's git calls through one environment strip, aoa-b3oj3 items (2) and (3),
which asked whether what the strip leaves inherited was deliberate, and
aoa-8rzo8, which found operator git configuration changing a measured answer.

## Context

AOA shells out to git in three places, and each one parses what git prints:

- the repository-root resolver in `aoa-trace` (`git rev-parse` for the top
  level and the git directory),
- the audit's pre-commit plane in `aoa-audit` (`git rev-parse` for the hook
  path),
- the CLI's data readers in `crates/aoa`: `policy infer-owners` (`git ls-tree`,
  `git blame`) and `gap mine-corpus` (`git rev-parse`, `git log`).

Every one of them names its repository explicitly with `-C`. Git still reads
two other inputs the caller never typed: the process environment and the
machine's git configuration. Both reached the parsed output:

- An inherited `GIT_DIR`, `GIT_OBJECT_DIRECTORY` or `GIT_COMMON_DIR` pointed
  the call at a different repository than the one named. A session running
  inside a git hook exports these without anybody asking.
- `GIT_TRACE=/dev/stdout` folded trace lines into the output being parsed.
- `GIT_TEST_ASSUME_DIFFERENT_OWNER=1` made git refuse the repository under
  audit, and the audit reported an installed pre-commit hook as missing
  (commit 88357cf).
- A global or system config setting `blame.ignoreRevsFile` to a file that does
  not exist made `git blame` exit 128, and `policy infer-owners` failed on a
  repository it could otherwise read. The same failure arrives through an
  ordinary `~/.gitconfig` with no environment variable set, so the class is
  operator configuration, not inherited environment (aoa-8rzo8).

Each was fixed where it was found. Without a stated rule the next variable or
setting gets decided again from scratch, and the two sites that deliberately
inherit look like oversights.

## Decision

**Strip what can choose, redirect or reshape the repository or the output.
Inherit only what can do nothing but make git refuse, loudly.**

A setting that silently changes which repository is read, where output goes, or
what the output says is removed, because a measurement that depends on it is a
measurement of the operator's shell. A setting whose only possible effect is
that git exits non-zero with its own message may stay inherited: the operator
sees git's refusal verbatim and nothing wrong is reported as a result.

The rule is applied in four places.

**1. The shared strip, for every git subprocess.**
`aoa_trace::git_free_of_inherited_state` is the only constructor for a git
`Command` in production code. It removes the 17 repository-local variables
(`GIT_DIR`, `GIT_WORK_TREE`, `GIT_OBJECT_DIRECTORY`, `GIT_CONFIG_PARAMETERS`,
`GIT_CONFIG_COUNT` and the rest of the list in that file), removes every
variable named `GIT_TRACE*` or `GIT_REDIRECT_*`, and sets the three trace2
targets to `0`. All of these choose a repository, redirect a stream, or add
lines to output.

**2. The root resolver inherits machine config and the ownership test
variable.** `resolve_repository_root` adds nothing to the shared strip, so
`GIT_CONFIG_GLOBAL`, `GIT_CONFIG_SYSTEM`, the files they default to, and
`GIT_TEST_ASSUME_DIFFERENT_OWNER` all reach it. The resolver asks git one
question, where a repository's top level and git directory are, and no
configuration changes that answer. What configuration can do is decide whether
git answers at all: `safe.directory` is what lets git read a repository owned
by another user, and it is honoured only from system, global and command scope.
Cutting the resolver off from machine config would turn every such repository
into a refusal the operator has already configured away. The ownership variable
is the mirror case: it can only make git refuse, the resolver reports that
refusal with git's own message, and
`enforce_check_reports_safe_directory_exit_128_as_the_actual_git_failure` uses
it to reach the refusal without a fixture owned by another user.

**3. The audit strips the ownership variable and reads no machine config.**
The same variable is not harmless to the audit. The audit's pre-commit plane
treats a failed `git rev-parse` as "no hook here", so an inherited
`GIT_TEST_ASSUME_DIFFERENT_OWNER=1` turned a present hook into a missing one:
a refusal that was not loud, and a wrong finding. The audit's own builder in
`crates/aoa-audit/src/planes.rs` therefore removes it and points
`GIT_CONFIG_GLOBAL` and `GIT_CONFIG_SYSTEM` at the null device with
`GIT_CONFIG_NOSYSTEM=1` (commit 88357cf). The strip is the audit's and not the
shared builder's, because the resolver needs the opposite.

**4. The CLI's data readers run without the operator's machine config, and are
handed its `safe.directory` values.** `policy infer-owners` and
`gap mine-corpus` parse blame, tree and log output, which machine config can
reshape or break. They build their commands with
`commands::git::reading_repository_data`, which starts from the shared strip,
sets `GIT_CONFIG_NOSYSTEM=1` and `GIT_CONFIG_GLOBAL` to the null device, and
passes each `safe.directory` value the operator's system and global config
hold as `-c safe.directory=<value>`, in the order git read them. The values are
read once per process, by a git that still sees the operator's config, with
`git config --includes --null --show-scope --get-all safe.directory`; only
`system` and `global` scope entries are kept. Command scope is one of git's
protected scopes, so the ownership decision these readers get is the one the
operator's own git would make.

Repository-local config (`.git/config`) stays in force everywhere. It belongs
to the repository being measured, travels with it, and is the same for every
operator who measures that checkout.

## Consequences

- A new git call in the CLI that parses output uses
  `reading_repository_data`. A new call that only locates a repository may use
  the shared strip directly, and has to be able to say why config cannot change
  its answer.
- A new environment variable or setting is placed by the rule, not by analogy:
  if it can only cause a loud refusal it may be inherited, otherwise it is
  stripped at the narrowest builder that needs it.
- The data readers no longer see the operator's `blame.ignoreRevsFile`,
  `log.*`, `diff.*`, `core.quotePath`, aliases or any other machine-level
  setting. An operator who relied on a global `blame.ignoreRevsFile` to keep a
  formatting commit out of `infer-owners` gets raw attribution; the setting
  works again when it is put in the repository's own config.
- `infer-owners` and `mine-corpus` run one extra `git config` per process. If
  that read fails for a reason other than the key being unset, the command
  fails with git's message instead of guessing an empty list.

### What this deliberately does not cover

- **`GIT_REDIRECT_*` is proven on the built `Command` only.** The variables
  redirect git's standard streams on Windows and are ignored elsewhere. CI
  never runs Windows, so
  `crates/aoa-trace/tests/git_environment_inherited_redirects.rs` asserts that
  the constructed command removes them and no test observes a Windows git
  being unaffected (aoa-b3oj3 item 2).
- **Every `safe.directory` value rides on the command line.** A machine whose
  config holds a very large list makes every data-reading git call carry all
  of it. The operating system's argument limit is the bound; past it the spawn
  fails with the system's error, naming the git call. Nothing filters the list
  to the repository being read, because that would reimplement git's matching.
- **The `safe.directory` read happens in the directory AOA was started from,
  once.** An `includeIf "gitdir:..."` block in machine config is evaluated
  against that directory's repository, not against each repository later named
  with `--repo` or `--clones`. A starting directory whose `.git` file is
  corrupt makes the read, and so the command, fail with git's message.
- **Repository-local config can still reshape output.** A `.git/config` that
  sets `blame.ignoreRevsFile` changes `infer-owners` for everyone who measures
  that checkout. That is the repository's statement about itself and is treated
  as input.

## Where this lives

- `crates/aoa-trace/src/path_trust/git_environment.rs`,
  `fn git_free_of_inherited_state`: the shared strip.
- `crates/aoa-trace/src/path_trust/root.rs`, `fn git_resolved_path`: the
  resolver, which inherits machine config and the ownership variable.
- `crates/aoa-audit/src/planes.rs`, `fn git`: the audit's builder.
- `crates/aoa/src/commands/git.rs`, `fn reading_repository_data`: the data
  readers' builder and the one read of the operator's `safe.directory`.
- `crates/aoa-trace/tests/git_environment_repository_variables.rs` and
  `git_environment_inherited_redirects.rs`: every stripped name pinned on the
  built command.
- `crates/aoa/tests/cli_sections/audit_git_environment.rs` and
  `cli_git_environment.rs`: the audit and the data readers answer the same
  under each inherited variable.
- `crates/aoa/tests/cli_sections/cli_operator_git_config.rs`: `infer-owners`
  answers the same under a global, a system and a home-directory config that
  breaks `git blame`; a distrusted repository the operator's config marks safe
  is still read; one it does not mark safe is refused with git's message.
