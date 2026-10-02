# MapLibre Vision

A visionOS host for maplibre-rs with globe and immersive terrain views, map gestures,
and recorded flight replay. The Swift app uses CompositorServices and the Rust renderer
through `maplibre_visionos.h`.

## Build and run

Requirements: Xcode with the visionOS platform, `xcodegen`, and the nightly toolchain
specified in `rust-toolchain.toml` with `rust-src`. The renderer has its own Cargo workspace.

```sh
cd apple/visionos
cargo build --release
cd MapLibreVision
sh Scripts/build-flight-overlay.sh aarch64-apple-visionos-sim
xcodegen generate
xcodebuild -project MapLibreVision.xcodeproj -scheme MapLibreVision \
  -destination 'generic/platform=visionOS Simulator' -derivedDataPath build build
xcrun simctl boot "Apple Vision Pro"
xcrun simctl install booted build/Build/Products/Debug-xrsimulator/MapLibreVision.app
xcrun simctl launch --console-pty booted com.sokolysystems.maplibre.vision --mode immersive
```

For a device build, use `cargo build --release --target aarch64-apple-visionos` in
`apple/visionos` and `sh Scripts/build-flight-overlay.sh aarch64-apple-visionos` in
`MapLibreVision`. Set the signing team in `MapLibreVision/project.yml` before generating
the Xcode project. Both the app and Share extension need the App Group
`group.com.sokolysystems.maplibre.vision`.

## Navigation and replay

The map controls open in a standard window. Table globe and Immersive select viewpoints
in the same scene; zoom moves between them. A stationary scene pinch recalls the controls.
Pinch a label or icon to select it, hold and drag to turn the globe or pan terrain, and
spread or twist two pinches to zoom or rotate. Moving both hands together carries the
table globe; in immersive mode, horizontal movement orbits and vertical movement tilts.
Head movement remains independent of navigation.

Select a recording and choose Fly track for FPV replay. Dragging starts free look and
pauses playback. Stay free cancels the automatic return; FPV or Chase returns to the aircraft.
Look forward recenters the viewing reference.

Launch options include `--mode tableGlobe`, `--mode immersive`, `--height N` (metres above
the focus), `--replay`, `--track mach-loop --replay`, and `--replay-rate 4`. Mach Loop is
simulated. Replay preserves missing telemetry and stops at the recording's coverage limit.
The Indicate HWD reports missing, stale, or failed fields; GPS ground speed is not IAS.
Replay is not intended for operational navigation.

## Track imports

Import track, AirDrop, Open With, and the Add to MapLibre Vision Share extension accept
GPX, timestamped absolute-altitude KML `gx:Track`, and flight JSON. Review the preview to
save a recording locally. Demos can be removed and restored from the library menu.

GPX requires an explicit EGM96 elevation choice; ellipsoid elevations also require
`geoidheight`. Flight JSON uses SI units, true-north angles in degrees, and EGM96 MSL
altitude. `indicatedAirspeed`, `roll`, `pitch`, and `heading` are optional. Imports are limited
to 8 MB and 20,000 samples, with at most 20 saved imports. Segment breaks and gaps over
20 seconds are not interpolated.

## Development checks

Run from `MapLibreVision`:

```sh
swift test
sh Scripts/check-flight-overlay.sh
python3 Scripts/check-flight-documents.py /path/to/MapLibreVision.app
```

See [IndicateOverlay](MapLibreVision/IndicateOverlay/README.md) for dependency snapshot
maintenance. Scripts under `MapLibreVision/Scripts` reproduce the Innsbruck and Mach Loop
examples; Liberty retains its supplied KML.
