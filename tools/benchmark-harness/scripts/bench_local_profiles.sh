#!/usr/bin/env bash

configure_benchmark_profile() {
  local profile="${1:-}"

  case "$profile" in
  "")
    BENCH_PROFILE_LABEL="default"
    BENCH_PROFILE_CARGO_FEATURES="all"
    BENCH_PROFILE_DEFAULT_FEATURES=true
    BENCH_PROFILE_TARGET_DIR="$REPO_ROOT/target"
    BENCH_PROFILE_BINARY=""
    BENCH_PROFILE_CARGO_ARGS=(--features all)
    ;;
  full)
    BENCH_PROFILE_LABEL="full"
    BENCH_PROFILE_CARGO_FEATURES="all"
    BENCH_PROFILE_DEFAULT_FEATURES=true
    BENCH_PROFILE_TARGET_DIR="$REPO_ROOT/target/benchmark-profiles/full"
    BENCH_PROFILE_BINARY="$BENCH_PROFILE_TARGET_DIR/release/xberg"
    BENCH_PROFILE_CARGO_ARGS=(--features all)
    ;;
  pdf-heuristic | pdf-ocr)
    BENCH_PROFILE_LABEL="$profile"
    BENCH_PROFILE_CARGO_FEATURES="$profile"
    BENCH_PROFILE_DEFAULT_FEATURES=false
    BENCH_PROFILE_TARGET_DIR="$REPO_ROOT/target/benchmark-profiles/$profile"
    BENCH_PROFILE_BINARY="$BENCH_PROFILE_TARGET_DIR/release/xberg"
    BENCH_PROFILE_CARGO_ARGS=(--no-default-features --features "$profile")
    ;;
  *)
    echo "[bench:local] unsupported XBERG_BENCH_PROFILE: $profile" >&2
    echo "[bench:local] expected one of: full, pdf-heuristic, pdf-ocr" >&2
    return 1
    ;;
  esac
}

apply_benchmark_profile_defaults() {
  case "$BENCH_PROFILE_LABEL" in
  pdf-heuristic | pdf-ocr)
    if [ "$FRAMEWORKS_EXPLICIT" = 0 ]; then
      FRAMEWORKS="xberg-markdown-baseline,liteparse"
    fi
    ;;
  esac

  if [ -n "${XBERG_BENCH_PROFILE:-}" ]; then
    OUT="$OUT/$BENCH_PROFILE_LABEL"
  fi
}

canonical_executable() {
  local candidate="$1"
  [ -x "$candidate" ] || return 1
  python3 - "$candidate" <<'PY'
import pathlib
import sys

print(pathlib.Path(sys.argv[1]).resolve(strict=True))
PY
}

resolve_default_xberg_binary() {
  local candidate resolved

  if [ -n "${XBERG_CLI_BINARY:-}" ]; then
    candidate="$XBERG_CLI_BINARY"
    resolved="$(command -v "$candidate" 2>/dev/null || true)"
    if [ -z "$resolved" ] || ! resolved="$(canonical_executable "$resolved")"; then
      echo "[bench:local] XBERG_CLI_BINARY is not executable: $candidate" >&2
      return 1
    fi
    printf '%s\n' "$resolved"
    return
  fi

  for candidate in "$REPO_ROOT/target/release/xberg" "$REPO_ROOT/target/debug/xberg"; do
    if resolved="$(canonical_executable "$candidate" 2>/dev/null)"; then
      printf '%s\n' "$resolved"
      return
    fi
  done

  resolved="$(command -v xberg 2>/dev/null || true)"
  if [ -n "$resolved" ] && resolved="$(canonical_executable "$resolved")"; then
    printf '%s\n' "$resolved"
    return
  fi

  echo "[bench:local] xberg CLI not found (checked XBERG_CLI_BINARY, target/release, target/debug, and PATH)." >&2
  return 1
}

frameworks_include_xberg() {
  local framework remaining="$1"

  while [ -n "$remaining" ]; do
    case "$remaining" in
    *,*)
      framework="${remaining%%,*}"
      remaining="${remaining#*,}"
      ;;
    *)
      framework="$remaining"
      remaining=""
      ;;
    esac
    case "$framework" in
    xberg-*) return 0 ;;
    esac
  done
  return 1
}

