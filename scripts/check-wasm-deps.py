#!/usr/bin/env python3
"""Guard the wasm32 normal dependency tree against native/OS-only crates."""

from __future__ import annotations

import argparse
from pathlib import Path
import re
import subprocess
import sys

# Native/OS runtime, socket, TLS, HTTP-middleware, and SQLite crates that must never enter
# any wasm32 normal dependency graph (`web` or `maplibre`).
FORBIDDEN_WASM_CRATES: frozenset[str] = frozenset(
    {
        "curl",
        "curl-sys",
        "http-cache-reqwest",
        "hyper",
        "hyper-util",
        "libsqlite3-sys",
        "mio",
        "openssl",
        "openssl-sys",
        "reqwest-middleware",
        "rusqlite",
        "rustls",
        "socket2",
        "tokio",
        "tokio-util",
    }
)

# `maplibre` itself uses `platform::web::http_client` (`web_sys::Window::fetch`) on wasm32
# and must not pull `reqwest` at all; `web` currently links `reqwest`'s wasm fetch shim for
# browser examples without any native hyper/tokio/TLS dependencies.
FORBIDDEN_CORE_CRATES: frozenset[str] = FORBIDDEN_WASM_CRATES | {"reqwest"}

CRATE_TOKEN_RE = re.compile(r"(?:^|[─\s])([a-zA-Z0-9_-]+)\s+v\d+\.\d+\.\d+")


def extract_crate_names(cargo_tree_output: str) -> set[str]:
    names: set[str] = set()
    for line in cargo_tree_output.splitlines():
        match = CRATE_TOKEN_RE.search(line)
        if match:
            names.add(match.group(1))
    return names


def find_forbidden_crates(
    cargo_tree_output: str,
    forbidden: frozenset[str] | set[str],
) -> list[str]:
    present = extract_crate_names(cargo_tree_output)
    return sorted(present & set(forbidden))


def run_cargo_tree(root: Path, package: str, features: list[str]) -> str:
    cmd = [
        "cargo",
        "tree",
        "-p",
        package,
        "--target",
        "wasm32-unknown-unknown",
        "-e",
        "normal",
    ]
    if features:
        cmd.extend(["--features", ",".join(features)])
    result = subprocess.run(
        cmd,
        cwd=root,
        capture_output=True,
        text=True,
        check=False,
    )
    if result.returncode != 0:
        raise RuntimeError(
            f"Command failed ({' '.join(cmd)}):\n{result.stderr.strip()}"
        )
    return result.stdout


def check_workspace_wasm_trees(
    root: Path,
    runner=run_cargo_tree,
) -> list[str]:
    checks: list[tuple[str, list[str], frozenset[str]]] = [
        ("web", [], FORBIDDEN_WASM_CRATES),
        ("web", ["web-webgl"], FORBIDDEN_WASM_CRATES),
        ("maplibre", [], FORBIDDEN_CORE_CRATES),
        ("maplibre", ["web-webgl"], FORBIDDEN_CORE_CRATES),
    ]
    errors: list[str] = []
    for package, features, forbidden in checks:
        label = f"{package} ({','.join(features) if features else 'default'})"
        output = runner(root, package, features)
        violations = find_forbidden_crates(output, forbidden)
        if violations:
            errors.append(
                f"{label} pulls forbidden wasm32 crates: {', '.join(violations)}"
            )
    return errors


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--root",
        type=Path,
        default=Path(__file__).resolve().parents[1],
        help="Workspace root directory",
    )
    args = parser.parse_args(argv)
    try:
        errors = check_workspace_wasm_trees(args.root)
    except RuntimeError as error:
        sys.stderr.write(f"{error}\n")
        return 1
    if errors:
        for line in errors:
            sys.stderr.write(f"FAIL: {line}\n")
        return 1
    sys.stdout.write(
        "PASS: wasm32 dependency trees for `web` and `maplibre` contain no forbidden native crates.\n"
    )
    return 0


def __getattr__(name: str):
    if name == "test":
        import importlib.util

        spec = importlib.util.spec_from_file_location(
            f"{__name__}.test", Path(__file__).with_name("check-wasm-deps.test.py")
        )
        assert spec is not None and spec.loader is not None
        module = importlib.util.module_from_spec(spec)
        sys.modules[spec.name] = module
        spec.loader.exec_module(module)
        return module
    raise AttributeError(f"module {__name__!r} has no attribute {name!r}")


if __name__ == "__main__":
    raise SystemExit(main())
