#!/usr/bin/env bash
# Restore the license-restricted reference benchmark corpus slice.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
source "${REPO_ROOT}/scripts/lib/common.sh"
validate_repo_root "$REPO_ROOT" || exit 1

BUCKET="${GCP_BENCHMARK_BUCKET:-xberg-benchmark-corpus}"
TEST_DOCS="${REPO_ROOT}/test_documents"
CACHE="${TEST_DOCS}/.corpus-cache"
MANIFEST="${TEST_DOCS}/ground_truth/corpus_manifest.json"
CACHE_MANIFEST="${REPO_ROOT}/tools/benchmark-harness/scripts/corpus_cache_manifest.py"
LEGACY_HISTORY_DEPTH=50
LOCK_DIR="${TEST_DOCS}/.corpus-cache.lock"

WORK_DIR=""
NEW_CACHE=""
LOCK_HELD=false

cleanup() {
  local exit_code=$?
  if [ -n "$WORK_DIR" ]; then
    rm -rf "$WORK_DIR"
  fi
  if [ -n "$NEW_CACHE" ]; then
    rm -rf "$NEW_CACHE"
  fi
  if [ "$LOCK_HELD" = true ]; then
    rmdir "$LOCK_DIR" 2>/dev/null || true
  fi
  return "$exit_code"
}
trap cleanup EXIT

WORK_DIR="$(mktemp -d "${TEST_DOCS}/.corpus-cache-restore-XXXXXX")"
NEW_CACHE="$(mktemp -d "${TEST_DOCS}/.corpus-cache-new-XXXXXX")"
TARBALL="${WORK_DIR}/corpus-cache.tar.zst"
RAW_TAR="${WORK_DIR}/corpus-cache.tar"
SNAPSHOT_MANIFEST="${WORK_DIR}/corpus_manifest.json"

if ! mkdir "$LOCK_DIR"; then
  echo "::error::Corpus cache is locked by another publish or restore operation." >&2
  exit 1
fi
LOCK_HELD=true

cp "$MANIFEST" "$SNAPSHOT_MANIFEST"
CACHE_KEY="$(python3 "$CACHE_MANIFEST" digest --manifest "$SNAPSHOT_MANIFEST")"
CONTENT_OBJECT="gs://${BUCKET}/corpus-cache/v2/${CACHE_KEY}.tar.zst"

object_exists() {
  local object="$1"
  local error_output
  if error_output="$(gcloud storage objects describe "$object" 2>&1)"; then
    return 0
  fi
  case "$error_output" in
  *404* | *"No URLs matched"*) return 1 ;;
  *)
    echo "::error::Failed to inspect corpus cache object ${object}." >&2
    echo "$error_output" >&2
    return 2
    ;;
  esac
}

legacy_object() {
  local revision revision_manifest revision_key object object_status
  revision_manifest="${WORK_DIR}/legacy-manifest.json"

  if [ "$(git -C "$TEST_DOCS" rev-parse --is-shallow-repository)" = true ]; then
    if ! git -C "$TEST_DOCS" fetch --quiet --no-tags --deepen="$LEGACY_HISTORY_DEPTH" origin main; then
      echo "::error::Failed to deepen test_documents history for legacy corpus cache lookup." >&2
      return 2
    fi
  fi

  while read -r revision; do
    if ! git -C "$TEST_DOCS" show "${revision}:ground_truth/corpus_manifest.json" >"$revision_manifest" 2>/dev/null; then
      continue
    fi
    if ! revision_key="$(python3 "$CACHE_MANIFEST" digest --manifest "$revision_manifest" 2>/dev/null)"; then
      continue
    fi
    if [ "$revision_key" != "$CACHE_KEY" ]; then
      continue
    fi
    object="gs://${BUCKET}/corpus-cache/${revision}.tar.zst"
    if object_exists "$object"; then
      echo "$object"
      return 0
    else
      object_status=$?
      if [ "$object_status" -eq 2 ]; then
        return 2
      fi
    fi
  done < <(git -C "$TEST_DOCS" rev-list --first-parent --max-count="$LEGACY_HISTORY_DEPTH" HEAD)
  return 1
}

if object_exists "$CONTENT_OBJECT"; then
  OBJECT="$CONTENT_OBJECT"
  LEGACY_OBJECT=false
else
  object_status=$?
  if [ "$object_status" -eq 2 ]; then
    exit 1
  fi
  if OBJECT="$(legacy_object)"; then
    LEGACY_OBJECT=true
    echo "Using verified-compatible legacy corpus object ${OBJECT}."
  else
    object_status=$?
    if [ "$object_status" -eq 2 ]; then
      exit 1
    fi
    echo "::error::No reference corpus cache found for content digest ${CACHE_KEY}." >&2
    echo "::error::A maintainer must run 'task benchmark:corpus:cache:publish' after materializing the reference corpus." >&2
    exit 1
  fi
fi

echo "Restoring reference corpus ${CACHE_KEY:0:12} from ${OBJECT}..."
gcloud storage cp "$OBJECT" "$TARBALL"
mkdir -p "${WORK_DIR}/extract"
zstd -dc "$TARBALL" >"$RAW_TAR"
ARCHIVE_COMPATIBILITY_ARGS=()
if [ "$LEGACY_OBJECT" = true ]; then
  ARCHIVE_COMPATIBILITY_ARGS+=(--allow-legacy-appledouble)
fi
python3 "$CACHE_MANIFEST" extract-archive \
  --manifest "$SNAPSHOT_MANIFEST" \
  --archive "$RAW_TAR" \
  --destination "${WORK_DIR}/extract" \
  "${ARCHIVE_COMPATIBILITY_ARGS[@]}"
RESTORED_CACHE="${WORK_DIR}/extract/.corpus-cache"
python3 "$CACHE_MANIFEST" verify --manifest "$SNAPSHOT_MANIFEST" --cache-root "$RESTORED_CACHE"

if [ -d "$CACHE" ]; then
  cp -a "${CACHE}/." "$NEW_CACHE/"
  rm -rf "${NEW_CACHE}/pdf" "${NEW_CACHE}/ground_truth"
fi
mv "${RESTORED_CACHE}/pdf" "${NEW_CACHE}/pdf"
mv "${RESTORED_CACHE}/ground_truth" "${NEW_CACHE}/ground_truth"

if [ "$(python3 "$CACHE_MANIFEST" digest --manifest "$MANIFEST")" != "$CACHE_KEY" ]; then
  echo "::error::Corpus manifest changed during restore; retry with a stable checkout." >&2
  exit 1
fi
python3 "$CACHE_MANIFEST" atomic-swap --current "$CACHE" --replacement "$NEW_CACHE"

pdf_count="$(find "${CACHE}/pdf" -type f 2>/dev/null | wc -l | tr -d ' ')"
gt_count="$(find "${CACHE}/ground_truth/pdf" -type f 2>/dev/null | wc -l | tr -d ' ')"
echo "✓ Restored ${pdf_count} reference PDFs + ${gt_count} ground-truth files."