validate_benchmark_profile_inputs() {
  local framework remaining

  if [ "$BENCH_PROFILE_LABEL" = "pdf-heuristic" ] \
    && { [ -n "$OCR_FIXTURES" ] || [ -n "$BATCH_OCR_FIXTURES" ]; }; then
    echo "[bench:local] pdf-heuristic cannot run OCR cohorts; use XBERG_BENCH_PROFILE=pdf-ocr." >&2
    return 1
  fi

  case "$BENCH_PROFILE_LABEL" in
  pdf-heuristic | pdf-ocr) ;;
  *) return ;;
  esac

  remaining="$FRAMEWORKS${BATCH_FRAMEWORKS:+,$BATCH_FRAMEWORKS}"
  while [ -n "$remaining" ]; do
    case "$remaining" in
    *,*)
      framework="${remaining%%,*}"
      remaining="${remaining#*,}"
      ;;
    *)
      framework="$remaining"
      remaining=""
      ;;
    esac
    case "$framework" in
    xberg-markdown-baseline | xberg-markdown-baseline-batch | "") ;;
    xberg-*)
      echo "[bench:local] profile '$BENCH_PROFILE_LABEL' does not support framework '$framework'." >&2
      echo "[bench:local] lean PDF profiles support the Xberg baseline pipeline only." >&2
      return 1
      ;;
    esac
  done
}

build_xberg_profile() {
  echo "[bench:local] Building xberg CLI profile '$BENCH_PROFILE_LABEL' in $BENCH_PROFILE_TARGET_DIR…"
  CARGO_TARGET_DIR="$BENCH_PROFILE_TARGET_DIR" \
    cargo build --locked --release -p xberg-cli "${BENCH_PROFILE_CARGO_ARGS[@]}"
}

activate_xberg_profile() {
  local resolved

  if [ -z "${XBERG_BENCH_PROFILE:-}" ]; then
    resolved="$(resolve_default_xberg_binary)" || return 1
    BENCH_PROFILE_BINARY="$resolved"
    export XBERG_CLI_BINARY="$resolved"
    return
  fi

  if [ ! -x "$BENCH_PROFILE_BINARY" ]; then
    echo "[bench:local] xberg profile binary is not executable: $BENCH_PROFILE_BINARY" >&2
    if [ "${SKIP_BUILD:-0}" = "1" ]; then
      echo "[bench:local] unset SKIP_BUILD or build XBERG_BENCH_PROFILE=$BENCH_PROFILE_LABEL first." >&2
    fi
    return 1
  fi

  resolved="$(canonical_executable "$BENCH_PROFILE_BINARY")" || return 1
  BENCH_PROFILE_BINARY="$resolved"
  export XBERG_CLI_BINARY="$resolved"
}

binary_sha256() {
  local binary="$1"
  python3 - "$binary" <<'PY'
import hashlib
import pathlib
import sys

digest = hashlib.sha256()
with pathlib.Path(sys.argv[1]).open("rb") as binary:
    for chunk in iter(lambda: binary.read(1024 * 1024), b""):
        digest.update(chunk)
print(digest.hexdigest())
PY
}

