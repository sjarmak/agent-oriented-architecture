#!/usr/bin/env bash
# Read-only verification for convoy landing, health, and recovery premises.
#
# Usage:
#   convoy-verify.sh land <branch-or-commit> <base>
#   convoy-verify.sh health <root-bead-id>
#   convoy-verify.sh recovery <bead-id>
#
# Every invocation emits exactly one verdict line. Exit 0 means the named
# predicate passed; exit 1 means it was falsified; exit 2 means it could not be
# evaluated. The script never mutates beads, sessions, or git state.

set -uo pipefail

verdict() {
  local rc=$1
  shift
  printf '%s\n' "$*"
  exit "$rc"
}

usage() {
  verdict 2 "ERROR usage: convoy-verify.sh {land <branch-or-commit> <base>|health <root-bead-id>|recovery <bead-id>}"
}

verify_land() {
  [ "$#" -eq 2 ] || usage
  local branch=$1 base=$2
  local -a paths=()

  git rev-parse --verify --quiet "${branch}^{commit}" >/dev/null ||
    verdict 2 "ERROR land: ref does not resolve: $branch"
  git rev-parse --verify --quiet "${base}^{commit}" >/dev/null ||
    verdict 2 "ERROR land: base does not resolve: $base"

  mapfile -d '' -t paths < <(git diff-tree --no-commit-id --name-only -r -z "$branch")
  [ "${#paths[@]}" -gt 0 ] || verdict 2 "ERROR land: $branch has no changed paths"

  if git diff --quiet "$branch" "$base" -- "${paths[@]}"; then
    verdict 0 "PASS land: $branch is patch-equivalent to $base"
  fi
  verdict 1 "FAIL land: $branch differs from $base on its changed paths"
}

verify_health() {
  [ "$#" -eq 1 ] || usage
  local root=$1 rows failure anomalous active_count active_assignee peek_json peek_output

  rows=$(timeout 90 bd list --status all --json 2>/dev/null | jq -c --arg root "$root" '
    [.[] | select(.metadata["gc.root_bead_id"] == $root and .metadata["gc.step_id"] != null)]
  ') || verdict 2 "ERROR health: could not read bead state for $root"
  [ "$(jq 'length' <<<"$rows")" -gt 0 ] ||
    verdict 2 "ERROR health: no convoy steps found for $root"

  failure=$(jq -r '
    [.[]
      | select(
          .status == "closed"
          and .metadata["gc.outcome"] != null
          and .metadata["gc.outcome"] != "pass"
        )
      | "\(.id) \(.metadata["gc.failure_reason"] // ("outcome=" + .metadata["gc.outcome"]))"]
    | first // empty
  ' <<<"$rows")
  [ -z "$failure" ] || verdict 1 "NOT HEALTHY $root: $failure"

  failure=$(jq -r '
    [.[]
      | select(
          .status == "closed"
          and .metadata["gc.coordinator_outcome.producer_disposition"] != null
        )
      | . as $step
      | (try ($step.metadata["gc.coordinator_outcome.producer_disposition"] | fromjson) catch null) as $disp
      | select($disp == null or $disp.disposition != "deliverable")
      | "\($step.id) producer disposition=\($disp.disposition // "invalid")"]
    | first // empty
  ' <<<"$rows")
  [ -z "$failure" ] || verdict 1 "NOT HEALTHY $root: $failure"

  anomalous=$(jq -r '
    [.[]
      | select(
          .status == "closed"
          and .metadata["gc.outcome"] == null
          and .metadata["gc.coordinator_outcome.producer_disposition"] == null
        )
      | .id]
    | first // empty
  ' <<<"$rows")
  [ -z "$anomalous" ] ||
    verdict 1 "NOT HEALTHY $root: $anomalous closed without outcome evidence"

  active_count=$(jq '[.[] | select(.status == "in_progress")] | length' <<<"$rows")
  if [ "$active_count" -eq 0 ]; then
    if jq -e 'all(.[]; .status == "closed")' <<<"$rows" >/dev/null; then
      verdict 0 "HEALTHY $root: all steps closed with passing outcome evidence"
    fi
    verdict 1 "NOT HEALTHY $root: no step is advancing"
  fi
  [ "$active_count" -eq 1 ] ||
    verdict 1 "NOT HEALTHY $root: $active_count steps are in progress"

  active_assignee=$(jq -r '[.[] | select(.status == "in_progress")][0].assignee // empty' <<<"$rows")
  [ -n "$active_assignee" ] ||
    verdict 1 "NOT HEALTHY $root: in-progress step has no assignee"

  peek_json=$(gc session peek "$active_assignee" --json --lines 20 2>/dev/null) ||
    verdict 1 "NOT HEALTHY $root: cannot inspect seat $active_assignee"
  peek_output=$(jq -r '.output // empty' <<<"$peek_json") ||
    verdict 1 "NOT HEALTHY $root: invalid seat output for $active_assignee"

  if grep -Eiq "usage limit|try again at|unauthorized|forbidden|authentication (failed|error)|auth (failed|error)|rate limit" <<<"$peek_output"; then
    verdict 1 "NOT HEALTHY $root: provider wall on seat $active_assignee"
  fi
  if ! grep -Eq '(^|[[:space:]])Working([[:space:](]|$)' <<<"$peek_output"; then
    verdict 1 "NOT HEALTHY $root: no work in flight on seat $active_assignee"
  fi

  verdict 0 "HEALTHY $root: passing bead evidence and active seat $active_assignee"
}

verify_recovery() {
  [ "$#" -eq 1 ] || usage
  local bead_id=$1 bead work_dir branch base

  bead=$(bd show "$bead_id" --json 2>/dev/null | jq -c '.[0] // empty') ||
    verdict 2 "ERROR recovery: could not read bead $bead_id"
  [ -n "$bead" ] || verdict 2 "ERROR recovery: bead not found: $bead_id"

  work_dir=$(jq -r '.metadata["gc.work_dir"] // .metadata.work_dir // empty' <<<"$bead")
  base=$(jq -r '.metadata["gc.base_ref"] // .metadata.base_branch // "main"' <<<"$bead")
  if [ -z "$work_dir" ] || [ ! -d "$work_dir" ]; then
    verdict 2 "ERROR recovery: no readable worktree for $bead_id"
  fi
  branch=$(git -C "$work_dir" symbolic-ref --quiet --short HEAD 2>/dev/null) ||
    verdict 2 "ERROR recovery: worktree for $bead_id is detached"
  git rev-parse --verify --quiet "${base}^{commit}" >/dev/null ||
    verdict 2 "ERROR recovery: base does not resolve: $base"

  if git merge-base --is-ancestor "$base" "$branch" 2>/dev/null; then
    verdict 0 "LAND-ONLY recovery: $branch already contains $base; do not re-resolve"
  fi
  verdict 1 "NOT-FF-ABLE recovery: $branch does not contain $base"
}

[ "$#" -gt 0 ] || usage
command=$1
shift
case "$command" in
  land) verify_land "$@" ;;
  health) verify_health "$@" ;;
  recovery) verify_recovery "$@" ;;
  *) usage ;;
esac
