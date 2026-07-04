# Feature capsules

One directory per feature. A capsule keeps everything that changes together
in one place:

- `src/` — the implementation
- `tests/` — the capsule's tests
- `README.md` — a few lines: what the feature does and its invariants

Add a feature by adding a capsule directory, not by growing a shared module.
Code shared across capsules moves into a named shared capsule only once three
capsules need it.
