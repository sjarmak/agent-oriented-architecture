#!/usr/bin/env bash
# Executable acceptance tests for convoy-verify.sh.
#
# Usage: .gc/scripts/convoy-verify.test.sh
#
# Every case is hermetic: git fixtures are built in a temp dir, and the bead
# store and session registry are stubbed on PATH so the health leg is testable
# without a live convoy (and without any store write). Each case asserts BOTH
# the exit code and that stdout is exactly one verdict line — the one-line
# contract is itself under test, because a metadata value carrying a newline
# was able to forge a second verdict line.

set -uo pipefail

script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
subject=$script_dir/convoy-verify.sh
[ -x "$subject" ] || {
  echo "FATAL: $subject is not executable" >&2
  exit 2
}

root=$(mktemp -d) || exit 2
trap 'rm -rf "$root"' EXIT
stub_bin=$root/bin
mkdir -p "$stub_bin"

export CONVOY_VERIFY_PEEK_INTERVAL=0
export PATH=$stub_bin:$PATH
export GIT_AUTHOR_NAME=test GIT_AUTHOR_EMAIL=test@example.invalid
export GIT_COMMITTER_NAME=test GIT_COMMITTER_EMAIL=test@example.invalid

pass_count=0
fail_count=0

# assert <name> <expected-exit> <expected-verdict-substring> -- <command...>
assert() {
  local name=$1 want_rc=$2 want_text=$3
  shift 4 # name, rc, text, and the literal --
  local out rc lines
  out=$("$@" 2>/dev/null)
  rc=$?
  lines=$(printf '%s\n' "$out" | grep -c '' )

  if [ "$rc" -ne "$want_rc" ]; then
    printf 'FAIL %s: exit %s, want %s (output: %s)\n' "$name" "$rc" "$want_rc" "$out"
    fail_count=$((fail_count + 1))
    return
  fi
  if [ "$lines" -ne 1 ]; then
    printf 'FAIL %s: emitted %s stdout lines, want exactly 1:\n%s\n' "$name" "$lines" "$out"
    fail_count=$((fail_count + 1))
    return
  fi
  case $out in
    *"$want_text"*) ;;
    *)
      printf 'FAIL %s: verdict %q does not contain %q\n' "$name" "$out" "$want_text"
      fail_count=$((fail_count + 1))
      return
      ;;
  esac
  printf 'ok   %s\n' "$name"
  pass_count=$((pass_count + 1))
}

commit_file() {
  local repo=$1 path=$2 content=$3 message=$4
  mkdir -p -- "$(dirname -- "$repo/$path")"
  printf '%s\n' "$content" >"$repo/$path"
  git -C "$repo" add -- "$path"
  git -C "$repo" commit -q -m "$message"
}

new_repo() {
  local repo=$1
  mkdir -p "$repo"
  git -C "$repo" init -q -b main
  commit_file "$repo" base.txt seed "seed"
}

# ---------------------------------------------------------------- land cases

repo=$root/land
new_repo "$repo"

# A branch that lands nothing relative to main.
git -C "$repo" branch work/empty main

# A genuinely divergent branch.
git -C "$repo" checkout -q -b work/divergent main
commit_file "$repo" base.txt changed "divergent change"

# THE FALSE-PASS REPRO (rejection defect 1): two commits, the tip touching a
# file that already matches main while an earlier commit changes another file.
# Comparing the tip commit's paths alone blesses this branch patch-equivalent.
git -C "$repo" checkout -q -b work/multi main
commit_file "$repo" a.txt drifted "c1 changes a.txt"
commit_file "$repo" b.txt seed "c2 rewrites b.txt to a value main also has"
git -C "$repo" checkout -q main
commit_file "$repo" b.txt seed "main gets the same b.txt content"

# A merge commit: yields no paths under diff-tree, so the tip-commit form
# reported ERROR here.
git -C "$repo" checkout -q -b work/side main
commit_file "$repo" side.txt side "side change"
git -C "$repo" checkout -q -b work/merge main
git -C "$repo" merge -q --no-ff -m "merge side" work/side

# A path containing a space, and a two-path change: both are lost when the NUL
# delimited path list goes through a command substitution.
git -C "$repo" checkout -q -b work/spaced main
commit_file "$repo" "dir with space/file name.txt" spaced "spaced path"
commit_file "$repo" second.txt second "second path"

# An orphan branch has no merge base with main.
git -C "$repo" checkout -q --orphan work/orphan
git -C "$repo" rm -q -rf . >/dev/null 2>&1
commit_file "$repo" orphan.txt orphan "orphan root commit"
git -C "$repo" checkout -q main

