#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
VERSION=""
TARGET=""
OUTPUT_DIR=""

while [ "$#" -gt 0 ]; do
  case "$1" in
    --version)
      VERSION="$2"
      shift 2
      ;;
    --target)
      TARGET="$2"
      shift 2
      ;;
    --output-dir)
      OUTPUT_DIR="$2"
      shift 2
      ;;
    *)
      echo "unknown argument: $1" >&2
      exit 1
      ;;
  esac
done

[ -n "$VERSION" ] || { echo "--version is required" >&2; exit 1; }
[ -n "$TARGET" ] || { echo "--target is required" >&2; exit 1; }
[ -n "$OUTPUT_DIR" ] || { echo "--output-dir is required" >&2; exit 1; }

ARCHIVE_BASENAME="ml-intern-codex-${VERSION}-${TARGET}"
STAGE_DIR="${ROOT_DIR}/${OUTPUT_DIR}/${ARCHIVE_BASENAME}"
TARGET_DIR="${ROOT_DIR}/target/${TARGET}/release"
if [ ! -d "$TARGET_DIR" ]; then
  TARGET_DIR="${ROOT_DIR}/target/release"
fi

rm -rf "$STAGE_DIR"
mkdir -p "$STAGE_DIR/bin" "$STAGE_DIR/skills" "$STAGE_DIR/helpers"

cp "${TARGET_DIR}/ml-intern" "$STAGE_DIR/bin/"
cp "${TARGET_DIR}/ml-intern-app-server" "$STAGE_DIR/bin/"
cp -R "${ROOT_DIR}/skills/system" "$STAGE_DIR/skills/system"
cp -R "${ROOT_DIR}/helpers/python" "$STAGE_DIR/helpers/python"
cp -R "${ROOT_DIR}/helpers/node" "$STAGE_DIR/helpers/node"
cp "${ROOT_DIR}/README.md" "$STAGE_DIR/README.md"

mkdir -p "${ROOT_DIR}/${OUTPUT_DIR}"
tar -C "${ROOT_DIR}/${OUTPUT_DIR}" -czf "${ROOT_DIR}/${OUTPUT_DIR}/${ARCHIVE_BASENAME}.tar.gz" "${ARCHIVE_BASENAME}"
