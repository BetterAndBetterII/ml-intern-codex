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

download_release_asset() {
  local asset_url="$1"
  local out_path="$2"

  if [ -n "${GITHUB_TOKEN:-}" ]; then
    curl -fsSL \
      -H "Authorization: Bearer ${GITHUB_TOKEN}" \
      "$asset_url" \
      -o "$out_path"
    return
  fi
  curl -fsSL "$asset_url" -o "$out_path"
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
  local latest_url tag version asset_name asset_url
  latest_url="$(curl -fsSL -o /dev/null -w '%{url_effective}' "https://github.com/${OWNER}/${REPO}/releases/latest")"
  tag="${latest_url##*/}"
  [ -n "$tag" ] || die "failed to resolve latest release tag"
  version="${tag#v}"
  asset_name="ml-intern-codex-${version}-${target}.tar.gz"
  asset_url="https://github.com/${OWNER}/${REPO}/releases/download/${tag}/${asset_name}"
  printf '%s\n%s\n%s\n' "$tag" "$asset_name" "$asset_url"
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
  [ "${#release_info[@]}" -ge 3 ] || die "failed to resolve latest release metadata"
  tag="${release_info[0]}"
  asset_name="${release_info[1]}"
  asset_url="${release_info[2]}"

  tmp_dir="$(mktemp -d)"
  trap 'rm -rf "'"$tmp_dir"'"' EXIT
  archive_path="${tmp_dir}/${asset_name}"
  bundle_root="${INSTALL_ROOT}/releases/${tag}/${target}"

  info "Downloading ${asset_name}"
  download_release_asset "$asset_url" "$archive_path"

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
