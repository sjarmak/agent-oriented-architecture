# 0007 — A git subprocess inherits only what can make git refuse, loudly

**Status:** Accepted. Recorded here in 2026-10 from aoa-j4xvf, which routed the
CLI's git calls through one environment strip, aoa-b3oj3 items (2) and (3),
which asked whether what the strip leaves inherited was deliberate, and
aoa-8rzo8, which found operator git configuration changing a measured answer.

## Context

AOA shells out to git in three places, and each one parses what git prints:

- the repository-root resolver in `aoa-path-trust` (`git rev-parse` for the top
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
`aoa_path_trust::git_free_of_inherited_state` is the only constructor for a git
`Command` in production code. It removes the 19 repository-local variables
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
handed its `safe.directory` values in a config file of their own.**
`policy infer-owners` and `gap mine-corpus` parse blame, tree and log output,
which machine config can reshape or break. They build their commands with
`commands::git::reading_repository_data`, which starts from the shared strip,
sets `GIT_CONFIG_NOSYSTEM=1`, and points `GIT_CONFIG_GLOBAL` at a private
temporary file that holds the operator's `safe.directory` values and nothing
else, in the order git read them. Global scope is one of git's protected
scopes, so git consults that file when it decides whether to trust the
repository.

The values are read once per process with
`git config --includes --null --show-scope --get-all safe.directory`, by a git
that still sees the operator's config (`GIT_CONFIG_GLOBAL`, `GIT_CONFIG_SYSTEM`,
`GIT_CONFIG_NOSYSTEM` and `HOME` are inherited); only `system` and `global`
scope entries are kept. That read runs in a fresh empty temporary directory
with `GIT_CEILING_DIRECTORIES` set to the directory's parent, so it discovers
no repository, whatever directory AOA was started in. This is the context git
itself has when it checks ownership: the check runs before a repository is
set up, so an `includeIf "gitdir:..."` or `includeIf "onbranch:..."` block
cannot match there, and a `safe.directory` granted inside one does not make
git trust anything. Measured on git 2.43.0 under
`GIT_TEST_ASSUME_DIFFERENT_OWNER=1`: with `safe.directory = *` reachable only
through `includeIf "gitdir:<A>/.git"`, `git -C <A> rev-parse HEAD` and
`git -C <B> rev-parse HEAD` both fail with the dubious-ownership message,
from a working directory inside A, inside B or outside both.

The one thing the empty directory changes is what a relative path means. Git
resolves a relative `GIT_CONFIG_GLOBAL` or `GIT_CONFIG_SYSTEM`, and the config
files it finds under a relative `HOME` or `XDG_CONFIG_HOME`, against its
working directory: the repository for plain `git -C <repo>`, the empty
directory for the trust read. A file inside the repository that withdraws
trust with an empty `safe.directory` would be read by plain git and missed by
the trust read, and AOA would read a repository git refuses. AOA does not
emulate that resolution. Before the trust read, `reading_repository_data`
refuses to run when any of those four variables is set to a non-empty relative
path, with an error that names the variable. An unset or empty variable is
left to git.

The refusal and the trust list are both computed by the first data-reading git
call and kept for the rest of the process (one `OnceLock` holds either the
rendered list or the error). A change to the environment after that call has
no effect: a config-locating variable made relative later is not refused, one
made absolute later is still refused, and a `safe.directory` added to the
operator's config later is not picked up. Each `aoa` command is one process
and does not change its own environment, so the first call sees what the
operator set.

With every config location absolute, reading the list with no repository in
view yields the set git's own check uses, for the repository AOA was started
in and for every other one, for every form of config this record measured.
The conditional include listed under "does not cover" below is the form it
did not measure.

