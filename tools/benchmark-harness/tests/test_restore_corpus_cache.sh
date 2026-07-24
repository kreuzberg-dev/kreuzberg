#!/usr/bin/env bash

set -euo pipefail

SOURCE_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
TEMPORARY_ROOT="$(mktemp -d)"
trap 'rm -rf "$TEMPORARY_ROOT"' EXIT

fail() {
  echo "test_restore_corpus_cache: $*" >&2
  exit 1
}

assert_equals() {
  local expected="$1"
  local actual="$2"
  local description="$3"
  if [ "$actual" != "$expected" ]; then
    fail "${description}: expected '${expected}', got '${actual}'"
  fi
}

assert_not_equals() {
  local unexpected="$1"
  local actual="$2"
  local description="$3"
  if [ "$actual" = "$unexpected" ]; then
    fail "${description}: did not expect '${unexpected}'"
  fi
}

assert_contains() {
  local expected="$1"
  local actual="$2"
  local description="$3"
  case "$actual" in
  *"$expected"*) ;;
  *) fail "${description}: output did not contain '${expected}'" ;;
  esac
}

assert_not_contains() {
  local unexpected="$1"
  local actual="$2"
  local description="$3"
  case "$actual" in
  *"$unexpected"*) fail "${description}: output contained '${unexpected}'" ;;
  *) ;;
  esac
}

assert_file_content() {
  local expected="$1"
  local path="$2"
  local description="$3"
  [ -f "$path" ] || fail "${description}: missing file ${path}"
  assert_equals "$expected" "$(cat "$path")" "$description"
}

write_manifest() {
  local repository_root="$1"
  local pdf_content="$2"
  local markdown_content="$3"
  local text_content="$4"
  python3 - "$repository_root" "$pdf_content" "$markdown_content" "$text_content" <<'PY'
import hashlib
import json
import sys
from pathlib import Path

root = Path(sys.argv[1])
pdf, markdown, text = (value.encode() for value in sys.argv[2:])
manifest = {
    "documents": [
        {
            "id": "reference",
            "redistribute": "reference",
            "gate_verdict": "ACCEPT",
            "size_tier": "core",
            "pdf_sha256": hashlib.sha256(pdf).hexdigest(),
            "out_gt_md_sha256": hashlib.sha256(markdown).hexdigest(),
            "gt_txt_sha256": hashlib.sha256(text).hexdigest(),
        }
    ]
}
path = root / "test_documents/ground_truth/corpus_manifest.json"
path.parent.mkdir(parents=True, exist_ok=True)
path.write_text(json.dumps(manifest), encoding="utf-8")
PY
}

create_archive() {
  local archive_path="$1"
  local pdf_content="$2"
  local markdown_content="$3"
  local text_content="$4"
  local extra_member="${5:-}"
  python3 - "$archive_path" "$pdf_content" "$markdown_content" "$text_content" "$extra_member" <<'PY'
import io
import sys
import tarfile
from pathlib import Path

archive_path = Path(sys.argv[1])
archive_path.parent.mkdir(parents=True, exist_ok=True)
members = {
    ".corpus-cache/pdf/reference.pdf": sys.argv[2].encode(),
    ".corpus-cache/ground_truth/pdf/reference.md": sys.argv[3].encode(),
    ".corpus-cache/ground_truth/pdf/reference.txt": sys.argv[4].encode(),
}
if sys.argv[5]:
    members[sys.argv[5]] = b"extra-metadata"
with tarfile.open(archive_path, "w") as archive:
    for name, content in members.items():
        member = tarfile.TarInfo(name)
        member.size = len(content)
        archive.addfile(member, io.BytesIO(content))
PY
}

