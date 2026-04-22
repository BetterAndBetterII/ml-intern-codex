from __future__ import annotations

import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
import uuid


REPO_ROOT = Path(__file__).resolve().parents[3]
HELPER_SRC = REPO_ROOT / "helpers/python/src"
NODE_HELPERS_DIR = REPO_ROOT / "helpers/node"
PYTHON = sys.executable


class ArtifactWriterTests(unittest.TestCase):
    maxDiff = None

    def make_artifact_dir(self, root: Path) -> Path:
        thread_id = uuid.uuid4()
        artifact_id = uuid.uuid4()
        artifact_dir = root / ".ml-intern" / "threads" / str(thread_id) / "artifacts" / str(artifact_id)
        artifact_dir.mkdir(parents=True)
        return artifact_dir

    def run_module(self, module: str, artifact_dir: Path, extra_args: list[str]) -> dict[str, object]:
        command = [
            PYTHON,
            "-m",
            module,
            "--artifact-dir",
            str(artifact_dir),
            *extra_args,
        ]
        completed = subprocess.run(
            command,
            check=True,
            text=True,
            capture_output=True,
            env={**os.environ, "PYTHONPATH": str(HELPER_SRC)},
        )
        return json.loads(completed.stdout)

    def test_dataset_audit_writer_generates_expected_bundle(self) -> None:
        with tempfile.TemporaryDirectory() as raw_root:
            root = Path(raw_root)
            artifact_dir = self.make_artifact_dir(root)
            result = self.run_module(
                "mli_helpers.artifacts.write_dataset_audit",
                artifact_dir,
                [
                    "--dataset",
                    "org/name",
                    "--split",
                    "train",
                    "--split",
                    "validation",
                    "--row-count",
                    "train=10",
                    "--row-count",
                    "validation=4",
                    "--issue",
                    "missing labels",
                    "--raw-text",
                    "debug sample",
                ],
            )

            manifest = json.loads((artifact_dir / "artifact.json").read_text(encoding="utf-8"))
            report = (artifact_dir / "report.md").read_text(encoding="utf-8")
            payload = json.loads((artifact_dir / "report.json").read_text(encoding="utf-8"))

            self.assertEqual(result["manifest_path"], str(artifact_dir / "artifact.json"))
            self.assertEqual(manifest["kind"], "dataset_audit")
            self.assertEqual(manifest["metadata"]["dataset"], "org/name")
            self.assertEqual(manifest["extra_paths"], ["report.json", "raw.txt"])
            self.assertIn("Dataset Audit: org/name", report)
            self.assertEqual(payload["row_counts"], {"train": 10, "validation": 4})
            self.assertEqual((artifact_dir / "raw.txt").read_text(encoding="utf-8"), "debug sample\n")

    def test_paper_report_writer_uses_structured_metadata(self) -> None:
        with tempfile.TemporaryDirectory() as raw_root:
            root = Path(raw_root)
            artifact_dir = self.make_artifact_dir(root)
            self.run_module(
                "mli_helpers.artifacts.write_paper_report",
                artifact_dir,
                [
                    "--query",
                    "trl sft best practices",
                    "--paper-count",
                    "3",
                    "--top-paper",
                    "Paper A",
                    "--top-paper",
                    "Paper B",
                    "--recommended-recipe",
                    "Use packed sequences with careful masking",
                ],
            )

            manifest = json.loads((artifact_dir / "artifact.json").read_text(encoding="utf-8"))
            self.assertEqual(manifest["kind"], "paper_report")
            self.assertEqual(manifest["metadata"]["paper_count"], 3)
            self.assertEqual(manifest["metadata"]["top_papers"], ["Paper A", "Paper B"])

    def test_job_snapshot_writer_records_optional_fields(self) -> None:
        with tempfile.TemporaryDirectory() as raw_root:
            root = Path(raw_root)
            artifact_dir = self.make_artifact_dir(root)
            self.run_module(
                "mli_helpers.artifacts.write_job_snapshot",
                artifact_dir,
                [
                    "--job-id",
                    "job-123",
                    "--status",
                    "running",
                    "--hardware",
                    "a10g-large",
                    "--dashboard-url",
                    "https://hf.co/jobs/job-123",
                    "--duration-seconds",
                    "120",
                ],
            )

            manifest = json.loads((artifact_dir / "artifact.json").read_text(encoding="utf-8"))
            report = (artifact_dir / "report.md").read_text(encoding="utf-8")
            self.assertEqual(manifest["metadata"]["hardware"], "a10g-large")
            self.assertEqual(manifest["metadata"]["duration_seconds"], 120)
            self.assertIn("Dashboard: https://hf.co/jobs/job-123", report)

    def test_rejects_noncanonical_artifact_dir(self) -> None:
        with tempfile.TemporaryDirectory() as raw_root:
            artifact_dir = Path(raw_root) / "not-canonical"
            command = [
                PYTHON,
                "-m",
                "mli_helpers.artifacts.write_dataset_audit",
                "--artifact-dir",
                str(artifact_dir),
                "--dataset",
                "org/name",
            ]
            completed = subprocess.run(
                command,
                check=False,
                text=True,
                capture_output=True,
                env={**os.environ, "PYTHONPATH": str(HELPER_SRC)},
            )
            self.assertEqual(completed.returncode, 2)
            self.assertIn("artifact_dir must contain", completed.stderr)


class NodeHelperManifestTests(unittest.TestCase):
    def test_node_helper_package_manifest_is_parseable_and_points_to_entrypoint(self) -> None:
        package_json = json.loads((NODE_HELPERS_DIR / "package.json").read_text(encoding="utf-8"))

        self.assertEqual(package_json["name"], "mli-helpers-node")
        self.assertEqual(package_json["type"], "module")
        self.assertEqual(package_json["exports"], "./src/index.mjs")
        self.assertEqual(package_json["scripts"]["lint"], "node --check src/index.mjs")
        self.assertIn("helperSummary", package_json["scripts"]["smoke"])
        self.assertTrue((NODE_HELPERS_DIR / "src/index.mjs").is_file())


if __name__ == "__main__":
    unittest.main()
