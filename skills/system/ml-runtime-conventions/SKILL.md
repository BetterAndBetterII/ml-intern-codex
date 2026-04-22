---
name: ml-runtime-conventions
description: Use this skill when a workflow needs to follow ml-intern-codex runtime conventions.
metadata:
  short-description: Follow ml-intern-codex runtime and artifact conventions
---

# ml-runtime-conventions

Use this skill when a workflow needs to follow ml-intern-codex runtime conventions.

## Responsibilities

- Persist artifacts under `<cwd>/.ml-intern/threads/<thread-id>/artifacts/<artifact-id>/`
- Prefer helper runtimes from `helpers/python` and `helpers/node`
- Emit stable `artifact.json` manifests for durable ML outputs

## Helper commands

- Python helpers live under `<ml-intern-codex>/helpers/python/src`
- Invoke them with `PYTHONPATH=<ml-intern-codex>/helpers/python/src python3 -m ...`
- All artifact writers require `--artifact-dir <cwd>/.ml-intern/threads/<thread-id>/artifacts/<artifact-id>`
- Pass `--turn-id <local-turn-id>` when you know the active local turn id; otherwise the helper will mint a UUID so the manifest still parses cleanly