setup_fixture() {
  local name="$1"
  local fixture_root="${TEMPORARY_ROOT}/${name}"

  mkdir -p \
    "${fixture_root}/scripts/benchmarks" \
    "${fixture_root}/scripts/lib" \
    "${fixture_root}/tools/benchmark-harness/scripts" \
    "${fixture_root}/fake-bin" \
    "${fixture_root}/fake-gcs" \
    "${fixture_root}/test_documents"
  cp "${SOURCE_ROOT}/Cargo.toml" "${fixture_root}/Cargo.toml"
  cp "${SOURCE_ROOT}/scripts/benchmarks/restore-corpus-cache.sh" \
    "${fixture_root}/scripts/benchmarks/restore-corpus-cache.sh"
  cp "${SOURCE_ROOT}/scripts/lib/common.sh" "${fixture_root}/scripts/lib/common.sh"
  cp "${SOURCE_ROOT}/tools/benchmark-harness/scripts/corpus_cache_manifest.py" \
    "${fixture_root}/tools/benchmark-harness/scripts/corpus_cache_manifest.py"

  cat >"${fixture_root}/fake-bin/gcloud" <<'SH'
#!/usr/bin/env bash
set -euo pipefail

printf '%s\n' "$*" >>"$FAKE_GCLOUD_LOG"
if [ "$1 $2 $3" = "storage objects describe" ]; then
  object="$4"
  if [ "${FAKE_GCLOUD_DESCRIBE_ERROR:-}" = "1" ]; then
    echo "ERROR: permission denied" >&2
    exit 1
  fi
  if [ ! -f "${FAKE_GCS_ROOT}/${object#gs://}" ]; then
    echo "ERROR: object not found: 404" >&2
    exit 1
  fi
elif [ "$1 $2" = "storage cp" ]; then
  object="$3"
  if [ "${FAKE_GCLOUD_INTERRUPT:-}" = "1" ]; then
    kill -TERM "$PPID"
    exit 143
  fi
  cp "${FAKE_GCS_ROOT}/${object#gs://}" "$4"
else
  echo "unexpected gcloud invocation: $*" >&2
  exit 64
fi
SH
  cat >"${fixture_root}/fake-bin/zstd" <<'SH'
#!/usr/bin/env bash
set -euo pipefail

[ "$1" = "-dc" ] || {
  echo "unexpected zstd invocation: $*" >&2
  exit 64
}
exec /bin/cat "$2"
SH
  chmod +x \
    "${fixture_root}/fake-bin/gcloud" \
    "${fixture_root}/fake-bin/zstd" \
    "${fixture_root}/scripts/benchmarks/restore-corpus-cache.sh"

  git -C "${fixture_root}/test_documents" init -q -b main
  git -C "${fixture_root}/test_documents" config user.email "cache-test@example.invalid"
  git -C "${fixture_root}/test_documents" config user.name "Corpus Cache Test"
  FIXTURE_ROOT="$fixture_root"
}

commit_documents() {
  local fixture_root="$1"
  local message="$2"
  git -C "${fixture_root}/test_documents" add .
  git -C "${fixture_root}/test_documents" commit -q -m "$message"
}

seed_existing_cache() {
  local fixture_root="$1"
  mkdir -p \
    "${fixture_root}/test_documents/.corpus-cache/pdf" \
    "${fixture_root}/test_documents/.corpus-cache/ground_truth/pdf" \
    "${fixture_root}/test_documents/.corpus-cache/model-store"
  printf 'old-pdf' >"${fixture_root}/test_documents/.corpus-cache/pdf/reference.pdf"
  printf 'old-markdown' >"${fixture_root}/test_documents/.corpus-cache/ground_truth/pdf/reference.md"
  printf 'old-text' >"${fixture_root}/test_documents/.corpus-cache/ground_truth/pdf/reference.txt"
  printf 'preserve-me' >"${fixture_root}/test_documents/.corpus-cache/model-store/model.bin"
}