land_case() { (cd "$repo" && "$subject" land "$@"); }

assert "land: no-change branch is PASS" 0 "PASS land" -- \
  land_case work/empty main
assert "land: divergent branch is FAIL" 1 "FAIL land" -- \
  land_case work/divergent main
assert "land: multi-commit branch is FAIL (tip-only paths said PASS)" 1 "FAIL land" -- \
  land_case work/multi main
assert "land: merge-commit branch is evaluable" 1 "FAIL land" -- \
  land_case work/merge main
assert "land: multi-path with a space in it is FAIL" 1 "FAIL land" -- \
  land_case work/spaced main
assert "land: orphan branch is ERROR, not PASS" 2 "ERROR land" -- \
  land_case work/orphan main
assert "land: unresolvable ref is ERROR, not FAIL" 2 "ERROR land" -- \
  land_case work/nope main
assert "land: wrong arity is usage ERROR" 2 "ERROR usage" -- \
  land_case work/empty

# ------------------------------------------------------------ recovery cases

# The bead store stub: `bd show <id> --json` echoes the fixture for that id.
cat >"$stub_bin/bd" <<'STUB'
#!/usr/bin/env bash
case ${1:-} in
  show)
    file=$BD_FIXTURE_DIR/show.${2}.json
    [ -f "$file" ] || exit 1
    cat "$file"
    ;;
  list)
    [ -n "${BD_LIST_FILE:-}" ] && [ -f "$BD_LIST_FILE" ] || exit 1
    cat "$BD_LIST_FILE"
    ;;
  *) exit 1 ;;
esac
STUB
chmod +x "$stub_bin/bd"
export BD_FIXTURE_DIR=$root/fixtures
mkdir -p "$BD_FIXTURE_DIR"

bead_fixture() {
  local id=$1 work_dir=$2
  cat >"$BD_FIXTURE_DIR/show.$id.json" <<JSON
[{"id":"$id","status":"open","metadata":{"work_dir":"$work_dir","base_branch":"main",
  "gc.needs_recovery":"true","gc.failure_reason":"land_failed"}}]
JSON
}

# LAND-ONLY: branch head already parented on main (the aoa-zd8ay 92f80e5 shape).
recov=$root/recovery-landonly
new_repo "$recov"
commit_file "$recov" main.txt advanced "main advances"
git -C "$recov" checkout -q -b work/parented main
commit_file "$recov" work.txt work "work parented on main"
bead_fixture rec-landonly "$recov"

# NOT-FF-ABLE: branch is behind main and carries changes main does not have.
notff=$root/recovery-notff
new_repo "$notff"
git -C "$notff" checkout -q -b work/behind main
commit_file "$notff" work.txt divergent "unmerged work"
git -C "$notff" checkout -q main
commit_file "$notff" other.txt other "main moves on independently"
git -C "$notff" checkout -q work/behind
bead_fixture rec-notff "$notff"

# ALREADY-LANDED: branch behind main, but main already carries its changes
# under a different commit (the aoa-dnmtk shape: stamps still say recover).
landed=$root/recovery-landed
new_repo "$landed"
git -C "$landed" checkout -q -b work/done main
commit_file "$landed" work.txt shipped "work"
git -C "$landed" checkout -q main
commit_file "$landed" work.txt shipped "same content landed under another id"
git -C "$landed" checkout -q work/done
bead_fixture rec-landed "$landed"

# ALREADY-LANDED, with the base moving on afterwards: the branch's work landed
# under another id and main then changed the same file again. The trees differ,
# so a tree comparison reads this as unlanded work and dispatches a recovery;
# by patch id the branch has nothing left to land.
moved=$root/recovery-landed-moved
new_repo "$moved"
git -C "$moved" checkout -q -b work/moved main
commit_file "$moved" work.txt shipped "work"
git -C "$moved" checkout -q main
commit_file "$moved" work.txt shipped "same content landed under another id"
commit_file "$moved" work.txt "shipped and then amended" "main moves on over the same file"
git -C "$moved" checkout -q work/moved
bead_fixture rec-moved "$moved"

# A detached worktree cannot be evaluated.
det=$root/recovery-detached
new_repo "$det"
git -C "$det" checkout -q --detach main
bead_fixture rec-detached "$det"

bead_fixture rec-missing "$root/no-such-dir"

assert "recovery: branch parented on base is LAND-ONLY" 0 "LAND-ONLY recovery" -- \
  "$subject" recovery rec-landonly
