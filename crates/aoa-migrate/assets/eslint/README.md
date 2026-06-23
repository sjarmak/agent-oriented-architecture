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
