# MapLibre Vision

A visionOS host for the maplibre-rs renderer: the globe with worldwide terrain, drawn into
an immersive space through CompositorServices. The renderer runs as a static library behind
a small C ABI (`maplibre_visionos.h`); the Swift app hands it the head pose and frustum of
each eye and copies the rendered texture into the compositor drawable.

## Layout

- `Cargo.toml`, `src/lib.rs`: the `maplibre-visionos` static library and its C ABI. It is
  its own workspace so the wgpu patch below does not touch the main workspace.
- `vendor-wgpu.sh`: copies wgpu 22, naga, metal-rs and the Objective-C runtime crates from
  the cargo registry into `vendor/` and widens their macOS/iOS gates to visionOS.
  `.cargo/config.toml` patches crates.io with those copies for this crate alone.
- `MapLibreVision/project.yml`: the xcodegen spec of the app. `MapLibreVision/MapLibreVision/`
  holds the Swift sources and the bundled `style.json` (Dark Matter over OpenFreeMap tiles,
  AWS Terrarium elevation worldwide, globe projection).

## Building for the simulator

Requirements: Xcode with the visionOS platform, `xcodegen`, and the nightly toolchain named
in `rust-toolchain.toml` with `rust-src` (visionOS is a tier 3 target, so the standard
library is built from source).

```sh
cd apple/visionos
./vendor-wgpu.sh
cargo build --release            # target and build-std come from .cargo/config.toml
cd MapLibreVision
xcodegen generate
xcodebuild -project MapLibreVision.xcodeproj -scheme MapLibreVision \
  -destination 'generic/platform=visionOS Simulator' -derivedDataPath build build
xcrun simctl boot "Apple Vision Pro"
xcrun simctl install booted build/Build/Products/Debug-xrsimulator/MapLibreVision.app
xcrun simctl launch --console-pty booted com.sokolysystems.maplibre.vision --enter
```

`--enter` opens the immersive space at launch; without it the launcher window shows a
button. A device build needs the `aarch64-apple-visionos` target in `.cargo/config.toml`
and a signing team in the project.

## How a frame is rendered

Per frame and eye, `MapRenderer` inverts the eye's world transform into the map's local
frame (east, north, up in metres from the anchor, tilted towards the viewer so a level gaze
stays within the pitch limit) and passes it with the view's tangents and depth range to
`maplibre_visionos_render`. The Rust side turns that into an external view of the map,
runs one frame of the schedule, waits for the GPU, and returns the Metal texture of the
offscreen head, which Swift blits into the drawable's colour texture.

## Known limits

- wgpu 22 is patched rather than upgraded; a wgpu release with visionOS support (25 and
  later) removes the vendoring.
- The compositor's depth texture is not written, so reprojection has no depth to work with.
- Each eye runs a full map frame; a stereo-aware frame would run the schedule once.
