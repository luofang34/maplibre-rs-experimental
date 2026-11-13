#!/usr/bin/env python3
"""Unit tests for scripts/check-release-version.py."""

from __future__ import annotations

import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

SCRIPT_PATH = Path(__file__).with_name("check-release-version.py")
SPEC = importlib.util.spec_from_file_location("check_release_version", SCRIPT_PATH)
assert SPEC is not None and SPEC.loader is not None
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


def write_workspace(
    root: Path,
    *,
    workspace_version: str | None = "0.2.0",
    crate_version: str | None = None,
    web_pkg_version: str | None = None,
) -> None:
    ws_pkg = f'version = "{workspace_version}"\n' if workspace_version else ""
    (root / "Cargo.toml").write_text(
        '[workspace]\nmembers = ["maplibre", "internal"]\n\n'
        f"[workspace.package]\n{ws_pkg}"
    )
    (root / "maplibre").mkdir()
    version_line = (
        f'version = "{crate_version}"'
        if crate_version is not None
        else "version.workspace = true"
    )
    (root / "maplibre" / "Cargo.toml").write_text(
        f'[package]\nname = "maplibre"\n{version_line}\n'
    )
    (root / "internal").mkdir()
    (root / "internal" / "Cargo.toml").write_text(
        '[package]\nname = "internal"\nversion = "9.9.9"\npublish = false\n'
    )
    if web_pkg_version is not None:
        (root / "web").mkdir()
        (root / "web" / "package.json").write_text(
            json.dumps({"name": "maplibre-web", "version": web_pkg_version})
        )


class CheckReleaseVersionTests(unittest.TestCase):
    def test_real_repository_passes_for_matching_tag(self) -> None:
        repo_root = SCRIPT_PATH.resolve().parents[1]
        version = MODULE.check_versions(repo_root)
        self.assertEqual(MODULE.check_versions(repo_root, tag=f"v{version}"), version)

    def test_matching_workspace_web_and_tag_pass(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            write_workspace(root, workspace_version="0.3.1", web_pkg_version="0.3.1")
            self.assertEqual(
                MODULE.check_versions(root, tag="refs/tags/v0.3.1"),
                "0.3.1",
            )

    def test_detects_tag_mismatch_and_malformed_tag(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            write_workspace(root, workspace_version="0.3.1")
            with self.assertRaisesRegex(ValueError, "Version mismatch"):
                MODULE.check_versions(root, tag="v0.3.2")
            with self.assertRaisesRegex(ValueError, "must start with 'v'"):
                MODULE.check_versions(root, tag="0.3.1")

    def test_detects_web_package_json_mismatch(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            write_workspace(root, workspace_version="0.3.1", web_pkg_version="0.3.0")
            with self.assertRaisesRegex(ValueError, "web/package.json=0.3.0"):
                MODULE.check_versions(root, tag="v0.3.1")

    def test_detects_crate_manifest_mismatch(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            write_workspace(root, workspace_version="0.3.1", crate_version="0.2.9")
            with self.assertRaisesRegex(ValueError, "maplibre/Cargo.toml=0.2.9"):
                MODULE.check_versions(root)


if __name__ == "__main__":
    unittest.main()
