"""Stable artifact writer helpers for ml-intern-codex workflows."""

from pathlib import Path


def artifact_root(base: Path) -> Path:
    return base / ".ml-intern"
