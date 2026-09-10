#!/bin/sh
set -eu
project_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
overlay_manifest="$project_dir/IndicateOverlay/Cargo.toml"
python3 - "$project_dir/IndicateOverlay" <<'PY'
from pathlib import Path
import sys
for directory in ('bridge/src', 'svs-set/src'):
    for path in (Path(sys.argv[1]) / directory).rglob('*.rs'):
        lines = path.read_text().splitlines()
        assert path.name != 'mod.rs', path
        assert len(lines) <= 500, path
        if path.name == 'lib.rs':
            assert lines[0].startswith('//!') and len(lines) < 100, path
PY
cd /private/tmp
cargo +stable fmt --all --check --manifest-path "$overlay_manifest"
cargo +stable clippy --all-targets --locked --manifest-path "$overlay_manifest" -- -D warnings
cargo +stable test --all-targets --locked --manifest-path "$overlay_manifest"
RUSTDOCFLAGS='-D missing_docs -D rustdoc::broken_intra_doc_links' cargo +stable doc --no-deps --locked --manifest-path "$overlay_manifest"
cargo +stable build --release --locked --manifest-path "$overlay_manifest"
