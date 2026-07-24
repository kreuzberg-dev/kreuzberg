#!/usr/bin/env bash
set -euo pipefail

log() { echo "vendor-native-closure: $*" >&2; }
die() { log "$*"; exit 1; }
cleanup() { [ -n "${WORKDIR:-}" ] && rm -rf "$WORKDIR"; }

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
  local native="$1" dir queue seen bin lib base destination
  dir="$(cd "$(dirname "$native")" && pwd)"
  native="$dir/$(basename "$native")"
  queue="$(mktemp)"
  seen="$(mktemp)"
  printf '%s\n' "$native" >"$queue"
  while [ -s "$queue" ]; do
    bin="$(head -n1 "$queue")"
    tail -n +2 "$queue" >"$queue.tmp" && mv "$queue.tmp" "$queue"
    ldd "$bin" 2>/dev/null |
      sed -n 's/.*=> *\(\/[^ ]*\).*/\1/p; s/^[[:space:]]*\(\/[^ ]*\) (0x[0-9a-f]*)$/\1/p' |
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
