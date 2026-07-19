// Contract tests for the architecture drift guard (check-links.mjs):
// valid relative and external links pass; a dead relative link exits
// non-zero so the Pages workflow cannot publish a stale model.

import { test } from "node:test";
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdtempSync, mkdirSync, writeFileSync, rmSync } from "node:fs";
import { join, dirname } from "node:path";
import { tmpdir } from "node:os";
import { fileURLToPath } from "node:url";

const script = join(dirname(fileURLToPath(import.meta.url)), "check-links.mjs");

// Writes the given relative-path → content map into a temp dir, runs the
// guard with that dir as cwd (the script walks "."), and returns the result.
function runInFixture(files) {
  const dir = mkdtempSync(join(tmpdir(), "check-links-"));
  try {
    for (const [rel, content] of Object.entries(files)) {
      const p = join(dir, rel);
      mkdirSync(dirname(p), { recursive: true });
      writeFileSync(p, content);
    }
    return spawnSync(process.execPath, [script], { cwd: dir, encoding: "utf8" });
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}

test("valid relative, external, and anchor links pass", () => {
  const res = runInFixture({
    "architecture/model.c4": [
      "link ../src/lib.rs 'impl'",
      "link ./notes.md",
      "link https://example.com 'site'",
      "link mailto:someone@example.com",
      "link #anchor",
    ].join("\n"),
    "architecture/notes.md": "notes",
    "src/lib.rs": "// code",
  });
  assert.equal(res.status, 0, `expected exit 0, got ${res.status}: ${res.stdout}${res.stderr}`);
});

test("a dead relative link exits non-zero and names the link", () => {
  const res = runInFixture({
    "architecture/model.c4": "link ../src/gone.rs 'moved away'",
  });
  assert.notEqual(res.status, 0, `expected non-zero exit: ${res.stdout}${res.stderr}`);
  assert.match(res.stdout, /gone\.rs/);
});
