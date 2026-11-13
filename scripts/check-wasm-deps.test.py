#!/usr/bin/env python3
"""Unit tests for scripts/check-wasm-deps.py."""

from __future__ import annotations

import importlib.util
from pathlib import Path
import unittest

SCRIPT_PATH = Path(__file__).with_name("check-wasm-deps.py")
SPEC = importlib.util.spec_from_file_location("check_wasm_deps", SCRIPT_PATH)
assert SPEC is not None and SPEC.loader is not None
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)

CLEAN_WEB_TREE = """\
web v0.1.0 (/workspace/web)
├── js-sys v0.3.107
├── maplibre v0.1.0 (/workspace/maplibre)
│   ├── wasm-bindgen v0.2.130
│   ├── web-sys v0.3.107
│   └── wgpu v30.0.2
└── wasm-bindgen-futures v0.4.80
"""

DIRTY_TREE_TOKIO_SQLITE = """\
web v0.1.0 (/workspace/web)
├── maplibre v0.1.0 (/workspace/maplibre)
│   ├── rusqlite v0.41.1
│   │   └── libsqlite3-sys v0.35.0
│   └── tokio v1.32.0
│       ├── mio v1.2.5
│       └── socket2 v0.6.5
└── openssl v0.10.70
    └── openssl-sys v0.9.105
"""


class CheckWasmDepsTests(unittest.TestCase):
    def test_extracts_crate_names_from_cargo_tree(self) -> None:
        names = MODULE.extract_crate_names(CLEAN_WEB_TREE)
        self.assertEqual(
            names,
            {"web", "js-sys", "maplibre", "wasm-bindgen", "web-sys", "wgpu", "wasm-bindgen-futures"},
        )

    def test_accepts_clean_wasm_tree(self) -> None:
        self.assertEqual(
            MODULE.find_forbidden_crates(CLEAN_WEB_TREE, MODULE.FORBIDDEN_CORE_CRATES),
            [],
        )

    def test_detects_forbidden_native_crates(self) -> None:
        violations = MODULE.find_forbidden_crates(
            DIRTY_TREE_TOKIO_SQLITE,
            MODULE.FORBIDDEN_WASM_CRATES,
        )
        self.assertEqual(
            violations,
            ["libsqlite3-sys", "mio", "openssl", "openssl-sys", "rusqlite", "socket2", "tokio"],
        )

    def test_core_maplibre_forbids_reqwest(self) -> None:
        tree_with_reqwest = CLEAN_WEB_TREE + "└── reqwest v0.12.28\n"
        calls: list[tuple[str, tuple[str, ...]]] = []

        def fake_runner(_root: Path, package: str, features: list[str]) -> str:
            calls.append((package, tuple(features)))
            return tree_with_reqwest if package == "maplibre" else CLEAN_WEB_TREE

        errors = MODULE.check_workspace_wasm_trees(Path("/workspace"), runner=fake_runner)
        self.assertEqual(
            calls,
            [
                ("web", ()),
                ("web", ("web-webgl",)),
                ("maplibre", ()),
                ("maplibre", ("web-webgl",)),
            ],
        )
        self.assertEqual(len(errors), 2)
        self.assertIn("maplibre (default) pulls forbidden wasm32 crates: reqwest", errors[0])
        self.assertIn("maplibre (web-webgl) pulls forbidden wasm32 crates: reqwest", errors[1])


if __name__ == "__main__":
    unittest.main()
