---
name: hf-jobs-operator
description: Use this skill for Hugging Face jobs inspection, summarize, and runbook flows.
metadata:
  short-description: Inspect jobs and persist job snapshots
---

# hf-jobs-operator

Use this skill for Hugging Face jobs inspection, summarize, and runbook flows.

## Expected outputs

- job snapshot manifest
- human-readable report
- optional raw logs or excerpts

## Preferred helper command

```bash
PYTHONPATH=<ml-intern-codex>/helpers/python/src python3 -m \
  mli_helpers.artifacts.write_job_snapshot \
  --artifact-dir <cwd>/.ml-intern/threads/<thread-id>/artifacts/<artifact-id> \
  --turn-id <local-turn-id> \
  --job-id <job-id> \
  --status running \
  --hardware a10g-large
```
