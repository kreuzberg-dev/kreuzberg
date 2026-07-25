#!/usr/bin/env bash


set -euo pipefail

FORMAT="plaintext"
OCR_FLAG=""
FILE_PATH=""
for arg in "$@"; do
  case "$arg" in
  --format=*)
    FORMAT="${arg#--format=}"
    ;;
  --ocr)
    OCR_FLAG=""
    ;;
  --no-ocr)
    OCR_FLAG="--no-ocr"
    ;;
  *)
    FILE_PATH="$arg"
    ;;
  esac
done

if [ -z "$FILE_PATH" ]; then
  echo "Usage: liteparse_extract.sh [--format=plaintext|markdown] [--ocr|--no-ocr] <file_path>" >&2
  exit 1
fi

case "$FORMAT" in
plaintext)
  LIT_FORMAT="text"
  LIT_EXTRA_FLAGS="--no-links"
  ;;
markdown)
  LIT_FORMAT="markdown"
  LIT_EXTRA_FLAGS=""
  ;;
*)
  echo "Error: unsupported format '$FORMAT'; must be plaintext or markdown" >&2
  exit 64
  ;;
esac

if [ ! -f "$FILE_PATH" ]; then
  echo "Error: File not found: $FILE_PATH" >&2
  exit 1
fi

START=$(date +%s%N)

if command -v timeout &>/dev/null; then
  CONTENT=$(timeout 180s lit parse "$FILE_PATH" --format "$LIT_FORMAT" $LIT_EXTRA_FLAGS $OCR_FLAG --quiet)
elif command -v gtimeout &>/dev/null; then
  CONTENT=$(gtimeout 180s lit parse "$FILE_PATH" --format "$LIT_FORMAT" $LIT_EXTRA_FLAGS $OCR_FLAG --quiet)
else
  CONTENT=$(lit parse "$FILE_PATH" --format "$LIT_FORMAT" $LIT_EXTRA_FLAGS $OCR_FLAG --quiet)
fi

END=$(date +%s%N)
DURATION_MS=$(((END - START) / 1000000))

if command -v jq &>/dev/null; then
  CONTENT_FILE=$(mktemp)
  printf '%s' "$CONTENT" >"$CONTENT_FILE"
  jq -n \
    --rawfile content "$CONTENT_FILE" \
    --arg fmt "$FORMAT" \
    --argjson duration "$DURATION_MS" \
    '{
      content: $content,
      metadata: {framework: "liteparse", output_format: $fmt},
      _extraction_time_ms: $duration
    }'
  rm -f "$CONTENT_FILE"
else
  ESCAPED_CONTENT=$(echo "$CONTENT" | sed 's/\\/\\\\/g' | sed 's/"/\\"/g' | awk '{printf "%s\\n", $0}' | sed '$ s/\\n$//')
  cat <<EOF
{"content":"$ESCAPED_CONTENT","metadata":{"framework":"liteparse","output_format":"$FORMAT"},"_extraction_time_ms":$DURATION_MS}
EOF
fi
