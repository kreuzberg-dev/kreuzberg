#!/usr/bin/env bash
set -euo pipefail

log() { echo "vendor-native-closure: $*" >&2; }
die() { log "$*"; exit 1; }
cleanup() { [ -n "${WORKDIR:-}" ] && rm -rf "$WORKDIR"; }

# --- diagnostics for task #490 (mysterious exit 127 late in the vendor loop) ---
# This script is invoked as `bash vendor-native-closure.sh ...` by the Dockerfile
# (see docker/Dockerfile.musl-python), so bash — not ash/dash — always interprets
# it; bash-only constructs below (PS4 expansions, ERR trap, BASH_COMMAND) are safe.
# `-x` plus a PS4 carrying file:line:function makes every executed command and its
# location visible in the CI log, so the exact failing invocation is no longer
# ambiguous even after ~28 prior successful loop iterations. ~keep
export PS4='+ [vendor-native-closure ${BASH_SOURCE##*/}:${LINENO} ${FUNCNAME[0]:-main}] '
set -x

# A bare "command not found" (exit 127) from the shell itself — as opposed to an
# error returned BY patchelf/ldd — most often means either (a) the binary truly
# isn't on PATH, or (b) the shell couldn't fork/exec it at all (e.g. process/
# memory exhaustion). Checking both up front rules out (a) immediately; the ERR
# trap below plus the Dockerfile's df/free dump right before this script runs
# help distinguish (b) from resource exhaustion mid-run. ~keep
for tool in patchelf ldd env readlink mktemp find sed grep chmod cp; do
  command -v "$tool" >/dev/null 2>&1 || die "required tool '$tool' not found on PATH ($PATH)"
done

# Fires on any command failing under `set -e`, including the exec-failure case
# that manufactures exit 127; prints the exact source line and command so the
# next CI run pins precisely which invocation broke.
#
# `set -E` is REQUIRED here and not redundant with the `set -euo pipefail` on
# line 2: without errtrace an ERR trap is NOT inherited by shell functions, and
# the two suspects (`set_origin_rpath`, `verify_local_closure`) are both
# functions -- so the trap would stay silent in exactly the frames we are
# trying to pin. ~keep
set -E
trap 'log "ERROR: exit $? at line ${LINENO} (function: ${FUNCNAME[0]:-main}): ${BASH_COMMAND}"' ERR

is_base_lib() {
  case "$1" in
  ld-linux* | ld-musl* | libc.so* | libc.musl* | libc-*.so* | libm.so* | libmvec.so* | \
    libdl.so* | librt.so* | libpthread.so* | libresolv.so* | libgcc_s.so* | \
    libstdc++.so* | libssl.so* | libcrypto.so*) return 0 ;;
  *) return 1 ;;
  esac
}

set_origin_rpath() {
  local elf="$1"
  # shellcheck disable=SC2016
  patchelf --set-rpath '$ORIGIN' "$elf"
}

verify_local_closure() {
  local native="$1" dir report lib base resolved
  dir="$(cd "$(dirname "$native")" && pwd)"
  report="$(mktemp)"

  if ! env -u LD_LIBRARY_PATH ldd "$native" >"$report" 2>&1; then
    cat "$report" >&2
    rm -f "$report"
    die "$(basename "$native") has unresolved dependencies after vendoring"
  fi
  if grep -q 'not found' "$report"; then
    cat "$report" >&2
    rm -f "$report"
    die "$(basename "$native") has unresolved dependencies after vendoring"
  fi

  while IFS= read -r lib; do
    [ -f "$lib" ] || continue
    base="$(basename "$lib")"
    is_base_lib "$base" && continue
    resolved="$(readlink -f "$lib")"
    case "$resolved" in
    "$dir"/*) ;;
    *)
      cat "$report" >&2
      rm -f "$report"
      die "$base still resolves outside the bundle: $resolved"
      ;;
    esac
  done < <(
    sed -n 's/.*=> *\(\/[^ ]*\).*/\1/p; s/^[[:space:]]*\(\/[^ ]*\) (0x[0-9a-f]*)$/\1/p' "$report"
  )

  rm -f "$report"
  log "verified local dependency closure for $(basename "$native")"
}

