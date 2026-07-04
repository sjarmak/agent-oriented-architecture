---
name: feature-capsule
description: How to add or extend a feature capsule in this repository.
---

# Feature capsule workflow

When adding a feature:

1. Create `features/<name>/` with `src/`, `tests/`, and a short `README.md`.
2. Keep the capsule self-contained; depend on other capsules through their
   public surface only.
3. Write the failing test first, then the implementation, in one change.

When extending a feature, re-read the whole capsule before editing and prefer
a smaller diff that reshapes existing code over bolting on a new branch.