run_restore() {
  local fixture_root="$1"
  local log_path="${fixture_root}/gcloud.log"
  : >"$log_path"
  set +e
  RUN_OUTPUT="$(
    PATH="${fixture_root}/fake-bin:${PATH}" \
      GCP_BENCHMARK_BUCKET="test-bucket" \
      FAKE_GCS_ROOT="${fixture_root}/fake-gcs" \
      FAKE_GCLOUD_LOG="$log_path" \
      FAKE_GCLOUD_INTERRUPT="${FAKE_GCLOUD_INTERRUPT:-}" \
      FAKE_GCLOUD_DESCRIBE_ERROR="${FAKE_GCLOUD_DESCRIBE_ERROR:-}" \
      bash "${fixture_root}/scripts/benchmarks/restore-corpus-cache.sh" 2>&1
  )"
  RUN_STATUS=$?
  set -e
}

cache_digest() {
  local fixture_root="$1"
  python3 "${fixture_root}/tools/benchmark-harness/scripts/corpus_cache_manifest.py" \
    digest \
    --manifest "${fixture_root}/test_documents/ground_truth/corpus_manifest.json"
}

cache_fingerprint() {
  local cache_root="$1"
  python3 - "$cache_root" <<'PY'
import hashlib
import sys
from pathlib import Path

root = Path(sys.argv[1])
digest = hashlib.sha256()
for path in sorted(root.rglob("*")):
    relative_path = path.relative_to(root).as_posix().encode()
    digest.update(b"d\0" if path.is_dir() else b"f\0")
    digest.update(relative_path)
    digest.update(b"\0")
    if path.is_file():
        digest.update(path.read_bytes())
        digest.update(b"\0")
print(digest.hexdigest())
PY
}

should_restore_compatible_legacy_parent_and_preserve_unrelated_namespace() {
  setup_fixture "compatible-legacy"
  local fixture_root="$FIXTURE_ROOT"
  write_manifest "$fixture_root" "new-pdf" "new-markdown" "new-text"
  commit_documents "$fixture_root" "parent manifest"
  local parent_revision
  parent_revision="$(git -C "${fixture_root}/test_documents" rev-parse HEAD)"
  printf 'child\n' >"${fixture_root}/test_documents/child.txt"
  commit_documents "$fixture_root" "child revision"
  seed_existing_cache "$fixture_root"

  local legacy_object="${fixture_root}/fake-gcs/test-bucket/corpus-cache/${parent_revision}.tar.zst"
  create_archive \
    "$legacy_object" \
    "new-pdf" \
    "new-markdown" \
    "new-text" \
    ".corpus-cache/._pdf"
  run_restore "$fixture_root"

  assert_equals "0" "$RUN_STATUS" "compatible legacy restore status"
  assert_contains \
    "Using verified-compatible legacy corpus object gs://test-bucket/corpus-cache/${parent_revision}.tar.zst." \
    "$RUN_OUTPUT" \
    "compatible parent selection"
  assert_file_content \
    "new-pdf" \
    "${fixture_root}/test_documents/.corpus-cache/pdf/reference.pdf" \
    "restored PDF"
  assert_file_content \
    "new-markdown" \
    "${fixture_root}/test_documents/.corpus-cache/ground_truth/pdf/reference.md" \
    "restored markdown"
  assert_file_content \
    "new-text" \
    "${fixture_root}/test_documents/.corpus-cache/ground_truth/pdf/reference.txt" \
    "restored text"
  assert_file_content \
    "preserve-me" \
    "${fixture_root}/test_documents/.corpus-cache/model-store/model.bin" \
    "unrelated cache namespace"
  [ ! -e "${fixture_root}/test_documents/.corpus-cache/._pdf" ] ||
    fail "legacy AppleDouble metadata was extracted"

  local digest
  digest="$(cache_digest "$fixture_root")"
  assert_equals \
    "storage objects describe gs://test-bucket/corpus-cache/v2/${digest}.tar.zst" \
    "$(sed -n '1p' "${fixture_root}/gcloud.log")" \
    "content-addressed lookup before legacy fallback"
  assert_contains \
    "storage cp gs://test-bucket/corpus-cache/${parent_revision}.tar.zst" \
    "$(cat "${fixture_root}/gcloud.log")" \
    "legacy parent download"
}

