#!/usr/bin/env bash

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
source "$REPO_ROOT/tools/benchmark-harness/scripts/bench_local_profiles.sh"

fail() {
  echo "bench_local_profiles_test: $1" >&2
  exit 1
}

configure_benchmark_profile ""
[ "$BENCH_PROFILE_LABEL" = "default" ] || fail "default profile label"
[ "${BENCH_PROFILE_CARGO_ARGS[*]}" = "--features all" ] || fail "default Cargo arguments"

configure_benchmark_profile full
full_target="$BENCH_PROFILE_TARGET_DIR"
[ "${BENCH_PROFILE_CARGO_ARGS[*]}" = "--features all" ] || fail "full Cargo arguments"

configure_benchmark_profile pdf-heuristic
heuristic_target="$BENCH_PROFILE_TARGET_DIR"
[ "${BENCH_PROFILE_CARGO_ARGS[*]}" = "--no-default-features --features pdf-heuristic" ] \
  || fail "pdf-heuristic Cargo arguments"

configure_benchmark_profile pdf-ocr
[ "${BENCH_PROFILE_CARGO_ARGS[*]}" = "--no-default-features --features pdf-ocr" ] \
  || fail "pdf-ocr Cargo arguments"
[ "$full_target" != "$heuristic_target" ] || fail "full and heuristic targets overlap"
[ "$heuristic_target" != "$BENCH_PROFILE_TARGET_DIR" ] || fail "heuristic and OCR targets overlap"

if configure_benchmark_profile invalid >/dev/null 2>&1; then
  fail "invalid profile accepted"
fi

XBERG_BENCH_PROFILE=pdf-heuristic
configure_benchmark_profile "$XBERG_BENCH_PROFILE"
FRAMEWORKS_EXPLICIT=0
FRAMEWORKS=unchanged
OUT=results/local
apply_benchmark_profile_defaults
[ "$FRAMEWORKS" = "xberg-markdown-baseline,liteparse" ] || fail "lean framework defaults"
[ "$OUT" = "results/local/pdf-heuristic" ] || fail "profile output isolation"

OUT=custom-results
apply_benchmark_profile_defaults
[ "$OUT" = "custom-results/pdf-heuristic" ] || fail "explicit output profile isolation"

FRAMEWORKS_EXPLICIT=1
FRAMEWORKS=xberg-markdown-layout
OCR_FIXTURES=""
BATCH_OCR_FIXTURES=""
BATCH_FRAMEWORKS=""
if validate_benchmark_profile_inputs >/dev/null 2>&1; then
  fail "incompatible layout framework accepted"
fi

FRAMEWORKS=xberg-markdown-baseline
OCR_FIXTURES=ocr-fixtures
if validate_benchmark_profile_inputs >/dev/null 2>&1; then
  fail "OCR cohort accepted by heuristic profile"
fi

test_directory="$(mktemp -d "${TMPDIR:-/tmp}/xberg-bench-profile-test.XXXXXX")"
test_directory="$(cd "$test_directory" && pwd -P)"
cleanup() {
  rm -rf -- "$test_directory"
}
trap cleanup EXIT
original_repo_root="$REPO_ROOT"
REPO_ROOT="$test_directory/repository"
mkdir -p "$REPO_ROOT/target/release" "$REPO_ROOT/target/debug" "$test_directory/path-bin"
printf 'explicit' >"$test_directory/explicit-xberg"
printf 'release' >"$REPO_ROOT/target/release/xberg"
printf 'debug' >"$REPO_ROOT/target/debug/xberg"
printf 'path' >"$test_directory/path-bin/xberg"
chmod +x \
  "$test_directory/explicit-xberg" \
  "$REPO_ROOT/target/release/xberg" \
  "$REPO_ROOT/target/debug/xberg" \
  "$test_directory/path-bin/xberg"

XBERG_CLI_BINARY="$test_directory/explicit-xberg"
[ "$(resolve_default_xberg_binary)" = "$test_directory/explicit-xberg" ] || fail "explicit binary priority"
XBERG_CLI_BINARY="$test_directory/missing-explicit-xberg"
if resolve_default_xberg_binary >/dev/null 2>&1; then
  fail "invalid explicit binary fell through to release"
