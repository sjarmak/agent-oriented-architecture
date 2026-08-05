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
# evaluated; exit 3 means the evidence was inconclusive (UNKNOWN). UNKNOWN is
# deliberately distinct from both 1 and 2: collapsing it into NOT HEALTHY
# manufactures the false failure premise this script exists to prevent, and
# collapsing it into HEALTHY accepts absence of evidence as evidence.
# The script never mutates beads, sessions, or git state.

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

# Sets scratch_dir. Like patch_state this reports through a global rather than
# stdout: called through a command substitution, the mktemp and its cleanup
# trap would both belong to the subshell, which deletes the directory the
# caller is about to write to.
scratch_dir=
scratch() {
  [ -z "$scratch_dir" ] || return 0
  scratch_dir=$(mktemp -d) || return 1
  # shellcheck disable=SC2064  # expand now; verdict() exits from anywhere
  trap "rm -rf '$scratch_dir'" EXIT
}

# Patch-equivalence, not SHA containment (aoa-el8m1). Sets patch_result to one
# of "empty" (branch introduces no changes), "equal", or "differ"; on failure
# returns non-zero and leaves an operator-readable reason in patch_error.
# Both results come back through globals rather than stdout, because a command
# substitution would run this in a subshell: the scratch-dir EXIT trap would
# fire on the subshell and delete the path list mid-read, and patch_error would
# never reach the caller.
patch_error=
patch_result=
patch_state() {
  local repo=$1 branch=$2 base=$3
  local -a paths
  local list

  patch_error=
  git -C "$repo" rev-parse --verify --quiet "${branch}^{commit}" >/dev/null || {
    patch_error="ref does not resolve: $branch"
    return 1
  }
  git -C "$repo" rev-parse --verify --quiet "${base}^{commit}" >/dev/null || {
    patch_error="base does not resolve: $base"
    return 1
  }

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
  if ! scratch; then
    patch_error="cannot allocate scratch space"
    return 1
  fi
  list=$scratch_dir/paths
  if ! : >"$list"; then
    patch_error="cannot allocate scratch space"
    return 1
  fi
  git -C "$repo" diff --name-only -z "${base}...${branch}" >"$list" || {
    patch_error="cannot diff $base...$branch (no merge base?)"
    return 1
  }
  mapfile -d '' -t paths <"$list"
  if [ "${#paths[@]}" -eq 0 ]; then
    patch_result=empty
    return 0
  fi

  git -C "$repo" diff --quiet "$branch" "$base" -- "${paths[@]}"
  case $? in
    0) patch_result=equal ;;
    1) patch_result=differ ;;
    *)
      patch_error="cannot compare $branch to $base"
      return 1
      ;;
  esac
}