The file is created with the `tempfile` crate (mode 0600 on Unix), written
once per git call and removed when that call returns. Git is handed the file
by path, so whoever can rename or unlink entries in the directory that holds
it can put a different file at that path before git opens it, and a global
config can name a program for git to run (`core.fsmonitor`). On Unix,
`reading_repository_data` therefore refuses to run when the temporary
directory (`TMPDIR`, or `/tmp`) is writable by group or other and does not
have the sticky bit, with an error that names the directory. The check runs
before the trust read and before every file is created; the trust read's
empty directory and the file are both created in the directory that was
checked. A sticky directory such as the usual `/tmp`, and a directory only
its owner can write, are accepted. On Windows there is no check: the mode
bits do not exist there, and AOA does not inspect the directory's access
control list.

A relative `TMPDIR` is accepted. `std::env::temp_dir` returns it as given, and
the `tempfile` crate joins a relative directory onto AOA's working directory
before it creates anything, so the path git is handed is absolute and names
the file AOA wrote, whichever directory `git -C <repo>` resolves paths
against. AOA does not make the path absolute itself and relies on the crate
for it (measured on `tempfile` 3.27.0); a CLI test fails if a relative
`TMPDIR` ever stops a trusted repository being read.

Each value is written as
a double-quoted config string with backslash, double quote, newline and tab
escaped, and is carried as bytes, so a value round-trips exactly whether it is
`*`, contains spaces, quotes or backslashes, or is not UTF-8. An empty value is
written as `""`: git treats an empty `safe.directory` as withdrawing every
value before it, and the file keeps both the entry and its position so that
still happens.

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
- `infer-owners` and `mine-corpus` run one extra `git config` per process, and
  write one small file per data-reading git call. If the read fails for a
  reason other than the key being unset, or the file cannot be created,
  written or removed, the command fails with that error instead of guessing an
  empty list.
- `infer-owners` and `mine-corpus` fail before running git when
  `GIT_CONFIG_GLOBAL`, `GIT_CONFIG_SYSTEM`, `HOME` or `XDG_CONFIG_HOME` is a
  relative path, and on Unix when the temporary directory is writable by group
  or other without the sticky bit. An operator in either position sets the
  variable to an absolute path, or points `TMPDIR` at a directory of their
  own. A directory made under a umask of 002 is mode 0775 and is refused,
  including where the group holds only its owner: AOA reads the mode and does
  not look up who is in the group. The refusal applies even where the variable
  could not have mattered (a relative `GIT_CONFIG_SYSTEM` under
  `GIT_CONFIG_NOSYSTEM=1`, a relative `HOME` beside an absolute
  `GIT_CONFIG_GLOBAL`), because deciding that would be the emulation the
  refusal exists to avoid.
- The length of the operator's trust list is not bounded by the operating
  system's argument limit. Nothing rides on the command line.

### What this deliberately does not cover

- **`GIT_REDIRECT_*` is proven on the built `Command` only.** The variables
  redirect git's standard streams on Windows and are ignored elsewhere. CI
  never runs Windows, so
  `crates/aoa-path-trust/tests/git_environment_inherited_redirects.rs` asserts that
  the constructed command removes them and no test observes a Windows git
  being unaffected (aoa-b3oj3 item 2).
- **The trust file is rewritten for every git call.** `infer-owners` blames
  each file with its own git process, so a machine with a very long trust list
  pays one write of that list per blamed file. The list is rendered once per
  process; only the write repeats. Nothing filters the list to the repository
  being read, because that would reimplement git's matching.
- **A replaceable ancestor of the temporary directory is not detected.** The
  check reads the mode of the temporary directory itself. If a directory above
  it is writable by another user without the sticky bit, that user can rename
  the whole temporary directory aside and put their own in its place, and the
  file git opens is theirs. Nothing walks the ancestors.
- **The temporary-directory check is a check, then a use.** The mode is read
  before the file is created, and a directory whose owner loosens it in
  between is not caught. Ownership of the directory is not examined either: a
  directory owned by another user who keeps it mode 0755 or sticky passes,
  and that owner can replace entries in it.
