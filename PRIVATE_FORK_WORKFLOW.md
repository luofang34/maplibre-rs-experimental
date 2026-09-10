# Private fork integration and review branches

`private/main` is the integrated, installable tree. Renderer, platform hosts, styles,
regression tests, and their validation records belong there together.

An active review branch represents one independently reviewable issue. A device
installation, screenshot round, or debugging session does not get a branch. Keep
follow-up fixes on the issue's branch and include its regression guardrail.

The current review topics are:

| Branch | Scope |
| --- | --- |
| `fix/xr-world-aligned-labels` | Gravity-aligned labels, stable terrain-anchor occlusion and GPU head-roll regression |
| `fix/terrain-bridge-depth` | Shared casing/deck tangent projection, transparent-depth rejection and bridge GPU regression |
| `fix/visionos-window-flight-library` | Desk/immersive scene lifecycle and independent row deletion with replay preservation |
| `feat/visionos-head-mounted-display` | Independent Indicate angular set, world-referenced readouts and geographic ownship marker |
| `fix/indicate-hud-layout` | Transparent readouts, rolling compass, heading/track qualification and missing-data indications |
| `fix/visionos-conformal-hud` | Flight-referenced angular symbology, per-eye projection, shared raster storage and bounded overlay buffers |
| `feat/visionos-ownship-controls` | On-demand compact controls, temporary free look, cancellable return countdown and boarding handoff |
| `feat/visionos-flight-library` | Liberty KML, removable demos, bounded Share extension inbox and document registration |
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
The HUD refinement stack starts at integrated `a6ef7869`. Its instrument layout
belongs with Indicate. Angular geometry and playback policy are independent of
Metal; the Apple renderer, document sharing and scene lifecycle stay in the
platform host. These changes introduce no aviation dependencies into MapLibre core.
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

## HUD and sharing validation — 2026-09-10

- Swift: 95 tests passed in debug and release. The overlay workspace passed fmt,
  strict clippy, nine tests, missing-docs/broken-link checks and release build.
- Simulator and unsigned device release builds passed, including the Share
  extension. Built-bundle checks validate the document and extension registration.
  The app and Share extension use the same registered App Group and signed
  provisioning profiles. Installation on the paired Vision Pro passed.
  Physical AirDrop transport remains unverified.
- Simulator replays reached the final observations at 3,057 seconds for Liberty
  and 379.125 seconds for Mach Loop. Visual checks covered missing-data readouts,
  compass, attitude ladder and prograde cue. Unit tests cover head rotation,
  retrograde direction and countdown cancellation.
- HUD readout raster CPU p95 measured approximately 0.85–1.08 ms in the simulator.
  Three shared IOSurfaces use 8.64 MB of pixel storage, without duplicate CPU/GPU
  raster copies. Route and angular-symbol buffers are bounded and reused.
- All MapLibre baseline gates were attempted again: formatting fails, and the
  compilation/documentation gates stop at the missing SQLite generated binding.
  No MapLibre core Rust changes are included in this refinement stack.

## Head-mounted display and rendering validation — 2026-09-10

The four HMD refinement topics start at integrated `e8f59ddc`. The first two
change MapLibre core only. Scene lifecycle, flight library and Metal projection
remain Apple host concerns. Indicate owns the bounded east/north/up angular
scene and qualified numeric instruments; it takes no head pose. The host projects
that scene per eye. The independent Indicate source is committed locally on
`feat/head-mounted-instrument-set` at `500ed883`; its identical vendored source
is included here, without publishing to the public Indicate remote.

- MapLibre library: 494 tests passed, one ignored, including GPU head-roll,
  stationary-frame, bridge casing/deck and buried-tunnel regressions.
- Swift: 95 tests passed in both debug and release. Deleting another flight
  preserves the current replay generation, camera revision, time and selection.
- Indicate's full workspace and the app overlay passed fmt, strict clippy, tests,
  missing-docs/broken-link checks and release builds. The app overlay has 13 tests.
- Native renderer device/simulator release builds and Xcode device/simulator
  builds passed. Both signed bundles contain the registered App Group.
  The app installed on the paired Vision Pro. Simulator checks covered Liberty
  with missing attitude/heading and Mach Loop with simulated attitude. The desk
  shell is absent during immersive replay; compass numerals are continuous vectors.
- Angular storage is fixed at 1,024 segments (24,580 bytes across the C ABI);
  overlay buffers are reused. This is not a new physical-device memory stress test.
- All full MapLibre workspace CI gates were attempted. Formatting passes; strict
  clippy and documentation stop in existing build-tool code. Host all-target
  tests/release include incompatible Android/Web targets and benchmark API errors.
  These baseline failures are separate from the passing map-library and native
  visionOS checks.
- FAA AC 20-185A sections 4.1.2.3–4.1.2.5 inform world alignment and truthful
  trajectory/data indications. This implementation is not a certification claim.
