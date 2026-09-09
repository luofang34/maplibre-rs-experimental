# Zero-tilt crash validation

Validated 2026-09-09.

## Device evidence

The Vision Pro report `MapLibreVision-2026-09-09-034749.ips` records SIGABRT
on the `maplibre render` thread at 03:47:48 EDT. The app's persistent panic log
identifies `ViewProjection::invert`: `Unable to invert view projection`.
The stack proceeds through `ViewState::frustum_corners`, Mercator tile covering,
vector requests, and `HeadlessMap::run_xr_frame`. This report is a renderer panic,
not a jetsam memory termination.

## Fix and regression protection

CPU unprojection inverts camera and projection transforms separately and keeps
those factors separate when applying them to a clip-space point. This avoids
cancellation between large Mercator translations and small near-plane depths.
Terrain coverage, horizon calculations, picking, and pointer-anchored gestures
use the same calculation. Singular and non-finite inversions return typed errors;
Mercator covering propagates the cause instead of aborting the host.

A numerical regression failed before the change: a valid external view at 150 m
with a very narrow near plane produced over one percent error in a far corner.
The test now checks 27,270 frusta spanning head yaw, pitch across the horizon,
three heights, five clip ranges, request overscan, and surround coverage. This is
an amplified precision regression; the exact device pose at the abort was not
recorded. Separate tests exercise singular and non-finite projections and error
propagation through tile covering.

The GPU regression repeatedly approaches and crosses level view with off-axis
head yaw and roll, draws both eyes, and verifies terrain remains present and every
pixel is covered by ground or sky. Terrain gesture regressions continue to verify
that pan and zoom preserve their pointer anchor.

`ViewProjection::invert` and `ViewState::frustum_corners` now return `Result`.
Callers that hold a `ViewState` should use `inverted_view_projection` so projection
and translation remain separate. Desktop query callers are updated and compile.

## Executed checks

- Renderer unit/GPU tests: **486 passed, 1 existing ignored**.
- visionOS style tests: **5 passed**.
- Swift interaction tests: **47 passed**.
- Desktop host `cargo check -p maplibre-winit --all-targets`: passed.
- `cargo fmt --all -- --check`: passed.
- Rust release builds for visionOS device and simulator: passed.
- Signed Vision Pro and simulator Xcode builds: passed.
- Final GPU regression after the test-module layout change: passed.

The complete workspace gates were executed. Clippy stops at the existing manual
`ok` implementation in `maplibre-build-tools`. Workspace tests/release/docs include
Android and Web targets incompatible with the macOS host; tests also expose an
existing benchmark `ProcessedLayers::clone` error. The missing-docs gate stops in
`maplibre-build-tools`. These workspace gates are not claimed green.

The signed app was installed on Vision Pro `00008112-001E152A0CE8A01E`,
installation database sequence **1748**. Device visual confirmation of the user's
exact motion is separate from the automated checks.
