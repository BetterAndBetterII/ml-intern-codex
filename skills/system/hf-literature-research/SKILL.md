---
name: hf-literature-research
description: Use this skill for paper and literature research workflows.
metadata:
  short-description: Research papers and persist paper reports
---

# hf-literature-research

Use this skill for paper and literature research workflows.

## Expected outputs

- `artifact.json`
- `report.md`
- `report.json`
- optional `raw.txt`

## Preferred helper command

```bash
PYTHONPATH=<ml-intern-codex>/helpers/python/src python3 -m \
  mli_helpers.artifacts.write_paper_report \
  --artifact-dir <cwd>/.ml-intern/threads/<thread-id>/artifacts/<artifact-id> \
  --turn-id <local-turn-id> \
  --query "trl sft best practices" \
  --paper-count 5 \
  --top-paper "Paper title" \
  --recommended-recipe "Concise recommendation"
```