fi
unset XBERG_CLI_BINARY
[ "$(resolve_default_xberg_binary)" = "$REPO_ROOT/target/release/xberg" ] || fail "release binary priority"
mv "$REPO_ROOT/target/release/xberg" "$test_directory/release-xberg"
[ "$(resolve_default_xberg_binary)" = "$REPO_ROOT/target/debug/xberg" ] || fail "debug binary priority"
mv "$REPO_ROOT/target/debug/xberg" "$test_directory/debug-xberg"
PATH="$test_directory/path-bin:$PATH"
[ "$(resolve_default_xberg_binary)" = "$test_directory/path-bin/xberg" ] || fail "PATH binary fallback"
frameworks_include_xberg "liteparse,xberg-markdown-baseline" || fail "Xberg framework detection"
if frameworks_include_xberg "liteparse,docling"; then
  fail "non-Xberg framework false positive"
fi

XBERG_BENCH_PROFILE=""
configure_benchmark_profile ""
activate_xberg_profile
[ "$XBERG_CLI_BINARY" = "$test_directory/path-bin/xberg" ] || fail "default binary pinning"

fixture_root="$test_directory/fixtures"
mkdir -p "$fixture_root"
printf '%s\n' '{"metadata":{"requires_ocr":false}}' >"$fixture_root/heuristic.json"
printf '%s\n' '{"metadata":{"requires_ocr":true}}' >"$fixture_root/ocr.json"
printf '%s\n' '{"schema_version":1,"name":"heuristic","batch_size":1,"fixtures":["heuristic.json"]}' \
  >"$test_directory/heuristic-cohort.json"
printf '%s\n' '{"schema_version":1,"name":"ocr","batch_size":1,"fixtures":["ocr.json"]}' \
  >"$test_directory/ocr-cohort.json"
validate_ocr_cohort "$fixture_root" "$test_directory/heuristic-cohort.json" false
validate_ocr_cohort "$fixture_root" "$test_directory/ocr-cohort.json" true
if validate_ocr_cohort "$fixture_root" "" false >/dev/null 2>&1; then
  fail "legacy fixture-root validation ignored mixed OCR fixtures"
fi
if validate_ocr_cohort "$fixture_root" "$test_directory/ocr-cohort.json" false >/dev/null 2>&1; then
  fail "manifest-listed OCR mismatch accepted"
fi

REPO_ROOT="$original_repo_root"
BENCH_PROFILE_LABEL=pdf-ocr
BENCH_PROFILE_CARGO_FEATURES=pdf-ocr
BENCH_PROFILE_DEFAULT_FEATURES=false
BENCH_PROFILE_BINARY="$test_directory/xberg binary"
printf 'xberg' >"$BENCH_PROFILE_BINARY"
chmod +x "$BENCH_PROFILE_BINARY"
XBERG_BENCH_PROFILE=pdf-ocr
activate_xberg_profile
[ "$XBERG_CLI_BINARY" = "$BENCH_PROFILE_BINARY" ] || fail "explicit binary override"
write_benchmark_profile_provenance "$test_directory" "xberg-markdown-baseline"

python3 - "$test_directory/benchmark-profile.json" <<'PY'
import json
import pathlib
import sys

metadata = json.loads(pathlib.Path(sys.argv[1]).read_text(encoding="utf-8"))
assert metadata["profile"] == "pdf-ocr", metadata
assert metadata["cargo_features"] == ["pdf-ocr"], metadata
assert metadata["cargo_default_features"] is False, metadata
expected_path = str((pathlib.Path(sys.argv[1]).parent / "xberg binary").resolve())
assert metadata["binary_path"] == expected_path, (metadata["binary_path"], expected_path)
assert metadata["binary_sha256"] == "092b366007207df5bd1f13ae5bf5d0606ff781c99c104700ae8e14d4218732de", metadata
assert len(metadata["run_git_sha"]) == 40
assert len(metadata["run_git_worktree_sha256"]) == 64
assert len(metadata["run_identity_sha256"]) == 64
assert metadata["provenance_semantics"] == "binary-and-run-checkout"
PY

BENCH_PROFILE_BINARY="$test_directory/missing-xberg"
if write_benchmark_profile_provenance "$test_directory" "xberg-markdown-baseline" >/dev/null 2>&1; then
  fail "missing Xberg provenance binary accepted"
fi
write_benchmark_profile_provenance "$test_directory/no-xberg" "liteparse"

