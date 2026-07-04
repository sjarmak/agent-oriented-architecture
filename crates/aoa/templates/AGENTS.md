# {{project_name}}

Agent context for this repository. Keep this file capped: short, factual,
and linked out instead of inlined. Run `aoa lint-context` before committing
changes to it.

## Layout

Code is organized as feature capsules under `features/` — one directory per
feature, holding its source, tests, and docs together. The convention lives
in [features/README.md](features/README.md).

## Working here

- Ship the test in the same change as the source it covers.
- Prefer editing an existing capsule in place over adding parallel variants.
- Adding a feature? Follow the
  [feature-capsule skill](.claude/skills/feature-capsule/SKILL.md).

## Commands

Fill these in for your stack (one command each, no prose):

- build: `<build command>`
- test: `<test command>`

## Policy

Operator policy (protected paths, generated artifacts, review gates) is
declared in `aoa-policy.yaml` and compiled with `aoa policy compile`.
