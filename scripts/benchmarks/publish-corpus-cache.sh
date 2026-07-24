#!/usr/bin/env bash
# Package the license-restricted reference benchmark corpus slice.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
source "${REPO_ROOT}/scripts/lib/common.sh"
validate_repo_root "$REPO_ROOT" || exit 1

BUCKET="${GCP_BENCHMARK_BUCKET:-xberg-benchmark-corpus}"
TEST_DOCS="${REPO_ROOT}/test_documents"
CACHE="${TEST_DOCS}/.corpus-cache"
MANIFEST="${TEST_DOCS}/ground_truth/corpus_manifest.json"
CACHE_MANIFEST="${REPO_ROOT}/tools/benchmark-harness/scripts/corpus_cache_manifest.py"
LOCK_DIR="${TEST_DOCS}/.corpus-cache.lock"

WORK_DIR=""
LOCK_HELD=false

cleanup() {
  local exit_code=$?
  if [ -n "$WORK_DIR" ]; then
    rm -rf "$WORK_DIR"
  fi
  if [ "$LOCK_HELD" = true ]; then
    rmdir "$LOCK_DIR" 2>/dev/null || true
  fi
  return "$exit_code"
}
trap cleanup EXIT

WORK_DIR="$(mktemp -d -t corpus-cache-publish-XXXXXX)"
SNAPSHOT="${WORK_DIR}/snapshot/.corpus-cache"
SNAPSHOT_MANIFEST="${WORK_DIR}/corpus_manifest.json"
RAW_TAR="${WORK_DIR}/corpus-cache.tar"
TARBALL="${RAW_TAR}.zst"

if ! mkdir "$LOCK_DIR"; then
  echo "::error::Corpus cache is locked by another publish or restore operation." >&2
  exit 1
fi
LOCK_HELD=true

SHA="$(git -C "$TEST_DOCS" rev-parse HEAD)"
cp "$MANIFEST" "$SNAPSHOT_MANIFEST"
CACHE_KEY="$(python3 "$CACHE_MANIFEST" digest --manifest "$SNAPSHOT_MANIFEST")"
OBJECT="gs://${BUCKET}/corpus-cache/v2/${CACHE_KEY}.tar.zst"

mkdir -p "$SNAPSHOT"
cp -a "${CACHE}/pdf" "${CACHE}/ground_truth" "$SNAPSHOT/"
python3 "$CACHE_MANIFEST" verify --manifest "$SNAPSHOT_MANIFEST" --cache-root "$SNAPSHOT"
echo "Packaging reference corpus ${CACHE_KEY:0:12} for test_documents ${SHA:0:12}..."
COPYFILE_DISABLE=1 tar -C "${WORK_DIR}/snapshot" -cf "$RAW_TAR" .corpus-cache/pdf .corpus-cache/ground_truth/pdf
python3 "$CACHE_MANIFEST" verify-archive --manifest "$SNAPSHOT_MANIFEST" --archive "$RAW_TAR"
zstd -19 -T0 -f "$RAW_TAR" -o "$TARBALL"

echo "Uploading $(du -h "$TARBALL" | cut -f1) → ${OBJECT}"
gcloud storage cp "$TARBALL" "$OBJECT" --if-generation-match=0
echo "✓ Published content-addressed reference corpus ${CACHE_KEY:0:12}."
