# ml-intern-codex

`ml-intern-codex` is a local-first ML engineering terminal app built on top of your installed `codex`.

- full-screen Rust TUI
- local wrapper app-server
- skill-first ML workflows
- local artifact persistence for reports, audits, and job snapshots

## Requirements

- `codex` on `PATH`
- a working local Codex auth/config under `~/.codex`
- `curl`
- `tar`
- `python3`

Quick check:

```bash
codex --version
```

## One-line install

Latest release install:

```bash
curl -fsSL \
  https://raw.githubusercontent.com/BetterAndBetterII/ml-intern-codex/main/scripts/install-remote.sh \
  | bash
```

This downloads the latest GitHub release for your platform and installs:

- release files into `~/.local/share/ml-intern-codex`
- launchers into `~/.local/bin`

## Local install from clone

If you already cloned the repo:

```bash
./scripts/install.sh
```

If `ml-intern` is still not found after install, open a new shell or run:

```bash
. "$HOME/.cargo/env"
```

## Quick Start

Start the TUI:

```bash
ml-intern
```

Useful first commands inside the app:

- `$` — open the skill picker
- `/skills` — list skills
- `/threads` — resume a thread
- `/artifacts` — browse saved artifacts
- `/help` — show keybindings

Start the local app-server directly:

```bash
ml-intern-app-server
```

## Development

Run the workspace tests:

```bash
cargo test --workspace
```

See `docs/IMPLEMENTATION_STATUS.md` for implementation notes and current validation status.