identity_repo="$test_directory/identity-repository"
mkdir -p "$identity_repo/crates/xberg-cli/src"
git -C "$identity_repo" init -q
git -C "$identity_repo" config user.email benchmark-test@example.invalid
git -C "$identity_repo" config user.name benchmark-test
printf '[workspace]\n' >"$identity_repo/Cargo.toml"
printf 'fn main() {}\n' >"$identity_repo/crates/xberg-cli/src/main.rs"
git -C "$identity_repo" add Cargo.toml crates/xberg-cli/src/main.rs
git -C "$identity_repo" commit -qm "test fixture"
REPO_ROOT="$identity_repo"
BENCH_PROFILE_BINARY="$identity_repo/xberg"
printf 'same binary bytes\n' >"$BENCH_PROFILE_BINARY"
chmod +x "$BENCH_PROFILE_BINARY"
first_worktree_hash="$(git_worktree_sha256)"
first_identity="$(run_identity_sha256 "$(binary_sha256 "$BENCH_PROFILE_BINARY")" \
  "$(git -C "$REPO_ROOT" rev-parse HEAD)" "$first_worktree_hash")"
printf 'untracked source state\n' >"$identity_repo/crates/xberg-cli/src/untracked.rs"
second_worktree_hash="$(git_worktree_sha256)"
second_identity="$(run_identity_sha256 "$(binary_sha256 "$BENCH_PROFILE_BINARY")" \
  "$(git -C "$REPO_ROOT" rev-parse HEAD)" "$second_worktree_hash")"
[ "$first_worktree_hash" != "$second_worktree_hash" ] || fail "untracked source omitted from worktree hash"
[ "$first_identity" != "$second_identity" ] || fail "differing run states share a run identity"

printf 'mode-sensitive\n' >"$identity_repo/mode-sensitive"
chmod 0644 "$identity_repo/mode-sensitive"
non_executable_hash="$(git_worktree_sha256)"
chmod 0755 "$identity_repo/mode-sensitive"
[ "$(git_worktree_sha256)" != "$non_executable_hash" ] \
  || fail "untracked executable mode omitted from worktree hash"
rm "$identity_repo/mode-sensitive"

exclude_file="$test_directory/excludes"
printf '%s\n' 'excluded-by-config-*' >"$exclude_file"
printf 'environment one\n' >"$identity_repo/excluded-by-config-environment"
environment_excluded_hash="$(
  GIT_CONFIG_COUNT=1 \
    GIT_CONFIG_KEY_0=core.excludesFile \
    GIT_CONFIG_VALUE_0="$exclude_file" \
    git_worktree_sha256
)"
printf 'environment two\n' >"$identity_repo/excluded-by-config-environment"
[ "$(
  GIT_CONFIG_COUNT=1 \
    GIT_CONFIG_KEY_0=core.excludesFile \
    GIT_CONFIG_VALUE_0="$exclude_file" \
    git_worktree_sha256
)" != "$environment_excluded_hash" ] || fail "environment Git excludes suppressed untracked content"

git -C "$identity_repo" config core.excludesFile "$exclude_file"
printf 'local one\n' >"$identity_repo/excluded-by-config-local"
local_excluded_hash="$(git_worktree_sha256)"
printf 'local two\n' >"$identity_repo/excluded-by-config-local"
[ "$(git_worktree_sha256)" != "$local_excluded_hash" ] || fail "local Git excludes suppressed untracked content"

helper="$test_directory/forbidden-git-helper"
helper_sentinel="$test_directory/forbidden-git-helper-ran"
cat >"$helper" <<'SH'
#!/usr/bin/env sh
: >"$PROVENANCE_HELPER_SENTINEL"
exit 1
SH
chmod +x "$helper"
export PROVENANCE_HELPER_SENTINEL="$helper_sentinel"
printf '%s\n' '*.rs filter=forbidden' >"$identity_repo/.gitattributes"
git -C "$identity_repo" config diff.external "$helper"
git -C "$identity_repo" config filter.forbidden.clean "$helper"
git -C "$identity_repo" config core.fsmonitor "$helper"
printf 'fn main() { /* changed */ }\n' >"$identity_repo/crates/xberg-cli/src/main.rs"
git_worktree_sha256 >/dev/null
[ ! -e "$helper_sentinel" ] || fail "worktree hashing executed a configured Git helper"

external_target="$test_directory/external-symlink-target"
printf 'external one\n' >"$external_target"
ln -s "$external_target" "$identity_repo/untracked-link"
symlink_hash="$(git_worktree_sha256)"
printf 'external two\n' >"$external_target"
[ "$(git_worktree_sha256)" = "$symlink_hash" ] || fail "worktree hashing followed an untracked symlink"
rm "$identity_repo/untracked-link"
ln -s "$test_directory/other-target" "$identity_repo/untracked-link"
[ "$(git_worktree_sha256)" != "$symlink_hash" ] || fail "untracked symlink target omitted from worktree hash"
rm "$identity_repo/untracked-link"

