"""Shared artifact-writing utilities.

These helpers intentionally keep the on-disk schema simple and deterministic so
Rust-side artifact scanning can treat the generated files as canonical inputs.
"""

from __future__ import annotations

from dataclasses import dataclass
from datetime import datetime, timezone
import json
from pathlib import Path
from typing import Any
from uuid import UUID, uuid4


REPORT_MARKDOWN = "report.md"
REPORT_JSON = "report.json"
RAW_TEXT = "raw.txt"
MANIFEST = "artifact.json"
THREADS_MARKER = "threads"
ARTIFACTS_MARKER = "artifacts"
SCHEMA_VERSION = 1


@dataclass(frozen=True)
class ArtifactLocation:
    artifact_dir: Path
    thread_id: str
    artifact_id: str
    turn_id: str


@dataclass(frozen=True)
class WrittenArtifact:
    artifact_dir: Path
    manifest_path: Path
    manifest: dict[str, Any]


class ArtifactWriterError(ValueError):
    pass


def parse_uuid(raw: str, field_name: str) -> str:
    try:
        return str(UUID(raw))
    except ValueError as error:
        raise ArtifactWriterError(f"invalid {field_name}: {raw}") from error


def iso_now() -> str:
    return datetime.now(timezone.utc).replace(microsecond=0).isoformat().replace("+00:00", "Z")


def resolve_artifact_location(artifact_dir: str, turn_id: str | None) -> ArtifactLocation:
    resolved_dir = Path(artifact_dir).expanduser().resolve()
    parts = resolved_dir.parts
    try:
        threads_index = parts.index(THREADS_MARKER)
        artifacts_index = parts.index(ARTIFACTS_MARKER)
    except ValueError as error:
        raise ArtifactWriterError(
            "artifact_dir must contain `.ml-intern/threads/<thread-id>/artifacts/<artifact-id>`"
        ) from error
    if artifacts_index - threads_index != 2 or artifacts_index != len(parts) - 2:
        raise ArtifactWriterError(
            "artifact_dir must end with `.ml-intern/threads/<thread-id>/artifacts/<artifact-id>`"
        )

    thread_id = parse_uuid(parts[threads_index + 1], "thread_id")
    artifact_id = parse_uuid(parts[artifacts_index + 1], "artifact_id")
    normalized_turn_id = parse_uuid(turn_id, "turn_id") if turn_id else str(uuid4())
    return ArtifactLocation(
        artifact_dir=resolved_dir,
        thread_id=thread_id,
        artifact_id=artifact_id,
        turn_id=normalized_turn_id,
    )


def default_report_json(payload: dict[str, Any]) -> str:
    return json.dumps(payload, ensure_ascii=True, indent=2, sort_keys=True) + "\n"


def write_artifact(
    *,
    artifact_dir: str,
    turn_id: str | None,
    kind: str,
    title: str,
    summary: str,
    metadata: dict[str, Any],
    report_markdown: str,
    report_json: str,
    raw_text: str | None = None,
    tags: list[str] | None = None,
) -> WrittenArtifact:
    location = resolve_artifact_location(artifact_dir, turn_id)
    normalized_tags = [tag for tag in (tags or []) if tag]
    if not title.strip():
        raise ArtifactWriterError("title must be non-empty")
    if not summary.strip():
        raise ArtifactWriterError("summary must be non-empty")

    location.artifact_dir.mkdir(parents=True, exist_ok=True)
    (location.artifact_dir / REPORT_MARKDOWN).write_text(report_markdown.rstrip() + "\n", encoding="utf-8")
    (location.artifact_dir / REPORT_JSON).write_text(report_json, encoding="utf-8")

    extra_paths = [REPORT_JSON]
    if raw_text:
        (location.artifact_dir / RAW_TEXT).write_text(raw_text.rstrip() + "\n", encoding="utf-8")
        extra_paths.append(RAW_TEXT)

    timestamp = iso_now()
    manifest = {
        "id": location.artifact_id,
        "version": SCHEMA_VERSION,
        "local_thread_id": location.thread_id,
        "local_turn_id": location.turn_id,
        "kind": kind,
        "title": title,
        "created_at": timestamp,
        "updated_at": timestamp,
        "summary": summary,
        "tags": normalized_tags,
        "primary_path": REPORT_MARKDOWN,
        "extra_paths": extra_paths,
        "metadata": metadata,
    }
    manifest_path = location.artifact_dir / MANIFEST
    manifest_path.write_text(default_report_json(manifest), encoding="utf-8")
    return WrittenArtifact(
        artifact_dir=location.artifact_dir,
        manifest_path=manifest_path,
        manifest=manifest,
    )


def print_result(result: WrittenArtifact) -> None:
    payload = {
        "artifact_dir": str(result.artifact_dir),
        "artifact_id": result.manifest["id"],
        "manifest_path": str(result.manifest_path),
        "title": result.manifest["title"],
    }
    print(default_report_json(payload), end="")
