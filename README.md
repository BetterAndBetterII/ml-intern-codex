# ml-intern-codex

A local-first ML engineering terminal app built on top of the installed `codex` binary.

The repo is organized as a Rust workspace that follows the dependency direction documented in `docs/SPEC.md`:

```text
Types -> Config -> Repo -> Service -> Runtime -> UI
```

Top-level areas:

- `crates/`: product code split by layer
- `skills/system/`: bundled Codex skills for ML workflows
- `helpers/`: local helper runtimes used by ML-oriented skills
- `docs/`: product docs, flows, and test plan

Helper runtime highlights:

- `helpers/python/`: stable `python3 -m mli_helpers.artifacts.write_*` commands for dataset audits, paper reports, and job snapshots
- `helpers/node/`: valid ESM helper package with `src/index.mjs` plus `lint` / `smoke` scripts for JS-native helper cases that are awkward in Python

## Run

- `ml-intern`: starts the default terminal client. Interactive TTYs use the full-screen event-driven UI; non-TTY runs fall back to the frozen line-mode client for deterministic scripting.
- `ml-intern --line-mode`: forces the legacy line-mode client for app-server debugging or transcript-oriented smoke tests.
- `ml-intern-app-server`: starts the local JSONL wrapper server.

## Current status

See `docs/IMPLEMENTATION_STATUS.md` for implemented slices, known gaps, and validation notes.
