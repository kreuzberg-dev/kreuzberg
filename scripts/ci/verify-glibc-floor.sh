#!/usr/bin/env bash
#   2. highest versioned GLIBC symbol <= the floor (default 2.28),
set -euo pipefail

MAX_GLIBC="${2:-2.28}"

log() { echo "verify-glibc-floor: $*" >&2; }
die() { log "$*"; exit 1; }
cleanup() { [ -n "${WORKDIR:-}" ] && rm -rf "$WORKDIR"; }

gt() { [ "$(printf '%s\n%s\n' "$1" "$2" | sort -V | tail -1)" = "$1" ] && [ "$1" != "$2" ]; }

check_lib() {
  local lib="$1" root="$2" name bad highest
  name="$(basename "$lib")"

  bad="$(objdump -T "$lib" 2>/dev/null | grep -oE '__isoc23[a-z_]*|__libc_single_threaded' | sort -u || true)"
  [ -z "$bad" ] || die "$name references too-new symbols: $(echo "$bad" | tr '\n' ' ')"

  highest="$(objdump -T "$lib" 2>/dev/null | grep -oE 'GLIBC_[0-9]+\.[0-9]+' | sed 's/GLIBC_//' | sort -V | tail -1)"
  if [ -n "$highest" ] && gt "$highest" "$MAX_GLIBC"; then
    die "$name requires glibc $highest > floor $MAX_GLIBC"
  fi

  if ! find "$root" -name 'libonnxruntime*.so*' -type f | grep -q .; then
    die "$name has no libonnxruntime.so bundled in the artifact"
  fi
  log "OK $name (floor <= $MAX_GLIBC, ORT bundled)"
}

verify_tree() {
  local root="$1" found=0 lib
  while IFS= read -r lib; do
    found=1
    check_lib "$lib" "$root"
  done < <(find "$root" \( -name 'libxberg_*.so' -o -name '_xberg*.so' -o -name 'php_xberg.so' -o -name '*.node' \) -type f)
  [ "$found" = 1 ] || die "no xberg native library found under $root"
}

main() {
  [ $# -ge 1 ] || die "usage: $(basename "$0") <artifact> [max-glibc]"
  local artifact
  artifact="$(cd "$(dirname "$1")" && pwd)/$(basename "$1")"
  WORKDIR="$(mktemp -d)"
  trap cleanup EXIT

  case "$artifact" in
  *.whl) unzip -qo "$artifact" -d "$WORKDIR"; verify_tree "$WORKDIR" ;;
  *.tar.gz | *.tgz) tar -xzf "$artifact" -C "$WORKDIR"; verify_tree "$WORKDIR" ;;
  *) [ -d "$artifact" ] || die "unsupported artifact '$artifact'"; verify_tree "$artifact" ;;
  esac
  log "artifact passes the glibc floor gate"
}

main "$@"
