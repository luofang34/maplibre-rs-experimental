# Input and label validation — 8 September 2026

## Changes exercised

Indirect dragging uses the same captured pointer mapping at globe and immersive scales, with a 0.6 m minimum hand reference depth. Globe surface points follow that pointer; ground grabs use an altitude-preserving plane. The bounded horizon plane rotates its screen motion onto the ground, including vertical input. Two-hand carry, zoom anchors, orbit targets, physical head independence and clearance regressions remain enabled.

The terrain style uses continuous size, rank tiers, distinct major-label weight and restrained halos. Renderer changes cover shared symbol height properties, display-zoom height evaluation for parent tiles, tracking without a trailing gap, orientation-independent SDF smoothing and zero-width halo suppression. Remaining symbol compatibility work is explicitly listed in [LABEL_RENDERING.md](LABEL_RENDERING.md). Cross-platform bindings and future aircraft/ruler behavior are in [INTERACTION_DESIGN.md](INTERACTION_DESIGN.md).

## Command results

| Check | Observed result |
| --- | --- |
| Swift interaction suite | 41 passed |
| Rust core with `headless,thread-safe-futures` and Metal | 468 passed, one existing ignored |
| Bundled style integration suite | 5 passed |
| Changed-module guard | Passed; no touched or new `mod.rs` |
| Workspace `cargo fmt --all -- --check` | Passed |
| Rust release for visionOS device and simulator | Both passed |
| Xcode simulator and signed device builds | Both passed |
| Final physical Vision Pro installation | Confirmed by CoreDevice, bundle `com.sokolysystems.maplibre.vision` |

All six requested workspace gates were executed on the Rust changes. Full CI remains failing: Clippy stops on `maplibre-build-tools`' existing `manual_ok_err`; native all-target tests/release include Android and web targets that reject macOS, and the benchmark has an existing `ProcessedLayers::clone` error. Documentation stops on existing missing docs and broken intra-doc links. Targeted renderer checks and the actual visionOS release builds pass. The final subsequent change is confined to Swift horizon-pan mapping and its tests.

## Runtime observation

The simulator loaded the updated style in immersive mode at 4000 m with `--memory-cap 1000`. The stationary checker passed 22 reports over 31.8 seconds: no pending requests, stable vector/symbol revisions and finest DEM level 13. Continued reports held vector revision 386 and symbol revision 89. No renderer or glyph-loading errors appeared. The screenshot shows differentiated major and secondary type over continuous terrain; the unit/pixel tests, not a still image, check stationary stability. Simulator memory counters and timings do not establish physical-device memory or frame performance.

The final gesture mapping is verified by deterministic input tests; hands-on headset feel is not asserted from simulator observation. Local artifacts use `/private/tmp/map-input-*`, including test/CI logs, builds, simulator capture and installation results.
