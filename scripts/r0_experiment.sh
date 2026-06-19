#!/usr/bin/env bash
# r0_experiment.sh — post-process a codeprobe R0 experiment into the AOA
# falsification verdict.
#
# This chains the AOA half of the R0 pipeline (see docs/r0_runbook.md). It
# assumes the codeprobe experiment has already been run and aggregated:
#
#   aoa eval experiment --manifest M --tasks T --out OUT/falsify_input.json
#   aoa falsify --repos OUT/falsify_input.json \
#               --build-meta OUT/falsify_input.build.json \
#               [--bias-warnings reports/aggregate.json] \
#               --out OUT/falsification.json
#
# The falsification verdict (proceed | pivot | inconclusive) is the Wave-1 gate.
# Exit code mirrors `aoa falsify`: 0 for a real gate verdict, non-zero when a
# precondition (too_few_repos / convention_inputs_degraded) blocked it.

set -euo pipefail

usage() {
  cat <<'USAGE'
Usage: r0_experiment.sh --manifest <file> --tasks <dir> --out <dir> [--aggregate <file>] [--aoa <bin>]

  --manifest   build manifest JSON (per-repo arms + eligibility); see docs/r0_runbook.md
  --tasks      codeprobe task-source dir (shared oracle across arms)
  --out        output dir for falsify_input.json, *.build.json, falsification.json
  --aggregate  codeprobe reports/aggregate.json to surface bias warnings (optional)
  --aoa        aoa binary to invoke (default: aoa on PATH)
USAGE
}

MANIFEST="" TASKS="" OUT="" AGGREGATE="" AOA="${AOA:-aoa}"

while [ $# -gt 0 ]; do
  case "$1" in
    --manifest)  MANIFEST="${2:?--manifest needs a value}"; shift 2 ;;
    --tasks)     TASKS="${2:?--tasks needs a value}"; shift 2 ;;
    --out)       OUT="${2:?--out needs a value}"; shift 2 ;;
    --aggregate) AGGREGATE="${2:?--aggregate needs a value}"; shift 2 ;;
    --aoa)       AOA="${2:?--aoa needs a value}"; shift 2 ;;
    -h|--help)   usage; exit 0 ;;
    *) echo "unknown argument: $1" >&2; usage >&2; exit 2 ;;
  esac
done

for req in MANIFEST TASKS OUT; do
  if [ -z "${!req}" ]; then
    echo "missing required --${req,,}" >&2
    usage >&2
    exit 2
  fi
done

mkdir -p "$OUT"
INPUT="$OUT/falsify_input.json"
BUILD_META="$OUT/falsify_input.build.json"
FALSIFICATION="$OUT/falsification.json"

echo "[r0] building FalsifyInput from experiment arms ..."
"$AOA" eval experiment --manifest "$MANIFEST" --tasks "$TASKS" --out "$INPUT"

echo "[r0] running the R0/R0' falsification gate ..."
falsify_args=(falsify --repos "$INPUT" --build-meta "$BUILD_META" --out "$FALSIFICATION")
if [ -n "$AGGREGATE" ]; then
  falsify_args+=(--bias-warnings "$AGGREGATE")
fi

# `aoa falsify` exits non-zero on a precondition-blocked (non-usable) verdict;
# surface that to the caller while still reporting where the artifact landed.
set +e
"$AOA" "${falsify_args[@]}"
code=$?
set -e

echo "[r0] verdict written to $FALSIFICATION (aoa falsify exit=$code)"
exit "$code"
