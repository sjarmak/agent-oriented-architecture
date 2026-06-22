// Hermetic, single-rule ESLint flat config for the AOA TypeScript/JS dead-import
// adapter. Enables EXACTLY ONE rule — `unused-imports/no-unused-imports` — which
// deletes only unused import specifiers/declarations (never unused locals, never
// reordering, never additions). This file is the load-bearing construct-validity
// artifact: it is OUR assertion that the single lint class is "unused import".
// Its content is fingerprinted into the fix provenance. Do not add rules here.
//
// The plugin and parser resolve via Node module resolution relative to THIS file
// (the adjacent node_modules/), so the target repo's node_modules is never used.
// The adapter runs ESLint with --no-config-lookup so this config fully replaces
// any eslint.config.* in the repo under analysis.

import tsParser from "@typescript-eslint/parser";
import unusedImports from "eslint-plugin-unused-imports";

export default [
  {
    files: ["**/*.{js,jsx,mjs,cjs,ts,tsx}"],
    languageOptions: {
      parser: tsParser,
      ecmaVersion: "latest",
      sourceType: "module",
      parserOptions: {
        ecmaFeatures: { jsx: true },
      },
    },
    plugins: {
      "unused-imports": unusedImports,
    },
    rules: {
      "unused-imports/no-unused-imports": "error",
    },
  },
];