should_refuse_incompatible_legacy_manifest_without_altering_old_cache() {
  setup_fixture "incompatible-legacy"
  local fixture_root="$FIXTURE_ROOT"
  write_manifest "$fixture_root" "parent-pdf" "parent-markdown" "parent-text"
  commit_documents "$fixture_root" "parent manifest"
  local parent_revision
  parent_revision="$(git -C "${fixture_root}/test_documents" rev-parse HEAD)"
  create_archive \
    "${fixture_root}/fake-gcs/test-bucket/corpus-cache/${parent_revision}.tar.zst" \
    "parent-pdf" \
    "parent-markdown" \
    "parent-text"

  write_manifest "$fixture_root" "current-pdf" "current-markdown" "current-text"
  commit_documents "$fixture_root" "incompatible child manifest"
  seed_existing_cache "$fixture_root"
  local old_cache_fingerprint
  old_cache_fingerprint="$(cache_fingerprint "${fixture_root}/test_documents/.corpus-cache")"
  run_restore "$fixture_root"

  assert_equals "1" "$RUN_STATUS" "incompatible legacy restore status"
  assert_contains "No reference corpus cache found for content digest" "$RUN_OUTPUT" "incompatible legacy refusal"
  assert_file_content \
    "old-pdf" \
    "${fixture_root}/test_documents/.corpus-cache/pdf/reference.pdf" \
    "old PDF after incompatible legacy refusal"
  assert_file_content \
    "old-markdown" \
    "${fixture_root}/test_documents/.corpus-cache/ground_truth/pdf/reference.md" \
    "old markdown after incompatible legacy refusal"
  assert_file_content \
    "preserve-me" \
    "${fixture_root}/test_documents/.corpus-cache/model-store/model.bin" \
    "old unrelated namespace after incompatible legacy refusal"
  assert_equals \
    "$old_cache_fingerprint" \
    "$(cache_fingerprint "${fixture_root}/test_documents/.corpus-cache")" \
    "complete cache after incompatible legacy refusal"
  if grep -Fq "$parent_revision" "${fixture_root}/gcloud.log"; then
    fail "incompatible legacy object was queried or downloaded"
  fi
}

should_leave_old_cache_unchanged_when_archive_is_corrupt() {
  setup_fixture "corrupt-archive"
  local fixture_root="$FIXTURE_ROOT"
  write_manifest "$fixture_root" "current-pdf" "current-markdown" "current-text"
  commit_documents "$fixture_root" "manifest"
  printf 'child\n' >"${fixture_root}/test_documents/child.txt"
  commit_documents "$fixture_root" "child revision"
  seed_existing_cache "$fixture_root"
  local old_cache_fingerprint
  old_cache_fingerprint="$(cache_fingerprint "${fixture_root}/test_documents/.corpus-cache")"

  local digest
  digest="$(cache_digest "$fixture_root")"
  local content_object="${fixture_root}/fake-gcs/test-bucket/corpus-cache/v2/${digest}.tar.zst"
  mkdir -p "$(dirname "$content_object")"
  printf 'not a tar archive' >"$content_object"
  run_restore "$fixture_root"

  assert_equals "1" "$RUN_STATUS" "corrupt archive restore status"
  assert_contains "corpus cache validation failed:" "$RUN_OUTPUT" "corrupt archive validation error"
  assert_file_content \
    "old-pdf" \
    "${fixture_root}/test_documents/.corpus-cache/pdf/reference.pdf" \
    "old PDF after corrupt archive"
  assert_file_content \
    "old-text" \
    "${fixture_root}/test_documents/.corpus-cache/ground_truth/pdf/reference.txt" \
    "old text after corrupt archive"
  assert_file_content \
    "preserve-me" \
    "${fixture_root}/test_documents/.corpus-cache/model-store/model.bin" \
    "old unrelated namespace after corrupt archive"
  assert_equals \
    "$old_cache_fingerprint" \
    "$(cache_fingerprint "${fixture_root}/test_documents/.corpus-cache")" \
    "complete cache after corrupt archive"
}

