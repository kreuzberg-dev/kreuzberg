#!/usr/bin/env bash

set -euo pipefail

repository_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
test_directory="$(mktemp -d)"
trap 'rm -rf "$test_directory"' EXIT

mkdir -p "$test_directory/bin"
printf 'fixture' >"$test_directory/input.pdf"

cat >"$test_directory/bin/timeout" <<'EOF'
#!/usr/bin/env bash
shift
exec "$@"
EOF
cat >"$test_directory/bin/lit" <<'EOF'
#!/usr/bin/env bash
echo "native liteparse failure" >&2
exit 42
EOF
chmod +x "$test_directory/bin/timeout" "$test_directory/bin/lit"

set +e
PATH="$test_directory/bin:$PATH" \
  bash "$repository_root/tools/benchmark-harness/scripts/liteparse_extract.sh" \
  --format=plaintext \
  "$test_directory/input.pdf" \
  >"$test_directory/stdout" \
  2>"$test_directory/stderr"
status=$?
set -e

if [ "$status" -ne 42 ]; then
  echo "expected liteparse wrapper to preserve exit 42, got $status" >&2
  exit 1
fi
if [ -s "$test_directory/stdout" ]; then
  echo "expected failed extraction to produce no result payload" >&2
  exit 1
fi
if ! grep -q "native liteparse failure" "$test_directory/stderr"; then
  echo "expected native liteparse error on stderr" >&2
  exit 1
fi
