#!/usr/bin/env bash
# Verify that every submodule gitlink recorded in HEAD's tree is reachable on the
# submodule's own remote. Prevents the incident class where a submodule commit is
# advanced locally, the superproject is pushed, but the submodule commit itself is
# never pushed: every local build/test passes (the commit exists in the local
# clone), and it only breaks CI / fresh clones at checkout time with a cryptic
# "not our ref" fatal (task #341). ~keep
#
# Usage: check-submodule-gitlinks.sh [--on-fetch-failure=warn|fail]
#
# Submodules are read generically from .gitmodules -- never hard-code a path.
# Each gitlink SHA is resolved from HEAD's tree (git ls-tree), so this works
# even without the submodule checked out locally.
#
# Two-step check per submodule:
#   1. `git ls-remote <url>`                    -- can we reach the remote at all?
#   2. `git fetch --depth 1 <url> <sha>`         -- does the remote actually have this SHA?
# --on-fetch-failure governs ONLY step 1: if we can't even reach the remote (network
# down, transient outage, auth hiccup), that's ambiguous, not proof the SHA is
# missing, so it can be treated leniently (warn). Step 2 is never lenient: if the
# remote answers but doesn't have the SHA, that is unconditionally a hard failure
# regardless of the flag -- that is the exact incident this script exists to catch.
set -u -o pipefail

TIMEOUT_SECS="${SUBMODULE_GITLINK_TIMEOUT:-15}"
ON_FETCH_FAILURE="fail"

for arg in "$@"; do
  case "$arg" in
    --on-fetch-failure=warn | --on-fetch-failure=fail)
      ON_FETCH_FAILURE="${arg#--on-fetch-failure=}"
      ;;
    *)
      echo "error: unrecognized argument '$arg'" >&2
      echo "usage: $0 [--on-fetch-failure=warn|fail]" >&2
      exit 2
      ;;
  esac
done

REPO_ROOT="$(git rev-parse --show-toplevel 2>/dev/null)" || {
  echo "error: not inside a git repository" >&2
  exit 2
}
cd "$REPO_ROOT" || exit 2

GITMODULES="$REPO_ROOT/.gitmodules"

if [ ! -f "$GITMODULES" ]; then
  # No submodules in this repo -- nothing to check.
  exit 0
fi

# Hand-rolled timeout: `timeout`/`gtimeout` are not reliably present on every
# runner/dev machine (stock macOS has neither without Homebrew coreutils), so
# wrap the command with a background killer instead of depending on either. ~keep
run_with_timeout() {
  local secs="$1"
  shift
  "$@" &
  local cmd_pid=$!
  (
    sleep "$secs" 2>/dev/null
    kill -TERM "$cmd_pid" 2>/dev/null
  ) &
  local timer_pid=$!
  local status=0
  wait "$cmd_pid" 2>/dev/null || status=$?
  kill "$timer_pid" 2>/dev/null
  wait "$timer_pid" 2>/dev/null
  return "$status"
}

overall_status=0

while IFS=' ' read -r key path; do
  [ -n "$key" ] || continue
  name="${key#submodule.}"
  name="${name%.path}"

  url="$(git config -f "$GITMODULES" --get "submodule.$name.url")" || {
    echo "warning: submodule '$path' has no url in .gitmodules; skipping" >&2
    continue
  }

  sha="$(git ls-tree HEAD -- "$path" 2>/dev/null | awk '{print $3}')"
  if [ -z "$sha" ]; then
    echo "warning: could not resolve a gitlink for submodule '$path' at HEAD; skipping" >&2
    continue
  fi

  if ! run_with_timeout "$TIMEOUT_SECS" git ls-remote "$url" >/dev/null 2>&1; then
    warning="could not reach remote for submodule '$path' ($url); skipping SHA check (network may be down)"
    if [ "$ON_FETCH_FAILURE" = "fail" ]; then
      echo "error: $warning" >&2
      overall_status=1
    else
      echo "warning: $warning" >&2
    fi
    continue
  fi

  tmp_dir="$(mktemp -d)" || {
    echo "error: mktemp failed while checking submodule '$path'" >&2
    overall_status=1
    continue
  }
  git init -q "$tmp_dir"

  if run_with_timeout "$TIMEOUT_SECS" git -C "$tmp_dir" fetch --depth 1 "$url" "$sha" >/dev/null 2>&1; then
    echo "ok: submodule '$path' gitlink $sha is reachable on $url"
  else
    {
      echo "error: submodule '$path' points at $sha, which is NOT reachable on its remote ($url)."
      echo "  The superproject was pushed with a submodule commit that was never pushed to the"
      echo "  submodule's own remote. It builds and tests fine locally (the commit exists in"
      echo "  your local clone) but breaks every fresh clone and CI checkout."
      echo "  Fix: cd $path && git push origin $sha:refs/heads/main"
    } >&2
    overall_status=1
  fi

  rm -rf "$tmp_dir"
done < <(git config -f "$GITMODULES" --get-regexp '\.path$' || true)

exit "$overall_status"