git_worktree_sha256() {
  python3 - "$REPO_ROOT" <<'PY'
import hashlib
import os
import pathlib
import stat
import subprocess
import sys

root = pathlib.Path(sys.argv[1]).resolve(strict=True)
digest = hashlib.sha256()
git_environment = os.environ.copy()
for key in list(git_environment):
    if key.startswith("GIT_"):
        del git_environment[key]
git_environment.update({
    "GIT_ATTR_NOSYSTEM": "1",
    "GIT_CONFIG_NOSYSTEM": "1",
    "GIT_CONFIG_GLOBAL": os.devnull,
    "GIT_NO_REPLACE_OBJECTS": "1",
    "LC_ALL": "C",
})
safe_git_config = [
    "-c", "core.excludesFile=" + os.devnull,
    "-c", "core.fsmonitor=false",
    "-c", "core.ignoreCase=false",
    "-c", "core.precomposeUnicode=false",
    "-c", "core.sparseCheckout=false",
    "-c", "core.sparseCheckoutCone=false",
    "-c", "core.untrackedCache=false",
    "-c", "diff.external=",
]

def update(value: bytes) -> None:
    digest.update(len(value).to_bytes(8, "little"))
    digest.update(value)

def git(repository: pathlib.Path, *arguments: str) -> bytes:
    return subprocess.run(
        ["git", "-C", str(repository), *safe_git_config, *arguments],
        check=True,
        env=git_environment,
        stdout=subprocess.PIPE,
    ).stdout

def decode_path(relative_bytes: bytes) -> pathlib.Path:
    relative_path = pathlib.Path(relative_bytes.decode("utf-8", "surrogateescape"))
    if relative_path.is_absolute() or ".." in relative_path.parts:
        raise SystemExit(f"unsafe Git path in worktree: {relative_path!s}")
    return relative_path

def canonical_regular_mode(metadata: os.stat_result) -> bytes:
    return b"100755" if metadata.st_mode & stat.S_IXUSR else b"100644"

def hash_regular_file(path: pathlib.Path, metadata: os.stat_result) -> None:
    update(canonical_regular_mode(metadata))
    update(metadata.st_size.to_bytes(8, "little"))
    descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW)
    with os.fdopen(descriptor, "rb") as source:
        opened_metadata = os.fstat(source.fileno())
        if (
            not stat.S_ISREG(opened_metadata.st_mode)
            or opened_metadata.st_dev != metadata.st_dev
            or opened_metadata.st_ino != metadata.st_ino
            or opened_metadata.st_size != metadata.st_size
            or canonical_regular_mode(opened_metadata) != canonical_regular_mode(metadata)
        ):
            raise SystemExit(f"file type changed while hashing: {path!s}")
        bytes_read = 0
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            bytes_read += len(chunk)
            digest.update(chunk)
        final_metadata = os.fstat(source.fileno())
        if (
            bytes_read != opened_metadata.st_size
            or final_metadata.st_dev != opened_metadata.st_dev
            or final_metadata.st_ino != opened_metadata.st_ino
            or final_metadata.st_size != opened_metadata.st_size
            or final_metadata.st_mtime_ns != opened_metadata.st_mtime_ns
            or final_metadata.st_ctime_ns != opened_metadata.st_ctime_ns
            or canonical_regular_mode(final_metadata) != canonical_regular_mode(opened_metadata)
        ):
            raise SystemExit(f"file changed while hashing: {path!s}")

def hash_path(path: pathlib.Path, relative_bytes: bytes, category: bytes) -> None:
    try:
        metadata = path.lstat()
    except FileNotFoundError:
        update(category + b"-missing")
        update(relative_bytes)
        return
    update(category)
    update(relative_bytes)
    if stat.S_ISREG(metadata.st_mode):
        update(b"regular")
        hash_regular_file(path, metadata)
    elif stat.S_ISLNK(metadata.st_mode):
        update(b"symlink")
        update(os.readlink(path).encode("utf-8", "surrogateescape"))
    else:
        raise SystemExit(f"unsupported worktree file type: {decode_path(relative_bytes)!s}")

def hash_untracked(repository: pathlib.Path) -> None:
    output = git(
        repository,
        "ls-files",
        "--others",
        "--exclude-per-directory=.gitignore",
        "-z",
    )
    for relative_bytes in sorted(path for path in output.split(b"\0") if path):
        relative_path = decode_path(relative_bytes)
        hash_path(repository / relative_path, relative_bytes, b"untracked")

def reject_special_worktree_files(repository: pathlib.Path, submodules: set[pathlib.Path]) -> None:
    ignored_output = git(
        repository,
        "ls-files",
        "--others",
        "--ignored",
        "--exclude-per-directory=.gitignore",
        "--directory",
        "-z",
    )
    ignored = {
        decode_path(record.rstrip(b"/"))
        for record in ignored_output.split(b"\0")
        if record
    }

    def scan(directory: pathlib.Path, relative_directory: pathlib.Path) -> None:
        with os.scandir(directory) as entries:
            for entry in entries:
                relative_path = relative_directory / entry.name
                if (
                    entry.name == ".git"
                    or any(ignored_path == relative_path or ignored_path in relative_path.parents for ignored_path in ignored)
                    or relative_path in submodules
                ):
                    continue
                try:
                    if entry.is_dir(follow_symlinks=False):
                        scan(pathlib.Path(entry.path), relative_path)
                    elif not entry.is_file(follow_symlinks=False) and not entry.is_symlink():
                        raise SystemExit(f"unsupported worktree file type: {relative_path!s}")
                except FileNotFoundError:
                    continue

    scan(repository, pathlib.Path())

def index_entries(repository: pathlib.Path) -> list[tuple[bytes, bytes, bytes, bytes]]:
    output = git(repository, "ls-files", "--stage", "-z")
    entries = []
    for record in output.split(b"\0"):
        if not record:
            continue
        metadata, separator, relative_bytes = record.partition(b"\t")
        fields = metadata.split()
        if separator and len(fields) == 3:
            entries.append((relative_bytes, fields[0], fields[1], fields[2]))
    return sorted(entries)

