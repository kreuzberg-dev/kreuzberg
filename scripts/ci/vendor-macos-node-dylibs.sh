#!/usr/bin/env bash
# Vendor a macOS .node's non-system dylib closure (the libheif stack) beside
# it, rewrite absolute load commands to @loader_path, and ad-hoc re-sign, so
# the darwin package dlopens without anything preinstalled. ONNX Runtime is
# untouched: @rpath-relocatable and resolved by co-location.
#
# Usage: vendor-macos-node-dylibs.sh <dir-containing-the-.node> ~keep
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="${REPO_ROOT:-$(cd "$SCRIPT_DIR/../.." && pwd)}"
source "$REPO_ROOT/scripts/lib/macho.sh"

DIR="${1:?usage: vendor-macos-node-dylibs.sh <dir>}"
DIR="$(cd "$DIR" && pwd)"

# Absolute path outside the system prefixes = vendorable. @rpath/@loader_path/
# @executable_path are already relocatable and left alone. ~keep
is_vendorable() {
  case "$1" in
    /usr/lib/*|/System/*) return 1 ;;
    @*)                   return 1 ;;
    /*)                   return 0 ;;
    *)                    return 1 ;;
  esac
}

# Load-command deps of a Mach-O, excluding the header line and the binary's own
# install id (LC_ID_DYLIB) — for a .node the id is often a CI build path that
# must never be vendored. ~keep
deps_of() {
  local self_id
  self_id="$(otool -D "$1" | tail -n +2 | head -1 | sed 's/^[[:space:]]*//')"
  # Drop the header (line 1) and exclude the binary's own id. `|| true` keeps a
  # binary with no vendorable deps from failing the pipeline under `set -e`. ~keep
  otool -L "$1" | tail -n +2 | sed 's/^[[:space:]]*//; s/ (compatibility.*//' \
    | grep -vxF -e "$self_id" || true
}

resolve() { readlink -f "$1" 2>/dev/null || python3 -c 'import os,sys;print(os.path.realpath(sys.argv[1]))' "$1"; }
resign()  { codesign --remove-signature "$1" 2>/dev/null || true; codesign -f -s - "$1"; }

declare -A seen
queue=()
for node in "$DIR"/*.node; do
  [ -e "$node" ] || continue
  base="$(basename "$node")"; queue+=("$base"); seen["$base"]=1
done
[ ${#queue[@]} -gt 0 ] || { echo "::error::no .node in $DIR to vendor for"; exit 1; }

i=0
while [ $i -lt ${#queue[@]} ]; do
  bin="${queue[$i]}"; i=$((i+1))
  target="$DIR/$bin"
  [ -f "$target" ] || { echo "::warning::$bin queued but not present"; continue; }
  changed=0
  while IFS= read -r dep; do
    [ -n "$dep" ] || continue
    is_vendorable "$dep" || continue
    b="$(basename "$dep")"
    if [ ! -f "$DIR/$b" ]; then
      src="$(resolve "$dep")"
      # Homebrew bottles target the runner's own macOS and would raise the
      # package's floor; the heif closure comes from build-macos-heif-deps.sh. ~keep
      case "$src" in
        /opt/homebrew/*|/usr/local/*)
          echo "::error::refusing to vendor Homebrew dylib $dep"; exit 1 ;;
      esac
      cp -f "$src" "$DIR/$b"; chmod u+w "$DIR/$b"
      echo "vendored $b"
    fi
    install_name_tool -change "$dep" "@loader_path/$b" "$target"
    changed=1
    if [ -z "${seen[$b]:-}" ]; then seen["$b"]=1; queue+=("$b"); fi
  done < <(deps_of "$target")
  case "$bin" in
    *.dylib) install_name_tool -id "@loader_path/$bin" "$target" 2>/dev/null || true; changed=1 ;;
  esac
  [ $changed -eq 1 ] && resign "$target"
done

leaks=0
for f in "$DIR"/*.node "$DIR"/*.dylib; do
  [ -e "$f" ] || continue
  while IFS= read -r dep; do
    is_vendorable "$dep" && { echo "::error::unvendored dep $(basename "$f") -> $dep"; leaks=$((leaks+1)); }
  done < <(deps_of "$f")
done
[ $leaks -eq 0 ] || { echo "::error::$leaks unvendored absolute deps remain"; exit 1; }

# The .node must sit at the declared floor. Vendored dylibs we did not compile
# (ONNX Runtime) are reported so the package's effective floor is visible. ~keep
if [ -n "${MACOSX_DEPLOYMENT_TARGET:-}" ]; then
  for f in "$DIR"/*.node "$DIR"/*.dylib; do
    [ -e "$f" ] || continue
    m="$(minos_of "$f")"
    echo "minos $m $(basename "$f")"
    case "$f" in
      *.node) [ "$m" = "$MACOSX_DEPLOYMENT_TARGET" ] || {
        echo "::error::$(basename "$f") targets macOS $m, expected $MACOSX_DEPLOYMENT_TARGET"; exit 1; } ;;
    esac
  done
fi
echo "macOS dylib closure vendored and self-contained under $DIR"
