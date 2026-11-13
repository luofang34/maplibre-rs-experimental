#!/usr/bin/env python3
"""Verify release version parity across Cargo manifests, web/package.json, and git tags."""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import re
import sys
import tomllib

SEMVER_RE = re.compile(
    r"^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$"
)


def normalize_tag(tag: str) -> str:
    stripped = tag.removeprefix("refs/tags/")
    if not stripped.startswith("v"):
        raise ValueError(f"Release tag '{tag}' must start with 'v' (expected vX.Y.Z)")
    version = stripped[1:]
    if not SEMVER_RE.match(version):
        raise ValueError(f"Release tag '{tag}' does not contain a valid semver version")
    return version


def check_versions(root: Path, tag: str | None = None) -> str:
    cargo_toml_path = root / "Cargo.toml"
    if not cargo_toml_path.is_file():
        raise ValueError(f"Workspace Cargo.toml not found at {cargo_toml_path}")

    workspace_doc = tomllib.loads(cargo_toml_path.read_text())
    workspace_section = workspace_doc.get("workspace", {})
    workspace_pkg_version = workspace_section.get("package", {}).get("version")
    members: list[str] = workspace_section.get("members", [])

    discovered: list[tuple[str, str]] = []
    if workspace_pkg_version is not None:
        if not SEMVER_RE.match(str(workspace_pkg_version)):
            raise ValueError(
                f"Invalid [workspace.package].version '{workspace_pkg_version}' in Cargo.toml"
            )
        discovered.append(("Cargo.toml [workspace.package]", str(workspace_pkg_version)))

    for member in members:
        manifest_path = root / member / "Cargo.toml"
        if not manifest_path.is_file():
            continue
        doc = tomllib.loads(manifest_path.read_text())
        package = doc.get("package", {})
        if package.get("publish") is False:
            continue
        raw_version = package.get("version")
        if isinstance(raw_version, dict) and raw_version.get("workspace") is True:
            if workspace_pkg_version is None:
                raise ValueError(
                    f"{member}/Cargo.toml inherits version.workspace = true, "
                    "but [workspace.package].version is missing"
                )
            resolved = str(workspace_pkg_version)
        elif isinstance(raw_version, str):
            resolved = raw_version
        else:
            raise ValueError(f"{member}/Cargo.toml has no valid package.version")
        if not SEMVER_RE.match(resolved):
            raise ValueError(f"Invalid version '{resolved}' in {member}/Cargo.toml")
        discovered.append((f"{member}/Cargo.toml", resolved))

    if not discovered:
        raise ValueError("No publishable Cargo package versions found in workspace")

    web_pkg_json = root / "web" / "package.json"
    if web_pkg_json.is_file():
        pkg_data = json.loads(web_pkg_json.read_text())
        web_version = pkg_data.get("version")
        if web_version is not None:
            discovered.append(("web/package.json", str(web_version)))

    canonical_source, canonical_version = discovered[0]
    mismatches = [
        f"{source}={version} (expected {canonical_version} from {canonical_source})"
        for source, version in discovered[1:]
        if version != canonical_version
    ]
    if tag is not None:
        tag_version = normalize_tag(tag)
        if tag_version != canonical_version:
            mismatches.append(
                f"tag {tag} -> {tag_version} (expected {canonical_version} from {canonical_source})"
            )

    if mismatches:
        raise ValueError("Version mismatch:\n  - " + "\n  - ".join(mismatches))

    return canonical_version


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--root",
        type=Path,
        default=Path(__file__).resolve().parents[1],
        help="Repository root directory",
    )
    parser.add_argument(
        "--tag",
        default=None,
        help="Git release tag (e.g. v0.1.0 or refs/tags/v0.1.0)",
    )
    parser.add_argument(
        "--print-version",
        action="store_true",
        help="Print only the verified semver string on stdout",
    )
    args = parser.parse_args(argv)

    tag = args.tag
    if tag is None:
        ref_type = os.environ.get("GITHUB_REF_TYPE")
        ref_name = os.environ.get("GITHUB_REF_NAME")
        if ref_type == "tag" and ref_name:
            tag = ref_name

    try:
        version = check_versions(args.root, tag=tag)
    except ValueError as error:
        sys.stderr.write(f"FAIL: {error}\n")
        return 1

    if args.print_version:
        sys.stdout.write(f"{version}\n")
    else:
        tag_note = f" (matches tag {tag})" if tag else ""
        sys.stdout.write(f"PASS: release version {version} is consistent{tag_note}.\n")
    return 0


def __getattr__(name: str):
    if name == "test":
        import importlib.util

        spec = importlib.util.spec_from_file_location(
            f"{__name__}.test", Path(__file__).with_name("check-release-version.test.py")
        )
        assert spec is not None and spec.loader is not None
        module = importlib.util.module_from_spec(spec)
        sys.modules[spec.name] = module
        spec.loader.exec_module(module)
        return module
    raise AttributeError(f"module {__name__!r} has no attribute {name!r}")


if __name__ == "__main__":
    raise SystemExit(main())