vendor_one() {
  local native="$1" dir queue seen bin lib base destination ldd_output ldd_status
  dir="$(cd "$(dirname "$native")" && pwd)"
  native="$dir/$(basename "$native")"
  queue="$(mktemp)"
  seen="$(mktemp)"
  printf '%s\n' "$native" >"$queue"
  while [ -s "$queue" ]; do
    bin="$(head -n1 "$queue")"
    tail -n +2 "$queue" >"$queue.tmp" && mv "$queue.tmp" "$queue"

    # musl's `ldd` (unlike glibc's) exits non-zero -- observed as a bare exit 127
    # -- when a transitive dependency can't be resolved, instead of exiting 0 and
    # annotating that entry "=> not found". Piping straight into `sed | while`
    # under this script's `set -euo pipefail` (line 2) turned that into a silent,
    # unattributed script death partway through the closure walk: stderr was
    # discarded (`2>/dev/null`) so nothing said which binary or which library
    # broke, and pipefail killed the whole multi-wheel loop rather than just this
    # one branch of the walk. Capture ldd's output and exit status explicitly so
    # a resolution failure here is logged with the offending binary and ldd's raw
    # output, then skipped -- exactly like the existing `[ -f "$lib" ] || continue`
    # policy for individually-missing libraries below. This does not weaken the
    # closure guarantee: `verify_local_closure` still runs ldd on the top-level
    # artifact afterward and `die`s loudly if anything is genuinely unresolved.
    ldd_output="$(mktemp)"
    ldd_status=0
    ldd "$bin" >"$ldd_output" 2>&1 || ldd_status=$?
    if [ "$ldd_status" -ne 0 ]; then
      log "WARNING: ldd exited $ldd_status resolving dependencies of $(basename "$bin"); output:"
      cat "$ldd_output" >&2
    fi
    sed -n 's/.*=> *\(\/[^ ]*\).*/\1/p; s/^[[:space:]]*\(\/[^ ]*\) (0x[0-9a-f]*)$/\1/p' "$ldd_output" |
      while IFS= read -r lib; do
        [ -f "$lib" ] || continue
        base="$(basename "$lib")"
        is_base_lib "$base" && continue
        grep -qxF "$base" "$seen" 2>/dev/null && continue
        printf '%s\n' "$base" >>"$seen"
        destination="$dir/$base"
        if [ "$(readlink -f "$lib")" != "$(readlink -f "$destination" 2>/dev/null || true)" ]; then
          cp -L "$lib" "$destination"
        fi
        chmod u+w "$destination" 2>/dev/null || true
        set_origin_rpath "$destination"
        printf '%s\n' "$destination" >>"$queue"
        log "vendored $base beside $(basename "$native")"
      done
    rm -f "$ldd_output"
  done
  rm -f "$queue" "$seen"
  set_origin_rpath "$native"
  verify_local_closure "$native"
}

vendor_tree() {
  local root="$1" found=0 lib
  while IFS= read -r lib; do
    found=1
    vendor_one "$lib"
  done < <(find "$root" \( -name 'libxberg_*.so' -o -name '_xberg*.so' -o -name 'php_xberg.so' -o -name '*.node' \) -type f)
  [ "$found" = 1 ] || die "no xberg native library found under $root"
}

main() {
  local artifact="${1:?usage: $(basename "$0") <artifact>}"
  case "$artifact" in
  *.tar.gz | *.tgz)
    WORKDIR="$(mktemp -d)"
    trap cleanup EXIT
    tar -xzf "$artifact" -C "$WORKDIR"
    vendor_tree "$WORKDIR"
    rm -f "$artifact"
    tar -czf "$artifact" -C "$WORKDIR" .
    ;;
  *.whl)
    WORKDIR="$(mktemp -d)"
    trap cleanup EXIT
    artifact="$(cd "$(dirname "$artifact")" && pwd)/$(basename "$artifact")"
    unzip -qo "$artifact" -d "$WORKDIR"
    vendor_tree "$WORKDIR"
    rm -f "$artifact"
    (cd "$WORKDIR" && zip -qr "$artifact" .)
    ;;
  *.so | *.node) vendor_one "$artifact" ;;
  *)
    if [ -f "$artifact" ]; then
      vendor_one "$artifact"
    elif [ -d "$artifact" ]; then
      vendor_tree "$artifact"
    else
      die "unsupported artifact '$artifact'"
    fi
    ;;
  esac
  log "done"
}

main "$@"