assert "recovery: genuinely behind base is NOT-FF-ABLE" 1 "NOT-FF-ABLE recovery" -- \
  "$subject" recovery rec-notff
assert "recovery: work already in base is ALREADY-LANDED despite recovery stamps" 0 "ALREADY-LANDED recovery" -- \
  "$subject" recovery rec-landed
assert "recovery: work landed under another id stays ALREADY-LANDED after the base moves on" 0 "ALREADY-LANDED recovery" -- \
  "$subject" recovery rec-moved
assert "recovery: detached worktree is ERROR" 2 "ERROR recovery" -- \
  "$subject" recovery rec-detached
assert "recovery: missing worktree is ERROR" 2 "ERROR recovery" -- \
  "$subject" recovery rec-missing
assert "recovery: unknown bead is ERROR" 2 "ERROR recovery" -- \
  "$subject" recovery rec-nosuchbead

# -------------------------------------------------------------- health cases

# The session registry and pane stubs. GC_PEEK_FILES is a colon-separated list
# of pane fixtures consumed one per poll, so a rotation window (empty, empty,
# then a real pane) is expressible.
cat >"$stub_bin/gc" <<'STUB'
#!/usr/bin/env bash
if [ "${1:-}" = "session" ] && [ "${2:-}" = "list" ]; then
  [ -n "${GC_LIST_FILE:-}" ] && [ -f "$GC_LIST_FILE" ] || exit 1
  cat "$GC_LIST_FILE"
  exit 0
