# Immersive rendering and interaction validation

Validated on 2026-09-08. Renderer changes and the visionOS host are kept in the private
`luofang34/maplibre-rs-globe-private` fork, branch `fix/immersive-rendering`.

## Resulting behavior

- Globe dragging crosses both polar caps. One pinch navigates; two-hand common motion
  places the globe without a mode button. Spread, twist and common motion have separate,
  latched intent. In free immersive camera, common hand motion orbits and pitches around
  the center ground target. A fixed-viewpoint policy preserves the eye for future FPV.
  Physical head movement cannot steer a held gesture. A small twist starts rotation.
- External tile coverage stays continuous across the horizon, prioritizes visible ground,
  continues loading while stationary, and shares detail hysteresis and content across eyes.
  Terrain retains ancestor coverage during refinement and cache reuse.
- Negative-elevation ocean terrain is no longer hidden by the sea-level globe background.
  Full immersion fills the sky behind valleys, including during globe/ground transitions;
  table-globe surroundings remain transparent. Far-depth background coverage has GPU
  regressions at ordinary and multisampled settings.
- Style document order is assigned during deserialization. Asynchronous tile completion
  therefore cannot reorder water, roads and bridges. Drape road widths account for the
  target tile's resolution, and loaded vector metadata follows view zoom. Near and far
  tiles retain consistent road widths and painter order.
- Label placement uses a spatial collision grid, cached component bounds, per-anchor
  distance/detail limits, layer priority, sort keys, padding, overlap/ignore/optional flags,
  and upright line orientation. The app enables fewer place layers at coarse zooms.
  Text and icons retain glyph/sprite assets, properties and IDs through worker transport.
  The rendered-symbol query and short-pinch selection expose the placed feature.
- Ground/sea height anchors and text/icon offsets apply above terrain. The depth snapshot
  hides terrain-occluded anchors without cutting a billboard into fragments. A component
  that crosses the eye plane is hidden as a whole. Disabling this guard makes the spike
  regression fail with 3,478 stretched glyph pixels; restoring it passes. The supplied
  screenshot alone cannot establish that every possible terrain spike has this cause.
- Tunnels use dashed subdued lines; bridge casing and inner strokes follow OSM layer
  ordering, including railways. The layer ordinal is not treated as an elevation in metres.
  This is cartographic bridge/tunnel handling; independent bridge deck meshes and tunnel
  interiors require source geometry/elevations that these vector tiles do not provide.
- CPU retention accounts for symbol atlases, glyph geometry, query properties/indexes,
  raster and DEM data. The retention target is 128 MiB, requests are limited to 12 with
  terrain capacity reserved, and drapes are limited to 64 (32 under pressure). The host
  uses both process footprint and available memory with a 3 GiB process envelope; it
  throttles before that envelope is reached. Rust and Swift do not themselves prevent OOM.
- Every changed `mod.rs` is renamed to its parent module file. Touched Rust files are
  below 500 lines. `scripts/check-module-layout.py --base <comparison-commit>` rejects
  additions or modifications of `mod.rs`, while allowing their removal.

## Checks actually run

| Check | Result |
| --- | --- |
| Swift interaction tests | 22 passed |
| Core Rust tests, `headless,thread-safe-futures` | 434 passed; 1 existing ignored |
| Application road/rail filter and bridge-order tests | 2 passed |
| Root `cargo fmt --all -- --check` | Passed |
| WebGL worker build, `wasm32-unknown-unknown` | Passed |
| Rust release, visionOS device and simulator | Passed |
| Xcode signed device and simulator builds | Passed |
| Module-layout guard | Passed |
| Physical Vision Pro installation | Confirmed; xrOS 27.0 (24M5361a) detected automatically |

The simulator opens the immersive scene with `--enter --mode immersive --height 4000`.
The inspected screenshot shows continuous foreground terrain and sky and a sparser label
layout. A stationary run reached zero pending requests with stable pool revision 656,
158 resident tiles and 58 terrain draws. Process footprint held near 393 MB in that run.
Simulator numbers do not measure physical-device memory pressure or binocular comfort.

The physical-device remote launch timed out. Installation is confirmed, but a new
physical-device rendering/memory run is not claimed. The prior device log showed about
2.8 GiB footprint; its available Jetsam report names `dtremotedisplayd` as the killed
process, with MapLibre the largest process. It does not prove MapLibre was the victim of
that particular report.

All requested full workspace CI commands were run before publication. The workspace is
not green: strict Clippy stops at `maplibre-build-tools` (`manual_ok_err`); native
`test --all-targets` and `build --release` include Android/web crates that reject macOS;
strict documentation fails on missing docs and links such as `WorldCoors`, `WorldTileCoors`
and `Transferables`. The targeted checks above completed independently. No passing full
workspace CI claim is made.

Measurement and vehicle following are designed in [INTERACTION_DESIGN.md](INTERACTION_DESIGN.md),
not implemented application features. Symbol support does not claim complete GL JS parity:
curved/repeated line text, variable anchors, vertical shaping, and the full range of
feature-dependent font/paint evaluation remain separate work. The query targets rendered
symbols; it is not a general query for all rendered polygon/line features.

## References and artifacts

Behavior was checked against the [MapLibre style specification](https://maplibre.org/maplibre-style-spec/layers/),
[GL JS rendered feature queries](https://maplibre.org/maplibre-gl-js/docs/API/classes/Map/#queryrenderedfeatures),
[OpenMapTiles schema](https://openmaptiles.org/schema/), and
[Positron bridge/tunnel styles](https://github.com/openmaptiles/positron-gl-style/blob/master/style.json).

Local logs and screenshots: `/private/tmp/immersive-next-tests.log`,
`/private/tmp/immersive-next-swift.log`, `/private/tmp/immersive-style-test.log`,
`/private/tmp/immersive-spike-mutation.log`, `/private/tmp/immersive-next-vision-release.log`,
`/private/tmp/immersive-final-ci-results.json`, `/private/tmp/immersive-final-ci-*.log`,
`/private/tmp/immersive-final-web-check.log`, `/private/tmp/immersive-final-device-build.log`,
`/private/tmp/immersive-final-simulator-build.log`, `/private/tmp/immersive-final-install.log`,
and `/private/tmp/immersive-final-simulator.png`.