def hash_repository(repository: pathlib.Path) -> None:
    entries = index_entries(repository)
    submodules = sorted({
        relative_bytes
        for relative_bytes, mode, _object_id, _stage in entries
        if mode == b"160000"
    })
    reject_special_worktree_files(repository, {decode_path(path) for path in submodules})
    update(b"repository")
    tracked_paths = set()
    for relative_bytes, mode, object_id, stage in entries:
        update(b"index")
        update(relative_bytes)
        update(mode)
        update(object_id)
        update(stage)
        if mode != b"160000":
            tracked_paths.add(relative_bytes)
    for relative_bytes in sorted(tracked_paths):
        hash_path(repository / decode_path(relative_bytes), relative_bytes, b"tracked")
    hash_untracked(repository)

    for relative_bytes in submodules:
        relative_path = decode_path(relative_bytes)
        submodule = repository / relative_path
        update(b"submodule")
        update(relative_bytes)
        try:
            metadata = submodule.lstat()
        except FileNotFoundError:
            update(b"uninitialized")
            continue
        if stat.S_ISLNK(metadata.st_mode) or not stat.S_ISDIR(metadata.st_mode):
            raise SystemExit(f"unsafe submodule worktree type: {relative_path!s}")
        git_marker = submodule / ".git"
        try:
            git_metadata = git_marker.lstat()
        except FileNotFoundError:
            try:
                next(submodule.iterdir())
            except StopIteration:
                update(b"uninitialized-empty")
                continue
            except FileNotFoundError:
                update(b"uninitialized-missing")
                continue
            raise SystemExit(f"uninitialized submodule contains unhashed files: {relative_path!s}")
        if stat.S_ISLNK(git_metadata.st_mode) or not (
            stat.S_ISREG(git_metadata.st_mode) or stat.S_ISDIR(git_metadata.st_mode)
        ):
            raise SystemExit(f"unsafe submodule Git metadata type: {relative_path!s}")
        try:
            top_level = pathlib.Path(
                git(submodule, "rev-parse", "--show-toplevel").decode("utf-8", "surrogateescape").strip()
            ).resolve(strict=True)
        except (subprocess.CalledProcessError, FileNotFoundError) as error:
            raise SystemExit(f"invalid submodule Git metadata: {relative_path!s}") from error
        if top_level != submodule.resolve(strict=True):
            raise SystemExit(f"submodule path resolves outside its worktree: {relative_path!s}")
        update(git(submodule, "rev-parse", "HEAD").strip())
        hash_repository(submodule)

hash_repository(root)
print(digest.hexdigest())
PY
}

git_head_sha() {
  python3 - "$REPO_ROOT" <<'PY'
import os
import pathlib
import subprocess
import sys

root = pathlib.Path(sys.argv[1]).resolve(strict=True)
environment = os.environ.copy()
for key in list(environment):
    if key.startswith("GIT_"):
        del environment[key]
environment.update({
    "GIT_CONFIG_NOSYSTEM": "1",
    "GIT_CONFIG_GLOBAL": os.devnull,
    "GIT_NO_REPLACE_OBJECTS": "1",
    "LC_ALL": "C",
})
result = subprocess.run(
    ["git", "-C", str(root), "-c", "core.fsmonitor=false", "rev-parse", "HEAD"],
    check=True,
    env=environment,
    stdout=subprocess.PIPE,
).stdout
print(result.decode("ascii").strip())
PY
}

run_identity_sha256() {
  local binary_hash="$1"
  local git_sha="$2"
  local worktree_hash="$3"
  python3 - \
    "$binary_hash" \
    "$git_sha" \
    "$worktree_hash" \
    "$BENCH_PROFILE_LABEL" \
    "$BENCH_PROFILE_CARGO_FEATURES" \
    "$BENCH_PROFILE_DEFAULT_FEATURES" <<'PY'
import hashlib
import sys

digest = hashlib.sha256()
for value in sys.argv[1:]:
    encoded = value.encode("utf-8")
    digest.update(len(encoded).to_bytes(8, "little"))
    digest.update(encoded)
print(digest.hexdigest())
PY
}

