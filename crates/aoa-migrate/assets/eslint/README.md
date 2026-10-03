# Vendored ESLint toolchain (pinned, regenerated locally)

This directory is the pinned, hermetic ESLint toolchain for the TypeScript/JS
dead-import adapter (`crates/aoa-migrate/src/imports/typescript.rs`).
`package.json` and `package-lock.json` are committed and are the source of
truth; the `node_modules/` install itself is gitignored.

## Setup

Install the pinned toolchain before running the TypeScript adapter or its tests:

```bash
npm ci   # in this directory; installs node_modules/ exactly from package-lock.json
```

`npm ci` is reproducible by construction: it installs the locked versions and
fails if `package.json` and `package-lock.json` disagree. The adapter checks for
the install and emits a loud `ToolchainUnavailable` (with this command) when it is
missing, so a fresh checkout never produces a silently empty migration plan.

## Why vendored

The only import-scoped, auto-fixable ESLint rule (`unused-imports/no-unused-imports`)
lives in a community plugin, not ESLint core. To keep the dead-import treatment a
construct-valid R0 arm, the analyzer must be:

- **pinned** — reproducibility is anchored by exact tool versions, recorded in the
  fix provenance (see `FixProvenance::toolchain`);
- **hermetic** — the target repo's own ESLint config, plugins, and `node_modules`
  must never influence the result (the adapter runs with `--config <this>/eslint.config.mjs
  --no-config-lookup --no-inline-config --no-ignore`).

`node` must be on `PATH`; ESLint itself is supplied by `node_modules/` here. The
plugin and parser resolve via Node module resolution relative to `eslint.config.mjs`.

## Cost and memory ceiling

The adapter lints every `.js`, `.jsx`, `.mjs`, `.cjs`, `.ts` and `.tsx` file in
the isolated copy, 500 files per `node` process (`ESLINT_BATCH_FILES` in
`typescript.rs`), one process at a time. Each batch is a single
`eslint --fix --format json` pass that both removes the unused imports and
reports parse failures, so no file is linted twice.

Memory is bounded two ways:

- Every `node` process runs with `--max-old-space-size=1024` (`NODE_HEAP_CAP`),
  so its JavaScript heap cannot pass 1 GiB however large the repo is. A batch
  that needs more makes `node` abort, which the adapter reports as a loud
  `BuildFailed` carrying node's out-of-memory message, never as an empty plan.
- `aoa` holds the before-image of one batch (at most 500 files) while that batch
  runs, not of the whole tree.

The ceiling is therefore one capped `node` process plus one batch of source
text, independent of repo size. On a synthetic tree of 6,000 TypeScript files
(71 MB) a zero-change plan peaked at about 250 MB resident and took 47 s; the
earlier two-pass, single-process, uncapped run took 494 MB and 69 s on the same
tree.

Wall time still scales with the repo: the engine copies the tree (minus build
output, `node_modules` and VCS metadata) to a temporary directory before
linting, and each batch pays one `node` start. A plan that changes nothing
reports how many files it examined in `fix_reports` (`--json`) and on the
`[fix:dead-imports-typescript]` line of the human output.

## Pins

- `eslint` 9.39.4
- `@typescript-eslint/parser` 8.46.0 (TS/TSX syntax; no type-info needed)
- `eslint-plugin-unused-imports` 4.4.1

To bump a pin: edit `package.json`, run `npm install --omit=dev` in this directory
to refresh `package-lock.json`, commit the lockfile, then re-run
`cargo test -p aoa-migrate --test imports_typescript`.

## Construct-validity disclosure

Unlike ruff's vendor-defined `F401`, the "exactly one lint class = unused-import"
binding here is **our** assertion: the choice of the plugin's `no-unused-imports`
rule plus the single-rule `eslint.config.mjs`. That config is fingerprinted into
provenance so the assertion is auditable. Do not add rules to `eslint.config.mjs`.
