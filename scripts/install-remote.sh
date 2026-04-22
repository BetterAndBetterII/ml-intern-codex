#!/usr/bin/env bash
set -euo pipefail

OWNER="${MLI_GITHUB_OWNER:-BetterAndBetterII}"
REPO="${MLI_GITHUB_REPO:-ml-intern-codex}"
REF="${MLI_GITHUB_REF:-main}"
INSTALL_ROOT="${MLI_INSTALL_ROOT:-${XDG_DATA_HOME:-$HOME/.local/share}/ml-intern-codex}"
REPO_DIR="${INSTALL_ROOT}/repo"

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
  need_cmd git || die "git is required"
  need_cmd curl || die "curl is required"
}

git_fetch_with_token() {
  local -a args=("$@")
  git -c "http.extraHeader=Authorization: Bearer ${GITHUB_TOKEN}" "${args[@]}"
}

clone_or_update_repo() {
  mkdir -p "$INSTALL_ROOT"

  if [ -d "$REPO_DIR/.git" ]; then
    info "Updating existing repo in $REPO_DIR"
    if [ -n "${GITHUB_TOKEN:-}" ]; then
      git_fetch_with_token -C "$REPO_DIR" fetch --depth=1 origin "$REF"
    elif need_cmd gh && gh auth status >/dev/null 2>&1; then
      git -C "$REPO_DIR" fetch --depth=1 origin "$REF"
    else
      die "Need GITHUB_TOKEN or authenticated gh to update private repo"
    fi
    git -C "$REPO_DIR" checkout -B "$REF" FETCH_HEAD
    return
  fi

  info "Cloning $OWNER/$REPO into $REPO_DIR"
  if [ -n "${GITHUB_TOKEN:-}" ]; then
    git_fetch_with_token clone --depth=1 --branch "$REF" "https://github.com/${OWNER}/${REPO}.git" "$REPO_DIR"
  elif need_cmd gh && gh auth status >/dev/null 2>&1; then
    gh repo clone "${OWNER}/${REPO}" "$REPO_DIR" -- --depth=1 --branch "$REF"
  else
    die "Need GITHUB_TOKEN or authenticated gh to clone private repo"
  fi
}

main() {
  require_prereqs
  clone_or_update_repo

  info "Running local installer"
  bash "$REPO_DIR/scripts/install.sh"

  printf '\n'
  printf 'Installed.\n'
  printf 'Run:\n'
  printf '  ml-intern\n'
}

main "$@"