verify_land() {
  [ "$#" -eq 2 ] || usage
  local branch=$1 base=$2

  patch_state . "$branch" "$base" ||
    verdict 2 "ERROR land: $patch_error"
  case $patch_result in
    empty) verdict 0 "PASS land: $branch introduces no changes relative to $base" ;;
    equal) verdict 0 "PASS land: $branch is patch-equivalent to $base" ;;
    *) verdict 1 "FAIL land: $branch differs from $base on its changed paths" ;;
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

  # Two closure-stamp schemes coexist in this store and neither is dead: 201 of
  # 273 closed steps carry gc.outcome only, 42 carry a producer disposition
  # only (every step of a modern convoy), 11 carry neither. A check reading one
  # scheme is blind to the other, so leg A reads BOTH and reconciles them:
  # passing under either scheme is passing, failing under either is failing,
  # and a step the two disagree about is NOT HEALTHY with both raw values
  # printed rather than letting one silently win. Only a step stamped under
  # neither scheme is genuinely unstamped — that, and only that, is the
  # "suspicious, check the artifact" case (aoa-73s9r).
  failure=$(jq -r '
    def disp: (try (.metadata["gc.coordinator_outcome.producer_disposition"] | fromjson) catch null);
    def outcome: .metadata["gc.outcome"];
    [.[]
      | select(.status == "closed")
      | . as $step
      | (if outcome == null then null elif outcome == "pass" then true else false end) as $by_outcome
      | (if disp == null then null elif (disp | type) != "object" then false
         elif disp.disposition == "deliverable" then true else false end) as $by_disp
      | if $by_outcome != null and $by_disp != null and $by_outcome != $by_disp then
          "\($step.id) stamp schemes disagree: gc.outcome=\(outcome|tostring) producer_disposition=\(disp.disposition // "invalid"|tostring)"
        elif $by_outcome == false then
          "\($step.id) \($step.metadata["gc.failure_reason"] // ("outcome=" + (outcome|tostring)))"
        elif $by_disp == false then
          "\($step.id) producer disposition=\((disp.disposition // "invalid")|tostring)"
        else
          empty
        end]
    | first // empty
  ' <<<"$rows") || verdict 2 "ERROR health: could not evaluate step outcomes for $root"
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
    verdict 1 "NOT HEALTHY $root: $anomalous closed under neither stamp scheme"

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

  # Leg B. The provider wall is invisible to the registry — a walled seat still
  # reads as active — so seat liveness stays a scrape, but it is a POSITIVE
  # test: the pane must show work in flight. Absence of pane evidence is not
  # evidence of death. An empty peek is routine here (measured 5 of 8 polls on
  # a continuously-live seat, and it is also what a seat mid-rotation returns),
  # so an empty pane re-polls and then reports UNKNOWN rather than either
  # verdict. Leg B is a veto: leg A passing can never on its own return HEALTHY.
  local attempt idle_seen=0
  for attempt in 1 2 3; do
    [ "$attempt" -eq 1 ] || sleep "${CONVOY_VERIFY_PEEK_INTERVAL:-5}"

    peek_json=$(gc session peek "$active_assignee" --json --lines 20 2>/dev/null) || continue
    peek_output=$(jq -r '.output // empty' <<<"$peek_json" 2>/dev/null) || continue
    [ -n "$peek_output" ] || continue

    if grep -Eiq "usage limit|try again at|unauthorized|forbidden|authentication (failed|error)|auth (failed|error)|rate limit" <<<"$peek_output"; then
      verdict 1 "NOT HEALTHY $root: provider wall on seat $active_assignee"
    fi

    # Positive tells only, and never a spinner-verb whitelist: the verb is
    # randomized and includes past-tense forms ("Churned for"), so eleven
    # samples across two observers found zero occurrences of any fixed word.
    # What is stable is the turn-in-flight decoration — an elapsed timer, the
    # interrupt hint, or a live token counter — alongside the status bar.
    if grep -Eq "esc to interrupt|[0-9]+s ·|[0-9.]+k tokens" <<<"$peek_output" &&
      grep -Eq "ctx: [0-9]+%|esc to interrupt" <<<"$peek_output"; then
      verdict 0 "HEALTHY $root: passing bead evidence and work in flight on seat $active_assignee"
    fi

    # A rendered pane with no turn in flight is SUSPECT, not dead and not
    # healthy: it is what an idle seat and a stalled seat both look like.
    grep -Eq "ctx: [0-9]+%" <<<"$peek_output" && idle_seen=1
  done

  [ "$idle_seen" -eq 0 ] ||
    verdict 3 "UNKNOWN $root: seat $active_assignee rendered but showed no work in flight across 3 polls"
  verdict 3 "UNKNOWN $root: no pane evidence from seat $active_assignee across 3 polls"
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
    1) : ;;
    *) verdict 2 "ERROR recovery: cannot compare $branch to $base" ;;
  esac

  # Not fast-forwardable is not yet grounds to dispatch. gc.needs_recovery and
  # gc.failure_reason are stamped at failure time and are never cleared when a
  # re-dispatch lands the same work under a DIFFERENT bead id, so they are not
  # consulted here at all. The sound test is whether the base already carries
  # this branch's commits by patch id: git cherry marks a commit "-" when an
  # equivalent patch is already upstream and "+" when it is not. A branch with
  # no "+" commits is done-in-fact and recovery must refuse regardless of any
  # stamp (aoa-dnmtk sat needs_recovery for ~3.5h after its only commit had
  # landed under another bead id as an equivalent patch).
  # Tree comparison is not enough here: main advancing on the same file after
  # the land makes the trees differ while the branch's own work is still
  # upstream, which reads as unlanded work and dispatches a recovery.
  local cherry unlanded
  cherry=$(git -C "$work_dir" cherry "$base" "$branch" 2>/dev/null) ||
    verdict 2 "ERROR recovery: cannot compare $branch to $base by patch id"
  # grep -c exits 1 on a zero count, which is the ALREADY-LANDED case, so the
  # count is validated rather than the exit status.
  unlanded=$(grep -c '^+' <<<"$cherry")
  [[ $unlanded =~ ^[0-9]+$ ]] ||
    verdict 2 "ERROR recovery: cannot count unlanded commits on $branch"
  if [ "$unlanded" -eq 0 ]; then
    verdict 0 "ALREADY-LANDED recovery: every commit on $branch is already in $base; do not dispatch"
  fi
  verdict 1 "NOT-FF-ABLE recovery: $branch does not contain $base and has $unlanded commit(s) not in $base"
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
