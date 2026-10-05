# Changelog

Changes an operator can observe, newest first.

## Unreleased

### Changed

- `aoa lint` applies ignore rules by distance: when `.ignore` or `.gitignore`
  files in different directories both have a rule matching a path, the file in
  the directory nearest that path decides. Before, an `.ignore` rule higher up
  overrode a `.gitignore` rule lower down, so a context file re-included by a
  nested `.gitignore` could still be skipped. Within one directory `.ignore`
  still overrides `.gitignore`. Ignore files above the linted directory are no
  longer read at all.
- `aoa lint` refuses an `.ignore` or `.gitignore` in the linted tree that has a
  second hard link, because its contents can be rewritten from outside the
  tree. A checkout produced by a tool that hard-links files will need those
  two files copied instead.
- `aoa_budget::fix_oversized` (a library entry point; no CLI command calls it
  yet) checks that it can write both the context file and an existing archive
  before changing either, and a rewritten file keeps its own mode, owner and
  (on Linux) `user.*` attributes and POSIX ACLs. A read-only context file or
  archive is now refused rather than replaced, and so is a context file whose
  link chain passes through the name its archive would take.
- `aoa audit` counts a runtime hook entry only when its `type` is `command`,
  reports the runtime plane as missing when `disableAllHooks` is set in
  `.claude/settings.json` or `.claude/settings.local.json`, and reports an
  enforce hook that also runs under a matcher outside its scope, which
  `aoa observe --enforce` already refused to install over.
- `aoa audit` and `aoa observe --enforce` refuse an enforce hook entry that
  sets `async` or `asyncRewake`. The host does not wait for such a hook, so it
  cannot block a write. The audit reports the runtime plane as missing when an
  enforce hook runs only that way and names the hook as a defect; the installer
  stops with an error instead of adding a second entry beside it.
- `aoa audit` stops waiting for git at its deadline even when git, or a process
  holding git's output open, never stops writing.
