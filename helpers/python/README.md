# Python Helpers

Stable helper commands live under `helpers/python/src/mli_helpers/artifacts/`.

Use them with:

```bash
PYTHONPATH=helpers/python/src python3 -m mli_helpers.artifacts.write_dataset_audit ...
PYTHONPATH=helpers/python/src python3 -m mli_helpers.artifacts.write_paper_report ...
PYTHONPATH=helpers/python/src python3 -m mli_helpers.artifacts.write_job_snapshot ...
```

All writers expect an artifact directory that already follows the product schema:

```text
<cwd>/.ml-intern/threads/<thread-id>/artifacts/<artifact-id>/
```

They always write:

- `artifact.json`
- `report.md`
- `report.json`

and optionally `raw.txt` when raw supporting text is provided.
