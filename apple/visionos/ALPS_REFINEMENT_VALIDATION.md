# Alps refinement, roads, tilt, and panel validation

Validated 2026-09-09.

## Changes and guardrails

- Tilt reports the rendered scene normal relative to room gravity. Absolute tilt
  and Level map rotate around the orbit target (or the fixed camera eye), including
  after a globe-to-ground pinch zoom. Head tracking remains independent. Three
  Swift regressions cover the normal, target/eye preservation, and head movement.
- Drape refinement prioritizes the nearest requested detail. Half the texture
  allowance stays available for complete replacements. Completed child textures
  release their fallback ancestor. Coverage, foreground priority, and texture
  reuse have behavioral regressions. Paint zoom has hysteresis and is shared
  between layer metadata and drape generation to avoid boundary oscillation.
- Displayed fallback geometry receives current paint and elevation updates.
  Elevated roads use the same map-space width as adjoining draped roads, so they
  foreshorten with perspective. Fractional subdivision intersections stay on the
  original line. GPU tests cover near/far deck width, tunnel occlusion, and a parent
  road remaining above a finer DEM; a geometry regression checks collinearity.
- The system window has bounded resizability, scrollable content, and persistent
  Level/Leave controls. Simulator screenshots verified that both actions fit in
  the default panel without clipped content.

## Executed checks

- Rust renderer unit tests: **481 passed, 1 existing ignored**.
- Default visionOS style tests: **5 passed**.
- Swift interaction tests: **47 passed**.
- `cargo fmt --all -- --check`: passed.
- Both visionOS Rust release targets: passed.
- Simulator and signed Vision Pro Xcode builds: passed.
- `git diff --check`: passed for the integrated change.

The required full-workspace gates were executed. Clippy stops at the existing
`maplibre-build-tools/src/mbtiles.rs` manual-`ok` lint. Workspace tests and release
build include Android code that cannot compile on macOS. Missing-docs fails in
`maplibre-build-tools`; broken-link rustdoc reports the existing `WorldCoors` link
in `coords.rs`. These gates are not claimed green.

## Runtime evidence

The simulator entered the default Innsbruck immersive view with a 900 MB available
memory cap. It settled with zero pending requests, 16 drapes (85 MB), finest DEM
zoom 13, and approximately 412 MB of accounted vector geometry. Over 71 seconds,
44 consecutive residency reports retained identical geometry and symbol revisions
(801 and 257). The final intervals averaged roughly 7–8 ms per frame. No renderer
errors were logged. This verifies the fixed-view loading case, not every head or
hand trajectory on hardware. Simulator process-footprint reporting does not
include the device's Metal allocation accounting.

The final signed build was installed on Vision Pro `00008112-001E152A0CE8A01E`,
installation database sequence 1740. Device visual confirmation remains separate
from the automated simulator and GPU checks.

## Reference behavior

[MapLibre line paint](https://maplibre.org/maplibre-style-spec/layers/#line)
defines cartographic line styling. The opt-in terrain structure metadata is a
renderer extension; it is not presented as a standard MapLibre paint property.
[OpenMapTiles transportation](https://github.com/openmaptiles/openmaptiles/blob/master/layers/transportation/transportation.yaml)
identifies bridges/tunnels through `brunnel`.
[OSM layer](https://wiki.openstreetmap.org/wiki/Key:layer) describes relative order,
not surveyed metres. DEM-based profiles therefore remain estimates unless an
absolute elevation is supplied; these changes do not manufacture surveyed bridge
engineering or tunnel interiors from OSM layer numbers.

Window sizing follows Apple's
[visionOS window guidance](https://developer.apple.com/documentation/visionos/positioning-and-sizing-windows)
and [content-size resizability](https://developer.apple.com/documentation/swiftui/windowresizability).
