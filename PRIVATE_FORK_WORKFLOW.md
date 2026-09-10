# Private fork integration and review branches

`private/main` is the integrated, installable tree. Renderer, platform hosts, styles,
regression tests, and their validation records belong there together.

An active review branch represents one independently reviewable issue. A device
installation, screenshot round, or debugging session does not get a branch. Keep
follow-up fixes on the issue's branch and include its regression guardrail.

The current review topics are:

| Branch | Scope |
| --- | --- |
| `feat/indicate-svs-overlay` | Independent Indicate instrument set, telemetry validity, bounded scene bridge and Rust regression gates |
| `feat/visionos-fpv-replay` | FPV/chase/free camera handoff, track import, simulated Mach Loop example, Apple-rendered overlay and FAA reference viewer |
| `feat/visionos-desk-flight-replay` | Restorable native desk Volume, bounded manipulation, licensed ADS-B approach replay and terrain follow camera |
| `fix/visionos-surface-controls` | Geographic drag and zoom anchors, focus-preserving tilt, gesture intent, first-view placement |
| `fix/visionos-rotation-stability` | Normalized gesture rotations and finite tilt reporting |
| `fix/xr-pose-validation` | Validate every eye before starting a frame; preserve the last valid view |
| `fix/terrain-detail-streaming` | Balance visible texture detail and bound movement prefetch |
| `fix/stable-view-unprojection` | Checked and factored CPU unprojection for level views, terrain selection, and gestures |
| `fix/foreground-terrain-refinement` | Foreground priority, bounded replacement textures, fallback retirement, stable paint |
| `fix/terrain-road-perspective` | Road alignment, perspective width, bridge elevation, tunnel occlusion |
| `fix/visionos-absolute-tilt` | Room-relative map tilt with independent head tracking |
| `fix/visionos-map-panel` | Resizable controls with persistent Level and Leave actions |

These branches form a dependency stack above the integrated globe/terrain work.
The stable unprojection change builds on the integrated `8d5b79e4` baseline; use
that commit as its review base to exclude the preceding validation documentation.
The rotation stability change uses integrated `cc7d163b` as its review base.
The XR validation and terrain streaming topics build on that stack.
The surface controls topic uses integrated `735b6f27` as its review base.
The desk Volume and recorded flight topic uses integrated `570531b2` as its review base.
The Indicate overlay topic uses integrated `d8b3a49b` as its review base; the FPV
integration builds on the overlay topic at `3eb19019`. The independent set belongs
with Indicate; the Swift host and import integration are MapLibre platform topics.
Their parent branch is the review base; comparing every branch directly with
upstream `main` would include unrelated prerequisites. Renderer topics can be
prepared for MapLibre review as their dependencies become available upstream.
The Swift host changes remain separate platform topics. A topic branch is not a
claim of complete MapLibre style-spec conformance or upstream acceptance.

The earlier `feat/*`, `terrain/*`, and `fix/*` branches record prerequisites. Keep
only branches still useful for reviewing those prerequisites; consolidate them by
issue when preparing their upstream PR, and retire superseded review branches.
`fix/immersive-rendering` is a frozen integration checkpoint, not a new PR topic.
New work starts from `main` or its explicit prerequisite branch.

Before rewriting a published topic, preserve the old refs and verify that the
integrated tree still contains every intended change. Never rewrite or publish to
the public upstream as part of private-fork maintenance. Current-session approval
is required for force pushes or remote branch deletion unless already authorized.

Run fmt, clippy, tests, missing-docs and broken-link documentation checks, and the
release build. Record platform-specific baseline failures separately from the
feature's passing checks. Install from the integrated tree, not a partial topic.

## FPV integration validation — 2026-09-10

- Swift: 87 tests passed in both debug and release. The overlay workspace passed
  formatting, strict clippy, eight tests, missing-docs/broken-link checks and release.
- Simulator and signed device builds passed. Both flight examples completed in
  the simulator; the GPX preview required an elevation reference before import.
  The app installed and launched on the paired Vision Pro. AirDrop transport
  between physical devices remains untested.
- The actual Indicate/Apple overlay measured approximately 2.1 ms p95 per raster
  in the simulator at 20 Hz, with head tracking updated each display frame.
- IndicateAppleDisplay's `fix/visionos-display-scale` topic passed 45 tests,
  its benchmark and a visionOS compile check. Its full CI still reports upstream
  conformance-corpus drift; the app pins the validated bridge revision.
- The MapLibre baseline CI attempt was not green: formatting returned nonzero,
  and compilation gates stopped on a missing generated SQLite binding in the
  validation cache. These failures are separate from the passing overlay gates.
