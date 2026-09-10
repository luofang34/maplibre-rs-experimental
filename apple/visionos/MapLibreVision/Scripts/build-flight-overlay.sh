#!/bin/sh
set -eu
project_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
overlay_manifest="$project_dir/IndicateOverlay/Cargo.toml"
# The overlay owns its dependency graph; the map renderer's Cargo patches must not leak into it.
cd /private/tmp
if [ "$#" -eq 0 ]; then set -- aarch64-apple-visionos aarch64-apple-visionos-sim; fi
for overlay_target in "$@"; do
    case "$overlay_target" in
        aarch64-apple-visionos|aarch64-apple-visionos-sim) ;;
        *) echo "Unsupported Apple target: $overlay_target" >&2; exit 2 ;;
    esac
    cargo +nightly-2026-08-06 build --locked --release -Z build-std=std,panic_abort \
        --manifest-path "$overlay_manifest" -p indicate-svs-bridge --target "$overlay_target"
done
