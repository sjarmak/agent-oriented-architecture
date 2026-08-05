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

# Exactly one verdict line, always. Metadata-sourced values reach this function
# verbatim, so control characters are folded to spaces here rather than at each
# call site: a gc.failure_reason containing a newline must not be able to emit a
# second stdout line that reads as a forged verdict.
verdict() {
  local rc=$1
  shift
  local message=$*
  printf '%s\n' "${message//[$'\001'-$'\037'$'\177']/ }"
  exit "$rc"
}

usage() {
  verdict 2 "ERROR usage: convoy-verify.sh {land <branch-or-commit> <base>|health <root-bead-id>|recovery <bead-id>}"
}

verify_land() {
  [ "$#" -eq 2 ] || usage
  local branch=$1 base=$2
  local -a paths

  git rev-parse --verify --quiet "${branch}^{commit}" >/dev/null ||
    verdict 2 "ERROR land: ref does not resolve: $branch"
  git rev-parse --verify --quiet "${base}^{commit}" >/dev/null ||
    verdict 2 "ERROR land: base does not resolve: $base"

  # Paths must come from the whole base..branch range, not the tip commit: a
  # multi-commit branch whose earlier commits touch other files would otherwise
  # be compared on the tip's paths alone and blessed patch-equivalent. The
  # range form also handles merge commits and root commits, which yield no
  # paths under diff-tree.
  # The path list goes through a temp file rather than a process substitution
  # or a command substitution, because both lose something load-bearing:
  # mapfile reports its own success and not the failure of a process
  # substitution (an orphan branch has no merge base), while command
  # substitution silently drops the NUL delimiters and collapses every path
  # into one. Either way the result is zero or one bogus path, a clean diff,
  # and a PASS — the exact false-pass shape this subcommand exists to catch.
  local list
  list=$(mktemp) || verdict 2 "ERROR land: cannot allocate scratch space"
  # shellcheck disable=SC2064  # expand $list now; verdict() exits from here
  trap "rm -f '$list'" EXIT
  git diff --name-only -z "${base}...${branch}" >"$list" ||
    verdict 2 "ERROR land: cannot diff $base...$branch (no merge base?)"
  mapfile -d '' -t paths <"$list"
  if [ "${#paths[@]}" -eq 0 ]; then
    verdict 0 "PASS land: $branch introduces no changes relative to $base"
  fi

  git diff --quiet "$branch" "$base" -- "${paths[@]}"
  case $? in
    0) verdict 0 "PASS land: $branch is patch-equivalent to $base" ;;
    1) verdict 1 "FAIL land: $branch differs from $base on its changed paths" ;;
    *) verdict 2 "ERROR land: cannot compare $branch to $base" ;;
  esac
}

verify_health() {
  [ "$#" -eq 1 ] || usage
  local root=$1 rows failure anomalous active_count active_assignee seat peek_json peek_output

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
  ' <<<"$rows") || verdict 2 "ERROR health: could not evaluate step outcomes for $root"
  [ -z "$failure" ] || verdict 1 "NOT HEALTHY $root: $failure"

  failure=$(jq -r '
    [.[]
      | select(
          .status == "closed"
          and .metadata["gc.coordinator_outcome.producer_disposition"] != null
        )
      | (try (.metadata["gc.coordinator_outcome.producer_disposition"] | fromjson) catch null) as $disp
      | select($disp.disposition != "deliverable")
      | "\(.id) producer disposition=\($disp.disposition // "invalid")"]
    | first // empty
  ' <<<"$rows") || verdict 2 "ERROR health: could not evaluate producer dispositions for $root"
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
  ' <<<"$rows") || verdict 2 "ERROR health: could not evaluate closure evidence for $root"
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

  # Liveness comes from the session registry, not from scraped TUI text: the
  # spinner verb is randomized ("Working", "Cogitating", ...), so grepping for
  # any one of them calls a demonstrably live seat dead.
  seat=$(gc session list --json 2>/dev/null | jq -c --arg seat "$active_assignee" '
    [.sessions[]? | select(
       .name == $seat or .agent_name == $seat or .alias == $seat
       or .session_name == $seat or .id == $seat)]
    | first // empty
  ') || verdict 2 "ERROR health: could not read session registry for $root"
  [ -n "$seat" ] ||
    verdict 1 "NOT HEALTHY $root: no session for seat $active_assignee"
  jq -e '.closed != true and .state == "active"' <<<"$seat" >/dev/null ||
    verdict 1 "NOT HEALTHY $root: seat $active_assignee is not active"

  # The provider wall is invisible to the registry — a walled seat still reads
  # as active — so it stays a scrape.
  peek_json=$(gc session peek "$active_assignee" --json --lines 20 2>/dev/null) ||
    verdict 1 "NOT HEALTHY $root: cannot inspect seat $active_assignee"
  peek_output=$(jq -r '.output // empty' <<<"$peek_json") ||
    verdict 1 "NOT HEALTHY $root: invalid seat output for $active_assignee"

  if grep -Eiq "usage limit|try again at|unauthorized|forbidden|authentication (failed|error)|auth (failed|error)|rate limit" <<<"$peek_output"; then
    verdict 1 "NOT HEALTHY $root: provider wall on seat $active_assignee"
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
  [ -d "$work_dir" ] ||
    verdict 2 "ERROR recovery: no readable worktree for $bead_id"
  # Every ref in this function belongs to the bead's worktree, so all three
  # git reads are anchored there rather than to the caller's cwd.
  branch=$(git -C "$work_dir" symbolic-ref --quiet --short HEAD 2>/dev/null) ||
    verdict 2 "ERROR recovery: worktree for $bead_id is detached"
  git -C "$work_dir" rev-parse --verify --quiet "${base}^{commit}" >/dev/null ||
    verdict 2 "ERROR recovery: base does not resolve: $base"

  git -C "$work_dir" merge-base --is-ancestor "$base" "$branch"
  case $? in
    0) verdict 0 "LAND-ONLY recovery: $branch already contains $base; do not re-resolve" ;;
    1) verdict 1 "NOT-FF-ABLE recovery: $branch does not contain $base" ;;
    *) verdict 2 "ERROR recovery: cannot compare $branch to $base" ;;
  esac
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
