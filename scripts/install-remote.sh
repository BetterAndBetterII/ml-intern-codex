#!/usr/bin/env bash
set -euo pipefail

OWNER="${MLI_GITHUB_OWNER:-BetterAndBetterII}"
REPO="${MLI_GITHUB_REPO:-ml-intern-codex}"
INSTALL_ROOT="${MLI_INSTALL_ROOT:-${XDG_DATA_HOME:-$HOME/.local/share}/ml-intern-codex}"
BIN_DIR="${MLI_BIN_DIR:-$HOME/.local/bin}"

info() {
  printf '\033[1;34m==>\033[0m %s\n' "$*"
}

die() {
  printf '\033[1;31merror:\033[0m %s\n' "$*" >&2
  exit 1
}

need_cmd() {
  command -v "$1" >/dev/null 2>&1
}

require_prereqs() {
  need_cmd curl || die "curl is required"
  need_cmd tar || die "tar is required"
  need_cmd python3 || die "python3 is required"
  need_cmd codex || die "codex is required"
}

github_api() {
  if [ -n "${GITHUB_TOKEN:-}" ]; then
    curl -fsSL \
      -H "Authorization: Bearer ${GITHUB_TOKEN}" \
      -H "Accept: application/vnd.github+json" \
      "$@"
    return
  fi
  if need_cmd gh && gh auth status >/dev/null 2>&1; then
    gh api "$@"
    return
  fi
  die "Need GITHUB_TOKEN or authenticated gh for private release downloads"
}

download_release_asset() {
  local tag="$1"
  local asset_name="$2"
  local asset_api_url="$3"
  local out_path="$4"

  if [ -n "${GITHUB_TOKEN:-}" ]; then
    curl -fsSL \
      -H "Authorization: Bearer ${GITHUB_TOKEN}" \
      -H "Accept: application/octet-stream" \
      "$asset_api_url" \
      -o "$out_path"
    return
  fi

  if need_cmd gh && gh auth status >/dev/null 2>&1; then
    local tmp_dir
    tmp_dir="$(dirname "$out_path")"
    gh release download "$tag" -R "${OWNER}/${REPO}" -p "$asset_name" -D "$tmp_dir"
    return
  fi

  die "Need GITHUB_TOKEN or authenticated gh for asset download"
}

detect_target() {
  local os arch
  os="$(uname -s)"
  arch="$(uname -m)"
  case "${os}:${arch}" in
    Linux:x86_64) echo "x86_64-unknown-linux-gnu" ;;
    Darwin:x86_64) echo "x86_64-apple-darwin" ;;
    Darwin:arm64|Darwin:aarch64) echo "aarch64-apple-darwin" ;;
    *)
      die "Unsupported platform ${os}/${arch}"
      ;;
  esac
}

resolve_latest_release_asset() {
  local target="$1"
  local release_json tag version asset_name
  release_json="$(github_api "https://api.github.com/repos/${OWNER}/${REPO}/releases/latest")"
  tag="$(printf '%s' "$release_json" | python3 -c 'import json,sys; print(json.load(sys.stdin)["tag_name"])')"
  version="${tag#v}"
  asset_name="ml-intern-codex-${version}-${target}.tar.gz"
  printf '%s' "$release_json" | python3 - "$asset_name" <<'PY'
import json
import sys

asset_name = sys.argv[1]
release = json.load(sys.stdin)
for asset in release.get("assets", []):
    if asset.get("name") == asset_name:
        print(release["tag_name"])
        print(asset["name"])
        print(asset["url"])
        raise SystemExit(0)
raise SystemExit(f"missing release asset: {asset_name}")
PY
}

install_wrappers() {
  local bundle_root="$1"
  mkdir -p "$BIN_DIR"

  cat > "${BIN_DIR}/ml-intern" <<EOF
#!/usr/bin/env bash
set -euo pipefail
export MLI_INSTALL_ROOT="${bundle_root}"
exec "${bundle_root}/bin/ml-intern" "\$@"
EOF

  cat > "${BIN_DIR}/ml-intern-app-server" <<EOF
#!/usr/bin/env bash
set -euo pipefail
export MLI_INSTALL_ROOT="${bundle_root}"
exec "${bundle_root}/bin/ml-intern-app-server" "\$@"
EOF

  chmod +x "${BIN_DIR}/ml-intern" "${BIN_DIR}/ml-intern-app-server"
}

main() {
  require_prereqs
  local target release_info tag asset_name asset_url tmp_dir archive_path bundle_root
  target="$(detect_target)"
  info "Resolving latest release for ${target}"
  mapfile -t release_info < <(resolve_latest_release_asset "$target")
  tag="${release_info[0]}"
  asset_name="${release_info[1]}"
  asset_url="${release_info[2]}"

  tmp_dir="$(mktemp -d)"
  trap 'rm -rf "$tmp_dir"' EXIT
  archive_path="${tmp_dir}/${asset_name}"
  bundle_root="${INSTALL_ROOT}/releases/${tag}/${target}"

  info "Downloading ${asset_name}"
  download_release_asset "$tag" "$asset_name" "$asset_url" "$archive_path"

  info "Installing release ${tag} into ${bundle_root}"
  rm -rf "$bundle_root"
  mkdir -p "$bundle_root"
  tar -xzf "$archive_path" --strip-components=1 -C "$bundle_root"

  info "Installing launchers into ${BIN_DIR}"
  install_wrappers "$bundle_root"

  printf '\n'
  printf 'Installed.\n'
  printf 'Run:\n'
  printf '  ml-intern\n'
  printf '\n'
  if [[ ":${PATH}:" != *":${BIN_DIR}:"* ]]; then
    printf 'If the command is not found, add this to your shell profile:\n'
    printf '  export PATH="%s:$PATH"\n' "$BIN_DIR"
  fi
}

main "$@"