mkfifo "$identity_repo/untracked-fifo"
if git_worktree_sha256 >/dev/null 2>&1; then
  fail "unsupported untracked FIFO accepted"
fi
rm "$identity_repo/untracked-fifo"

leaf_repo="$test_directory/leaf-repository"
mkdir -p "$leaf_repo"
git -C "$leaf_repo" init -q
git -C "$leaf_repo" config user.email benchmark-test@example.invalid
git -C "$leaf_repo" config user.name benchmark-test
printf 'tracked leaf\n' >"$leaf_repo/tracked.txt"
git -C "$leaf_repo" add tracked.txt
git -C "$leaf_repo" commit -qm "leaf fixture"

middle_repo="$test_directory/middle-repository"
mkdir -p "$middle_repo"
git -C "$middle_repo" init -q
git -C "$middle_repo" config user.email benchmark-test@example.invalid
git -C "$middle_repo" config user.name benchmark-test
printf 'tracked middle\n' >"$middle_repo/tracked.txt"
git -C "$middle_repo" add tracked.txt
git -C "$middle_repo" commit -qm "middle fixture"
git -C "$middle_repo" -c protocol.file.allow=always submodule add -q "$leaf_repo" nested/leaf
git -C "$middle_repo" commit -qam "add nested submodule"

super_repo="$test_directory/super-repository"
mkdir -p "$super_repo"
git -C "$super_repo" init -q
git -C "$super_repo" config user.email benchmark-test@example.invalid
git -C "$super_repo" config user.name benchmark-test
printf 'tracked super\n' >"$super_repo/tracked.txt"
git -C "$super_repo" add tracked.txt
git -C "$super_repo" commit -qm "super fixture"
git -C "$super_repo" -c protocol.file.allow=always submodule add -q "$middle_repo" nested/middle
git -C "$super_repo" commit -qam "add middle submodule"
git -C "$super_repo" -c protocol.file.allow=always submodule update -q --init --recursive
REPO_ROOT="$super_repo"
clean_submodule_hash="$(git_worktree_sha256)"
printf 'nested untracked\n' >"$super_repo/nested/middle/nested/leaf/untracked.txt"
[ "$(git_worktree_sha256)" != "$clean_submodule_hash" ] \
  || fail "recursive submodule untracked content omitted from worktree hash"
rm "$super_repo/nested/middle/nested/leaf/untracked.txt"
middle_head="$(git -C "$super_repo/nested/middle" rev-parse HEAD)"
git -C "$super_repo" update-index --force-remove nested/middle
printf '160000 %s 1\tnested/middle\n160000 %s 2\tnested/middle\n160000 %s 3\tnested/middle\n' \
  "$middle_head" "$middle_head" "$middle_head" | git -C "$super_repo" update-index --index-info
conflicted_submodule_hash="$(git_worktree_sha256)"
printf 'conflicted submodule payload\n' >"$super_repo/nested/middle/untracked-payload"
[ "$(git_worktree_sha256)" != "$conflicted_submodule_hash" ] \
  || fail "conflicted submodule worktree content omitted from worktree hash"
rm "$super_repo/nested/middle/untracked-payload"
git -C "$super_repo" update-index --force-remove nested/middle
printf '160000 %s 0\tnested/middle\n' "$middle_head" | git -C "$super_repo" update-index --index-info
mv "$super_repo/nested/middle" "$test_directory/saved-middle-submodule"
uninitialized_submodule_hash="$(git_worktree_sha256)"
[ -n "$uninitialized_submodule_hash" ] || fail "uninitialized submodule state could not be hashed"
[ "$uninitialized_submodule_hash" != "$clean_submodule_hash" ] \
  || fail "uninitialized submodule state omitted from worktree hash"
mkdir -p "$super_repo/nested/middle"
printf 'unhashed payload\n' >"$super_repo/nested/middle/payload"
if git_worktree_sha256 >/dev/null 2>&1; then
  fail "uninitialized submodule payload accepted without hashing"
fi
printf 'gitdir: missing-git-directory\n' >"$super_repo/nested/middle/.git"
if git_worktree_sha256 >/dev/null 2>&1; then
  fail "corrupt submodule Git metadata accepted with unhashed payload"
fi

echo "bench_local_profiles_test: passed"