should_refuse_lock_contention_without_altering_old_cache() {
  setup_fixture "lock-contention"
  local fixture_root="$FIXTURE_ROOT"
  write_manifest "$fixture_root" "current-pdf" "current-markdown" "current-text"
  commit_documents "$fixture_root" "manifest"
  printf 'child\n' >"${fixture_root}/test_documents/child.txt"
  commit_documents "$fixture_root" "child revision"
  seed_existing_cache "$fixture_root"
  local old_cache_fingerprint
  old_cache_fingerprint="$(cache_fingerprint "${fixture_root}/test_documents/.corpus-cache")"
  mkdir "${fixture_root}/test_documents/.corpus-cache.lock"
  run_restore "$fixture_root"

  assert_equals "1" "$RUN_STATUS" "lock contention restore status"
  assert_contains \
    "Corpus cache is locked by another publish or restore operation." \
    "$RUN_OUTPUT" \
    "lock contention error"
  assert_file_content \
    "old-pdf" \
    "${fixture_root}/test_documents/.corpus-cache/pdf/reference.pdf" \
    "old PDF after lock contention"
  assert_file_content \
    "preserve-me" \
    "${fixture_root}/test_documents/.corpus-cache/model-store/model.bin" \
    "old unrelated namespace after lock contention"
  assert_equals \
    "$old_cache_fingerprint" \
    "$(cache_fingerprint "${fixture_root}/test_documents/.corpus-cache")" \
    "complete cache after lock contention"
  assert_equals "" "$(cat "${fixture_root}/gcloud.log")" "gcloud calls under lock contention"
  [ -d "${fixture_root}/test_documents/.corpus-cache.lock" ] ||
    fail "contended lock directory was removed"
}

should_clean_up_after_download_interruption_without_altering_old_cache() {
  setup_fixture "download-interruption"
  local fixture_root="$FIXTURE_ROOT"
  write_manifest "$fixture_root" "current-pdf" "current-markdown" "current-text"
  commit_documents "$fixture_root" "manifest"
  seed_existing_cache "$fixture_root"
  local old_cache_fingerprint
  old_cache_fingerprint="$(cache_fingerprint "${fixture_root}/test_documents/.corpus-cache")"

  local digest
  digest="$(cache_digest "$fixture_root")"
  create_archive \
    "${fixture_root}/fake-gcs/test-bucket/corpus-cache/v2/${digest}.tar.zst" \
    "current-pdf" \
    "current-markdown" \
    "current-text"
  FAKE_GCLOUD_INTERRUPT=1 run_restore "$fixture_root"

  assert_not_equals "0" "$RUN_STATUS" "interrupted restore status"
  assert_equals \
    "$old_cache_fingerprint" \
    "$(cache_fingerprint "${fixture_root}/test_documents/.corpus-cache")" \
    "complete cache after interrupted download"
  [ ! -e "${fixture_root}/test_documents/.corpus-cache.lock" ] ||
    fail "restore lock remained after interruption"
  if compgen -G "${fixture_root}/test_documents/.corpus-cache-restore-*" >/dev/null ||
    compgen -G "${fixture_root}/test_documents/.corpus-cache-new-*" >/dev/null; then
    fail "restore temporary directory remained after interruption"
  fi
}

should_surface_storage_errors_without_altering_old_cache() {
  setup_fixture "storage-error"
  local fixture_root="$FIXTURE_ROOT"
  write_manifest "$fixture_root" "current-pdf" "current-markdown" "current-text"
  commit_documents "$fixture_root" "manifest"
  seed_existing_cache "$fixture_root"
  local old_cache_fingerprint
  old_cache_fingerprint="$(cache_fingerprint "${fixture_root}/test_documents/.corpus-cache")"

  FAKE_GCLOUD_DESCRIBE_ERROR=1 run_restore "$fixture_root"

  assert_equals "1" "$RUN_STATUS" "storage error restore status"
  assert_contains "Failed to inspect corpus cache object" "$RUN_OUTPUT" "storage error context"
  assert_contains "permission denied" "$RUN_OUTPUT" "storage error cause"
  assert_not_contains \
    "No reference corpus cache found" \
    "$RUN_OUTPUT" \
    "storage error must not be reported as a cache miss"
  assert_equals \
    "$old_cache_fingerprint" \
    "$(cache_fingerprint "${fixture_root}/test_documents/.corpus-cache")" \
    "complete cache after storage error"
  assert_equals \
    "1" \
    "$(wc -l <"${fixture_root}/gcloud.log" | tr -d ' ')" \
    "storage error must not trigger legacy fallback"
}

