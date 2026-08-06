#!/bin/sh
# Installed by `aoa observe --enforce`. Do not hand-edit: `aoa` rewrites this
# file from its own copy, and a drift test fails when the two disagree.
#
# Claude Code runs hook commands through /bin/sh with the host's environment,
# not a login shell, so a bare `aoa` resolves only when the binary happens to
# sit on that environment's PATH. When it does not, every hook fails
# non-blocking and the host swallows it: the enforcement plane reads as
# installed and present while never having run once. This wrapper exists so
# that case is impossible to mistake for enforcement passing.
set -eu

if [ "$#" -eq 0 ]; then
    echo "aoa-enforce: missing enforce subcommand" >&2
    exit 1
fi
verb=$1

# The repo root is two directories above this script (<repo>/.claude/hooks),
# resolved from the script's own path so the wrapper works regardless of the
# caller's working directory.
script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
repo_root=$(CDPATH= cd -- "$script_dir/../.." && pwd)

# Resolution order: an explicit operator override, then PATH, then this
# checkout's own build outputs. Release before debug, so a repo that has both
# enforces with the binary it ships rather than the last one someone debugged.
aoa_bin=""
for candidate in \
    "${AOA_BIN:-}" \
    "$(command -v aoa 2>/dev/null || true)" \
    "$repo_root/target/release/aoa" \
    "$repo_root/target/debug/aoa"; do
    if [ -n "$candidate" ] && [ -x "$candidate" ]; then
        aoa_bin=$candidate
        break
    fi
done

if [ -z "$aoa_bin" ]; then
    echo "aoa-enforce: ENFORCEMENT UNAVAILABLE — no aoa binary found for '$verb'." >&2
    echo "aoa-enforce: looked at \$AOA_BIN, PATH, $repo_root/target/{release,debug}/aoa." >&2
    echo "aoa-enforce: build it (cargo build --release) or set AOA_BIN; this hook did NOT enforce." >&2
    # `check` is the only blocking hook. Unavailable enforcement must not read
    # as an allowed write, so it denies (exit 2) rather than falling open. The
    # advisory hooks cannot block; they exit non-zero so the host surfaces the
    # failure instead of recording a span that was never written.
    if [ "$verb" = "check" ]; then
        exit 2
    fi
    exit 1
fi

exec "$aoa_bin" enforce "$@"
