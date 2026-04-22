#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
export CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}"
export RUSTUP_HOME="${RUSTUP_HOME:-$HOME/.rustup}"

info() {
  printf '\033[1;34m==>\033[0m %s\n' "$*"
}

warn() {
  printf '\033[1;33mwarning:\033[0m %s\n' "$*"
}

need_cmd() {
  command -v "$1" >/dev/null 2>&1
}

ensure_rust() {
  if need_cmd cargo && need_cmd rustup; then
    return
  fi

  info "Rust toolchain not found; installing rustup"
  need_cmd curl || {
    echo "curl is required to install Rust." >&2
    exit 1
  }

  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
    | sh -s -- -y --default-toolchain stable --profile default
}

ensure_codex() {
  if ! need_cmd codex; then
    echo "codex is required but was not found on PATH." >&2
    exit 1
  fi

  info "Using codex: $(codex --version)"
  if [ ! -f "$HOME/.codex/config.toml" ] && [ ! -f "$HOME/.codex/auth.json" ]; then
    warn "~/.codex does not look initialized yet. If startup fails, run codex login/setup first."
  fi
}

ensure_repo_skill_links() {
  local repo_skills_root="$ROOT_DIR/.agents/skills"
  mkdir -p "$repo_skills_root"

  for dir in "$ROOT_DIR"/skills/system/* "$HOME/.ml-intern-codex/runtime/generated-skills"/*; do
    [ -d "$dir" ] || continue
    local name
    name="$(basename "$dir")"
    local dest="$repo_skills_root/$name"

    if [ -L "$dest" ]; then
      rm -f "$dest"
    elif [ -e "$dest" ]; then
      warn "Skipping existing non-symlink path: $dest"
      continue
    fi

    ln -s "$dir" "$dest"
  done
}

ensure_generated_runtime_skill() {
  local generated_dir="$HOME/.ml-intern-codex/runtime/generated-skills/runtime-artifact-contract"
  mkdir -p "$generated_dir"
  cat > "$generated_dir/SKILL.md" <<EOF
---
name: runtime-artifact-contract
description: Write artifacts into the canonical ml-intern-codex thread artifact tree and emit artifact.json manifests.
metadata:
  short-description: Persist canonical runtime artifacts
---

# runtime-artifact-contract

Write artifacts into \`<cwd>/.ml-intern/threads/<thread-id>/artifacts/<artifact-id>/\` and always emit \`artifact.json\`.

## Concrete helper lanes

- dataset audit: \`PYTHONPATH=$ROOT_DIR/helpers/python/src python3 -m mli_helpers.artifacts.write_dataset_audit --artifact-dir <cwd>/.ml-intern/threads/<thread-id>/artifacts/<artifact-id> --turn-id <local-turn-id> ...\`
- paper report: \`PYTHONPATH=$ROOT_DIR/helpers/python/src python3 -m mli_helpers.artifacts.write_paper_report --artifact-dir <cwd>/.ml-intern/threads/<thread-id>/artifacts/<artifact-id> --turn-id <local-turn-id> ...\`
- job snapshot: \`PYTHONPATH=$ROOT_DIR/helpers/python/src python3 -m mli_helpers.artifacts.write_job_snapshot --artifact-dir <cwd>/.ml-intern/threads/<thread-id>/artifacts/<artifact-id> --turn-id <local-turn-id> ...\`
EOF
}

main() {
  info "Installing ml-intern-codex from $ROOT_DIR"
  ensure_rust
  # shellcheck disable=SC1090
  . "$HOME/.cargo/env"
  ensure_codex

  info "Installing ml-intern binaries"
  cargo install --path "$ROOT_DIR/crates/mli-cli" --force --locked

  info "Preparing generated runtime skill"
  ensure_generated_runtime_skill

  info "Preparing repo-local skill links"
  ensure_repo_skill_links

  info "Install complete"
  printf '\n'
  printf 'Run:\n'
  printf '  ml-intern\n'
  printf '\n'
  printf 'If the command is not found yet, run:\n'
  printf '  . "$HOME/.cargo/env"\n'
}

main "$@"
