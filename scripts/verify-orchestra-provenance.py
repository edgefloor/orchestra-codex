#!/usr/bin/env python3
"""Verify the fork's sealed upstream and Orchestra core source identities."""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import subprocess
import sys
from pathlib import Path, PurePosixPath

ROOT = Path(__file__).resolve().parents[1]
MANIFEST = ROOT / "orchestra-provenance.json"
HEX40 = re.compile(r"[0-9a-f]{40}")
HEX64 = re.compile(r"[0-9a-f]{64}")


def require_fields(
    value: object, expected: set[str], context: str
) -> dict[str, object]:
    if not isinstance(value, dict):
        raise ValueError(f"{context} must be an object")
    actual = set(value)
    if actual != expected:
        raise ValueError(
            f"{context} fields must be {sorted(expected)}, found {sorted(actual)}"
        )
    return value


def require_revision(value: object, context: str) -> str:
    if not isinstance(value, str) or HEX40.fullmatch(value) is None:
        raise ValueError(f"{context} must be a lowercase 40-character Git identity")
    return value


def safe_relative_path(value: object, context: str) -> PurePosixPath:
    if not isinstance(value, str):
        raise ValueError(f"{context} must be a string")
    path = PurePosixPath(value)
    if path.is_absolute() or ".." in path.parts or str(path) in {"", "."}:
        raise ValueError(f"{context} must be a safe relative path")
    return path


def file_identities(root: Path) -> dict[str, str]:
    result: dict[str, str] = {}
    for prefix in ("fixtures", "src", "tests"):
        directory = root / prefix
        if not directory.is_dir():
            continue
        for path in sorted(item for item in directory.rglob("*") if item.is_file()):
            relative = path.relative_to(root).as_posix()
            result[relative] = hashlib.sha256(path.read_bytes()).hexdigest()
    return result


def git_identity(repository: Path, revision: str, path: PurePosixPath) -> str:
    return subprocess.check_output(
        ["git", "-C", str(repository), "rev-parse", f"{revision}:{path.as_posix()}"],
        text=True,
    ).strip()


def git_commit_tree(repository: Path, revision: str) -> str:
    return subprocess.check_output(
        ["git", "-C", str(repository), "rev-parse", f"{revision}^{{tree}}"],
        text=True,
    ).strip()


def verify(source_root: Path | None) -> None:
    manifest = json.loads(MANIFEST.read_text())
    root = require_fields(
        manifest, {"schemaVersion", "fork", "upstream", "orchestraCore"}, "manifest"
    )
    if root["schemaVersion"] != 1:
        raise ValueError("schemaVersion must be 1")

    require_fields(root["fork"], {"repository", "defaultBranch"}, "fork")
    upstream = require_fields(
        root["upstream"], {"repository", "baseRevision", "baseTree"}, "upstream"
    )
    base_revision = require_revision(upstream["baseRevision"], "upstream.baseRevision")
    require_revision(upstream["baseTree"], "upstream.baseTree")

    core = require_fields(
        root["orchestraCore"],
        {
            "repository",
            "revision",
            "sourcePath",
            "sourceTree",
            "snapshotPath",
            "snapshotTransform",
            "files",
        },
        "orchestraCore",
    )
    source_revision = require_revision(core["revision"], "orchestraCore.revision")
    source_tree = require_revision(core["sourceTree"], "orchestraCore.sourceTree")
    source_path = safe_relative_path(core["sourcePath"], "orchestraCore.sourcePath")
    snapshot_path = safe_relative_path(
        core["snapshotPath"], "orchestraCore.snapshotPath"
    )
    files = core["files"]
    if not isinstance(files, dict) or not files:
        raise ValueError("orchestraCore.files must be a non-empty object")
    if list(files) != sorted(files):
        raise ValueError("orchestraCore.files must be sorted")
    for path, digest in files.items():
        safe_relative_path(path, f"orchestraCore.files[{path!r}]")
        if not isinstance(digest, str) or HEX64.fullmatch(digest) is None:
            raise ValueError(
                f"orchestraCore.files[{path!r}] must be a lowercase SHA-256"
            )

    snapshot_files = file_identities(ROOT / snapshot_path)
    if snapshot_files != files:
        raise ValueError(
            "fork snapshot file set or SHA-256 identities do not match the manifest"
        )

    subprocess.run(
        ["git", "-C", str(ROOT), "merge-base", "--is-ancestor", base_revision, "HEAD"],
        check=True,
    )
    actual_base_tree = git_commit_tree(ROOT, base_revision)
    if actual_base_tree != upstream["baseTree"]:
        raise ValueError("upstream base tree does not match the manifest")

    if source_root is not None:
        if git_identity(source_root, source_revision, source_path) != source_tree:
            raise ValueError(
                "canonical orchestra-core Git tree does not match the manifest"
            )


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--source-root", type=Path)
    args = parser.parse_args()
    try:
        verify(args.source_root.resolve() if args.source_root else None)
    except (
        OSError,
        ValueError,
        subprocess.CalledProcessError,
        json.JSONDecodeError,
    ) as error:
        print(f"orchestra provenance verification failed: {error}", file=sys.stderr)
        return 1
    print("orchestra provenance verified")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