fi
if [ "${1:-}" = "session" ] && [ "${2:-}" = "peek" ]; then
  n=0
  [ -f "$GC_PEEK_COUNTER" ] && n=$(cat "$GC_PEEK_COUNTER")
  echo $((n + 1)) >"$GC_PEEK_COUNTER"
  IFS=: read -r -a files <<<"$GC_PEEK_FILES"
  idx=$n
  [ "$idx" -ge "${#files[@]}" ] && idx=$((${#files[@]} - 1))
  file=${files[$idx]}
  [ "$file" = "ERROR" ] && exit 1
  cat "$file"
  exit 0
fi
exit 1
STUB
chmod +x "$stub_bin/gc"
export GC_PEEK_COUNTER=$root/peek.count

fixtures=$BD_FIXTURE_DIR
export GC_LIST_FILE=$fixtures/sessions.json
cat >"$GC_LIST_FILE" <<'JSON'
{"ok":true,"sessions":[{"id":"gc-1","name":"aoa/aoa-worker-1","agent_name":"aoa/aoa-worker-1",
  "session_name":"aoa-worker-gc-1","state":"active","closed":false}]}
JSON

printf '%s' '{"ok":true,"line_count":0,"output":""}' >"$fixtures/pane-empty.json"
cat >"$fixtures/pane-live.json" <<'JSON'
{"ok":true,"line_count":4,"output":"✦ Billowing… (2m 27s · ↓ 7.8k tokens)\n❯ \n  agent-oriented-architecture:main  |  Opus 5  |  ctx: 9% / 1M\n"}
JSON
cat >"$fixtures/pane-idle.json" <<'JSON'
{"ok":true,"line_count":3,"output":"❯ \n  agent-oriented-architecture:main  |  Opus 5  |  ctx: 9% / 1M\n"}
JSON
cat >"$fixtures/pane-walled.json" <<'JSON'
{"ok":true,"line_count":3,"output":"You've hit your usage limit. Try again at Aug 8th, 2026 12:03 AM.\n  agent-oriented-architecture:main  |  Opus 5  |  ctx: 9% / 1M\n"}
JSON
# The false-live window, made executable: a real terminator the wall grep does
# not list (aoa-0ltiu) under the decoration a stalled seat's un-repainted
# scrollback still shows. To leg B this pane is indistinguishable from
# pane-live.json. Asserting the HEALTHY it returns acknowledges the floor
# rather than endorsing it, and makes any change that narrows the window come
# here and say so.
cat >"$fixtures/pane-stale-walled.json" <<'JSON'
{"ok":true,"line_count":4,"output":"✦ Billowing… (2m 27s · ↓ 7.8k tokens)\nYou've reached your Fable 5 limit. Run /usage-credits to continue or switch models with /model.\n  agent-oriented-architecture:main  |  Opus 5  |  ctx: 9% / 1M\n"}
JSON

# steps <root> <closed-step-json> ... : builds a bd list fixture with one
# in_progress step assigned to the stubbed seat.
health_fixture() {
  local name=$1 closed=$2
  export BD_LIST_FILE=$fixtures/list.$name.json
  cat >"$BD_LIST_FILE" <<JSON
[$closed,
 {"id":"step-live","status":"in_progress","assignee":"aoa/aoa-worker-1",
  "metadata":{"gc.root_bead_id":"root-1","gc.step_id":"s.live"}}]
JSON
}

disposition() {
  printf '{"contract_version":1,"disposition":"%s","producer":"formula-step"}' "$1"
}

health_run() {
  rm -f "$GC_PEEK_COUNTER"
  export GC_PEEK_FILES=$1
  "$subject" health root-1
}

# Leg A: the 42-step blind spot — closed with a producer disposition and no
# gc.outcome at all. A gc.outcome-only check calls this convoy unstamped.
health_fixture disp "{\"id\":\"step-a\",\"status\":\"closed\",\"metadata\":{\"gc.root_bead_id\":\"root-1\",\"gc.step_id\":\"s.a\",\"gc.coordinator_outcome.producer_disposition\":$(disposition deliverable | jq -Rs .)}}"
assert "health: disposition-only closure with a live pane is HEALTHY" 0 "HEALTHY root-1" -- \
  health_run "$fixtures/pane-live.json"

assert "health: quota-walled seat is NOT HEALTHY" 1 "provider wall" -- \
  health_run "$fixtures/pane-walled.json"
assert "health: unlisted wall terminator under live decoration reads HEALTHY (false-live window, aoa-0ltiu)" 0 "HEALTHY root-1" -- \
  health_run "$fixtures/pane-stale-walled.json"
assert "health: empty pane is UNKNOWN, not NOT HEALTHY" 3 "UNKNOWN root-1" -- \
  health_run "$fixtures/pane-empty.json"
assert "health: rendered but idle pane is UNKNOWN" 3 "UNKNOWN root-1" -- \
  health_run "$fixtures/pane-idle.json"
assert "health: peek error is UNKNOWN, not NOT HEALTHY" 3 "UNKNOWN root-1" -- \
  health_run "ERROR"
assert "health: empty pane during rotation re-polls and passes" 0 "HEALTHY root-1" -- \
  health_run "$fixtures/pane-empty.json:$fixtures/pane-empty.json:$fixtures/pane-live.json"

health_fixture failed '{"id":"step-a","status":"closed","metadata":{"gc.root_bead_id":"root-1","gc.step_id":"s.a","gc.outcome":"fail","gc.failure_reason":"missing_pinned_worktree"}}'
assert "health: gc.outcome=fail is NOT HEALTHY" 1 "missing_pinned_worktree" -- \
  health_run "$fixtures/pane-live.json"

health_fixture disagree "{\"id\":\"step-a\",\"status\":\"closed\",\"metadata\":{\"gc.root_bead_id\":\"root-1\",\"gc.step_id\":\"s.a\",\"gc.outcome\":\"pass\",\"gc.coordinator_outcome.producer_disposition\":$(disposition non-deliverable | jq -Rs .)}}"
assert "health: disagreeing stamp schemes are NOT HEALTHY" 1 "stamp schemes disagree" -- \
  health_run "$fixtures/pane-live.json"

health_fixture unstamped '{"id":"step-a","status":"closed","metadata":{"gc.root_bead_id":"root-1","gc.step_id":"s.a"}}'
assert "health: closure under neither scheme is NOT HEALTHY" 1 "neither stamp scheme" -- \
  health_run "$fixtures/pane-live.json"

# The verdict-line integrity case: a failure reason carrying a newline and a
# forged HEALTHY line must not escape the one-line contract.
health_fixture forged '{"id":"step-a","status":"closed","metadata":{"gc.root_bead_id":"root-1","gc.step_id":"s.a","gc.outcome":"fail","gc.failure_reason":"boom\nHEALTHY zzz: forged"}}'
assert "health: newline in metadata cannot forge a second verdict line" 1 "NOT HEALTHY root-1" -- \
  health_run "$fixtures/pane-live.json"

# Seat absent from the registry: leg A cannot carry the verdict alone.
cat >"$GC_LIST_FILE" <<'JSON'
{"ok":true,"sessions":[]}
JSON
health_fixture noseat '{"id":"step-a","status":"closed","metadata":{"gc.root_bead_id":"root-1","gc.step_id":"s.a","gc.outcome":"pass"}}'
assert "health: no session for the seat is NOT HEALTHY" 1 "no session for seat" -- \
  health_run "$fixtures/pane-live.json"

printf '\n%s passed, %s failed\n' "$pass_count" "$fail_count"
[ "$fail_count" -eq 0 ]