should_reject_appledouble_in_v2_without_altering_old_cache() {
  setup_fixture "v2-appledouble"
  local fixture_root="$FIXTURE_ROOT"
  write_manifest "$fixture_root" "current-pdf" "current-markdown" "current-text"
  commit_documents "$fixture_root" "manifest"
  seed_existing_cache "$fixture_root"
  local old_cache_fingerprint
  old_cache_fingerprint="$(cache_fingerprint "${fixture_root}/test_documents/.corpus-cache")"

  local digest
  digest="$(cache_digest "$fixture_root")"
  create_archive \
    "${fixture_root}/fake-gcs/test-bucket/corpus-cache/v2/${digest}.tar.zst" \
    "current-pdf" \
    "current-markdown" \
    "current-text" \
    ".corpus-cache/._pdf"
  run_restore "$fixture_root"

  assert_equals "1" "$RUN_STATUS" "v2 AppleDouble restore status"
  assert_contains "unexpected archive file: .corpus-cache/._pdf" "$RUN_OUTPUT" "v2 exact archive contract"
  assert_equals \
    "$old_cache_fingerprint" \
    "$(cache_fingerprint "${fixture_root}/test_documents/.corpus-cache")" \
    "complete cache after v2 AppleDouble rejection"
}

should_reject_non_appledouble_legacy_extra_without_altering_old_cache() {
  setup_fixture "legacy-extra"
  local fixture_root="$FIXTURE_ROOT"
  write_manifest "$fixture_root" "current-pdf" "current-markdown" "current-text"
  commit_documents "$fixture_root" "parent manifest"
  local parent_revision
  parent_revision="$(git -C "${fixture_root}/test_documents" rev-parse HEAD)"
  printf 'child\n' >"${fixture_root}/test_documents/child.txt"
  commit_documents "$fixture_root" "child revision"
  seed_existing_cache "$fixture_root"
  local old_cache_fingerprint
  old_cache_fingerprint="$(cache_fingerprint "${fixture_root}/test_documents/.corpus-cache")"

  create_archive \
    "${fixture_root}/fake-gcs/test-bucket/corpus-cache/${parent_revision}.tar.zst" \
    "current-pdf" \
    "current-markdown" \
    "current-text" \
    ".corpus-cache/pdf/unexpected.bin"
  run_restore "$fixture_root"

  assert_equals "1" "$RUN_STATUS" "legacy extra restore status"
  assert_contains \
    "unexpected archive file: .corpus-cache/pdf/unexpected.bin" \
    "$RUN_OUTPUT" \
    "legacy non-AppleDouble rejection"
  assert_equals \
    "$old_cache_fingerprint" \
    "$(cache_fingerprint "${fixture_root}/test_documents/.corpus-cache")" \
    "complete cache after legacy extra rejection"
}

should_restore_compatible_legacy_parent_and_preserve_unrelated_namespace
should_refuse_incompatible_legacy_manifest_without_altering_old_cache
should_leave_old_cache_unchanged_when_archive_is_corrupt
should_refuse_lock_contention_without_altering_old_cache
should_clean_up_after_download_interruption_without_altering_old_cache
should_surface_storage_errors_without_altering_old_cache
should_reject_appledouble_in_v2_without_altering_old_cache
should_reject_non_appledouble_legacy_extra_without_altering_old_cache
