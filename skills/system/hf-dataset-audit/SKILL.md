---
name: hf-dataset-audit
description: Use this skill for dataset inspection and audit workflows.
metadata:
  short-description: Inspect datasets and persist audit artifacts
---

# hf-dataset-audit

Use this skill for dataset inspection and audit workflows.

## Expected outputs

- dataset schema notes
- split counts and issue summary
- artifact manifest plus markdown/json reports

## Preferred helper command

```bash
PYTHONPATH=<ml-intern-codex>/helpers/python/src python3 -m \
  mli_helpers.artifacts.write_dataset_audit \
  --artifact-dir <cwd>/.ml-intern/threads/<thread-id>/artifacts/<artifact-id> \
  --turn-id <local-turn-id> \
  --dataset <org/name> \
  --split train \
  --row-count train=123 \
  --issue "label skew"
```