- **Windows has no temporary-directory check.** See the decision above.
- **An absolute config path that depends on the working directory is not
  refused.** `GIT_CONFIG_GLOBAL=/proc/self/cwd/<file>` is absolute, so the
  relative-path refusal passes it, yet it names a different file for each
  process: one in the empty directory for the trust read, one in the
  repository for plain `git -C <repo>`. Its only effect is that a file inside
  the repository cannot withdraw trust that the operator's machine config
  grants: plain git reads that file's empty `safe.directory` and refuses, and
  AOA reads the repository. A repository can never gain trust this way. The
  trust read resolves the path in a directory AOA created empty, so it never
  opens a file the repository supplies, and the list it hands the data readers
  is never wider than the machine config read with no repository in view.
  Measured on git 2.43.0 under `GIT_TEST_ASSUME_DIFFERENT_OWNER=1`: with the
  system config granting `*` and the repository's file withdrawing it, plain
  git refuses and `infer-owners` reads; with no machine config granting
  anything and the repository's file granting `*`, plain git reads and
  `infer-owners` refuses with git's dubious-ownership message. This
  is a ruling (aoa-8rzo8, 2026-10-05), not an oversight: the operator's own
  environment chose a config location that moves with the working directory,
  and the forms that do so (`/proc/self/cwd`, `/dev/fd`, bind mounts) have no
  closed list to refuse. No test pins this form.
- **A conditional include that needs no repository is evaluated once.** An
  `includeIf "hasconfig:remote.*.url:..."` block is matched against the
  operator's machine config alone. Whether git's ownership check matches it
  the same way was not measured, and no test pins that form.
- **A process killed mid-call leaves its trust file behind.** The file is
  removed when the git call returns. It holds a list of directory names and is
  readable only by its owner.
- **Repository-local config can still reshape output.** A `.git/config` that
  sets `blame.ignoreRevsFile` changes `infer-owners` for everyone who measures
  that checkout. That is the repository's statement about itself and is treated
  as input.

## Where this lives

- `crates/aoa-path-trust/src/git_environment.rs`,
  `fn git_free_of_inherited_state`: the shared strip.
- `crates/aoa-path-trust/src/root.rs`, `fn git_resolved_path`: the
  resolver, which inherits machine config and the ownership variable.
- `crates/aoa-audit/src/planes.rs`, `fn git`: the audit's builder.
- `crates/aoa/src/commands/git.rs`, `fn reading_repository_data`: the data
  readers' builder; `fn operator_trust_config`: the one read of the operator's
  `safe.directory`, outside any repository; `fn trust_config`: the file's
  contents; `fn refuse_relative_config_locations` and
  `fn refuse_replaceable_directory`: the two refusals.
- `crates/aoa-path-trust/tests/git_environment_repository_variables.rs` and
  `git_environment_inherited_redirects.rs`: every stripped name pinned on the
  built command.
- `crates/aoa/tests/cli_sections/audit_git_environment.rs` and
  `cli_git_environment.rs`: the audit and the data readers answer the same
  under each inherited variable.
- `crates/aoa/tests/cli_sections/cli_operator_git_config.rs`: `infer-owners`
  answers the same under a global, a system and a home-directory config that
  breaks `git blame`; a distrusted repository the operator's config marks safe
  is still read; one it does not mark safe is refused with git's message;
  started inside a trusted repository, it trusts that repository and another
  one exactly as plain git does, with the trust named outright, withdrawn by
  an empty value, and widened by a conditional include; a repository listed
  after fifty thousand others is read; a corrupt `.git` in the starting
  directory does not stop `--repo`; nothing is left in the temporary directory;
  a relative `GIT_CONFIG_GLOBAL` naming a file in the repository that withdraws
  trust is refused with the variable named and no owners printed; each of the
  four config-locating variables set relative is refused, and one set empty is
  not; a temporary directory writable by group or other without the sticky bit
  is refused by name with nothing created in it, while a sticky one and an
  owner-only one are accepted; started with a relative `TMPDIR`, a distrusted
  repository the operator's config marks safe is still read, with nothing left
  in that directory and nothing created in the repository. `mine-corpus`
  answers the same under a global, a system and a home-directory config whose
  `log.date` makes plain `git log` exit 128.