validate_ocr_cohort() {
  local fixture_root="$1"
  local manifest="$2"
  local expected="$3"
  python3 - "$fixture_root" "$manifest" "$expected" <<'PY'
import json
import pathlib
import sys

root = pathlib.Path(sys.argv[1])
manifest_path = pathlib.Path(sys.argv[2]) if sys.argv[2] else None
expected = sys.argv[3] == "true"
image_types = {"png", "jpg", "jpeg", "gif", "bmp", "tiff", "tif", "webp", "jp2", "jpx", "jpm", "mj2"}
if manifest_path is not None:
    try:
        manifest_data = json.loads(manifest_path.read_text(encoding="utf-8"))
        listed_fixtures = manifest_data["fixtures"]
        if not isinstance(listed_fixtures, list) or not listed_fixtures:
            raise ValueError("fixtures must be a non-empty list")
        relative_fixtures = [pathlib.Path(item) for item in listed_fixtures]
        if any(path.is_absolute() or ".." in path.parts for path in relative_fixtures):
            raise ValueError("fixtures must contain normalized relative paths")
        fixture_paths = [root / path for path in relative_fixtures]
    except (OSError, UnicodeError, json.JSONDecodeError, KeyError, TypeError, ValueError) as error:
        raise SystemExit(f"invalid cohort manifest {manifest_path}: {error}") from error
else:
    fixture_paths = sorted(root.rglob("*.json")) if root.is_dir() else [root]

bad = []
for path in fixture_paths:
    try:
        fixture = json.loads(path.read_text(encoding="utf-8"))
        metadata_value = fixture.get("metadata", {}).get("requires_ocr")
        if isinstance(metadata_value, bool):
            requires_ocr = metadata_value
        else:
            file_type = str(fixture.get("file_type", "")).lower()
            document_type = pathlib.Path(str(fixture.get("document", ""))).suffix.lstrip(".").lower()
            requires_ocr = file_type in image_types or document_type in image_types
        if requires_ocr != expected:
            bad.append(str(path))
    except (OSError, UnicodeError, json.JSONDecodeError, AttributeError) as error:
        bad.append(f"{path} ({error})")

if not fixture_paths:
    raise SystemExit(f"cohort contains no fixture JSON files: {root}")
if bad:
    label = "OCR-required" if expected else "non-OCR"
    preview = "\n  - ".join(bad[:10])
    raise SystemExit(f"cohort must contain only {label} fixtures; mismatches:\n  - {preview}")
PY
}

write_benchmark_profile_provenance() {
  local output="$1"
  local frameworks="$2"
  local binary_hash git_sha run_identity_hash worktree_hash

  frameworks_include_xberg "$frameworks" || return 0

  if [ ! -x "$BENCH_PROFILE_BINARY" ]; then
    echo "[bench:local] cannot record Xberg provenance; binary is not executable: $BENCH_PROFILE_BINARY" >&2
    return 1
  fi

  if ! binary_hash="$(binary_sha256 "$BENCH_PROFILE_BINARY")" || [ -z "$binary_hash" ]; then
    echo "[bench:local] failed to hash Xberg binary: $BENCH_PROFILE_BINARY" >&2
    return 1
  fi
  worktree_hash="$(git_worktree_sha256)"
  git_sha="$(git_head_sha)"
  run_identity_hash="$(run_identity_sha256 "$binary_hash" "$git_sha" "$worktree_hash")"
  python3 - \
    "$output/benchmark-profile.json" \
    "$BENCH_PROFILE_LABEL" \
    "$BENCH_PROFILE_CARGO_FEATURES" \
    "$BENCH_PROFILE_DEFAULT_FEATURES" \
    "$BENCH_PROFILE_BINARY" \
    "$binary_hash" \
    "$git_sha" \
    "$worktree_hash" \
    "$run_identity_hash" <<'PY'
import json
import pathlib
import sys

path = pathlib.Path(sys.argv[1])
metadata = {
    "profile": sys.argv[2],
    "cargo_features": sys.argv[3].split(","),
    "cargo_default_features": sys.argv[4] == "true",
    "binary_path": sys.argv[5],
    "binary_sha256": sys.argv[6],
    "run_git_sha": sys.argv[7],
    "run_git_worktree_sha256": sys.argv[8],
    "run_identity_sha256": sys.argv[9],
    "provenance_semantics": "binary-and-run-checkout",
}
path.write_text(json.dumps(metadata, indent=2, sort_keys=True) + "\n", encoding="utf-8")
PY
}
